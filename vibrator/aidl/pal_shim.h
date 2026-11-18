/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Packs the PAL haptics structs for the closed loop path. Holds no state: the stream handle
// and all play state are owned by the caller.

// Implemented by the HAL. Called on a Binder thread for every DSP event of the stream.
void vib_pal_on_event(uint32_t event);

int32_t vib_pal_open(void** handle);
int32_t vib_pal_configure(void* handle, int16_t effect_id, int16_t strength, int32_t time_ms);
int32_t vib_pal_start(void* handle);
int32_t vib_pal_stop_effect(void* handle);
int32_t vib_pal_persistent(void* handle);
int32_t vib_pal_update_amplitude(void* handle, float amplitude);
int32_t vib_pal_close(void* handle);
int32_t vib_pal_dynamic_cal(void);

#ifdef __cplusplus
}
#endif
