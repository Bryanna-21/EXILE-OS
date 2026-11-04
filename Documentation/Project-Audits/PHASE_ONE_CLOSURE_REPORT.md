# Phase One Closure Report

Status: **host components build and pass their tests; the OS has NOT been built and NOT booted.** Nothing here shows verified boot.

## 1. Baseline

- Base: `origin/main` at `c8edf3d0`. The supplied completion patch applied cleanly to it and had not been applied before; it was imported once, unmodified, as the first commit of this branch. The prekernel approval patch was not applied.
- Reports in the supplied package were treated as claims. The two LibCore build errors it described were reproduced and confirmed to be signature errors independent of the compiler version.

## 2. Host LibCore blockers (fixed, real build)

| Blocker | Cause | Fix |
|---|---|---|
| `System::unsetenv` undeclared (`SystemServerTakeover.cpp`) | Wrong owner: environment mutation lives in `Core::Environment` | Call `Environment::unset(StringView)` (already existed) |
| `Process::get_name(pid)` undeclared (`Process.cpp`) | Overload never declared or implemented | Declared and implemented; Linux reads `/proc/<pid>/cmdline`, other platforms return `"???"` as `get_name()` does |
| (found while reading callers) `IPCProcess::get_process_pid` | Inverted `stat` check: an existing pid file returned "no process", so a running process could never be found | Check inverted to "no pid file means no process" |

Consequence of the third item: single-instance detection in `LibWebView::ChromeProcess` now behaves as intended; previously a second launch would never have connected to the first. On non-Linux platforms the process-name check is degraded (always `"???"`).

Regression tests: `Tests/LibCore/TestLibCoreIPCProcess.cpp`, 8 cases. Mutation check: re-inverting the `stat` check and removing the `unset` call each made the tests fail; with the fixes restored 8/8 pass.

## 3. Reproducible build

`Meta/Exile/build-and-test-host.sh <build-dir> [--offline-data] [--full]` (clean configure, build, ctest). Verified from an empty build directory with `--offline-data`.

Toolchain used for every result below: clang/clang++ 20.1.2 and lld 20.1.2 (Ubuntu 24.04 packages), CMake 3.28.3, Ninja 1.11.1, Python 3, rustc/cargo 1.97.0 (not used by host tests), QEMU 8.2.2, OVMF 2024.02. clang 18.1.3 and g++ 13.3 cannot build the project (the former needs source-warning workarounds, the latter lacks C++26).

Downloads: mandatory for a full build are the CA bundle, Unicode, CLDR and time-zone data (disabled by `--offline-data`). Package registries (apt) were reachable; `curl.se`, `unicode.org`, `data.iana.org` and the web archive returned proxy 403. Exact failure: `curl: (56) CONNECT tunnel failed, response 403` for `https://curl.se/ca/cacert.pem`.

With data downloads off, LibUnicode `CharacterTypes.cpp` fails under `-Werror,-Wunused-parameter`. This is a pre-existing defect on that path, deliberately not fixed here (unrelated); the script passes the narrow `-Wno-error=unused-parameter`. Unicode-dependent test results from such a build are not meaningful.

## 4. QEMU validation

| Question | Finding | Evidence |
|---|---|---|
| Does prekernel output reach serial? | Prekernel `dbgln` writes to EFI `ConOut` (`DebugOutput.cpp`). Under OVMF `-display none -serial file:` ConOut reached the serial capture for a stand-in image, together with OVMF's own log lines | stand-in self-test |
| Exit codes/timeouts | A halted or booted guest never exits, so every case under the old runner ended by `timeout` (rc 124) after the full wait. The runner now polls the serial file, stops QEMU on the first decisive line, and classifies VERIFIED / REJECTED / QEMU_EXITED / NO_OUTPUT / TIMEOUT | stand-in self-test |
| Silent image | OVMF itself logs to serial, so a started-but-silent image is TIMEOUT; NO_OUTPUT means serial entirely empty | stand-in self-test |
| Real boot | **Not executed.** No `Kernel.efi` exists | see blockers |

The stand-in (`Tests/ExileBoot/qemu/standin/`) is a minimal UEFI application, **not the Exile prekernel**. It validates runner mechanics only. Unverified assumptions that remain: the real prekernel reaches the verification call under OVMF q35, the embedded-marker offsets in the runner match the real image, and the real boot requires no extra disk, cmdline or firmware setup beyond `Meta/exile.sh` (not yet compared against it).

Evidence required by the task, by scenario:

| Scenario | Real QEMU boot | What exists instead |
|---|---|---|
| Successful boot | BLOCKED (B1) | Verifier unit test; fixture `valid-accepted` |
| Bad manifest | BLOCKED (B1) | Fixture `modified-manifest`; tamper script |
| Bad key / anchor | BLOCKED (B1) | Fixtures `mismatched-seed`, `invalid-anchor-zero`, `untrusted-anchor`; `check-keys` |
| Tampered artifacts | BLOCKED (B1) | Fixture `modified-payload`; tamper script |
| Missing/malformed config | BLOCKED (B1); not covered even by fixtures for prekernel config files | Tool-level missing-input checks |
| Timeout/unexpected exit | Runner logic validated with stand-in only | stand-in self-test |

## 5. Blockers

- **B1 No Exile `Kernel.efi`.** The OS superbuild (`Meta/CMake/Superbuild`) fails at its nested Lagom configure on the CA bundle download (403), and there is no cross toolchain (`Toolchain/Local` absent; its build scripts need network sources). Needed: allow-listed access to the toolchain and data sources, or a prebuilt toolchain/artifact cache.
- **B2 Prekernel hook never compiled for target.** `BootVerification.cpp` and the `ExileBootKeyCheck` CMake hook were never built in the real OS build. The hook was tested in a mock CMake project only.
- **B3 Full host suite not run.** A clean full build is 3599 targets; on the 2-CPU environment it was stopped at about 6 percent. Only the Phase One targets plus anything they depend on were built. The rest of the ctest suite is unverified here and CI covers it.
- **B4 CI not observed.** The workflow change below has not run on GitHub.
- **B5 Off-toolchain prekernel compile** with host clang failed on kernel-only headers (`errno_codes.h`, deletes without a size); not a substitute for B1.

## 6. Prekernel approval patch (not applied)

`git apply --check` passes against this branch. It changes one line of `Kernel/EFIPrekernel/CMakeLists.txt`: the signing command gains `1 --anchor "${BOOT_TRUST_ANCHOR_PUBKEY}"`, so `sign` itself refuses (exit 3) when the seed does not derive the configured anchor, as defence in depth beside `ExileBootKeyCheck`. Risk if applied: it is part of the kernel build and has never been built; a mistake would break `ENABLE_BOOT_VERIFICATION=ON` builds only (the default build is unaffected). It stays pending your explicit approval.

## 7. CI

`.github/workflows/lagom-template.yml` gained a named step after the existing full `ctest` step, running `ctest -R 'TestBootGuardian|ExileBootTamperTests|TestLibCoreIPCProcess' --no-tests=error`. Nothing was weakened. No QEMU job was added: the runner has not been run against a real image and the CI would need the toolchain (B1). The stand-in self-test is not in CI.

## 8. Test records

All on the toolchain in section 3, Linux x86_64, build dir from an empty directory (`--offline-data`).

| Test | Command | Exit | Status | Kind |
|---|---|---|---|---|
| TestLibCoreIPCProcess (8 cases) | `ctest -R TestLibCoreIPCProcess` | 0 | pass | real host build and test |
| TestBootGuardian (12 cases) | `ctest -R TestBootGuardian` | 0 | pass | real host build, unit test |
| ExileBootTamperTests (53 checks) | `ctest -R ExileBootTamperTests` | 0 | pass | real host tool, script test (not a boot) |
| Fixture matrix (6 scenarios) | `Tests/ExileBoot/qemu/check-fixture-matrix.sh <ExileBootManifest> <fixture-dir>` after `make-fixtures.sh <dir>` | 0 | pass | fixture test (not QEMU) |
| Runner self-test (3 cases) | `Tests/ExileBoot/qemu/standin/run-standin-selftest.sh` | 0 | pass | actual QEMU/OVMF boot of a stand-in, not the prekernel |
| Reproducible script | `Meta/Exile/build-and-test-host.sh <empty-dir> --offline-data` | 0 | pass (3/3 ctest) | real host build |
| Real prekernel QEMU boot | `Tests/ExileBoot/qemu/run-qemu-boot-tests.sh <Kernel.efi>` | n/a | NOT RUN | blocked (B1) |
| Full ctest suite | `... --full` | n/a | NOT RUN | blocked (B3) |

Logs are not committed; rerun the commands above. Mutation checks (section 2) were run on an earlier build directory.

## 9. Verify independently

    git checkout feature/phase-one-closure
    Meta/Exile/build-and-test-host.sh /path/to/empty-build-dir --offline-data
    Tests/ExileBoot/qemu/standin/run-standin-selftest.sh
    git diff --check origin/main..HEAD
    git diff --stat origin/main..HEAD -- Kernel ExileKernel    # must be empty
