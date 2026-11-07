# Batch 9 verification report

Everything below was run on 2026-11-05 in a Linux x86-64 container with the toolchain listed. Results are reproduced from the
output of `scripts/verify.sh` (phase `pre`, run before the manifest existed) and from separate runs where stated. A check that
could not run is marked SKIPPED or NOT RUN and is never counted as passed.

## Environment

| Tool | Version | Required | Note |
|---|---|---|---|
| rustc / cargo | 1.97.0 | 1.91.1 (pinned by `ExileKernel`, used by Batch 7) | **Pinned compiler unavailable (no network).** Environment issue. Results are not evidence for 1.91.1 |
| Rust target | host x86-64 Linux | `x86_64-unknown-none` | Target unavailable. Bare-metal compilation with the pinned target is BLOCKED |
| clippy | 1.97.0 | n/a | present |
| Zig | 0.15.2 | 0.15.2 | matches |
| gcc 13.3, GNU ld 2.42, Python 3 | present | n/a | |
| QEMU, GRUB, xorriso, KVM | **absent** | needed for Batch 8 boot tests | QEMU tests NOT RUN (environment issue) |

## Commands and actual results

Run from `Meta/Experiments/OMNI-Kernel-Batch-9` with `OMNI9_PHASE=pre OMNI9_SKIP_B8_VERIFY=1` and external build caches:

```
scripts/verify.sh
```

| # | Check | Command (inside `verify.sh`) | Result |
|---|---|---|---|
| 1 | required files | file existence | PASS, 23 files |
| 2 | Rust compiler pin | `rustc --version` | WARN: 1.97.0 is not 1.91.1 |
| 3 | Zig version | `zig version` | PASS, 0.15.2 |
| 4 | formatting | `cargo fmt --check` | PASS |
| 5 | static analysis | `cargo clippy --offline --all-targets -- -D warnings` | PASS (Batch 9 crate only; Batch 7/8 sources are linted by their own scripts) |
| 6 | Batch 9 tests, debug | `cargo test --offline --locked` | PASS, 34 passed, 0 ignored (conformance 7, memory 17, syscall/scheduler 10) |
| 7 | Batch 9 tests, release | `cargo test --offline --locked --release` | PASS, 34 passed, 0 ignored |
| 8 | ABI conformance | `python3 -I scripts/check_abi_conformance.py` | PASS, 23 checks (source text only, nothing executed) |
| 9 | ABI checker self-test | `... --self-test` | PASS, 7 of 7 injected mutations detected |
| 10 | Batch 7 regression | `cargo test --offline --locked --manifest-path ../OMNI-Kernel-Batch-7/Cargo.toml` | PASS, 56 passed, 0 ignored |
| 11 | Batch 8 regression | same with Batch 8 | PASS, 65 passed, 0 ignored |
| 12 | Batch 2 regression | `zig build test --summary all` | PASS, 59/59 |
| 13 | Batch 3 regression | same | PASS, 116/116 |
| 14 | Batch 4 regression | same | PASS, 171/171 |
| 15 | Batch 5 regression | same | PASS, 228/228 |
| 16 | Batch 6 regression | same | PASS, 282/282 |
| 17 | whitespace | `git diff --check` and a trailing-whitespace scan of package files | PASS |
| 18 | hygiene | no absolute paths, session links, watermarks, banned colour terms, caches | PASS |
| 19 | protected paths | `git status --porcelain` on `Kernel/`, `ExileKernel/`, `Documentation/`, Batches 2 to 7 | PASS, no modifications |
| 20 | scope of change | working tree differs from `HEAD` only under Batch 8 and Batch 9 directories | PASS |

Summary line of that run: `21 passed, 0 failed, 3 skipped, 1 warnings`.

Notes on the counts:

- Zig counts are cumulative: each batch's `build.zig` also runs the suites it builds on, so Batch 6's 282 includes Batches 2 to 5.
  Raw `test "` declarations (287 in total across Batches 2 to 6) are not the runner's count, and the two are not expected to match.
- The Zig results above reuse compiled caches. Each suite was also compiled and run from an empty cache earlier in the same session
  (same counts), and the Batch 8 verify run below built Batch 6 from an empty cache.
- A first full run was interrupted while compiling Batch 6, which left its log incomplete. It was re-run; the numbers above come from the
  complete run. No failure occurred in the interrupted run.

### Batch 8's own verification script (run once, separately)

`scripts/verify.sh` of Batch 8, run from its directory: `Summary: 25 passed, 0 failed, 2 skipped, 1 warnings`. It covers the six image
variants building with the host-target fallback, static image and assembly checks, byte-identical double builds and the Batch 6 Zig suite.
The two SKIPPED items: **QEMU boot tests NOT RUN** (qemu-system-x86_64 and grub-mkrescue missing) and the base-ref diff (no ref supplied). The
warning is the unpinned rustc.

## Not run, and why

| Item | Reason | Kind |
|---|---|---|
| Any test under the pinned rustc 1.91.1 | compiler not installable offline | environment |
| Bare-metal build for `x86_64-unknown-none` | target not installed | environment |
| Batch 8 QEMU boot tests (6 variants) | no QEMU/GRUB/xorriso | environment |
| Anything on real hardware | none available | environment |
| ARM64 anything | nothing implemented | implementation (NOT IMPLEMENTED) |
| Fuzzing, sanitizers, Miri, loom | not set up | not done |
| Manifest and `SHA256SUMS` check inside `verify.sh` | the manifest did not yet exist in this phase; it is checked after packaging (see the final response and `APPLY_BATCH_9.md`) | ordering |

## End-to-end coverage against the required list

All Batch 9 tests are **hosted** (no MMU, no ring 3, no interrupts). The "page tables" are real `Mapper`-built tables stored in a
software physical memory, so they test table construction and walking logic, not hardware translation. The context switches execute
real assembly on the host CPU.

| Required area | Covered by | Limit |
|---|---|---|
| Kernel-facing interface definitions | `cf_01..cf_05`, ABI checker | constants and layouts only |
| Memory and address validation | `mem_01..mem_17` | software memory |
| Capability operations, capability transfer, IPC message handling, malformed messages, invalid capabilities, stale handles | Zig suites of Batches 2 to 5 (regression, rows 12 to 16) | **not exercised end to end from Rust: the Rust kernel has no capabilities or IPC.** `sys_01` proves the IPC numbers return `NO_SYS` |
| Thread lifecycle and failures | `sys_04..sys_08` | cooperative only |
| Scheduling, context switching | `sys_04`, `sys_05`, `sys_06` | real asm, host CPU |
| Syscall dispatch, invalid identifiers, error propagation | `sys_01..sys_03`, `sys_07`, `sys_09`, `sys_10` | not entered through a hardware `SYSCALL` |
| Allocation failure, resource exhaustion | `mem_16`, `mem_17`, `sys_08`, `mem_11` | |
| Unauthorized access, invalid user addresses | `mem_03..mem_08`, `mem_10`, `sys_09` | |
| Cleanup and destruction | `sys_05`, `mem_10`, `mem_14` | |
| Concurrency | `conc_01`, `conc_02` | OS threads, not SMP kernel concurrency |
| Regression of earlier batches | rows 10 to 16 and the Batch 8 script | no earlier test removed or edited |

No real bootable integration path was available to run; the Batch 8 image has been built but never booted.

## Reproduce

```
cd Meta/Experiments/OMNI-Kernel-Batch-9
CARGO_TARGET_DIR=/path/outside/repo/cargo9 OMNI9_ZIG_CACHE=/path/outside/repo/zig scripts/verify.sh
```
