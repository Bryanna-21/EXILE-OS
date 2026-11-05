//! Blocking, wake-up, cancellation, timeout and closure-while-blocked tests.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const ms = c.ms;

fn sleepMs(n: u64) void {
    std.Thread.sleep(n * ms);
}

/// Poll until `n` callers are registered as blocked (bounded: fails instead of hanging).
fn waitBlocked(rig: *c.Rig, n: u32) !void {
    var i: u32 = 0;
    while (rig.x.ledger.waitersTotal() < n) : (i += 1) {
        if (i > 3000) return error.NobodyBlocked;
        std.Thread.sleep(ms);
    }
}

const RecvJob = struct {
    rig: *c.Rig,
    space: c.SpaceId,
    ep: c.Handle,
    w: c.kipc.Wait,
    result: ?c.OmniError = null,
    got: bool = false,
    byte: u8 = 0,
    elapsed: u64 = 0,

    fn run(j: *RecvJob) void {
        var t = std.time.Timer.start() catch unreachable;
        var out: c.ipc4.Received = undefined;
        if (j.rig.x.recv(j.space, j.ep, j.w, &out)) |_| {
            j.got = true;
            j.byte = if (out.payload.len > 0) out.payload[0] else 0;
            j.rig.x.deinitReceived(&out);
        } else |e| {
            j.result = e;
        }
        j.elapsed = t.read();
    }
};

const SendJob = struct {
    rig: *c.Rig,
    space: c.SpaceId,
    ep: c.Handle,
    w: c.kipc.Wait,
    payload: []const u8,
    result: ?c.OmniError = null,
    done: bool = false,
    elapsed: u64 = 0,

    fn run(j: *SendJob) void {
        var t = std.time.Timer.start() catch unreachable;
        var buf: [256]u8 = undefined;
        j.rig.x.send(j.space, j.ep, c.msg(&buf, 1, j.payload, 0), &.{}, j.w, null) catch |e| {
            j.result = e;
        };
        j.done = true;
        j.elapsed = t.read();
    }
};

test "BLK-01 a blocked receiver is woken by a send" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var job = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 5000 * ms } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    try waitBlocked(&rig, 1);
    try c.sendPoll(&rig, p.a, p.a0, "Z");
    th.join();
    try testing.expect(job.got);
    try testing.expectEqual(@as(u8, 'Z'), job.byte);
    try testing.expect(job.elapsed < 3000 * ms);
    try testing.expectEqual(@as(u32, 0), rig.x.ledger.waitersTotal());
    try rig.consistent();
}

test "BLK-02 a blocked sender is woken by a receive that makes room" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 1 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    try c.sendPoll(&rig, p.a, p.a0, "1");
    var job = SendJob{ .rig = &rig, .space = p.a, .ep = p.a0, .w = .{ .timeout_ns = 5000 * ms }, .payload = "2" };
    const th = try std.Thread.spawn(.{}, SendJob.run, .{&job});
    try waitBlocked(&rig, 1);
    try testing.expect(!job.done);
    var out: c.ipc4.Received = undefined;
    try rig.x.recv(p.b, p.b1, .{}, &out);
    try testing.expectEqualStrings("1", out.payload);
    rig.x.deinitReceived(&out);
    th.join();
    try testing.expectEqual(@as(?c.OmniError, null), job.result);
    try rig.x.recv(p.b, p.b1, .{}, &out);
    try testing.expectEqualStrings("2", out.payload); // FIFO and nothing lost
    rig.x.deinitReceived(&out);
    try rig.consistent();
}

test "BLK-03 recv and send time out no earlier than their deadline, leaving no state behind" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 1 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var out: c.ipc4.Received = undefined;
    var t = try std.time.Timer.start();
    try testing.expectError(error.TimedOut, rig.x.recv(p.b, p.b1, .{ .timeout_ns = 40 * ms }, &out));
    try testing.expect(t.read() >= 40 * ms);
    try c.sendPoll(&rig, p.a, p.a0, "1");
    var buf: [64]u8 = undefined;
    t.reset();
    try testing.expectError(error.TimedOut, rig.x.send(p.a, p.a0, c.msg(&buf, 1, "2", 0), &.{}, .{ .timeout_ns = 40 * ms }, null));
    try testing.expect(t.read() >= 40 * ms);
    const st = try rig.x.endpointStats(p.b, p.b1);
    try testing.expectEqual(@as(u32, 1), st.queued_messages);
    try testing.expectEqual(@as(u32, 0), st.reserved_messages);
    try testing.expectEqual(@as(u32, 0), rig.x.ledger.waitersTotal());
    try rig.consistent();
}

test "BLK-04 poll mode never waits: WouldBlock immediately" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 1 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var out: c.ipc4.Received = undefined;
    var t = try std.time.Timer.start();
    try testing.expectError(error.WouldBlock, rig.x.recv(p.b, p.b1, .{}, &out));
    try c.sendPoll(&rig, p.a, p.a0, "1");
    try testing.expectError(error.WouldBlock, c.sendPoll(&rig, p.a, p.a0, "2"));
    try testing.expect(t.read() < 1000 * ms);
    try testing.expectEqual(@as(u64, 0), rig.x.ledger.snapshot().waits_started);
}

test "BLK-05 cancellation wakes a blocked receiver and a blocked sender" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 1 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    const cx = try rig.x.cancelCreate(p.b);
    var rj = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 5000 * ms, .cancel = cx } };
    const t1 = try std.Thread.spawn(.{}, RecvJob.run, .{&rj});
    try waitBlocked(&rig, 1);
    try rig.x.cancelSignal(p.b, cx);
    t1.join();
    try testing.expectEqual(@as(?c.OmniError, error.Cancelled), rj.result);
    try testing.expect(rj.elapsed < 3000 * ms);

    // sender side
    try c.sendPoll(&rig, p.a, p.a0, "1");
    const cy = try rig.x.cancelCreate(p.a);
    var sj = SendJob{ .rig = &rig, .space = p.a, .ep = p.a0, .w = .{ .timeout_ns = 5000 * ms, .cancel = cy }, .payload = "2" };
    const t2 = try std.Thread.spawn(.{}, SendJob.run, .{&sj});
    try waitBlocked(&rig, 1);
    try rig.x.cancelSignal(p.a, cy);
    t2.join();
    try testing.expectEqual(@as(?c.OmniError, error.Cancelled), sj.result);
    const st = try rig.x.endpointStats(p.b, p.b1);
    try testing.expectEqual(@as(u32, 1), st.queued_messages); // nothing published
    try testing.expectEqual(@as(u32, 0), rig.x.ledger.waitersTotal());
    try rig.consistent();
}

test "BLK-06 wait_forever is allowed only with a cancellation handle and ends on cancel" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var out: c.ipc4.Received = undefined;
    try testing.expectError(error.InvalidArgument, rig.x.recv(p.b, p.b1, .{ .timeout_ns = c.kipc.wait_forever }, &out));
    try testing.expectError(error.InvalidArgument, rig.x.recv(p.b, p.b1, .{ .timeout_ns = 31 * std.time.ns_per_s }, &out));
    const cx = try rig.x.cancelCreate(p.b);
    var job = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = c.kipc.wait_forever, .cancel = cx } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    try waitBlocked(&rig, 1);
    try rig.x.cancelSignal(p.b, cx);
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.Cancelled), job.result);
}

test "BLK-07 a cancel signalled before the call wins without consuming anything" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    const cx = try rig.x.cancelCreate(p.b);
    try c.sendPoll(&rig, p.a, p.a0, "queued");
    try rig.x.cancelSignal(p.b, cx);
    try rig.x.cancelSignal(p.b, cx); // idempotent
    var out: c.ipc4.Received = undefined;
    try testing.expectError(error.Cancelled, rig.x.recv(p.b, p.b1, .{ .timeout_ns = 100 * ms, .cancel = cx }, &out));
    try rig.x.recv(p.b, p.b1, .{}, &out);
    try testing.expectEqualStrings("queued", out.payload);
    rig.x.deinitReceived(&out);
}

test "BLK-08 closing the peer end wakes blocked receivers and senders with PeerDead" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 1 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var rj = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 5000 * ms } };
    const t1 = try std.Thread.spawn(.{}, RecvJob.run, .{&rj});
    try waitBlocked(&rig, 1);
    try rig.x.endpointClose(p.a, p.a0);
    t1.join();
    try testing.expectEqual(@as(?c.OmniError, error.PeerDead), rj.result);
    try testing.expect(rj.elapsed < 3000 * ms);

    var rig2 = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 1 } });
    defer testing.expectEqual(@as(usize, 0), rig2.deinit()) catch @panic("leak");
    const q = try c.pair(&rig2);
    try c.sendPoll(&rig2, q.a, q.a0, "1");
    var sj = SendJob{ .rig = &rig2, .space = q.a, .ep = q.a0, .w = .{ .timeout_ns = 5000 * ms }, .payload = "2" };
    const t2 = try std.Thread.spawn(.{}, SendJob.run, .{&sj});
    try waitBlocked(&rig2, 1);
    try rig2.x.endpointClose(q.b, q.b1);
    t2.join();
    try testing.expectEqual(@as(?c.OmniError, error.PeerDead), sj.result);
    try testing.expectEqual(@as(u64, 0), try rig2.k.spaceUsed(q.a, .queued_bytes)); // dropped and refunded
    try rig2.consistent();
}

test "BLK-09 destroying the peer's space or releasing its last handle wakes waiters promptly" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var rj = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 5000 * ms } };
    const t1 = try std.Thread.spawn(.{}, RecvJob.run, .{&rj});
    try waitBlocked(&rig, 1);
    try rig.x.spaceClose(p.a);
    t1.join();
    try testing.expectEqual(@as(?c.OmniError, error.PeerDead), rj.result);
    try testing.expect(rj.elapsed < 3000 * ms);

    const q = try c.pair(&rig);
    var rj2 = RecvJob{ .rig = &rig, .space = q.b, .ep = q.b1, .w = .{ .timeout_ns = 5000 * ms } };
    const t2 = try std.Thread.spawn(.{}, RecvJob.run, .{&rj2});
    try waitBlocked(&rig, 1);
    try rig.x.releaseHandle(q.a, q.a0);
    t2.join();
    try testing.expectEqual(@as(?c.OmniError, error.PeerDead), rj2.result);
    try testing.expect(rj2.elapsed < 3000 * ms);
    try rig.consistent();
}

test "BLK-10 revoking a waiter's capability wakes it and it fails with InvalidHandle" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const a = try rig.space();
    const b = try rig.space();
    const e = try rig.x.endpointCreate(a);
    const parent = try rig.k.transfer(a, e[1], b);
    const child = try rig.k.derive(b, parent, c.rights(c.R_READ));
    var job = RecvJob{ .rig = &rig, .space = b, .ep = child, .w = .{ .timeout_ns = 5000 * ms } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    try waitBlocked(&rig, 1);
    _ = try rig.x.revoke(b, parent);
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.InvalidHandle), job.result);
    try testing.expect(job.elapsed < 3000 * ms);
    try rig.consistent();
}

test "BLK-11 documented limit: a state change made behind the interface is seen at the deadline" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var job = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 150 * ms } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    try waitBlocked(&rig, 1);
    try rig.k.closeSpace(p.a); // direct kernel call: no notification
    th.join();
    // never an unbounded wait; the final attempt after the deadline sees the dead peer
    try testing.expectEqual(@as(?c.OmniError, error.PeerDead), job.result);
    try testing.expect(job.elapsed >= 150 * ms);
    try testing.expectEqual(@as(u32, 0), rig.x.ledger.waitersTotal());
}

test "BLK-12 unrelated traffic does not end a wait early or wrongly" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    const q = try c.pair(&rig);
    var job = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 200 * ms } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    try waitBlocked(&rig, 1);
    var i: u32 = 0;
    while (i < 60) : (i += 1) {
        try c.sendPoll(&rig, q.a, q.a0, "n");
        var out: c.ipc4.Received = undefined;
        try rig.x.recv(q.b, q.b1, .{}, &out);
        rig.x.deinitReceived(&out);
    }
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.TimedOut), job.result);
    try testing.expect(job.elapsed >= 200 * ms);
    try rig.consistent();
}

test "BLK-13 blocked callers are bounded per space and in total, and slots are returned" {
    var rig = try c.Rig.init(testing.allocator, .{ .policy = .{ .max_waiters_per_space = 2, .max_waiters_total = 3 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var jobs: [2]RecvJob = undefined;
    var ths: [2]std.Thread = undefined;
    for (&jobs, &ths) |*j, *t| {
        j.* = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 600 * ms } };
        t.* = try std.Thread.spawn(.{}, RecvJob.run, .{j});
    }
    try waitBlocked(&rig, 2);
    var out: c.ipc4.Received = undefined;
    // third blocking call from the same space is refused at once
    var t0 = try std.time.Timer.start();
    try testing.expectError(error.QuotaExceeded, rig.x.recv(p.b, p.b1, .{ .timeout_ns = 600 * ms }, &out));
    try testing.expect(t0.read() < 300 * ms);
    // another space may still block (total limit is 3)
    const q = try c.pair(&rig);
    var other = RecvJob{ .rig = &rig, .space = q.b, .ep = q.b1, .w = .{ .timeout_ns = 600 * ms } };
    const to = try std.Thread.spawn(.{}, RecvJob.run, .{&other});
    try waitBlocked(&rig, 3);
    const r = try rig.space();
    const e = try rig.x.endpointCreate(r);
    try testing.expectError(error.QuotaExceeded, rig.x.recv(r, e[0], .{ .timeout_ns = 600 * ms }, &out)); // total
    for (ths) |t| t.join();
    to.join();
    try testing.expectEqual(@as(u32, 0), rig.x.ledger.waitersTotal());
    try testing.expect(rig.x.ledger.audit() == null);
    // the limit does not affect polling
    try testing.expectError(error.WouldBlock, rig.x.recv(p.b, p.b1, .{}, &out));
}

const Slot = struct {
    rig: *c.Rig,
    space: c.SpaceId,
    ep: c.Handle,
    got: std.atomic.Value(u32) = .init(0),
    seen: [16]std.atomic.Value(u8) = [_]std.atomic.Value(u8){.init(0)} ** 16,
    bad: std.atomic.Value(u32) = .init(0),
};

fn manyReceiver(s: *Slot) void {
    var out: c.ipc4.Received = undefined;
    s.rig.x.recv(s.space, s.ep, .{ .timeout_ns = 8000 * ms }, &out) catch {
        _ = s.bad.fetchAdd(1, .monotonic);
        return;
    };
    const idx = out.payload[0];
    s.rig.x.deinitReceived(&out);
    if (idx >= s.seen.len or s.seen[idx].fetchAdd(1, .monotonic) != 0) _ = s.bad.fetchAdd(1, .monotonic);
    _ = s.got.fetchAdd(1, .monotonic);
}

test "BLK-14 eight blocked receivers, eight sends: every message wakes exactly one receiver" {
    var rig = try c.Rig.init(testing.allocator, .{ .policy = .{ .max_waiters_per_space = 8, .max_waiters_total = 8 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var s = Slot{ .rig = &rig, .space = p.b, .ep = p.b1 };
    var ths: [8]std.Thread = undefined;
    for (&ths) |*t| t.* = try std.Thread.spawn(.{}, manyReceiver, .{&s});
    try waitBlocked(&rig, 8);
    var i: u8 = 0;
    while (i < 8) : (i += 1) {
        var buf: [64]u8 = undefined;
        const payload = [_]u8{i};
        try rig.x.send(p.a, p.a0, c.msg(&buf, 1, &payload, 0), &.{}, .{}, null);
    }
    for (ths) |t| t.join();
    try testing.expectEqual(@as(u32, 0), s.bad.load(.acquire));
    try testing.expectEqual(@as(u32, 8), s.got.load(.acquire));
    for (s.seen[0..8]) |*x| try testing.expectEqual(@as(u8, 1), x.load(.acquire));
    try testing.expectEqual(@as(u32, 0), rig.x.ledger.waitersTotal());
    try rig.consistent();
}

test "BLK-15 a waiting thread uses almost no CPU (no busy-waiting)" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    const before = std.posix.getrusage(0);
    var job = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 400 * ms } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    th.join();
    const after = std.posix.getrusage(0);
    try testing.expectEqual(@as(?c.OmniError, error.TimedOut), job.result);
    const used = tvUs(after.utime) + tvUs(after.stime) - tvUs(before.utime) - tvUs(before.stime);
    try testing.expect(used < 150_000); // a spinning waiter would use about 400000 us
}

fn tvUs(tv: anytype) i64 {
    return @as(i64, @intCast(tv.sec)) * 1_000_000 + @as(i64, @intCast(tv.usec));
}

test "BLK-16 timeout vs arrival, 100 rounds: either outcome, never a lost or duplicated message" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var delivered: u32 = 0;
    var timed_out: u32 = 0;
    var round: u32 = 0;
    while (round < 100) : (round += 1) {
        var job = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 3 * ms } };
        const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
        std.Thread.sleep((round % 7) * 500 * std.time.ns_per_us);
        try c.sendPoll(&rig, p.a, p.a0, "x");
        th.join();
        const st = try rig.x.endpointStats(p.b, p.b1);
        if (job.got) {
            delivered += 1;
            try testing.expectEqual(@as(u32, 0), st.queued_messages);
        } else {
            timed_out += 1;
            try testing.expectEqual(@as(?c.OmniError, error.TimedOut), job.result);
            try testing.expectEqual(@as(u32, 1), st.queued_messages); // not lost
            var out: c.ipc4.Received = undefined;
            try rig.x.recv(p.b, p.b1, .{}, &out);
            rig.x.deinitReceived(&out);
        }
    }
    try testing.expectEqual(@as(u32, 100), delivered + timed_out);
    try rig.consistent();
}

test "BLK-17 cancel vs arrival, 200 rounds: exactly one outcome, message conserved" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var round: u32 = 0;
    var delivered: u32 = 0;
    var cancelled: u32 = 0;
    while (round < 200) : (round += 1) {
        const cx = try rig.x.cancelCreate(p.b);
        var job = RecvJob{ .rig = &rig, .space = p.b, .ep = p.b1, .w = .{ .timeout_ns = 4000 * ms, .cancel = cx } };
        const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
        if (round % 3 == 0) std.Thread.yield() catch {};
        if (round % 2 == 0) {
            try c.sendPoll(&rig, p.a, p.a0, "x");
            try rig.x.cancelSignal(p.b, cx);
        } else {
            try rig.x.cancelSignal(p.b, cx);
            try c.sendPoll(&rig, p.a, p.a0, "x");
        }
        th.join();
        const st = try rig.x.endpointStats(p.b, p.b1);
        if (job.got) {
            delivered += 1;
            try testing.expectEqual(@as(u32, 0), st.queued_messages);
        } else {
            cancelled += 1;
            try testing.expectEqual(@as(?c.OmniError, error.Cancelled), job.result);
            try testing.expectEqual(@as(u32, 1), st.queued_messages);
            var out: c.ipc4.Received = undefined;
            try rig.x.recv(p.b, p.b1, .{}, &out);
            rig.x.deinitReceived(&out);
        }
        try rig.x.releaseHandle(p.b, cx);
    }
    try testing.expectEqual(@as(u32, 200), delivered + cancelled);
    try rig.consistent();
}
