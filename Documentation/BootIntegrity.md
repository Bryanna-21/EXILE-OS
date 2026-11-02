# Exile Boot Integrity

Status: **experimental, opt-in, not verified end to end.** Nothing in this document means that Exile OS currently provides verified boot.

## What exists

- `Kernel/Boot/Exile/BootManifest.h`: the v2 manifest wire format (128 bytes, little-endian, parsed field by field).
- `Kernel/Boot/Exile/BootGuardian.{h,cpp}`: `verify_kernel_integrity()`. It returns `VerifyStatus::Ok` only if the manifest parses strictly, a compiled-in trust anchor's Ed25519 signature over it is valid, the kernel size matches, and the SHA-256 digest matches. Every other outcome is a failure, including "no trust anchor configured".
- `Meta/Tools/ExileBootManifest.cpp`: host tool with `keygen`, `sign` and `verify`.
- `Kernel/EFIPrekernel/BootVerification.{h,cpp}`: calls the verifier before the kernel ELF is parsed, mapped or relocated, and panics on any failure. Compiled only with `ENABLE_BOOT_VERIFICATION=ON`.

## What has been tested

- Verifier unit tests (`Tests/ExileBoot/TestBootGuardian.cpp`, 12 cases) and the tamper script (`Tests/ExileBoot/test-boot-integrity.sh`, 44 checks) run on a development host with scratch builds. The script is registered with CTest as `ExileBootTamperTests` in the host (Lagom) build. They cover a modified kernel byte, size changes, truncation, every manifest byte flipped, a wrong key, an attacker-signed manifest, domain separation, missing inputs, all-zero keys and a signing seed that does not match the trust anchor.

## What has NOT been tested

- The prekernel integration has not been compiled for the target and has not been booted in QEMU or on hardware. There is no tamper boot test.
- Until that exists, do not describe Exile OS as having verified boot.

## Threat model and limits

1. **Secure Boot dependence.** The trust anchor is a public key compiled into the EFI prekernel. That only protects anything if the prekernel is itself authenticated by the platform (UEFI Secure Boot). Without Secure Boot an attacker replaces the whole prekernel, anchor included.
2. **Single PE image.** The prekernel embeds the kernel ELF in its own `.data`. A digest or manifest stored beside the kernel authenticates nothing by itself; the guarantee comes from the signature checked against the anchor inside the authenticated image.
3. **Scope.** EFI boot only. Other boot paths are unprotected.
4. **Not covered:** initrd or userland images, rollback or downgrade protection, key revocation or rotation, measured boot or TPM attestation.
5. **Opt-in.** The default build is unchanged and does not verify.

## Keys

- Generate with `ExileBootManifest keygen <seed-out> <pubkey-out>`. Both are raw 32-byte files.
- Sign with `ExileBootManifest sign <kernel> <seed> <manifest-out> [os-version] [--anchor <pubkey>]`. With `--anchor` the tool refuses (exit 3, nothing written) unless the seed derives exactly that public key, and all-zero keys are always refused.
- Exit codes: 0 ok, 1 usage or I/O error, 2 verification failed, 3 seed does not match the trust anchor.
- The seed is the signing authority. Keep it offline, never commit it, and never put it in CI logs. Pass it to the build with `BOOT_SIGNING_SEED`.
- The public key goes in `BOOT_TRUST_ANCHOR_PUBKEY`.
- Enabling the option without both paths is a configure error.

## Known gaps

- Seed and anchor consistency is checked before the prekernel is built by `Meta/CMake/exile_boot_verification_checks.cmake` (target `ExileBootKeyCheck`, using `ExileBootManifest check-keys`), which also refuses a signing seed inside the source tree. That hook was tested only in a mock CMake project, not in a real OS build. The signing step itself (`Kernel/EFIPrekernel/CMakeLists.txt`) does not yet pass `--anchor`; a separate patch for the kernel maintainers adds it as defence in depth.
- A QEMU boot test exists as `Tests/ExileBoot/qemu/run-qemu-boot-tests.sh` but has **never been run**: it needs a `Kernel.efi` built with `ENABLE_BOOT_VERIFICATION=ON`.
- No separation between development and production keys is enforced by the build. The fixtures in `Tests/ExileBoot/qemu/make-fixtures.sh` use test seeds derived from public labels; they must never be used as a real trust anchor.
