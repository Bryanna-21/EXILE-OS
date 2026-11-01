//! Context-switch contract (Batch 6).
//!
//! CONTRACT CHECKER, NOT A CONTEXT SWITCH. `SimCpu.contextSwitch` switches no registers and
//! no stacks. It enforces, against the hosted scheduler model, the PRECONDITIONS and the
//! BOOKKEEPING a real switch must respect, so that a later assembly implementation has a
//! precise, tested contract to satisfy (docs/CONTEXT-SWITCHING.md). Architecture-specific
//! register and stack requirements are listed as data (`callee_saved`) and in the document.

const std = @import("std");
const sched_mod = @import("sched.zig");
const arch = @import("arch.zig");

pub const Sched = sched_mod.Sched;
pub const ThreadId = sched_mod.ThreadId;
pub const SpaceId = sched_mod.SpaceId;

/// The MINIMUM register set a switch between two threads that both gave up the CPU
/// voluntarily must preserve: the callee-saved registers of the architecture's calling
/// convention (System V AMD64, AAPCS64), the kernel stack pointer and the resume address.
/// The existing `Processor::switch_context` in `Kernel/Arch/x86_64/Processor.cpp` saves a
/// LARGER set (rflags, rbx, rcx, rbp, rsi, rdi, r8-r15) because the same path serves
/// preemption; a real implementation may save more, never less.
pub fn calleeSaved(a: arch.Arch) []const []const u8 {
    return switch (a) {
        .x86_64 => &.{ "rbx", "rbp", "r12", "r13", "r14", "r15", "rsp", "rip (resume address)" },
        .aarch64 => &.{ "x19", "x20", "x21", "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29 (fp)", "x30 (lr)", "sp" },
    };
}

/// State that is NOT part of the callee-saved set but must be handled by the switch.
pub fn extraState(a: arch.Arch) []const []const u8 {
    return switch (a) {
        .x86_64 => &.{ "CR3 (address space)", "FPU/SSE state (eager or lazy, one policy)", "FS/GS base (thread pointer)", "TSS rsp0 / per-CPU kernel stack top" },
        .aarch64 => &.{ "TTBR0_EL1 + ASID (address space)", "FP/SIMD state", "TPIDR_EL0 (thread pointer)", "SP_EL0 / per-CPU kernel stack top" },
    };
}

pub const SwitchError = error{
    InIrq,
    IrqEnabled,
    CriticalDepth,
    LocksHeld,
    SameThread,
    FromNotCurrent,
    FromStillRunning,
    ToMissing,
    ToNotRunnable,
    NoAddressSpace,
};

pub const SimCpu = struct {
    id: u8,
    current: ?ThreadId = null,
    loaded_space: ?SpaceId = null,
    switches: u64 = 0,
    space_switches: u64 = 0,

    /// Switch this CPU from `from` (null for the first switch / from idle) to `to`.
    ///
    /// Preconditions (all checked, in this order):
    ///   1. not in IRQ context;
    ///   2. interrupts disabled on this CPU;
    ///   3. exactly one critical section (the scheduler's own) and no spinlock held;
    ///   4. `from` is the thread this CPU is running, and `to` differs from it;
    ///   5. `from` has ALREADY left `running` (the scheduler moved it to runnable, blocked,
    ///      dying or stopped BEFORE the switch, and released the CPU);
    ///   6. `to` exists and is `runnable`.
    /// Effects: `to` becomes `running` on this CPU; the address space is switched only if it
    /// differs from the one loaded; `current` is updated. On error NOTHING changes.
    pub fn contextSwitch(self: *SimCpu, s: *Sched, ctx: arch.ExecContext, from: ?ThreadId, to: ThreadId) SwitchError!void {
        if (ctx.in_irq) return error.InIrq;
        if (ctx.irq_enabled) return error.IrqEnabled;
        if (ctx.critical_depth != 1) return error.CriticalDepth;
        if (ctx.spinlocks_held != 0) return error.LocksHeld;
        if (from) |f| {
            if (self.current == null or self.current.? != f) return error.FromNotCurrent;
            if (f == to) return error.SameThread;
            if (s.stateOf(f)) |st| {
                if (st == .running) return error.FromStillRunning;
            }
        } else if (self.current != null) {
            return error.FromNotCurrent;
        }
        const to_space = s.spaceOf(to) orelse return error.ToMissing;
        const st = s.stateOf(to) orelse return error.ToMissing;
        if (st != .runnable) return error.ToNotRunnable;

        s.dispatch(to, self.id) catch |e| switch (e) {
            error.CpuBusy => return error.FromStillRunning,
            error.NoSuchThread => return error.ToMissing,
            else => return error.ToNotRunnable,
        };
        // --- architecture step: save `from` callee-saved registers and kernel sp, load
        // `to`'s, and (below) switch the address space; simulated by bookkeeping only ---
        if (self.loaded_space) |ls| {
            if (!ls.eql(to_space)) {
                self.loaded_space = to_space;
                self.space_switches += 1;
            }
        } else {
            self.loaded_space = to_space;
            self.space_switches += 1;
        }
        self.current = to;
        self.switches += 1;
    }
};
