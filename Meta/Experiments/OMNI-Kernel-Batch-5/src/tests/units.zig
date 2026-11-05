//! Unit tests of the building blocks: wait queue, accounting, status mapping, hosted user memory.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;

// ------------------------------------------------------------------ wait queue

test "WQ-01 notification between prepare and wait is never lost" {
    var q = c.wq.WaitQueue.init(4);
    const t = q.prepare();
    q.notifyAll();
    var dl = c.wq.Deadline.init(10 * c.ms);
    try testing.expectEqual(c.wq.Outcome.notified, try q.wait(t, &dl));
    try testing.expectEqual(@as(u32, 0), q.waiterCount());
}

test "WQ-02 wait times out with no notification, not before the deadline" {
    var q = c.wq.WaitQueue.init(4);
    var t0 = try std.time.Timer.start();
    const t = q.prepare();
    var dl = c.wq.Deadline.init(30 * c.ms);
    try testing.expectEqual(c.wq.Outcome.timed_out, try q.wait(t, &dl));
    try testing.expect(t0.read() >= 30 * c.ms);
    try testing.expectEqual(@as(u32, 0), q.waiterCount());
}

test "WQ-03 an expired deadline never blocks" {
    var q = c.wq.WaitQueue.init(4);
    const t = q.prepare();
    var dl = c.wq.Deadline.init(0);
    try testing.expectEqual(c.wq.Outcome.timed_out, try q.wait(t, &dl));
}

test "WQ-04 waiters are bounded" {
    var q = c.wq.WaitQueue.init(1);
    const Ctx = struct {
        fn run(qq: *c.wq.WaitQueue) void {
            const t = qq.prepare();
            var dl = c.wq.Deadline.init(300 * c.ms);
            _ = qq.wait(t, &dl) catch {};
        }
    };
    const th = try std.Thread.spawn(.{}, Ctx.run, .{&q});
    while (q.waiterCount() == 0) std.Thread.sleep(c.ms);
    const t = q.prepare();
    var dl = c.wq.Deadline.init(300 * c.ms);
    try testing.expectError(error.TooManyWaiters, q.wait(t, &dl));
    q.notifyAll();
    th.join();
    try testing.expectEqual(@as(u32, 0), q.waiterCount());
}

const Flag = struct {
    q: c.wq.WaitQueue = c.wq.WaitQueue.init(8),
    value: std.atomic.Value(u32) = .init(0),
    lost: std.atomic.Value(u32) = .init(0),
};

fn flagWaiter(f: *Flag, rounds: u32) void {
    var want: u32 = 1;
    while (want <= rounds) {
        const t = f.q.prepare();
        if (f.value.load(.acquire) >= want) {
            want += 1;
            continue;
        }
        var dl = c.wq.Deadline.init(5000 * c.ms);
        const o = f.q.wait(t, &dl) catch {
            _ = f.lost.fetchAdd(1, .monotonic);
            return;
        };
        if (o == .timed_out) { // a lost wake-up would show up as a 5 s stall
            _ = f.lost.fetchAdd(1, .monotonic);
            return;
        }
    }
}

test "WQ-05 no lost wake-ups: 20000 state changes against a waiting thread" {
    var f = Flag{};
    const rounds: u32 = 20000;
    const th = try std.Thread.spawn(.{}, flagWaiter, .{ &f, rounds });
    var i: u32 = 0;
    while (i < rounds) : (i += 1) {
        f.value.store(i + 1, .release); // state change first
        f.q.notifyAll(); // then publish
        if (i % 64 == 0) std.Thread.yield() catch {};
    }
    th.join();
    try testing.expectEqual(@as(u32, 0), f.lost.load(.acquire));
}

// ------------------------------------------------------------------ accounting

fn space(idx: u32) c.SpaceId {
    return .{ .index = idx, .gen = 1 };
}

test "ACC-01 per-space and total waiter limits are enforced and refunded" {
    var l = c.acct.Ledger.init(testing.allocator, .{ .max_waiters_per_space = 2, .max_waiters_total = 3 });
    defer l.deinit();
    try l.acquireWaiter(space(1));
    try l.acquireWaiter(space(1));
    try testing.expectError(error.QuotaExceeded, l.acquireWaiter(space(1)));
    try l.acquireWaiter(space(2));
    try testing.expectError(error.QuotaExceeded, l.acquireWaiter(space(3))); // total
    try testing.expectEqual(@as(u32, 3), l.waitersTotal());
    try testing.expect(l.audit() == null);
    l.releaseWaiter(space(1));
    try l.acquireWaiter(space(3));
    l.releaseWaiter(space(1));
    l.releaseWaiter(space(2));
    l.releaseWaiter(space(3));
    try testing.expectEqual(@as(u32, 0), l.waitersTotal());
    try testing.expectEqual(@as(u32, 0), l.waitersOf(space(1)));
    try testing.expect(l.audit() == null);
}

test "ACC-02 a double release is detected and reported by audit" {
    var l = c.acct.Ledger.init(testing.allocator, .{});
    defer l.deinit();
    try l.acquireWaiter(space(1));
    l.releaseWaiter(space(1));
    l.releaseWaiter(space(1)); // no matching acquire
    l.releaseWaiter(space(9)); // never acquired
    try testing.expectEqual(@as(u64, 2), l.snapshot().underflows);
    try testing.expectEqual(@as(u32, 0), l.waitersTotal());
    try testing.expect(l.audit() != null);
}

test "ACC-03 ledger allocation failure leaves nothing changed" {
    var fa = std.testing.FailingAllocator.init(testing.allocator, .{ .fail_index = 0 });
    var l = c.acct.Ledger.init(fa.allocator(), .{});
    defer l.deinit();
    try testing.expectError(error.QuotaExceeded, l.acquireWaiter(space(1)));
    try testing.expectEqual(@as(u32, 0), l.waitersTotal());
    try testing.expect(l.audit() == null);
}

test "ACC-04 policy validation" {
    try (c.acct.Policy{}).validate();
    try testing.expectError(error.InvalidArgument, (c.acct.Policy{ .max_wait_ns = 0 }).validate());
    try testing.expectError(error.InvalidArgument, (c.acct.Policy{ .max_wait_ns = c.acct.wait_forever }).validate());
    try testing.expectError(error.InvalidArgument, (c.acct.Policy{ .max_waiters_per_space = 0 }).validate());
    try testing.expectError(error.InvalidArgument, (c.acct.Policy{ .max_waiters_per_space = 8, .max_waiters_total = 4 }).validate());
}

// ------------------------------------------------------------------ status mapping

test "STAT-01 every error maps to a distinct negative status; syscall-only codes are distinct too" {
    const errs = @typeInfo(c.OmniError).error_set.?;
    var seen = std.AutoHashMap(i64, void).init(testing.allocator);
    defer seen.deinit();
    inline for (errs) |e| {
        const err: c.sys.SysError = @field(c.OmniError, e.name);
        const s = c.sys.statusOf(err);
        try testing.expect(s < 0);
        try testing.expect(!seen.contains(s));
        try seen.put(s, {});
        try testing.expectEqual(@as(i64, @intFromEnum(c.b2.err.toStatus(@field(c.OmniError, e.name)))), s);
    }
    try testing.expect(!seen.contains(c.sys.status_fault));
    try testing.expect(!seen.contains(c.sys.status_no_sys));
    try testing.expect(c.sys.status_fault < 0 and c.sys.status_no_sys < 0);
    try testing.expectEqual(@as(usize, 17), errs.len); // a new error forces a review of the mapping
}

// ------------------------------------------------------------------ hosted user memory

test "UMEM-01 copies are bounds-checked, all-or-nothing and overflow-safe" {
    var m = c.hum.HostedUserMemory{};
    var a: [16]u8 = undefined;
    var b: [16]u8 = undefined;
    @memset(&a, 1);
    @memset(&b, 2);
    try m.map(0x1000, &a, true);
    try m.map(0x1010, &b, false); // adjacent, read-only
    const um = m.memory();
    var out: [8]u8 = undefined;
    try um.copy_in(um.ctx, 0x1000, &out);
    try testing.expectEqual(@as(u8, 1), out[0]);
    // spanning two mappings is refused even though both are mapped
    var span: [8]u8 = undefined;
    try testing.expectError(error.Fault, um.copy_in(um.ctx, 0x100C, &span));
    // read-only
    try testing.expectError(error.Fault, um.copy_out(um.ctx, 0x1010, &span));
    try testing.expectError(error.Fault, um.probe_write(um.ctx, 0x1010, 1));
    // out of range and overflow
    try testing.expectError(error.Fault, um.copy_in(um.ctx, 0x2000, &span));
    try testing.expectError(error.Fault, um.copy_in(um.ctx, std.math.maxInt(u64) - 3, &span));
    try testing.expectError(error.Fault, um.copy_in(um.ctx, 0, &span));
    try um.copy_in(um.ctx, 0, out[0..0]); // zero length touches nothing
    // nothing was partially written by the failed copies
    try testing.expectEqual(@as(u8, 2), b[0]);
    try testing.expectError(error.Overlap, m.map(0x1008, &a, true));
    m.unmap(0x1000);
    try testing.expectError(error.Fault, um.copy_in(um.ctx, 0x1000, &span));
}
