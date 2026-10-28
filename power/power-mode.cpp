/*
 * SPDX-FileCopyrightText: 2024-2026 The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 *
 * SM8750 power HAL mode extension, built into the QTI power HAL through
 * TARGET_POWERHAL_MODE_EXT.
 * Battery Saver (Mode::LOW_POWER) caps CPU and GPU max frequency, and game
 * loading screens (Mode::GAME_LOADING) get a short CPU boost. Both use perf
 * HAL locks, so they work together with thermal-engine and other boosts.
 */

#include <aidl/android/hardware/power/BnPower.h>

#include <android-base/logging.h>

#include <mutex>

#include "utils.h"

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

}  // namespace

bool isDeviceSpecificModeSupported(Mode type, bool* _aidl_return) {
    switch (type) {
        case Mode::LOW_POWER:
        case Mode::GAME_LOADING:
            *_aidl_return = true;
            return true;
        default:
            return false;
    }
}

bool setDeviceSpecificMode(Mode type, bool enabled) {
    switch (type) {
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
