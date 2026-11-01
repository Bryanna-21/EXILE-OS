# Context switching

Status: a CONTRACT with a hosted checker (`src/context.zig`). **No context switch is implemented.** `SimCpu` updates
bookkeeping and enforces preconditions; it saves and restores no register, stack or page table.

## Architecture-independent contract (checked by CTX-01)

Before a switch from `from` (or from idle) to `to` on CPU `n`:

1. not in IRQ context;
2. interrupts disabled on this CPU;
3. exactly one critical section (the scheduler's) and no spinlock held;
4. `from` is the thread this CPU runs, and `to` is a different thread;
5. `from` has already left `running` (runnable, blocked, dying or stopped) and released the CPU;
6. `to` exists and is `runnable`.

Effects: `to` becomes `running` on CPU `n`; the address space is switched only when it differs from the loaded one;
the CPU's current thread is updated. On any error nothing changes. A thread is never `running` on two CPUs.

## Architecture-specific requirements (specified)

| | x86-64 | ARM64 |
|---|---|---|
| minimum registers | rbx, rbp, r12-r15, rsp, resume rip (the existing kernel saves more: rflags, rcx, rsi, rdi, r8-r11) | x19-x28, x29, x30, sp |
| address space | CR3 (PCID optional) | TTBR0_EL1 + ASID; barriers (dsb/isb) |
| thread pointer | FS base (and GS base for per-CPU) | TPIDR_EL0 |
| kernel stack | TSS rsp0 updated to the new thread's stack top | SP_EL1 / per-CPU stack |
| FPU/SIMD | one policy, eager or lazy, applied consistently | one policy for FP/SIMD |
| interrupt state | rflags.IF saved and restored with the thread | DAIF saved and restored |

## Address-space switching

The scheduler (not the syscall path) owns it, at the switch. The thread's owner space never changes, so the loaded space
after a switch equals the owner of the thread now running (checked). Kernel threads without a user space need a
defined "no user space" state; not modelled.

## Interrupt state and preemption

Preemption requests set a flag; the switch happens at a safe point (return from IRQ to a preemptible context, or a
voluntary yield/block). Critical sections defer it. A thread must never be switched out while it holds a spinlock.

## Minimum interfaces before a real switch can be implemented safely

1. a per-CPU structure with current thread, kernel stack top, loaded address space, preempt count;
2. a thread control block with saved kernel `rsp`/`sp` and resume address, and a kernel stack with a guard page;
3. a first-entry path for new threads (they have no saved context to resume);
4. address-space objects with a loadable root (CR3/TTBR0 value) and ASID management;
5. the scheduler lock and run queue with the state machine of SCHEDULER-INTEGRATION.md;
6. a tested entry/exit path (the existing `Kernel/` has one; ExileKernel has none).

## Status

implemented: contract checker and bookkeeping (hosted). partial: none. specified: register sets, stack and
address-space steps above. deferred: assembly, FPU policy, SMP shootdowns, hardware tests.
