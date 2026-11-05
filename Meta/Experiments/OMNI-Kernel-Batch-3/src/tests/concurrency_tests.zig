//! Concurrency tests (CON-xx). Real threads against one Kernel.
//!
//! Interleavings that matter are forced with events, not left to chance. The
//! stress and race tests then check that no invariant breaks over many
//! unforced interleavings. They are not a proof: see docs/CONCURRENCY.md.

const std = @import("std");
const c = @import("common.zig");
const expect = std.testing.expect;
const expectEqual = std.testing.expectEqual;
const Event = std.Thread.ResetEvent;

const wait_ns: u64 = 10 * std.time.ns_per_s;

fn waitFor(e: *Event) !void {
    e.timedWait(wait_ns) catch return error.TestTimedOut;
}

// ---------------------------------------------------------------------------

test "CON-01 release is linearized before its destructor: other threads observe the removal, and the lock is free, while the destructor runs" {
    const S = struct {
        var in_dtor: Event = .{};
        var observed: Event = .{};
        var dying_seen: usize = 0;
        fn dtor(_: ?*anyopaque, _: c.ObjectId) void {
            in_dtor.set();
            observed.timedWait(wait_ns) catch {};
        }
        fn worker(k: *c.Kernel, s: c.SpaceId, h: c.Handle) void {
            k.release(s, h) catch @panic("release failed");
        }
    };
    S.in_dtor = .{};
    S.observed = .{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const h = try k.createObject(s, .channel, c.rights(c.R_DUP | c.R_ADMIN), .{ .destroy_fn = S.dtor, .charge_resource = .pages, .charge_amount = 4 });
    const th = try std.Thread.spawn(.{}, S.worker, .{ k, s, h });
    try waitFor(&S.in_dtor);
    // The releasing thread is inside the destructor and has returned nothing yet.
    try std.testing.expectError(error.InvalidHandle, k.lookup(s, h, .channel, c.Rights.none));
    try std.testing.expectError(error.InvalidHandle, k.derive(s, h, c.Rights.none));
    try std.testing.expectError(error.InvalidHandle, k.revoke(s, h));
    const other = try k.createObject(s, .timer, c.Rights.none, .{}); // kernel is usable
    try expectEqual(@as(usize, 1), k.stats().dying_objects);
    try expectEqual(@as(u64, 4), try k.spaceUsed(s, .pages)); // still charged until the destructor returns
    try expect(k.mutex.tryLock());
    k.mutex.unlock();
    S.observed.set();
    th.join();
    try expectEqual(@as(usize, 0), k.stats().dying_objects);
    try expectEqual(@as(u64, 0), try k.spaceUsed(s, .pages));
    _ = try k.lookup(s, other, .timer, c.Rights.none);
    try c.expectConsistent(k);
}

// ---------------------------------------------------------------------------

test "CON-02 revoke is atomic: an observer never sees a later-read descendant alive after an earlier-read one was revoked" {
    const S = struct {
        k: *c.Kernel,
        s: c.SpaceId,
        pair: std.atomic.Value(u64) = .init(0),
        stop: std.atomic.Value(bool) = .init(false),
        violations: std.atomic.Value(u32) = .init(0),
        samples: std.atomic.Value(u32) = .init(0),

        fn observer(self: *@This()) void {
            while (!self.stop.load(.acquire)) {
                const p = self.pair.load(.acquire);
                if (p == 0) continue;
                const h1 = c.Handle.fromRaw(@truncate(p));
                const h2 = c.Handle.fromRaw(@truncate(p >> 32));
                const a = self.k.lookup(self.s, h1, .channel, c.Rights.none);
                const b = self.k.lookup(self.s, h2, .channel, c.Rights.none);
                _ = self.samples.fetchAdd(1, .monotonic);
                if (std.meta.isError(a) and !std.meta.isError(b)) {
                    // h1 was already revoked when read, so h2 (revoked in the same step) must be too.
                    _ = self.violations.fetchAdd(1, .monotonic);
                }
            }
        }
    };
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    var st = S{ .k = k, .s = try k.createSpace(c.big) };
    const root = try k.createObject(st.s, .channel, c.rights(c.R_DUP | c.R_ADMIN), .{});
    const th = try std.Thread.spawn(.{}, S.observer, .{&st});
    var i: usize = 0;
    while (i < 3000) : (i += 1) {
        const h1 = try k.derive(st.s, root, c.Rights.none);
        const h2 = try k.derive(st.s, root, c.Rights.none);
        st.pair.store(@as(u64, h2.raw()) << 32 | h1.raw(), .release);
        try expectEqual(@as(usize, 2), try k.revoke(st.s, root));
    }
    st.stop.store(true, .release);
    th.join();
    try expectEqual(@as(u32, 0), st.violations.load(.monotonic));
    try c.expectConsistent(k);
}

// ---------------------------------------------------------------------------

test "CON-03 derive versus revoke race: nothing derived is left dangling, accounting balances" {
    const S = struct {
        const n = 4000;
        k: *c.Kernel,
        s: c.SpaceId,
        root: c.Handle,
        got: [n]c.Handle = undefined,
        ok: [n]bool = [_]bool{false} ** n,
        fn deriver(self: *@This()) void {
            for (0..n) |i| {
                if (self.k.derive(self.s, self.root, c.rights(c.R_DUP | c.R_READ))) |h| {
                    self.got[i] = h;
                    self.ok[i] = true;
                } else |e| switch (e) {
                    error.InvalidHandle, error.AccessDenied => {},
                    else => @panic("unexpected derive error"),
                }
            }
        }
        fn revoker(self: *@This()) void {
            for (0..n / 4) |_| _ = self.k.revoke(self.s, self.root) catch @panic("revoke failed");
        }
    };
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    var st = S{ .k = k, .s = try k.createSpace(c.big), .root = undefined };
    st.root = try k.createObject(st.s, .channel, c.rights(c.R_DUP | c.R_ADMIN | c.R_READ), .{});
    const t1 = try std.Thread.spawn(.{}, S.deriver, .{&st});
    const t2 = try std.Thread.spawn(.{}, S.revoker, .{&st});
    t1.join();
    t2.join();
    try c.expectConsistent(k);
    var alive: usize = 0;
    for (0..S.n) |i| {
        if (!st.ok[i]) continue;
        if (k.lookup(st.s, st.got[i], .channel, c.Rights.none)) |_| alive += 1 else |e| try expectEqual(error.InvalidHandle, e);
    }
    try expectEqual(alive + 1, k.stats().nodes);
    try expectEqual(@as(u64, alive + 1), try k.spaceUsed(st.s, .handles));
    try expectEqual(alive, try k.revoke(st.s, st.root));
    try expectEqual(@as(usize, 1), k.stats().nodes);
}

// ---------------------------------------------------------------------------

test "CON-04 double release race: exactly one release of each handle succeeds; destructor once each" {
    const S = struct {
        const n = 3000;
        k: *c.Kernel,
        s: c.SpaceId,
        hs: [n]c.Handle = undefined,
        wins: std.atomic.Value(u32) = .init(0),
        bad: std.atomic.Value(u32) = .init(0),
        fn run(self: *@This(), reverse: bool) void {
            for (0..n) |j| {
                const i = if (reverse) n - 1 - j else j;
                if (self.k.release(self.s, self.hs[i])) |_| {
                    _ = self.wins.fetchAdd(1, .monotonic);
                } else |e| {
                    if (e != error.InvalidHandle) _ = self.bad.fetchAdd(1, .monotonic);
                }
            }
        }
    };
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    // No mutex probe here: another thread may legitimately hold the lock.
    var st = S{ .k = k, .s = try k.createSpace(c.big) };
    for (&st.hs) |*h| h.* = try k.createObject(st.s, .timer, c.Rights.none, t.opts());
    const t1 = try std.Thread.spawn(.{}, S.run, .{ &st, false });
    const t2 = try std.Thread.spawn(.{}, S.run, .{ &st, true });
    t1.join();
    t2.join();
    try expectEqual(@as(u32, S.n), st.wins.load(.monotonic));
    try expectEqual(@as(u32, 0), st.bad.load(.monotonic));
    try expectEqual(@as(u32, S.n), t.total);
    for (t.counts[1 .. S.n + 1]) |cnt| try expectEqual(@as(u8, 1), cnt);
    try expectEqual(@as(usize, 0), k.stats().objects);
    try c.expectConsistent(k);
}

test "CON-05 transfer race: two threads move the same handles to different spaces; each moves at most once" {
    const S = struct {
        const n = 2000;
        k: *c.Kernel,
        src: c.SpaceId,
        dst: [2]c.SpaceId,
        hs: [n]c.Handle = undefined,
        wins: [2]std.atomic.Value(u32) = .{ .init(0), .init(0) },
        bad: std.atomic.Value(u32) = .init(0),
        fn run(self: *@This(), who: usize) void {
            for (0..n) |i| {
                if (self.k.transfer(self.src, self.hs[i], self.dst[who])) |_| {
                    _ = self.wins[who].fetchAdd(1, .monotonic);
                } else |e| {
                    if (e != error.InvalidHandle) _ = self.bad.fetchAdd(1, .monotonic);
                }
            }
        }
    };
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    var st = S{ .k = k, .src = try k.createSpace(c.big), .dst = undefined };
    st.dst = .{ try k.createSpace(c.big), try k.createSpace(c.big) };
    for (&st.hs) |*h| h.* = try k.createObject(st.src, .channel, c.rights(c.R_XFER), .{});
    const t1 = try std.Thread.spawn(.{}, S.run, .{ &st, @as(usize, 0) });
    const t2 = try std.Thread.spawn(.{}, S.run, .{ &st, @as(usize, 1) });
    t1.join();
    t2.join();
    try expectEqual(@as(u32, 0), st.bad.load(.monotonic));
    try expectEqual(@as(u32, S.n), st.wins[0].load(.monotonic) + st.wins[1].load(.monotonic));
    try expectEqual(@as(u64, st.wins[0].load(.monotonic)), try k.spaceUsed(st.dst[0], .handles));
    try expectEqual(@as(u64, st.wins[1].load(.monotonic)), try k.spaceUsed(st.dst[1], .handles));
    try expectEqual(@as(u64, 0), try k.spaceUsed(st.src, .handles));
    try c.expectConsistent(k);
}

test "CON-06 revoke versus transfer race: moved capabilities are still revoked; nothing leaks" {
    const S = struct {
        const n = 3000;
        k: *c.Kernel,
        a: c.SpaceId,
        b: c.SpaceId,
        root: c.Handle,
        fn mover(self: *@This()) void {
            for (0..n) |_| {
                const h = self.k.derive(self.a, self.root, c.rights(c.R_XFER | c.R_DUP)) catch continue;
                _ = self.k.transfer(self.a, h, self.b) catch |e| switch (e) {
                    error.InvalidHandle => {},
                    else => @panic("unexpected transfer error"),
                };
            }
        }
        fn revoker(self: *@This()) void {
            for (0..n / 3) |_| _ = self.k.revoke(self.a, self.root) catch @panic("revoke failed");
        }
    };
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    var st = S{ .k = k, .a = try k.createSpace(c.big), .b = try k.createSpace(c.big), .root = undefined };
    st.root = try k.createObject(st.a, .channel, c.rights(c.R_DUP | c.R_ADMIN), .{});
    const t1 = try std.Thread.spawn(.{}, S.mover, .{&st});
    const t2 = try std.Thread.spawn(.{}, S.revoker, .{&st});
    t1.join();
    t2.join();
    try c.expectConsistent(k);
    _ = try k.revoke(st.a, st.root);
    // After a final revoke with no concurrent derive, only the root remains anywhere.
    try expectEqual(@as(usize, 1), k.stats().nodes);
    try expectEqual(@as(u64, 0), try k.spaceUsed(st.b, .handles));
    try expectEqual(@as(u64, 1), try k.spaceUsed(st.a, .handles));
    try c.expectConsistent(k);
}

// ---------------------------------------------------------------------------

const Mail = struct {
    lock: std.Thread.Mutex = .{},
    items: [256]c.Handle = undefined,
    n: usize = 0,
    fn push(m: *Mail, h: c.Handle) bool {
        m.lock.lock();
        defer m.lock.unlock();
        if (m.n == m.items.len) return false;
        m.items[m.n] = h;
        m.n += 1;
        return true;
    }
};

const Stress = struct {
    const workers = 4;
    const ops = 4000;
    k: *c.Kernel,
    tracker: *c.Tracker,
    spaces: [workers]c.SpaceId,
    mail: [workers]Mail = [_]Mail{.{}} ** workers,
    created: std.atomic.Value(u32) = .init(0),
    failed: std.atomic.Value(bool) = .init(false),

    fn run(self: *Stress, me: usize, seed: u64) void {
        var prng = std.Random.DefaultPrng.init(seed);
        const rnd = prng.random();
        var mine: [96]c.Handle = undefined;
        var n: usize = 0;
        const my_space = self.spaces[me];
        var step: usize = 0;
        while (step < ops and !self.failed.load(.monotonic)) : (step += 1) {
            // Drain the mailbox.
            {
                const m = &self.mail[me];
                m.lock.lock();
                defer m.lock.unlock();
                while (m.n > 0 and n < mine.len) {
                    m.n -= 1;
                    mine[n] = m.items[m.n];
                    n += 1;
                }
                // Anything that does not fit is dropped from the list but stays valid in the
                // kernel; it is released when the space is closed at the end.
                m.n = 0;
            }
            const pick: usize = if (n == 0) 0 else rnd.uintLessThan(usize, n);
            const op = rnd.uintLessThan(u8, 100);
            var err: ?anyerror = null;
            var drop = false;
            if (n == 0 or op < 25) {
                if (n < mine.len) {
                    const bits: u32 = (rnd.uintLessThan(u32, 512)) | if (rnd.boolean()) (c.R_DUP | c.R_XFER | c.R_ADMIN) else 0;
                    const typ: c.ObjectType = if (rnd.boolean()) .channel else .timer;
                    if (self.k.createObject(my_space, typ, c.rights(bits), self.tracker.optsCharged(rnd.uintLessThan(u64, 3)))) |h| {
                        mine[n] = h;
                        n += 1;
                        _ = self.created.fetchAdd(1, .monotonic);
                    } else |e| err = e;
                }
            } else if (op < 50) {
                if (n < mine.len) {
                    if (self.k.derive(my_space, mine[pick], c.rights(rnd.uintLessThan(u32, 512)))) |h| {
                        mine[n] = h;
                        n += 1;
                    } else |e| err = e;
                }
            } else if (op < 65) {
                if (self.k.release(my_space, mine[pick])) |_| {
                    drop = true;
                } else |e| {
                    err = e;
                }
            } else if (op < 75) {
                _ = self.k.revoke(my_space, mine[pick]) catch |e| {
                    err = e;
                };
            } else if (op < 90) {
                const to = rnd.uintLessThan(usize, workers);
                if (self.k.transfer(my_space, mine[pick], self.spaces[to])) |nh| {
                    drop = true;
                    if (!self.mail[to].push(nh)) self.k.release(self.spaces[to], nh) catch {};
                } else |e| err = e;
            } else if (op < 97) {
                if (self.k.beginOp(my_space, mine[pick], .channel, c.Rights.none)) |tok| {
                    _ = self.k.opStillAuthorized(tok);
                    self.k.endOp(tok) catch |e| {
                        err = e;
                    };
                } else |e| err = e;
            } else {
                _ = self.k.lookup(my_space, mine[pick], .timer, c.Rights.none) catch |e| {
                    err = e;
                };
            }
            if (err) |e| switch (e) {
                // The handle is gone (revoked or released through another path), or a
                // permitted refusal: all expected in this workload.
                error.InvalidHandle => drop = true,
                error.AccessDenied, error.QuotaExceeded, error.InvalidArgument => {},
                else => {
                    std.debug.print("stress worker {d}: unexpected error {s}\n", .{ me, @errorName(e) });
                    self.failed.store(true, .monotonic);
                },
            };
            if (drop and n > 0) {
                mine[pick] = mine[n - 1];
                n -= 1;
            }
        }
    }
};

test "CON-07 stress: four threads mixing create, derive, release, revoke, transfer and ops; conservation at the end" {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    // No mutex probe here: another thread may legitimately hold the lock.
    const st = try std.testing.allocator.create(Stress);
    defer std.testing.allocator.destroy(st);
    st.* = .{ .k = k, .tracker = &t, .spaces = undefined };
    for (&st.spaces) |*sp| sp.* = try k.createSpace(c.big);

    var threads: [Stress.workers]std.Thread = undefined;
    for (&threads, 0..) |*th, i| th.* = try std.Thread.spawn(.{}, Stress.run, .{ st, i, 0x5EED + i });
    for (threads) |th| th.join();
    try expect(!st.failed.load(.monotonic));
    try c.expectConsistent(k);

    for (st.spaces) |sp| try k.closeSpace(sp);
    try expectEqual(@as(u32, st.created.load(.monotonic)), t.total);
    for (t.counts[1 .. t.total + 1]) |cnt| try expectEqual(@as(u8, 1), cnt);
    const s = k.stats();
    try expectEqual(@as(usize, 0), s.objects);
    try expectEqual(@as(usize, 0), s.nodes);
    try expectEqual(@as(usize, 0), s.pins);
    try expectEqual(@as(usize, 0), s.spaces);
    inline for (std.meta.tags(c.Resource)) |r| try expectEqual(@as(u64, 0), k.rootUsed(r));
    try c.expectConsistent(k);
}
