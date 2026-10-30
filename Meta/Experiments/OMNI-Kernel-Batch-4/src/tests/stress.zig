//! Randomised model test (recorded seeds) and multi-threaded stress (areas 13-14).
//!
//! Thread-safety is claimed ONLY for what these tests exercise: concurrent
//! send/recv/close/cancel/revoke through one `Ipc` over one `Kernel`.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const ipc = c.ipc;

const Atomic = std.atomic.Value;

const Model = struct {
    const Item = struct { len: u32, tag: u8, caps: u8 };
    q: std.ArrayListUnmanaged(Item) = .empty,
    bytes: u64 = 0,
};

/// Single-threaded: the IPC layer must behave exactly like a bounded FIFO model.
/// `seed` is printed when a seed fails so the run can be replayed.
fn modelRun(seed: u64) !void {
    errdefer std.debug.print("MODEL FAILURE seed={d}\n", .{seed});
    var prng = std.Random.DefaultPrng.init(seed);
    const rnd = prng.random();
    const cfg = ipc.Config{ .max_messages = 4, .max_queue_bytes = 4096 };
    var rig = try c.Rig.init(testing.allocator, cfg);
    var model = Model{};
    defer model.q.deinit(testing.allocator);
    const a = try rig.space();
    const b = try rig.space();
    const e = try rig.ipc.createChannel(a);
    const b1 = try rig.k.transfer(a, e[1], b);
    var buf: [4096]u8 = undefined;
    var step: usize = 0;
    while (step < 120) : (step += 1) {
        switch (rnd.uintLessThan(u8, 10)) {
            0...4 => { // send, sometimes with a capability
                const len = rnd.uintLessThan(u32, 1500);
                var payload: [1500]u8 = undefined;
                const tag = rnd.int(u8);
                @memset(payload[0..len], tag);
                const with_cap = rnd.boolean();
                const total: u32 = 32 + len;
                const fits = model.q.items.len < cfg.max_messages and model.bytes + total <= cfg.max_queue_bytes;
                var obj: c.Handle = .invalid;
                var descs: []const ipc.Desc = &.{};
                var one: [1]ipc.Desc = undefined;
                if (with_cap) {
                    obj = try rig.k.createObject(a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
                    one[0] = c.moveDesc(obj, .memory_object, c.R_READ | c.R_XFER);
                    descs = &one;
                }
                const bytes = c.msg(&buf, 1, payload[0..len], if (with_cap) 1 else 0);
                if (rig.ipc.send(a, e[0], bytes, descs, .{}, null)) |_| {
                    try testing.expect(fits);
                    try model.q.append(testing.allocator, .{ .len = len, .tag = tag, .caps = @intFromBool(with_cap) });
                    model.bytes += total;
                } else |er| {
                    try testing.expectEqual(error.WouldBlock, er);
                    try testing.expect(!fits);
                    if (with_cap) { // the capability must still be ours
                        _ = try rig.k.lookup(a, obj, .memory_object, c.Rights.none);
                        try rig.k.release(a, obj);
                    }
                }
            },
            5...8 => { // receive
                var out: ipc.Received = undefined;
                if (rig.ipc.recv(b, b1, .{}, &out)) |_| {
                    try testing.expect(model.q.items.len > 0);
                    const it = model.q.orderedRemove(0);
                    model.bytes -= 32 + it.len;
                    try testing.expectEqual(it.len, @as(u32, @intCast(out.payload.len)));
                    if (it.len > 0) try testing.expectEqual(it.tag, out.payload[0]);
                    try testing.expectEqual(it.caps, out.count);
                    for (out.slots[0..out.count]) |s| {
                        try testing.expectEqual(ipc.SlotStatus.delivered, s.status);
                        try rig.k.release(b, s.handle);
                    }
                    out.deinit(rig.ipc);
                } else |er| {
                    try testing.expectEqual(error.WouldBlock, er);
                    try testing.expectEqual(@as(usize, 0), model.q.items.len);
                }
            },
            else => { // inspect
                const st = try rig.ipc.endStats(b, b1);
                try testing.expectEqual(@as(u32, @intCast(model.q.items.len)), st.queued_messages);
                try testing.expectEqual(model.bytes, st.queued_bytes);
            },
        }
        try rig.consistent();
    }
    // The sender is charged exactly for what is still queued (accounting object per message).
    try testing.expectEqual(model.bytes - 32 * model.q.items.len + 32 * model.q.items.len, try rig.k.spaceUsed(a, .queued_bytes));
    try testing.expectEqual(@as(usize, 0), rig.deinit());
}

test "MODEL-01 randomised ops match a bounded FIFO model (seeds 1..48, recorded on failure)" {
    var seed: u64 = 1;
    while (seed <= 48) : (seed += 1) try modelRun(seed);
}

test "MODEL-02 a fixed regression seed list stays green" {
    for ([_]u64{ 0, 7, 42, 1337, 0xDEADBEEF, 0xFFFF_FFFF_FFFF_FFFF }) |s| try modelRun(s);
}

// ------------------------------------------------------------------ threads

const Shared = struct {
    rig: *c.Rig,
    a: c.SpaceId,
    b: c.SpaceId,
    send_end: c.Handle,
    recv_end: c.Handle,
    produced: Atomic(u32) = .init(0),
    consumed: Atomic(u32) = .init(0),
    fail: Atomic(u32) = .init(0),
    seen: []Atomic(u8),
    per_producer: u32,
};

fn producer(sh: *Shared, pid: u8) void {
    var buf: [128]u8 = undefined;
    var i: u32 = 0;
    while (i < sh.per_producer) : (i += 1) {
        var payload: [5]u8 = undefined;
        payload[0] = pid;
        std.mem.writeInt(u32, payload[1..5], i, .little);
        const bytes = c.msg(&buf, 1, &payload, 0);
        sh.rig.ipc.send(sh.a, sh.send_end, bytes, &.{}, .{ .timeout_ns = null }, null) catch {
            _ = sh.fail.fetchAdd(1, .monotonic);
            return;
        };
        _ = sh.produced.fetchAdd(1, .monotonic);
    }
}

fn consumer(sh: *Shared, total: u32) void {
    var last = [_]i64{-1} ** 8;
    while (sh.consumed.load(.acquire) < total) {
        var out: ipc.Received = undefined;
        sh.rig.ipc.recv(sh.b, sh.recv_end, .{ .timeout_ns = 20 * std.time.ns_per_ms }, &out) catch |e| {
            if (e == error.TimedOut) continue;
            _ = sh.fail.fetchAdd(1, .monotonic);
            return;
        };
        const pid = out.payload[0];
        const seq = std.mem.readInt(u32, out.payload[1..5], .little);
        out.deinit(sh.rig.ipc);
        // per-consumer order must follow queue order per producer
        if (@as(i64, seq) <= last[pid]) _ = sh.fail.fetchAdd(1, .monotonic);
        last[pid] = seq;
        const idx = @as(usize, pid) * sh.per_producer + seq;
        if (sh.seen[idx].fetchAdd(1, .monotonic) != 0) _ = sh.fail.fetchAdd(1, .monotonic); // duplicate
        _ = sh.consumed.fetchAdd(1, .release);
    }
}

test "STRESS-01 4 producers, 3 consumers, small queue: every message exactly once, per-producer order kept" {
    var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 4 });
    const a = try rig.space();
    const b = try rig.space();
    const e = try rig.ipc.createChannel(a);
    const b1 = try rig.k.transfer(a, e[1], b);
    const per: u32 = 400;
    const total: u32 = per * 4;
    const seen = try testing.allocator.alloc(Atomic(u8), total);
    defer testing.allocator.free(seen);
    for (seen) |*s| s.* = .init(0);
    var sh = Shared{ .rig = &rig, .a = a, .b = b, .send_end = e[0], .recv_end = b1, .seen = seen, .per_producer = per };
    var prods: [4]std.Thread = undefined;
    var cons: [3]std.Thread = undefined;
    for (&cons) |*t| t.* = try std.Thread.spawn(.{}, consumer, .{ &sh, total });
    for (&prods, 0..) |*t, i| t.* = try std.Thread.spawn(.{}, producer, .{ &sh, @as(u8, @intCast(i)) });
    for (prods) |t| t.join();
    for (cons) |t| t.join();
    try testing.expectEqual(@as(u32, 0), sh.fail.load(.acquire));
    try testing.expectEqual(total, sh.produced.load(.acquire));
    try testing.expectEqual(total, sh.consumed.load(.acquire));
    for (seen) |*s| try testing.expectEqual(@as(u8, 1), s.load(.acquire));
    try rig.consistent();
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(a, .queued_bytes));
    try testing.expectEqual(@as(usize, 0), rig.deinit());
}

const CapShared = struct {
    rig: *c.Rig,
    a: c.SpaceId,
    b: c.SpaceId,
    send_end: c.Handle,
    recv_end: c.Handle,
    parent: c.Handle,
    stop: Atomic(bool) = .init(false),
    sent: Atomic(u32) = .init(0),
    got_ok: Atomic(u32) = .init(0),
    got_revoked: Atomic(u32) = .init(0),
    lost_race: Atomic(u32) = .init(0),
    bad: Atomic(u32) = .init(0),
};

fn capProducer(sh: *CapShared, n: u32) void {
    var buf: [64]u8 = undefined;
    var done: u32 = 0;
    while (done < n) {
        const bytes = c.msg(&buf, 1, "c", 1);
        const d = [_]ipc.Desc{c.copyDesc(sh.parent, .memory_object, c.R_READ | c.R_XFER)};
        if (sh.rig.ipc.send(sh.a, sh.send_end, bytes, &d, .{ .timeout_ns = null }, null)) |_| {
            done += 1;
            _ = sh.sent.fetchAdd(1, .monotonic);
        } else |e| {
            // Legal outcome of the revocation race: the freshly derived copy was revoked
            // between derivation and staging, so the send FAILED with nothing published.
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
        var out: ipc.Received = undefined;
        sh.rig.ipc.recv(sh.b, sh.recv_end, .{ .timeout_ns = null }, &out) catch {
            _ = sh.bad.fetchAdd(1, .monotonic);
            return;
        };
        switch (out.slots[0].status) {
            .delivered => {
                _ = sh.got_ok.fetchAdd(1, .monotonic);
                // may already be revoked by now: both outcomes are legal
                sh.rig.k.release(sh.b, out.slots[0].handle) catch {};
            },
            .revoked => _ = sh.got_revoked.fetchAdd(1, .monotonic),
            else => _ = sh.bad.fetchAdd(1, .monotonic),
        }
        out.deinit(sh.rig.ipc);
    }
}

fn revoker(sh: *CapShared) void {
    while (!sh.stop.load(.acquire)) {
        _ = sh.rig.k.revoke(sh.a, sh.parent) catch {
            _ = sh.bad.fetchAdd(1, .monotonic);
            return;
        };
        std.Thread.yield() catch {};
    }
}

test "STRESS-02 capability copies sent while the parent is revoked concurrently: no loss, no leak" {
    var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 4 });
    const a = try rig.space();
    const b = try rig.space();
    const e = try rig.ipc.createChannel(a);
    const b1 = try rig.k.transfer(a, e[1], b);
    const parent = try rig.k.createObject(a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER | c.R_ADMIN), .{});
    var sh = CapShared{ .rig = &rig, .a = a, .b = b, .send_end = e[0], .recv_end = b1, .parent = parent };
    const n: u32 = 600;
    const tp = try std.Thread.spawn(.{}, capProducer, .{ &sh, n });
    const tc = try std.Thread.spawn(.{}, capConsumer, .{ &sh, n });
    const tr = try std.Thread.spawn(.{}, revoker, .{&sh});
    tp.join();
    tc.join();
    sh.stop.store(true, .release);
    tr.join();
    try testing.expectEqual(@as(u32, 0), sh.bad.load(.acquire));
    try testing.expectEqual(n, sh.sent.load(.acquire));
    try testing.expectEqual(n, sh.got_ok.load(.acquire) + sh.got_revoked.load(.acquire));
    try rig.consistent();
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(a, .queued_bytes));
    try testing.expectEqual(@as(usize, 0), rig.deinit());
}

const CloseShared = struct {
    rig: *c.Rig,
    a: c.SpaceId,
    b: c.SpaceId,
    a0: c.Handle,
    b1: c.Handle,
    bad: Atomic(u32) = .init(0),
};

fn closeSender(sh: *CloseShared) void {
    var buf: [64]u8 = undefined;
    while (true) {
        sh.rig.ipc.send(sh.a, sh.a0, c.msg(&buf, 1, "s", 0), &.{}, .{ .timeout_ns = null }, null) catch |e| {
            // PeerDead: the end/peer closed. InvalidHandle: the caller's own space was closed
            // while it waited (its capability vanished); both are the documented outcomes.
            if (e != error.PeerDead and e != error.InvalidHandle) _ = sh.bad.fetchAdd(1, .monotonic);
            return;
        };
    }
}

fn closeReceiver(sh: *CloseShared) void {
    while (true) {
        var out: ipc.Received = undefined;
        sh.rig.ipc.recv(sh.b, sh.b1, .{ .timeout_ns = null }, &out) catch |e| {
            // PeerDead: the end/peer closed. InvalidHandle: the caller's own space was closed
            // while it waited (its capability vanished); both are the documented outcomes.
            if (e != error.PeerDead and e != error.InvalidHandle) _ = sh.bad.fetchAdd(1, .monotonic);
            return;
        };
        out.deinit(sh.rig.ipc);
    }
}

test "STRESS-03 closing under load: every blocked or running thread terminates, nothing leaks (60 rounds)" {
    var round: u32 = 0;
    while (round < 60) : (round += 1) {
        var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 2 });
        const a = try rig.space();
        const b = try rig.space();
        const e = try rig.ipc.createChannel(a);
        const b1 = try rig.k.transfer(a, e[1], b);
        var sh = CloseShared{ .rig = &rig, .a = a, .b = b, .a0 = e[0], .b1 = b1 };
        const t1 = try std.Thread.spawn(.{}, closeSender, .{&sh});
        const t2 = try std.Thread.spawn(.{}, closeSender, .{&sh});
        const t3 = try std.Thread.spawn(.{}, closeReceiver, .{&sh});
        std.Thread.sleep((round % 5) * std.time.ns_per_ms);
        switch (round % 3) {
            0 => try rig.ipc.closeEnd(a, e[0]),
            1 => try rig.ipc.closeEnd(b, b1),
            else => try rig.k.closeSpace(a),
        }
        t1.join();
        t2.join();
        t3.join();
        try testing.expectEqual(@as(u32, 0), sh.bad.load(.acquire));
        try rig.consistent();
        try testing.expectEqual(@as(usize, 0), rig.deinit());
    }
}
