#pragma once

#include <AK/Types.h>

namespace Exile::Boot {

struct BootMeasurement {
    u8 digest[32] {};
};

void sha256(void const* data, size_t size, BootMeasurement& measurement);

bool verify_kernel_integrity(void const* kernel_data, size_t kernel_size);

}
