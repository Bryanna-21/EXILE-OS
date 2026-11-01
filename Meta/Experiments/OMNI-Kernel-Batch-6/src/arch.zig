//! Architecture interface (Batch 6): saved register state, syscall register conventions,
//! entry validation and the trap/execution-context rules.
//!
//! SPECIFICATION WITH A CHECKER. There is no entry stub and no assembly here. `Regs`
//! models the register state an architecture stub saves at kernel entry, and the
//! functions are what the architecture-independent code does with it. A real stub (x86-64
//! `syscall`, ARM64 `svc #0`) must produce exactly this state; docs/ARCHITECTURE-INTERFACE.md
//! says how. Nothing here executes on hardware.
//!
//! TWO syscall register conventions are modelled, because they genuinely differ:
//!   * `omni_proposed`  - the convention proposed in Batch 5 (x86-64 rdi, rsi, rdx, r10, r8, r9;
//!                        ARM64 x0..x5). Batch 5 is unchanged.
//!   * `kernel_existing`- the convention the EXISTING `Kernel/` uses, read from
//!                        `Kernel/Arch/x86_64/RegisterState.h` and
//!                        `Kernel/Arch/aarch64/RegisterState.h` (`capture_syscall_params`):
//!                        x86-64 rdx, rdi, rbx, rsi; ARM64 x1..x4 (x0 carries the result).
//! Which one a real integration uses is an open decision for the project owner.

const std = @import("std");
const omni5 = @import("omni5");
const sys = omni5.syscall;

pub const Args = sys.Args;

pub const Arch = enum { x86_64, aarch64 };

pub const X86Regs = struct {
    rax: u64 = 0,
    rbx: u64 = 0,
    rcx: u64 = 0,
    rdx: u64 = 0,
    rsi: u64 = 0,
    rdi: u64 = 0,
    rbp: u64 = 0,
    rsp: u64 = 0,
    r8: u64 = 0,
    r9: u64 = 0,
    r10: u64 = 0,
    r11: u64 = 0,
    r12: u64 = 0,
    r13: u64 = 0,
    r14: u64 = 0,
    r15: u64 = 0,
    rip: u64 = 0,
    rflags: u64 = 0,
    cs: u64 = 0,
    ss: u64 = 0,
};

pub const ArmRegs = struct {
    x: [31]u64 = [_]u64{0} ** 31,
    sp: u64 = 0,
    pc: u64 = 0,
    pstate: u64 = 0,
    /// ESR_EL1 as captured at the synchronous exception.
    esr: u64 = 0,
};

pub const Regs = union(Arch) {
    x86_64: X86Regs,
    aarch64: ArmRegs,
};

pub const Convention = enum { omni_proposed, kernel_existing };

pub const CaptureError = error{BadSyscallNumber};

/// Read the syscall number and arguments from the saved registers. A number that does not
/// fit 32 bits is rejected, never truncated. Arguments the convention does not carry are 0.
pub fn captureArgs(conv: Convention, regs: *const Regs) CaptureError!Args {
    var nr: u64 = undefined;
    var a: [6]u64 = undefined;
    switch (regs.*) {
        .x86_64 => |r| {
            nr = r.rax;
            a = switch (conv) {
                .omni_proposed => .{ r.rdi, r.rsi, r.rdx, r.r10, r.r8, r.r9 },
                .kernel_existing => .{ r.rdx, r.rdi, r.rbx, r.rsi, 0, 0 },
            };
        },
        .aarch64 => |r| {
            nr = r.x[8];
            a = switch (conv) {
                .omni_proposed => .{ r.x[0], r.x[1], r.x[2], r.x[3], r.x[4], r.x[5] },
                .kernel_existing => .{ r.x[1], r.x[2], r.x[3], r.x[4], 0, 0 },
            };
        },
    }
    if (nr > std.math.maxInt(u32)) return error.BadSyscallNumber;
    return .{ .nr = @intCast(nr), .a = a };
}

/// Write the result register (rax on x86-64, x0 on ARM64) as a two's-complement value.
pub fn setResult(regs: *Regs, v: i64) void {
    const raw: u64 = @bitCast(v);
    switch (regs.*) {
        .x86_64 => |*r| r.rax = raw,
        .aarch64 => |*r| r.x[0] = raw,
    }
}

pub fn getResult(regs: *const Regs) i64 {
    return switch (regs.*) {
        .x86_64 => |r| @bitCast(r.rax),
        .aarch64 => |r| @bitCast(r.x[0]),
    };
}

// ------------------------------------------------------------------ entry validation

pub const EntryError = error{ NotFromUser, BadFlags, BadPc, BadStack, BadException };

pub const aarch64_ec_svc64: u64 = 0x15;

/// Check the saved state of a syscall entry. Every failure is FATAL for the calling
/// process (no status is returned to user mode): the state is impossible for a correct
/// stub, or proves the user did something the architecture should not allow.
pub fn validateEntry(regs: *const Regs, user_top: u64) EntryError!void {
    switch (regs.*) {
        .x86_64 => |r| {
            if ((r.cs & 3) != 3) return error.NotFromUser;
            // IOPL must be 0: the existing Kernel/ panics on a syscall with IOPL != 0.
            if (((r.rflags >> 12) & 3) != 0) return error.BadFlags;
            if (r.rip >= user_top) return error.BadPc;
            if (r.rsp >= user_top) return error.BadStack;
        },
        .aarch64 => |r| {
            if (((r.esr >> 26) & 0x3F) != aarch64_ec_svc64) return error.BadException;
            // OMNI requires `svc #0`; the existing Kernel/ does not look at the immediate.
            if ((r.esr & 0xFFFF) != 0) return error.BadException;
            // PSTATE.M[4:0] == 0 is EL0t in AArch64 state; anything else is not user mode.
            if ((r.pstate & 0x1F) != 0) return error.NotFromUser;
            if (r.pc >= user_top) return error.BadPc;
            if (r.sp >= user_top) return error.BadStack;
        },
    }
}

// ------------------------------------------------------------------ execution context

/// What the architecture code knows about the CPU when it hands control to common code.
pub const ExecContext = struct {
    from_user: bool = true,
    in_irq: bool = false,
    irq_enabled: bool = true,
    /// Preemption-disable (critical section) nesting depth.
    critical_depth: u32 = 0,
    spinlocks_held: u32 = 0,
};

pub const ContextError = error{ NotFromUser, InIrq, IrqDisabled, InCritical, LocksHeld };

/// A syscall may be dispatched only from user mode, in thread context, with interrupts
/// enabled, preemption allowed and no spinlock held (a syscall may block).
pub fn checkSyscallContext(c: ExecContext) ContextError!void {
    if (!c.from_user) return error.NotFromUser;
    try checkMayBlock(c);
}

/// A thread may block only in thread context, interrupts enabled, preemption allowed and
/// holding no spinlock. (`Thread::block_impl` VERIFYs it is not in an IRQ.)
pub fn checkMayBlock(c: ExecContext) ContextError!void {
    if (c.in_irq) return error.InIrq;
    if (!c.irq_enabled) return error.IrqDisabled;
    if (c.critical_depth != 0) return error.InCritical;
    if (c.spinlocks_held != 0) return error.LocksHeld;
}

pub const Trap = enum { syscall, page_fault, user_exception, kernel_exception, irq };

pub const TrapAction = enum {
    dispatch_syscall,
    /// Try to resolve the fault (demand paging); kill the process if it cannot be resolved.
    resolve_or_kill,
    /// A fault inside a guarded kernel copy: return `Fault` to the copy's caller.
    fail_copy,
    kill_process,
    kernel_panic,
    /// Run the handler without blocking; wakeups are deferred.
    handle_irq,
};

/// The architecture-independent decision for each trap. `in_guarded_copy` is true when the
/// kernel was executing a fault-safe user copy (`copy_from_user` family) at the fault.
pub fn actionFor(trap: Trap, from_user: bool, in_guarded_copy: bool) TrapAction {
    return switch (trap) {
        .syscall => if (from_user) .dispatch_syscall else .kernel_panic,
        .page_fault => if (from_user) .resolve_or_kill else if (in_guarded_copy) .fail_copy else .kernel_panic,
        .user_exception => if (from_user) .kill_process else .kernel_panic,
        .kernel_exception => .kernel_panic,
        .irq => .handle_irq,
    };
}
