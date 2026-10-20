/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

/*
 * Exile Boot Guardian: authenticated kernel-image verification.
 */

#pragma once

#include <AK/Types.h>
#include <Kernel/Boot/Exile/BootManifest.h>

namespace Exile::Boot {

struct BootMeasurement {
    u8 digest[SHA256_DIGEST_SIZE] {};
};

// A public key that is allowed to sign boot manifests. This is the trust anchor: it must be compiled
// into code that is itself authenticated by the platform (for example a UEFI Secure Boot signed image).
// A key that is stored next to the kernel it is supposed to authenticate is not a trust anchor.
struct TrustAnchor {
    u8 public_key[ED25519_PUBLIC_KEY_SIZE] {};
};

// Verifies an Ed25519 signature. Must return true only if the signature is valid for the message.
using SignatureVerifyFunction = bool (*)(u8 const* public_key, u8 const* signature, u8 const* message, size_t message_size);

enum class VerifyStatus : u8 {
    Ok,
    InvalidArgument,
    MissingManifest,
    MalformedManifest,
    UnsupportedVersion,
    UnsupportedAlgorithm,
    NoTrustAnchor,
    UnknownKey,
    BadSignature,
    SizeMismatch,
    DigestMismatch,
};

char const* verify_status_to_string(VerifyStatus);

void sha256(void const* data, size_t size, BootMeasurement& measurement);

// Key id of a public key: the first KEY_ID_SIZE bytes of its SHA-256 digest.
void compute_key_id(u8 const (&public_key)[ED25519_PUBLIC_KEY_SIZE], u8 (&key_id)[KEY_ID_SIZE]);

// Strictly decodes the serialized manifest. This checks structure only; it does NOT authenticate anything.
// The input must be exactly MANIFEST_SIZE bytes: truncated and over-long inputs are rejected.
VerifyStatus parse_manifest(u8 const* data, size_t size, BootManifest& manifest);

// Verifies that `kernel` is exactly the image that an authorized signer approved. The result is Ok only if
//   1. the manifest is present, exactly MANIFEST_SIZE bytes, and uses the supported version and algorithms,
//   2. its key id selects a trust anchor, and that anchor's signature over the manifest is valid,
//   3. the manifest's kernel size equals kernel_size, and
//   4. SHA-256 over all kernel_size bytes equals the signed digest.
// Every other outcome is a failure. Callers must not boot the kernel unless this returns VerifyStatus::Ok.
VerifyStatus verify_kernel_integrity(
    u8 const* kernel, size_t kernel_size,
    u8 const* manifest, size_t manifest_size,
    TrustAnchor const* trust_anchors, size_t trust_anchor_count,
    SignatureVerifyFunction verify_signature);

}
