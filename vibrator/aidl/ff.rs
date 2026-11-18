/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

//! The qcom haptics input force feedback device, and the LED class fallback.

use log::{error, info};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::mem::size_of;
use std::os::fd::AsRawFd;

const INPUT_DIR: &str = "/dev/input";
const HAPTICS_NAMES: [&[u8]; 2] = [b"qcom-hv-haptics", b"qti-haptics"];
const LED_DEVICE: &str = "/sys/class/leds/vibrator";
const VISENSE_ENABLED: &str = "/sys/class/qcom-haptics/visense_enabled";

const STRONG_MAGNITUDE: i16 = 0x7fff;
const MEDIUM_MAGNITUDE: i16 = 0x5fff;
const LIGHT_MAGNITUDE: i16 = 0x3fff;
/// Primitives scaled below this are too weak to feel and are not played.
const PRIMITIVE_MIN_SCALE: f32 = 0.02;
pub const PRIMITIVE_ID_MASK: u32 = 0x8000;
const MAX_PATTERN_ID: u32 = 32767;

// linux/input.h and input-event-codes.h
const EV_FF: u16 = 0x15;
const FF_PERIODIC: u16 = 0x51;
const FF_CONSTANT: u16 = 0x52;
const FF_CUSTOM: u16 = 0x5d;
const FF_GAIN: u16 = 0x60;
const FF_CNT: usize = 0x80;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FfEnvelope {
    attack_length: u16,
    attack_level: u16,
    fade_length: u16,
    fade_level: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FfConstantEffect {
    level: i16,
    envelope: FfEnvelope,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FfPeriodicEffect {
    waveform: u16,
    period: u16,
    magnitude: i16,
    offset: i16,
    phase: u16,
    envelope: FfEnvelope,
    custom_len: u32,
    custom_data: *mut i16,
}

#[repr(C)]
#[derive(Clone, Copy)]
union FfEffectUnion {
    constant: FfConstantEffect,
    periodic: FfPeriodicEffect,
    // The kernel union also holds ramp, condition[2] and rumble; it is 32 bytes on arm64.
    _size: [u64; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FfTrigger {
    button: u16,
    interval: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FfReplay {
    length: u16,
    delay: u16,
}

#[repr(C)]
struct FfEffect {
    kind: u16,
    id: i16,
    direction: u16,
    trigger: FfTrigger,
    replay: FfReplay,
    u: FfEffectUnion,
}

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    kind: u16,
    code: u16,
    value: i32,
}

/// The FIFO descriptor the qcom haptics driver takes as FF_CUSTOM data for PWLE playback.
#[repr(C)]
struct CustomFifo {
    idx: u32,
    length: u32,
    play_rate_hz: u32,
    data: *const u8,
}

/// effect.h `struct effect_stream`, owned by the device effect library.
#[repr(C)]
pub struct EffectStream {
    pub effect_id: u32,
    pub length: u32,
    pub play_rate_hz: u32,
    pub data: *const i8,
}

extern "C" {
    fn get_effect_stream(effect_id: u32) -> *const EffectStream;
}

fn effect_stream(effect_id: u32) -> Option<&'static EffectStream> {
    // SAFETY: the effect library returns null or a pointer into its static tables.
    unsafe { get_effect_stream(effect_id).as_ref() }
}

/// Play length in ms of a waveform in the effect tables.
pub fn stream_length_ms(effect_id: u32) -> Option<i64> {
    effect_stream(effect_id)
        .filter(|s| s.play_rate_hz != 0)
        .map(|s| i64::from(s.length) * 1000 / i64::from(s.play_rate_hz) + 1)
}

// EVIOCSFF is _IOW but the kernel writes the assigned id back, so it takes a mutable pointer.
const EVIOCSFF: libc::c_ulong = nix::request_code_write!(b'E', 0x80, size_of::<FfEffect>()) as _;
nix::ioctl_write_int_bad!(eviocrmff, nix::request_code_write!(b'E', 0x81, size_of::<i32>()));
nix::ioctl_read_buf!(eviocgname, b'E', 0x06, u8);
nix::ioctl_read_buf!(eviocgbit_ff, b'E', 0x20 + EV_FF as u8, u8);

fn test_bit(bit: u16, mask: &[u8]) -> bool {
    mask[bit as usize / 8] & (1 << (bit % 8)) != 0
}

pub struct FfDevice {
    file: Option<File>,
    pub support_gain: bool,
    pub support_effects: bool,
    pub support_external_control: bool,
    pub in_external_control: bool,
    current_id: i16,
    magnitude: i16,
}

/// What a played request asks the driver for.
enum Request<'a> {
    Constant { length_ms: u32 },
    Effect { id: u32 },
    Fifo(&'a CustomFifo),
}

impl FfDevice {
    pub fn open() -> FfDevice {
        let mut device = FfDevice {
            file: None,
            support_gain: false,
            support_effects: false,
            support_external_control: false,
            in_external_control: false,
            current_id: -1,
            magnitude: STRONG_MAGNITUDE,
        };

        let entries = match fs::read_dir(INPUT_DIR) {
            Ok(entries) => entries,
            Err(e) => {
                error!("Failed to open {INPUT_DIR}: {e}");
                return device;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
                continue;
            };
            let mut name = [0u8; 32];
            // SAFETY: the buffer outlives the call and the kernel writes at most its length.
            if unsafe { eviocgname(file.as_raw_fd(), &mut name) }.is_err() {
                continue;
            }
            let name = name.split(|&b| b == 0).next().unwrap_or_default();
            if !HAPTICS_NAMES.contains(&name) {
                continue;
            }

            let mut bits = [0u8; FF_CNT / 8];
            // SAFETY: as above.
            if let Err(e) = unsafe { eviocgbit_ff(file.as_raw_fd(), &mut bits) } {
                error!("EVIOCGBIT failed on {}: {e}", path.display());
                continue;
            }
            if test_bit(FF_CONSTANT, &bits) || test_bit(FF_PERIODIC, &bits) {
                info!("{} is detected at {}", String::from_utf8_lossy(name), path.display());
                device.support_effects = test_bit(FF_CUSTOM, &bits);
                device.support_gain = test_bit(FF_GAIN, &bits);
                // Every sun haptics driver supports haptics coupled audio.
                device.support_external_control = true;
                device.file = Some(file);
                break;
            }
        }
        device
    }

    pub fn is_present(&self) -> bool {
        self.file.is_some()
    }

    pub fn on(&mut self, timeout_ms: i32) -> Result<(), ()> {
        self.play(Request::Constant { length_ms: timeout_ms.max(0) as u32 }).map(|_| ())
    }

    pub fn off(&mut self) -> Result<(), ()> {
        self.play(Request::Constant { length_ms: 0 }).map(|_| ())
    }

    /// Plays a perform() effect and returns its length in ms.
    pub fn play_effect(&mut self, effect_id: u32, magnitude: EffectMagnitude) -> Result<i64, ()> {
        if effect_id > MAX_PATTERN_ID {
            error!("effect id {effect_id} exceeds {MAX_PATTERN_ID}");
            return Err(());
        }
        self.magnitude = match magnitude {
            EffectMagnitude::Light => LIGHT_MAGNITUDE,
            EffectMagnitude::Medium => MEDIUM_MAGNITUDE,
            EffectMagnitude::Strong => STRONG_MAGNITUDE,
        };
        self.play(Request::Effect { id: effect_id })
    }

    /// Plays a compose() primitive scaled by `scale` and returns its length in ms.
    pub fn play_primitive(&mut self, primitive: u32, scale: f32) -> Result<i64, ()> {
        if primitive > MAX_PATTERN_ID {
            error!("primitive id {primitive} exceeds {MAX_PATTERN_ID}");
            return Err(());
        }
        let id = primitive | PRIMITIVE_ID_MASK;
        if scale < PRIMITIVE_MIN_SCALE {
            return Ok(stream_length_ms(id).unwrap_or(0));
        }
        self.magnitude = (scale * f32::from(STRONG_MAGNITUDE)) as i16;
        self.play(Request::Effect { id }).inspect_err(|_| error!("Failed to play primitive {id}"))
    }

    /// Plays amplitude samples through the haptics FIFO and returns the length in ms.
    pub fn play_pwle(&mut self, samples: &[u8], play_rate_hz: u32) -> Result<i64, ()> {
        let fifo = CustomFifo {
            idx: 0,
            length: samples.len() as u32,
            play_rate_hz,
            data: samples.as_ptr(),
        };
        self.play(Request::Fifo(&fifo))?;
        Ok(samples.len() as i64 * 1000 / i64::from(play_rate_hz))
    }

    pub fn set_amplitude(&mut self, amplitude: u8) -> Result<(), ()> {
        let Some(file) = &self.file else { return Ok(()) };
        let gain = i32::from(amplitude) * i32::from(STRONG_MAGNITUDE - LIGHT_MAGNITUDE) / 255
            + i32::from(LIGHT_MAGNITUDE);
        write_event(file, FF_GAIN, gain).map_err(|e| error!("write FF_GAIN failed: {e}"))?;
        self.magnitude = gain as i16;
        Ok(())
    }

    fn remove_current(&mut self) -> Result<(), ()> {
        if self.current_id < 0 {
            return Ok(());
        }
        let id = std::mem::replace(&mut self.current_id, -1);
        let file = self.file.as_ref().ok_or(())?;
        // SAFETY: EVIOCRMFF takes the effect id by value.
        unsafe { eviocrmff(file.as_raw_fd(), i32::from(id)) }
            .map(|_| ())
            .map_err(|e| error!("ioctl EVIOCRMFF failed: {e}"))
    }

    /// Uploads and starts the request, replacing the previous effect. Returns the play length
    /// in ms for effects; a constant request of length 0 only stops the current effect.
    fn play(&mut self, request: Request) -> Result<i64, ()> {
        if !self.is_present() {
            return Ok(0);
        }
        if let Request::Constant { length_ms: 0 } = request {
            return self.remove_current().map(|_| 0);
        }
        self.remove_current()?;

        // <effect id, play time s, play time ms>: the driver returns the length of patterns
        // it defines itself in the last two words.
        let mut data: [i16; 3] = [0; 3];
        let mut stream = None;
        // SAFETY: all-zero is a valid ff_effect.
        let mut effect: FfEffect = unsafe { std::mem::zeroed() };
        match request {
            Request::Constant { length_ms } => {
                effect.kind = FF_CONSTANT;
                effect.u.constant = FfConstantEffect { level: self.magnitude, envelope: Default::default() };
                effect.replay.length = length_ms.min(u16::MAX.into()) as u16;
            }
            Request::Effect { id } => {
                data[0] = id as i16;
                stream = effect_stream(id);
                let (custom_data, custom_len) = match stream {
                    Some(s) => (s as *const EffectStream as *mut i16, size_of::<EffectStream>()),
                    None => (data.as_mut_ptr(), size_of_val(&data)),
                };
                effect.kind = FF_PERIODIC;
                effect.u.periodic = FfPeriodicEffect {
                    waveform: FF_CUSTOM,
                    period: 0,
                    magnitude: self.magnitude,
                    offset: 0,
                    phase: 0,
                    envelope: Default::default(),
                    custom_len: custom_len as u32,
                    custom_data,
                };
            }
            Request::Fifo(fifo) => {
                effect.kind = FF_PERIODIC;
                effect.u.periodic = FfPeriodicEffect {
                    waveform: FF_CUSTOM,
                    period: 0,
                    magnitude: self.magnitude,
                    offset: 0,
                    phase: 0,
                    envelope: Default::default(),
                    custom_len: size_of::<CustomFifo>() as u32,
                    custom_data: fifo as *const CustomFifo as *mut i16,
                };
            }
        }
        effect.id = -1;

        let file = self.file.as_ref().unwrap();
        // SAFETY: `effect` and the custom data it points at (`data`, the static effect stream or
        // the caller's FIFO and samples) outlive the call. The driver only writes back into
        // `data`, never into effect stream or FIFO memory.
        if unsafe { libc::ioctl(file.as_raw_fd(), EVIOCSFF as _, &mut effect as *mut FfEffect) } < 0 {
            error!("ioctl EVIOCSFF failed: {}", std::io::Error::last_os_error());
            return Err(());
        }
        self.current_id = effect.id;

        let mut length_ms = i64::from(data[1]) * 1000 + i64::from(data[2]);
        if let Some(s) = stream.filter(|s| s.play_rate_hz != 0) {
            length_ms = i64::from(s.length) * 1000 / i64::from(s.play_rate_hz) + 1;
        }

        if let Err(e) = write_event(file, self.current_id as u16, 1) {
            error!("Failed to start effect {}: {e}", self.current_id);
            let _ = self.remove_current();
            return Err(());
        }
        Ok(length_ms)
    }
}

pub enum EffectMagnitude {
    Light,
    Medium,
    Strong,
}

fn write_event(mut file: &File, code: u16, value: i32) -> std::io::Result<()> {
    let event = InputEvent {
        time: libc::timeval { tv_sec: 0, tv_usec: 0 },
        kind: EV_FF,
        code,
        value,
    };
    // SAFETY: InputEvent is a plain repr(C) struct without padding.
    let bytes = unsafe {
        std::slice::from_raw_parts(&event as *const InputEvent as *const u8, size_of::<InputEvent>())
    };
    file.write_all(bytes)
}

/// The LED class vibrator of older targets. Only used when it exists.
pub struct LedVibrator;

impl LedVibrator {
    pub fn detect() -> Option<LedVibrator> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!("{LED_DEVICE}/activate"))
            .ok()
            .map(|_| LedVibrator)
    }

    fn write(node: &str, value: &str) -> std::io::Result<()> {
        // The QTI HAL wrote the terminating NUL too; the LED class parsers accept it.
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        OpenOptions::new().write(true).open(format!("{LED_DEVICE}/{node}"))?.write_all(&bytes)
    }

    pub fn on(&self, timeout_ms: i32) -> Result<(), ()> {
        Self::write("state", "1")
            .and_then(|_| Self::write("duration", &format!("{timeout_ms}\n")))
            .and_then(|_| Self::write("activate", "1"))
            .map_err(|e| error!("Failed to turn on vibrator: {e}"))
    }

    pub fn off(&self) -> Result<(), ()> {
        Self::write("activate", "0").map_err(|e| error!("Failed to turn off vibrator: {e}"))
    }
}

/// The PMIC reports V/I sense when the closed loop path through the DSP can be used.
pub fn visense_supported() -> bool {
    let mut value = String::new();
    File::open(VISENSE_ENABLED)
        .and_then(|f| f.take(3).read_to_string(&mut value))
        .map(|_| value.trim().parse::<i32>().unwrap_or(0) != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_struct_sizes() {
        assert_eq!(size_of::<FfEffect>(), 48);
        assert_eq!(size_of::<InputEvent>(), 24);
        assert_eq!(size_of::<CustomFifo>(), 24);
        assert_eq!(size_of::<EffectStream>(), 24);
        assert_eq!(std::mem::offset_of!(FfEffect, u), 16);
        assert_eq!(std::mem::offset_of!(FfPeriodicEffect, custom_len), 20);
        assert_eq!(std::mem::offset_of!(FfPeriodicEffect, custom_data), 24);
    }

    #[test]
    fn ff_bits() {
        let mut bits = [0u8; FF_CNT / 8];
        bits[(FF_CUSTOM / 8) as usize] |= 1 << (FF_CUSTOM % 8);
        assert!(test_bit(FF_CUSTOM, &bits));
        assert!(!test_bit(FF_GAIN, &bits));
    }
}
