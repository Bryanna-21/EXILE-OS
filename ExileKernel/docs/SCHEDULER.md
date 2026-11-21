# EUK Mission D — threads, stacks and scheduling

## Current implementation status

The repository contains a host-tested, `no_std`-compatible implementation
model for:

- generation-checked thread handles and lifecycle transitions;
- fixed-capacity, allocation-free priority run queues;
- scheduler policy, time slices, aging, sleep, wake and join bookkeeping;
- x86-64 initial kernel-stack context-frame construction;
- fixed-capacity kernel-stack slot ownership with guard-region reservations;
- thread creation, termination, stack scrubbing and destruction management;
- a basic spinlock and a shared thread-manager wrapper.

The modules are not yet integrated into the booted kernel execution path.
The host tests do not demonstrate a real CPU context switch, timer-driven
preemption, interrupt-safe locking, guard-page fault handling or SMP safety.
No claim is made that the kernel can currently execute multiple threads.

## Scheduler policy

- Eight priority levels. Level 0 is reserved for the idle thread;
  ordinary threads use levels 1 through 7. Higher values are more urgent.
- Strict priority between levels and FIFO round-robin within each level.
- A time slice is `SLICE_TICKS = 4` timer ticks.
- A ready thread waiting `AGING_TICKS = 16` ticks is boosted one level,
  up to level 7. The temporary boost is cleared when dispatched.
- The idle thread cannot block, sleep or terminate and is not treated as
  competition for a running worker.
- Ready-thread priority changes requeue the thread. Running-thread priority
  changes take effect on its next requeue.
- Priority inheritance is not implemented.

## Lifecycle and switch protocol

Thread states are `New`, `Ready`, `Running`, `Blocked`, `Sleeping` and
`Terminated`. Only documented lifecycle transitions are accepted.

Handles contain a slot and generation. Stale handles are rejected, and a
slot is retired rather than allowing its generation to wrap. Thread IDs are
monotonically allocated and are not reused.

Scheduling is deliberately separated into three operations:

1. `tick()` updates time/accounting and may request rescheduling.
2. `schedule()` selects the next runnable thread and returns a switch record.
3. The architecture layer must perform the actual context switch and call
   `switch_complete(prev)` after execution has moved away from the old thread.

A terminated thread cannot be reaped while it remains on-CPU. The manager
also validates that a handle belongs to its own metadata before performing
manager-level lifecycle operations. Stack destruction scrubs memory before
releasing the stack slot.

## Stack and context model

The stack pool uses fixed-size slots with reserved guard regions and
generation-checked ownership tokens. These guard regions are reservations in
the current model; actual unmapped guard pages and overflow fault handling
require page-table integration.

The x86-64 context module builds and serializes an initial context frame.
This is a frame-construction contract, not an assembly context-switch
implementation. The stack-memory interface has no production physical-memory
backend yet. Its `write` operation must be atomic on failure.

## Locking limitations

`SpinLock` uses acquire/release atomic ordering, but it does not disable
interrupts, provide IRQ-save semantics, guarantee fairness or support
recursive acquisition. An interrupt handler that attempts to acquire a lock
held by the code it interrupted can deadlock. The current design is not
SMP-ready and must not be treated as a complete kernel synchronization layer.

## Verification

`scripts/test.py` runs:

- memory-map parser unit tests;
- integrated thread, run-queue, scheduler, stack, context, manager and lock
  host tests;
- a `no_std` metadata compilation with warnings denied.

These are host-side tests. They do not boot the kernel or prove real
context switching, interrupt behavior, timer preemption, page protection,
or SMP correctness.

## Known limitations and next prerequisites

- The modules are not wired into `kernel.rs` or the boot path.
- No assembly context-switch routine or complete register/FPU state
  management is implemented.
- No IRQ-saving scheduler lock or interrupt dispatch/timer integration exists.
- Guard regions are not yet enforced by page tables.
- Stack memory has no production backend.
- Scheduler sleep expiry, aging and join wake-up use bounded table scans.
- Thread entry points are stored in the model; they are not yet executed.

Before boot-path integration, implement and validate the architecture switch
routine, interrupt/IRQ-save locking, page-backed stack mapping and guard-page
enforcement, and a real stack-memory backend. Then test under an emulator
before claiming runtime thread support.
