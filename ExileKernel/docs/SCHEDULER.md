# EUK Mission D — threads, processes and scheduling

Status: **superseded in part by Mission D3 (see `THREADS.md`).** The scheduler policy below (priority round-robin, aging, handles, run queues) is unchanged. As of D3 it is linked into the kernel in the `--demo` build only, with a real cooperative x86-64 context switch demonstrated on QEMU. Statements below about the D1 batch ("not compiled into the kernel", "no context switch", "blocked on C") describe the D1 baseline and are historical. The normal boot image does not run the scheduler.

## Baseline at start of Mission D

Repository HEAD `f75608a9` (`main`). `ExileKernel/` holds only the Mission A boot milestone (`entry.S`, `kernel.rs`, `memory.rs`, linker script). Missions B and C are **not present**: no physical allocator, kernel heap, page-table manager, GDT/TSS/IDT, IRQ dispatch, timer, per-CPU state, atomics/lock layer, context-switch assembly or FPU management. See `VERIFICATION.md` and `ARCHITECTURE.md`.

## Policy (implemented as a model)

- 8 priority levels. Level 0 is the idle thread only; ordinary threads use 1..=7; higher is more urgent. Default 4.
- Strict priority between levels, FIFO round-robin within a level. Preempted/yielding threads go to the back of their base-level queue.
- Time slice: `SLICE_TICKS = 4` timer ticks.
- Anti-starvation aging: a Ready thread waiting `AGING_TICKS = 16` ticks is boosted one level (repeatedly, to level 7) and loses the boost when dispatched. Starvation of a level-1 thread behind a permanently runnable level-7 thread is bounded (tested).
- Idle: always runnable, never blocks, sleeps or dies, never boosted. It does not count as competition for a running worker.
- Priority changes: immediate requeue for a Ready thread; for a Running thread, effective at next requeue (and a reschedule is requested if a higher level is waiting).
- Priority inheritance: **not implemented**. It is needed once blocking mutexes exist (Phase 4); until then priority inversion is not handled.
- Three distinct steps: `tick()` only advances time/accounting and may set `need_resched`; `schedule()` only chooses; the arch layer must perform the switch and then call `switch_complete(prev)`.

## Thread lifecycle

States: New, Ready, Running, Blocked, Sleeping, Terminated. Legal transitions (everything else is rejected and the full 6x6 table is tested):

```
New -> Ready | Terminated          Blocked  -> Ready | Terminated
Ready -> Running | Terminated      Sleeping -> Ready | Terminated
Running -> Ready | Blocked | Sleeping | Terminated      Terminated -> (final)
```

- Handles are `(slot, generation)`; stale handles fail with `Stale`. A slot whose generation counter would wrap is retired, never reused.
- Thread IDs are 64-bit, never reused; exhaustion is an error.
- A Terminated thread is reapable only when `on_cpu` is false, i.e. after `switch_complete`. This is the rule that will prevent freeing a stack still in use.
- Wake-up race: waking a Running thread records a pending wake-up consumed by its next `block_current`, so the wake-up cannot be lost. A sleep drops a stale pending wake-up.
- Sleep: wakes at or after the deadline, never early; zero ticks is an immediate yield point; `now + ticks` overflow is an error; an explicit wake cancels a sleep (`WakeReason::Cancelled`).
- Join: `join_wait` blocks until the target terminates; `reap` then returns the exit status.
- Killing a Running thread is refused; it must exit itself. Cross-CPU kill needs requirement 18.

## Locking rules (design, not yet enforced)

No lock layer exists yet (Mission C). Intended order, outermost first: process table → scheduler lock (IRQ-saving) → run queues/thread table → memory-management locks are **never** taken while holding the scheduler lock. Sleeping or blocking while holding a spinlock, and any blocking call from interrupt context, are forbidden. All `Scheduler` methods assume the caller already holds the scheduler lock with interrupts disabled; this batch provides no synchronization and is single-CPU only.

## What was verified (Batch 1)

Host-side unit/model tests only (`python3 scripts/test.py`): run-queue behaviour including a randomized comparison against a reference model; lifecycle table; stale handles; slot retirement; ID/slot exhaustion; round-robin, priority order, yield, block/wake, lost-wakeup race, sleep deadlines/cancel/overflow, slice expiry vs. reschedule vs. switch, preemption on wake/spawn, starvation bound, terminate/reap/join ordering, kill, set_priority, exhaustion rollback, 60k-step randomized operation sequences x4 seeds with invariants checked after every step, 32-thread fairness, 32 blocked threads, 10k create/destroy cycles. Compile-time check: the modules build as `#![no_std]` with `-D warnings`.

Not verified: any execution on x86-64 or QEMU, real context switching, interrupts, preemption from a hardware timer, SMP, races between CPUs.

## Requirement status after Batch 1

| # | Requirement | Status |
|---|---|---|
| 1 | Thread structure and lifecycle | Partial: states, IDs, handles, accounting, join semantics done; saved context, kernel stack, FPU, TLS, process link absent |
| 2 | Creation and destruction | Partial: table slot, ID and reap safety done; stack allocation needs Mission B |
| 3 | Initial stack and context | Blocked: needs Mission C context-switch ABI and Mission B stack/guard pages |
| 4 | Ready/blocked/terminated queues | Partial: allocation-free ready queues, duplicate prevention; blocked sets are state-only; no locking |
| 5 | Scheduler | Partial: selection policy and switch protocol; no real context switch (blocked on C) |
| 6 | Priority and fairness | Partial: policy implemented and tested in the model; no priority inheritance |
| 7 | Timer-driven preemption | Partial: tick/need_resched logic only; blocked on Mission C timer/interrupt frame |
| 8 | Yield and blocking | Partial: model only |
| 9 | Sleep and wake-up | Partial: tick-based model, O(N) expiry scan, no hardware timer |
| 10 | Synchronization primitives | Not started: blocked on Mission C locks/atomics |
| 11 | Lock order / deadlock diagnostics | Not started (order documented as design only) |
| 12-19 | Process, address spaces, user mode, ELF, TLS, events, accounting | Not started; 14, 15, 17 blocked on Missions B/C |
| 20 | Stress/starvation/race tests | Partial: host model stress and starvation only; no emulator or SMP tests |

## Known limitations

- Single CPU, no locking, no SMP readiness claim.
- `expire_sleepers`, `age` and `wake_joiners` scan all slots (O(N)); acceptable for a model, to be replaced before real use.
- `Thread::entry/arg` are stored but never executed.
- Preempted threads go to the back of their queue even when preempted by a higher-priority wake-up.

## Recommended next batch

Either (a) Mission C prerequisites in their own mission if the owner wants them, or (b) within Mission D scope that is still independently testable: a process/resource-limit accounting model (requirements 12, 13, 19) and a pure ELF header/segment validator (requirement 16) with host tests.
