//! IPC / scheduler coordination tests. Blocking is done by the scheduler MODEL (SIM) on
//! hosted OS threads; each OS thread stands for one simulated kernel thread.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const Atomic = std.atomic.Value;
const Rig = c.Rig;
const OmniError = c.kipc.OmniError;

const Job = struct {
    r: *Rig,
    tid: c.ThreadId,
    ep: c.Handle,
    w: c.kipc.Wait = .{},
    byte: u8 = 0,
    err: ?OmniError = null,
    got: u8 = 0,
    plen: usize = 0,
    ok: bool = false,
    done: Atomic(bool) = .init(false),
};

fn recvJob(j: *Job) void {
    defer j.done.store(true, .release);
    var out: c.kipc.Received = undefined;
    j.r.ip.recv(j.tid, j.ep, j.w, &out) catch |e| {
        j.err = e;
        return;
    };
    j.plen = out.payload.len;
    if (out.payload.len > 0) j.got = out.payload[0];
    j.ok = true;
    j.r.x.deinitReceived(&out);
}

fn sendJob(j: *Job) void {
    defer j.done.store(true, .release);
    var buf: [64]u8 = undefined;
    j.r.ip.send(j.tid, j.ep, c.msg(&buf, 1, &.{j.byte}, 0), &.{}, j.w, null) catch |e| {
        j.err = e;
        return;
    };
    j.ok = true;
}

fn waiters(r: *Rig, space: c.SpaceId) u32 {
    return r.x.ledger.waitersOf(space);
}

fn waitFor(cond: anytype) !void {
    var i: u32 = 0;
    while (i < 3000) : (i += 1) {
        if (cond.check()) return;
        std.Thread.sleep(1 * c.ms);
    }
    return error.ConditionNotReached;
}

const Waiters = struct {
    r: *Rig,
    space: c.SpaceId,
    n: u32,
    fn check(self: Waiters) bool {
        return waiters(self.r, self.space) == self.n;
    }
};

fn cleanup(r: *Rig, ps: []const c.Proc) !void {
    try r.audit();
    for (ps) |p| r.freeProc(p);
    try testing.expectEqual(@as(usize, 0), r.destroy());
}

fn queued(r: *Rig, p: c.Proc, ep: c.Handle) !u32 {
    return (try r.x.endpointStats(p.space, ep)).queued_messages;
}

test "IPC-01 SIM a blocked receiver is `blocked` in the scheduler and a send wakes it" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb = try r.spawn(p.b);
    var job = Job{ .r = r, .tid = tb, .ep = p.b1, .w = .{ .timeout_ns = 5_000 * c.ms } };
    const th = try std.Thread.spawn(.{}, recvJob, .{&job});
    try c.waitState(&r.s, tb, .blocked);
    try waitFor(Waiters{ .r = r, .space = p.b.space, .n = 1 });
    try testing.expect(!job.done.load(.acquire));
    var buf: [64]u8 = undefined;
    try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{0x5A}, 0), &.{}, .{}, null);
    th.join();
    try testing.expect(job.ok);
    try testing.expectEqual(@as(u8, 0x5A), job.got);
    try testing.expectEqual(c.ts.State.running, r.s.stateOf(tb).?);
    try testing.expectEqual(@as(u32, 0), waiters(r, p.b.space));
    try cleanup(r, &.{ p.a, p.b });
}

test "IPC-02 SIM a blocked sender is woken by a receive that makes room" {
    const r = try Rig.create(testing.allocator, .{ .ipc = .{ .max_messages = 1 } });
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb = try r.spawn(p.b);
    var buf: [64]u8 = undefined;
    try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{1}, 0), &.{}, .{}, null); // queue now full
    var job = Job{ .r = r, .tid = ta, .ep = p.a0, .byte = 2, .w = .{ .timeout_ns = 5_000 * c.ms } };
    const th = try std.Thread.spawn(.{}, sendJob, .{&job});
    try c.waitState(&r.s, ta, .blocked);
    var out: c.kipc.Received = undefined;
    try r.ip.recv(tb, p.b1, .{}, &out);
    try testing.expectEqual(@as(u8, 1), out.payload[0]);
    r.x.deinitReceived(&out);
    th.join();
    try testing.expect(job.ok);
    try r.ip.recv(tb, p.b1, .{}, &out);
    try testing.expectEqual(@as(u8, 2), out.payload[0]); // order preserved
    r.x.deinitReceived(&out);
    try testing.expectEqual(@as(u32, 0), waiters(r, p.a.space));
    try cleanup(r, &.{ p.a, p.b });
}

test "IPC-03 SIM timeout: TimedOut no earlier than the deadline, thread running, slot returned" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const tb = try r.spawn(p.b);
    var out: c.kipc.Received = undefined;
    var timer = try std.time.Timer.start();
    try testing.expectError(error.TimedOut, r.ip.recv(tb, p.b1, .{ .timeout_ns = 40 * c.ms }, &out));
    try testing.expect(timer.read() >= 40 * c.ms);
    try testing.expectEqual(c.ts.State.running, r.s.stateOf(tb).?);
    try testing.expectEqual(@as(u32, 0), waiters(r, p.b.space));
    // poll never waits
    timer.reset();
    try testing.expectError(error.WouldBlock, r.ip.recv(tb, p.b1, .{}, &out));
    try testing.expect(timer.read() < 20 * c.ms);
    // policy: a longer wait than the limit and a missing cancel for "forever" are refused
    try testing.expectError(error.InvalidArgument, r.ip.recv(tb, p.b1, .{ .timeout_ns = c.kipc.wait_forever }, &out));
    try testing.expectError(error.InvalidArgument, r.ip.recv(tb, p.b1, .{ .timeout_ns = 31_000_000 * c.ms }, &out));
    try cleanup(r, &.{ p.a, p.b });
}

test "IPC-04 SIM cancellation wakes a blocked receiver; a cancel signalled first consumes nothing" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb = try r.spawn(p.b);
    const tb2 = try r.spawn(p.b); // a second thread of the same process signals
    const cancel = try r.x.cancelCreate(p.b.space);
    var job = Job{ .r = r, .tid = tb, .ep = p.b1, .w = .{ .timeout_ns = c.kipc.wait_forever, .cancel = cancel } };
    const th = try std.Thread.spawn(.{}, recvJob, .{&job});
    try c.waitState(&r.s, tb, .blocked);
    try r.ip.cancelSignal(tb2, cancel);
    th.join();
    try testing.expectEqual(@as(?OmniError, error.Cancelled), job.err);
    try testing.expectEqual(c.ts.State.running, r.s.stateOf(tb).?);
    try testing.expectEqual(@as(u32, 0), waiters(r, p.b.space));

    // already signalled: a queued message is NOT consumed
    var buf: [64]u8 = undefined;
    try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{9}, 0), &.{}, .{}, null);
    var out: c.kipc.Received = undefined;
    try testing.expectError(error.Cancelled, r.ip.recv(tb, p.b1, .{ .cancel = cancel }, &out));
    try testing.expectEqual(@as(u32, 1), try queued(r, p.b, p.b1));
    try r.ip.recv(tb, p.b1, .{}, &out);
    try testing.expectEqual(@as(u8, 9), out.payload[0]);
    r.x.deinitReceived(&out);
    try cleanup(r, &.{ p.a, p.b });
}

test "IPC-05 SIM closing the peer wakes blocked receivers and senders with PeerDead" {
    const r = try Rig.create(testing.allocator, .{ .ipc = .{ .max_messages = 1 } });
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb = try r.spawn(p.b);
    var rj = Job{ .r = r, .tid = tb, .ep = p.b1, .w = .{ .timeout_ns = 5_000 * c.ms } };
    const th = try std.Thread.spawn(.{}, recvJob, .{&rj});
    try c.waitState(&r.s, tb, .blocked);
    try r.ip.endpointClose(ta, p.a0);
    th.join();
    try testing.expectEqual(@as(?OmniError, error.PeerDead), rj.err);

    // sender side: fresh pair, fill the queue, block a sender, close the receiving end
    const q = try c.pair(r);
    const qa = try r.spawn(q.a);
    const qb = try r.spawn(q.b);
    var buf: [64]u8 = undefined;
    try r.ip.send(qa, q.a0, c.msg(&buf, 1, &.{1}, 0), &.{}, .{}, null);
    var sj = Job{ .r = r, .tid = qa, .ep = q.a0, .byte = 2, .w = .{ .timeout_ns = 5_000 * c.ms } };
    const th2 = try std.Thread.spawn(.{}, sendJob, .{&sj});
    try c.waitState(&r.s, qa, .blocked);
    try r.ip.releaseHandle(qb, q.b1);
    th2.join();
    try testing.expectEqual(@as(?OmniError, error.PeerDead), sj.err);
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try cleanup(r, &.{ p.a, p.b, q.a, q.b });
}

test "IPC-06 SIM waiter slots: bounded per space, returned on every path" {
    const r = try Rig.create(testing.allocator, .{ .policy = .{ .max_waiters_per_space = 2, .max_waiters_total = 4 } });
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    var jobs: [3]Job = undefined;
    var ths: [3]std.Thread = undefined;
    for (&jobs, 0..) |*j, i| {
        const tb = try r.spawn(p.b);
        j.* = Job{ .r = r, .tid = tb, .ep = p.b1, .w = .{ .timeout_ns = 5_000 * c.ms } };
        ths[i] = try std.Thread.spawn(.{}, recvJob, .{j});
        if (i < 2) try waitFor(Waiters{ .r = r, .space = p.b.space, .n = @intCast(i + 1) });
    }
    // the third could not get a slot: refused at once, nothing consumed
    var spins: u32 = 0;
    while (!jobs[2].done.load(.acquire) and spins < 3000) : (spins += 1) std.Thread.sleep(1 * c.ms);
    try testing.expect(jobs[2].done.load(.acquire));
    try testing.expectEqual(@as(?OmniError, error.QuotaExceeded), jobs[2].err);
    try testing.expectEqual(@as(u32, 2), waiters(r, p.b.space));
    var buf: [64]u8 = undefined;
    for (0..2) |i| try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{@intCast(i)}, 0), &.{}, .{}, null);
    for (ths) |t| t.join();
    try testing.expect(jobs[0].ok and jobs[1].ok);
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try cleanup(r, &.{ p.a, p.b });
}

test "IPC-07 SIM killing a blocked receiver: it ends with Cancelled, no message is lost, no slot leaks" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb1 = try r.spawn(p.b);
    const tb2 = try r.spawn(p.b);
    var job = Job{ .r = r, .tid = tb1, .ep = p.b1, .w = .{ .timeout_ns = 10_000 * c.ms } };
    const th = try std.Thread.spawn(.{}, recvJob, .{&job});
    try c.waitState(&r.s, tb1, .blocked);
    _ = r.s.kill(tb1);
    th.join();
    try testing.expectEqual(@as(?OmniError, error.Cancelled), job.err);
    try testing.expectEqual(@as(u32, 0), waiters(r, p.b.space));
    try r.s.exit(tb1); // the thread now runs its exit path
    // a message sent afterwards is delivered to the surviving thread, not swallowed
    var buf: [64]u8 = undefined;
    try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{7}, 0), &.{}, .{}, null);
    try testing.expectEqual(@as(u32, 1), try queued(r, p.b, p.b1));
    var out: c.kipc.Received = undefined;
    try r.ip.recv(tb2, p.b1, .{ .timeout_ns = 1000 * c.ms }, &out);
    try testing.expectEqual(@as(u8, 7), out.payload[0]);
    r.x.deinitReceived(&out);
    try cleanup(r, &.{ p.a, p.b });
}

test "IPC-08 a thread that is already being killed starts no operation and consumes nothing" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb = try r.spawn(p.b);
    var buf: [64]u8 = undefined;
    try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{3}, 0), &.{}, .{}, null);
    _ = r.s.kill(tb);
    var out: c.kipc.Received = undefined;
    try testing.expectError(error.Cancelled, r.ip.recv(tb, p.b1, .{}, &out));
    try testing.expectError(error.Cancelled, r.ip.recv(tb, p.b1, .{ .timeout_ns = 100 * c.ms }, &out));
    try testing.expectEqual(@as(u32, 1), try queued(r, p.b, p.b1)); // still there
    _ = r.s.kill(ta);
    try testing.expectError(error.Cancelled, r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{4}, 0), &.{}, .{}, null));
    try testing.expectEqual(@as(u32, 1), try queued(r, p.b, p.b1)); // nothing was sent
    try r.s.exit(ta);
    try r.s.exit(tb);
    try cleanup(r, &.{ p.a, p.b });
}

test "IPC-09 SIM eight blocked receivers, eight sends: every message wakes exactly one" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    var jobs: [8]Job = undefined;
    var ths: [8]std.Thread = undefined;
    for (&jobs, 0..) |*j, i| {
        j.* = Job{ .r = r, .tid = try r.spawn(p.b), .ep = p.b1, .w = .{ .timeout_ns = 10_000 * c.ms } };
        ths[i] = try std.Thread.spawn(.{}, recvJob, .{j});
    }
    try waitFor(Waiters{ .r = r, .space = p.b.space, .n = 8 });
    var buf: [64]u8 = undefined;
    for (0..8) |i| try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{@intCast(10 + i)}, 0), &.{}, .{}, null);
    for (ths) |t| t.join();
    var seen = [_]bool{false} ** 8;
    for (jobs) |j| {
        try testing.expect(j.ok);
        try testing.expect(j.got >= 10 and j.got < 18);
        try testing.expect(!seen[j.got - 10]); // no message delivered twice
        seen[j.got - 10] = true;
    }
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try cleanup(r, &.{ p.a, p.b });
}

test "IPC-10 the calling space comes from the thread: a thread of another process cannot use a handle value" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const intruder = try r.newProc(); // owns no handles at all
    const ti = try r.spawn(intruder);
    const ta = try r.spawn(p.a);
    var buf: [64]u8 = undefined;
    // the same raw handle values that work for A and B are meaningless to the intruder
    try testing.expectError(error.InvalidHandle, r.ip.send(ti, p.a0, c.msg(&buf, 1, &.{1}, 0), &.{}, .{}, null));
    var out: c.kipc.Received = undefined;
    try testing.expectError(error.InvalidHandle, r.ip.recv(ti, p.b1, .{}, &out));
    try testing.expectError(error.InvalidHandle, r.ip.endpointClose(ti, p.a0));
    try testing.expectError(error.InvalidHandle, r.ip.cancelSignal(ti, p.a0));
    try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{1}, 0), &.{}, .{}, null); // the owner still can
    try testing.expectEqual(@as(u32, 1), try queued(r, p.b, p.b1));
    // an exited / unknown thread has no space: refused, never defaulted
    try r.s.exit(ti);
    try testing.expectError(error.InvalidState, r.ip.send(ti, p.a0, c.msg(&buf, 1, &.{1}, 0), &.{}, .{}, null));
    try cleanup(r, &.{ p.a, p.b, intruder });
}

const Pump = struct {
    r: *Rig,
    pa: c.Proc,
    pb: c.Proc,
    a0: c.Handle,
    b1: c.Handle,
    per: u32,
    total: u32,
    consumed: Atomic(u32) = .init(0),
    bad: Atomic(u32) = .init(0),
    seen: []Atomic(u8),
};

fn pumpProducer(pm: *Pump, id: u8) void {
    const tid = pm.r.spawn(pm.pa) catch {
        _ = pm.bad.fetchAdd(1, .monotonic);
        return;
    };
    var i: u32 = 0;
    while (i < pm.per) : (i += 1) {
        var buf: [64]u8 = undefined;
        var pl: [5]u8 = undefined;
        pl[0] = id;
        std.mem.writeInt(u32, pl[1..5], i, .little);
        pm.r.ip.send(tid, pm.a0, c.msg(&buf, 1, &pl, 0), &.{}, .{ .timeout_ns = 10_000 * c.ms }, null) catch {
            _ = pm.bad.fetchAdd(1, .monotonic);
            break;
        };
    }
    pm.r.s.exit(tid) catch {
        _ = pm.bad.fetchAdd(1, .monotonic);
    };
}

fn pumpConsumer(pm: *Pump) void {
    const tid = pm.r.spawn(pm.pb) catch {
        _ = pm.bad.fetchAdd(1, .monotonic);
        return;
    };
    while (pm.consumed.load(.acquire) < pm.total) {
        var out: c.kipc.Received = undefined;
        pm.r.ip.recv(tid, pm.b1, .{ .timeout_ns = 20 * c.ms }, &out) catch |e| {
            if (e == error.TimedOut) continue;
            _ = pm.bad.fetchAdd(1, .monotonic);
            break;
        };
        const id = out.payload[0];
        const seq = std.mem.readInt(u32, out.payload[1..5], .little);
        pm.r.x.deinitReceived(&out);
        if (id >= 3) {
            _ = pm.bad.fetchAdd(1, .monotonic);
            continue;
        }
        if (pm.seen[@as(usize, id) * pm.per + seq].fetchAdd(1, .monotonic) != 0) _ = pm.bad.fetchAdd(1, .monotonic);
        _ = pm.consumed.fetchAdd(1, .release);
    }
    pm.r.s.exit(tid) catch {
        _ = pm.bad.fetchAdd(1, .monotonic);
    };
}

test "IPC-11 SIM concurrent senders and receivers (3x3 threads) through the scheduler model: exactly once" {
    const r = try Rig.create(testing.allocator, .{ .ipc = .{ .max_messages = 4 } });
    const p = try c.pair(r);
    const per: u32 = 150;
    const seen = try testing.allocator.alloc(Atomic(u8), per * 3);
    defer testing.allocator.free(seen);
    for (seen) |*s| s.* = .init(0);
    var pm = Pump{ .r = r, .pa = p.a, .pb = p.b, .a0 = p.a0, .b1 = p.b1, .per = per, .total = per * 3, .seen = seen };
    var cons: [3]std.Thread = undefined;
    var prods: [3]std.Thread = undefined;
    for (&cons) |*t| t.* = try std.Thread.spawn(.{}, pumpConsumer, .{&pm});
    for (&prods, 0..) |*t, i| t.* = try std.Thread.spawn(.{}, pumpProducer, .{ &pm, @as(u8, @intCast(i)) });
    for (prods) |t| t.join();
    for (cons) |t| t.join();
    try testing.expectEqual(@as(u32, 0), pm.bad.load(.acquire));
    try testing.expectEqual(per * 3, pm.consumed.load(.acquire));
    for (seen) |*s| try testing.expectEqual(@as(u8, 1), s.load(.acquire));
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try testing.expectEqual(@as(u32, 0), r.s.totalThreads());
    try cleanup(r, &.{ p.a, p.b });
}

fn lateSend(j: *Job) void {
    std.Thread.sleep(@as(u64, j.byte % 5) * 300 * std.time.ns_per_us);
    var buf: [64]u8 = undefined;
    j.r.ip.send(j.tid, j.ep, c.msg(&buf, 1, &.{j.byte}, 0), &.{}, .{}, null) catch |e| {
        j.err = e;
        return;
    };
    j.ok = true;
}

test "IPC-12 SIM timeout racing an arrival, 100 rounds: the message is received or still queued, never lost" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb = try r.spawn(p.b);
    var got: u32 = 0;
    var late: u32 = 0;
    var i: u32 = 0;
    while (i < 100) : (i += 1) {
        var sj = Job{ .r = r, .tid = ta, .ep = p.a0, .byte = @intCast(i) };
        const th = try std.Thread.spawn(.{}, lateSend, .{&sj});
        var out: c.kipc.Received = undefined;
        if (r.ip.recv(tb, p.b1, .{ .timeout_ns = 1 * c.ms }, &out)) |_| {
            try testing.expectEqual(@as(u8, @intCast(i)), out.payload[0]);
            r.x.deinitReceived(&out);
            got += 1;
        } else |e| {
            try testing.expectEqual(error.TimedOut, e);
            th.join();
            try testing.expect(sj.ok);
            try r.ip.recv(tb, p.b1, .{}, &out); // the send completed: it MUST be there
            try testing.expectEqual(@as(u8, @intCast(i)), out.payload[0]);
            r.x.deinitReceived(&out);
            late += 1;
            continue;
        }
        th.join();
        try testing.expect(sj.ok);
    }
    try testing.expectEqual(@as(u32, 100), got + late);
    try testing.expectEqual(@as(u32, 0), try queued(r, p.b, p.b1));
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try cleanup(r, &.{ p.a, p.b });
}

fn lateCancel(j: *Job) void {
    std.Thread.sleep(@as(u64, j.byte % 5) * 300 * std.time.ns_per_us);
    j.r.ip.cancelSignal(j.tid, j.ep) catch |e| {
        j.err = e;
        return;
    };
    j.ok = true;
}

test "IPC-13 SIM cancel racing an arrival, 100 rounds: exactly one outcome and the message is conserved" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb = try r.spawn(p.b);
    const tb2 = try r.spawn(p.b);
    var cancelled: u32 = 0;
    var received: u32 = 0;
    var i: u32 = 0;
    while (i < 100) : (i += 1) {
        const cancel = try r.x.cancelCreate(p.b.space);
        var sj = Job{ .r = r, .tid = ta, .ep = p.a0, .byte = @intCast(i) };
        var cj = Job{ .r = r, .tid = tb2, .ep = cancel, .byte = @intCast(i + 2) };
        const t1 = try std.Thread.spawn(.{}, lateSend, .{&sj});
        const t2 = try std.Thread.spawn(.{}, lateCancel, .{&cj});
        var out: c.kipc.Received = undefined;
        const res = r.ip.recv(tb, p.b1, .{ .timeout_ns = c.kipc.wait_forever, .cancel = cancel }, &out);
        t1.join();
        t2.join();
        try testing.expect(sj.ok and cj.ok);
        if (res) |_| {
            r.x.deinitReceived(&out);
            received += 1;
            try testing.expectEqual(@as(u32, 0), try queued(r, p.b, p.b1));
        } else |e| {
            try testing.expectEqual(error.Cancelled, e);
            cancelled += 1;
            try testing.expectEqual(@as(u32, 1), try queued(r, p.b, p.b1)); // conserved
            try r.ip.recv(tb, p.b1, .{}, &out);
            r.x.deinitReceived(&out);
        }
        try r.ip.releaseHandle(tb, cancel);
    }
    try testing.expectEqual(@as(u32, 100), cancelled + received);
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try cleanup(r, &.{ p.a, p.b });
}

fn noise(r: *Rig, q: c.Pair, ta: c.ThreadId, tb: c.ThreadId, stop: *Atomic(bool)) void {
    var buf: [64]u8 = undefined;
    while (!stop.load(.acquire)) {
        r.ip.send(ta, q.a0, c.msg(&buf, 1, &.{1}, 0), &.{}, .{}, null) catch return;
        var out: c.kipc.Received = undefined;
        r.ip.recv(tb, q.b1, .{}, &out) catch return;
        r.x.deinitReceived(&out);
    }
}

test "IPC-14 SIM unrelated traffic wakes a waiter spuriously but never ends its wait early" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const q = try c.pair(r);
    const tb = try r.spawn(p.b);
    const qa = try r.spawn(q.a);
    const qb = try r.spawn(q.b);
    var stop = Atomic(bool).init(false);
    const th = try std.Thread.spawn(.{}, noise, .{ r, q, qa, qb, &stop });
    var out: c.kipc.Received = undefined;
    var timer = try std.time.Timer.start();
    try testing.expectError(error.TimedOut, r.ip.recv(tb, p.b1, .{ .timeout_ns = 120 * c.ms }, &out));
    try testing.expect(timer.read() >= 120 * c.ms);
    stop.store(true, .release);
    th.join();
    try testing.expect(r.s.statsSnapshot().made_runnable > 0); // it WAS woken by the noise
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try cleanup(r, &.{ p.a, p.b, q.a, q.b });
}
