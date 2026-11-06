/*
 * SPDX-FileCopyrightText: 2024-2026 The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 *
 * SM8750 power HAL mode extension, built into the QTI power HAL through
 * TARGET_POWERHAL_MODE_EXT.
 * Battery Saver (Mode::LOW_POWER) caps CPU and GPU max frequency, and game
 * loading screens (Mode::GAME_LOADING) get a short CPU boost. Both use perf
 * HAL locks, so they work together with thermal-engine and other boosts.
 *
 * thermal-engine limits the CPUs to the stock boost ceilings. Like stock, the
 * stricter stock limits listed in thermal-engine.conf apply while the user is
 * not touching the screen or launching an app.
 */

#include <aidl/android/hardware/power/BnPower.h>

#include <android-base/file.h>
#include <android-base/logging.h>
#include <android-base/strings.h>

#include <algorithm>
#include <chrono>
#include <condition_variable>
#include <cstring>
#include <dirent.h>
#include <map>
#include <mutex>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include "utils.h"

using ::aidl::android::hardware::power::Boost;
using ::aidl::android::hardware::power::Mode;

namespace {

// Private hint id used to track the Battery Saver perf lock.
constexpr int kLowPowerHintId = 0x00C41001;

// Perf HAL opcodes. On sun the perf HAL has two clusters: big = cpu0-5 and
// prime = cpu6-7. Frequency values are in MHz.
constexpr int kMaxFreqBig = 0x40804000;
constexpr int kMaxFreqPrime = 0x40804200;
// GPU max power level. Higher index means lower frequency; level 6 is
// 660 MHz (top level is 1100 MHz).
constexpr int kGpuMaxPwrlevel = 0x42808000;

int gLowPowerResources[] = {
        kMaxFreqBig,     2227,  // stock max 3532
        kMaxFreqPrime,   2649,  // stock max 4320
        kGpuMaxPwrlevel, 6,
};

// Min frequency opcodes, same cluster order as above.
constexpr int kMinFreqBig = 0x40800000;
constexpr int kMinFreqPrime = 0x40800200;

// Game loading screens (GameManager Mode::GAME_LOADING): raise CPU floors so
// level loads finish faster. Capped by kGameLoadingMaxMs in case the
// framework never clears the mode.
constexpr int kGameLoadingMaxMs = 10000;
int gGameLoadingResources[] = {
        kMinFreqBig,   2227,
        kMinFreqPrime, 2649,
};

std::mutex gLowPowerLock;
bool gLowPowerActive = false;
int gGameLoadingHandle = 0;

void setGameLoading(bool enabled) {
    std::lock_guard<std::mutex> lock(gLowPowerLock);
    if (gGameLoadingHandle > 0) {
        release_request(gGameLoadingHandle);
        gGameLoadingHandle = 0;
    }
    // Battery Saver wins over the loading boost.
    if (!enabled || gLowPowerActive) return;

    int handle = interaction_with_handle(
            0, kGameLoadingMaxMs,
            sizeof(gGameLoadingResources) / sizeof(gGameLoadingResources[0]),
            gGameLoadingResources);
    if (handle > 0) gGameLoadingHandle = handle;
}

void setLowPower(bool enabled) {
    std::lock_guard<std::mutex> lock(gLowPowerLock);
    if (enabled == gLowPowerActive) return;

    // Drop any running game-loading boost before capping.
    if (enabled && gGameLoadingHandle > 0) {
        release_request(gGameLoadingHandle);
        gGameLoadingHandle = 0;
    }

    // perform_hint_action() logs a failed perf lock itself; undo_hint_action()
    // is a no-op for a hint id that never got a lock.
    if (enabled) {
        perform_hint_action(kLowPowerHintId, gLowPowerResources,
                            sizeof(gLowPowerResources) / sizeof(gLowPowerResources[0]));
    } else {
        undo_hint_action(kLowPowerHintId);
    }
    gLowPowerActive = enabled;
    LOG(INFO) << "Battery saver CPU/GPU cap " << (enabled ? "on" : "off");
}

// Idle CPU limits, see the file comment.
constexpr const char* kThermalEngineConfig = "/vendor/etc/thermal-engine.conf";
constexpr const char* kIdleLimitsTag = "# idle_limits";
// Keep the boost ceilings this long after the last input event, so scrolls,
// flings and the animations that follow a tap run unlimited.
constexpr auto kInteractionHold = std::chrono::milliseconds(2000);
constexpr auto kLaunchMax = std::chrono::milliseconds(5000);
// thermal-engine samples the shell temperature once a second.
constexpr auto kPollInterval = std::chrono::milliseconds(1000);

using Clock = std::chrono::steady_clock;
using FreqPair = std::pair<int, int>;  // big, prime in kHz

struct Cluster {
    std::string coolingState;
    std::vector<int> freqsDescending;
};

class IdleLimits {
  public:
    static IdleLimits& get() {
        static IdleLimits instance;
        return instance;
    }

    bool active() const { return mActive; }

    void interaction(Clock::duration hold) {
        if (!mActive) return;
        std::lock_guard<std::mutex> lock(mLock);
        mInteractUntil = std::max(mInteractUntil, Clock::now() + hold);
        // Lift the limit right away, the worker restores it later
        if (mHandle > 0) applyLocked(Clock::now());
        mCv.notify_one();
    }

    void setLaunching(bool launching) {
        if (!mActive) return;
        std::lock_guard<std::mutex> lock(mLock);
        mLaunchUntil = launching ? Clock::now() + kLaunchMax : Clock::time_point();
        if (!launching) mInteractUntil = std::max(mInteractUntil, Clock::now() + kInteractionHold);
        applyLocked(Clock::now());
        mCv.notify_one();
    }

    void setScreenOn(bool on) {
        if (!mActive) return;
        std::lock_guard<std::mutex> lock(mLock);
        mScreenOn = on;
        if (!on) {
            mInteractUntil = Clock::time_point();
            mLaunchUntil = Clock::time_point();
        }
        mCv.notify_one();
    }

  private:
    IdleLimits() {
        mActive = loadLimits() && findClusters();
        if (mActive) {
            LOG(INFO) << "Idle CPU limits for " << mLimits.size() << " thermal levels";
            std::thread([this] { loop(); }).detach();
        }
    }

    bool loadLimits() {
        std::string config;
        if (!android::base::ReadFileToString(kThermalEngineConfig, &config)) return false;
        for (const auto& line : android::base::Split(config, "\n")) {
            if (!android::base::StartsWith(line, kIdleLimitsTag)) continue;
            std::istringstream fields(line.substr(strlen(kIdleLimitsTag)));
            std::string limits, idle;
            int big, prime, idleBig, idlePrime;
            char plus;
            if (!(fields >> limits >> idle)) continue;
            std::istringstream l(limits), i(idle);
            if (!(l >> big >> plus >> prime) || !(i >> idleBig >> plus >> idlePrime)) continue;
            mLimits[{big, prime}] = {idleBig, idlePrime};
        }
        return !mLimits.empty();
    }

    // cpu-clusterN cooling devices step through policy N's frequencies from the top
    bool findClusters() {
        std::vector<std::string> policies;
        if (DIR* dir = opendir("/sys/devices/system/cpu/cpufreq")) {
            while (dirent* entry = readdir(dir)) {
                if (android::base::StartsWith(entry->d_name, "policy")) {
                    policies.push_back(entry->d_name);
                }
            }
            closedir(dir);
        }
        std::sort(policies.begin(), policies.end(), [](const auto& a, const auto& b) {
            return std::stoi(a.substr(6)) < std::stoi(b.substr(6));
        });

        mClusters.assign(2, {});
        if (policies.size() != mClusters.size()) return false;
        if (DIR* dir = opendir("/sys/class/thermal")) {
            while (dirent* entry = readdir(dir)) {
                std::string path = std::string("/sys/class/thermal/") + entry->d_name;
                std::string type;
                if (!android::base::ReadFileToString(path + "/type", &type)) continue;
                type = android::base::Trim(type);
                for (size_t c = 0; c < mClusters.size(); c++) {
                    if (type == "cpu-cluster" + std::to_string(c)) {
                        mClusters[c].coolingState = path + "/cur_state";
                    }
                }
            }
            closedir(dir);
        }
        for (size_t c = 0; c < mClusters.size(); c++) {
            std::string freqs;
            if (mClusters[c].coolingState.empty() ||
                !android::base::ReadFileToString("/sys/devices/system/cpu/cpufreq/" + policies[c] +
                                                         "/scaling_available_frequencies",
                                                 &freqs)) {
                return false;
            }
            std::istringstream input(freqs);
            int freq;
            while (input >> freq) mClusters[c].freqsDescending.push_back(freq);
            if (mClusters[c].freqsDescending.empty()) return false;
            std::sort(mClusters[c].freqsDescending.rbegin(), mClusters[c].freqsDescending.rend());
        }
        return true;
    }

    bool currentLimits(FreqPair* limits) {
        int freqs[2];
        for (size_t c = 0; c < mClusters.size(); c++) {
            std::string state;
            if (!android::base::ReadFileToString(mClusters[c].coolingState, &state)) return false;
            const auto& table = mClusters[c].freqsDescending;
            size_t index = std::min<size_t>(std::strtoul(state.c_str(), nullptr, 10),
                                            table.size() - 1);
            freqs[c] = table[index];
        }
        *limits = {freqs[0], freqs[1]};
        return true;
    }

    void applyLocked(Clock::time_point now) {
        FreqPair target = {0, 0};
        FreqPair limits;
        if (now >= mInteractUntil && now >= mLaunchUntil && currentLimits(&limits)) {
            auto it = mLimits.find(limits);
            if (it != mLimits.end()) target = it->second;
        }
        if (target == mApplied) return;

        if (mHandle > 0) release_request(mHandle);
        mHandle = 0;
        mApplied = {0, 0};
        if (target.first > 0) {
            int resources[] = {
                    kMaxFreqBig,   target.first / 1000,
                    kMaxFreqPrime, target.second / 1000,
            };
            mHandle = interaction_with_handle(0, 0, sizeof(resources) / sizeof(resources[0]),
                                              resources);
            if (mHandle > 0) mApplied = target;
            if (target != mLogged) {
                LOG(INFO) << "Idle CPU limit " << target.first / 1000 << "/"
                          << target.second / 1000 << " MHz";
                mLogged = target;
            }
        }
    }

    void loop() {
        std::unique_lock<std::mutex> lock(mLock);
        while (true) {
            auto now = Clock::now();
            applyLocked(now);
            if (!mScreenOn) {
                // The limits stay as they are until the screen is back on
                mCv.wait(lock);
                continue;
            }
            auto next = now + kPollInterval;
            if (mInteractUntil > now) next = std::min(next, mInteractUntil);
            if (mLaunchUntil > now) next = std::min(next, mLaunchUntil);
            mCv.wait_until(lock, next);
        }
    }

    bool mActive = false;
    std::map<FreqPair, FreqPair> mLimits;
    std::vector<Cluster> mClusters;

    std::mutex mLock;
    std::condition_variable mCv;
    Clock::time_point mInteractUntil;
    Clock::time_point mLaunchUntil;
    bool mScreenOn = true;
    int mHandle = 0;
    FreqPair mApplied = {0, 0};
    FreqPair mLogged = {0, 0};
};

}  // namespace

bool isDeviceSpecificModeSupported(Mode type, bool* _aidl_return) {
    switch (type) {
        case Mode::LOW_POWER:
        case Mode::GAME_LOADING:
            *_aidl_return = true;
            return true;
        case Mode::LAUNCH:
            if (!IdleLimits::get().active()) return false;
            *_aidl_return = true;
            return true;
        default:
            return false;
    }
}

bool setDeviceSpecificBoost(Boost type, int32_t durationMs) {
    if (type != Boost::INTERACTION) return false;
    IdleLimits::get().interaction(
            std::max<Clock::duration>(kInteractionHold, std::chrono::milliseconds(durationMs)));
    return true;
}

bool setDeviceSpecificMode(Mode type, bool enabled) {
    switch (type) {
        case Mode::LAUNCH:
            if (!IdleLimits::get().active()) return false;
            IdleLimits::get().setLaunching(enabled);
            return true;
        case Mode::INTERACTIVE:
            // Also handled by the common power HAL
            IdleLimits::get().setScreenOn(enabled);
            return false;
        case Mode::LOW_POWER:
            setLowPower(enabled);
            return true;
        case Mode::GAME_LOADING:
            setGameLoading(enabled);
            return true;
        default:
            return false;
    }
}
