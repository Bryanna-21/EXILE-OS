# OMNI-Kernel — Batch 2: hosted core foundation

**Status: a small, hosted, architecture-neutral implementation foundation with automated tests.
Nothing here boots. No architecture is operational. No security guarantee is claimed.**

Batch 2 continues OMNI-Kernel from the Batch 1 specification (`OMNI-Kernel-Batch-1`,
ABI label **OMNI-ABI 0.1 (draft)**). It implements, in Zig, the checking rules that a later kernel
will need, in a form that runs and is tested on an ordinary host. The package is separate from
Exile OS: it modifies nothing in `Kernel/` or `ExileKernel/`, is not integrated with any Exile OS
build, and does not assume OMNI-Kernel replaces the existing kernel.

## What Batch 2 delivers

| Area | File | Contents |
|---|---|---|
| Errors | `src/core/error.zig` | One error set mapping to the Batch 1 error codes (plus `InvalidState`), provisional wire `Status` values |
| Identifiers | `src/core/ids.zig` | Distinct `ObjectId`, `ProcessId`, `Handle` types; handle = 16-bit index + 16-bit generation |
| Capabilities | `src/core/capabilities.zig` | `Rights` (9 rights, unknown bits rejected), fixed-capacity `CapabilityTable` with insert, lookup, derive (rights can only shrink), release, generation-based stale-handle rejection |
| Resources | `src/core/resources.zig` | Hierarchical, all-or-nothing, overflow-checked accounting for pages, handles, threads, channels, queued bytes |
| Ranges | `src/core/memory.zig` | Checked ranges, alignment, user-window validation, validated sub-slices; arithmetic only |
| IPC | `src/core/ipc.zig` | Bounded message wire format, encode/decode with deterministic validation, handle-transfer-list validation |
| Arch boundary | `src/arch/interface.zig` | Compile-time contract a backend must satisfy; no backend exists |
| Tests | `src/tests.zig` | 59 deterministic tests (positive, negative, randomized with fixed seeds) |
| Script | `scripts/verify.sh` | File check, version check, format check, build and test, manifest check |
| Docs | `docs/` | `IMPLEMENTATION-PLAN.md`, `CORE-INTERFACES.md`, `TEST-PLAN.md`, `SECURITY-REVIEW.md` |

## What Batch 2 does not deliver

* No kernel image, boot code, assembly, bootloader, linker script, scheduler, interrupt handling,
  page-table code, system-call entry, driver, filesystem, network stack or GUI.
* No real inter-process communication: only message encoding and validation.
* No capability revocation, transfer of handles between tables, object destruction, or concurrency
  control. The code is **single-threaded**.
* No cryptography or random-number generation.
* No automated release-mode, freestanding or aarch64 builds. A few manual probes of these were run and
  are recorded, with their limits, in `REPORT.md` and in the toolchain notes below.
* No claim of isolation, verified memory safety, or a secure capability system. A hosted table runs
  in an ordinary process and enforces nothing against code in the same process.

## Architecture support status

| Architecture | Status |
|---|---|
| x86-64 | **Not implemented.** `src/arch/x86_64/README.md` lists requirements only. Does not boot |
| ARM64 (AArch64) | **Not implemented.** `src/arch/aarch64/README.md` lists requirements only. Does not boot |
| RISC-V | Not addressed in Batch 2 |
| Hosted (this package's tests) | Tests run on an x86-64 Linux host in Debug mode only |

No boot test of any kind was performed.

## Toolchain requirements

* **Zig 0.15.2** (exact; `build.zig.zon` sets `minimum_zig_version = "0.15.2"` and `scripts/verify.sh`
  requires `zig version` to print `0.15.2`).
* Bash and standard Unix tools for `scripts/verify.sh`. `python3` is optional (manifest check); if it
  is missing the check is reported as SKIPPED.
* No other dependencies. `build.zig.zon` declares none.

**Tested configuration.** Exactly this combination was used for the results in `REPORT.md`:
Zig 0.15.2 built from source commit `e4cbd752c8c05f131051f8c873cff7823177d7d3` (tag `0.15.2`)
**without LLVM**, Debug mode, on Linux x86-64. That compiler lacks features of an official Zig release
(release-mode optimization, the aarch64 machine-code backend, `@cImport`, assembly files, some linker
features), so its results say little about optimized code generation or other targets. Beyond the
Debug run that `scripts/verify.sh` performs, the authoring session also ran, by hand, the same suite
in `ReleaseSafe`, `ReleaseFast` and `ReleaseSmall` (all 59 passed), compiled a small probe of the core
modules for `x86_64-freestanding` as an object file (succeeded; compile only, nothing linked or run),
and tried the same probe for `aarch64-freestanding` (the compiler failed with an internal code-generation
error; see `REPORT.md`). Running `scripts/verify.sh` with an official Zig 0.15.2 release is the
recommended way to widen coverage; that was not possible in the authoring environment.

## Build and test commands

From the package root, with `zig` 0.15.2 on `PATH` (or `ZIG=/path/to/zig`):

```sh
zig build test --summary all     # compile and run the 59 tests
zig build fmt                    # check formatting (equivalent: zig fmt --check build.zig build.zig.zon src)
scripts/verify.sh                # everything: files, version, format, tests, manifest
```

`scripts/verify.sh` exit codes: **0** every executed check passed; **1** a check failed (missing file,
wrong Zig version, formatting, build or test failure, manifest mismatch); **2** the Zig compiler was
not found, so build and test were not attempted (this is not a test failure). Missing optional tools
are reported as `SKIPPED`, distinct from `FAIL`.

## How to reproduce the reported results

1. Obtain Zig 0.15.2. To rebuild the exact compiler used (no LLVM required; needs a C compiler, GCC at `-O2` exceeded
   the roughly 6 GB available in the authoring sandbox and was killed; Clang at `-O1` was used instead):

   ```sh
   git clone --depth 1 --branch 0.15.2 https://github.com/ziglang/zig.git
   cd zig && git rev-parse HEAD            # expect e4cbd752c8c05f131051f8c873cff7823177d7d3
   cc -o bootstrap bootstrap.c && ./bootstrap
   # The final step of ./bootstrap compiles a ~250 MB generated C file. On the authoring machine GCC at -O2
   # was killed for exceeding memory, so that step was repeated by hand with Clang:
   #   clang -o zig2 zig2.c compiler_rt.c -std=c99 -O1 -fno-stack-protector -Istage1 \
   #         -Wl,-z,stack-size=0x10000000 -pthread -w -g0
   ./zig2 build -p ../stage3 -Dno-langref -Dversion-string=0.15.2 --zig-lib-dir lib
   ../stage3/bin/zig version               # expect 0.15.2
   ```
2. `export PATH=/path/to/stage3/bin:$PATH` (or set `ZIG`).
3. Run `scripts/verify.sh` from the package root, or extract `OMNI-Kernel-Batch-2.zip` first and run it
   there. Expected: `59/59 tests passed` and exit status 0.

Test results depend on the compiler and host; they are expected to be identical because the tests are
deterministic (fixed seeds), but only the configuration above was actually run.

## Known limitations

* Hosted model only; see "What Batch 2 does not deliver".
* Rights, derivation and release rules are tested; **revocation is not implemented** (derived handles
  survive release of their source; test CAP-08 records this).
* Validation helpers do arithmetic only; a validated range does not make a user pointer safe to use.
* Table capacity is fixed at compile time and slots retire after 65,535 releases.
* Many ABI details are **provisional** (`docs/CORE-INTERFACES.md`, decision register DEC-01 to DEC-12).
* The tested compiler is a no-LLVM Debug build, built locally, not an official release.
* Tests were run on one machine and one compiler.

## Relationship to Batch 1

Batch 2 uses Batch 1 terminology (capability, handle, rights, resource account, channel, memory
object) and respects its rules (default deny, kernel-held authority, handle 0 invalid, rights only
shrink on derivation, error codes from A.4). It adds provisional decisions where Batch 1 was silent
(handle layout, an `InvalidState` error, a concrete IPC wire layout, default limits). Nothing in
Batch 1 is contradicted; gaps and how they were filled are listed in `docs/CORE-INTERFACES.md`
section 11, with a migration note for the IPC header. Batch 1 is not modified. Its package
(`OMNI-Kernel-Batch-1.zip`) was re-verified before Batch 2 work began (see `REPORT.md`).

## Reading order

1. This file.
2. `docs/CORE-INTERFACES.md` — types, invariants, errors, ownership, decisions.
3. `docs/TEST-PLAN.md` — requirement-to-test mapping.
4. `docs/SECURITY-REVIEW.md` — threats, present and missing mitigations.
5. `docs/IMPLEMENTATION-PLAN.md` — remaining milestones and dependencies.
6. `REPORT.md` — what was run, with commands, exit codes and counts.
