//! Thread-state and scheduler-model tests. SIM: `Sched` is a model, not a scheduler.
const std = @import("std");
const c = @import("common.zig");
const ts = c.ts;
const Sched = c.Sched;
const WakeReason = c.sched.WakeReason;
const testing = std.testing;

const sp1 = c.SpaceId{ .index = 1, .gen = 1 };
const sp2 = c.SpaceId{ .index = 2, .gen = 1 };

fn running(s: *Sched, space: c.SpaceId) !c.ThreadId {
    const t = try s.create(space);
    try s.dispatch(t, null);
    return t;
}

test "TS-01 thread-state transition table is exactly the specified set" {
    const S = ts.State;
    const all = [_]S{ .invalid, .runnable, .running, .blocked, .stopped, .dying, .dead };
    const allowed_pairs = [_][2]S{
        .{ .invalid, .runnable },
        .{ .runnable, .running },
        .{ .runnable, .dying },
        .{ .running, .runnable },
        .{ .running, .blocked },
        .{ .running, .stopped },
        .{ .running, .dying },
        .{ .blocked, .runnable },
        .{ .stopped, .runnable },
        .{ .stopped, .dying },
        .{ .dying, .dead },
    };
    var n: usize = 0;
    for (all) |from| {
        for (all) |to| {
            var want = false;
            for (allowed_pairs) |p| {
                if (p[0] == from and p[1] == to) want = true;
            }
            try testing.expectEqual(want, ts.allowed(from, to));
            if (want) n += 1;
            if (from == to) {
                try testing.expectError(error.SameState, ts.check(from, to));
            } else if (!want) {
                try testing.expectError(error.InvalidTransition, ts.check(from, to));
            } else try ts.check(from, to);
        }
    }
    try testing.expectEqual(allowed_pairs.len, n);
    // The rules the existing Kernel/ enforces with PANIC/VERIFY:
    try testing.expect(!ts.allowed(.blocked, .dying)); // wake first
    try testing.expect(!ts.allowed(.runnable, .stopped)); // stop only from running
    try testing.expect(!ts.allowed(.blocked, .running)); // never run without being woken
    try testing.expect(ts.canBlock(.running) and !ts.canBlock(.runnable) and !ts.canBlock(.blocked));
    for (all) |to| try testing.expect(!ts.allowed(.dead, to)); // terminal
}

test "TS-02 scheduler model lifecycle; every invalid operation is refused and changes nothing" {
    var s = Sched{};
    const t = try s.create(sp1);
    try testing.expectEqual(ts.State.runnable, s.stateOf(t).?);
    try testing.expectError(error.InvalidState, s.prepareBlock(t)); // not running: cannot block
    try s.dispatch(t, null);
    try testing.expectError(error.InvalidTransition, s.dispatch(t, null)); // running -> running
    try testing.expectError(error.InvalidTransition, s.finish(t)); // running -> dead
    try testing.expectEqual(ts.State.running, s.stateOf(t).?);

    const tk = try s.prepareBlock(t);
    try testing.expect((try s.block(tk)) == .blocked);
    try testing.expectEqual(ts.State.blocked, s.stateOf(t).?);
    try testing.expectError(error.InvalidState, s.prepareBlock(t)); // blocked cannot block again
    try testing.expectError(error.InvalidTransition, s.beginDeath(t)); // blocked -> dying refused
    try testing.expectError(error.InvalidTransition, s.resumeThread(t, null)); // blocked -> running refused
    try testing.expectError(error.InvalidState, s.yieldRunning(t)); // blocked -> runnable only by wake
    try testing.expectEqual(ts.State.blocked, s.stateOf(t).?);

    try testing.expectEqual(c.sched.WakeResult.made_runnable, s.wake(t, .woken));
    try testing.expectEqual(ts.State.runnable, s.stateOf(t).?);
    try testing.expectEqual(WakeReason.woken, try s.resumeThread(t, null));
    try s.exit(t);
    try testing.expectEqual(@as(?ts.State, null), s.stateOf(t));
    try testing.expect(s.audit() == null);
    try testing.expectEqual(@as(u32, 0), s.totalThreads());
}

test "TS-03 stale thread ids are rejected; slots are reused with a new generation" {
    var s = Sched{};
    const t1 = try running(&s, sp1);
    try s.exit(t1);
    const t2 = try running(&s, sp2);
    try testing.expect(t1 != t2);
    try testing.expectEqual(@as(?c.SpaceId, null), s.spaceOf(t1));
    try testing.expectEqual(sp2.index, s.spaceOf(t2).?.index);
    try testing.expectEqual(c.sched.WakeResult.ignored, s.wake(t1, .woken));
    try testing.expectEqual(c.sched.WakeResult.ignored, s.kill(t1));
    try testing.expectError(error.NoSuchThread, s.prepareBlock(t1));
    try testing.expectError(error.NoSuchThread, s.dispatch(t1, null));
    try testing.expectEqual(ts.State.running, s.stateOf(t2).?); // untouched by the stale calls
    try testing.expectError(error.NoSuchThread, s.prepareBlock(.invalid));
    try testing.expectError(error.NoSuchThread, s.prepareBlock(@enumFromInt(0xFFFF)));
    try s.exit(t2);

    var ids: [c.sched.max_threads]c.ThreadId = undefined;
    for (&ids) |*id| id.* = try s.create(sp1);
    try testing.expectError(error.TooManyThreads, s.create(sp1));
    try s.dispatch(ids[5], null);
    try s.exit(ids[5]);
    const again = try s.create(sp1);
    try testing.expect(again != ids[5]);
    var prev = again;
    try s.dispatch(again, null);
    try s.exit(again);
    var i: u32 = 0;
    while (i < 1000) : (i += 1) {
        const nt = try s.create(sp2);
        try testing.expect(nt != prev); // a reused slot never reuses an id
        try s.dispatch(nt, null);
        try s.exit(nt);
        prev = nt;
    }
    try testing.expect(s.audit() == null);
}

test "TS-04 SIM cpu table: one running thread per cpu, busy and bad cpus refused" {
    var s = Sched{};
    const a = try s.create(sp1);
    const b = try s.create(sp1);
    try s.dispatch(a, 0);
    try testing.expectEqual(a, s.current(0).?);
    try testing.expectError(error.CpuBusy, s.dispatch(b, 0));
    try testing.expectEqual(ts.State.runnable, s.stateOf(b).?); // unchanged
    try testing.expectError(error.BadCpu, s.dispatch(b, 9));
    try testing.expectEqual(ts.State.runnable, s.stateOf(b).?);
    try s.dispatch(b, 1);
    try testing.expect(s.audit() == null);
    const tk = try s.prepareBlock(a);
    _ = try s.block(tk);
    try testing.expectEqual(@as(?c.ThreadId, null), s.current(0)); // blocked releases the cpu
    try s.yieldRunning(b);
    try testing.expectEqual(@as(?c.ThreadId, null), s.current(1));
    try testing.expectEqual(@as(?c.ThreadId, null), s.current(7));
    try testing.expect(s.audit() == null);
}

test "WK-01 a wake that precedes prepareBlock is stale: the thread then blocks" {
    var s = Sched{};
    const t = try running(&s, sp1);
    try testing.expectEqual(c.sched.WakeResult.recorded, s.wake(t, .woken)); // nobody waits yet
    const tk = try s.prepareBlock(t);
    try testing.expect((try s.block(tk)) == .blocked);
    try testing.expectEqual(ts.State.blocked, s.stateOf(t).?);
    _ = s.wake(t, .woken);
    _ = try s.resumeThread(t, null);
    try s.exit(t);
}

test "WK-02 a wake between prepareBlock and block is never lost" {
    var s = Sched{};
    const t = try running(&s, sp1);
    const tk = try s.prepareBlock(t);
    try testing.expectEqual(c.sched.WakeResult.recorded, s.wake(t, .woken));
    const r = try s.block(tk);
    try testing.expectEqual(WakeReason.woken, r.immediate);
    try testing.expectEqual(ts.State.running, s.stateOf(t).?); // it never blocked
    const st = s.statsSnapshot();
    try testing.expectEqual(@as(u64, 1), st.immediate);
    try testing.expectEqual(@as(u64, 0), st.blocks);
    try s.exit(t);
}

test "WK-03 a wake after block makes the thread runnable; resume reports the reason" {
    var s = Sched{};
    const t = try running(&s, sp1);
    const tk = try s.prepareBlock(t);
    try testing.expect((try s.block(tk)) == .blocked);
    try testing.expectEqual(c.sched.WakeResult.made_runnable, s.timeoutFire(t));
    try testing.expectEqual(ts.State.runnable, s.stateOf(t).?);
    // A second wake before the thread runs again is recorded, not a second transition.
    try testing.expectEqual(c.sched.WakeResult.recorded, s.wake(t, .woken));
    try testing.expectEqual(WakeReason.woken, try s.resumeThread(t, null)); // woken outranks timeout
    try testing.expectEqual(ts.State.running, s.stateOf(t).?);
    try s.exit(t);
}

test "WK-04 wake reasons have a fixed precedence" {
    var s = Sched{};
    const t = try running(&s, sp1);
    const Case = struct { a: WakeReason, b: WakeReason, want: WakeReason };
    const cases = [_]Case{
        .{ .a = .timeout, .b = .woken, .want = .woken },
        .{ .a = .woken, .b = .timeout, .want = .woken },
        .{ .a = .woken, .b = .cancelled, .want = .cancelled },
        .{ .a = .cancelled, .b = .timeout, .want = .cancelled },
        .{ .a = .cancelled, .b = .woken, .want = .cancelled },
    };
    for (cases) |cs| {
        const tk = try s.prepareBlock(t);
        _ = s.wake(t, cs.a);
        _ = s.wake(t, cs.b);
        try testing.expectEqual(cs.want, (try s.block(tk)).immediate);
    }
    try s.exit(t);
}

test "WK-05 kill is sticky, wakes a blocked thread, and killSpace is per address space" {
    var s = Sched{};
    const t = try running(&s, sp1);
    try testing.expectEqual(c.sched.WakeResult.recorded, s.kill(t));
    for (0..3) |_| {
        const tk = try s.prepareBlock(t);
        try testing.expectEqual(WakeReason.killed, (try s.block(tk)).immediate); // never blocks again
    }
    try testing.expect(s.killRequested(t));
    try testing.expectEqual(WakeReason.killed, try s.blockWait(try s.prepareBlock(t), 5_000 * c.ms));
    try s.exit(t);
    try testing.expectEqual(c.sched.WakeResult.ignored, s.kill(t)); // gone

    const blocked = try running(&s, sp1);
    const runner = try running(&s, sp1);
    const ready = try s.create(sp1);
    const other = try running(&s, sp2);
    _ = try s.block(try s.prepareBlock(blocked));
    try testing.expectEqual(@as(u32, 3), s.killSpace(sp1));
    try testing.expectEqual(ts.State.runnable, s.stateOf(blocked).?); // woken by the kill
    try testing.expect(s.killRequested(runner) and s.killRequested(ready) and s.killRequested(blocked));
    try testing.expect(!s.killRequested(other));
    try testing.expectEqual(WakeReason.killed, try s.resumeThread(blocked, null));
    for ([_]c.ThreadId{ blocked, runner }) |x| try s.exit(x);
    try s.beginDeath(ready); // runnable -> dying is allowed
    try s.finish(ready);
    try testing.expectEqual(@as(u32, 0), s.liveThreads(sp1));
    try testing.expectEqual(@as(u32, 1), s.liveThreads(sp2));
    try s.exit(other);
    try testing.expect(s.audit() == null);
}

const Ev = enum { w, t, k };

fn applyEvent(s: *Sched, tid: c.ThreadId, e: Ev) void {
    switch (e) {
        .w => _ = s.wake(tid, .woken),
        .t => _ = s.timeoutFire(tid),
        .k => _ = s.kill(tid),
    }
}

fn rankOf(e: Ev) u8 {
    return switch (e) {
        .t => @intFromEnum(WakeReason.timeout),
        .w => @intFromEnum(WakeReason.woken),
        .k => @intFromEnum(WakeReason.killed),
    };
}

test "WK-06 SIM exhaustive interleavings of {wake, timeout, kill} around prepare/block (384 cases)" {
    const perms = [6][3]usize{ .{ 0, 1, 2 }, .{ 0, 2, 1 }, .{ 1, 0, 2 }, .{ 1, 2, 0 }, .{ 2, 0, 1 }, .{ 2, 1, 0 } };
    const evs = [3]Ev{ .w, .t, .k };
    var cases: u32 = 0;
    var code: u32 = 0;
    while (code < 64) : (code += 1) {
        // place[i]: 0 = absent, 1 = before prepare, 2 = between prepare and block, 3 = after block
        const place = [3]u32{ code % 4, (code / 4) % 4, (code / 16) % 4 };
        for (perms) |perm| {
            cases += 1;
            var s = Sched{};
            const t = try running(&s, sp1);
            for (perm) |i| if (place[i] == 1) applyEvent(&s, t, evs[i]);
            const tk = try s.prepareBlock(t);
            for (perm) |i| if (place[i] == 2) applyEvent(&s, t, evs[i]);
            const res = try s.block(tk);

            const kill_early = place[2] == 1 or place[2] == 2;
            var best_p2: u8 = 0; // best rank among events in the window
            var any_p2 = false;
            for (perm) |i| {
                if (place[i] == 2) {
                    any_p2 = true;
                    best_p2 = @max(best_p2, rankOf(evs[i]));
                }
            }
            if (kill_early) {
                try testing.expectEqual(WakeReason.killed, res.immediate);
                try testing.expectEqual(ts.State.running, s.stateOf(t).?);
            } else if (any_p2) {
                try testing.expectEqual(best_p2, @intFromEnum(res.immediate));
                try testing.expectEqual(ts.State.running, s.stateOf(t).?);
            } else {
                try testing.expect(res == .blocked); // stale events before prepare do not count
                try testing.expectEqual(ts.State.blocked, s.stateOf(t).?);
                var best_p3: u8 = 0;
                var any_p3 = false;
                for (perm) |i| if (place[i] == 3) {
                    any_p3 = true;
                    best_p3 = @max(best_p3, rankOf(evs[i]));
                    applyEvent(&s, t, evs[i]);
                };
                if (any_p3) {
                    try testing.expectEqual(ts.State.runnable, s.stateOf(t).?);
                    const reason = try s.resumeThread(t, null);
                    try testing.expectEqual(best_p3, @intFromEnum(reason));
                } else {
                    try testing.expectEqual(ts.State.blocked, s.stateOf(t).?); // nothing woke it
                    _ = s.wake(t, .woken);
                    _ = try s.resumeThread(t, null);
                }
            }
            try testing.expect(s.audit() == null);
            try s.exit(t);
        }
    }
    try testing.expectEqual(@as(u32, 384), cases);
}

fn wakerAfterBlocked(s: *Sched, tid: c.ThreadId) void {
    c.waitState(s, tid, .blocked) catch return;
    _ = s.wake(tid, .woken);
}

test "WK-07 SIM blockWait: timeout, wake from another thread, wake already pending" {
    var s = Sched{};
    const t = try running(&s, sp1);

    var timer = try std.time.Timer.start();
    const r1 = try s.blockWait(try s.prepareBlock(t), 30 * c.ms);
    try testing.expectEqual(WakeReason.timeout, r1);
    try testing.expect(timer.read() >= 30 * c.ms); // never early
    try testing.expectEqual(ts.State.running, s.stateOf(t).?);

    const tk = try s.prepareBlock(t);
    const th = try std.Thread.spawn(.{}, wakerAfterBlocked, .{ &s, t });
    timer.reset();
    const r2 = try s.blockWait(tk, null);
    th.join();
    try testing.expectEqual(WakeReason.woken, r2);
    try testing.expect(timer.read() < 2_000 * c.ms);
    try testing.expectEqual(ts.State.running, s.stateOf(t).?);

    const tk3 = try s.prepareBlock(t);
    _ = s.wake(t, .cancelled);
    try testing.expectEqual(WakeReason.cancelled, try s.blockWait(tk3, null)); // returns at once
    try s.exit(t);
    try testing.expect(s.audit() == null);
}

fn racer(s: *Sched, tid: c.ThreadId, delay_us: u64) void {
    std.Thread.sleep(delay_us * std.time.ns_per_us);
    _ = s.wake(tid, .woken);
}

test "WK-08 SIM timeout racing a wake, 100 rounds: either outcome, state always valid" {
    var s = Sched{};
    const t = try running(&s, sp1);
    var woken: u32 = 0;
    var timed: u32 = 0;
    var i: u32 = 0;
    while (i < 100) : (i += 1) {
        const tk = try s.prepareBlock(t);
        const th = try std.Thread.spawn(.{}, racer, .{ &s, t, 600 + (i % 7) * 100 });
        const r = try s.blockWait(tk, 1 * c.ms);
        th.join();
        switch (r) {
            .woken => woken += 1,
            .timeout => timed += 1,
            else => return error.UnexpectedReason,
        }
        try testing.expectEqual(ts.State.running, s.stateOf(t).?);
        try testing.expect(s.audit() == null);
    }
    try testing.expectEqual(@as(u32, 100), woken + timed);
    try s.exit(t);
}
