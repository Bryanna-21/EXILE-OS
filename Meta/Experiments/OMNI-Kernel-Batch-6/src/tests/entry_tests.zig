//! Syscall entry tests. SIM: the "architecture stub" is the test itself, which builds
//! saved-register state and calls the architecture-independent entry.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const Rig = c.Rig;

const Combo = struct { a: c.arch.Arch, conv: c.arch.Convention };
const combos = [_]Combo{
    .{ .a = .x86_64, .conv = .omni_proposed },
    .{ .a = .x86_64, .conv = .kernel_existing },
    .{ .a = .aarch64, .conv = .omni_proposed },
    .{ .a = .aarch64, .conv = .kernel_existing },
};

fn mk(cb: Combo, cfg: c.kipc.Config) !*Rig {
    return Rig.createWith(testing.allocator, testing.allocator, cfg, cb.a, cb.conv);
}

fn cleanup(r: *Rig, ps: []const c.Proc) !void {
    try r.audit();
    for (ps) |p| r.freeProc(p);
    try testing.expectEqual(@as(usize, 0), r.destroy());
}

test "ENT-01 SIM endpoint_create, send and recv through the entry on x86-64 and ARM64, both conventions" {
    for (combos) |cb| {
        const r = try mk(cb, .{});
        const a = try r.newProc();
        const b = try r.newProc();
        _ = try r.spawnOn(a, 0);
        _ = try r.spawnOn(b, 1);

        // endpoint_create writes two handles to user memory
        try testing.expectEqual(@as(i64, 0), try c.call(r, 0, .endpoint_create, .{ c.A_MISC, 0, 0, 0, 0, 0 }));
        var raw: [8]u8 = undefined;
        try a.model.userLoad(c.A_MISC, &raw);
        const h0 = std.mem.readInt(u32, raw[0..4], .little);
        const h1 = std.mem.readInt(u32, raw[4..8], .little);
        try testing.expect(h0 != 0 and h1 != 0 and h0 != h1);
        // hand end 1 to B (the kernel-side transfer stands in for a capability message)
        const b1 = try r.k.transfer(a.space, c.Handle.fromRaw(h1), b.space);

        var buf: [64]u8 = undefined;
        try c.prepSend(a, c.msg(&buf, 1, "hello", 0), 0, 0, 0);
        try testing.expectEqual(@as(i64, 0), try c.call(r, 0, .send, .{ h0, c.A_ARGS, 0, 0, 0, 0 }));

        try c.prepRecv(b, 0, 0);
        try testing.expectEqual(@as(i64, 0), try c.call(r, 1, .recv, .{ @intFromEnum(b1), c.A_ARGS, 0, 0, 0, 0 }));
        try testing.expectEqual(@as(u32, 5), try c.resU32(b, 0));
        var got: [5]u8 = undefined;
        try b.model.userLoad(c.A_RBUF, &got);
        try testing.expectEqualStrings("hello", &got);
        // the result register carries the status
        var regs = c.regsFor(cb.a, cb.conv, @intFromEnum(c.sys.Nr.recv), .{ @intFromEnum(b1), c.A_ARGS, 0, 0, 0, 0 });
        _ = r.sysm.syscallEntry(1, .{}, &regs);
        try testing.expectEqual(@as(i64, -7), c.arch.getResult(&regs)); // empty now: would_block
        try cleanup(r, &.{ a, b });
    }
}

test "ENT-02 SIM invalid entry states are fatal for the process and change nothing" {
    const r = try Rig.create(testing.allocator, .{});
    const a = try r.newProc();
    const ta = try r.spawnOn(a, 0);
    const good = c.regsFor(.x86_64, .omni_proposed, @intFromEnum(c.sys.Nr.cancel_create), .{ 0, 0, 0, 0, 0, 0 });

    var regs = good;
    try c.expectFatal(r.sysm.syscallEntry(0, .{ .from_user = false }, &regs), error.NotFromUser);
    try c.expectFatal(r.sysm.syscallEntry(0, .{ .in_irq = true }, &regs), error.InIrq);
    try c.expectFatal(r.sysm.syscallEntry(0, .{ .irq_enabled = false }, &regs), error.IrqDisabled);
    try c.expectFatal(r.sysm.syscallEntry(0, .{ .critical_depth = 1 }, &regs), error.InCritical);
    try c.expectFatal(r.sysm.syscallEntry(0, .{ .spinlocks_held = 1 }, &regs), error.LocksHeld);
    // context is checked before the registers: both bad, the context error wins
    regs.x86_64.cs = 0x08;
    try c.expectFatal(r.sysm.syscallEntry(0, .{ .in_irq = true }, &regs), error.InIrq);
    try c.expectFatal(r.sysm.syscallEntry(0, .{}, &regs), error.NotFromUser);
    regs = good;
    regs.x86_64.rflags |= 3 << 12;
    try c.expectFatal(r.sysm.syscallEntry(0, .{}, &regs), error.BadFlags);
    regs = c.regsFor(.aarch64, .omni_proposed, 1, .{ 0, 0, 0, 0, 0, 0 });
    try c.expectFatal(r.sysm.syscallEntry(0, .{}, &regs), error.ArchMismatch);
    regs = good;
    try c.expectFatal(r.sysm.syscallEntry(2, .{}, &regs), error.NoCurrentThread); // idle cpu
    try c.expectFatal(r.sysm.syscallEntry(200, .{}, &regs), error.NoCurrentThread); // no such cpu
    // fatal entries never touch the result register or create anything
    try testing.expectEqual(good.x86_64.rax, regs.x86_64.rax);
    try testing.expectEqual(@as(usize, 0), try r.k.spaceHandleCount(a.space));

    // a dying process admits no new syscall
    _ = r.s.kill(ta);
    try c.expectFatal(r.sysm.syscallEntry(0, .{}, &regs), error.ProcessDying);
    try r.s.exit(ta);
    try c.expectFatal(r.sysm.syscallEntry(0, .{}, &regs), error.NoCurrentThread);

    // a thread whose space has no registered memory
    const bare = try r.k.createSpace(c.space_budget);
    const tbare = try r.s.create(bare);
    try r.s.dispatch(tbare, 1);
    try c.expectFatal(r.sysm.syscallEntry(1, .{}, &regs), error.NoAddressSpace);
    try r.s.exit(tbare);

    // a model that no longer belongs to the thread's space is refused, not trusted
    const ta2 = try r.spawnOn(a, 0);
    const other = c.SpaceId{ .index = 200, .gen = 9 };
    const saved = a.model.owner;
    a.model.owner = other;
    try c.expectFatal(r.sysm.syscallEntry(0, .{}, &regs), error.WrongAddressSpace);
    a.model.owner = saved;
    try testing.expect((try c.call(r, 0, .cancel_create, .{ 0, 0, 0, 0, 0, 0 })) > 0); // the thread works again
    try r.s.exit(ta2);
    try cleanup(r, &.{a});
}

test "ENT-03 SIM the caller is the thread's space: registers cannot select another address space" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const intruder = try r.newProc();
    _ = try r.spawnOn(p.a, 0);
    _ = try r.spawnOn(p.b, 1);
    _ = try r.spawnOn(intruder, 2);
    var buf: [64]u8 = undefined;

    // The intruder presents A's handle value and stuffs every spare register with A's and B's
    // space identity. The unused registers must be zero, so the call is refused outright ...
    try c.prepSend(intruder, c.msg(&buf, 1, "x", 0), 0, 0, 0);
    try testing.expectEqual(@as(i64, -1), try c.call(r, 2, .send, .{ @intFromEnum(p.a0), c.A_ARGS, p.a.space.index, p.a.space.gen, p.b.space.index, p.b.space.gen }));
    // ... and with clean registers the handle simply does not exist in the intruder's table.
    try testing.expectEqual(@as(i64, -2), try c.call(r, 2, .send, .{ @intFromEnum(p.a0), c.A_ARGS, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(i64, -2), try c.call(r, 2, .endpoint_close, .{ @intFromEnum(p.a0), 0, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(i64, -2), try c.call(r, 2, .endpoint_stats, .{ @intFromEnum(p.b1), c.A_MISC, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(u32, 0), (try r.x.endpointStats(p.b.space, p.b1)).queued_messages);
    try testing.expectEqual(@as(u64, 0), try r.k.spaceUsed(p.a.space, .queued_bytes));
    try cleanup(r, &.{ p.a, p.b, intruder });
}

test "ENT-05 SIM unused-register rule: enforced by omni_proposed; kernel_existing cannot carry registers it never captures" {
    for ([_]c.arch.Convention{ .omni_proposed, .kernel_existing }) |conv| {
        const r = try Rig.createWith(testing.allocator, testing.allocator, .{}, .x86_64, conv);
        const p = try c.pair(r);
        _ = try r.spawnOn(p.a, 0);
        var buf: [64]u8 = undefined;
        try c.prepSend(p.a, c.msg(&buf, 1, "x", 0), 0, 0, 0);
        const st = try c.call(r, 0, .send, .{ @intFromEnum(p.a0), c.A_ARGS, 0, 0, 0, 0 });
        try testing.expectEqual(@as(i64, 0), st);
        // set r8 (a4 under omni_proposed) by hand: kernel_existing never reads r8
        var regs = c.regsFor(.x86_64, conv, @intFromEnum(c.sys.Nr.send), .{ @intFromEnum(p.a0), c.A_ARGS, 0, 0, 0, 0 });
        regs.x86_64.r8 = 1;
        regs.x86_64.r9 = 1;
        _ = r.sysm.syscallEntry(0, .{}, &regs);
        const want: i64 = if (conv == .omni_proposed) -1 else 0;
        try testing.expectEqual(want, c.arch.getResult(&regs));
        try cleanup(r, &.{ p.a, p.b });
    }
}

test "ENT-06 SIM unknown and over-wide syscall numbers return no_sys; only the result register changes" {
    for (combos) |cb| {
        const r = try mk(cb, .{});
        const a = try r.newProc();
        _ = try r.spawnOn(a, 0);
        for ([_]u64{ 0, 0xFF, 0x108, 0xFFFF_FFFF, 0x1_0000_0102, std.math.maxInt(u64) }) |nr| {
            var regs = c.regsFor(cb.a, cb.conv, nr, .{ 1, 2, 3, 4, 5, 6 });
            const before = regs;
            const res = r.sysm.syscallEntry(0, .{}, &regs);
            try testing.expectEqual(@as(i64, -33), res.returned);
            try testing.expectEqual(@as(i64, -33), c.arch.getResult(&regs));
            switch (regs) {
                .x86_64 => |x| {
                    var want = before.x86_64;
                    want.rax = x.rax;
                    try testing.expectEqual(want, x);
                },
                .aarch64 => |x| {
                    var want = before.aarch64;
                    want.x[0] = x.x[0];
                    try testing.expectEqual(want, x);
                },
            }
        }
        try cleanup(r, &.{a});
    }
}

fn validStatus(s: i64) bool {
    return s >= 0 or (s >= -17 and s <= -1) or s == c.sys.status_fault or s == c.sys.status_no_sys;
}

test "ENT-04 SIM seeded fuzz through the entry: random registers and user blocks, valid statuses only (seeds 1..16)" {
    var seed: u64 = 1;
    while (seed <= 16) : (seed += 1) {
        errdefer std.debug.print("entry fuzz failure: seed={d}\n", .{seed});
        var prng = std.Random.DefaultPrng.init(seed);
        const rnd = prng.random();
        const cb = combos[seed % combos.len];
        const r = try mk(cb, .{ .ipc = .{ .max_messages = 4 } });
        const p = try c.pair(r);
        _ = try r.spawnOn(p.a, 0);
        _ = try r.spawnOn(p.b, 1);
        const obj = try r.k.createObject(p.a.space, .memory_object, c.rights(c.R_READ | c.R_XFER | c.R_DUP), .{});
        const hs = [_]u64{ @intFromEnum(p.a0), @intFromEnum(p.b1), @intFromEnum(obj), 0 };
        var i: u32 = 0;
        while (i < 300) : (i += 1) {
            const cpu: u8 = if (rnd.boolean()) 0 else 1;
            const pr = if (cpu == 0) p.a else p.b;
            var blk: [48]u8 = undefined;
            rnd.bytes(&blk);
            if (rnd.boolean()) { // mostly-valid block
                std.mem.writeInt(u64, blk[0..8], c.A_MSG, .little);
                std.mem.writeInt(u32, blk[8..12], rnd.uintLessThan(u32, 200), .little);
                std.mem.writeInt(u32, blk[12..16], rnd.uintLessThan(u32, 4), .little);
                std.mem.writeInt(u64, blk[16..24], c.A_DESC, .little);
                @memset(blk[36..48], 0);
            }
            // keep waits short: forever -> poll, long -> 2 ms
            const t = std.mem.readInt(u64, blk[24..32], .little);
            if (t == c.kipc.wait_forever) {
                std.mem.writeInt(u64, blk[24..32], 0, .little);
            } else if (t > 2 * c.ms and t <= 30 * std.time.ns_per_s) std.mem.writeInt(u64, blk[24..32], 2 * c.ms, .little);
            try pr.model.userStore(c.A_ARGS, &blk);
            var junk: [64]u8 = undefined;
            rnd.bytes(&junk);
            try pr.model.userStore(c.A_MSG, &junk);
            var args: [6]u64 = undefined;
            for (&args) |*a| a.* = switch (rnd.uintLessThan(u8, 6)) {
                0 => rnd.int(u64),
                1 => 0,
                2 => c.A_ARGS,
                3 => hs[rnd.uintLessThan(usize, hs.len)],
                4 => c.A_MISC,
                else => hs[rnd.uintLessThan(usize, hs.len)] | (@as(u64, 1) << 32),
            };
            const nr: u64 = if (rnd.uintLessThan(u8, 10) < 8) 0x100 + rnd.uintLessThan(u64, 8) else rnd.int(u64);
            var regs = c.regsFor(cb.a, cb.conv, nr, args);
            switch (r.sysm.syscallEntry(cpu, .{}, &regs)) {
                .returned => |s| try testing.expect(validStatus(s)),
                .fatal => return error.UnexpectedFatal,
            }
            if (i % 100 == 99) try r.audit();
        }
        try cleanup(r, &.{ p.a, p.b });
    }
}
