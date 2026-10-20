/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

/*
 * Exile Boot Manifest v2
 */

#pragma once

#include <AK/Types.h>

namespace Exile::Boot {

/*
 * Wire format. All multi-byte integers are little-endian and the layout has no padding.
 * The manifest is parsed and serialized field by field (never by casting the buffer to a struct),
 * so the encoding is identical on every host and compiler.
 *
 *   offset  size  field
 *   0       4     magic: the bytes 'E' 'X' 'L' 'B'
 *   4       2     format version (2)
 *   6       2     flags (must be 0)
 *   8       2     hash algorithm (1 = SHA-256)
 *   10      2     signature algorithm (1 = Ed25519)
 *   12      4     Exile OS version
 *   16      8     kernel image size in bytes
 *   24      8     key id: the first 8 bytes of SHA-256(signer public key)
 *   32      32    SHA-256 digest of the kernel image
 *   64      64    Ed25519 signature
 *
 * The signature covers SIGNING_DOMAIN followed by bytes [0, 64) of the manifest. The domain string
 * keeps a signature made for this purpose from being valid for any other message format.
 *
 * Version 1 of this format carried no signature and used an ambiguous magic value. It is not accepted.
 */
static constexpr size_t MANIFEST_SIZE = 128;
static constexpr size_t MANIFEST_SIGNED_SIZE = 64;
static constexpr size_t MANIFEST_SIGNATURE_OFFSET = 64;

static constexpr u8 MANIFEST_MAGIC[4] = { 'E', 'X', 'L', 'B' };
static constexpr u16 MANIFEST_VERSION = 2;

static constexpr u16 HASH_ALGORITHM_SHA256 = 1;
static constexpr u16 SIGNATURE_ALGORITHM_ED25519 = 1;

static constexpr size_t SHA256_DIGEST_SIZE = 32;
static constexpr size_t ED25519_PUBLIC_KEY_SIZE = 32;
static constexpr size_t ED25519_SIGNATURE_SIZE = 64;
static constexpr size_t KEY_ID_SIZE = 8;

// 22 characters plus a terminating zero byte, which is part of the signed message.
static constexpr u8 SIGNING_DOMAIN[23] = {
    'E', 'X', 'I', 'L', 'E', '-', 'B', 'O', 'O', 'T', '-', 'M', 'A', 'N', 'I', 'F', 'E', 'S', 'T', '-', 'V', '2', 0
};
static constexpr size_t SIGNING_MESSAGE_SIZE = sizeof(SIGNING_DOMAIN) + MANIFEST_SIGNED_SIZE;

struct BootManifest {
    u16 version { MANIFEST_VERSION };
    u16 flags { 0 };
    u16 hash_algorithm { HASH_ALGORITHM_SHA256 };
    u16 signature_algorithm { SIGNATURE_ALGORITHM_ED25519 };
    u32 os_version { 1 };
    u64 kernel_size { 0 };
    u8 key_id[KEY_ID_SIZE] {};
    u8 kernel_sha256[SHA256_DIGEST_SIZE] {};
    u8 signature[ED25519_SIGNATURE_SIZE] {};
};

namespace Detail {

inline void store_le(u8* destination, u64 value, size_t byte_count)
{
    for (size_t i = 0; i < byte_count; ++i)
        destination[i] = static_cast<u8>(value >> (8 * i));
}

inline u64 load_le(u8 const* source, size_t byte_count)
{
    u64 value = 0;
    for (size_t i = 0; i < byte_count; ++i)
        value |= static_cast<u64>(source[i]) << (8 * i);
    return value;
}

}

// Writes the fields covered by the signature, i.e. bytes [0, MANIFEST_SIGNED_SIZE).
inline void serialize_signed_region(BootManifest const& manifest, u8 (&out)[MANIFEST_SIGNED_SIZE])
{
    for (size_t i = 0; i < sizeof(MANIFEST_MAGIC); ++i)
        out[i] = MANIFEST_MAGIC[i];
    Detail::store_le(out + 4, manifest.version, 2);
    Detail::store_le(out + 6, manifest.flags, 2);
    Detail::store_le(out + 8, manifest.hash_algorithm, 2);
    Detail::store_le(out + 10, manifest.signature_algorithm, 2);
    Detail::store_le(out + 12, manifest.os_version, 4);
    Detail::store_le(out + 16, manifest.kernel_size, 8);
    for (size_t i = 0; i < KEY_ID_SIZE; ++i)
        out[24 + i] = manifest.key_id[i];
    for (size_t i = 0; i < SHA256_DIGEST_SIZE; ++i)
        out[32 + i] = manifest.kernel_sha256[i];
}

inline void serialize_manifest(BootManifest const& manifest, u8 (&out)[MANIFEST_SIZE])
{
    u8 signed_region[MANIFEST_SIGNED_SIZE];
    serialize_signed_region(manifest, signed_region);
    for (size_t i = 0; i < MANIFEST_SIGNED_SIZE; ++i)
        out[i] = signed_region[i];
    for (size_t i = 0; i < ED25519_SIGNATURE_SIZE; ++i)
        out[MANIFEST_SIGNATURE_OFFSET + i] = manifest.signature[i];
}

// Builds the exact message that is signed and verified: SIGNING_DOMAIN || signed region.
inline void build_signing_message(u8 const (&signed_region)[MANIFEST_SIGNED_SIZE], u8 (&out)[SIGNING_MESSAGE_SIZE])
{
    for (size_t i = 0; i < sizeof(SIGNING_DOMAIN); ++i)
        out[i] = SIGNING_DOMAIN[i];
    for (size_t i = 0; i < MANIFEST_SIGNED_SIZE; ++i)
        out[sizeof(SIGNING_DOMAIN) + i] = signed_region[i];
}

}
