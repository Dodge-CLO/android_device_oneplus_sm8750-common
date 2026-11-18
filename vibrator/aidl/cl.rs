/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

//! Closed loop path: haptics played by the audio DSP through PAL, with V/I sense feedback.

use crate::ol::{magnitude, EFFECTS};
use crate::scheduler::Scheduler;
use crate::{complete, illegal_argument, service_specific, unsupported};
use android_hardware_vibrator::aidl::android::hardware::vibrator::{
    Effect::Effect, EffectStrength::EffectStrength, IVibrator,
    IVibratorCallback::IVibratorCallback,
};
use binder::Strong;
use log::{debug, error, info};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

extern "C" {
    fn vib_pal_open(handle: *mut *mut c_void) -> i32;
    fn vib_pal_configure(handle: *mut c_void, effect_id: i16, strength: i16, time_ms: i32) -> i32;
    fn vib_pal_start(handle: *mut c_void) -> i32;
    fn vib_pal_stop_effect(handle: *mut c_void) -> i32;
    fn vib_pal_persistent(handle: *mut c_void) -> i32;
    fn vib_pal_update_amplitude(handle: *mut c_void, amplitude: f32) -> i32;
    fn vib_pal_close(handle: *mut c_void, started: bool) -> i32;
    fn vib_pal_dynamic_cal() -> i32;
}

/// The perform() length the DSP does not report back.
const EFFECT_LENGTH_MS: i32 = 50;
/// How long completion callbacks wait for the DSP waveform event.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);
/// The stream stays open this long after off() so a following play does not reopen it.
const IDLE_CLOSE_DELAY: Duration = Duration::from_secs(10);
const DYNAMIC_CAL_PERIOD: Duration = Duration::from_secs(30 * 60);

const STATE_PLAYING: u8 = 0;
const STATE_IDLE: u8 = 2;

// Written by PAL callbacks, which have no context and can arrive on the Binder thread while it
// waits in a PAL call. They must not take the stream lock.
static HAPTICS_STATE: AtomicU8 = AtomicU8::new(STATE_IDLE);
static PENDING: Mutex<Vec<(u64, Strong<dyn IVibratorCallback>)>> = Mutex::new(Vec::new());
static NEXT_PENDING_ID: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
extern "C" fn vib_pal_on_event(event: u32) {
    debug!("event received from DSP {event}");
    HAPTICS_STATE.store(event as u8, Ordering::SeqCst);
    let callbacks = std::mem::take(&mut *PENDING.lock().unwrap());
    for (_, callback) in &callbacks {
        complete(callback);
    }
}

struct Stream(*mut c_void);

// SAFETY: the PAL stream handle is an opaque token that PAL accepts from any thread.
unsafe impl Send for Stream {}

#[derive(Default)]
struct StreamState {
    stream: Option<Stream>,
    /// pal_stream_start succeeded on the open stream.
    started: bool,
    active: bool,
    /// Bumped by every play and off, so a delayed close can tell it is stale.
    generation: u64,
}

pub struct VibratorCl {
    state: Arc<Mutex<StreamState>>,
    /// Set once the DSP refuses to open, configure or start a stream; callers then play open
    /// loop instead of dropping the vibration.
    failed: AtomicBool,
    scheduler: Arc<Scheduler>,
}

fn close_stream(state: &mut StreamState) {
    if let Some(stream) = state.stream.take() {
        // SAFETY: the handle came from vib_pal_open and is not used after this.
        unsafe { vib_pal_close(stream.0, state.started) };
    }
    state.started = false;
    HAPTICS_STATE.store(STATE_IDLE, Ordering::SeqCst);
}

impl VibratorCl {
    pub fn new(scheduler: Arc<Scheduler>) -> VibratorCl {
        thread::Builder::new()
            .name("vibrator-cal".to_owned())
            .spawn(|| loop {
                info!("set dynamic calib param");
                // SAFETY: plain call into PAL.
                if unsafe { vib_pal_dynamic_cal() } != 0 {
                    error!("Dynamic calibration failed");
                }
                thread::sleep(DYNAMIC_CAL_PERIOD);
            })
            .expect("Failed to start the calibration thread");
        VibratorCl { state: Default::default(), failed: AtomicBool::new(false), scheduler }
    }

    /// Completes `callback` at the next DSP event, or after EVENT_TIMEOUT without one.
    fn complete_on_event(&self, callback: &Strong<dyn IVibratorCallback>) {
        let id = NEXT_PENDING_ID.fetch_add(1, Ordering::Relaxed);
        PENDING.lock().unwrap().push((id, callback.clone()));
        self.scheduler.after(EVENT_TIMEOUT, move || {
            let mut pending = PENDING.lock().unwrap();
            if let Some(i) = pending.iter().position(|(p, _)| *p == id) {
                let (_, callback) = pending.swap_remove(i);
                drop(pending);
                complete(&callback);
            }
        });
    }

    fn play(&self, effect_id: i16, strength: i16, time_ms: i32) -> binder::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.active {
            error!("Haptics is already active, skipping this request");
            return Err(unsupported());
        }
        state.generation += 1;

        if state.stream.is_none() {
            let mut handle = std::ptr::null_mut();
            // SAFETY: `handle` is a valid out pointer.
            let ret = unsafe { vib_pal_open(&mut handle) };
            if ret != 0 || handle.is_null() {
                error!("Failed to open the haptics stream: {ret}");
                self.failed.store(true, Ordering::Relaxed);
                return Err(service_specific());
            }
            state.stream = Some(Stream(handle));
        }
        let handle = state.stream.as_ref().unwrap().0;

        // SAFETY: `handle` is the open stream, kept alive by the lock.
        let ret = unsafe { vib_pal_configure(handle, effect_id, strength, time_ms) };
        if ret != 0 {
            error!("Failed to configure haptics: {ret}");
            close_stream(&mut state);
            self.failed.store(true, Ordering::Relaxed);
            return Err(service_specific());
        }
        // Set before starting: a DSP event may arrive while pal_stream_start runs.
        HAPTICS_STATE.store(STATE_PLAYING, Ordering::SeqCst);
        // SAFETY: as above.
        let ret = unsafe { vib_pal_start(handle) };
        if ret != 0 {
            error!("Failed to start haptics: {ret}");
            close_stream(&mut state);
            self.failed.store(true, Ordering::Relaxed);
            return Err(service_specific());
        }
        state.started = true;
        state.active = true;
        Ok(())
    }

    /// False once the DSP has refused a stream.
    pub fn usable(&self) -> bool {
        !self.failed.load(Ordering::Relaxed)
    }

    pub fn capabilities(&self) -> i32 {
        IVibrator::CAP_ON_CALLBACK
            | IVibrator::CAP_PERFORM_CALLBACK
            | IVibrator::CAP_AMPLITUDE_CONTROL
            | IVibrator::CAP_EXTERNAL_CONTROL
            | IVibrator::CAP_COMPOSE_EFFECTS
    }

    pub fn off(&self) -> binder::Result<()> {
        let mut state = self.state.lock().unwrap();
        let mut ret = 0;
        if let Some(stream) = &state.stream {
            if HAPTICS_STATE.load(Ordering::SeqCst) == STATE_PLAYING {
                // SAFETY: the open stream, kept alive by the lock.
                if unsafe { vib_pal_stop_effect(stream.0) } != 0 {
                    error!("Failed to stop the current effect");
                }
                HAPTICS_STATE.store(STATE_IDLE, Ordering::SeqCst);
            }
            // SAFETY: as above.
            ret = unsafe { vib_pal_persistent(stream.0) };
        }
        if ret != 0 {
            close_stream(&mut state);
        }
        state.active = false;
        state.generation += 1;
        let generation = state.generation;
        drop(state);

        let state = self.state.clone();
        self.scheduler.after(IDLE_CLOSE_DELAY, move || {
            let mut state = state.lock().unwrap();
            if !state.active && state.generation == generation {
                close_stream(&mut state);
            }
        });

        if ret != 0 {
            return Err(service_specific());
        }
        Ok(())
    }

    pub fn on(&self, timeout_ms: i32, callback: &Strong<dyn IVibratorCallback>) -> binder::Result<()> {
        debug!("on for {timeout_ms} ms");
        self.play(-1, -1, timeout_ms)?;
        self.complete_on_event(callback);
        Ok(())
    }

    pub fn perform(
        &self,
        effect: Effect,
        strength: EffectStrength,
        callback: &Strong<dyn IVibratorCallback>,
    ) -> binder::Result<i32> {
        if self.state.lock().unwrap().active {
            return Err(unsupported());
        }
        if !EFFECTS.contains(&effect) || magnitude(strength).is_none() {
            return Err(unsupported());
        }
        self.play(effect.0 as i16, i16::from(strength.0), -1)?;
        self.complete_on_event(callback);
        Ok(EFFECT_LENGTH_MS)
    }

    pub fn set_amplitude(&self, amplitude: f32) -> binder::Result<()> {
        if amplitude <= 0.0 || amplitude > 1.0 {
            return Err(illegal_argument());
        }
        let state = self.state.lock().unwrap();
        let stream = state.stream.as_ref().ok_or_else(service_specific)?;
        // SAFETY: the open stream, kept alive by the lock.
        if unsafe { vib_pal_update_amplitude(stream.0, amplitude) } != 0 {
            error!("Failed to update the haptics amplitude");
            return Err(service_specific());
        }
        Ok(())
    }
}
