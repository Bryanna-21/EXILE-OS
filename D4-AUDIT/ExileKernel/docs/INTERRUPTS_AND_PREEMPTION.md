# EUK Mission D4 — interrupts, hardware timer and timer-driven preemption

Status: **implemented and demonstrated on QEMU (TCG, `pc` and `q35`); single CPU.** The code is linked into every kernel image, but it RUNS only in the `--d4` demo builds (cfg `euk_d4_demo`). The normal boot path (`EUK BOOT_OK`) and the D3 demo are unchanged and still pass. Nothing has run on real hardware.

## Scope and how it was chosen
The repository defines no "D4". The scope is Mission D phase 3 (requirements 7 and 9: timer-driven preemption, hardware-tick sleep) plus the prerequisites it needs and the C1 modules did not provide (a loaded IDT, entry stubs, an interrupt controller, a timer, an IRQ-safe lock). Choices made without a decision from the owner, all recorded here: PIT + 8259 PIC (not LAPIC), 100 Hz, interrupt-exit switching with a full register frame, FXSAVE for FPU/SSE state. Mission D phase 4 (mutexes, semaphores, wait queues, lock-order diagnostics) is NOT part of D4.

## Modules
| File | Role |
|---|---|
| `src/irq.rs` | `save_disable`/`restore`/`enable`, `IrqOff` (RAII), `IrqSpinLock` (clears IF, then locks; unlocks, then restores IF) |
| `src/preempt.rs` | `PREEMPT` (preemption-disable depth), `HARDIRQ` (handler depth), pure `irq_exit_action` decision |
| `src/timer.rs` | PIT divisor, checked ms/tick conversion (sleeps round up), saturating `TickClock` |
| `src/arch/x86_64/pic.rs` | 8259 remap/EOI/spurious check, PIT programming, over a `PortIo` trait |
| `src/arch/x86_64/trap.rs` | 48 entry stubs, common save/restore with FXSAVE, IDT build + `lidt`, handler registry, dispatch |
| `src/dispatch.rs` (changed) | IRQ-saving scheduler lock; interrupt-atomic block/sleep/join/yield; `preempt_disable/enable`; `irq_exit_reschedule` |
| `src/kthread.rs` (changed) | `Error::{InterruptContext, PreemptDisabled, PreemptCount}` |
| `src/d4_demo.rs` | on-target demonstration |
| `src/kernel.rs`, `scripts/{build,boot_test,test}.py` (changed) | module wiring, `--d4` variants, QEMU cases, host harness |

D1-D3 interfaces are unchanged except: `KernelSlot::with` now also disables interrupts (a strengthening), the blocking calls return the new errors in the new situations, and `Error` gained three variants.

## The three steps (Mission D 5.7) are separate
1. **Delivery**: the vector-32 stub saves state and calls the handler with `HARDIRQ` raised. The handler acknowledges the PIC, advances `CLOCK` and calls `Scheduler::tick`.
2. **Request**: `tick` sets `need_resched`. Nothing switches yet.
3. **Switch**: after the handler returns and `HARDIRQ` is lowered, the exit hook calls `irq_exit_reschedule`, which uses `irq_exit_action(need, PREEMPT, HARDIRQ, switch_pending)`: `None`, `Deferred` (preemption disabled, nested interrupt, or a switch still pending; the request stays pending) or `Reschedule`. A reschedule runs `plan_switch` / `switch_context` / `complete_switch` on the interrupted thread's own kernel stack.

Re-enabling preemption (`preempt_enable`) runs a deferred reschedule when the depth reaches zero.

## Preserved state
The trap stub pushes all 15 general registers and executes `fxsave` (512 bytes) on the thread's kernel stack, then `fxrstor`/`iretq` on return. The interrupted thread resumes with every register and its x87/SSE state intact even though other threads and the handler itself use them. CR4.OSFXSR was already set by `entry.S`. Not saved: AVX/XSAVE state, FS/GS base, debug registers, CR3 (none are used).

## Interrupt and lock rules
* All gates are interrupt gates (IF clear on entry) with IST 0; interrupts do not nest.
* The scheduler lock is an `IrqSpinLock`; every use clears IF first, so a handler cannot deadlock on a lock held by the code it interrupted.
* `reschedule`, `block_current`, `sleep_current`, `join_wait` run state change + switch with interrupts off and restore the caller's own previous IF state after being resumed. A new thread starts with IF clear and its first-run hook enables it.
* A thread cannot yield, block, sleep or join from interrupt context (`InterruptContext`) or while preemption is disabled (`PreemptDisabled`). Exiting threads and the idle thread rules are unchanged.
* #BP is counted and resumed. Every other exception calls the fatal hook (diagnostic, never returns). A spurious IRQ 7 is counted and not acknowledged; an unregistered IRQ is counted and acknowledged.
* Sleeps use `sched` ticks, now driven by the PIT; `timer::ms_to_ticks_ceil` rounds up so a sleep is never early.

## Verification (see `VERIFICATION_RESULTS.md` for exact output)
Host: 184 tests in the D1-D4 binary (108 D1-D3, 49 new D4, 27 gdt/idt included only because `trap` depends on them), A1/C1 modules as before, `#![no_std] -D warnings` compile check. The IRQ-lock tests use a model of RFLAGS.IF; the trap stubs are assembled but never executed on the host; the real context-switch assembly runs in user mode on the host.
QEMU (`scripts/boot_test.py`): all earlier cases + D4 demo on `pc` and `q35` (12 ordered checkpoints) + three expected-failure variants (`#UD`, `#GP` with error code `0x28`, and a build without the interrupt-exit switch that the watchdog must catch).

## Limitations and blockers
* **Not the pinned build.** Rust 1.91.1 could not be installed (`static.rust-lang.org` returned 403 through the proxy). Images were built with rustc 1.97 against host `core` plus `hostmem.rs` (`EUK_ALLOW_UNPINNED_RUSTC=1 EUK_TARGET=host`). The pinned-toolchain build and its QEMU run are NOT done; no reproducibility claim.
* Single CPU only; no SMP, no APIC/IOAPIC/HPET, no TSC calibration; the PIT rate is nominal and QEMU TCG timing is not wall-clock accurate.
* No TSS/IST: a double fault or stack overflow runs on the current stack and can triple-fault. Guard pages are reserved but not unmapped.
* No user mode, TLS, address-space switching, signals or process objects (Mission D 12-19).
* No mutex/semaphore/wait queue/lock-order diagnostics (Mission D 10-11) beyond the IRQ-saving spinlock and preemption counter.
* Cooperative-switch correctness against nested/recursive interrupts is not claimed (interrupts do not nest by construction).
* Priority inversion handling is undefined: no sleepable locks exist yet.
* The `no_eoi` and `no_irq_enable` mutants end in a harness timeout, not a diagnostic: with the tick stopped, the in-kernel watchdog cannot fire.
* The Mission D five-files-per-batch rule was exceeded (13 files changed or added).
