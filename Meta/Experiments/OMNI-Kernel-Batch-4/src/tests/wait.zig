//! Waiting, cancellation, timeout, closure and exact-once cleanup (areas 12-16).
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const ipc = c.ipc;

const ms = std.time.ns_per_ms;

fn sleepMs(n: u64) void {
    std.Thread.sleep(n * ms);
}

const Pair = struct { a: c.SpaceId, b: c.SpaceId, a0: c.Handle, b1: c.Handle };

fn pair(rig: *c.Rig) !Pair {
    const a = try rig.space();
    const b = try rig.space();
    const e = try rig.ipc.createChannel(a);
    const b1 = try rig.k.transfer(a, e[1], b);
    return .{ .a = a, .b = b, .a0 = e[0], .b1 = b1 };
}

fn send1(rig: *c.Rig, sp: c.SpaceId, h: c.Handle, payload: []const u8) !void {
    var buf: [256]u8 = undefined;
    try rig.ipc.send(sp, h, c.msg(&buf, 1, payload, 0), &.{}, .{}, null);
}

test "WAIT-01 recv timeout: TimedOut after about the deadline, nothing consumed" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    var t = try std.time.Timer.start();
    var out: ipc.Received = undefined;
    try testing.expectError(error.TimedOut, rig.ipc.recv(p.b, p.b1, .{ .timeout_ns = 40 * ms }, &out));
    const el = t.read();
    try testing.expect(el >= 40 * ms);
    try testing.expect(el < 2000 * ms);
    try testing.expectEqual(@as(u64, 1), rig.ipc.stats().timed_out);
    try rig.consistent();
}

test "WAIT-02 send timeout on a full queue: TimedOut, queue unchanged" {
    var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 1 });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    try send1(&rig, p.a, p.a0, "first");
    var buf: [64]u8 = undefined;
    try testing.expectError(error.TimedOut, rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "second", 0), &.{}, .{ .timeout_ns = 30 * ms }, null));
    const st = try rig.ipc.endStats(p.b, p.b1);
    try testing.expectEqual(@as(u32, 1), st.queued_messages);
    try testing.expectEqual(@as(u32, 0), st.reserved_messages);
    try rig.consistent();
}

const RecvJob = struct {
    rig: *c.Rig,
    space: c.SpaceId,
    end: c.Handle,
    opts: ipc.WaitOptions,
    result: ?c.OmniError = null,
    got: bool = false,
    byte: u8 = 0,

    fn run(j: *RecvJob) void {
        var out: ipc.Received = undefined;
        if (j.rig.ipc.recv(j.space, j.end, j.opts, &out)) |_| {
            j.got = true;
            j.byte = if (out.payload.len > 0) out.payload[0] else 0;
            out.deinit(j.rig.ipc);
        } else |e| {
            j.result = e;
        }
    }
};

const SendJob = struct {
    rig: *c.Rig,
    space: c.SpaceId,
    end: c.Handle,
    opts: ipc.WaitOptions,
    payload: []const u8,
    result: ?c.OmniError = null,
    done: bool = false,

    fn run(j: *SendJob) void {
        var buf: [256]u8 = undefined;
        j.rig.ipc.send(j.space, j.end, c.msg(&buf, 1, j.payload, 0), &.{}, j.opts, null) catch |e| {
            j.result = e;
        };
        j.done = true;
    }
};

test "WAIT-03 blocked recv is woken by a send (no timeout)" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    var job = RecvJob{ .rig = &rig, .space = p.b, .end = p.b1, .opts = .{ .timeout_ns = null } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    sleepMs(50);
    try send1(&rig, p.a, p.a0, "Z");
    th.join();
    try testing.expect(job.got);
    try testing.expectEqual(@as(u8, 'Z'), job.byte);
    try rig.consistent();
}

test "WAIT-04 blocked send is woken by a recv making room" {
    var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 1 });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    try send1(&rig, p.a, p.a0, "1");
    var job = SendJob{ .rig = &rig, .space = p.a, .end = p.a0, .opts = .{ .timeout_ns = null }, .payload = "2" };
    const th = try std.Thread.spawn(.{}, SendJob.run, .{&job});
    sleepMs(50);
    try testing.expect(!job.done);
    var out: ipc.Received = undefined;
    try rig.ipc.recv(p.b, p.b1, .{}, &out);
    out.deinit(rig.ipc);
    th.join();
    try testing.expectEqual(@as(?c.OmniError, null), job.result);
    try rig.ipc.recv(p.b, p.b1, .{}, &out);
    try testing.expectEqualStrings("2", out.payload);
    out.deinit(rig.ipc);
    try rig.consistent();
}

test "CANCEL-01 cancel wakes a blocked recv with Cancelled; the end stays usable" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const cx = try rig.ipc.createCancel(p.b);
    var job = RecvJob{ .rig = &rig, .space = p.b, .end = p.b1, .opts = .{ .timeout_ns = null, .cancel = cx } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    sleepMs(50);
    try rig.ipc.signalCancel(p.b, cx);
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.Cancelled), job.result);
    try send1(&rig, p.a, p.a0, "after");
    var out: ipc.Received = undefined;
    try rig.ipc.recv(p.b, p.b1, .{}, &out);
    try testing.expectEqualStrings("after", out.payload);
    out.deinit(rig.ipc);
    try testing.expectEqual(@as(u64, 1), rig.ipc.stats().cancelled);
    try rig.consistent();
}

test "CANCEL-02 cancel wakes a blocked send; nothing is published" {
    var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 1 });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    try send1(&rig, p.a, p.a0, "1");
    const cx = try rig.ipc.createCancel(p.a);
    var job = SendJob{ .rig = &rig, .space = p.a, .end = p.a0, .opts = .{ .timeout_ns = null, .cancel = cx }, .payload = "2" };
    const th = try std.Thread.spawn(.{}, SendJob.run, .{&job});
    sleepMs(50);
    try rig.ipc.signalCancel(p.a, cx);
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.Cancelled), job.result);
    const st = try rig.ipc.endStats(p.b, p.b1);
    try testing.expectEqual(@as(u32, 1), st.queued_messages);
    try testing.expectEqual(@as(u64, 33), try rig.k.spaceUsed(p.a, .queued_bytes)); // only the first message is charged
    try rig.consistent();
}

test "CANCEL-03 a cancel signalled before the call returns Cancelled even if a message is queued" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const cx = try rig.ipc.createCancel(p.b);
    try send1(&rig, p.a, p.a0, "queued");
    try rig.ipc.signalCancel(p.b, cx);
    try rig.ipc.signalCancel(p.b, cx); // idempotent
    var out: ipc.Received = undefined;
    try testing.expectError(error.Cancelled, rig.ipc.recv(p.b, p.b1, .{ .cancel = cx }, &out));
    // the queued message was not consumed
    try rig.ipc.recv(p.b, p.b1, .{}, &out);
    try testing.expectEqualStrings("queued", out.payload);
    out.deinit(rig.ipc);
}

fn cancelAfterCommit(ctx: ?*anyopaque, point: ipc.HookPoint, index: usize) void {
    _ = index;
    const rc: *struct { rig: *c.Rig, space: c.SpaceId, cx: c.Handle } = @ptrCast(@alignCast(ctx.?));
    if (point == .send_committed) rc.rig.ipc.signalCancel(rc.space, rc.cx) catch {};
}

test "CANCEL-04 message arrival beats a cancel signalled right after it (deterministic)" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const cx = try rig.ipc.createCancel(p.b);
    var ctx = .{ .rig = &rig, .space = p.b, .cx = cx };
    rig.ipc.hook = .{ .func = cancelAfterCommit, .ctx = &ctx };
    var job = RecvJob{ .rig = &rig, .space = p.b, .end = p.b1, .opts = .{ .timeout_ns = null, .cancel = cx } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    sleepMs(50);
    try send1(&rig, p.a, p.a0, "M");
    th.join();
    try testing.expect(job.got);
    try testing.expectEqual(@as(?c.OmniError, null), job.result);
}

test "CANCEL-05 cancel vs arrival race, 300 rounds: exactly one outcome, no loss, no duplicate" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    var delivered: u32 = 0;
    var cancelled: u32 = 0;
    var round: u32 = 0;
    while (round < 300) : (round += 1) {
        const cx = try rig.ipc.createCancel(p.b);
        var job = RecvJob{ .rig = &rig, .space = p.b, .end = p.b1, .opts = .{ .timeout_ns = null, .cancel = cx } };
        const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
        if (round % 3 == 0) std.Thread.yield() catch {};
        if (round % 2 == 0) {
            try send1(&rig, p.a, p.a0, "x");
            try rig.ipc.signalCancel(p.b, cx);
        } else {
            try rig.ipc.signalCancel(p.b, cx);
            try send1(&rig, p.a, p.a0, "x");
        }
        th.join();
        const st = try rig.ipc.endStats(p.b, p.b1);
        if (job.got) {
            delivered += 1;
            try testing.expectEqual(@as(?c.OmniError, null), job.result);
            try testing.expectEqual(@as(u32, 0), st.queued_messages); // consumed exactly once
        } else {
            cancelled += 1;
            try testing.expectEqual(@as(?c.OmniError, error.Cancelled), job.result);
            try testing.expectEqual(@as(u32, 1), st.queued_messages); // not lost
            var out: ipc.Received = undefined;
            try rig.ipc.recv(p.b, p.b1, .{}, &out);
            out.deinit(rig.ipc);
        }
        try rig.k.release(p.b, cx);
    }
    try testing.expectEqual(@as(u32, 300), delivered + cancelled);
    try rig.consistent();
}

test "CANCEL-06 cancellation capability: wrong type, rights, stale" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const cx = try rig.ipc.createCancel(p.b);
    var out: ipc.Received = undefined;
    // an endpoint is not a cancellation object
    try testing.expectError(error.InvalidHandle, rig.ipc.recv(p.b, p.b1, .{ .cancel = p.b1 }, &out));
    // WAIT is needed to wait on it, SIGNAL to signal it
    const sig_only = try rig.k.derive(p.b, cx, c.rights(c.R_SIGNAL));
    const wait_only = try rig.k.derive(p.b, cx, c.rights(c.R_WAIT));
    try testing.expectError(error.AccessDenied, rig.ipc.recv(p.b, p.b1, .{ .cancel = sig_only }, &out));
    try testing.expectError(error.AccessDenied, rig.ipc.signalCancel(p.b, wait_only));
    try rig.ipc.signalCancel(p.b, sig_only);
    try testing.expectError(error.Cancelled, rig.ipc.recv(p.b, p.b1, .{ .cancel = wait_only }, &out));
    try rig.k.release(p.b, cx);
    try rig.k.release(p.b, sig_only);
    try rig.k.release(p.b, wait_only); // last reference: object destroyed
    try testing.expectError(error.InvalidHandle, rig.ipc.recv(p.b, p.b1, .{ .cancel = wait_only }, &out));
    try testing.expectEqual(@as(usize, 2), rig.ipc.registered()); // the two channel ends; the cancellation objects are gone
}

test "CLOSE-01 closing the receiving end wakes a blocked sender with PeerDead" {
    var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 1 });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    try send1(&rig, p.a, p.a0, "1");
    var job = SendJob{ .rig = &rig, .space = p.a, .end = p.a0, .opts = .{ .timeout_ns = null }, .payload = "2" };
    const th = try std.Thread.spawn(.{}, SendJob.run, .{&job});
    sleepMs(50);
    try rig.ipc.closeEnd(p.b, p.b1);
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.PeerDead), job.result);
    // queued message was dropped and its accounting refunded
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
    try rig.consistent();
}

test "CLOSE-02 closing the sending end wakes a blocked receiver with PeerDead" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    var job = RecvJob{ .rig = &rig, .space = p.b, .end = p.b1, .opts = .{ .timeout_ns = null } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    sleepMs(50);
    try rig.ipc.closeEnd(p.a, p.a0);
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.PeerDead), job.result);
}

test "CLOSE-03 messages queued before the peer closed are still delivered, then PeerDead" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    try send1(&rig, p.a, p.a0, "x");
    try send1(&rig, p.a, p.a0, "y");
    try rig.ipc.closeEnd(p.a, p.a0);
    var out: ipc.Received = undefined;
    try rig.ipc.recv(p.b, p.b1, .{}, &out);
    try testing.expectEqualStrings("x", out.payload);
    out.deinit(rig.ipc);
    try rig.ipc.recv(p.b, p.b1, .{}, &out);
    try testing.expectEqualStrings("y", out.payload);
    out.deinit(rig.ipc);
    try testing.expectError(error.PeerDead, rig.ipc.recv(p.b, p.b1, .{}, &out));
    try testing.expectError(error.PeerDead, send1(&rig, p.b, p.b1, "late"));
    const st = try rig.ipc.endStats(p.b, p.b1);
    try testing.expect(st.peer_closed);
}

test "CLOSE-04 destroying the owning space closes the end and unblocks the peer" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    var job = RecvJob{ .rig = &rig, .space = p.b, .end = p.b1, .opts = .{ .timeout_ns = null } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    sleepMs(50);
    try rig.k.closeSpace(p.a); // releases a0
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.PeerDead), job.result);
    try rig.consistent();
}

test "CLOSE-05 operations on a closed end fail; double close is harmless" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    try rig.ipc.closeEnd(p.a, p.a0);
    try rig.ipc.closeEnd(p.a, p.a0);
    var out: ipc.Received = undefined;
    try testing.expectError(error.PeerDead, send1(&rig, p.a, p.a0, "x"));
    try testing.expectError(error.PeerDead, rig.ipc.recv(p.a, p.a0, .{}, &out));
    const st = try rig.ipc.endStats(p.a, p.a0);
    try testing.expect(st.closed);
}

test "CLOSE-06 closing an end releases in-flight capabilities exactly once" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    var tr = c.Tracker{};
    const m1 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
    const m2 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER), tr.opts());
    const id1 = try c.idOf(rig.k, p.a, m1, .memory_object);
    const id2 = try c.idOf(rig.k, p.a, m2, .memory_object);
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "f", 2), &.{ c.moveDesc(m1, .memory_object, c.R_READ | c.R_XFER), c.copyDesc(m2, .memory_object, c.R_READ | c.R_XFER) }, .{}, null);
    try testing.expectEqual(@as(u8, 0), tr.count(id1));
    try rig.ipc.closeEnd(p.b, p.b1); // receiver never reads
    try testing.expectEqual(@as(u8, 1), tr.count(id1)); // moved-only object: last reference gone
    try testing.expectEqual(@as(u8, 0), tr.count(id2)); // sender still holds m2
    try rig.k.release(p.a, m2);
    try testing.expectEqual(@as(u8, 1), tr.count(id2));
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
    try rig.consistent();
}

test "CLOSE-07 end objects are destroyed exactly once and the registry empties" {
    var rig = try c.Rig.init(testing.allocator, .{});
    const a = try rig.space();
    var i: usize = 0;
    while (i < 20) : (i += 1) {
        const e = try rig.ipc.createChannel(a);
        try send1(&rig, a, e[0], "m");
        if (i % 2 == 0) try rig.ipc.closeEnd(a, e[1]);
        try rig.k.release(a, e[0]);
        try rig.k.release(a, e[1]);
    }
    try testing.expectEqual(@as(usize, 0), rig.ipc.registered());
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(a, .queued_bytes));
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(a, .channels));
    try rig.consistent();
    try testing.expectEqual(@as(usize, 0), rig.deinit());
}

test "CLOSE-08 kernel teardown with queued messages and in-flight capabilities leaks nothing" {
    var rig = try c.Rig.init(testing.allocator, .{});
    const p = try pair(&rig);
    var tr = c.Tracker{};
    var buf: [64]u8 = undefined;
    var i: usize = 0;
    var ids: [4]c.ObjectId = undefined;
    while (i < 4) : (i += 1) {
        const m = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
        ids[i] = try c.idOf(rig.k, p.a, m, .memory_object);
        try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "t", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    }
    try testing.expectEqual(@as(usize, 0), rig.deinit()); // testing.allocator reports leaks
    for (ids) |id| try testing.expectEqual(@as(u8, 1), tr.count(id));
}

test "CLOSE-09 revoked end capability stops a blocked recv at the next wake-up" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const a = try rig.space();
    const b = try rig.space();
    const e = try rig.ipc.createChannel(a);
    const parent = try rig.k.transfer(a, e[1], b);
    const child = try rig.k.derive(b, parent, c.rights(c.R_READ | c.R_ADMIN));
    var job = RecvJob{ .rig = &rig, .space = b, .end = child, .opts = .{ .timeout_ns = null } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    sleepMs(50);
    _ = try rig.k.revoke(b, parent); // removes `child`
    try send1(&rig, a, e[0], "wake"); // any state change wakes waiters
    th.join();
    try testing.expectEqual(@as(?c.OmniError, error.InvalidHandle), job.result);
    const st = try rig.ipc.endStats(b, parent);
    try testing.expectEqual(@as(u32, 1), st.queued_messages); // not consumed by the revoked waiter
    try rig.consistent();
}

test "WAIT-05 waiting does not spin: CPU time stays far below wall time" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const before = std.posix.getrusage(0);
    var job = RecvJob{ .rig = &rig, .space = p.b, .end = p.b1, .opts = .{ .timeout_ns = 400 * ms } };
    const th = try std.Thread.spawn(.{}, RecvJob.run, .{&job});
    th.join();
    const after = std.posix.getrusage(0);
    try testing.expectEqual(@as(?c.OmniError, error.TimedOut), job.result);
    const used_us = tvUs(after.utime) + tvUs(after.stime) - tvUs(before.utime) - tvUs(before.stime);
    try testing.expect(used_us < 150_000); // a spinning waiter would use ~400000
}

fn tvUs(tv: anytype) i64 {
    return @as(i64, @intCast(tv.sec)) * 1_000_000 + @as(i64, @intCast(tv.usec));
}
