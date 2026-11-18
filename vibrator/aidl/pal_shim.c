/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

#define LOG_TAG "vendor.qti.vibratorCL"

#include "pal_shim.h"

#include <errno.h>
#include <log/log.h>
#include <stdlib.h>
#include <string.h>

#include "PalApi.h"
#include "PalDefs.h"
#include "rx_haptics_api.h"
#include "wsa_haptics_vi_api.h"

static int32_t on_stream_event(pal_stream_handle_t* handle __unused, uint32_t event_id __unused,
                               uint32_t* event_data, uint32_t event_size __unused,
                               uint64_t cookie __unused) {
    if (event_data) vib_pal_on_event(*event_data);
    return 0;
}

static int32_t set_param(void* handle, uint32_t param_id, const void* data, size_t size) {
    pal_param_payload* payload = calloc(1, sizeof(*payload) + size);
    if (!payload) return -ENOMEM;

    payload->payload_size = size;
    memcpy(payload->payload, data, size);
    int32_t ret = pal_stream_set_param(handle, param_id, payload);
    free(payload);
    return ret;
}

int32_t vib_pal_open(void** handle) {
    struct pal_stream_attributes attributes;
    struct pal_device device;

    memset(&attributes, 0, sizeof(attributes));
    attributes.type = PAL_STREAM_HAPTICS;
    attributes.direction = PAL_AUDIO_OUTPUT;
    attributes.info.opt_stream_info.haptics_type = PAL_STREAM_HAPTICS_TOUCH;

    memset(&device, 0, sizeof(device));
    device.id = PAL_DEVICE_OUT_HAPTICS_DEVICE;
    device.config.bit_width = 16;
    device.config.sample_rate = 48000;
    device.config.ch_info.channels = 1;

    return pal_stream_open(&attributes, 1, &device, 0, NULL, on_stream_event, 0,
                           (pal_stream_handle_t**)handle);
}

int32_t vib_pal_configure(void* handle, int16_t effect_id, int16_t strength, int32_t time_ms) {
    pal_param_haptics_cnfg_t config;

    memset(&config, 0, sizeof(config));
    config.mode = PAL_STREAM_HAPTICS_TOUCH;
    config.effect_id = effect_id;
    config.strength = strength;
    config.time = time_ms;
    config.ch_mask = 1;
    config.amplitude = 0.5f;
    return set_param(handle, PAL_PARAM_ID_HAPTICS_CNFG, &config, sizeof(config));
}

int32_t vib_pal_start(void* handle) {
    return pal_stream_start(handle);
}

int32_t vib_pal_stop_effect(void* handle) {
    param_id_haptics_wave_designer_wave_designer_stop_param_t stop = {.channel_mask = 1};

    return set_param(handle, PARAM_ID_HAPTICS_WAVE_DESIGNER_STOP_PARAM, &stop, sizeof(stop));
}

// The DSP reads these two params as a pal_param_haptics_cnfg_t, not as their SPF structs.
int32_t vib_pal_persistent(void* handle) {
    pal_param_haptics_cnfg_t config;

    memset(&config, 0, sizeof(config));
    config.ch_mask = 1;
    return set_param(handle, PARAM_ID_HAPTICS_EX_VI_PERSISTENT, &config, sizeof(config));
}

int32_t vib_pal_update_amplitude(void* handle, float amplitude) {
    pal_param_haptics_cnfg_t config;

    memset(&config, 0, sizeof(config));
    config.ch_mask = 1;
    config.amplitude = amplitude;
    return set_param(handle, PARAM_ID_HAPTICS_WAVE_DESIGNER_UPDATE_PARAM, &config,
                     sizeof(config));
}

int32_t vib_pal_close(void* handle) {
    int32_t ret = pal_stream_stop(handle);
    if (ret) ALOGE("Failed to stop haptics stream: %d", ret);

    ret = pal_stream_close(handle);
    if (ret) ALOGE("Failed to close haptics stream: %d", ret);
    return ret;
}

int32_t vib_pal_dynamic_cal(void) {
    pal_haptics_payload mode = {.operationMode = PAL_HAP_MODE_DYNAMIC_CAL};

    return pal_set_param(PAL_PARAM_ID_HAPTICS_MODE, &mode, sizeof(mode));
}
