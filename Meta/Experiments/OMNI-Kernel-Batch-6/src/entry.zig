//! Architecture-independent syscall entry (Batch 6): what common kernel code does after an
//! architecture stub has saved the user registers.
//!
//! HOSTED SIMULATION. `System.syscallEntry` is the function an x86-64 `syscall` stub or an
//! ARM64 `svc` vector would call. The stub itself does not exist. The function:
//!   1. checks the execution context (user mode, thread context, interrupts on, ...);
//!   2. validates the saved register state for the architecture;
//!   3. takes the calling thread from the CPU (trusted kernel state), refuses a thread
//!      that is not running or is being killed;
//!   4. derives the CALLING ADDRESS SPACE from that thread - never from any user value - and
//!      binds that space's user memory;
//!   5. captures the syscall number and arguments with the selected register convention;
//!   6. calls the unmodified Batch 5 `dispatch`, and writes the status into the result
//!      register.
//! A failure in steps 1-4 is FATAL for the process (no status reaches user mode); step 5
//! and 6 failures are ordinary negative statuses from Batch 5.

const std = @import("std");
const omni5 = @import("omni5");
const arch = @import("arch.zig");
const sched_mod = @import("sched.zig");
const um = @import("user_memory.zig");

const sys = omni5.syscall;
pub const KernelIpc = omni5.kernel_ipc.KernelIpc;

pub const Fatal = arch.EntryError || arch.ContextError || error{
    ArchMismatch,
    NoCurrentThread,
    ThreadNotRunning,
    ProcessDying,
    NoAddressSpace,
    WrongAddressSpace,
};

pub const EntryResult = union(enum) {
    returned: i64,
    fatal: Fatal,
};

pub const System = struct {
    x: *KernelIpc,
    s: *sched_mod.Sched,
    memory: um.Registry = .{},
    arch: arch.Arch,
    conv: arch.Convention,
    user_top: u64,

    pub fn syscallEntry(self: *System, cpu: u8, ctx: arch.ExecContext, regs: *arch.Regs) EntryResult {
        arch.checkSyscallContext(ctx) catch |e| return .{ .fatal = e };
        if (std.meta.activeTag(regs.*) != self.arch) return .{ .fatal = error.ArchMismatch };
        arch.validateEntry(regs, self.user_top) catch |e| return .{ .fatal = e };

        const tid = self.s.current(cpu) orelse return .{ .fatal = error.NoCurrentThread };
        const st = self.s.stateOf(tid) orelse return .{ .fatal = error.NoCurrentThread };
        if (st != .running) return .{ .fatal = error.ThreadNotRunning };
        // A process that is dying admits no new syscall (the existing Kernel/ does the same).
        if (self.s.killRequested(tid)) return .{ .fatal = error.ProcessDying };

        // TRUSTED: the caller space comes from the thread, never from the registers.
        const space = self.s.spaceOf(tid) orelse return .{ .fatal = error.NoAddressSpace };
        const model = self.memory.modelOf(space) orelse return .{ .fatal = error.NoAddressSpace };
        var binding = um.Binding.init(space, model) catch return .{ .fatal = error.WrongAddressSpace };

        const status: i64 = if (arch.captureArgs(self.conv, regs)) |args|
            sys.dispatch(self.x, binding.memory(), space, args)
        else |_|
            sys.status_no_sys;
        arch.setResult(regs, status);
        return .{ .returned = status };
    }
};
