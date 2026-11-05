# OMNI-Kernel Batch 6 report

**Scope statement:** this package is hosted. It models and checks the architecture and scheduler boundary of OMNI-Kernel.
It does **not** implement real architecture entry, hardware context switching or scheduler integration, and it is
**not integrated into any kernel**. Batch 5 (hosted, kernel-facing) is not represented as integrated or independently
confirmed: its own verification result is the one recorded in its report.

## Identification

* Branch: `experiment/omni-kernel-batch-6`
* Base: Batch 5 commit `0d2f7d97826a54b6540a01f6d1dcbb29dd661777` (branch `experiment/omni-kernel-batch-5`), which is
  based on Batch 4 commit `faa9273ee6c51f3150abaca3ab71b24e80bc325a`. Neither is on `origin/main`
  (`3922a59aeb1beb484a266013f963d12c2b444a71`, the Batch 3 merge, when this was written).
* Commit SHA: reported in the hand-off message (a file cannot contain the hash of the commit that adds it).
* Compiler actually used: a locally built Zig 0.15.2 (stage3, no LLVM, Debug). An official release was not available in
  the build environment; verification with an official 0.15.2 remains pending.

## Baseline inspection (verified in source, not assumed from documentation)

| question | finding |
|---|---|
| Batches 1-5 | Batch 1 is documentation; Batches 2-5 are hosted Zig prototypes (capability kernel, IPC core, syscall dispatcher). None is linked into a kernel. |
| `Kernel/` | existing C++ kernel (Serenity-derived): `Kernel/Arch/{x86_64,aarch64,riscv64}`, `Kernel/Tasks/{Thread,Scheduler,Process,...}`, `Processor::switch_context`, `safe_memcpy`/`copy_from_user`. |
| `ExileKernel/` | Rust x86-64 boot milestone only (Multiboot entry, GDT, identity map, memory-map checks). No scheduler, no user address space, no AArch64. |
| x86-64 / ARM64 syscall support | both exist in `Kernel/` (`syscall_entry`, `int 0x82`; SVC exception class), neither in OMNI or `ExileKernel/`. |
| thread / scheduler / context-switch interfaces | exist in `Kernel/` (`Thread::State`, `block_impl`, `set_state`, `Scheduler::context_switch`, `switch_context`); not called by anything here. |
| syscall register convention | existing: x86-64 rax; rdx, rdi, rbx, rsi. ARM64 x8; x1..x4. Batch 5 proposal: rax; rdi, rsi, rdx, r10, r8, r9 / x8; x0..x5. **They differ.** |
| Batch 5 interfaces used | `syscall.dispatch`, `UserMemory`, `KernelIpc` non-blocking calls, `Ledger` waiter slots, `validateWait`, `spaceClose`. Unchanged. |

## Commands run

```
export ZIG_LOCAL_CACHE_DIR=... ZIG_GLOBAL_CACHE_DIR=...   # outside the repo
cd Meta/Experiments/OMNI-Kernel-Batch-6
zig fmt --check build.zig build.zig.zon src
zig build test --summary all
bash scripts/verify.sh            # OMNI_BASE_REF=0d2f7d97 to check the diff scope
```

## Results (last full run before packaging)

| suite | tests |
|---|---|
| Batch 6 (`src/tests.zig`) | 54 reported = 53 named + the root test block (arch 8, sched 12, ipc 14, entry 6, mem 8, proc 5) |
| Batch 5 regression (unmodified sources) | 57 |
| Batch 4 regression (unmodified sources) | 55 |
| Batch 3 regression (unmodified sources) | 57 |
| Batch 2 regression (unmodified sources) | 59 |
| total | 282 |

The Batch 6 test binary was run 12 times in a row without a failure (thread tests). A deliberate defect (disabling the
ticket comparison in `Sched.blockLocked`) made WK-02 fail; it was reverted. The final fresh-clone `verify.sh` output is
given in the hand-off message. Skipped: nothing in `zig build test`; `verify.sh` skips the base-ref diff unless
`OMNI_BASE_REF` is set.

## 1. Implemented and tested (hosted)

* Register state for x86-64 and ARM64, both syscall conventions, entry validation, execution-context rules, trap table.
* Architecture-independent syscall entry calling the unmodified Batch 5 `dispatch`, with the calling space taken from the
  thread, fatal handling of invalid entry states, user-memory binding per address space.
* Thread-state transition table (exhaustive) and a scheduler MODEL with the block/wake/kill protocol; 384 interleavings.
* Blocking IPC through the model: wake-up, timeout, cancellation, close, kill, waiter-slot bounds, exactly-once delivery,
  races (timeout vs arrival, cancel vs arrival, timeout vs wake), concurrent senders and receivers.
* Process termination and cleanup (blocked threads, queued messages, in-flight capabilities); 100 lifecycles.
* User-memory page model with page-spanning copies, permissions, range and overflow checks, probe-first copy_out, race
  injection, post-consume fault recovery.
* Context-switch precondition and bookkeeping checker.

## 2. Implemented but not fully verified

* Thread-safety of the models: stress and race tests only, x86-64 Linux, Debug, one compiler, hosted OS threads.
* The two blocking paths (see gap below): `IpcSched` is tested; the syscall path blocks in Batch 5's own queue.
* Timing assertions (`no earlier than`, `no later than 3 s`) use wall-clock time; they passed in every run here.
* `runnable -> dying` and `stopped -> dying` in the transition table are this package's reading of the C++ source and have
  not been confirmed by its maintainers.
* ENT-04 fuzz asserts valid statuses and invariants only (16 seeds x 300 calls).

## 3. Specified but not implemented

* x86-64 `syscall` and ARM64 `svc` entry stubs, MSR/vector setup, `swapgs`/PAN handling.
* Real context switch (register save/restore, stacks, CR3/TTBR0, FPU policy), first-entry path for new threads.
* Real scheduler, run queues, preemption, timers, IRQ-context wake-ups through deferred calls.
* Fault-safe copy primitives and page-fault handling on real page tables.
* A wait-provider hook in Batch 5 `dispatch`, so blocking syscalls use the scheduler.

## 4. Deferred

* Choosing the production syscall register convention and numbers (needs owner approval).
* riscv64, 32-bit compatibility entry, signals, ptrace, SMP, TLB shootdown, fairness and priorities.
* Integration into `Kernel/` or `ExileKernel/`, emulator or hardware tests.
* An official Zig 0.15.2 run.

## Known gap and design decisions worth review

* **Two blocking paths.** `syscall.dispatch` blocks inside Batch 5's hosted wait queue; `Sched` cannot see or kill such a
  thread (PR-03). Whole-process termination still frees it. Batch 5 was not modified to fix this.
* Batch 6 `IpcSched` re-implements the retry loop on Batch 5's non-blocking calls rather than changing Batch 5.
* The eventcount is the model's mechanism; the existing kernel uses locks held across condition test and registration.
  Both give the property "test and block are one step with respect to wakers".
* A fatal entry (invalid state) returns no status; the stub must terminate the process.
* Under `kernel_existing`, registers the convention never captures cannot be checked for zero.
* Batch 5 counters count each non-blocking attempt, so blocked `IpcSched` calls inflate `*_failed` counters.

## Pre-existing findings (not fixed, outside Batch 6)

* Batch 2 `scripts/verify.sh` is mode 644 (exit 126 when run directly; run via `bash`) and its manifest does not list
  `EXILE-OS-STATUS.md`.
* The Batch 4 and Batch 5 branches are local only: the git proxy refused a push (403, repository not authorized in the
  session), so integration needs the bundles delivered with each batch.

## Layout notes

No deviation from the requested file list except `src/omni5.zig` (re-export glue for the Batch 5 sources) and
`src/context.zig`, `src/entry.zig`, `src/ipc_sched.zig`, `src/user_memory.zig`, which are the implementation files.
