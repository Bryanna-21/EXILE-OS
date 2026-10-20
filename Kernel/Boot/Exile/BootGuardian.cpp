/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <Kernel/Boot/Exile/BootGuardian.h>

namespace Exile::Boot {

static constexpr u32 RoundConstants[64] {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152,
    0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3,
    0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354,
    0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1,
    0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
    0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3,
    0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee,
    0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa,
    0xa4506ceb, 0xbef9a3f7, 0xc67178f2
};

static constexpr u32 InitialState[8] {
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19
};

static constexpr u32 rotr(u32 value, u32 bits)
{
    return (value >> bits) | (value << (32 - bits));
}

static constexpr u32 choose(u32 x, u32 y, u32 z)
{
    return (x & y) ^ (~x & z);
}

static constexpr u32 majority(u32 x, u32 y, u32 z)
{
    return (x & y) ^ (x & z) ^ (y & z);
}

static constexpr u32 big_sigma0(u32 x)
{
    return rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22);
}

static constexpr u32 big_sigma1(u32 x)
{
    return rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25);
}

static constexpr u32 small_sigma0(u32 x)
{
    return rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3);
}

static constexpr u32 small_sigma1(u32 x)
{
    return rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10);
}

static void transform(u32 state[8], u8 const* block)
{
    u32 schedule[64];

    for (size_t i = 0; i < 16; ++i) {
        auto offset = i * 4;
        schedule[i] = (static_cast<u32>(block[offset]) << 24)
            | (static_cast<u32>(block[offset + 1]) << 16)
            | (static_cast<u32>(block[offset + 2]) << 8)
            | static_cast<u32>(block[offset + 3]);
    }

    for (size_t i = 16; i < 64; ++i)
        schedule[i] = small_sigma1(schedule[i - 2]) + schedule[i - 7]
            + small_sigma0(schedule[i - 15]) + schedule[i - 16];

    u32 a = state[0];
    u32 b = state[1];
    u32 c = state[2];
    u32 d = state[3];
    u32 e = state[4];
    u32 f = state[5];
    u32 g = state[6];
    u32 h = state[7];

    for (size_t i = 0; i < 64; ++i) {
        auto t1 = h + big_sigma1(e) + choose(e, f, g) + RoundConstants[i] + schedule[i];
        auto t2 = big_sigma0(a) + majority(a, b, c);

        h = g;
        g = f;
        f = e;
        e = d + t1;
        d = c;
        c = b;
        b = a;
        a = t1 + t2;
    }

    state[0] += a;
    state[1] += b;
    state[2] += c;
    state[3] += d;
    state[4] += e;
    state[5] += f;
    state[6] += g;
    state[7] += h;
}

void sha256(void const* data, size_t size, BootMeasurement& measurement)
{
    u32 state[8];
    for (size_t i = 0; i < 8; ++i)
        state[i] = InitialState[i];

    auto bytes = static_cast<u8 const*>(data);
    auto full_blocks = size / 64;

    for (size_t i = 0; i < full_blocks; ++i)
        transform(state, bytes + i * 64);

    u8 block[128] {};
    auto remaining = size % 64;

    for (size_t i = 0; i < remaining; ++i)
        block[i] = bytes[full_blocks * 64 + i];

    block[remaining] = 0x80;

    auto final_size = remaining < 56 ? 64 : 128;
    auto bit_length = static_cast<u64>(size) * 8;

    for (size_t i = 0; i < 8; ++i)
        block[final_size - 1 - i] = static_cast<u8>(bit_length >> (i * 8));

    transform(state, block);

    if (final_size == 128)
        transform(state, block + 64);

    for (size_t i = 0; i < 8; ++i) {
        measurement.digest[i * 4] = static_cast<u8>(state[i] >> 24);
        measurement.digest[i * 4 + 1] = static_cast<u8>(state[i] >> 16);
        measurement.digest[i * 4 + 2] = static_cast<u8>(state[i] >> 8);
        measurement.digest[i * 4 + 3] = static_cast<u8>(state[i]);
    }
}

char const* verify_status_to_string(VerifyStatus status)
{
    switch (status) {
    case VerifyStatus::Ok:
        return "ok";
    case VerifyStatus::InvalidArgument:
        return "invalid argument";
    case VerifyStatus::MissingManifest:
        return "boot manifest is missing";
    case VerifyStatus::MalformedManifest:
        return "boot manifest is malformed";
    case VerifyStatus::UnsupportedVersion:
        return "boot manifest version is not supported";
    case VerifyStatus::UnsupportedAlgorithm:
        return "boot manifest uses an unsupported algorithm";
    case VerifyStatus::NoTrustAnchor:
        return "no trust anchor is configured";
    case VerifyStatus::UnknownKey:
        return "boot manifest is signed by an unknown key";
    case VerifyStatus::BadSignature:
        return "boot manifest signature is invalid";
    case VerifyStatus::SizeMismatch:
        return "kernel image size does not match the boot manifest";
    case VerifyStatus::DigestMismatch:
        return "kernel image digest does not match the boot manifest";
    }
    return "unknown verification status";
}

void compute_key_id(u8 const (&public_key)[ED25519_PUBLIC_KEY_SIZE], u8 (&key_id)[KEY_ID_SIZE])
{
    BootMeasurement measurement;
    sha256(public_key, sizeof(public_key), measurement);
    for (size_t i = 0; i < KEY_ID_SIZE; ++i)
        key_id[i] = measurement.digest[i];
}

// Compares without an early exit, so the running time does not depend on where the inputs differ.
static bool equal_constant_time(u8 const* a, u8 const* b, size_t size)
{
    u8 difference = 0;
    for (size_t i = 0; i < size; ++i)
        difference |= a[i] ^ b[i];
    return difference == 0;
}

VerifyStatus parse_manifest(u8 const* data, size_t size, BootManifest& manifest)
{
    if (data == nullptr || size == 0)
        return VerifyStatus::MissingManifest;

    // Exact length only: truncated input would read out of bounds and trailing bytes would make the encoding ambiguous.
    if (size != MANIFEST_SIZE)
        return VerifyStatus::MalformedManifest;

    for (size_t i = 0; i < sizeof(MANIFEST_MAGIC); ++i) {
        if (data[i] != MANIFEST_MAGIC[i])
            return VerifyStatus::MalformedManifest;
    }

    BootManifest parsed;
    parsed.version = static_cast<u16>(Detail::load_le(data + 4, 2));
    parsed.flags = static_cast<u16>(Detail::load_le(data + 6, 2));
    parsed.hash_algorithm = static_cast<u16>(Detail::load_le(data + 8, 2));
    parsed.signature_algorithm = static_cast<u16>(Detail::load_le(data + 10, 2));
    parsed.os_version = static_cast<u32>(Detail::load_le(data + 12, 4));
    parsed.kernel_size = Detail::load_le(data + 16, 8);

    if (parsed.version != MANIFEST_VERSION)
        return VerifyStatus::UnsupportedVersion;

    // No flags are defined. Rejecting unknown bits keeps a future flag from being silently ignored by this verifier.
    if (parsed.flags != 0)
        return VerifyStatus::MalformedManifest;

    if (parsed.hash_algorithm != HASH_ALGORITHM_SHA256 || parsed.signature_algorithm != SIGNATURE_ALGORITHM_ED25519)
        return VerifyStatus::UnsupportedAlgorithm;

    // An empty kernel image is never valid.
    if (parsed.kernel_size == 0)
        return VerifyStatus::MalformedManifest;

    for (size_t i = 0; i < KEY_ID_SIZE; ++i)
        parsed.key_id[i] = data[24 + i];
    for (size_t i = 0; i < SHA256_DIGEST_SIZE; ++i)
        parsed.kernel_sha256[i] = data[32 + i];
    for (size_t i = 0; i < ED25519_SIGNATURE_SIZE; ++i)
        parsed.signature[i] = data[MANIFEST_SIGNATURE_OFFSET + i];

    manifest = parsed;
    return VerifyStatus::Ok;
}

VerifyStatus verify_kernel_integrity(
    u8 const* kernel, size_t kernel_size,
    u8 const* manifest_bytes, size_t manifest_size,
    TrustAnchor const* trust_anchors, size_t trust_anchor_count,
    SignatureVerifyFunction verify_signature)
{
    if (kernel == nullptr || kernel_size == 0 || verify_signature == nullptr)
        return VerifyStatus::InvalidArgument;

    // With nothing to trust, nothing can be authenticated. Fail closed instead of treating this as "unchecked".
    if (trust_anchors == nullptr || trust_anchor_count == 0)
        return VerifyStatus::NoTrustAnchor;

    BootManifest manifest;
    if (auto status = parse_manifest(manifest_bytes, manifest_size, manifest); status != VerifyStatus::Ok)
        return status;

    // The signed region is re-serialized from the parsed fields. parse_manifest accepts only one encoding of
    // every field, so this is byte-for-byte what was in the input.
    u8 signed_region[MANIFEST_SIGNED_SIZE];
    serialize_signed_region(manifest, signed_region);
    u8 message[SIGNING_MESSAGE_SIZE];
    build_signing_message(signed_region, message);

    // Authenticate the manifest before trusting any value inside it.
    bool key_known = false;
    bool signature_valid = false;
    for (size_t i = 0; i < trust_anchor_count; ++i) {
        u8 anchor_key_id[KEY_ID_SIZE];
        compute_key_id(trust_anchors[i].public_key, anchor_key_id);
        if (!equal_constant_time(anchor_key_id, manifest.key_id, KEY_ID_SIZE))
            continue;
        key_known = true;
        if (verify_signature(trust_anchors[i].public_key, manifest.signature, message, sizeof(message))) {
            signature_valid = true;
            break;
        }
    }
    if (!key_known)
        return VerifyStatus::UnknownKey;
    if (!signature_valid)
        return VerifyStatus::BadSignature;

    if (manifest.kernel_size != static_cast<u64>(kernel_size))
        return VerifyStatus::SizeMismatch;

    BootMeasurement measurement;
    sha256(kernel, kernel_size, measurement);
    if (!equal_constant_time(measurement.digest, manifest.kernel_sha256, SHA256_DIGEST_SIZE))
        return VerifyStatus::DigestMismatch;

    return VerifyStatus::Ok;
}

}
