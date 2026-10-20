/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

/*
 * Host tool to create, sign and verify Exile boot manifests.
 */

#include <AK/ByteBuffer.h>
#include <Kernel/Boot/Exile/BootGuardian.h>
#include <LibCrypto/Curves/Ed25519.h>

#include <cstdio>
#include <cstring>
#include <fstream>
#include <string>
#include <vector>

using namespace Exile::Boot;

namespace {

// Exit codes: 0 success, 1 usage or I/O error, 2 the manifest did not verify.
constexpr int ExitUsage = 1;
constexpr int ExitVerificationFailed = 2;

bool read_file(char const* path, std::vector<u8>& data)
{
    std::ifstream input(path, std::ios::binary | std::ios::ate);
    if (!input)
        return false;
    auto size = input.tellg();
    if (size < 0)
        return false;
    data.resize(static_cast<size_t>(size));
    input.seekg(0);
    if (!data.empty())
        input.read(reinterpret_cast<char*>(data.data()), static_cast<std::streamsize>(data.size()));
    return static_cast<bool>(input) || data.empty();
}

bool write_file(char const* path, u8 const* data, size_t size)
{
    std::ofstream output(path, std::ios::binary | std::ios::trunc);
    if (!output)
        return false;
    output.write(reinterpret_cast<char const*>(data), static_cast<std::streamsize>(size));
    return static_cast<bool>(output);
}

// Key files are raw: exactly 32 bytes (an Ed25519 seed or public key, RFC 8032).
bool read_key_file(char const* path, char const* what, u8 (&key)[32])
{
    std::vector<u8> data;
    if (!read_file(path, data)) {
        std::fprintf(stderr, "failed to read %s: %s\n", what, path);
        return false;
    }
    if (data.size() != 32) {
        std::fprintf(stderr, "%s must be exactly 32 raw bytes, got %zu: %s\n", what, data.size(), path);
        return false;
    }
    std::memcpy(key, data.data(), 32);
    return true;
}

bool ed25519_verify(u8 const* public_key, u8 const* signature, u8 const* message, size_t message_size)
{
    return Crypto::Curves::Ed25519::verify(
        ReadonlyBytes { public_key, ED25519_PUBLIC_KEY_SIZE },
        ReadonlyBytes { signature, ED25519_SIGNATURE_SIZE },
        ReadonlyBytes { message, message_size });
}

void print_hex(char const* label, u8 const* data, size_t size)
{
    std::printf("%s", label);
    for (size_t i = 0; i < size; ++i)
        std::printf("%02x", data[i]);
    std::printf("\n");
}

int usage(char const* argv0)
{
    std::fprintf(stderr,
        "usage:\n"
        "  %s keygen <seed-out> <pubkey-out>\n"
        "  %s sign <kernel> <seed-file> <manifest-out> [os-version]\n"
        "  %s verify <kernel> <manifest> <pubkey-file>\n"
        "\n"
        "Key files are raw 32-byte Ed25519 values. Keep the seed offline: it is the signing authority.\n"
        "Exit status: 0 ok, 1 usage or I/O error, 2 verification failed.\n",
        argv0, argv0, argv0);
    return ExitUsage;
}

int keygen(char const* seed_path, char const* pubkey_path)
{
    auto seed = Crypto::Curves::Ed25519::generate_private_key();
    if (seed.is_error() || seed.value().size() != 32) {
        std::fprintf(stderr, "failed to generate a private key\n");
        return ExitUsage;
    }
    auto pubkey = Crypto::Curves::Ed25519::generate_public_key(seed.value());
    if (pubkey.is_error() || pubkey.value().size() != 32) {
        std::fprintf(stderr, "failed to derive the public key\n");
        return ExitUsage;
    }
    if (!write_file(seed_path, seed.value().data(), 32) || !write_file(pubkey_path, pubkey.value().data(), 32)) {
        std::fprintf(stderr, "failed to write key files\n");
        return ExitUsage;
    }
    std::printf("wrote seed (keep offline): %s\nwrote public key: %s\n", seed_path, pubkey_path);
    return 0;
}

int sign(char const* kernel_path, char const* seed_path, char const* manifest_path, u32 os_version)
{
    std::vector<u8> kernel;
    if (!read_file(kernel_path, kernel)) {
        std::fprintf(stderr, "failed to read kernel: %s\n", kernel_path);
        return ExitUsage;
    }
    if (kernel.empty()) {
        std::fprintf(stderr, "kernel image is empty\n");
        return ExitUsage;
    }

    u8 seed[32];
    if (!read_key_file(seed_path, "signing seed", seed))
        return ExitUsage;

    auto public_key = Crypto::Curves::Ed25519::generate_public_key(ReadonlyBytes { seed, sizeof(seed) });
    if (public_key.is_error() || public_key.value().size() != 32) {
        std::fprintf(stderr, "failed to derive the public key from the seed\n");
        return ExitUsage;
    }

    BootManifest manifest;
    manifest.os_version = os_version;
    manifest.kernel_size = kernel.size();

    BootMeasurement measurement;
    sha256(kernel.data(), kernel.size(), measurement);
    std::memcpy(manifest.kernel_sha256, measurement.digest, sizeof(manifest.kernel_sha256));

    u8 public_key_bytes[ED25519_PUBLIC_KEY_SIZE];
    std::memcpy(public_key_bytes, public_key.value().data(), sizeof(public_key_bytes));
    compute_key_id(public_key_bytes, manifest.key_id);

    u8 signed_region[MANIFEST_SIGNED_SIZE];
    serialize_signed_region(manifest, signed_region);
    u8 message[SIGNING_MESSAGE_SIZE];
    build_signing_message(signed_region, message);

    auto signature = Crypto::Curves::Ed25519::sign(
        ReadonlyBytes { public_key_bytes, sizeof(public_key_bytes) },
        ReadonlyBytes { seed, sizeof(seed) },
        ReadonlyBytes { message, sizeof(message) });
    if (signature.is_error() || signature.value().size() != ED25519_SIGNATURE_SIZE) {
        std::fprintf(stderr, "failed to sign the manifest\n");
        return ExitUsage;
    }
    std::memcpy(manifest.signature, signature.value().data(), sizeof(manifest.signature));

    u8 serialized[MANIFEST_SIZE];
    serialize_manifest(manifest, serialized);

    // Never write a manifest that this tool's own verifier would reject.
    TrustAnchor anchor;
    std::memcpy(anchor.public_key, public_key_bytes, sizeof(anchor.public_key));
    auto status = verify_kernel_integrity(kernel.data(), kernel.size(), serialized, sizeof(serialized), &anchor, 1, ed25519_verify);
    if (status != VerifyStatus::Ok) {
        std::fprintf(stderr, "internal error: freshly signed manifest does not verify: %s\n", verify_status_to_string(status));
        return ExitUsage;
    }

    if (!write_file(manifest_path, serialized, sizeof(serialized))) {
        std::fprintf(stderr, "failed to write manifest: %s\n", manifest_path);
        return ExitUsage;
    }

    std::printf("Exile Boot Manifest v%u\n", static_cast<unsigned>(manifest.version));
    std::printf("Kernel size: %llu bytes\n", static_cast<unsigned long long>(manifest.kernel_size));
    print_hex("Kernel SHA-256: ", manifest.kernel_sha256, sizeof(manifest.kernel_sha256));
    print_hex("Signing key id: ", manifest.key_id, sizeof(manifest.key_id));
    return 0;
}

int verify(char const* kernel_path, char const* manifest_path, char const* pubkey_path)
{
    std::vector<u8> kernel;
    std::vector<u8> manifest;
    if (!read_file(kernel_path, kernel)) {
        std::fprintf(stderr, "failed to read kernel: %s\n", kernel_path);
        return ExitUsage;
    }
    if (!read_file(manifest_path, manifest)) {
        std::fprintf(stderr, "failed to read manifest: %s\n", manifest_path);
        return ExitUsage;
    }
    TrustAnchor anchor;
    if (!read_key_file(pubkey_path, "public key", anchor.public_key))
        return ExitUsage;

    auto status = verify_kernel_integrity(
        kernel.data(), kernel.size(),
        manifest.data(), manifest.size(),
        &anchor, 1, ed25519_verify);

    if (status != VerifyStatus::Ok) {
        std::fprintf(stderr, "REJECTED: %s\n", verify_status_to_string(status));
        return ExitVerificationFailed;
    }
    std::printf("OK: kernel image matches the signed manifest\n");
    return 0;
}

}

int main(int argc, char** argv)
{
    if (argc >= 2 && std::strcmp(argv[1], "keygen") == 0 && argc == 4)
        return keygen(argv[2], argv[3]);

    if (argc >= 2 && std::strcmp(argv[1], "sign") == 0 && (argc == 5 || argc == 6)) {
        u32 os_version = 1;
        if (argc == 6) {
            char* end = nullptr;
            auto parsed = std::strtoul(argv[5], &end, 10);
            if (end == argv[5] || *end != '\0' || parsed > 0xFFFFFFFFul) {
                std::fprintf(stderr, "invalid os-version: %s\n", argv[5]);
                return ExitUsage;
            }
            os_version = static_cast<u32>(parsed);
        }
        return sign(argv[2], argv[3], argv[4], os_version);
    }

    if (argc >= 2 && std::strcmp(argv[1], "verify") == 0 && argc == 5)
        return verify(argv[2], argv[3], argv[4]);

    return usage(argv[0]);
}
