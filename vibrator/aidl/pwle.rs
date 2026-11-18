/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

//! Amplitude envelope for composePwle(), played by the haptics FIFO.

pub const PLAY_RATE_HZ: u32 = 8000;
pub const MAX_DURATION_MS: i32 = 16383;
const MAX_SAMPLES: usize = MAX_DURATION_MS as usize * PLAY_RATE_HZ as usize / 1000;

pub enum Segment {
    Active { start_amplitude: f32, end_amplitude: f32, duration_ms: i32 },
    Braking { duration_ms: i32 },
}

fn samples_for(duration_ms: i32) -> usize {
    duration_ms.max(0) as usize * PLAY_RATE_HZ as usize / 1000
}

/// Linear ramps for active segments and silence for braking. Frequencies are not used: the
/// FIFO plays at the LRA resonance. None when the segments are invalid or too long.
pub fn envelope(segments: &[Segment]) -> Option<Vec<u8>> {
    let mut total_ms: i64 = 0;
    for segment in segments {
        match *segment {
            Segment::Active { duration_ms, .. } if duration_ms <= 0 => return None,
            Segment::Active { duration_ms, .. } | Segment::Braking { duration_ms } => {
                total_ms += i64::from(duration_ms.max(0))
            }
        }
    }
    let total = usize::try_from(total_ms).ok()? * PLAY_RATE_HZ as usize / 1000;
    if total == 0 || total > MAX_SAMPLES {
        return None;
    }

    let mut samples = Vec::with_capacity(total);
    for segment in segments {
        match *segment {
            Segment::Active { start_amplitude, end_amplitude, duration_ms } => {
                let n = samples_for(duration_ms);
                samples.extend((0..n).map(|i| {
                    let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 1.0 };
                    let amplitude = start_amplitude + t * (end_amplitude - start_amplitude);
                    (amplitude.clamp(0.0, 1.0) * 255.0) as u8
                }));
            }
            Segment::Braking { duration_ms } => {
                samples.resize(samples.len() + samples_for(duration_ms), 0)
            }
        }
    }
    samples.truncate(total);
    Some(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_and_brakes() {
        let samples = envelope(&[
            Segment::Active { start_amplitude: 0.0, end_amplitude: 1.0, duration_ms: 1 },
            Segment::Braking { duration_ms: 1 },
        ])
        .unwrap();
        assert_eq!(samples.len(), 16);
        assert_eq!(samples[0], 0);
        assert_eq!(samples[7], 255);
        assert!(samples[8..].iter().all(|&s| s == 0));
    }

    #[test]
    fn clamps_amplitude() {
        let samples = envelope(&[Segment::Active {
            start_amplitude: 2.0,
            end_amplitude: -1.0,
            duration_ms: 1,
        }])
        .unwrap();
        assert_eq!(samples[0], 255);
        assert_eq!(samples[7], 0);
    }

    #[test]
    fn rejects_invalid_segments() {
        assert!(envelope(&[]).is_none());
        assert!(envelope(&[Segment::Active {
            start_amplitude: 1.0,
            end_amplitude: 1.0,
            duration_ms: 0
        }])
        .is_none());
        assert!(envelope(&[Segment::Braking { duration_ms: 0 }]).is_none());
        assert!(envelope(&[Segment::Braking { duration_ms: MAX_DURATION_MS + 1 }]).is_none());
        assert!(envelope(&[Segment::Braking { duration_ms: MAX_DURATION_MS }]).is_some());
    }
}
