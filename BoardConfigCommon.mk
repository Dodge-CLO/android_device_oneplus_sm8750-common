#
# SPDX-FileCopyrightText: The LineageOS Project
# SPDX-License-Identifier: Apache-2.0
#

COMMON_PATH := device/oneplus/sm8750-common

# add-radio-file-sha1-checked is used by the generated firmware makefiles
define add-radio-file-sha1-checked
  $(eval path := $(LOCAL_PATH)/$(1))
  $(eval sha1 := $(shell sha1sum "$(path)" | cut -d" " -f 1))
  $(if $(filter $(sha1),$(2)),
    $(call add-radio-file,$(1)),
    $(error $(path) SHA1 mismatch ($(sha1) != $(2))))
endef

# A/B
AB_OTA_UPDATER := true

AB_OTA_PARTITIONS += \
    boot \
    dtbo \
    init_boot \
    odm \
    product \
    recovery \
    system \
    system_dlkm \
    system_ext \
    vbmeta \
    vbmeta_system \
    vbmeta_vendor \
    vendor \
    vendor_boot \
    vendor_dlkm

# Architecture
TARGET_ARCH := arm64
TARGET_ARCH_VARIANT := armv8-2a-dotprod
TARGET_CPU_ABI := arm64-v8a
TARGET_CPU_VARIANT := generic
TARGET_CPU_VARIANT_RUNTIME := oryon

# Audio
AUDIO_FEATURE_ENABLED_DLKM := true
AUDIO_FEATURE_ENABLED_EXTENDED_COMPRESS_FORMAT := true
AUDIO_FEATURE_ENABLED_GKI := true
AUDIO_FEATURE_ENABLED_INSTANCE_ID := true
AUDIO_FEATURE_ENABLED_MCS := true
AUDIO_FEATURE_ENABLED_SVA_MULTI_STAGE := true
BOARD_SUPPORTS_OPENSOURCE_STHAL := true
BOARD_SUPPORTS_SOUND_TRIGGER := true
BOARD_USES_ALSA_AUDIO := true

# Bootloader
TARGET_BOOTLOADER_BOARD_NAME := sun

# Build
BUILD_BROKEN_DUP_RULES := true

# Boot
BOARD_BOOT_HEADER_VERSION := 4
BOARD_MKBOOTIMG_ARGS += --header_version $(BOARD_BOOT_HEADER_VERSION)
BOARD_RAMDISK_USE_LZ4 := true

# DTB / DTBO
BOARD_INCLUDE_DTB_IN_BOOTIMG := true

# Power: Battery Saver and game loading modes, and double tap to wake through the Oplus touch HAL
TARGET_POWERHAL_MODE_EXT := $(COMMON_PATH)/power/power-mode.cpp
TARGET_POWER_FEATURE_EXT_LIB := //$(COMMON_PATH):libpowerfeature_ext_sm8750

# Properties
TARGET_ODM_PROP += $(COMMON_PATH)/properties/odm.prop
TARGET_PRODUCT_PROP += $(COMMON_PATH)/properties/product.prop
TARGET_SYSTEM_EXT_PROP += $(COMMON_PATH)/properties/system_ext.prop
TARGET_VENDOR_PROP += $(COMMON_PATH)/properties/vendor.prop

# Filesystem
TARGET_FS_CONFIG_GEN += $(COMMON_PATH)/config.fs

# Init Boot
BOARD_INIT_BOOT_HEADER_VERSION := 4
BOARD_MKBOOTIMG_INIT_ARGS += --header_version $(BOARD_INIT_BOOT_HEADER_VERSION)

# Kernel
BOARD_BOOTCONFIG := \
    androidboot.hardware=qcom \
    androidboot.hypervisor.protected_vm.supported=true \
    androidboot.load_modules_parallel=true \
    androidboot.memcg=1 \
    androidboot.serialconsole=0 \
    androidboot.usbcontroller=a600000.dwc3 \
    androidboot.vendor.qspa=true

BOARD_KERNEL_CMDLINE := \
    console=ttynull \
    log_buf_len=2M \
    nohugevmalloc \
    nosoftlockup \
    qcom_geni_serial.con_enabled=0 \
    sysctl.kernel.firmware_config.force_sysfs_fallback=1

BOARD_USES_GENERIC_KERNEL_IMAGE := true
BOARD_KERNEL_BASE := 0x00000000
BOARD_KERNEL_PAGESIZE := 4096
BOARD_KERNEL_IMAGE_NAME := Image

# Kernel
TARGET_KERNEL_VARIANT := perf
TARGET_USES_SMMU_PROXY := true

# Kernel modules
BOARD_VENDOR_KERNEL_MODULES_LOAD := $(strip $(shell sed 's/#.*$$//;/^$$/d' $(COMMON_PATH)/modules.load))
BOARD_VENDOR_RAMDISK_RECOVERY_KERNEL_MODULES_LOAD := $(strip $(shell sed 's/#.*$$//;/^$$/d' $(COMMON_PATH)/modules.load.recovery))

BOARD_SYSTEM_KERNEL_MODULES := $(wildcard device/qcom/sun-kernel/system_dlkm/flatten/lib/modules/*.ko)
BOARD_SYSTEM_KERNEL_MODULES_LOAD := $(strip $(shell sed 's/#.*$$//;/^$$/d' $(COMMON_PATH)/modules.load.system_dlkm))

BOARD_VENDOR_KERNEL_MODULES += $(addprefix $(KERNEL_MODULES_OUT)/, \
    ipclite.ko \
    smmu_proxy_dlkm.ko)

BOARD_VENDOR_KERNEL_MODULES += $(addprefix $(KERNEL_MODULES_OUT)/, \
    cnss2.ko \
    cnss_nl.ko \
    cnss_plat_ipc_qmi_svc.ko \
    cnss_prealloc.ko \
    cnss_utils.ko \
    icnss2.ko \
    qca_cld3_kiwi_v2.ko \
    qca_cld3_peach_v2.ko \
    qca_cld3_wcn7750.ko \
    wlan_firmware_service.ko)

BOARD_VENDOR_RAMDISK_KERNEL_MODULES += $(addprefix $(KERNEL_MODULES_OUT)/, \
    adsp_loader_dlkm.ko \
    gpr_dlkm.ko \
    hdcp_qseecom_dlkm.ko \
    ipclite.ko \
    msm_drm.ko \
    msm_ext_display.ko \
    msm_hw_fence.ko \
    msm-mmrm.ko \
    q6_notifier_dlkm.ko \
    q6_pdr_dlkm.ko \
    smcinvoke_dlkm.ko \
    smmu_proxy_dlkm.ko \
    snd_event_dlkm.ko \
    spf_core_dlkm.ko \
    sync_fence.ko \
    synx-driver.ko)

# Platform
BOARD_USES_QCOM_HARDWARE := true
TARGET_BOARD_PLATFORM := sun

# Metadata
BOARD_USES_METADATA_PARTITION := true

# Partitions
BOARD_PRODUCTIMAGE_MINIMAL_PARTITION_RESERVED_SIZE := false
BOARD_BOOTIMAGE_PARTITION_SIZE := 100663296
BOARD_DTBOIMG_PARTITION_SIZE := 25165824
BOARD_INIT_BOOT_IMAGE_PARTITION_SIZE := 8388608
BOARD_USERDATAIMAGE_PARTITION_SIZE := 233871900672
BOARD_VENDOR_BOOTIMAGE_PARTITION_SIZE := 100663296
BOARD_RECOVERYIMAGE_PARTITION_SIZE := 104857600
BOARD_SYSTEMIMAGE_PARTITION_RESERVED_SIZE := 94371840
BOARD_PRODUCTIMAGE_PARTITION_RESERVED_SIZE := 2097152000
BOARD_SYSTEM_EXTIMAGE_PARTITION_RESERVED_SIZE := 157286400
BOARD_SYSTEMIMAGE_EXTFS_INODE_COUNT := -1
BOARD_PRODUCTIMAGE_EXTFS_INODE_COUNT := -1
BOARD_SYSTEM_EXTIMAGE_EXTFS_INODE_COUNT := -1
BOARD_ODMIMAGE_FILE_SYSTEM_TYPE := erofs
BOARD_PRODUCTIMAGE_FILE_SYSTEM_TYPE := ext4
BOARD_SYSTEM_EXTIMAGE_FILE_SYSTEM_TYPE := ext4
BOARD_SYSTEMIMAGE_FILE_SYSTEM_TYPE := ext4
BOARD_SYSTEM_DLKMIMAGE_FILE_SYSTEM_TYPE := erofs
BOARD_VENDORIMAGE_FILE_SYSTEM_TYPE := erofs
BOARD_VENDOR_DLKMIMAGE_FILE_SYSTEM_TYPE := erofs
BOARD_ONEPLUS_DYNAMIC_PARTITIONS_PARTITION_LIST := odm product system system_dlkm system_ext vendor vendor_dlkm
BOARD_ONEPLUS_DYNAMIC_PARTITIONS_SIZE := $(shell echo $$(($(BOARD_SUPER_PARTITION_SIZE) - 4194304))) # (BOARD_SUPER_PARTITION_SIZE - "reasonable overhead of 4 MiB")
BOARD_SUPER_PARTITION_GROUPS := oneplus_dynamic_partitions
BOARD_FLASH_BLOCK_SIZE := 262144 # (BOARD_KERNEL_PAGESIZE * 64)
TARGET_COPY_OUT_ODM := odm
TARGET_COPY_OUT_PRODUCT := product
TARGET_COPY_OUT_SYSTEM_DLKM := system_dlkm
TARGET_COPY_OUT_SYSTEM_EXT := system_ext
TARGET_COPY_OUT_VENDOR := vendor
TARGET_COPY_OUT_VENDOR_DLKM := vendor_dlkm

# Recovery
BOARD_EXCLUDE_KERNEL_FROM_RECOVERY_IMAGE := true
TARGET_RECOVERY_FSTAB := $(COMMON_PATH)/init/fstab.default
TARGET_RECOVERY_PIXEL_FORMAT := RGBX_8888
TARGET_USERIMAGES_USE_EXT4 := true
TARGET_USERIMAGES_USE_F2FS := true

# RIL
ENABLE_VENDOR_RIL_SERVICE := true

# Security
BOOT_SECURITY_PATCH := 2026-09-01
VENDOR_SECURITY_PATCH := $(BOOT_SECURITY_PATCH)

# SEPolicy
# QCOM policy first, hardware/oplus attaches attributes to its domains
TARGET_EXCLUDE_QCOM_SEPOLICY := true
include device/qcom/sepolicy_vndr/SEPolicy.mk
include device/qcom/common/sepolicy/SEPolicy.mk
QCOM_SEPOLICY_PATH := $(SEPOLICY_PATH)
SEPOLICY_PATH :=
include hardware/oplus/sepolicy/qti/SEPolicy.mk
SEPOLICY_PATH := $(QCOM_SEPOLICY_PATH)
include $(COMMON_PATH)/sepolicy/SEPolicy.mk

# UFS
SOONG_CONFIG_NAMESPACES += ufsbsg
SOONG_CONFIG_ufsbsg += ufsframework
SOONG_CONFIG_ufsbsg_ufsframework := bsg

# Verified Boot
BOARD_AVB_ENABLE := true
BOARD_AVB_MAKE_VBMETA_IMAGE_ARGS += --flags 3
BOARD_MOVE_GSI_AVB_KEYS_TO_VENDOR_BOOT := true

BOARD_AVB_BOOT_KEY_PATH := external/avb/test/data/testkey_rsa4096.pem
BOARD_AVB_BOOT_ALGORITHM := SHA256_RSA4096
BOARD_AVB_BOOT_ROLLBACK_INDEX := $(PLATFORM_SECURITY_PATCH_TIMESTAMP)
BOARD_AVB_BOOT_ROLLBACK_INDEX_LOCATION := 4

BOARD_AVB_DTBO_KEY_PATH := external/avb/test/data/testkey_rsa4096.pem
BOARD_AVB_DTBO_ALGORITHM := SHA256_RSA4096
BOARD_AVB_DTBO_ROLLBACK_INDEX := $(PLATFORM_SECURITY_PATCH_TIMESTAMP)
BOARD_AVB_DTBO_ROLLBACK_INDEX_LOCATION := 3

BOARD_AVB_RECOVERY_KEY_PATH := external/avb/test/data/testkey_rsa4096.pem
BOARD_AVB_RECOVERY_ALGORITHM := SHA256_RSA4096
BOARD_AVB_RECOVERY_ROLLBACK_INDEX := $(PLATFORM_SECURITY_PATCH_TIMESTAMP)
BOARD_AVB_RECOVERY_ROLLBACK_INDEX_LOCATION := 1

BOARD_AVB_VBMETA_SYSTEM := system system_dlkm system_ext product
BOARD_AVB_VBMETA_SYSTEM_KEY_PATH := external/avb/test/data/testkey_rsa4096.pem
BOARD_AVB_VBMETA_SYSTEM_ALGORITHM := SHA256_RSA4096
BOARD_AVB_VBMETA_SYSTEM_ROLLBACK_INDEX := $(PLATFORM_SECURITY_PATCH_TIMESTAMP)
BOARD_AVB_VBMETA_SYSTEM_ROLLBACK_INDEX_LOCATION := 2

BOARD_AVB_VBMETA_VENDOR := odm vendor vendor_dlkm
BOARD_AVB_VBMETA_VENDOR_KEY_PATH := external/avb/test/data/testkey_rsa4096.pem
BOARD_AVB_VBMETA_VENDOR_ALGORITHM := SHA256_RSA4096
BOARD_AVB_VBMETA_VENDOR_ROLLBACK_INDEX := $(PLATFORM_SECURITY_PATCH_TIMESTAMP)
BOARD_AVB_VBMETA_VENDOR_ROLLBACK_INDEX_LOCATION := 5

BOARD_AVB_VENDOR_BOOT_KEY_PATH := external/avb/test/data/testkey_rsa4096.pem
BOARD_AVB_VENDOR_BOOT_ALGORITHM := SHA256_RSA4096
BOARD_AVB_VENDOR_BOOT_ROLLBACK_INDEX := $(PLATFORM_SECURITY_PATCH_TIMESTAMP)
BOARD_AVB_VENDOR_BOOT_ROLLBACK_INDEX_LOCATION := 6

# WiFi
BOARD_WPA_SUPPLICANT_PRIVATE_LIB_EVENT := "ON"
WIFI_HIDL_FEATURE_AWARE := true
WIFI_HIDL_FEATURE_DUAL_INTERFACE := true

# Include the proprietary files BoardConfig.
include vendor/oneplus/sm8750-common/BoardConfigVendor.mk

# Oplus camera extension
BOARD_VENDOR_KERNEL_MODULES += $(KERNEL_MODULES_OUT)/camera_extension.ko
