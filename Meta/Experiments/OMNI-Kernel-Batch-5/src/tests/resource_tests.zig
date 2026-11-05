//! Resource exhaustion, accounting consistency, allocation-failure injection,
//! repeated creation/destruction, endpoint destruction and failure recovery.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const ipc4 = c.ipc4;

fn sendOne(x: *c.KernelIpc, sp: c.SpaceId, h: c.Handle, payload: []const u8) !void {
    var buf: [4096]u8 = undefined;
    try x.send(sp, h, c.msg(&buf, 1, payload, 0), &.{}, .{}, null);
}

fn recvDrop(x: *c.KernelIpc, sp: c.SpaceId, h: c.Handle) !void {
    var out: ipc4.Received = undefined;
    try x.recv(sp, h, .{}, &out);
    x.deinitReceived(&out);
}

test "RES-01 queue exhaustion by count is deterministic; a blocking send times out, not WouldBlock" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 2 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    try sendOne(rig.x, p.a, p.a0, "a");
    try sendOne(rig.x, p.a, p.a0, "b");
    try testing.expectError(error.WouldBlock, sendOne(rig.x, p.a, p.a0, "c"));
    var buf: [64]u8 = undefined;
    try testing.expectError(error.TimedOut, rig.x.send(p.a, p.a0, c.msg(&buf, 1, "c", 0), &.{}, .{ .timeout_ns = 20 * c.ms }, null));
    const st = try rig.x.endpointStats(p.b, p.b1);
    try testing.expectEqual(@as(u32, 2), st.queued_messages);
    try testing.expectEqual(@as(u32, 0), st.reserved_messages);
    try recvDrop(rig.x, p.b, p.b1);
    try sendOne(rig.x, p.a, p.a0, "c"); // recovered
    try rig.consistent();
}

test "RES-02 queue exhaustion by bytes" {
    var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 8, .max_queue_bytes = 4096 } });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try c.pair(&rig);
    var payload: [1000]u8 = undefined;
    @memset(&payload, 9);
    try sendOne(rig.x, p.a, p.a0, &payload);
    try sendOne(rig.x, p.a, p.a0, &payload);
    try sendOne(rig.x, p.a, p.a0, &payload);
    try testing.expectError(error.WouldBlock, sendOne(rig.x, p.a, p.a0, &payload));
    try sendOne(rig.x, p.a, p.a0, "small");
    try testing.expectEqual(@as(u64, 3 * 1032 + 37), (try rig.x.endpointStats(p.b, p.b1)).queued_bytes);
    try rig.consistent();
}

test "RES-03 sender quotas: queued bytes and channels; state unchanged on failure; recovery" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    var budget = c.space_budget;
    budget.queued_bytes = 80;
    budget.channels = 3;
    const a = try rig.k.createSpace(budget);
    const b = try rig.space();
    const e = try rig.x.endpointCreate(a); // 2 of 3 channels
    try testing.expectError(error.QuotaExceeded, rig.x.endpointCreate(a));
    try testing.expectEqual(@as(u64, 2), try rig.k.spaceUsed(a, .channels));
    const b1 = try rig.k.transfer(a, e[1], b);
    try sendOne(rig.x, a, e[0], "12345678"); // 40
    try sendOne(rig.x, a, e[0], "12345678"); // 80
    try testing.expectError(error.QuotaExceeded, sendOne(rig.x, a, e[0], "1"));
    try testing.expectEqual(@as(u64, 80), try rig.k.spaceUsed(a, .queued_bytes));
    try testing.expectEqual(@as(u32, 2), (try rig.x.endpointStats(b, b1)).queued_messages);
    try recvDrop(rig.x, b, b1);
    try sendOne(rig.x, a, e[0], "12345678");
    try rig.consistent();
}

test "RES-04 receiver handle quota exhaustion through the syscall layer: -6, message kept, retry works" {
    var env: c.Env = undefined;
    try env.init(.{});
    defer env.deinit() catch @panic("leak");
    const k = env.rig.k;
    var budget = c.space_budget;
    budget.handles = 4;
    const tight = try k.createSpace(budget);
    const e = try env.rig.x.endpointCreate(env.p.a);
    const tb1 = try k.transfer(env.p.a, e[1], tight); // 1 used
    const dummy = try k.createObject(tight, .memory_object, c.rights(c.R_READ), .{}); // 2 used, 2 free
    var ds: [3]ipc4.Desc = undefined;
    for (&ds) |*d| {
        const o = try k.createObject(env.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        d.* = c.moveDesc(o, .memory_object, c.R_READ | c.R_XFER);
    }
    var buf: [64]u8 = undefined;
    for (ds, 0..) |d, i| env.ua.putDesc(i, d);
    env.ua.prepSend(c.msg(&buf, 1, "three", 3), 3, 0, 0);
    try testing.expectEqual(@as(i64, 0), env.ua.call(&env.rig, env.p.a, .send, .{ @intFromEnum(e[0]), c.User.A_ARGS, 0, 0, 0, 0 }));
    env.ub.prepRecv(0, 0);
    const call = [6]u64{ @intFromEnum(tb1), c.User.A_ARGS, 0, 0, 0, 0 };
    try testing.expectEqual(@as(i64, -6), env.ub.call(&env.rig, tight, .recv, call));
    try testing.expectEqual(@as(u32, 2), @as(u32, @intCast(try k.spaceHandleCount(tight)))); // nothing leaked in
    try testing.expectEqual(@as(u32, 1), (try env.rig.x.endpointStats(tight, tb1)).queued_messages);
    try k.release(tight, dummy);
    try testing.expectEqual(@as(i64, 0), env.ub.call(&env.rig, tight, .recv, call));
    try testing.expectEqual(@as(u32, 3), env.ub.resCount());
    try env.rig.consistent();
}

test "RES-05 accounting consistency: bytes, handles and counters match a model through random traffic (seeds 1..24)" {
    var seed: u64 = 1;
    while (seed <= 24) : (seed += 1) {
        errdefer std.debug.print("RES-05 failure seed={d}\n", .{seed});
        var prng = std.Random.DefaultPrng.init(seed);
        const rnd = prng.random();
        var rig = try c.Rig.init(testing.allocator, .{ .ipc = .{ .max_messages = 4, .max_queue_bytes = 4096 } });
        const p = try c.pair(&rig);
        const base_a = try rig.k.spaceHandleCount(p.a);
        const base_b = try rig.k.spaceHandleCount(p.b);
        var model_bytes: u64 = 0;
        var model_msgs: u32 = 0;
        var q = std.ArrayListUnmanaged(struct { len: u32, caps: u8 }).empty;
        defer q.deinit(testing.allocator);
        var step: usize = 0;
        while (step < 150) : (step += 1) {
            if (rnd.boolean()) {
                const len = rnd.uintLessThan(u32, 900);
                var payload: [900]u8 = undefined;
                @memset(payload[0..len], 1);
                const with_cap = rnd.boolean();
                var buf: [4096]u8 = undefined;
                var d: [1]ipc4.Desc = undefined;
                var obj: c.Handle = .invalid;
                if (with_cap) {
                    obj = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
                    d[0] = c.moveDesc(obj, .memory_object, c.R_READ | c.R_XFER);
                }
                const wire = c.msg(&buf, 1, payload[0..len], @intFromBool(with_cap));
                const fits = model_msgs < 4 and model_bytes + wire.len <= 4096;
                if (rig.x.send(p.a, p.a0, wire, d[0..@intFromBool(with_cap)], .{}, null)) |_| {
                    try testing.expect(fits);
                    model_msgs += 1;
                    model_bytes += wire.len;
                    try q.append(testing.allocator, .{ .len = @intCast(wire.len), .caps = @intFromBool(with_cap) });
                } else |e| {
                    try testing.expectEqual(error.WouldBlock, e);
                    try testing.expect(!fits);
                    if (with_cap) try rig.k.release(p.a, obj);
                }
            } else {
                var out: ipc4.Received = undefined;
                if (rig.x.recv(p.b, p.b1, .{}, &out)) |_| {
                    const it = q.orderedRemove(0);
                    model_msgs -= 1;
                    model_bytes -= it.len;
                    for (out.slots[0..out.count]) |s| try rig.k.release(p.b, s.handle);
                    rig.x.deinitReceived(&out);
                } else |e| {
                    try testing.expectEqual(error.WouldBlock, e);
                    try testing.expectEqual(@as(u32, 0), model_msgs);
                }
            }
            const st = try rig.x.endpointStats(p.b, p.b1);
            try testing.expectEqual(model_msgs, st.queued_messages);
            try testing.expectEqual(model_bytes, st.queued_bytes);
            try testing.expectEqual(model_bytes, try rig.k.spaceUsed(p.a, .queued_bytes)); // charged to the sender
            try rig.consistent();
        }
        while (model_msgs > 0) : (model_msgs -= 1) {
            var out: ipc4.Received = undefined;
            try rig.x.recv(p.b, p.b1, .{}, &out);
            for (out.slots[0..out.count]) |s| try rig.k.release(p.b, s.handle);
            rig.x.deinitReceived(&out);
        }
        try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
        try testing.expectEqual(base_a, try rig.k.spaceHandleCount(p.a));
        try testing.expectEqual(base_b, try rig.k.spaceHandleCount(p.b));
        const cn = rig.x.ledger.snapshot();
        try testing.expectEqual(cn.sends_ok, cn.recvs_ok);
        try rig.consistent();
        try testing.expectEqual(@as(usize, 0), rig.deinit());
    }
}

test "RES-06 repeated creation and destruction (400 cycles) leaves no residue" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const a = try rig.space();
    const base = try rig.k.spaceHandleCount(a);
    var i: usize = 0;
    while (i < 400) : (i += 1) {
        const e = try rig.x.endpointCreate(a);
        const m = try rig.k.createObject(a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        var buf: [64]u8 = undefined;
        try rig.x.send(a, e[0], c.msg(&buf, 1, "r", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
        switch (i % 4) {
            0 => {
                var out: ipc4.Received = undefined;
                try rig.x.recv(a, e[1], .{}, &out);
                try rig.k.release(a, out.slots[0].handle);
                rig.x.deinitReceived(&out);
                try rig.x.releaseHandle(a, e[0]);
                try rig.x.releaseHandle(a, e[1]);
            },
            1 => {
                try rig.x.endpointClose(a, e[1]);
                try rig.x.releaseHandle(a, e[0]);
                try rig.x.releaseHandle(a, e[1]);
            },
            2 => {
                try rig.x.releaseHandle(a, e[1]);
                try rig.x.releaseHandle(a, e[0]);
            },
            else => {
                try rig.x.releaseHandle(a, e[0]);
                try rig.x.releaseHandle(a, e[1]);
            },
        }
    }
    try testing.expectEqual(@as(usize, 0), rig.x.ipc.registered());
    try testing.expectEqual(base, try rig.k.spaceHandleCount(a));
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(a, .channels));
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(a, .queued_bytes));
    try rig.consistent();
}

test "RES-07 endpoint destruction releases queued messages and in-flight capabilities exactly once" {
    var env: c.Env = undefined;
    try env.init(.{});
    defer env.deinit() catch @panic("leak");
    const k = env.rig.k;
    var tr = c.Tracker{};
    var ids: [3]c.k3.ObjectId = undefined;
    var buf: [64]u8 = undefined;
    for (&ids) |*id| {
        const m = try k.createObject(env.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
        id.* = try c.idOf(k, env.p.a, m, .memory_object);
        try env.rig.x.send(env.p.a, env.p.a0, c.msg(&buf, 1, "d", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    }
    try testing.expect(try k.spaceUsed(env.p.a, .queued_bytes) > 0);
    // the receiving side releases its only handle: the end is destroyed with 3 queued messages
    try testing.expectEqual(@as(i64, 0), env.ub.call(&env.rig, env.p.b, .handle_release, .{ @intFromEnum(env.p.b1), 0, 0, 0, 0, 0 }));
    for (ids) |id| try testing.expectEqual(@as(u8, 1), tr.count(id));
    try testing.expectEqual(@as(u64, 0), try k.spaceUsed(env.p.a, .queued_bytes));
    // the survivor can no longer send or receive
    try testing.expectEqual(@as(i64, -10), env.send(c.msg(&buf, 1, "x", 0), &.{}, 0, 0));
    var out: ipc4.Received = undefined;
    try testing.expectError(error.PeerDead, env.rig.x.recv(env.p.a, env.p.a0, .{}, &out));
    try env.rig.consistent();
}

test "RES-08 kernel teardown with queued messages and in-flight capabilities: no leak, exactly once" {
    var rig = try c.Rig.init(testing.allocator, .{});
    const p = try c.pair(&rig);
    var tr = c.Tracker{};
    var ids: [4]c.k3.ObjectId = undefined;
    var buf: [64]u8 = undefined;
    for (&ids) |*id| {
        const m = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
        id.* = try c.idOf(rig.k, p.a, m, .memory_object);
        try rig.x.send(p.a, p.a0, c.msg(&buf, 1, "t", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    }
    try testing.expectEqual(@as(usize, 0), rig.deinit()); // testing.allocator reports leaks
    for (ids) |id| try testing.expectEqual(@as(u8, 1), tr.count(id));
}

fn expectOwned(rig: *c.Rig, sp: c.SpaceId, u: *c.User, i: usize, otype: c.k3.ObjectType) !void {
    // after a failed send the user's descriptor holds the handle the capability now has
    const h = c.Handle.fromRaw(u.getDescHandle(i));
    _ = try rig.k.lookup(sp, h, otype, c.Rights.none);
}

test "RES-09 kernel allocation failure at every point of a syscall send: -6, nothing published, no authority lost" {
    var failures: u32 = 0;
    var successes: u32 = 0;
    var idx: usize = 0;
    while (idx < 10 * 24) : (idx += 1) {
        const warm = idx / 24;
        const n = idx % 24;
        var fa = std.testing.FailingAllocator.init(testing.allocator, .{});
        var env: c.Env = undefined;
        try env.initWith(fa.allocator(), testing.allocator, .{});
        var wi: usize = 0;
        while (wi < warm) : (wi += 1) {
            _ = try env.rig.k.createObject(env.p.a, .memory_object, c.rights(c.R_READ), .{});
            _ = try env.rig.k.createObject(env.p.b, .memory_object, c.rights(c.R_READ), .{});
        }
        const m1 = try env.rig.k.createObject(env.p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER), .{});
        const m2 = try env.rig.k.createObject(env.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        const descs = [_]ipc4.Desc{ c.copyDesc(m1, .memory_object, c.R_READ | c.R_XFER), c.moveDesc(m2, .memory_object, c.R_READ | c.R_XFER) };
        var buf: [64]u8 = undefined;
        fa.fail_index = fa.alloc_index + n;
        const r = env.send(c.msg(&buf, 1, "inject", 2), &descs, 0, 0);
        fa.fail_index = std.math.maxInt(usize);
        if (r == 0) {
            successes += 1;
            try testing.expectEqual(@as(i64, 0), env.recv(0, 0));
            try testing.expectEqual(@as(u32, 2), env.ub.resCount());
        } else {
            failures += 1;
            try testing.expectEqual(@as(i64, -6), r);
            try testing.expectEqual(@as(u32, 0), try env.queued());
            try testing.expectEqual(@as(u64, 0), try env.rig.k.spaceUsed(env.p.a, .queued_bytes));
            try expectOwned(&env.rig, env.p.a, &env.ua, 0, .memory_object);
            try expectOwned(&env.rig, env.p.a, &env.ua, 1, .memory_object);
            // and the layer still works
            var b2buf: [64]u8 = undefined;
            try testing.expectEqual(@as(i64, 0), env.send(c.msg(&b2buf, 1, "ok", 0), &.{}, 0, 0));
            try testing.expectEqual(@as(i64, 0), env.recv(0, 0));
        }
        try env.rig.consistent();
        try env.deinit();
    }
    try testing.expect(failures > 5);
    try testing.expect(successes > 5);
}

test "RES-10 kernel allocation failure at every point of a syscall recv: the message is never lost" {
    var failures: u32 = 0;
    var idx: usize = 0;
    while (idx < 10 * 24) : (idx += 1) {
        const warm = idx / 24;
        const n = idx % 24;
        var fa = std.testing.FailingAllocator.init(testing.allocator, .{});
        var env: c.Env = undefined;
        try env.initWith(fa.allocator(), testing.allocator, .{});
        var wi: usize = 0;
        while (wi < warm) : (wi += 1) _ = try env.rig.k.createObject(env.p.b, .memory_object, c.rights(c.R_READ), .{});
        const m1 = try env.rig.k.createObject(env.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        const m2 = try env.rig.k.createObject(env.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        var buf: [64]u8 = undefined;
        try testing.expectEqual(@as(i64, 0), env.send(c.msg(&buf, 1, "rx", 2), &.{ c.moveDesc(m1, .memory_object, c.R_READ | c.R_XFER), c.moveDesc(m2, .memory_object, c.R_READ | c.R_XFER) }, 0, 0));
        const before = try env.rig.k.spaceHandleCount(env.p.b);
        fa.fail_index = fa.alloc_index + n;
        const r = env.recv(0, 0);
        fa.fail_index = std.math.maxInt(usize);
        if (r == 0) {
            try testing.expectEqual(@as(u32, 2), env.ub.resCount());
        } else {
            failures += 1;
            try testing.expectEqual(@as(i64, -6), r);
            try testing.expectEqual(@as(u32, 1), try env.queued()); // still queued
            try testing.expectEqual(before, try env.rig.k.spaceHandleCount(env.p.b)); // nothing leaked in
            try testing.expectEqual(@as(i64, 0), env.recv(0, 0)); // retry delivers it whole
            try testing.expectEqual(@as(u32, 2), env.ub.resCount());
        }
        try env.rig.consistent();
        try env.deinit();
    }
    try testing.expect(failures > 0);
}

test "RES-11 allocation failure in the IPC and wait layers (send, recv, blocking recv): -6 or the normal result, never corruption" {
    var failures: u32 = 0;
    var n: usize = 0;
    while (n < 48) : (n += 1) {
        var fa = std.testing.FailingAllocator.init(testing.allocator, .{});
        var env: c.Env = undefined;
        try env.initWith(testing.allocator, fa.allocator(), .{});
        var buf: [64]u8 = undefined;
        fa.fail_index = fa.alloc_index + n % 6;
        const r1 = env.send(c.msg(&buf, 1, "a", 0), &.{}, 0, 0);
        fa.fail_index = std.math.maxInt(usize);
        if (r1 != 0) {
            failures += 1;
            try testing.expectEqual(@as(i64, -6), r1);
            try testing.expectEqual(@as(u64, 0), try env.rig.k.spaceUsed(env.p.a, .queued_bytes));
            try testing.expectEqual(@as(i64, 0), env.send(c.msg(&buf, 1, "a", 0), &.{}, 0, 0));
        }
        fa.fail_index = fa.alloc_index + n % 4;
        const r2 = env.recv(0, 0);
        fa.fail_index = std.math.maxInt(usize);
        if (r2 != 0) {
            failures += 1;
            try testing.expectEqual(@as(i64, -6), r2);
            try testing.expectEqual(@as(u32, 1), try env.queued());
            try testing.expectEqual(@as(i64, 0), env.recv(0, 0));
        }
        // a blocking receive on an empty queue needs a ledger entry
        fa.fail_index = fa.alloc_index;
        const r3 = env.recv(2 * c.ms, 0);
        fa.fail_index = std.math.maxInt(usize);
        try testing.expect(r3 == -6 or r3 == -8);
        try testing.expectEqual(@as(u32, 0), env.rig.x.ledger.waitersTotal());
        try env.rig.consistent();
        try env.deinit();
    }
    try testing.expect(failures > 3);
}

test "RES-12 failure recovery: after every class of failure the endpoints keep working" {
    var env: c.Env = undefined;
    try env.init(.{ .ipc = .{ .max_messages = 2 } });
    defer env.deinit() catch @panic("leak");
    var buf: [64]u8 = undefined;
    const wire = c.msg(&buf, 1, "alive", 0);
    // would-block, timeout, bad argument, fault, access denied, cancelled, stale
    try testing.expectEqual(@as(i64, 0), env.send(wire, &.{}, 0, 0));
    try testing.expectEqual(@as(i64, 0), env.send(wire, &.{}, 0, 0));
    try testing.expectEqual(@as(i64, -7), env.send(wire, &.{}, 0, 0));
    try testing.expectEqual(@as(i64, -8), env.send(wire, &.{}, 10 * c.ms, 0));
    try testing.expectEqual(@as(i64, -1), env.send(wire, &.{}, 40 * std.time.ns_per_s, 0));
    try testing.expectEqual(@as(i64, -32), env.ua.call(&env.rig, env.p.a, .send, .{ @intFromEnum(env.p.a0), 0x9000_0000, 0, 0, 0, 0 }));
    const ro = try env.rig.k.derive(env.p.a, env.p.a0, c.rights(c.R_READ));
    env.ua.prepSend(wire, 0, 0, 0);
    try testing.expectEqual(@as(i64, -3), env.ua.call(&env.rig, env.p.a, .send, .{ @intFromEnum(ro), c.User.A_ARGS, 0, 0, 0, 0 }));
    const cx = try env.rig.x.cancelCreate(env.p.a);
    try env.rig.x.cancelSignal(env.p.a, cx);
    try testing.expectEqual(@as(i64, -9), env.send(wire, &.{}, 5 * c.ms, @intFromEnum(cx)));
    try env.rig.k.release(env.p.a, ro);
    try testing.expectEqual(@as(i64, -2), env.ua.call(&env.rig, env.p.a, .send, .{ @intFromEnum(ro), c.User.A_ARGS, 0, 0, 0, 0 }));
    // still exactly the two accepted messages, in order, then room again
    try testing.expectEqual(@as(i64, 0), env.recv(0, 0));
    try testing.expectEqual(@as(i64, 0), env.recv(0, 0));
    try testing.expectEqual(@as(i64, -7), env.recv(0, 0));
    try testing.expectEqual(@as(i64, 0), env.send(wire, &.{}, 0, 0));
    try env.rig.consistent();
}
