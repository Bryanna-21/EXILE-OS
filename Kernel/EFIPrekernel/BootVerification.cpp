/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <Kernel/EFIPrekernel/BootVerification.h>
#include <Kernel/EFIPrekernel/Panic.h>
#include <LibCrypto/Curves/Ed25519.h>

extern "C" u8 start_of_boot_manifest[];
extern "C" u8 end_of_boot_manifest[];

// The Ed25519 implementation can generate keys, which needs randomness. The prekernel only ever verifies,
// so reaching this would be a bug: refuse instead of returning predictable bytes.
extern "C" void arc4random_buf(void*, size_t);
extern "C" void arc4random_buf(void*, size_t)
{
    VERIFY_NOT_REACHED();
}

namespace Kernel {

static bool ed25519_verify(u8 const* public_key, u8 const* signature, u8 const* message, size_t message_size)
{
    return Crypto::Curves::Ed25519::verify(
        ReadonlyBytes { public_key, Exile::Boot::ED25519_PUBLIC_KEY_SIZE },
        ReadonlyBytes { signature, Exile::Boot::ED25519_SIGNATURE_SIZE },
        ReadonlyBytes { message, message_size });
}

void verify_kernel_image_or_panic(ReadonlyBytes kernel_image)
{
    auto manifest_size = static_cast<size_t>(end_of_boot_manifest - start_of_boot_manifest);
    auto trust_anchors = boot_trust_anchors();

    auto status = Exile::Boot::verify_kernel_integrity(
        kernel_image.data(), kernel_image.size(),
        start_of_boot_manifest, manifest_size,
        trust_anchors.data(), trust_anchors.size(),
        ed25519_verify);

    if (status != Exile::Boot::VerifyStatus::Ok)
        PANIC("Kernel image verification failed: {}", Exile::Boot::verify_status_to_string(status));

    dbgln("Kernel image verified against the signed boot manifest ({} bytes).", kernel_image.size());
}

}
