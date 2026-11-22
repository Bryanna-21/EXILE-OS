# EUK Mission D3 — scheduler dispatch and x86-64 context switching

Status: **cooperative, single-CPU kernel threads, demonstrated on QEMU (TCG).** The scheduler, stack manager and real context switch are linked into the kernel only in the demo build (`--demo`, cfg `euk_d3_demo`). The normal boot path (`BOOT_OK`) is unchanged and does not start the scheduler. Nothing has run on real hardware.

## Single authority

- `Scheduler<N>` (`sched.rs`) owns the only `ThreadTable`, all thread states, run queues, the idle identity, blocking, sleeping, wake, termination and reaping.
- `KThreadStacks<N>` (`kthread.rs`) owns only stacks, initial frames and each thread's saved stack pointer, keyed by the full generation-checked `Handle`. It has no thread table, no state and no `on_cpu`.
- `Kernel<N, M>` (`dispatch.rs`) binds the two. `KernelSlot` is a static-friendly `SpinLock<Option<Kernel>>`.
- The D2 `KThreadManager` (a second table) is gone. `thread.rs` is the canonical D1 file.

## Creation (transactional)

`reserve()` / `reserve_idle()` allocate a `New` slot that is invisible to dispatch. A stack is allocated, the initial frame written, then `publish()` (New -> Ready only). Any failure calls `discard_reserved()` and releases the stack. `publish` rejects non-`New` threads (a test found and fixed an earlier flaw there).

## Destruction and reclamation

`destroy` order: `Scheduler::reap_ready` (Terminated and not `on_cpu`) -> scrub stack -> `reap` -> release stack to the pool. A thread cannot destroy itself while still on its stack (`SwitchInProgress` / refused). A stack returns to the pool only after the switch away from it has completed.

## Switch protocol

1. `plan_switch` (lock held): picks the next thread, marks outgoing/incoming, records `pending_prev`. Outgoing stays `on_cpu`.
2. `switch_context` (lock released, interrupts disabled by contract): saves rbx, rbp, r12-r15 on the outgoing stack, stores rsp, loads the incoming rsp, pops, `ret`.
3. `complete_switch` runs on the incoming stack: a new thread via the trampoline's first-run hook, a resumed thread right after `switch_context` returns. It clears the outgoing thread's `on_cpu`.

Initial frame (64 bytes, lowest first): r15=0, r14=0, r13=arg, r12=entry, rbp=0, rbx=0, ret=trampoline, 0 sentinel. The trampoline runs the first-run hook, aligns the stack (`sub rsp, 8`; entry sees rsp = top-24), calls `entry(arg)` and passes the return value to the exit hook, which terminates the thread through `exit_current`. If an exit hook ever returned, the trampoline halts. The idle thread (priority 0) cannot enter ordinary exit.

## Interrupt and lock boundary (single CPU, no SMP claim)

`SpinLock` is a test-and-set lock that does NOT mask interrupts and is not recursive. D3 never enables interrupts: the demo runs with IF clear and a software tick from the idle thread. Rules: the lock is never held across `switch_context`; callers must have interrupts disabled around plan/switch/complete; an interrupt handler must not take the lock until an IRQ-saving variant exists. Real preemption from a hardware timer is NOT implemented.

## Unsafe code

`spin.rs` (lock internals), `switch.rs` (asm and `switch_context`), the `real` module in `dispatch.rs` (calls through the static slot), and the demo's raw memory access. Each is commented.

## Verification

Host tests (`python3 scripts/test.py`): 108 D1-D3 tests plus unchanged A1/C1 tests, and a `#![no_std] -D warnings` compile check. The `switch` and real-dispatch tests EXECUTE the real assembly in user mode on the x86-64 host (register preservation, first-run, trampoline alignment, exit path, yield/block/wake across real stacks). Others are model tests of the state machine, queues, stack pool and rollback. 40 repeated runs were stable.

Emulator tests (`python3 scripts/boot_test.py`): all A1/C1 cases plus `d3_demo` on QEMU `pc` and `q35` (exit 33, 16 ordered serial checkpoints): two worker threads, yield and resume, block and wake, sleep, exit through the trampoline, refused self-destroy while on CPU, and reclamation only after switch-away (`context_switches=8`, `stacks_in_use=1`).

Mutation checks (deliberate breakage, then restored): host mutants (on-CPU guard, publish state check, rollback, frame alignment, etc.) and two emulator mutants (swapped r12/r13 pops; first-run hook skipping `complete_switch`) were all detected. A few host mutants surfaced as process crashes rather than clean assertion failures.

## Not verified / limitations

- Kernel images in this environment are NOT the pinned reproducible build: rustc is 1.97 (pinned 1.91.1), built with `EUK_ALLOW_UNPINNED_RUSTC=1 EUK_TARGET=host` against host `core` plus a freestanding mem shim (`hostmem.rs`). The pinned `x86_64-unknown-none` path was not run. No reproducibility claim.
- No hardware timer, IDT or interrupts; cooperative switching only. No SMP, FPU/SSE state, TLS/FS/GS, CR3 switching, user mode or full interrupt-frame switching.
- Stack guard regions are reserved but not unmapped; overflow is not trapped.
- The `publish`-failure rollback branch cannot be forced by tests.
- The mission's five-files-per-batch limit was exceeded.
