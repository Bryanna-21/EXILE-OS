# OMNI-Kernel Implementation Plan (after Batch 2)

Status: planning document. Everything marked **planned** is not done. Batch 2 delivers only the
hosted core abstractions described in `CORE-INTERFACES.md`; no architecture boots.

Batch 1 (`OMNI-Kernel-Batch-1`, ABI label **OMNI-ABI 0.1 (draft)**) defined a 13-stage roadmap
(stage 0 specification, stage 1 toolchain and build foundation, stage 2 boot path, and so on).
Batch 2 corresponds to **stage 1 (partially)**: a build foundation and a hosted test harness for
architecture-neutral logic. This plan refines the remaining work into milestones with dependencies
and exit evidence. Milestone numbers are new and are not Batch 1 stage numbers; the mapping is shown.

## 1. Where Batch 2 stands

| Item | State |
|---|---|
| Hosted core abstractions (errors, ids, capabilities, resources, ranges, IPC validation) | Implemented in `src/core/`, tested on the host (results in `REPORT.md`) |
| Architecture contract (`src/arch/interface.zig`) | Implemented as a compile-time contract; exercised only by a test mock |
| x86-64 / ARM64 backends | **Not implemented**; README requirement notes only |
| Boot, MMU, interrupts, scheduler, real IPC | **Not implemented** |
| Release-mode builds, freestanding targets, aarch64 code generation | Only manual probes (recorded in `REPORT.md`): suite passed in all three release modes; an x86-64 freestanding object compiled; aarch64 failed inside the compiler. Nothing is automated and none of it is a kernel build |

## 2. Toolchain constraint that shapes the plan

Batch 2 was compiled with Zig 0.15.2 built from source **without LLVM** (the only way available in
the authoring environment). That compiler, per the Zig project's own README, lacks release-mode
optimization, the aarch64 machine-code backend, `@cImport`, assembly-file compilation, and some
linker features. Consequences:

* Milestone M1 must obtain a fully featured Zig 0.15.2 (official release binary or an LLVM-enabled
  build) before any freestanding, assembly, ReleaseSafe/ReleaseFast, or aarch64 work.
* The automated results in `REPORT.md` are Debug-mode, x86-64-Linux-host results. Manual runs in the
  release modes passed too, but this compiler has no LLVM optimizer, so they do not show how the code
  behaves under optimized code generation.

## 3. Milestones

Dependencies are listed by milestone ID. "Exit evidence" is what must be recorded (command output,
test counts) before the milestone may be called done.

| ID | Milestone | Scope | Depends on | Exit evidence | Batch 1 stage |
|---|---|---|---|---|---|
| M0 | Hosted core (Batch 2) | Modules and tests in this package | Batch 1 | `scripts/verify.sh` exit 0 with recorded counts | 1 (part) |
| M1 | Toolchain hardening | Official Zig 0.15.2 (or LLVM build) pinned; verify.sh run in release modes; cross-compile for `aarch64-linux` and a freestanding x86-64/aarch64 target (compile only); reproducible-build check | M0 | Test run in Debug and ReleaseSafe on the full-featured compiler; freestanding objects build | 1 |
| M2 | Core completion | Capability **revocation** (choose among Batch 1 R1/R2/R3 after prototyping), handle **transfer between tables**, per-object reference counts and destruction, charge-transfer for shared objects (Batch 1 D5), cancellation objects as state machines (D2) | M0 | Property tests and a deterministic simulation harness (seeded) covering races as interleavings | 5, 6 |
| M3 | Concurrency model | Decide locking/atomics rules for tables and accounts; make hosted tests multi-threaded where meaningful | M2 | Documented lock order; stress tests; no claim beyond tested interleavings | 4, 5, 6 |
| M4 | Hosted kernel simulation | Hosted backend (Batch 1 section 14): simulated clock, interrupts, scheduler core and queues with deterministic replay | M2, M3 | Deterministic replay of scheduler/IPC scenarios | 4, 5 |
| M5 | Memory model | Physical allocator and address-space object (software model first), memory objects, page-fault policy hooks | M2 | Model tests; later VM tests | 3 |
| M6 | First architecture bring-up in a VM | Choose x86-64 or ARM64 first; boot entry, exception vectors, console, timer; implement `arch/interface.zig` contract for real | M1, M5 | A recorded VM boot log from an actual run | 2 |
| M7 | Second architecture | The other target, same contract | M6 | Recorded VM boot log | 2 |
| M8 | User-space transition | Syscall entry, user address windows validated with `UserWindow`, copy-in/copy-out with fault handling (TOCTOU rule), first user process | M5, M6 | Syscall fuzzing against the real entry path | 3, 5 |
| M9 | Services and drivers | Driver framework, interrupt delivery to user space, DMA policy, first virtual devices, `omni_clock`/`omni_random` services | M8 | Device tests in VM; entropy-source documentation | 7 |
| M10 | Filesystem, networking, GUI | `omni_fs`, `omni_net`, `omni_gui` services | M9 | Per-interface conformance tests | 8, 9 |
| M11 | Compatibility and Wasm (optional) | Isolated runtimes | M10 | Sandbox-escape review, fuzzing | 10 |
| M12 | Hardening | Fuzzing, external review, side-channel assessment | continuous | Reported findings and fixes | 11 |
| M13 | Exile OS integration | Staged and reversible; first requires inspecting Exile OS code (Batch 1 open question Q7) | M10 | Documented rollback per step | 12 |

Milestones M6 onward require emulator or hardware runs. None of them may be claimed from code
inspection alone.

## 4. Decisions that block later milestones

| Decision | Needed by | Notes |
|---|---|---|
| Revocation mechanism (Batch 1 D1: R1, R2, R3) | M2 | Batch 2 stores no derivation links, so adopting R2 will require adding parent tracking to table entries |
| Concurrency primitives | M3 | Batch 2 is single-threaded by design |
| Boot protocol per architecture | M6 | Not chosen; see arch READMEs |
| Handle table sizing policy (fixed `capacity` per process vs growable) | M2 | Batch 2 uses a comptime fixed capacity (provisional DEC-11) |
| Confirming the provisional ABI choices in `CORE-INTERFACES.md` section 8 | M2 | Each is marked provisional and unvalidated |
| Source of an official Zig toolchain in the build environment | M1 | The authoring environment could not download one |

## 5. Rules that carry forward

1. A milestone is complete only when its exit evidence has been produced by an actual run and recorded.
2. Architecture-specific and privileged code stays out of `src/core/`, which must keep building and
   testing on the host.
3. Any change that contradicts Batch 1 or an earlier batch carries a migration note.
4. Provisional decisions stay labelled provisional until a test or review validates them.
