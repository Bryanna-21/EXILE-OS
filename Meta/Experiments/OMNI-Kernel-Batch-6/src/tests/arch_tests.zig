//! Architecture interface tests. SIM: register state is a value, not a CPU.
const std = @import("std");
const c = @import("common.zig");
const arch = c.arch;
const testing = std.testing;

test "ARCH-01 SIM omni_proposed register mapping on x86-64 and ARM64" {
    const args = [6]u64{ 11, 22, 33, 44, 55, 66 };
    for ([_]arch.Arch{ .x86_64, .aarch64 }) |a| {
        const regs = c.regsFor(a, .omni_proposed, 0x102, args);
        const got = try arch.captureArgs(.omni_proposed, &regs);
        try testing.expectEqual(@as(u32, 0x102), got.nr);
        try testing.expectEqualSlices(u64, &args, &got.a);
    }
    // The documented registers, spelled out.
    const x = c.regsFor(.x86_64, .omni_proposed, 0x103, args).x86_64;
    try testing.expectEqual(@as(u64, 0x103), x.rax);
    try testing.expectEqual(@as(u64, 11), x.rdi);
    try testing.expectEqual(@as(u64, 22), x.rsi);
    try testing.expectEqual(@as(u64, 33), x.rdx);
    try testing.expectEqual(@as(u64, 44), x.r10);
    try testing.expectEqual(@as(u64, 55), x.r8);
    try testing.expectEqual(@as(u64, 66), x.r9);
    const r = c.regsFor(.aarch64, .omni_proposed, 0x103, args).aarch64;
    try testing.expectEqual(@as(u64, 0x103), r.x[8]);
    for (0..6) |i| try testing.expectEqual(args[i], r.x[i]);
}

test "ARCH-02 SIM kernel_existing mapping: four arguments, the rest zero" {
    // Source of truth: Kernel/Arch/{x86_64,aarch64}/RegisterState.h capture_syscall_params.
    var x = arch.Regs{ .x86_64 = .{ .rax = 7, .rdx = 1, .rdi = 2, .rbx = 3, .rsi = 4, .r10 = 99, .r8 = 98, .r9 = 97, .rcx = 96 } };
    const gx = try arch.captureArgs(.kernel_existing, &x);
    try testing.expectEqual(@as(u32, 7), gx.nr);
    try testing.expectEqualSlices(u64, &.{ 1, 2, 3, 4, 0, 0 }, &gx.a);
    var r = arch.Regs{ .aarch64 = .{} };
    r.aarch64.x[8] = 9;
    r.aarch64.x[0] = 777; // x0 is the RESULT register under this convention, never an argument
    r.aarch64.x[1] = 1;
    r.aarch64.x[2] = 2;
    r.aarch64.x[3] = 3;
    r.aarch64.x[4] = 4;
    r.aarch64.x[5] = 55;
    const ga = try arch.captureArgs(.kernel_existing, &r);
    try testing.expectEqual(@as(u32, 9), ga.nr);
    try testing.expectEqualSlices(u64, &.{ 1, 2, 3, 4, 0, 0 }, &ga.a);
}

test "ARCH-03 syscall numbers wider than 32 bits are rejected, never truncated" {
    for ([_]arch.Arch{ .x86_64, .aarch64 }) |a| {
        for ([_]arch.Convention{ .omni_proposed, .kernel_existing }) |conv| {
            const bad = [_]u64{ 0x1_0000_0102, std.math.maxInt(u64), 1 << 32 };
            for (bad) |nr| {
                const regs = c.regsFor(a, conv, nr, .{ 0, 0, 0, 0, 0, 0 });
                try testing.expectError(error.BadSyscallNumber, arch.captureArgs(conv, &regs));
            }
            const ok = c.regsFor(a, conv, std.math.maxInt(u32), .{ 0, 0, 0, 0, 0, 0 });
            try testing.expectEqual(@as(u32, std.math.maxInt(u32)), (try arch.captureArgs(conv, &ok)).nr);
        }
    }
}

test "ARCH-04 SIM result register: negative statuses round-trip, other registers untouched" {
    var x = c.regsFor(.x86_64, .omni_proposed, 1, .{ 1, 2, 3, 4, 5, 6 });
    const before = x.x86_64;
    for ([_]i64{ 0, 7, -1, -33, std.math.minInt(i64), std.math.maxInt(i64) }) |v| {
        arch.setResult(&x, v);
        try testing.expectEqual(v, arch.getResult(&x));
    }
    x.x86_64.rax = before.rax;
    try testing.expectEqual(before, x.x86_64);
    var r = c.regsFor(.aarch64, .omni_proposed, 1, .{ 1, 2, 3, 4, 5, 6 });
    const rb = r.aarch64;
    arch.setResult(&r, -17);
    try testing.expectEqual(@as(i64, -17), arch.getResult(&r));
    try testing.expectEqual(rb.x[8], r.aarch64.x[8]);
    try testing.expectEqual(rb.pc, r.aarch64.pc);
    try testing.expectEqual(rb.sp, r.aarch64.sp);
}

test "ARCH-05 SIM x86-64 entry validation" {
    const top = c.um.user_top_x86_64;
    var regs = c.regsFor(.x86_64, .omni_proposed, 1, .{ 0, 0, 0, 0, 0, 0 });
    try arch.validateEntry(&regs, top);
    regs.x86_64.cs = 0x08; // kernel code segment
    try testing.expectError(error.NotFromUser, arch.validateEntry(&regs, top));
    regs.x86_64.cs = 0x23;
    regs.x86_64.rflags |= 3 << 12; // IOPL 3
    try testing.expectError(error.BadFlags, arch.validateEntry(&regs, top));
    regs.x86_64.rflags = 0x202 | (1 << 12);
    try testing.expectError(error.BadFlags, arch.validateEntry(&regs, top));
    regs.x86_64.rflags = 0x202;
    regs.x86_64.rip = top; // first kernel-half address is not user
    try testing.expectError(error.BadPc, arch.validateEntry(&regs, top));
    regs.x86_64.rip = 0xffff_ffff_8000_0000;
    try testing.expectError(error.BadPc, arch.validateEntry(&regs, top));
    regs.x86_64.rip = top - 1;
    try arch.validateEntry(&regs, top);
    regs.x86_64.rsp = top;
    try testing.expectError(error.BadStack, arch.validateEntry(&regs, top));
}

test "ARCH-06 SIM ARM64 entry validation" {
    const top = c.um.user_top_aarch64;
    var regs = c.regsFor(.aarch64, .omni_proposed, 1, .{ 0, 0, 0, 0, 0, 0 });
    try arch.validateEntry(&regs, top);
    regs.aarch64.esr = (0x24 << 26) | (1 << 25); // data abort, not SVC
    try testing.expectError(error.BadException, arch.validateEntry(&regs, top));
    regs.aarch64.esr = (arch.aarch64_ec_svc64 << 26) | (1 << 25) | 1; // svc #1
    try testing.expectError(error.BadException, arch.validateEntry(&regs, top));
    regs.aarch64.esr = (arch.aarch64_ec_svc64 << 26) | (1 << 25);
    regs.aarch64.pstate = 0x5; // EL1h
    try testing.expectError(error.NotFromUser, arch.validateEntry(&regs, top));
    regs.aarch64.pstate = 0x10; // AArch32 state
    try testing.expectError(error.NotFromUser, arch.validateEntry(&regs, top));
    regs.aarch64.pstate = 0;
    regs.aarch64.pc = top;
    try testing.expectError(error.BadPc, arch.validateEntry(&regs, top));
    regs.aarch64.pc = 0x1000;
    regs.aarch64.sp = std.math.maxInt(u64);
    try testing.expectError(error.BadStack, arch.validateEntry(&regs, top));
}

test "ARCH-07 execution-context rules for syscalls and for blocking" {
    try arch.checkSyscallContext(.{});
    try testing.expectError(error.NotFromUser, arch.checkSyscallContext(.{ .from_user = false }));
    try testing.expectError(error.InIrq, arch.checkSyscallContext(.{ .in_irq = true }));
    try testing.expectError(error.IrqDisabled, arch.checkSyscallContext(.{ .irq_enabled = false }));
    try testing.expectError(error.InCritical, arch.checkSyscallContext(.{ .critical_depth = 1 }));
    try testing.expectError(error.LocksHeld, arch.checkSyscallContext(.{ .spinlocks_held = 2 }));
    // A kernel thread may block, a syscall requires user mode.
    try arch.checkMayBlock(.{ .from_user = false });
    try testing.expectError(error.InIrq, arch.checkMayBlock(.{ .from_user = false, .in_irq = true }));
    // In IRQ context nothing may block, whatever else is set.
    try testing.expectError(error.InIrq, arch.checkMayBlock(.{ .in_irq = true, .irq_enabled = false, .critical_depth = 3 }));
}

test "ARCH-08 trap decisions: every trap kind from every context" {
    const T = arch.Trap;
    const A = arch.TrapAction;
    // syscall
    try testing.expectEqual(A.dispatch_syscall, arch.actionFor(T.syscall, true, false));
    try testing.expectEqual(A.kernel_panic, arch.actionFor(T.syscall, false, false));
    // page fault
    try testing.expectEqual(A.resolve_or_kill, arch.actionFor(T.page_fault, true, false));
    try testing.expectEqual(A.resolve_or_kill, arch.actionFor(T.page_fault, true, true));
    try testing.expectEqual(A.fail_copy, arch.actionFor(T.page_fault, false, true));
    try testing.expectEqual(A.kernel_panic, arch.actionFor(T.page_fault, false, false));
    // exceptions
    try testing.expectEqual(A.kill_process, arch.actionFor(T.user_exception, true, false));
    try testing.expectEqual(A.kernel_panic, arch.actionFor(T.user_exception, false, false));
    try testing.expectEqual(A.kernel_panic, arch.actionFor(T.kernel_exception, true, false));
    try testing.expectEqual(A.kernel_panic, arch.actionFor(T.kernel_exception, false, true));
    // irq
    try testing.expectEqual(A.handle_irq, arch.actionFor(T.irq, true, false));
    try testing.expectEqual(A.handle_irq, arch.actionFor(T.irq, false, false));
    // Only a syscall trap can ever dispatch.
    for ([_]T{ .page_fault, .user_exception, .kernel_exception, .irq }) |t| {
        for ([_]bool{ true, false }) |from_user| {
            for ([_]bool{ true, false }) |copy| {
                try testing.expect(arch.actionFor(t, from_user, copy) != A.dispatch_syscall);
            }
        }
    }
}
