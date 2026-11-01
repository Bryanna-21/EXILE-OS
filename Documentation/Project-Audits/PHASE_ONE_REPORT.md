# Phase One report: boot and trust foundation

Branch: `feature/non-kernel-foundation-phase-zero-one`, based on `origin/main` `d0b61526ad2378fe3bf801bf2c94834bad8b3220`.

## What changed

| File | Change |
|---|---|
| `Meta/Tools/ExileBootManifest.cpp` | `sign` accepts `--anchor <pubkey>` and refuses (exit 3, nothing written) unless the seed derives that key. All-zero seed or public key is refused in `sign`, `verify` and `--anchor`. Argument parsing for `sign` is strict. Exit codes documented in `usage`. |
| `Tests/ExileBoot/test-boot-integrity.sh` | 24 checks became 44: anchor match and mismatch, bad or missing anchor files, all-zero keys, junk arguments, missing kernel, seed, key and manifest, empty kernel, keygen hygiene. Each rejected invocation also asserts that no manifest file was written. |
| `Tests/ExileBoot/CMakeLists.txt` | Registers the script as CTest test `ExileBootTamperTests` (host build only), so the existing host CI `ctest` step runs it. |
| `Documentation/BootIntegrity.md` | Documents `--anchor`, exit codes, the CTest registration and the remaining gaps. |
| `Documentation/Project-Audits/NON_KERNEL_SYSTEM_AUDIT.md` | Phase 0 audit. |
| `Documentation/Architecture/NON_KERNEL_SYSTEM_ARCHITECTURE.md`, `Documentation/Project-Plans/NON_KERNEL_ROADMAP.md` | Architecture proposal and roadmap. |
| `Documentation/Project-Audits/PHASE_ONE_REPORT.md` | This report. |

`Kernel/` and `ExileKernel/` are **not modified**. `Kernel/Boot/Exile/` (the verifier) is unchanged.

## Known audit items

| Item | Outcome |
|---|---|
| ExileBootManifest registered | Verified by reading; unchanged |
| `TestBootGuardian.cpp` in `exile_test`/CTest | Verified by reading; unchanged |
| `test-boot-integrity.sh` not in CI | **Fixed**: registered as `ExileBootTamperTests`. CI result not observed |
| Signing-tool trust-anchor validation | **Fixed in the tool**. The build-system half is blocked by the no-`Kernel/`-changes rule; see the proposed patch below |

## Tests run (this environment)

Built with `g++ 13.3 -std=c++23` directly, against the repository's `AK` and `LibCrypto` sources (full Lagom configure was blocked, see the audit).

| Test | Result |
|---|---|
| `test-boot-integrity.sh` against the new tool | 44 passed, 0 failed |
| Same script against the **previous** tool | 7 of the new checks fail, as intended (they detect the gap) |
| `TestBootGuardian.cpp`, 12 cases (unchanged code), stand-in `LibTest` | 12 passed, 0 failed |
| CTest registration block in isolation (stubbed `lagom_test`, prebuilt tool as an imported target) | `ExileBootTamperTests` passed under `ctest` |
| `clang-format --dry-run` on the changed C++ | no output (clean) |
| `bash -n` on the script | OK |

Not run: the real Lagom build and `ctest`, the OS build, any QEMU boot, the EFI prekernel compile.

## Security requirements, against the tests

| Requirement | Covered by |
|---|---|
| Reject invalid signatures | `any_single_byte_change...`, script "every manifest byte" (128 of 128) |
| Reject modified kernel images | `modified_kernel_is_rejected`, script first/middle/last byte, truncation, append, empty |
| Reject missing or malformed manifests | `malformed_and_missing_manifests_fail_closed`, script empty/short/long/missing |
| Reject untrusted signing keys | `manifest_from_an_untrusted_key_is_rejected`, script wrong key, attacker-signed |
| No insecure fallback | `missing_inputs_fail_closed` (no anchors fails closed) |
| Clear failure codes | `VerifyStatus` strings; tool exit codes 0/1/2/3 |
| Recovery behaviour | **Not tested**: no recovery path exists (audit section 3.7) |
| Valid signed image | `valid_manifest_is_accepted`, script "untouched kernel verifies" |
| Dev/test signing separate from production trust | Test keys are per-run scratch files. **No build enforcement exists**; not claimed |

## Proposed patch for the kernel maintainers (not applied)

`Kernel/EFIPrekernel/CMakeLists.txt` is outside this change's permitted scope. It would pass the anchor to the signing step and refuse a seed inside the source tree:

```diff
@@ -18,6 +18,15 @@ if (ENABLE_BOOT_VERIFICATION)
             message(FATAL_ERROR "Boot key file must be exactly 32 raw bytes, got ${key_file_size}: ${key_file}")
         endif()
     endforeach()
+    # A signing seed inside the source tree is one `git add .` away from being published. Keep it outside.
+    get_filename_component(boot_seed_real "${BOOT_SIGNING_SEED}" REALPATH)
+    get_filename_component(boot_source_real "${ExileOS_SOURCE_DIR}" REALPATH)
+    cmake_path(IS_PREFIX boot_source_real "${boot_seed_real}" NORMALIZE boot_seed_in_source)
+    if (boot_seed_in_source)
+        message(FATAL_ERROR
+            "BOOT_SIGNING_SEED must not be inside the source tree (${boot_seed_real}).\n"
+            "Keep the signing seed offline or in a secret store outside the repository checkout.")
+    endif()
     set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${BOOT_TRUST_ANCHOR_PUBKEY}")

     file(READ "${BOOT_TRUST_ANCHOR_PUBKEY}" boot_trust_anchor_hex HEX)
@@ -27,7 +36,7 @@ if (ENABLE_BOOT_VERIFICATION)
     # Sign the very file that KernelImage.S embeds, so the manifest always describes the bytes that will be loaded.
     add_custom_command(
         OUTPUT ${PREKERNEL_BOOT_MANIFEST_PATH}
-        COMMAND $<TARGET_FILE:Lagom::ExileBootManifest> sign ${PREKERNEL_KERNEL_IMAGE_PATH} "${BOOT_SIGNING_SEED}" ${PREKERNEL_BOOT_MANIFEST_PATH}
+        COMMAND $<TARGET_FILE:Lagom::ExileBootManifest> sign ${PREKERNEL_KERNEL_IMAGE_PATH} "${BOOT_SIGNING_SEED}" ${PREKERNEL_BOOT_MANIFEST_PATH} 1 --anchor "${BOOT_TRUST_ANCHOR_PUBKEY}"
         DEPENDS Kernel ${PREKERNEL_KERNEL_IMAGE_PATH} Lagom::ExileBootManifest
         VERBATIM
     )
```

This patch was not built or applied; it is shown as a review artifact. `cmake_path(IS_PREFIX ...)` needs CMake 3.20 and the project minimum is 3.25.

## Build and verification commands

Host build (full, needs network access for the Lagom dependencies):

```sh
cmake -S Meta/Lagom -B Build/lagom -G Ninja
cmake --build Build/lagom
ctest --test-dir Build/lagom -R "ExileBoot|TestBootGuardian" --output-on-failure
```

Without the full build (what was run here), from the repository root, with an `AK/Debug.h` generated from `AK/Debug.h.in` into `$GEN/AK/`:

```sh
g++ -std=c++23 -O1 -w -I. -IUserland/Libraries -I$GEN \
    Meta/Tools/ExileBootManifest.cpp Kernel/Boot/Exile/BootGuardian.cpp \
    Userland/Libraries/LibCrypto/Curves/Curve25519.cpp Userland/Libraries/LibCrypto/Curves/Ed25519.cpp \
    Userland/Libraries/LibCrypto/Hash/SHA2.cpp <AK sources> -o ExileBootManifest
bash Tests/ExileBoot/test-boot-integrity.sh ./ExileBootManifest
```

## Known limitations and unresolved dependencies

* Verified boot remains experimental, opt-in and not proven on a target; do not describe Exile OS as having verified boot.
* The build-system half of the anchor check needs a `Kernel/` change (patch above).
* A tamper boot test in QEMU is not written: it needs the target toolchain and the patch above.
* CI results for the new CTest entry are unobserved.
* The tamper script needs `python3` and uses `openssl` when available (the OpenSSL cross-check is skipped, with a printed notice, when it is not).
* Production key custody, rotation, revocation and rollback protection are not designed.
