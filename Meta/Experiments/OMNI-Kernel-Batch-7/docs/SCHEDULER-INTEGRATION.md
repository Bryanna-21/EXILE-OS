# Scheduler

## Model

Uniprocessor, cooperative, round-robin FIFO. A thread leaves the CPU only by calling `yield_now`, `block` or
`exit_current` (directly or through a syscall). There is **no preemption**: a thread that never yields starves
everything else, and a timer interrupt can wake sleepers (`tick`) but cannot take the CPU away.

Threads never switch directly to each other. They switch to the scheduler context (the caller of `Sched::run`),
which picks the next thread. This keeps one rule for every transition: *a stack is resumed only by `run`.*

## Thread table

16 slots, 16 KiB stacks inside the scheduler (no allocator needed). A `ThreadId` is `(index, generation)`; freeing a
slot increments the generation, so a stale id (`wake`, `kill`, `state_of`) fails with `Stale` and can never name the
thread that reused the slot (`ctx_08`).

States: Free, Runnable, Running, Blocked (Blocked with a deadline reports as Sleeping). Termination is the sticky
`killed` flag plus `exit_current`; there is no Zombie / wait state in this batch (the exit code goes to a 16-entry
log, readable with `exit_code_of`; there is no `join`).

## Transitions

| from | to | by |
|---|---|---|
| Free | Runnable | `spawn` |
| Runnable | Running | `run` picks it (validated first, see below) |
| Running | Runnable | yield, or a block whose wake already arrived |
| Running | Blocked | block committed with no intervening wake |
| Blocked | Runnable | `wake` / `cancel` / `kill` / `tick` past the deadline |
| Running | Free | `exit_current`, or reaping after a canary failure |
| Runnable | Free | killed before its first run (never switched in), or refused switch |

Anything else is not representable: there is no API that sets a state directly.

## Block / wake without lost wake-ups

```
t = prepare_block()      snapshot of the thread's sequence number; clears a stale pending reason
... caller re-checks its condition ...
block(t, deadline)       under the lock: if seq != t.seq or killed -> return at once
                         else record action=Block, then switch to the scheduler
```
`wake` always does `seq += 1` and raises `pending` (precedence none < timeout < woken < cancelled < killed); it moves
the thread to Runnable only if it is Blocked. A thread that has switched away is still `Running` until the scheduler
context *commits* the block; the commit re-compares `seq` with the snapshot and keeps the thread Runnable if a wake
arrived in the window. Three windows are therefore covered, each by its own test:

1. wake before `block` (an interrupt landing after `prepare_block`): `sch_04`;
2. wake after the thread left but before the commit: `sch_05` (fault-injected through a test-only hook);
3. wake from another CPU at an arbitrary instant: `sch_17`, 20 000 handshakes between the scheduler on one host OS
   thread and a waker on another. Disabling the commit re-check makes `sch_05` and `sch_17` fail; I ran that mutation.

No lock is held across a context switch; on bare metal the lock masks interrupts (`sync.rs`), so `wake`, `kill` and
`tick` are safe from interrupt context. That bare-metal masking path has not been run.

## Before every switch-in (`pick`)

A thread is switched into only if: its slot is Runnable with a matching generation; it was not killed before its first
run (otherwise it is reaped without ever running); its stack canary is intact; its saved rsp is 16-byte aligned and
the whole switch frame lies inside its own stack. Failure frees the slot and counts `refused_switches`
(`ctx_06`, `ctx_07`). After every switch-out the canary is checked again.

## Address-space handling

Each thread has a `SpaceId` (0 = kernel). `pick` updates `loaded_space` and calls the optional space hook **only when
the space changes**; on bare metal that hook would write `cr3`. The tests prove the hook runs before the thread does
and is skipped for same-space neighbours (`sch_16`). No page tables exist in this batch.

## Process cleanup

`terminate_space(space)` marks every thread of the space killed and wakes the blocked ones; each exits at its next
block, syscall boundary or (if it never ran) is reaped unrun. `Spaces::destroy` then drops the address space
(`ua_09`). A thread running pure user code that never makes a syscall would not notice the kill: **no preemption**.

## Differences from the Batch 6 model and from `Kernel/`

* Batch 6's `Sched` also had `stopped` and `dying` states; this scheduler has neither.
* `Kernel/` (C++) has SMP, preemption, per-CPU scheduler locks and deferred wake-ups from IRQ context; none of that
  is here.
* Batch 5's IPC `dispatch` blocks in its own wait queue, invisible to this scheduler. Connecting blocking IPC would
  mean calling `prepare_block` / `block` from the IPC layer and `wake` from the sender. **Not done**; the
  scheduler API is shaped for it, nothing proves it.
