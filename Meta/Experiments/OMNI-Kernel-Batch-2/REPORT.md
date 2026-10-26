# OMNI-Kernel Batch 2 — Delivery and Validation Report

This report records what was produced and what was actually run. Every number below was observed in
the authoring session; nothing is estimated. Where something was not run, it says so.

## 1. Summary

Batch 2 delivers a hosted, architecture-neutral Zig core (errors, identifiers, a capability table,
resource accounting, range validation, IPC message validation), a compile-time architecture contract,
59 automated tests, a verification script and four design documents. **No architecture boots, no kernel
image exists, and no security guarantee is claimed.** Scripted results are for one compiler (Zig 0.15.2
built without LLVM), Debug mode, x86-64 Linux host.

## 2. Files created (package `OMNI-Kernel-Batch-2/`)

| File | Purpose |
|---|---|
| `README.md` | Scope, status, toolchain, commands, limitations, relationship to Batch 1, reproduction |
| `REPORT.md` | This report |
| `manifest.json` | SHA-256 and byte size of every other file; excludes itself |
| `build.zig`, `build.zig.zon` | Hosted test build (`zig build test`, `zig build fmt`); no kernel target |
| `src/core/error.zig` | Error set, provisional `Status`, mappings |
| `src/core/ids.zig` | `ObjectId`, `ProcessId`, `Handle`, `ObjectIdAllocator` |
| `src/core/capabilities.zig` | `Rights`, `ObjectType`, `CapabilityTable(capacity)` |
| `src/core/resources.zig` | `Resource`, `Counters`, hierarchical `Account` |
| `src/core/memory.zig` | Checked ranges, alignment, `UserWindow`, `subslice` |
| `src/core/ipc.zig` | Wire format, `encode`, `decode`, `validateTransferList` |
| `src/arch/interface.zig` | Compile-time backend contract |
| `src/arch/x86_64/README.md`, `src/arch/aarch64/README.md` | Requirement notes only; no code |
| `src/tests.zig` | 59 tests |
| `docs/IMPLEMENTATION-PLAN.md`, `CORE-INTERFACES.md`, `TEST-PLAN.md`, `SECURITY-REVIEW.md` | Design, mapping and review documents |
| `scripts/verify.sh` | Verification script |

Source size (lines): capabilities 253, error 128, ids 98, ipc 162, memory 94, resources 121, interface 80,
tests 958. The directory layout follows the suggested one with no structural change.

## 3. Features implemented versus design intent

**Implemented and tested (hosted):** error set and status mapping; distinct id/handle types; rights
parsing that rejects unknown bits; capability insert/lookup/derive/release with generation-based stale
rejection and slot retirement; hierarchical all-or-nothing accounting; checked ranges, alignment and user
windows; IPC encode/decode/validation and transfer-list validation; the architecture contract check.

**Design intent only (not implemented):** capability revocation, handle transfer between tables, object
lifetime and reference counts, concurrency control, real IPC, copy-in/out, any architecture backend, any
isolation. These are listed in `docs/SECURITY-REVIEW.md` section 3 and `docs/IMPLEMENTATION-PLAN.md`.

**Batch 1 gaps filled with provisional decisions** (DEC-01 to DEC-12, `docs/CORE-INTERFACES.md`
sections 9 and 11): handle layout, retirement on generation wrap, a new `InvalidState` error, provisional
numeric status values, a concrete IPC wire layout (with a migration note), default limits, range rules,
duplicate-handle rejection, child-account limit rule, error-selection order, fixed table capacity,
derivation requiring `duplicate`. No Batch 1 statement was contradicted.

## 4. Environment and exact tool versions

| Item | Value |
|---|---|
| Host | Linux 6.18.44-fc-v153 x86_64, 2 CPUs, about 8 GB RAM (about 6 GB usable by a process before the kernel killed one) |
| Zig used for all results | `0.15.2` (output of `zig version`), source tag `0.15.2`, commit `e4cbd752c8c05f131051f8c873cff7823177d7d3`, built **without LLVM** (`dev = full`, no `-Denable-llvm`) |
| Zig binary SHA-256 | `448d395e06f42ee6d33e85f02fc4a9095ccbe4febe8f494730062d7fc092242e` |
| C compilers | Ubuntu clang 18.1.3 (built the bootstrap compiler), gcc 13.3.0 |
| Others | git 2.43.0, Python 3.13.17, GNU bash 5.2.21 |

An official Zig release could not be downloaded (section 6), so the compiler was built locally from the
official source tag. It is not an official release binary. Per the Zig project README, a no-LLVM build
lacks release-mode optimization and some other features; the manual probes in section 5.4 show what
actually worked.

## 5. Commands run and observed results

### 5.1 Batch 1 inspection (before writing)

* `sha256sum OMNI-Kernel-Batch-1.zip`: `e3029a9c739235fe052dbea95ee08d1aac650b5c7b664d3150bf38420bd53d5f`.
* `unzip -tq`: no errors. Fresh extraction and manifest check: 4 of 4 entries matched (sha256 and size).
* Batch 1 documents were read and the gaps in `docs/CORE-INTERFACES.md` section 11 were derived from them.

### 5.2 Primary results (Debug, scripted)

| Command | Result |
|---|---|
| `zig fmt --check build.zig build.zig.zon src` | exit 0 (the compiler also printed 140 lines of its own debug-allocator diagnostics on stderr; they are not formatting findings; `verify.sh` reports this) |
| `zig build test --summary all` | exit 0; `Build Summary: 3/3 steps succeeded; 59/59 tests passed` |
| Packaging pass 1 (made before the last README/doc/REPORT edits, with a one-line placeholder `REPORT.md`): ZIP built, `unzip -t` clean, no duplicate entries, extracted to a fresh directory, `scripts/verify.sh` run there | exit 0; 19 required files present; version `0.15.2`; fmt passed; `59/59 tests passed`; manifest matched (checked with python3); 5 passed, 0 failed, 0 skipped |

The source and test files did not change after pass 1; only documentation files were edited afterwards, and
the manifest and ZIP were regenerated from the final bytes (see section 9 for what is and is not recorded
about that final package).

The 59 tests are: ERR 4, ID 5, CAP 15, RES 10, MEM 9, IPC 12, ARCH 3, X 1 (all IDs unique;
`docs/TEST-PLAN.md` and `src/tests.zig` agree exactly on the set, checked by script).

### 5.3 Verification-script behaviour (negative cases, run on scratch copies)

| Case | Observed |
|---|---|
| Intact tree | exit 0 |
| Required file deleted | `FAIL missing required file`; exit 1 |
| `ZIG` points at a non-existent path | `TOOL-MISSING`, build/test not attempted; exit 2 |
| A test deliberately changed to fail | `FAIL zig build test`, output shown (`58/59 passed, 1 failed`); exit 1 |
| A source file deformatted | `FAIL zig fmt --check`; exit 1 |
| Wrong expected Zig version | `FAIL`; exit 1. With `OMNI_ALLOW_ZIG_VERSION_MISMATCH=1`: `WARN`; exit 0 |
| A packaged file changed after the manifest was made | `sha256 mismatch` and `size mismatch` reported, `FAIL manifest`; exit 1 |
| `python3` absent from `PATH` | manifest check `SKIPPED` (reported distinctly from FAIL); other checks passed; exit 0 |

### 5.4 Mutation checks (do the tests catch real bugs?)

Nine bugs were injected one at a time into scratch copies; each run exited 1, i.e. the suite detected it.

| Mutant | Detected by |
|---|---|
| Derivation without the subset check | CAP-07, CAP-10, CAP-15 |
| Release does not bump the generation | CAP-03, CAP-15 |
| Parent account ignored when reserving | RES-04 |
| IPC decode without the exact-size check | Crash (index out of bounds) in IPC-02 |
| Range upper bound `>` changed to `>=` | MEM-01, MEM-02 and one more |
| Rights bits masked instead of rejected | CAP-06 and two more |
| Slot retirement disabled | Crash (integer overflow) in CAP-12 |
| Duplicate transfer handles allowed | IPC-11 |
| Release underflow check disabled | Crash (test run aborted before its summary; the panic text was not inspected) |

Three were detected as crashes, which stop the test binary before it prints a summary (the build then
shows a failed step). A first attempt at the duplicate-handle mutant did not compile and was discarded and
redone. This is a small, hand-chosen sample, not a mutation-testing score.

### 5.5 Manual probes outside `verify.sh` (one-off, not automated)

| Command | Result |
|---|---|
| `zig test -OReleaseSafe src/tests.zig` | exit 0, `All 59 tests passed.` |
| `zig test -OReleaseFast src/tests.zig` | exit 0, `All 59 tests passed.` |
| `zig test -OReleaseSmall src/tests.zig` | exit 0, `All 59 tests passed.` |
| `zig build-obj -target x86_64-freestanding` of a probe that instantiates the table, IPC, memory and resource code | exit 0 (compile only; nothing linked or run) |
| Same probe for `aarch64-freestanding` | exit 1: compiler error `bad intcast_safe core.ipc.MsgKind u16` raised inside the compiler (`std/enums.zig`) |
| `zig test -target aarch64-linux --test-no-exec src/tests.zig` | exit 134: compiler panic in code generation (`attempt to use null value`) |

Interpretation: release-mode passes include `ReleaseFast`/`ReleaseSmall`, where Zig's runtime safety
checks are off, which supports that the explicit checks (not safety traps) enforce the rules on the paths
tested. But this compiler has no LLVM optimizer, so optimized code generation was not exercised. The
aarch64 failures occur inside the compiler's aarch64 back end; whether the Batch 2 code would build for
aarch64 with a full compiler is **unknown**. These probes were not repeated in the final extraction.

## 6. Failures and obstacles encountered, and how they were handled

1. **No Zig installed.** Searching the machine found none.
2. **Package downloads blocked.** `pip install ziglang` failed first with a DNS error (the host is on the
   no-proxy list), then, when sent through the proxy for that one command, the proxy answered `403
   Forbidden`. `npm` for `registry.npmjs.org` behaved the same (DNS failure, then proxy 403). `apt` could not
   resolve its mirrors. The 403s are policy denials; the denied hosts were not retried or worked around.
   GitHub was reachable (the environment allowlist normally includes it), so Zig was built from its source
   tag instead.
3. **Bootstrap compile ran out of memory.** `./bootstrap` generates a ~250 MB C file; GCC at `-O2` was
   killed by the kernel's memory limit (about 6 GB). A GCC `-O1` retry with low-memory settings was still
   running after about 34 CPU-minutes and was abandoned. Clang `-O1` finished (exit 0).
4. **The first compiler (`zig2`) lacks `fmt` and `version`.** A full compiler was built with it
   (`zig2 build -Dno-langref -Dversion-string=0.15.2`; exit 0), which reports `0.15.2` and supports `fmt`.
5. **Invalid package fingerprint.** The first `zig build test` rejected `.fingerprint = 0x0`; the value the
   compiler suggested (`0xee3a1f22dec83f54`) was used. The first compile of the tests then passed (59/59).
6. **A background build was stopped when a turn ended**, and one `pkill -f` pattern matched my own shell and
   aborted that command. Both were re-run; neither affected results.
7. **Documentation corrections found by my own checks:** an unresolved short file name and an unresolved
   Batch 1 file reference were made explicit; statements that release modes and aarch64 were "not tested /
   not attempted" were corrected after the probes in 5.5 were run.
8. **Compiler diagnostic noise.** `zig fmt` prints leak reports about itself (a Debug-built compiler). The
   script keeps the exit status authoritative and notes the line count instead of failing.

No test was disabled, skipped or edited to make it pass, and no result in this report is hard-coded.

## 7. Known limitations

* Hosted single-threaded model; no isolation, no concurrency safety, no revocation, no object lifetime.
* Validation helpers do not make a pointer safe to dereference.
* Provisional ABI decisions are unvalidated.
* Table slots retire after 65,535 releases and are never reused.
* Compiler is a locally built no-LLVM Debug-capable build, not an official release; one host, one run set.
* Randomized tests use fixed seeds (one per test).
* See `docs/SECURITY-REVIEW.md` sections 3 and 4 for the complete list.

## 8. Features deliberately deferred

Boot code for either architecture; assembly; exception and interrupt handling; MMU and page tables;
scheduler; real IPC transport and queues; revocation; handle transfer; cancellation objects;
copy-in/copy-out; drivers and services; entropy and cryptography; RISC-V; Exile OS integration.
Milestones and dependencies are in `docs/IMPLEMENTATION-PLAN.md`.

## 9. Manual or unperformed checks

* No independent human review of code or documents; the security review is a self-review.
* No official Zig release, no LLVM-enabled compiler, no run on another OS or CPU, no VM or hardware boot.
* No Markdown renderer, spell-checker or link checker beyond the scripted reference check; no external
  link exists in the documents.
* Documentation consistency (terminology, decision IDs, test IDs) was checked by script and by reading
  by the author only.
* No coverage measurement and no mutation-testing tool; only the nine hand-made mutants above.
* Strict "clean extraction" of the **final** archive (after this report was last edited) is not recorded
  here, because recording it would change this file and invalidate the manifest and archive. It is
  reported in the delivery message together with the final ZIP size and SHA-256, which cannot be stored
  inside the archive either.

## 10. Remaining risks and contradictions

* Provisional choices in `CORE-INTERFACES.md` may be revised; code in other components should not depend
  on them yet.
* `peek` and `resolveMut`'s `@constCast` are review points for later milestones (`SECURITY-REVIEW.md`
  section 4).
* The aarch64 compiler failures leave ARM64 buildability of the core undetermined.
* The earlier statement in Batch 1 that the Zig version was undecided is now resolved to 0.15.2 only for
  Batch 2's hosted tests; a kernel toolchain decision remains for milestone M1.
