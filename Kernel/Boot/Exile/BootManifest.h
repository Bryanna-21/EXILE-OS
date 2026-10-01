#pragma once

#include <AK/Types.h>

namespace Exile::Boot {

/*
 * Exile Boot Manifest v1
 *
 * Serialized representation:
 *
 *   offset  size  field
 *   0       4     magic ("EXLB", little-endian)
 *   4       2     format version
 *   6       2     reserved
 *   8       4     Exile OS version
 *   12      4     kernel image size
 *   16      32    kernel SHA-256
 *
 * All integer fields are little-endian.
 */
static constexpr u32 BOOT_MANIFEST_MAGIC = 0x45584C42; // "EXLB"
static constexpr u16 BOOT_MANIFEST_VERSION = 1;

struct BootManifest {
    u32 magic { BOOT_MANIFEST_MAGIC };
    u16 version { BOOT_MANIFEST_VERSION };
    u16 reserved { 0 };

    u32 os_version { 1 };
    u32 kernel_size { 0 };

    u8 kernel_sha256[32] {};
};

static_assert(sizeof(BootManifest) == 48);
static_assert(offsetof(BootManifest, kernel_sha256) == 16);

}
