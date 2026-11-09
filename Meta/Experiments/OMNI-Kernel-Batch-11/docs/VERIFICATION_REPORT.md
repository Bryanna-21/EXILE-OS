# Batch 11 verification report

Everything below is copied from the output of `scripts/verify.sh` run in the authoring work tree (rustc 1.97.0, host target; Zig 0.15.2 built without
LLVM; Linux x86-64). Environment variables: `OMNI11_SKIP_B8_VERIFY=1`, `OMNI11_KERNEL_SYNTAX="x86_64 aarch64"`, `OMNI_BASE_REF=92ab9921c9fff0f2fd457c54050c692bbcf32bf8`. A second run on a clean checkout
with the extracted archive is reported separately in the delivery message, because this file is part of the archive and cannot contain its own hash.

```
PASS    all 38 required files present
WARN    rustc is 1.97.0, not the pinned 1.91.1; results are NOT evidence for the pinned compiler
WARN    toolchain does not match toolchain.pin (set OMNI11_REQUIRE_PINNED=1 to make this fatal)
PASS    zig is the required 0.15.2
PASS    cargo fmt --check
PASS    cargo clippy --all-targets -D warnings (Batch 11 crate)
PASS    Batch 11 cargo test (debug): 18 passed, 0 ignored
PASS    Batch 11 cargo test (release): 18 passed, 0 ignored
PASS    IPC conformance: ALL PASSED
PASS    IPC conformance self-test: self-test: 5/5 mutations detected
PASS    ABI conformance: check_abi_conformance: 23 passed, 0 failed (source text only; nothing executed)
PASS    ABI conformance self-test: self-test: 7 of 7 mutations detected
PASS    Batch 7 hosted tests (debug): 56 passed, 0 ignored
PASS    Batch 8 hosted tests (debug): 65 passed, 0 ignored
PASS    Batch 9 hosted tests (debug): 34 passed, 0 ignored
PASS    Batch 10 hosted tests (debug): 24 passed, 0 ignored
SKIPPED Batch 8 verify.sh (OMNI11_SKIP_B8_VERIFY=1)
PASS    Batch 2 Zig tests: Build Summary: 3/3 steps succeeded; 59/59 tests passed
PASS    Batch 3 Zig tests: Build Summary: 6/6 steps succeeded; 116/116 tests passed
PASS    Batch 4 Zig tests: Build Summary: 8/8 steps succeeded; 171/171 tests passed
PASS    Batch 5 Zig tests: Build Summary: 10/10 steps succeeded; 228/228 tests passed
PASS    Batch 6 Zig tests: Build Summary: 13/13 steps succeeded; 282/282 tests passed
PASS    Kernel/ syntax check (x86_64): kernel_syntax_check: 432 passed, 0 failed, 7 not checked, of 439 (static check only)
PASS    Kernel/ syntax check (aarch64): kernel_syntax_check: 437 passed, 0 failed, 7 not checked, of 444 (static check only)
PASS    no absolute paths, session links, watermarks or banned colour terms in the package
PASS    no build artifact or cache directories in the package
PASS    git diff --check
PASS    no trailing whitespace in package files
PASS    protected paths (Kernel/, ExileKernel/, Documentation/, Batches 2-10) have no local modifications
PASS    the work tree differs from HEAD only under Meta/Experiments/OMNI-Kernel-Batch-11/ and .github/workflows/
PASS    tracked files equal 92ab9921c9fff0f2fd457c54050c692bbcf32bf8 outside Batch 11 and the CI workflow
Summary: 28 passed, 0 failed, 1 skipped, 2 warnings
```

Zig "tests passed" figures are the Zig build runner's own step totals (they count each test in every test binary the build runs), so they differ from the raw
declaration counts of the Batch 1 to 8 audit.

## Kernel/ static syntax check (host clang 18; not a build, not a test)

Run with `scripts/kernel_syntax_check.sh` against the repository at 92ab9921, one architecture at a time (output copied):

```
KARCH=x86_64:  kernel_syntax_check: 432 passed, 0 failed, 7 not checked, of 439 (static check only)
KARCH=aarch64: kernel_syntax_check: 437 passed, 0 failed, 7 not checked, of 444 (static check only)
```

Not checked (x86-64): Kernel/Arch/init.cpp, Kernel/Devices/GPU/Console/ContiguousFramebufferConsole.cpp, Kernel/Devices/GPU/VirtIO/Console.cpp, Kernel/Devices/Input/KeyboardDevice.cpp, Kernel/Devices/TTY/VirtualConsole.cpp, Kernel/Tasks/PowerStateSwitchTask.cpp, Kernel/kprintf.cpp.
These units need LibVT's generated state-machine header, which this environment cannot generate. The check uses host clang, generated configuration headers with every optional debug switch off, and the
two kernel-coverage files are excluded; it says nothing about linking, code generation or behaviour. The earlier run of this check was done before the final script and gave the same x86-64 totals.

## Not run

- Any QEMU boot (x86-64 `pc`, `q35`, ARM64 `virt`): QEMU is not installed. **NOT RUN.**
- `Kernel/` build, link and tests: no project cross toolchain. **NOT RUN.** (Only the static syntax check above ran.)
- Batch 8's own `verify.sh` (skipped here to avoid repeating the Zig regression): **SKIPPED.**
- The CI workflow: only YAML syntax was checked (a YAML parser loaded it); it was never run by GitHub Actions. **NOT RUN.**
- ARM64 stub: `build_aarch64.sh` was not run to success; the authoring Zig could not assemble the stub. **NOT BUILT.**
- Pinned toolchain (rustc 1.91.1, `x86_64-unknown-none`): unavailable. The two WARN lines above record this.

## Batch 11 tests (18)

`tests/caps_unit.rs`: cap_01..cap_10 (rights order, per-space handles, stale handles, generation retirement, table and object-pool exhaustion, descriptor
rules, cycle collection, idempotent cleanup, move attenuation and dead-capability purge). `tests/ipc_e2e.rs`: ipc_01..ipc_08 (two-space round trip with moved and copied
endpoints, blocking receive with timeout and wake, malformed arguments, queue full and peer death, terminate_space, unreadable output buffers, 400-case fuzz, pass-through).
Mutation checks run during development, each detected: escalation check removed, move-attenuation check removed, cycle check removed, dead-capability purge disabled.
