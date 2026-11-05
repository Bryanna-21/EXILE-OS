/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#include <AK/ByteBuffer.h>
#include <AK/StringView.h>
#include <AK/Vector.h>
#include <Kernel/Boot/Exile/BootGuardian.h>
#include <LibCrypto/Curves/Ed25519.h>
#include <LibTest/TestCase.h>

using namespace Exile::Boot;

namespace {

bool ed25519_verify(u8 const* public_key, u8 const* signature, u8 const* message, size_t message_size)
{
    return Crypto::Curves::Ed25519::verify(
        ReadonlyBytes { public_key, ED25519_PUBLIC_KEY_SIZE },
        ReadonlyBytes { signature, ED25519_SIGNATURE_SIZE },
        ReadonlyBytes { message, message_size });
}

struct KeyPair {
    Array<u8, 32> seed {};
    TrustAnchor anchor {};
};

KeyPair make_key_pair(u8 fill)
{
    KeyPair pair;
    pair.seed.fill(fill);
    auto public_key = MUST(Crypto::Curves::Ed25519::generate_public_key(pair.seed.span()));
    VERIFY(public_key.size() == ED25519_PUBLIC_KEY_SIZE);
    __builtin_memcpy(pair.anchor.public_key, public_key.data(), ED25519_PUBLIC_KEY_SIZE);
    return pair;
}

// Copies a signature out of the ByteBuffer returned by Ed25519::sign() one byte at a time, with a size check.
void set_signature(BootManifest& manifest, ByteBuffer const& signature)
{
    VERIFY(signature.size() == ED25519_SIGNATURE_SIZE);
    for (size_t i = 0; i < ED25519_SIGNATURE_SIZE; ++i)
        manifest.signature[i] = signature[i];
}

Vector<u8> make_kernel(size_t size)
{
    Vector<u8> kernel;
    for (size_t i = 0; i < size; ++i)
        kernel.append(static_cast<u8>((i * 131 + 7) & 0xff));
    return kernel;
}

BootManifest describe_kernel(KeyPair const& pair, Vector<u8> const& kernel)
{
    BootManifest manifest;
    manifest.kernel_size = kernel.size();
    BootMeasurement measurement;
    sha256(kernel.data(), kernel.size(), measurement);
    __builtin_memcpy(manifest.kernel_sha256, measurement.digest, SHA256_DIGEST_SIZE);
    compute_key_id(pair.anchor.public_key, manifest.key_id);
    return manifest;
}

// Signs the manifest exactly as the host tool does, over SIGNING_DOMAIN || signed region.
void sign_manifest(KeyPair const& pair, BootManifest& manifest)
{
    u8 signed_region[MANIFEST_SIGNED_SIZE];
    serialize_signed_region(manifest, signed_region);
    u8 message[SIGNING_MESSAGE_SIZE];
    build_signing_message(signed_region, message);
    auto signature = MUST(Crypto::Curves::Ed25519::sign(
        ReadonlyBytes { pair.anchor.public_key, ED25519_PUBLIC_KEY_SIZE },
        pair.seed.span(),
        ReadonlyBytes { message, sizeof(message) }));
    set_signature(manifest, signature);
}

Array<u8, MANIFEST_SIZE> make_signed_manifest(KeyPair const& pair, Vector<u8> const& kernel)
{
    auto manifest = describe_kernel(pair, kernel);
    sign_manifest(pair, manifest);
    Array<u8, MANIFEST_SIZE> serialized;
    serialize_manifest(manifest, *reinterpret_cast<u8(*)[MANIFEST_SIZE]>(serialized.data()));
    return serialized;
}

VerifyStatus check(Vector<u8> const& kernel, ReadonlyBytes manifest, TrustAnchor const& anchor)
{
    return verify_kernel_integrity(kernel.data(), kernel.size(), manifest.data(), manifest.size(), &anchor, 1, ed25519_verify);
}

}

TEST_CASE(sha256_known_answer_vectors)
{
    // Hex-encodes a digest into a fixed buffer so the comparison does not depend on any string class.
    struct Hex {
        char text[SHA256_DIGEST_SIZE * 2] {};
        explicit Hex(BootMeasurement const& m)
        {
            static constexpr char digits[] = "0123456789abcdef";
            for (size_t i = 0; i < SHA256_DIGEST_SIZE; ++i) {
                text[i * 2] = digits[m.digest[i] >> 4];
                text[i * 2 + 1] = digits[m.digest[i] & 0xf];
            }
        }
        StringView view() const { return { text, sizeof(text) }; }
    };

    BootMeasurement empty;
    sha256("", 0, empty);
    EXPECT_EQ(Hex(empty).view(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"sv);

    BootMeasurement abc;
    sha256("abc", 3, abc);
    EXPECT_EQ(Hex(abc).view(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"sv);

    // 56 bytes: the padding spills into a second block.
    auto message = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"sv;
    BootMeasurement two_block;
    sha256(message.characters_without_null_termination(), message.length(), two_block);
    EXPECT_EQ(Hex(two_block).view(), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"sv);
}

TEST_CASE(manifest_encoding_is_explicit)
{
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(1000);
    auto serialized = make_signed_manifest(pair, kernel);

    // The magic is the byte sequence "EXLB", independent of host endianness.
    EXPECT_EQ(serialized[0], 'E');
    EXPECT_EQ(serialized[1], 'X');
    EXPECT_EQ(serialized[2], 'L');
    EXPECT_EQ(serialized[3], 'B');
    EXPECT_EQ(serialized.size(), 128u);

    BootManifest parsed;
    EXPECT_EQ(parse_manifest(serialized.data(), serialized.size(), parsed), VerifyStatus::Ok);
    EXPECT_EQ(parsed.version, MANIFEST_VERSION);
    EXPECT_EQ(parsed.kernel_size, 1000u);
}

TEST_CASE(valid_manifest_is_accepted)
{
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(70000);
    auto manifest = make_signed_manifest(pair, kernel);
    EXPECT_EQ(check(kernel, manifest.span(), pair.anchor), VerifyStatus::Ok);
}

TEST_CASE(modified_kernel_is_rejected)
{
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(70000);
    auto manifest = make_signed_manifest(pair, kernel);

    // Flip a bit at the start, middle and end.
    for (size_t index : { size_t { 0 }, kernel.size() / 2, kernel.size() - 1 }) {
        auto tampered = kernel;
        tampered[index] ^= 0x01;
        EXPECT_EQ(check(tampered, manifest.span(), pair.anchor), VerifyStatus::DigestMismatch);
    }

    auto longer = kernel;
    longer.append(0);
    EXPECT_EQ(check(longer, manifest.span(), pair.anchor), VerifyStatus::SizeMismatch);

    auto shorter = kernel;
    shorter.take_last();
    EXPECT_EQ(check(shorter, manifest.span(), pair.anchor), VerifyStatus::SizeMismatch);
}

TEST_CASE(any_single_byte_change_to_the_manifest_is_rejected)
{
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(5000);
    auto manifest = make_signed_manifest(pair, kernel);

    for (size_t i = 0; i < manifest.size(); ++i) {
        auto tampered = manifest;
        tampered[i] ^= 0x01;
        EXPECT_NE(check(kernel, tampered.span(), pair.anchor), VerifyStatus::Ok);
    }
}

TEST_CASE(replacing_the_digest_without_resigning_is_rejected)
{
    // An attacker who swaps in a new kernel and its digest but cannot sign.
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(5000);
    auto evil_kernel = make_kernel(5000);
    evil_kernel[100] ^= 0xff;

    auto forged = describe_kernel(pair, evil_kernel);
    // Reuse the old signature.
    auto genuine = make_signed_manifest(pair, kernel);
    __builtin_memcpy(forged.signature, genuine.data() + MANIFEST_SIGNATURE_OFFSET, ED25519_SIGNATURE_SIZE);

    Array<u8, MANIFEST_SIZE> serialized;
    serialize_manifest(forged, *reinterpret_cast<u8(*)[MANIFEST_SIZE]>(serialized.data()));
    EXPECT_EQ(check(evil_kernel, serialized.span(), pair.anchor), VerifyStatus::BadSignature);
}

TEST_CASE(manifest_from_an_untrusted_key_is_rejected)
{
    auto trusted = make_key_pair(1);
    auto attacker = make_key_pair(2);
    auto kernel = make_kernel(5000);

    // A perfectly valid manifest, signed by someone who is not a trust anchor.
    auto manifest = make_signed_manifest(attacker, kernel);
    EXPECT_EQ(check(kernel, manifest.span(), trusted.anchor), VerifyStatus::UnknownKey);

    // The attacker claims the trusted key's id but signs with their own key.
    auto claimed = describe_kernel(trusted, kernel);
    sign_manifest(attacker, claimed);
    Array<u8, MANIFEST_SIZE> serialized;
    serialize_manifest(claimed, *reinterpret_cast<u8(*)[MANIFEST_SIZE]>(serialized.data()));
    EXPECT_EQ(check(kernel, serialized.span(), trusted.anchor), VerifyStatus::BadSignature);
}

TEST_CASE(key_rotation_with_multiple_trust_anchors)
{
    auto old_key = make_key_pair(1);
    auto new_key = make_key_pair(2);
    auto kernel = make_kernel(5000);
    auto manifest = make_signed_manifest(new_key, kernel);

    TrustAnchor both[] = { old_key.anchor, new_key.anchor };
    EXPECT_EQ(verify_kernel_integrity(kernel.data(), kernel.size(), manifest.data(), manifest.size(), both, 2, ed25519_verify), VerifyStatus::Ok);

    // Once the new key is retired, manifests it signed no longer verify.
    EXPECT_EQ(verify_kernel_integrity(kernel.data(), kernel.size(), manifest.data(), manifest.size(), both, 1, ed25519_verify), VerifyStatus::UnknownKey);
}

TEST_CASE(malformed_and_missing_manifests_fail_closed)
{
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(5000);
    auto manifest = make_signed_manifest(pair, kernel);

    EXPECT_EQ(check(kernel, ReadonlyBytes {}, pair.anchor), VerifyStatus::MissingManifest);
    EXPECT_EQ(verify_kernel_integrity(kernel.data(), kernel.size(), nullptr, 0, &pair.anchor, 1, ed25519_verify), VerifyStatus::MissingManifest);

    // Truncated by one byte, and with a trailing byte.
    EXPECT_EQ(check(kernel, manifest.span().slice(0, MANIFEST_SIZE - 1), pair.anchor), VerifyStatus::MalformedManifest);
    Vector<u8> extended;
    extended.append(manifest.data(), manifest.size());
    extended.append(0);
    EXPECT_EQ(check(kernel, extended.span(), pair.anchor), VerifyStatus::MalformedManifest);

    // The old unsigned 48-byte v1 layout.
    Array<u8, 48> v1 {};
    EXPECT_EQ(check(kernel, v1.span(), pair.anchor), VerifyStatus::MalformedManifest);
}

TEST_CASE(unsupported_fields_are_rejected_even_when_validly_signed)
{
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(5000);

    auto expect_status = [&](auto mutate, VerifyStatus expected) {
        auto manifest = describe_kernel(pair, kernel);
        mutate(manifest);
        sign_manifest(pair, manifest);
        Array<u8, MANIFEST_SIZE> serialized;
        serialize_manifest(manifest, *reinterpret_cast<u8(*)[MANIFEST_SIZE]>(serialized.data()));
        EXPECT_EQ(check(kernel, serialized.span(), pair.anchor), expected);
    };

    expect_status([](BootManifest& m) { m.version = 3; }, VerifyStatus::UnsupportedVersion);
    expect_status([](BootManifest& m) { m.hash_algorithm = 2; }, VerifyStatus::UnsupportedAlgorithm);
    expect_status([](BootManifest& m) { m.signature_algorithm = 2; }, VerifyStatus::UnsupportedAlgorithm);
    expect_status([](BootManifest& m) { m.flags = 1; }, VerifyStatus::MalformedManifest);
    expect_status([](BootManifest& m) { m.kernel_size = 0; }, VerifyStatus::MalformedManifest);
    // A huge declared size must be a mismatch, never an out-of-range read.
    expect_status([](BootManifest& m) { m.kernel_size = 0xFFFFFFFFFFFFFFFFull; }, VerifyStatus::SizeMismatch);
}

TEST_CASE(signature_without_domain_separation_is_rejected)
{
    // A signature over the bare signed region (no SIGNING_DOMAIN prefix) must not be accepted.
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(5000);
    auto manifest = describe_kernel(pair, kernel);

    u8 signed_region[MANIFEST_SIGNED_SIZE];
    serialize_signed_region(manifest, signed_region);
    auto signature = MUST(Crypto::Curves::Ed25519::sign(
        ReadonlyBytes { pair.anchor.public_key, ED25519_PUBLIC_KEY_SIZE },
        pair.seed.span(),
        ReadonlyBytes { signed_region, sizeof(signed_region) }));
    set_signature(manifest, signature);

    Array<u8, MANIFEST_SIZE> serialized;
    serialize_manifest(manifest, *reinterpret_cast<u8(*)[MANIFEST_SIZE]>(serialized.data()));
    EXPECT_EQ(check(kernel, serialized.span(), pair.anchor), VerifyStatus::BadSignature);
}

TEST_CASE(missing_inputs_fail_closed)
{
    auto pair = make_key_pair(1);
    auto kernel = make_kernel(5000);
    auto manifest = make_signed_manifest(pair, kernel);

    // No trust anchor means nothing can be authenticated: that is a failure, not a pass.
    EXPECT_EQ(verify_kernel_integrity(kernel.data(), kernel.size(), manifest.data(), manifest.size(), nullptr, 0, ed25519_verify), VerifyStatus::NoTrustAnchor);
    EXPECT_EQ(verify_kernel_integrity(kernel.data(), kernel.size(), manifest.data(), manifest.size(), &pair.anchor, 0, ed25519_verify), VerifyStatus::NoTrustAnchor);

    EXPECT_EQ(verify_kernel_integrity(nullptr, 0, manifest.data(), manifest.size(), &pair.anchor, 1, ed25519_verify), VerifyStatus::InvalidArgument);
    EXPECT_EQ(verify_kernel_integrity(kernel.data(), kernel.size(), manifest.data(), manifest.size(), &pair.anchor, 1, nullptr), VerifyStatus::InvalidArgument);
}
