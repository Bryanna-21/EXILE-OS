//! Process / thread termination, resource cleanup and context-switch contract tests.
//! SIM: threads are hosted OS threads standing for kernel threads; no real exit path runs.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const Atomic = std.atomic.Value;
const Rig = c.Rig;
const OmniError = c.kipc.OmniError;

fn cleanup(r: *Rig, ps: []const c.Proc) !void {
    try r.audit();
    for (ps) |p| r.freeProc(p);
    try testing.expectEqual(@as(usize, 0), r.destroy());
}

const Waiter = struct {
    r: *Rig,
    tid: c.ThreadId,
    ep: c.Handle,
    err: ?OmniError = null,
    ok: bool = false,
    done: Atomic(bool) = .init(false),
};

/// A kernel thread that blocks in recv and, if it is killed, runs its exit path.
fn waiterMain(w: *Waiter) void {
    defer w.done.store(true, .release);
    var out: c.kipc.Received = undefined;
    if (w.r.ip.recv(w.tid, w.ep, .{ .timeout_ns = 20_000 * c.ms }, &out)) |_| {
        w.ok = true;
        w.r.x.deinitReceived(&out);
    } else |e| w.err = e;
    w.r.s.exit(w.tid) catch {
        w.err = error.Internal;
    };
}

test "PR-01 SIM terminating a process with blocked threads, queued messages and in-flight capabilities" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r); // pair 1: A <-> B
    const ta = try r.spawn(p.a);
    const q2 = try c.pair(r); // pair 2: carries a message with an in-flight capability
    const tq = try r.spawn(q2.a);
    const obj = try r.k.createObject(q2.a.space, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    try r.ip.send(tq, q2.a0, c.msg(&buf, 1, "queued", 1), &.{c.moveDesc(obj, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    try testing.expect(try r.k.spaceUsed(q2.a.space, .queued_bytes) > 0);

    // process B: two threads blocked on pair 1, a third one running
    var ws: [2]Waiter = undefined;
    var ths: [2]std.Thread = undefined;
    for (&ws, 0..) |*w, i| {
        w.* = .{ .r = r, .tid = try r.spawn(p.b), .ep = p.b1 };
        ths[i] = try std.Thread.spawn(.{}, waiterMain, .{w});
    }
    const runner = try r.spawn(p.b);
    for (ws) |w| try c.waitState(&r.s, w.tid, .blocked);
    try testing.expectEqual(@as(u32, 2), r.x.ledger.waitersOf(p.b.space));

    const marked = try r.ip.terminateSpace(p.b.space);
    try testing.expectEqual(@as(u32, 3), marked);
    for (ths) |t| t.join();
    for (ws) |w| try testing.expectEqual(@as(?OmniError, error.Cancelled), w.err);
    try r.s.exit(runner);
    try testing.expect(r.s.waitSpaceEmpty(p.b.space, 2_000 * c.ms));

    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    // the peer sees the closure
    try testing.expectError(error.PeerDead, r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{1}, 0), &.{}, .{}, null));
    // closing the receiving process of pair 2 would release the queued message and capability
    _ = try r.ip.terminateSpace(q2.b.space);
    try testing.expectEqual(@as(u64, 0), try r.k.spaceUsed(q2.a.space, .queued_bytes));
    try r.s.exit(ta);
    try r.s.exit(tq);
    try testing.expectEqual(@as(u32, 0), r.s.totalThreads());
    try cleanup(r, &.{ p.a, p.b, q2.a, q2.b });
}

test "PR-02 SIM one thread exiting leaves the process alive; terminating twice is an error, not a crash" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    const tb1 = try r.spawn(p.b);
    const tb2 = try r.spawn(p.b);
    try r.s.exit(tb1); // one thread of B leaves
    try testing.expectEqual(@as(u32, 1), r.s.liveThreads(p.b.space));
    var buf: [64]u8 = undefined;
    try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{5}, 0), &.{}, .{}, null); // B's endpoint still works
    var out: c.kipc.Received = undefined;
    try r.ip.recv(tb2, p.b1, .{}, &out);
    r.x.deinitReceived(&out);

    try testing.expectEqual(@as(u32, 1), try r.ip.terminateSpace(p.b.space));
    try r.s.exit(tb2);
    // the second termination: no threads left, the space is already closed
    try testing.expectEqual(@as(u32, 0), r.s.killSpace(p.b.space));
    if (r.ip.terminateSpace(p.b.space)) |_| {
        return error.ExpectedError;
    } else |_| {}
    // the dead process's ids are gone for good
    try testing.expectError(error.NoSuchThread, r.s.prepareBlock(tb2));
    try r.s.exit(ta);
    try cleanup(r, &.{ p.a, p.b });
}

const Inner = struct {
    r: *Rig,
    mem: c.sys.UserMemory,
    space: c.SpaceId,
    ep: c.Handle,
    status: i64 = 1,
    done: Atomic(bool) = .init(false),
};

fn innerRecv(j: *Inner) void {
    defer j.done.store(true, .release);
    j.status = c.sys.dispatch(j.r.x, j.mem, j.space, .{ .nr = @intFromEnum(c.sys.Nr.recv), .a = .{ @intFromEnum(j.ep), c.A_ARGS, 0, 0, 0, 0 } });
}

test "PR-03 SIM KNOWN GAP: a thread blocked inside Batch 5 dispatch is invisible to the scheduler model; closing its space still frees it" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const tb = try r.spawn(p.b);
    var binding = try c.um.Binding.init(p.b.space, p.b.model);
    try c.prepRecv(p.b, 20_000 * c.ms, 0);
    var job = Inner{ .r = r, .mem = binding.memory(), .space = p.b.space, .ep = p.b1 };
    const th = try std.Thread.spawn(.{}, innerRecv, .{&job});
    var spins: u32 = 0;
    while (r.x.ledger.waitersOf(p.b.space) != 1 and spins < 3000) : (spins += 1) std.Thread.sleep(1 * c.ms);
    try testing.expectEqual(@as(u32, 1), r.x.ledger.waitersOf(p.b.space)); // it IS waiting (Batch 5's queue)
    // ... but the scheduler model still believes the thread is running: it cannot wake it
    try testing.expectEqual(c.ts.State.running, r.s.stateOf(tb).?);
    var timer = try std.time.Timer.start();
    _ = try r.ip.terminateSpace(p.b.space); // spaceClose wakes Batch 5's own waiters
    th.join();
    try testing.expect(timer.read() < 3_000 * c.ms);
    try testing.expect(job.status < 0); // woken with an error, not left for the 20 s deadline
    try r.s.exit(tb);
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try cleanup(r, &.{ p.a, p.b });
}

test "PR-04 SIM 100 process lifecycles (create, IPC, terminate, free): no residue, ids never reused" {
    const r = try Rig.create(testing.allocator, .{});
    var last: c.ThreadId = .invalid;
    var i: u32 = 0;
    while (i < 100) : (i += 1) {
        const p = try c.pair(r);
        const ta = try r.spawn(p.a);
        const tb = try r.spawn(p.b);
        try testing.expect(ta != last and tb != last);
        const obj = try r.k.createObject(p.a.space, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        var buf: [64]u8 = undefined;
        try r.ip.send(ta, p.a0, c.msg(&buf, 1, &.{@truncate(i)}, 1), &.{c.moveDesc(obj, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
        if (i % 2 == 0) {
            var out: c.kipc.Received = undefined;
            try r.ip.recv(tb, p.b1, .{}, &out);
            r.x.deinitReceived(&out);
        }
        _ = try r.ip.terminateSpace(p.a.space);
        _ = try r.ip.terminateSpace(p.b.space);
        try r.s.exit(ta);
        try r.s.exit(tb);
        try testing.expectEqual(@as(u32, 0), r.s.liveThreads(p.a.space) + r.s.liveThreads(p.b.space));
        r.freeProc(p.a);
        r.freeProc(p.b);
        last = tb;
        if (i % 25 == 24) try r.audit();
    }
    try testing.expectEqual(@as(u32, 0), r.s.totalThreads());
    try testing.expectEqual(@as(u32, 0), r.x.ledger.waitersTotal());
    try r.audit();
    try testing.expectEqual(@as(usize, 0), r.destroy());
}

const sp1 = c.SpaceId{ .index = 1, .gen = 1 };
const sp2 = c.SpaceId{ .index = 2, .gen = 1 };
const quiet = c.arch.ExecContext{ .from_user = false, .irq_enabled = false, .critical_depth = 1 };

test "CTX-01 SIM context-switch contract: preconditions, bookkeeping and address-space switching" {
    var s = c.Sched{};
    var cpu = c.ctxsw.SimCpu{ .id = 0 };
    const t1 = try s.create(sp1);
    const t2 = try s.create(sp1);
    const t3 = try s.create(sp2);

    // every precondition, each leaving the CPU and the threads untouched
    var bad = quiet;
    bad.in_irq = true;
    try testing.expectError(error.InIrq, cpu.contextSwitch(&s, bad, null, t1));
    bad = quiet;
    bad.irq_enabled = true;
    try testing.expectError(error.IrqEnabled, cpu.contextSwitch(&s, bad, null, t1));
    for ([_]u32{ 0, 2 }) |depth| {
        bad = quiet;
        bad.critical_depth = depth;
        try testing.expectError(error.CriticalDepth, cpu.contextSwitch(&s, bad, null, t1));
    }
    bad = quiet;
    bad.spinlocks_held = 1;
    try testing.expectError(error.LocksHeld, cpu.contextSwitch(&s, bad, null, t1));
    try testing.expectEqual(@as(?c.ThreadId, null), cpu.current);
    try testing.expectEqual(c.ts.State.runnable, s.stateOf(t1).?);

    try cpu.contextSwitch(&s, quiet, null, t1); // first switch, from idle
    try testing.expectEqual(t1, cpu.current.?);
    try testing.expectEqual(c.ts.State.running, s.stateOf(t1).?);
    try testing.expectEqual(@as(u64, 1), cpu.space_switches);
    try testing.expectError(error.FromNotCurrent, cpu.contextSwitch(&s, quiet, null, t2)); // not idle
    try testing.expectError(error.FromNotCurrent, cpu.contextSwitch(&s, quiet, t2, t3));
    try testing.expectError(error.SameThread, cpu.contextSwitch(&s, quiet, t1, t1));
    try testing.expectError(error.FromStillRunning, cpu.contextSwitch(&s, quiet, t1, t2)); // t1 not yet descheduled

    try s.yieldRunning(t1);
    try cpu.contextSwitch(&s, quiet, t1, t2);
    try testing.expectEqual(@as(u64, 1), cpu.space_switches); // same address space: no switch
    try testing.expectEqual(c.ts.State.runnable, s.stateOf(t1).?);

    try s.yieldRunning(t2);
    try cpu.contextSwitch(&s, quiet, t2, t3);
    try testing.expectEqual(@as(u64, 2), cpu.space_switches); // different space: switched
    try testing.expectEqual(sp2.index, cpu.loaded_space.?.index);

    // a blocking thread leaves the CPU, then another thread is switched in
    const tk = try s.prepareBlock(t3);
    try testing.expect((try s.block(tk)) == .blocked);
    try cpu.contextSwitch(&s, quiet, t3, t1);
    try testing.expectEqual(@as(u64, 3), cpu.space_switches);
    // switching to a blocked, unknown or already-running thread is refused
    try s.yieldRunning(t1);
    try testing.expectError(error.ToNotRunnable, cpu.contextSwitch(&s, quiet, t1, t3)); // blocked
    try testing.expectError(error.ToMissing, cpu.contextSwitch(&s, quiet, t1, .invalid));
    try cpu.contextSwitch(&s, quiet, t1, t2);
    try testing.expectEqual(@as(u64, 6) - 3, cpu.space_switches - 0); // sp1 -> sp1: unchanged since the last switch
    try testing.expect(s.audit() == null);

    // two CPUs: a thread running on cpu 0 cannot also be switched in on cpu 1
    var cpu1 = c.ctxsw.SimCpu{ .id = 1 };
    try testing.expectError(error.ToNotRunnable, cpu1.contextSwitch(&s, quiet, null, t2));
    try testing.expect(s.audit() == null);
    try testing.expectEqual(@as(usize, 8), c.ctxsw.calleeSaved(.x86_64).len);
    try testing.expectEqual(@as(usize, 13), c.ctxsw.calleeSaved(.aarch64).len);
    try testing.expectEqual(@as(usize, 4), c.ctxsw.extraState(.x86_64).len);
}
