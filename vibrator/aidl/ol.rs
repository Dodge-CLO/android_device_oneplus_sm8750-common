/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

//! Open loop path: the PMIC haptics input device (or the LED class vibrator).

use crate::ff::{self, EffectMagnitude, FfDevice, LedVibrator, PRIMITIVE_ID_MASK};
use crate::pwle;
use crate::scheduler::Scheduler;
use crate::{complete, illegal_argument, service_specific, unsupported};
use android_hardware_vibrator::aidl::android::hardware::vibrator::{
    Braking::Braking, CompositeEffect::CompositeEffect, CompositePrimitive::CompositePrimitive,
    Effect::Effect, EffectStrength::EffectStrength, IVibrator,
    IVibratorCallback::IVibratorCallback, PrimitivePwle::PrimitivePwle,
};
use binder::Strong;
use log::{debug, error};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const COMPOSE_DELAY_MAX_MS: i32 = 1000;
pub const COMPOSE_SIZE_MAX: i32 = 256;
const PWLE_COMPOSITION_SIZE_MAX: i32 = 127;
/// dodge LRA: qcom,lra-period-us = 7692.
const DEFAULT_LRA_FREQ_HZ: f32 = 130.0;

pub const EFFECTS: [Effect; 6] = [
    Effect::CLICK,
    Effect::DOUBLE_CLICK,
    Effect::TICK,
    Effect::THUD,
    Effect::POP,
    Effect::HEAVY_CLICK,
];

pub const PRIMITIVES: [CompositePrimitive; 9] = [
    CompositePrimitive::NOOP,
    CompositePrimitive::CLICK,
    CompositePrimitive::THUD,
    CompositePrimitive::SPIN,
    CompositePrimitive::QUICK_RISE,
    CompositePrimitive::SLOW_RISE,
    CompositePrimitive::QUICK_FALL,
    CompositePrimitive::LIGHT_TICK,
    CompositePrimitive::LOW_TICK,
];

pub fn magnitude(strength: EffectStrength) -> Option<EffectMagnitude> {
    match strength {
        EffectStrength::LIGHT => Some(EffectMagnitude::Light),
        EffectStrength::MEDIUM => Some(EffectMagnitude::Medium),
        EffectStrength::STRONG => Some(EffectMagnitude::Strong),
        _ => None,
    }
}

/// A CompositeEffect, which the AIDL Rust backend does not make Clone.
struct Step {
    delay_ms: i32,
    primitive: CompositePrimitive,
    scale: f32,
}

struct Composition {
    steps: Vec<Step>,
    callback: Strong<dyn IVibratorCallback>,
}

#[derive(Default)]
struct ComposeState {
    queued: Option<Composition>,
    /// A composition is queued or playing.
    active: bool,
    /// off() asked the playing composition to stop.
    stop: bool,
}

#[derive(Default)]
struct Composer {
    state: Mutex<ComposeState>,
    changed: Condvar,
}

impl Composer {
    /// Waits for `duration` and returns true if the composition was stopped meanwhile.
    fn wait_stopped(&self, duration: Duration) -> bool {
        let state = self.state.lock().unwrap();
        let (state, _) = self.changed.wait_timeout_while(state, duration, |s| !s.stop).unwrap();
        state.stop
    }

    fn run(self: Arc<Self>, ff: Arc<Mutex<FfDevice>>) {
        loop {
            let composition = {
                let mut state = self.state.lock().unwrap();
                loop {
                    if let Some(c) = state.queued.take() {
                        break c;
                    }
                    state = self.changed.wait(state).unwrap();
                }
            };

            self.play(&ff, &composition.steps);
            complete(&composition.callback);

            let mut state = self.state.lock().unwrap();
            state.active = false;
            state.stop = false;
            self.changed.notify_all();
        }
    }

    fn play(&self, ff: &Mutex<FfDevice>, steps: &[Step]) {
        for e in steps {
            if e.delay_ms > 0 && self.wait_stopped(Duration::from_millis(e.delay_ms as u64)) {
                return;
            }
            if e.primitive == CompositePrimitive::NOOP {
                continue;
            }
            let length_ms = ff.lock().unwrap().play_primitive(e.primitive.0 as u32, e.scale);
            if self.wait_stopped(Duration::from_millis(length_ms.unwrap_or(0).max(0) as u64)) {
                // off() can run before a short primitive is triggered here on a busy system.
                // The driver wants an explicit off after every play, so stop it again.
                let _ = ff.lock().unwrap().off();
                return;
            }
        }
    }
}

pub struct VibratorOl {
    ff: Arc<Mutex<FfDevice>>,
    led: Option<LedVibrator>,
    /// None without FF_CUSTOM support, which compositions need.
    composer: Option<Arc<Composer>>,
    scheduler: Arc<Scheduler>,
}

impl VibratorOl {
    pub fn new(scheduler: Arc<Scheduler>) -> VibratorOl {
        let ff = Arc::new(Mutex::new(FfDevice::open()));
        let composer = ff.lock().unwrap().support_effects.then(|| {
            let composer = Arc::new(Composer::default());
            let (c, ff) = (composer.clone(), ff.clone());
            thread::Builder::new()
                .name("vibrator-compose".to_owned())
                .spawn(move || c.run(ff))
                .expect("Failed to start the compose thread");
            composer
        });
        VibratorOl { ff, led: LedVibrator::detect(), composer, scheduler }
    }

    fn complete_after(&self, length_ms: i64, callback: &Strong<dyn IVibratorCallback>) {
        let callback = callback.clone();
        self.scheduler
            .after(Duration::from_millis(length_ms.max(0) as u64), move || complete(&callback));
    }

    pub fn capabilities(&self) -> i32 {
        let mut caps = IVibrator::CAP_ON_CALLBACK;
        if self.led.is_some() {
            return caps | IVibrator::CAP_PERFORM_CALLBACK;
        }
        let ff = self.ff.lock().unwrap();
        if ff.support_gain {
            caps |= IVibrator::CAP_AMPLITUDE_CONTROL;
        }
        if ff.support_effects {
            caps |= IVibrator::CAP_PERFORM_CALLBACK | IVibrator::CAP_COMPOSE_EFFECTS;
        }
        if ff.support_external_control {
            caps |= IVibrator::CAP_EXTERNAL_CONTROL;
        }
        caps | IVibrator::CAP_COMPOSE_PWLE_EFFECTS | IVibrator::CAP_GET_RESONANT_FREQUENCY
    }

    pub fn off(&self) -> binder::Result<()> {
        debug!("off");
        match &self.led {
            Some(led) => led.off(),
            None => self.ff.lock().unwrap().off(),
        }
        .map_err(|_| service_specific())?;

        if let Some(composer) = &self.composer {
            let mut state = composer.state.lock().unwrap();
            if state.active {
                state.stop = true;
                composer.changed.notify_all();
            }
        }
        Ok(())
    }

    pub fn on(&self, timeout_ms: i32, callback: &Strong<dyn IVibratorCallback>) -> binder::Result<()> {
        debug!("on for {timeout_ms} ms");
        match &self.led {
            Some(led) => led.on(timeout_ms),
            None => self.ff.lock().unwrap().on(timeout_ms),
        }
        .map_err(|_| service_specific())?;
        self.complete_after(i64::from(timeout_ms), callback);
        Ok(())
    }

    pub fn perform(
        &self,
        effect: Effect,
        strength: EffectStrength,
        callback: &Strong<dyn IVibratorCallback>,
    ) -> binder::Result<i32> {
        if self.led.is_some() || !EFFECTS.contains(&effect) {
            return Err(unsupported());
        }
        let magnitude = magnitude(strength).ok_or_else(unsupported)?;
        debug!("perform effect {effect:?}");

        // Effect tables are keyed by the AOSP effect ids, not the ColorOS waveform ids.
        let length_ms = self
            .ff
            .lock()
            .unwrap()
            .play_effect(effect.0 as u32, magnitude)
            .map_err(|_| service_specific())?;
        self.complete_after(length_ms, callback);
        Ok(length_ms as i32)
    }

    pub fn supported_effects(&self) -> Vec<Effect> {
        if self.led.is_some() {
            return vec![];
        }
        EFFECTS.to_vec()
    }

    pub fn set_amplitude(&self, amplitude: f32) -> binder::Result<()> {
        if self.led.is_some() {
            return Err(unsupported());
        }
        if amplitude <= 0.0 || amplitude > 1.0 {
            return Err(illegal_argument());
        }
        let mut ff = self.ff.lock().unwrap();
        if ff.in_external_control {
            return Err(unsupported());
        }
        ff.set_amplitude((amplitude * 255.0) as u8).map_err(|_| service_specific())
    }

    pub fn set_external_control(&self, enabled: bool) -> binder::Result<()> {
        let mut ff = self.ff.lock().unwrap();
        if self.led.is_some() || !ff.support_external_control {
            return Err(unsupported());
        }
        ff.in_external_control = enabled;
        Ok(())
    }

    pub fn primitive_duration(&self, primitive: CompositePrimitive) -> i32 {
        ff::stream_length_ms(primitive.0 as u32 | PRIMITIVE_ID_MASK).unwrap_or(0) as i32
    }

    pub fn compose(
        &self,
        composite: &[CompositeEffect],
        callback: &Strong<dyn IVibratorCallback>,
    ) -> binder::Result<()> {
        let composer = self.composer.as_ref().ok_or_else(unsupported)?;
        if composite.len() > COMPOSE_SIZE_MAX as usize {
            return Err(illegal_argument());
        }
        let mut total_ms: i64 = 0;
        for e in composite {
            if e.delayMs < 0 || e.delayMs > COMPOSE_DELAY_MAX_MS || !(0.0..=1.0).contains(&e.scale) {
                return Err(illegal_argument());
            }
            if !PRIMITIVES.contains(&e.primitive) {
                return Err(unsupported());
            }
            total_ms += i64::from(self.primitive_duration(e.primitive)) + i64::from(e.delayMs);
        }

        let mut state = composer.state.lock().unwrap();
        if state.active {
            debug!("Stopping the previous composition");
            drop(state);
            self.off()?;
            // Twice the play length of the new composition, as the QTI HAL waited.
            let deadline = Instant::now() + Duration::from_millis(((total_ms + 10) * 2) as u64);
            state = composer.state.lock().unwrap();
            while state.active {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    error!("Timed out waiting for the previous composition to stop");
                    return Err(service_specific());
                }
                state = composer.changed.wait_timeout(state, left).unwrap().0;
            }
        }

        let steps = composite
            .iter()
            .map(|e| Step { delay_ms: e.delayMs, primitive: e.primitive, scale: e.scale })
            .collect();
        state.queued = Some(Composition { steps, callback: callback.clone() });
        state.active = true;
        state.stop = false;
        composer.changed.notify_all();
        Ok(())
    }

    pub fn resonant_frequency(&self) -> f32 {
        rustutils::android::system_properties::read("ro.vendor.vibrator.lra_freq_hz")
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|&hz| hz > 0.0)
            .unwrap_or(DEFAULT_LRA_FREQ_HZ)
    }

    pub fn pwle_primitive_duration_max(&self) -> i32 {
        pwle::MAX_DURATION_MS
    }

    pub fn pwle_composition_size_max(&self) -> i32 {
        PWLE_COMPOSITION_SIZE_MAX
    }

    pub fn supported_braking(&self) -> Vec<Braking> {
        vec![Braking::NONE]
    }

    pub fn compose_pwle(
        &self,
        composite: &[PrimitivePwle],
        callback: &Strong<dyn IVibratorCallback>,
    ) -> binder::Result<()> {
        if composite.is_empty() {
            return Err(illegal_argument());
        }
        let mut ff = self.ff.lock().unwrap();
        if !ff.is_present() {
            return Err(unsupported());
        }
        let segments: Vec<pwle::Segment> = composite
            .iter()
            .map(|p| match p {
                PrimitivePwle::Active(a) => pwle::Segment::Active {
                    start_amplitude: a.startAmplitude,
                    end_amplitude: a.endAmplitude,
                    duration_ms: a.duration,
                },
                PrimitivePwle::Braking(b) => pwle::Segment::Braking { duration_ms: b.duration },
            })
            .collect();
        let samples = pwle::envelope(&segments).ok_or_else(illegal_argument)?;
        let length_ms =
            ff.play_pwle(&samples, pwle::PLAY_RATE_HZ).map_err(|_| service_specific())?;
        drop(ff);
        self.complete_after(length_ms, callback);
        Ok(())
    }
}
