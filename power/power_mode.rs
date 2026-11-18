/*
 * SPDX-FileCopyrightText: 2026 The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

//! sm8750 power HAL extension.
//! Battery Saver (Mode::LOW_POWER) caps CPU and GPU max frequency, and game loading screens
//! (Mode::GAME_LOADING) get a short CPU boost.
//!
//! thermal-engine limits the CPUs to the stock boost ceilings. Like stock, the stricter stock
//! limits listed in thermal-engine.conf apply while the user is not touching the screen or
//! launching an app.

use android_hardware_power::aidl::android::hardware::power::{Boost::Boost, Mode::Mode};
use log::{info, warn};
use std::collections::HashMap;
use std::fs;
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// Per cluster values in kHz: big, prime.
type FreqPair = (i32, i32);

// Battery Saver CPU limits, stock max is 3532800/4320000.
const LOW_POWER_MAX: FreqPair = (2227200, 2649600);
// GPU max power level for Battery Saver. Higher index means lower frequency; level 6 is 660 MHz
// (top level is 1100 MHz).
const LOW_POWER_GPU_LEVEL: i32 = 6;
// Game loading screens (GameManager Mode::GAME_LOADING): raise the CPU floors so level loads
// finish faster. Capped by GAME_LOADING_MAX in case the framework never clears the mode.
const GAME_LOADING_MIN: FreqPair = (2227200, 2649600);
const GAME_LOADING_MAX: Duration = Duration::from_millis(10000);
const GPU_MAX_PWRLEVEL: &str = "/sys/class/kgsl/kgsl-3d0/max_pwrlevel";
const CPUFREQ: &str = "/sys/devices/system/cpu/cpufreq";

const THERMAL_ENGINE_CONFIG: &str = "/vendor/etc/thermal-engine.conf";
const IDLE_LIMITS_TAG: &str = "# idle_limits";
// Keep the boost ceilings this long after the last input event, so scrolls, flings and the
// animations that follow a tap run unlimited.
const INTERACTION_HOLD: Duration = Duration::from_millis(2000);
const LAUNCH_MAX: Duration = Duration::from_millis(5000);
// thermal-engine samples the shell temperature once a second.
const POLL_INTERVAL: Duration = Duration::from_millis(1000);

fn cluster_value(pair: FreqPair, cluster: usize) -> i32 {
    match cluster {
        0 => pair.0,
        1 => pair.1,
        _ => 0,
    }
}

fn read_int(path: &str) -> Option<i32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn write(path: &str, value: i32) -> bool {
    match fs::write(path, value.to_string()) {
        Ok(()) => true,
        Err(e) => {
            warn!("Cannot write {value} to {path}: {e}");
            false
        }
    }
}

/// cpufreq policy directories, ordered by their first CPU.
fn cpufreq_policies() -> Vec<String> {
    let mut policies: Vec<(u32, String)> = fs::read_dir(CPUFREQ)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let cpu = name.strip_prefix("policy")?.parse().ok()?;
            Some((cpu, format!("{CPUFREQ}/{name}")))
        })
        .collect();
    policies.sort();
    policies.into_iter().map(|(_, path)| path).collect()
}

struct Policy {
    min_freq: String,
    max_freq: String,
    bottom: i32,
    top: i32,
    applied_min: i32,
    applied_max: i32,
}

#[derive(Default)]
struct Limits {
    policies: Vec<Policy>,
    /// Zero means no idle limit for that cluster.
    idle_max: FreqPair,
    low_power: bool,
    /// Set while a loading boost runs.
    loading_until: Option<Instant>,
}

impl Limits {
    fn apply(&mut self) {
        let loading = self.loading_until.is_some();
        for (c, policy) in self.policies.iter_mut().enumerate() {
            let mut max = policy.top;
            if self.low_power {
                max = max.min(cluster_value(LOW_POWER_MAX, c));
            }
            // A loading screen also lifts the idle limit.
            let idle_max = cluster_value(self.idle_max, c);
            if !loading && idle_max > 0 {
                max = max.min(idle_max);
            }
            if max > 0 && max != policy.applied_max && write(&policy.max_freq, max) {
                policy.applied_max = max;
            }

            let mut min = policy.bottom;
            if loading {
                min = min.max(cluster_value(GAME_LOADING_MIN, c));
            }
            if min > 0 && min != policy.applied_min && write(&policy.min_freq, min) {
                policy.applied_min = min;
            }
        }
    }
}

/// The perf HAL on sun rejects CPU frequency and GPU power level locks, so the limits are
/// written to the kernel nodes directly. scaling_min_freq and scaling_max_freq are frequency QoS
/// requests of their own, and kgsl applies thermal limits separately from max_pwrlevel, so
/// thermal-engine still wins. Reading the cpufreq nodes returns the combined policy limits, so
/// releasing a limit writes the cpuinfo bounds, the kernel defaults of these requests.
struct FreqLimits {
    limits: Mutex<Limits>,
    changed: Condvar,
}

impl FreqLimits {
    fn get() -> &'static FreqLimits {
        static INSTANCE: OnceLock<FreqLimits> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let policies = cpufreq_policies()
                .into_iter()
                .filter_map(|dir| {
                    Some(Policy {
                        bottom: read_int(&format!("{dir}/cpuinfo_min_freq"))?,
                        top: read_int(&format!("{dir}/cpuinfo_max_freq"))?,
                        min_freq: format!("{dir}/scaling_min_freq"),
                        max_freq: format!("{dir}/scaling_max_freq"),
                        applied_min: 0,
                        applied_max: 0,
                    })
                })
                .collect();
            // Drop any limits left by a previous instance.
            let mut limits = Limits { policies, ..Default::default() };
            limits.apply();
            write(GPU_MAX_PWRLEVEL, 0);
            thread::Builder::new()
                .name("power-loading".to_owned())
                .spawn(|| FreqLimits::get().expire_loading())
                .expect("Failed to start the loading boost thread");
            FreqLimits { limits: Mutex::new(limits), changed: Condvar::new() }
        })
    }

    fn set_idle_max(&self, max: FreqPair) {
        let mut limits = self.limits.lock().unwrap();
        limits.idle_max = max;
        limits.apply();
    }

    fn set_low_power(&self, enabled: bool) {
        let mut limits = self.limits.lock().unwrap();
        if enabled == limits.low_power {
            return;
        }
        limits.low_power = enabled;
        // Battery Saver wins over the loading boost.
        if enabled {
            limits.loading_until = None;
        }
        limits.apply();
        write(GPU_MAX_PWRLEVEL, if enabled { LOW_POWER_GPU_LEVEL } else { 0 });
        info!("Battery saver CPU/GPU cap {}", if enabled { "on" } else { "off" });
    }

    fn set_game_loading(&self, enabled: bool) {
        let mut limits = self.limits.lock().unwrap();
        limits.loading_until =
            (enabled && !limits.low_power).then(|| Instant::now() + GAME_LOADING_MAX);
        limits.apply();
        self.changed.notify_one();
    }

    /// Ends a loading boost the framework did not clear within GAME_LOADING_MAX.
    fn expire_loading(&self) {
        let mut limits = self.limits.lock().unwrap();
        loop {
            limits = match limits.loading_until {
                None => self.changed.wait(limits).unwrap(),
                Some(until) if Instant::now() >= until => {
                    limits.loading_until = None;
                    limits.apply();
                    limits
                }
                Some(until) => {
                    let timeout = until.saturating_duration_since(Instant::now());
                    self.changed.wait_timeout(limits, timeout).unwrap().0
                }
            };
        }
    }
}

/// Parses `# idle_limits <big>+<prime> <idle big>+<idle prime>` lines of thermal-engine.conf.
fn parse_idle_limits(config: &str) -> HashMap<FreqPair, FreqPair> {
    fn pair(field: &str) -> Option<FreqPair> {
        let (a, b) = field.split_once(|c: char| !c.is_ascii_digit())?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }
    config
        .lines()
        .filter_map(|line| {
            let mut fields = line.strip_prefix(IDLE_LIMITS_TAG)?.split_whitespace();
            Some((pair(fields.next()?)?, pair(fields.next()?)?))
        })
        .collect()
}

struct Cluster {
    cooling_state: String,
    freqs_descending: Vec<i32>,
}

#[derive(Default)]
struct IdleState {
    interact_until: Option<Instant>,
    launch_until: Option<Instant>,
    screen_off: bool,
    applied: FreqPair,
    logged: FreqPair,
}

struct IdleLimits {
    limits: HashMap<FreqPair, FreqPair>,
    clusters: Vec<Cluster>,
    state: Mutex<IdleState>,
    changed: Condvar,
}

fn after(deadline: Option<Instant>, now: Instant) -> bool {
    deadline.is_some_and(|d| d > now)
}

impl IdleLimits {
    /// None when the stock config has no idle limits or the cooling devices are missing.
    fn get() -> Option<&'static IdleLimits> {
        static INSTANCE: OnceLock<Option<IdleLimits>> = OnceLock::new();
        let instance = INSTANCE.get_or_init(|| {
            let limits = parse_idle_limits(&fs::read_to_string(THERMAL_ENGINE_CONFIG).ok()?);
            if limits.is_empty() {
                return None;
            }
            let clusters = find_clusters()?;
            info!("Idle CPU limits for {} thermal levels", limits.len());
            thread::Builder::new()
                .name("power-idle".to_owned())
                .spawn(|| IdleLimits::get().unwrap().run())
                .expect("Failed to start the idle limits thread");
            Some(IdleLimits {
                limits,
                clusters,
                state: Mutex::default(),
                changed: Condvar::new(),
            })
        });
        instance.as_ref()
    }

    fn current_limits(&self) -> Option<FreqPair> {
        let mut freqs = [0; 2];
        for (freq, cluster) in freqs.iter_mut().zip(&self.clusters) {
            let state: usize = fs::read_to_string(&cluster.cooling_state).ok()?.trim().parse().ok()?;
            *freq = cluster.freqs_descending[state.min(cluster.freqs_descending.len() - 1)];
        }
        Some((freqs[0], freqs[1]))
    }

    fn apply(&self, state: &mut IdleState, now: Instant) {
        let mut target = (0, 0);
        if !after(state.interact_until, now) && !after(state.launch_until, now) {
            if let Some(limits) = self.current_limits() {
                target = self.limits.get(&limits).copied().unwrap_or((0, 0));
            }
        }
        if target == state.applied {
            return;
        }
        FreqLimits::get().set_idle_max(target);
        state.applied = target;
        if target.0 > 0 && target != state.logged {
            info!("Idle CPU limit {}/{} MHz", target.0 / 1000, target.1 / 1000);
            state.logged = target;
        }
    }

    fn interaction(&self, hold: Duration) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.interact_until = state.interact_until.max(Some(now + hold));
        // Lift the limit right away, the worker restores it later.
        if state.applied.0 > 0 {
            self.apply(&mut state, now);
        }
        self.changed.notify_one();
    }

    fn set_launching(&self, launching: bool) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        state.launch_until = launching.then(|| now + LAUNCH_MAX);
        if !launching {
            state.interact_until = state.interact_until.max(Some(now + INTERACTION_HOLD));
        }
        self.apply(&mut state, now);
        self.changed.notify_one();
    }

    fn set_screen_on(&self, on: bool) {
        let mut state = self.state.lock().unwrap();
        state.screen_off = !on;
        if !on {
            state.interact_until = None;
            state.launch_until = None;
        }
        self.changed.notify_one();
    }

    fn run(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            let now = Instant::now();
            self.apply(&mut state, now);
            if state.screen_off {
                // The limits stay as they are until the screen is back on.
                state = self.changed.wait(state).unwrap();
                continue;
            }
            let next = [state.interact_until, state.launch_until]
                .into_iter()
                .flatten()
                .filter(|&d| d > now)
                .fold(now + POLL_INTERVAL, Instant::min);
            state = self.changed.wait_timeout(state, next - now).unwrap().0;
        }
    }
}

/// cpu-clusterN cooling devices step through policy N's frequencies from the top.
fn find_clusters() -> Option<Vec<Cluster>> {
    let policies = cpufreq_policies();
    if policies.len() != 2 {
        return None;
    }
    let mut states = [None, None];
    for entry in fs::read_dir("/sys/class/thermal").ok()?.flatten() {
        let path = entry.path();
        let Ok(kind) = fs::read_to_string(path.join("type")) else { continue };
        for (c, state) in states.iter_mut().enumerate() {
            if kind.trim() == format!("cpu-cluster{c}") {
                *state = Some(path.join("cur_state").to_string_lossy().into_owned());
            }
        }
    }
    policies
        .iter()
        .zip(states)
        .map(|(policy, cooling_state)| {
            let freqs = fs::read_to_string(format!("{policy}/scaling_available_frequencies")).ok()?;
            let mut freqs_descending: Vec<i32> =
                freqs.split_whitespace().filter_map(|f| f.parse().ok()).collect();
            if freqs_descending.is_empty() {
                return None;
            }
            freqs_descending.sort_unstable_by(|a, b| b.cmp(a));
            Some(Cluster { cooling_state: cooling_state?, freqs_descending })
        })
        .collect()
}

/// Some(supported) for the modes this extension handles.
pub fn is_mode_supported(mode: Mode) -> Option<bool> {
    match mode {
        Mode::LOW_POWER | Mode::GAME_LOADING => Some(true),
        Mode::LAUNCH => IdleLimits::get().map(|_| true),
        _ => None,
    }
}

/// Returns true when the mode was handled here and the common handling must be skipped.
pub fn set_mode(mode: Mode, enabled: bool) -> bool {
    match mode {
        Mode::LAUNCH => match IdleLimits::get() {
            Some(idle) => {
                idle.set_launching(enabled);
                true
            }
            None => false,
        },
        Mode::INTERACTIVE => {
            // Also handled by the common power HAL.
            if let Some(idle) = IdleLimits::get() {
                idle.set_screen_on(enabled);
            }
            false
        }
        Mode::LOW_POWER => {
            FreqLimits::get().set_low_power(enabled);
            true
        }
        Mode::GAME_LOADING => {
            FreqLimits::get().set_game_loading(enabled);
            true
        }
        _ => false,
    }
}

/// No boosts of its own: the common HAL decides.
pub fn is_boost_supported(_boost: Boost) -> Option<bool> {
    None
}

/// Input lifts the idle CPU limits for a while.
pub fn set_boost(boost: Boost, duration_ms: i32) {
    if boost == Boost::INTERACTION {
        if let Some(idle) = IdleLimits::get() {
            idle.interaction(INTERACTION_HOLD.max(Duration::from_millis(duration_ms.max(0) as u64)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_idle_limits() {
        let config = "\
[CPU0]\n\
# idle_limits 3532800+4320000 2227200+1689600\n\
# idle_limits 2400000+2246400 1555200+1401600\n\
# idle_limits broken\n\
#idle_limits 1+2 3+4\n";
        let limits = parse_idle_limits(config);
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[&(3532800, 4320000)], (2227200, 1689600));
        assert_eq!(limits[&(2400000, 2246400)], (1555200, 1401600));
    }

    #[test]
    fn cluster_values() {
        assert_eq!(cluster_value(LOW_POWER_MAX, 0), 2227200);
        assert_eq!(cluster_value(LOW_POWER_MAX, 1), 2649600);
        assert_eq!(cluster_value(LOW_POWER_MAX, 2), 0);
    }
}
