/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

//! Vibrator HAL for the sm8750 PMIC haptics: open loop through the input force feedback device,
//! closed loop through the audio DSP when the PMIC has V/I sense.

// The test build has no caller for main() and the service type.
#![cfg_attr(test, allow(dead_code))]

mod cl;
mod ff;
mod ol;
mod policy;
mod pwle;
mod scheduler;

use android_hardware_vibrator::aidl::android::hardware::vibrator::{
    Braking::Braking,
    CompositeEffect::CompositeEffect,
    CompositePrimitive::CompositePrimitive,
    Effect::Effect,
    EffectStrength::EffectStrength,
    IVibrator::{BnVibrator, IVibrator},
    IVibratorCallback::IVibratorCallback,
    PrimitivePwle::PrimitivePwle,
};
use binder::{BinderFeatures, ExceptionCode, Interface, Status, Strong};
use cl::VibratorCl;
use log::{error, info, warn, LevelFilter};
use ol::VibratorOl;
use policy::Policy;
use scheduler::Scheduler;
use std::sync::{Arc, Mutex};

const LOG_TAG: &str = "vendor.qti.vibrator";
const POLICY_FILE: &str = "/vendor/etc/HapticsPolicy.xml";

fn unsupported() -> Status {
    Status::new_exception(ExceptionCode::UNSUPPORTED_OPERATION, None)
}

fn illegal_argument() -> Status {
    Status::new_exception(ExceptionCode::ILLEGAL_ARGUMENT, None)
}

fn service_specific() -> Status {
    Status::new_exception(ExceptionCode::SERVICE_SPECIFIC, None)
}

fn complete(callback: &Strong<dyn IVibratorCallback>) {
    if let Err(e) = callback.onComplete() {
        error!("Failed to call onComplete: {e}");
    }
}

struct Vibrator {
    ol: VibratorOl,
    cl: Option<VibratorCl>,
    /// Without a policy every request plays open loop.
    policy: Option<Policy>,
    /// Whether the last on() or perform() went to the closed loop, which then also takes the
    /// following off() and setAmplitude().
    cl_selected: Mutex<bool>,
}

impl Vibrator {
    fn new() -> Vibrator {
        let scheduler = Arc::new(Scheduler::new("vibrator-timer"));
        let ol = VibratorOl::new(scheduler.clone());
        let (cl, policy) = if ff::visense_supported() {
            let policy = match std::fs::read_to_string(POLICY_FILE) {
                Ok(xml) => Some(Policy::parse(&xml)),
                Err(e) => {
                    error!("Failed to read {POLICY_FILE}: {e}");
                    None
                }
            };
            (Some(VibratorCl::new(scheduler)), policy)
        } else {
            (None, None)
        };
        if policy.as_ref().is_some_and(|p| p.cl_compose) {
            warn!("The closed loop has no compose support, compositions play open loop");
        }
        info!("closed loop {}, policy {:?}", cl.is_some(), policy);
        Vibrator { ol, cl, policy, cl_selected: Mutex::new(false) }
    }

    /// The closed loop vibrator when `use_cl` and it exists.
    fn select(&self, use_cl: bool) -> Option<&VibratorCl> {
        self.cl.as_ref().filter(|_| use_cl && self.policy.is_some())
    }

    /// The closed loop vibrator for a new play, unless the DSP has already refused one.
    fn select_for_play(&self, use_cl: bool) -> Option<&VibratorCl> {
        self.select(use_cl).filter(|cl| cl.usable())
    }

    fn log_fallback(&self) {
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| warn!("The closed loop cannot play, using the open loop from now on"));
    }
}

impl Interface for Vibrator {}

#[allow(non_snake_case)]
impl IVibrator for Vibrator {
    fn getCapabilities(&self) -> binder::Result<i32> {
        Ok(self.ol.capabilities() | self.cl.as_ref().map_or(0, VibratorCl::capabilities))
    }

    fn off(&self) -> binder::Result<()> {
        let selected = self.cl_selected.lock().unwrap();
        match self.select(*selected) {
            Some(cl) => cl.off(),
            None => self.ol.off(),
        }
    }

    fn on(&self, timeoutMs: i32, callback: &Strong<dyn IVibratorCallback>) -> binder::Result<()> {
        if timeoutMs <= 0 {
            return Err(illegal_argument());
        }
        let mut selected = self.cl_selected.lock().unwrap();
        let use_cl = self.policy.as_ref().is_some_and(|p| p.on_uses_cl(timeoutMs));
        if let Some(cl) = self.select_for_play(use_cl) {
            *selected = true;
            match cl.on(timeoutMs, callback) {
                Err(_) if !cl.usable() => self.log_fallback(),
                result => return result,
            }
        }
        *selected = false;
        self.ol.on(timeoutMs, callback)
    }

    fn perform(
        &self,
        effect: Effect,
        strength: EffectStrength,
        callback: &Strong<dyn IVibratorCallback>,
    ) -> binder::Result<i32> {
        let mut selected = self.cl_selected.lock().unwrap();
        let use_cl = self.policy.as_ref().is_some_and(|p| p.perform_uses_cl(effect.0));
        if let Some(cl) = self.select_for_play(use_cl) {
            *selected = true;
            match cl.perform(effect, strength, callback) {
                Err(_) if !cl.usable() => self.log_fallback(),
                result => return result,
            }
        }
        *selected = false;
        self.ol.perform(effect, strength, callback)
    }

    fn getSupportedEffects(&self) -> binder::Result<Vec<Effect>> {
        let mut effects = self.ol.supported_effects();
        if self.cl.is_some() {
            for effect in ol::EFFECTS {
                if !effects.contains(&effect) {
                    effects.push(effect);
                }
            }
        }
        Ok(effects)
    }

    fn setAmplitude(&self, amplitude: f32) -> binder::Result<()> {
        let selected = self.cl_selected.lock().unwrap();
        match self.select(*selected) {
            Some(cl) => cl.set_amplitude(amplitude),
            None => self.ol.set_amplitude(amplitude),
        }
    }

    fn setExternalControl(&self, enabled: bool) -> binder::Result<()> {
        // The closed loop has nothing to switch: it plays through the DSP either way.
        self.ol.set_external_control(enabled)
    }

    fn getCompositionDelayMax(&self) -> binder::Result<i32> {
        Ok(ol::COMPOSE_DELAY_MAX_MS)
    }

    fn getCompositionSizeMax(&self) -> binder::Result<i32> {
        Ok(ol::COMPOSE_SIZE_MAX)
    }

    fn getSupportedPrimitives(&self) -> binder::Result<Vec<CompositePrimitive>> {
        Ok(ol::PRIMITIVES.to_vec())
    }

    fn getPrimitiveDuration(&self, primitive: CompositePrimitive) -> binder::Result<i32> {
        Ok(self.ol.primitive_duration(primitive))
    }

    fn compose(
        &self,
        composite: &[CompositeEffect],
        callback: &Strong<dyn IVibratorCallback>,
    ) -> binder::Result<()> {
        *self.cl_selected.lock().unwrap() = false;
        self.ol.compose(composite, callback)
    }

    fn getSupportedAlwaysOnEffects(&self) -> binder::Result<Vec<Effect>> {
        Err(unsupported())
    }

    fn alwaysOnEnable(&self, _: i32, _: Effect, _: EffectStrength) -> binder::Result<()> {
        Err(unsupported())
    }

    fn alwaysOnDisable(&self, _: i32) -> binder::Result<()> {
        Err(unsupported())
    }

    fn getResonantFrequency(&self) -> binder::Result<f32> {
        Ok(self.ol.resonant_frequency())
    }

    fn getQFactor(&self) -> binder::Result<f32> {
        Err(unsupported())
    }

    fn getFrequencyResolution(&self) -> binder::Result<f32> {
        Err(unsupported())
    }

    fn getFrequencyMinimum(&self) -> binder::Result<f32> {
        Err(unsupported())
    }

    fn getBandwidthAmplitudeMap(&self) -> binder::Result<Vec<f32>> {
        Err(unsupported())
    }

    fn getPwlePrimitiveDurationMax(&self) -> binder::Result<i32> {
        Ok(self.ol.pwle_primitive_duration_max())
    }

    fn getPwleCompositionSizeMax(&self) -> binder::Result<i32> {
        Ok(self.ol.pwle_composition_size_max())
    }

    fn getSupportedBraking(&self) -> binder::Result<Vec<Braking>> {
        Ok(self.ol.supported_braking())
    }

    fn composePwle(
        &self,
        composite: &[PrimitivePwle],
        callback: &Strong<dyn IVibratorCallback>,
    ) -> binder::Result<()> {
        *self.cl_selected.lock().unwrap() = false;
        self.ol.compose_pwle(composite, callback)
    }
}

fn main() {
    logger::init(
        logger::Config::default().with_tag_on_device(LOG_TAG).with_max_level(LevelFilter::Info),
    );

    binder::ProcessState::set_thread_pool_max_thread_count(0);

    let service = BnVibrator::new_binder(Vibrator::new(), BinderFeatures::default());
    let instance = format!("{}/default", <Vibrator as IVibrator>::get_descriptor());
    binder::add_service(&instance, service.as_binder()).expect("Failed to register service");

    binder::ProcessState::join_thread_pool();
}
