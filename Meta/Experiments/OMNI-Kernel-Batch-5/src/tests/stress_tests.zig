//! Concurrency stress and syscall fuzzing (recorded seeds).
//!
//! Thread-safety is claimed ONLY for what these tests exercise: concurrent syscalls
//! and kernel-facing calls over one `KernelIpc` and one `Kernel`, on x86-64 Linux in
//! a Debug build. See docs/SCHEDULER-CONTRACT.md for what is not claimed.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const ipc4 = c.ipc4;
const Atomic = std.atomic.Value;

const Shared = struct {
    rig: *c.Rig,
    a: c.SpaceId,
    b: c.SpaceId,
    send_ep: c.Handle,
    recv_ep: c.Handle,
    per_producer: u32,
    total: u32,
    consumed: Atomic(u32) = .init(0),
    produced: Atomic(u32) = .init(0),
    fail: Atomic(u32) = .init(0),
    seen: []Atomic(u8),
};

fn newUser() !*c.User {
    const u = try testing.allocator.create(c.User);
    u.* = .{};
    try u.setup();
    return u;
}

fn producer(sh: *Shared, pid: u8) void {
    const u = newUser() catch {
        _ = sh.fail.fetchAdd(1, .monotonic);
        return;
    };
    defer testing.allocator.destroy(u);
    var i: u32 = 0;
    while (i < sh.per_producer) : (i += 1) {
        var payload: [5]u8 = undefined;
        payload[0] = pid;
        std.mem.writeInt(u32, payload[1..5], i, .little);
        var buf: [64]u8 = undefined;
        u.prepSend(c.msg(&buf, 1, &payload, 0), 0, 10_000 * c.ms, 0);
        const r = u.call(sh.rig, sh.a, .send, .{ @intFromEnum(sh.send_ep), c.User.A_ARGS, 0, 0, 0, 0 });
        if (r != 0) {
            _ = sh.fail.fetchAdd(1, .monotonic);
            return;
        }
        _ = sh.produced.fetchAdd(1, .monotonic);
    }
}

fn consumer(sh: *Shared) void {
    const u = newUser() catch {
        _ = sh.fail.fetchAdd(1, .monotonic);
        return;
    };
    defer testing.allocator.destroy(u);
    var last = [_]i64{-1} ** 8;
    while (sh.consumed.load(.acquire) < sh.total) {
        u.prepRecv(20 * c.ms, 0);
        const r = u.call(sh.rig, sh.b, .recv, .{ @intFromEnum(sh.recv_ep), c.User.A_ARGS, 0, 0, 0, 0 });
        if (r == -8) continue; // timeout while idle: re-check the exit condition
        if (r != 0 or u.resPayloadLen() != 5) {
            _ = sh.fail.fetchAdd(1, .monotonic);
            return;
        }
        const pid = u.rbuf[0];
        const seq = std.mem.readInt(u32, u.rbuf[1..5], .little);
        if (pid >= 8 or @as(i64, seq) <= last[pid]) _ = sh.fail.fetchAdd(1, .monotonic); // per-consumer order
        if (pid < 8) last[pid] = seq;
        const idx = @as(usize, pid) * sh.per_producer + seq;
        if (sh.seen[idx].fetchAdd(1, .monotonic) != 0) _ = sh.fail.fetchAdd(1, .monotonic); // duplicate
        _ = sh.consumed.fetchAdd(1, .release);
    }
}

test "STRESS-01 four producers, three consumers over syscalls: every message exactly once, order kept" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 4 } });
    const p = try c.pair(&rig);
    const per: u32 = 300;
    const total: u32 = per * 4;
    const seen = try testing.allocator.alloc(Atomic(u8), total);
    defer testing.allocator.free(seen);
    for (seen) |*s| s.* = .init(0);
    var sh = Shared{ .rig = &rig, .a = p.a, .b = p.b, .send_ep = p.a0, .recv_ep = p.b1, .per_producer = per, .total = total, .seen = seen };
    var cons: [3]std.Thread = undefined;
    var prods: [4]std.Thread = undefined;
    for (&cons) |*t| t.* = try std.Thread.spawn(.{}, consumer, .{&sh});
    for (&prods, 0..) |*t, i| t.* = try std.Thread.spawn(.{}, producer, .{ &sh, @as(u8, @intCast(i)) });
    for (prods) |t| t.join();
    for (cons) |t| t.join();
    try testing.expectEqual(@as(u32, 0), sh.fail.load(.acquire));
    try testing.expectEqual(total, sh.consumed.load(.acquire));
    for (seen) |*s| try testing.expectEqual(@as(u8, 1), s.load(.acquire));
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
    try testing.expectEqual(@as(u32, 0), rig.x.ledger.waitersTotal());
    try rig.consistent();
    try testing.expectEqual(@as(usize, 0), rig.deinit());
}

const CapShared = struct {
    rig: *c.Rig,
    a: c.SpaceId,
    b: c.SpaceId,
    send_ep: c.Handle,
    recv_ep: c.Handle,
    parent: c.Handle,
    stop: Atomic(bool) = .init(false),
    sent: Atomic(u32) = .init(0),
    ok: Atomic(u32) = .init(0),
    revoked: Atomic(u32) = .init(0),
    lost_race: Atomic(u32) = .init(0),
    bad: Atomic(u32) = .init(0),
};

fn capProducer(sh: *CapShared, n: u32) void {
    var buf: [64]u8 = undefined;
    var done: u32 = 0;
    while (done < n) {
        const d = [_]ipc4.Desc{c.copyDesc(sh.parent, .memory_object, c.R_READ | c.R_XFER)};
        if (sh.rig.x.send(sh.a, sh.send_ep, c.msg(&buf, 1, "c", 1), &d, .{ .timeout_ns = 10_000 * c.ms }, null)) |_| {
            done += 1;
            _ = sh.sent.fetchAdd(1, .monotonic);
        } else |e| {
            // Documented outcome: the copy was revoked between derivation and staging; nothing published.
            if (e == error.InvalidHandle) {
                _ = sh.lost_race.fetchAdd(1, .monotonic);
            } else {
                _ = sh.bad.fetchAdd(1, .monotonic);
                return;
            }
        }
    }
}

fn capConsumer(sh: *CapShared, n: u32) void {
    var i: u32 = 0;
    while (i < n) : (i += 1) {
        var out: ipc4.Received = undefined;
        sh.rig.x.recv(sh.b, sh.recv_ep, .{ .timeout_ns = 10_000 * c.ms }, &out) catch {
            _ = sh.bad.fetchAdd(1, .monotonic);
            return;
        };
        switch (out.slots[0].status) {
            .delivered => {
                _ = sh.ok.fetchAdd(1, .monotonic);
                sh.rig.k.release(sh.b, out.slots[0].handle) catch {}; // may already be revoked
            },
            .revoked => _ = sh.revoked.fetchAdd(1, .monotonic),
            else => _ = sh.bad.fetchAdd(1, .monotonic),
        }
        sh.rig.x.deinitReceived(&out);
    }
}

fn revoker(sh: *CapShared) void {
    while (!sh.stop.load(.acquire)) {
        _ = sh.rig.x.revoke(sh.a, sh.parent) catch {
            _ = sh.bad.fetchAdd(1, .monotonic);
            return;
        };
        std.Thread.yield() catch {};
    }
}

test "STRESS-02 capability copies sent while the parent is revoked concurrently: no loss, no leak" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 4 } });
    const p = try c.pair(&rig);
    const parent = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER | c.R_ADMIN), .{});
    var sh = CapShared{ .rig = &rig, .a = p.a, .b = p.b, .send_ep = p.a0, .recv_ep = p.b1, .parent = parent };
    const n: u32 = 400;
    const tp = try std.Thread.spawn(.{}, capProducer, .{ &sh, n });
    const tc = try std.Thread.spawn(.{}, capConsumer, .{ &sh, n });
    const tr = try std.Thread.spawn(.{}, revoker, .{&sh});
    tp.join();
    tc.join();
    sh.stop.store(true, .release);
    tr.join();
    try testing.expectEqual(@as(u32, 0), sh.bad.load(.acquire));
    try testing.expectEqual(n, sh.sent.load(.acquire));
    try testing.expectEqual(n, sh.ok.load(.acquire) + sh.revoked.load(.acquire));
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
    try rig.consistent();
    try testing.expectEqual(@as(usize, 0), rig.deinit());
}

const CloseShared = struct {
    rig: *c.Rig,
    p: c.Pair,
    bad: Atomic(u32) = .init(0),
};

fn closeSender(sh: *CloseShared) void {
    var buf: [64]u8 = undefined;
    while (true) {
        sh.rig.x.send(sh.p.a, sh.p.a0, c.msg(&buf, 1, "s", 0), &.{}, .{ .timeout_ns = 8000 * c.ms }, null) catch |e| {
            // PeerDead: end or peer closed. InvalidHandle: the caller's own space or handle vanished.
            if (e != error.PeerDead and e != error.InvalidHandle) _ = sh.bad.fetchAdd(1, .monotonic);
            return;
        };
    }
}

fn closeReceiver(sh: *CloseShared) void {
    while (true) {
        var out: ipc4.Received = undefined;
        sh.rig.x.recv(sh.p.b, sh.p.b1, .{ .timeout_ns = 8000 * c.ms }, &out) catch |e| {
            if (e != error.PeerDead and e != error.InvalidHandle) _ = sh.bad.fetchAdd(1, .monotonic);
            return;
        };
        sh.rig.x.deinitReceived(&out);
    }
}

test "STRESS-03 closing under load: every blocked or running thread terminates promptly, nothing leaks (60 rounds)" {
    var round: u32 = 0;
    while (round < 60) : (round += 1) {
        var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 2 } });
        const p = try c.pair(&rig);
        var sh = CloseShared{ .rig = &rig, .p = p };
        var t = try std.time.Timer.start();
        const t1 = try std.Thread.spawn(.{}, closeSender, .{&sh});
        const t2 = try std.Thread.spawn(.{}, closeSender, .{&sh});
        const t3 = try std.Thread.spawn(.{}, closeReceiver, .{&sh});
        std.Thread.sleep((round % 5) * c.ms);
        switch (round % 4) {
            0 => try rig.x.endpointClose(p.a, p.a0),
            1 => try rig.x.endpointClose(p.b, p.b1),
            2 => try rig.x.spaceClose(p.a),
            else => try rig.x.releaseHandle(p.b, p.b1),
        }
        t1.join();
        t2.join();
        t3.join();
        try testing.expect(t.read() < 6000 * c.ms); // woken, not left to the 8 s deadline
        try testing.expectEqual(@as(u32, 0), sh.bad.load(.acquire));
        try testing.expectEqual(@as(u32, 0), rig.x.ledger.waitersTotal());
        try rig.consistent();
        try testing.expectEqual(@as(usize, 0), rig.deinit());
    }
}

// ------------------------------------------------------------------ fuzzing

const FuzzState = struct {
    env: *c.Env,
    handles: [8]u64 = [_]u64{0} ** 8,
    n: usize = 0,

    fn refresh(fs: *FuzzState) !void {
        const e = fs.env;
        const m = try e.rig.k.createObject(e.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER | c.R_DUP), .{});
        const cx = try e.rig.x.cancelCreate(e.p.a);
        fs.handles = [_]u64{ @intFromEnum(e.p.a0), @intFromEnum(e.p.b1), @intFromEnum(m), @intFromEnum(cx), 0, 0, 0, 0 };
        fs.n = 4;
    }
};

fn validStatus(s: i64) bool {
    return s >= 0 or (s >= -17 and s <= -1) or s == c.sys.status_fault or s == c.sys.status_no_sys;
}

fn randPtr(rnd: std.Random) u64 {
    return switch (rnd.uintLessThan(u8, 10)) {
        0 => 0,
        1 => 0x9000_0000,
        2 => std.math.maxInt(u64) - rnd.uintLessThan(u64, 64),
        3 => rnd.int(u64),
        4 => c.User.A_MSG,
        5 => c.User.A_DESC,
        6 => c.User.A_RBUF,
        7 => c.User.A_RRES,
        8 => c.User.A_MISC,
        else => c.User.A_ARGS,
    };
}

fn randHandle(rnd: std.Random, fs: *const FuzzState) u64 {
    return switch (rnd.uintLessThan(u8, 8)) {
        0 => 0,
        1 => rnd.int(u64),
        2 => rnd.int(u32),
        3 => fs.handles[rnd.uintLessThan(usize, fs.n)] | (@as(u64, 1) << 32),
        else => fs.handles[rnd.uintLessThan(usize, fs.n)],
    };
}

/// Fuzzed waits stay short: forever becomes a poll, any longer valid timeout becomes 2 ms.
/// Invalid (over-policy) timeouts are left alone so they are rejected.
fn clampTimeout(block: []u8) void {
    const t = std.mem.readInt(u64, block[24..32], .little);
    if (t == c.acct.wait_forever) {
        std.mem.writeInt(u64, block[24..32], 0, .little);
    } else if (t > 2 * c.ms and t <= 30 * std.time.ns_per_s) {
        std.mem.writeInt(u64, block[24..32], 2 * c.ms, .little);
    }
}

fn fuzzOne(fs: *FuzzState, rnd: std.Random) !void {
    const e = fs.env;
    const u = &e.ua;
    const caller = if (rnd.boolean()) e.p.a else e.p.b;
    const nr: u32 = if (rnd.uintLessThan(u8, 20) < 17) 0x100 + rnd.uintLessThan(u32, 8) else rnd.int(u32);
    var a = [_]u64{0} ** 6;
    a[0] = if (rnd.uintLessThan(u8, 4) != 0) randHandle(rnd, fs) else randPtr(rnd);
    a[1] = if (rnd.uintLessThan(u8, 4) == 0) randPtr(rnd) else c.User.A_ARGS;
    if (rnd.uintLessThan(u8, 6) == 0) a[rnd.uintLessThan(usize, 6)] = rnd.int(u64);

    var buf: [256]u8 = undefined;
    var pay: [120]u8 = undefined;
    if (rnd.boolean()) {
        const plen = rnd.uintLessThan(usize, 120);
        @memset(pay[0..plen], 0xAB);
        const nd = rnd.uintLessThan(u32, 3);
        u.prepSend(c.msg(&buf, 1, pay[0..plen], nd), nd, 0, if (rnd.boolean()) 0 else @as(u32, @truncate(fs.handles[3])));
        for (0..nd) |i| u.putDesc(i, .{
            .handle = @enumFromInt(@as(u32, @truncate(randHandle(rnd, fs)))),
            .otype = if (rnd.boolean()) .memory_object else .channel,
            .mode = if (rnd.boolean()) .move else .copy,
            .rights = c.rights(rnd.int(u32) & c.R_ALL),
        });
    } else {
        u.prepRecv(if (rnd.uintLessThan(u8, 8) == 0) 2 * c.ms else 0, 0);
    }
    for (0..rnd.uintLessThan(usize, 4)) |_| u.args[rnd.uintLessThan(usize, 48)] = rnd.int(u8);
    if (rnd.uintLessThan(u8, 4) == 0) u.msgb[rnd.uintLessThan(usize, 64)] = rnd.int(u8);
    clampTimeout(&u.args);

    const r = u.callRaw(&e.rig, caller, nr, a);
    if (!validStatus(r)) {
        std.debug.print("seed-run produced invalid status {d} for nr={x}\n", .{ r, nr });
        return error.InvalidStatus;
    }
}

test "STRESS-04 seeded syscall fuzz: only documented statuses, invariants hold, no leaks (seeds 1..24)" {
    var seed: u64 = 1;
    while (seed <= 24) : (seed += 1) {
        errdefer std.debug.print("fuzz failure: seed={d}\n", .{seed});
        var prng = std.Random.DefaultPrng.init(seed);
        const rnd = prng.random();
        const env = try testing.allocator.create(c.Env);
        defer testing.allocator.destroy(env);
        try env.init(.{ .ipc = .{ .max_messages = 4 } });
        var fs = FuzzState{ .env = env };
        try fs.refresh();
        var i: u32 = 0;
        while (i < 400) : (i += 1) {
            try fuzzOne(&fs, rnd);
            if (i % 50 == 49) try env.rig.consistent();
            if (i % 100 == 99) {
                // fuzzed closes may have destroyed the fixtures; start over with fresh ones
                env.rig.x.releaseHandle(env.p.a, env.p.a0) catch {};
                fs.refresh() catch {};
            }
        }
        try env.rig.consistent();
        try env.deinit();
    }
}
