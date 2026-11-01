# Architecture interface

Status: SPECIFIED, with a hosted checker (`src/arch.zig`, `src/entry.zig`). No entry stub, no assembly,
no interrupt controller code exists in this package. Nothing here has run on hardware or an emulator.

## Baseline: what the repository actually contains (verified in source)

| item | finding | where |
|---|---|---|
| `Kernel/` | existing Serenity-derived C++ kernel with real x86-64, aarch64 and riscv64 code, threads, scheduler, `switch_context`, page-fault handling, safe user copies. OMNI is NOT part of it. | `Kernel/Arch/*`, `Kernel/Tasks/*` |
| `ExileKernel/` | a separate Rust x86-64 boot milestone: Multiboot entry, GDT, identity map, memory-map validation. "No user address space or scheduler exists"; AArch64 has no implementation. | `ExileKernel/docs/ARCHITECTURE.md` |
| x86-64 syscall entry in `Kernel/` | `syscall_entry` (naked) builds a `RegisterState`, calls `syscall_handler`; `int 0x82` is also registered. | `Kernel/Arch/x86_64/SyscallEntry.cpp`, `Interrupts.cpp` |
| ARM64 syscall entry in `Kernel/` | EC "SVC from AArch64" in the synchronous exception handler calls `syscall_handler`. | `Kernel/Arch/aarch64/Interrupts.cpp` |
| existing register convention | x86-64: number rax, arguments rdx, rdi, rbx, rsi; ARM64: number x8, arguments x1..x4 (x0 is the result). Four arguments. | `RegisterState.h::capture_syscall_params` |
| Batch 5 proposal | x86-64: rax; rdi, rsi, rdx, r10, r8, r9; ARM64: x8; x0..x5. Six arguments. | Batch 5 `docs/SYSCALL-CONTRACT.md` |

**The Batch 5 proposal and the existing `Kernel/` convention differ.** Batch 6 does not change Batch 5. Both are
implemented as selectable conventions (`Convention.omni_proposed`, `Convention.kernel_existing`) and tested.
Choosing one is a decision for the project owner; the ABI is not fixed here. Batch 5's operations use only two
arguments, so either convention can carry them. Under `kernel_existing`, registers it does not capture are
treated as zero, so Batch 5's "unused registers must be zero" rule cannot be enforced for them (ENT-05).

## Common syscall-entry interface

An architecture stub must, before calling common code (`System.syscallEntry(cpu, ctx, regs)`):

1. have switched to the kernel stack and saved ALL user registers into a `Regs` value (x86-64: including rcx = user rip
   and r11 = user rflags, which `syscall` overwrites; ARM64: x0..x30, SP_EL0, ELR (pc), SPSR (pstate), ESR);
2. report honestly in `ExecContext`: whether it came from user mode, whether it is in IRQ context, whether
   interrupts are enabled, the critical-section depth and spinlocks held;
3. identify the CPU (`cpu`), NOT the process: the calling thread and address space are looked up from trusted
   per-CPU kernel state, never from registers;
4. on return, restore registers and return to user mode (`sysret`/`iret`, `eret`), with the result in rax / x0.

Common code (implemented, hosted) then: checks the context; checks the register state (`validateEntry`);
takes the current thread of `cpu`; refuses a thread that is being killed; derives the address space from the
thread; binds that space's user memory; captures arguments with the selected convention; calls the unmodified
Batch 5 `dispatch`; writes the status to the result register.

### x86-64 requirements (specified)

* `syscall` entry: IA32_LSTAR points at the stub; IA32_STAR selects segments; IA32_FMASK clears IF, TF, DF, AC (so
  the stub starts with interrupts off, no trap flag, direction flag clear, SMAP not bypassed). The stub swaps GS
  (`swapgs`) to reach per-CPU data, loads the kernel stack, and may enable interrupts only after the register state
  is saved.
* `validateEntry`: CS RPL 3, IOPL 0 (the existing kernel panics otherwise), RIP and RSP below the user limit
  (`0x0000_8000_0000_0000`).
* result in rax; `sysret` is only safe with a canonical RIP and, for the return path, rcx/r11 restored.

### ARM64 requirements (specified)

* synchronous exception from a lower EL, EC = 0x15 (SVC, AArch64); OMNI additionally requires the immediate to be 0
  (`svc #0`); SPSR M[4:0] = 0 (EL0t, AArch64 state); PC and SP below the user limit (`0x0001_0000_0000_0000`, 48-bit VA).
* the vector entry saves x0..x30, SP_EL0, ELR_EL1, SPSR_EL1, ESR_EL1; result in x0; return with `eret`.
* PAN: privileged access to user memory must go through the fault-safe copy functions only.

## Interrupt and exception boundary

`arch.actionFor(trap, from_user, in_guarded_copy)` is the architecture-independent decision (tested exhaustively):

| trap | from user | from kernel |
|---|---|---|
| syscall | dispatch | impossible: kernel panic |
| page fault | resolve (demand paging) or kill the process | in a guarded copy: fail the copy with `Fault`; otherwise panic |
| user exception | kill the process | panic |
| kernel exception | n/a | panic |
| IRQ | handle without blocking; wake-ups deferred | same |

Rules: an IRQ handler never blocks and never calls blocking IPC; it may only request a wake-up in a deferred context
(the existing kernel uses `Processor::deferred_call_queue` for `Thread::unblock_from_blocker` in IRQ). Interrupts
are enabled during syscall dispatch; they are disabled across a context switch.

## Transfer of control: arch code to common code

stub saves registers -> builds `Regs` + `ExecContext` -> `System.syscallEntry` -> (Batch 5 `dispatch`) -> result in
`Regs` -> stub restores registers -> return to user. A FATAL result (`EntryResult.fatal`) means the process must be
terminated; the stub must not return to user mode with a made-up status.

## Implemented, partial, specified, deferred

* implemented (hosted): `Regs`, both conventions, entry validation, context rules, trap table, `System.syscallEntry`.
* partial: dispatch of BLOCKING syscalls goes through Batch 5's own hosted wait queue, not the scheduler model (see
  SCHEDULER-INTEGRATION.md, "Known gap").
* specified only: the stubs themselves, MSR/vector setup, `swapgs`/PAN handling, the final convention and numbers.
* deferred: riscv64, 32-bit compatibility (`int 0x82`), signals, ptrace.
