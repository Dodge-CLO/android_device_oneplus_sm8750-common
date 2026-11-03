/*
 * SPDX-FileCopyrightText: The LineageOS Project
 * SPDX-License-Identifier: Apache-2.0
 */

#include <android-base/logging.h>
#include <android-base/parseint.h>
#include <android-base/properties.h>

#include <fs_mgr.h>

#define _REALLY_INCLUDE_SYS__SYSTEM_PROPERTIES_H_
#include <sys/_system_properties.h>

#include <string>
#include <unordered_map>

namespace android {
namespace init {
uint32_t InitPropertySet(const std::string& name, const std::string& value);
}  // namespace init
}  // namespace android

using android::base::ParseInt;
using android::fs_mgr::GetKernelCmdline;
using android::base::GetProperty;
using android::init::InitPropertySet;
using namespace std::string_literals;

namespace {
constexpr std::string kCmdlineRegion = "oplus_region";
const std::unordered_map<int, std::string> kRegionMap = {
        {27, "IN"},
        {68, "EU"},
        {151, "CN"},
        {161, "NA"},
        {167, "ROW"},
};

const char* const kRoProductPartitions[] = {
        "bootimage.", "product.", "system.", "system_dlkm.",
        "system_ext.", "vendor.", "vendor_dlkm.",
};

void OverrideProperty(const std::string& name, const std::string& value) {
    auto pi = const_cast<prop_info*>(__system_property_find(name.c_str()));
    if (pi != nullptr) {
        __system_property_update(pi, value.c_str(), value.size());
    }
}
}  // anonymous namespace

/*
 * Only for read-only properties. Properties that can be wrote to more
 * than once should be set in a typical init script (e.g. init.oplus.hw.rc)
 * after the original property has been set.
 */
void vendor_process_bootenv() {
    std::string buf;
    if (!GetKernelCmdline(kCmdlineRegion, &buf)) {
        LOG(ERROR) << kCmdlineRegion << " not found in /proc/cmdline";
        return;
    }

    int region_id;
    if (!ParseInt(buf, &region_id)) {
        LOG(ERROR) << "Region ID [" << buf << "] is invalid";
        return;
    }

    auto it = kRegionMap.find(region_id);
    if (it == kRegionMap.end()) {
        LOG(ERROR) << "Unexpected region ID: " << region_id;
    } else {
        InitPropertySet("ro.boot.hardware.revision", it->second);
    }
}

/*
 * The build-time model is the EU one. The variant specific odm props
 * (properties/<prjname>/build.<region>.prop) carry the real model, so
 * mirror them to the other partitions to keep every ro.product.* consistent.
 */
void vendor_load_properties() {
    for (const auto& prop : {"model", "name"}) {
        auto value = GetProperty("ro.product.odm."s + prop, "");
        if (value.empty()) continue;
        for (const auto& partition : kRoProductPartitions) {
            OverrideProperty("ro.product."s + partition + prop, value);
        }
    }

    // ro.build.fingerprint is not shipped in system/build.prop, so init would
    // derive it from the build. Report the stock one set on the vendor partition.
    auto fingerprint = GetProperty("ro.vendor.build.fingerprint", "");
    if (!fingerprint.empty()) {
        InitPropertySet("ro.build.fingerprint", fingerprint);
    }
}
