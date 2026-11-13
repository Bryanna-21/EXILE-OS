/*
 * Copyright (c) 2026, the Exile OS developers
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

// Host-only check of the boot guardian: SHA-256 digests (compared against an independent implementation by
// host-guardian-check.sh) and the verification decision logic, using a fake signature verifier.
// This does not exercise Ed25519; Tests/ExileBoot/TestBootGuardian.cpp does.

#include <Kernel/Boot/Exile/BootGuardian.h>
#include <stdio.h>
#include <string.h>
#include <vector>
using namespace Exile::Boot;
static bool fake_verify(u8 const*, u8 const* sig, u8 const*, size_t) { return sig[0] == 0xAA; }
int main()
{
    // digests of patterns for sizes 0..300 are compared externally via sha256sum; emit them
    for (size_t n = 0; n <= 300; ++n) {
        std::vector<u8> d(n);
        for (size_t i = 0; i < n; ++i) d[i] = (u8)(i * 7 + 3);
        BootMeasurement m; sha256(d.data(), n, m);
        printf("%zu ", n);
        for (auto b : m.digest) printf("%02x", b);
        printf("\n");
    }
    // verification logic with a fake verifier
    std::vector<u8> k(1000, 0x42);
    TrustAnchor a; for (int i = 0; i < 32; ++i) a.public_key[i] = i;
    BootManifest bm; bm.kernel_size = k.size();
    compute_key_id(a.public_key, bm.key_id);
    BootMeasurement m; sha256(k.data(), k.size(), m);
    memcpy(bm.kernel_sha256, m.digest, 32); bm.signature[0] = 0xAA;
    u8 buf[MANIFEST_SIZE]; serialize_manifest(bm, buf);
    int fails = 0;
    auto expect = [&](char const* name, VerifyStatus got, VerifyStatus want) { if (got != want) { fprintf(stderr, "FAIL %s: %s\n", name, verify_status_to_string(got)); fails++; } else fprintf(stderr, "ok   %s\n", name); };
    expect("valid", verify_kernel_integrity(k.data(), k.size(), buf, sizeof buf, &a, 1, fake_verify), VerifyStatus::Ok);
    expect("no anchors", verify_kernel_integrity(k.data(), k.size(), buf, sizeof buf, nullptr, 0, fake_verify), VerifyStatus::NoTrustAnchor);
    expect("truncated", verify_kernel_integrity(k.data(), k.size(), buf, sizeof buf - 1, &a, 1, fake_verify), VerifyStatus::MalformedManifest);
    expect("overlong", verify_kernel_integrity(k.data(), k.size(), buf, sizeof buf + 1, &a, 1, fake_verify), VerifyStatus::MalformedManifest);
    expect("null manifest", verify_kernel_integrity(k.data(), k.size(), nullptr, 0, &a, 1, fake_verify), VerifyStatus::MissingManifest);
    expect("null verifier", verify_kernel_integrity(k.data(), k.size(), buf, sizeof buf, &a, 1, nullptr), VerifyStatus::InvalidArgument);
    u8 t[MANIFEST_SIZE]; memcpy(t, buf, sizeof t); t[64] = 0;
    expect("bad sig", verify_kernel_integrity(k.data(), k.size(), t, sizeof t, &a, 1, fake_verify), VerifyStatus::BadSignature);
    memcpy(t, buf, sizeof t); t[24] ^= 1;
    expect("unknown key", verify_kernel_integrity(k.data(), k.size(), t, sizeof t, &a, 1, fake_verify), VerifyStatus::UnknownKey);
    memcpy(t, buf, sizeof t); t[4] = 1;
    expect("version", verify_kernel_integrity(k.data(), k.size(), t, sizeof t, &a, 1, fake_verify), VerifyStatus::UnsupportedVersion);
    memcpy(t, buf, sizeof t); t[6] = 1;
    expect("flags", verify_kernel_integrity(k.data(), k.size(), t, sizeof t, &a, 1, fake_verify), VerifyStatus::MalformedManifest);
    k[500] ^= 1;
    expect("tampered kernel", verify_kernel_integrity(k.data(), k.size(), buf, sizeof buf, &a, 1, fake_verify), VerifyStatus::DigestMismatch);
    k[500] ^= 1;
    expect("size mismatch", verify_kernel_integrity(k.data(), k.size() - 1, buf, sizeof buf, &a, 1, fake_verify), VerifyStatus::SizeMismatch);
    return fails;
}
