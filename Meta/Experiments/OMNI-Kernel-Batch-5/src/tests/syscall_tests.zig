//! Syscall boundary tests: authorization, argument validation, overflow, boundaries,
//! user-memory faults and failure recovery.
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const sys = c.sys;
const Nr = sys.Nr;

const max_msg: usize = 4096;

const S = c.Env;

const St = enum(i64) {
    ok = 0,
    invalid_argument = -1,
    invalid_handle = -2,
    access_denied = -3,
    quota_exceeded = -6,
    would_block = -7,
    timed_out = -8,
    cancelled = -9,
    peer_dead = -10,
    fault = -32,
    no_sys = -33,
};

fn expectSt(want: St, got: i64) !void {
    if (@intFromEnum(want) != got) {
        std.debug.print("expected status {s} ({d}), got {d}\n", .{ @tagName(want), @intFromEnum(want), got });
        return error.WrongStatus;
    }
}

test "SYS-01 endpoint_create, send and recv through the syscall boundary" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    // endpoint_create writes both handles to user memory
    try expectSt(.ok, s.ua.call(&s.rig, s.p.a, .endpoint_create, .{ c.User.A_MISC, 0, 0, 0, 0, 0 }));
    const h0 = std.mem.readInt(u32, s.ua.misc[0..4], .little);
    const h1 = std.mem.readInt(u32, s.ua.misc[4..8], .little);
    try testing.expect(h0 != 0 and h1 != 0 and h0 != h1);
    // use the pre-built pair for the exchange
    var buf: [64]u8 = undefined;
    try expectSt(.ok, s.send(c.msg(&buf, 7, "hello", 0), &.{}, 0, 0));
    try expectSt(.ok, s.recv(0, 0));
    try testing.expectEqual(@as(u32, 5), s.ub.resPayloadLen());
    try testing.expectEqualStrings("hello", s.ub.rbuf[0..5]);
    try testing.expectEqual(@as(u16, 7), std.mem.readInt(u16, s.ub.rres[4..6], .little));
    try testing.expectEqual(@as(u32, 0), s.ub.resCount());
    try expectSt(.would_block, s.recv(0, 0));
    try s.rig.consistent();
}

test "SYS-02 capability authorization is checked on every operation" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    const k = s.rig.k;
    var buf: [64]u8 = undefined;
    const wire = c.msg(&buf, 1, "x", 0);
    // read-only and write-only views of the send end (space A)
    const ro = try k.derive(s.p.a, s.p.a0, c.rights(c.R_READ));
    const wo = try k.derive(s.p.a, s.p.a0, c.rights(c.R_WRITE));
    const noadmin = try k.derive(s.p.a, s.p.a0, c.rights(c.R_WRITE | c.R_READ));
    s.ua.prepSend(wire, 0, 0, 0);
    try expectSt(.access_denied, s.ua.call(&s.rig, s.p.a, .send, .{ @intFromEnum(ro), c.User.A_ARGS, 0, 0, 0, 0 }));
    try expectSt(.ok, s.ua.call(&s.rig, s.p.a, .send, .{ @intFromEnum(wo), c.User.A_ARGS, 0, 0, 0, 0 }));
    s.ub.prepRecv(0, 0);
    const b_wo = try k.derive(s.p.b, s.p.b1, c.rights(c.R_WRITE));
    try expectSt(.access_denied, s.ub.call(&s.rig, s.p.b, .recv, .{ @intFromEnum(b_wo), c.User.A_ARGS, 0, 0, 0, 0 }));
    try expectSt(.access_denied, s.ua.call(&s.rig, s.p.a, .endpoint_close, .{ @intFromEnum(noadmin), 0, 0, 0, 0, 0 }));
    try expectSt(.access_denied, s.ua.call(&s.rig, s.p.a, .endpoint_stats, .{ @intFromEnum(noadmin), c.User.A_MISC, 0, 0, 0, 0 }));
    // cancellation object: WAIT to wait on it, SIGNAL to signal it
    const cx = try s.rig.x.cancelCreate(s.p.a);
    const sig_only = try k.derive(s.p.a, cx, c.rights(c.R_SIGNAL));
    const wait_only = try k.derive(s.p.a, cx, c.rights(c.R_WAIT));
    try expectSt(.access_denied, s.ua.call(&s.rig, s.p.a, .cancel_signal, .{ @intFromEnum(wait_only), 0, 0, 0, 0, 0 }));
    try expectSt(.ok, s.ua.call(&s.rig, s.p.a, .cancel_signal, .{ @intFromEnum(sig_only), 0, 0, 0, 0, 0 }));
    s.ua.prepSend(wire, 0, 0, @intFromEnum(sig_only)); // SIGNAL does not allow waiting on it
    try expectSt(.access_denied, s.ua.call(&s.rig, s.p.a, .send, .{ @intFromEnum(s.p.a0), c.User.A_ARGS, 0, 0, 0, 0 }));
    s.ua.prepSend(wire, 0, 0, @intFromEnum(wait_only)); // already signalled: cancelled, nothing sent
    try expectSt(.cancelled, s.ua.call(&s.rig, s.p.a, .send, .{ @intFromEnum(s.p.a0), c.User.A_ARGS, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(u32, 1), try s.queued());
    try s.rig.consistent();
}

test "SYS-03 invalid endpoint access: zero, stale, foreign, wrong type, other space's handle" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    const k = s.rig.k;
    var buf: [64]u8 = undefined;
    const wire = c.msg(&buf, 1, "x", 0);
    s.ua.prepSend(wire, 0, 0, 0);
    const A = c.User.A_ARGS;
    try expectSt(.invalid_handle, s.ua.call(&s.rig, s.p.a, .send, .{ 0, A, 0, 0, 0, 0 }));
    // a memory object is not an endpoint
    const mo = try k.createObject(s.p.a, .memory_object, c.rights(c.R_ALL), .{});
    try expectSt(.invalid_handle, s.ua.call(&s.rig, s.p.a, .send, .{ @intFromEnum(mo), A, 0, 0, 0, 0 }));
    // stale
    const d = try k.derive(s.p.a, s.p.a0, c.rights(c.R_WRITE));
    try k.release(s.p.a, d);
    try expectSt(.invalid_handle, s.ua.call(&s.rig, s.p.a, .send, .{ @intFromEnum(d), A, 0, 0, 0, 0 }));
    // a handle that exists only in the sender's space names nothing in the receiver's
    // (handle values are per space: a0 itself would coincide with b1's value)
    const only_a = try k.derive(s.p.a, s.p.a0, c.rights(c.R_WRITE));
    s.ub.prepSend(wire, 0, 0, 0);
    try expectSt(.invalid_handle, s.ub.call(&s.rig, s.p.b, .send, .{ @intFromEnum(only_a), A, 0, 0, 0, 0 }));
    // a closed/never-existing space id
    const gone = try s.rig.space();
    try s.rig.x.spaceClose(gone);
    try expectSt(.invalid_handle, s.ua.call(&s.rig, gone, .send, .{ 1, A, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(u32, 0), try s.queued());
}

test "SYS-04 unknown numbers, non-zero unused registers and wide handles are rejected" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    const z = [_]u64{0} ** 6;
    for ([_]u32{ 0, 1, 0xFF, 0xFF, 0x108, 0x1FF, 0x8000_0000, 0xFFFF_FFFF }) |nr| {
        try expectSt(.no_sys, s.ua.callRaw(&s.rig, s.p.a, nr, z));
    }
    // every call rejects a non-zero register it does not use
    const used = [_]struct { nr: Nr, n: usize }{
        .{ .nr = .endpoint_create, .n = 1 },
        .{ .nr = .endpoint_close, .n = 1 },
        .{ .nr = .send, .n = 2 },
        .{ .nr = .recv, .n = 2 },
        .{ .nr = .cancel_create, .n = 0 },
        .{ .nr = .cancel_signal, .n = 1 },
        .{ .nr = .endpoint_stats, .n = 2 },
        .{ .nr = .handle_release, .n = 1 },
    };
    for (used) |u| {
        var i = u.n;
        while (i < 6) : (i += 1) {
            var a = z;
            a[i] = 1;
            try expectSt(.invalid_argument, s.ua.call(&s.rig, s.p.a, u.nr, a));
        }
    }
    // a 64-bit register whose low 32 bits name a valid handle must not be truncated into it
    const wide = @as(u64, @intFromEnum(s.p.a0)) | (@as(u64, 1) << 32);
    try expectSt(.invalid_argument, s.ua.call(&s.rig, s.p.a, .endpoint_close, .{ wide, 0, 0, 0, 0, 0 }));
    try expectSt(.invalid_argument, s.ua.call(&s.rig, s.p.a, .handle_release, .{ wide, 0, 0, 0, 0, 0 }));
    try expectSt(.invalid_argument, s.ua.call(&s.rig, s.p.a, .cancel_signal, .{ std.math.maxInt(u64), 0, 0, 0, 0, 0 }));
    // nothing above changed anything
    _ = try s.rig.x.endpointStats(s.p.a, s.p.a0);
    try s.rig.consistent();
}

test "SYS-05 send argument validation table: invalid input is rejected without side effects" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    var buf: [64]u8 = undefined;
    const good = c.msg(&buf, 1, "ok", 0);
    const w = std.mem.writeInt;
    const ep: u64 = @intFromEnum(s.p.a0);
    const A = c.User.A_ARGS;

    const Case = struct { name: []const u8, field: usize, width: u8, value: u64, want: St };
    const cases = [_]Case{
        .{ .name = "flags set", .field = 36, .width = 4, .value = 1, .want = .invalid_argument },
        .{ .name = "reserved set", .field = 40, .width = 8, .value = 1, .want = .invalid_argument },
        .{ .name = "length below header", .field = 8, .width = 4, .value = 31, .want = .invalid_argument },
        .{ .name = "length zero", .field = 8, .width = 4, .value = 0, .want = .invalid_argument },
        .{ .name = "length above limit", .field = 8, .width = 4, .value = max_msg + 1, .want = .invalid_argument },
        .{ .name = "length u32 max", .field = 8, .width = 4, .value = std.math.maxInt(u32), .want = .invalid_argument },
        .{ .name = "nine descriptors", .field = 12, .width = 4, .value = 9, .want = .invalid_argument },
        .{ .name = "descriptor count u32 max", .field = 12, .width = 4, .value = std.math.maxInt(u32), .want = .invalid_argument },
        .{ .name = "count differs from header", .field = 12, .width = 4, .value = 1, .want = .invalid_argument },
        .{ .name = "null message pointer", .field = 0, .width = 8, .value = 0, .want = .fault },
        .{ .name = "unmapped message pointer", .field = 0, .width = 8, .value = 0x9000_0000, .want = .fault },
        .{ .name = "message pointer + length overflows", .field = 0, .width = 8, .value = std.math.maxInt(u64) - 3, .want = .fault },
        .{ .name = "timeout above policy", .field = 24, .width = 8, .value = 31 * std.time.ns_per_s, .want = .invalid_argument },
        .{ .name = "timeout u64 max - 1", .field = 24, .width = 8, .value = std.math.maxInt(u64) - 1, .want = .invalid_argument },
        .{ .name = "forever without cancel", .field = 24, .width = 8, .value = std.math.maxInt(u64), .want = .invalid_argument },
        .{ .name = "cancel handle that is nothing", .field = 32, .width = 4, .value = 0x7777_7777, .want = .invalid_handle },
    };
    for (cases) |cs| {
        s.ua.prepSend(good, 0, 0, 0);
        switch (cs.width) {
            4 => w(u32, s.ua.args[cs.field..][0..4], @intCast(cs.value), .little),
            else => w(u64, s.ua.args[cs.field..][0..8], cs.value, .little),
        }
        const got = s.ua.call(&s.rig, s.p.a, .send, .{ ep, A, 0, 0, 0, 0 });
        if (@intFromEnum(cs.want) != got) {
            std.debug.print("case '{s}': expected {d}, got {d}\n", .{ cs.name, @intFromEnum(cs.want), got });
            return error.WrongStatus;
        }
        try testing.expectEqual(@as(u32, 0), try s.queued());
        try testing.expectEqual(@as(u64, 0), try s.rig.k.spaceUsed(s.p.a, .queued_bytes));
    }
    // argument block itself: unmapped, null, overflowing address
    for ([_]u64{ 0, 0x9000_0000, std.math.maxInt(u64) - 7 }) |bad| {
        try expectSt(.fault, s.ua.call(&s.rig, s.p.a, .send, .{ ep, bad, 0, 0, 0, 0 }));
        try expectSt(.fault, s.ub.call(&s.rig, s.p.b, .recv, .{ @intFromEnum(s.p.b1), bad, 0, 0, 0, 0 }));
    }
    try s.rig.consistent();
}

test "SYS-06 descriptor validation: bad type, mode, rights, reserved bits, pointer overflow" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    const k = s.rig.k;
    const m = try k.createObject(s.p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    const one = c.msg(&buf, 1, "d", 1);
    const good = c.copyDesc(m, .memory_object, c.R_READ | c.R_XFER);
    const ep: u64 = @intFromEnum(s.p.a0);
    const A = c.User.A_ARGS;
    const w = std.mem.writeInt;

    const Mut = struct { name: []const u8, off: usize, width: u8, value: u64, want: St };
    const muts = [_]Mut{
        .{ .name = "otype 0", .off = 4, .width = 1, .value = 0, .want = .invalid_argument },
        .{ .name = "otype 99", .off = 4, .width = 1, .value = 99, .want = .invalid_argument },
        .{ .name = "mode 0", .off = 5, .width = 1, .value = 0, .want = .invalid_argument },
        .{ .name = "mode 3", .off = 5, .width = 1, .value = 3, .want = .invalid_argument },
        .{ .name = "reserved16", .off = 6, .width = 2, .value = 1, .want = .invalid_argument },
        .{ .name = "reserved rights bit", .off = 8, .width = 4, .value = 0x8000_0010, .want = .invalid_argument },
        .{ .name = "reserved32", .off = 12, .width = 4, .value = 1, .want = .invalid_argument },
        .{ .name = "rights escalation", .off = 8, .width = 4, .value = c.R_READ | c.R_WRITE | c.R_XFER, .want = .access_denied },
        .{ .name = "copy without TRANSFER", .off = 8, .width = 4, .value = c.R_READ, .want = .invalid_argument },
        .{ .name = "wrong claimed type", .off = 4, .width = 1, .value = @intFromEnum(c.k3.ObjectType.channel), .want = .invalid_handle },
        .{ .name = "stale handle", .off = 0, .width = 4, .value = 0x0001_0001, .want = .invalid_handle },
    };
    for (muts) |mu| {
        s.ua.putDesc(0, good);
        const o = mu.off;
        switch (mu.width) {
            1 => s.ua.descs[o] = @intCast(mu.value),
            2 => w(u16, s.ua.descs[o..][0..2], @intCast(mu.value), .little),
            else => w(u32, s.ua.descs[o..][0..4], @intCast(mu.value), .little),
        }
        s.ua.prepSend(one, 1, 0, 0);
        const got = s.ua.call(&s.rig, s.p.a, .send, .{ ep, A, 0, 0, 0, 0 });
        if (@intFromEnum(mu.want) != got) {
            std.debug.print("mutation '{s}': expected {d}, got {d}\n", .{ mu.name, @intFromEnum(mu.want), got });
            return error.WrongStatus;
        }
        try testing.expectEqual(@as(u32, 0), try s.queued());
        _ = try k.lookup(s.p.a, m, .memory_object, c.rights(c.R_DUP | c.R_XFER)); // sender intact
    }
    // descriptor array pointer: null, unmapped, overflow
    s.ua.putDesc(0, good);
    for ([_]u64{ 0, 0x9000_0000, std.math.maxInt(u64) - 7 }) |bad| {
        s.ua.prepSend(one, 1, 0, 0);
        w(u64, s.ua.args[16..24], bad, .little);
        try expectSt(.fault, s.ua.call(&s.rig, s.p.a, .send, .{ ep, A, 0, 0, 0, 0 }));
    }
    try s.rig.consistent();
}

test "SYS-07 boundary sizes: largest message, empty payload, eight descriptors" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    const k = s.rig.k;
    // largest legal message: 32-byte header + 4064 payload = 4096
    var big: [max_msg]u8 = undefined;
    var payload: [max_msg - 32]u8 = undefined;
    for (&payload, 0..) |*b, i| b.* = @truncate(i);
    const wire = c.msg(&big, 3, &payload, 0);
    try testing.expectEqual(max_msg, wire.len);
    try expectSt(.ok, s.send(wire, &.{}, 0, 0));
    try expectSt(.ok, s.recv(0, 0));
    try testing.expectEqual(@as(u32, max_msg - 32), s.ub.resPayloadLen());
    try testing.expectEqualSlices(u8, &payload, s.ub.rbuf[0 .. max_msg - 32]);
    // empty payload
    var small: [64]u8 = undefined;
    try expectSt(.ok, s.send(c.msg(&small, 4, "", 0), &.{}, 0, 0));
    try expectSt(.ok, s.recv(0, 0));
    try testing.expectEqual(@as(u32, 0), s.ub.resPayloadLen());
    // eight capabilities
    var descs: [8]c.ipc4.Desc = undefined;
    for (&descs) |*d| {
        const o = try k.createObject(s.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        d.* = c.moveDesc(o, .memory_object, c.R_READ | c.R_XFER);
    }
    try expectSt(.ok, s.send(c.msg(&small, 5, "8", 8), &descs, 0, 0));
    try expectSt(.ok, s.recv(0, 0));
    try testing.expectEqual(@as(u32, 8), s.ub.resCount());
    for (0..8) |i| {
        try testing.expectEqual(@as(u32, @intFromEnum(c.ipc4.SlotStatus.delivered)), s.ub.resSlotStatus(i));
        _ = try k.lookup(s.p.b, c.Handle.fromRaw(s.ub.resSlotHandle(i)), .memory_object, c.rights(c.R_XFER));
    }
    try s.rig.consistent();
}

test "SYS-08 recv argument validation and probing: a rejected receive never consumes the message" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    var buf: [64]u8 = undefined;
    try expectSt(.ok, s.send(c.msg(&buf, 1, "keep", 0), &.{}, 0, 0));
    const w = std.mem.writeInt;
    const ep: u64 = @intFromEnum(s.p.b1);
    const A = c.User.A_ARGS;
    const Case = struct { name: []const u8, field: usize, width: u8, value: u64, want: St };
    const cases = [_]Case{
        .{ .name = "buffer smaller than largest payload", .field = 8, .width = 4, .value = 4063, .want = .invalid_argument },
        .{ .name = "buffer capacity zero", .field = 8, .width = 4, .value = 0, .want = .invalid_argument },
        .{ .name = "flags", .field = 12, .width = 4, .value = 1, .want = .invalid_argument },
        .{ .name = "reserved32", .field = 36, .width = 4, .value = 1, .want = .invalid_argument },
        .{ .name = "reserved64", .field = 40, .width = 8, .value = 1, .want = .invalid_argument },
        .{ .name = "unmapped buffer", .field = 0, .width = 8, .value = 0x9000_0000, .want = .fault },
        .{ .name = "buffer overflowing address", .field = 0, .width = 8, .value = std.math.maxInt(u64) - 100, .want = .fault },
        .{ .name = "null result pointer", .field = 16, .width = 8, .value = 0, .want = .fault },
        .{ .name = "unmapped result pointer", .field = 16, .width = 8, .value = 0x9000_0000, .want = .fault },
        .{ .name = "timeout above policy", .field = 24, .width = 8, .value = 31 * std.time.ns_per_s, .want = .invalid_argument },
        .{ .name = "forever without cancel", .field = 24, .width = 8, .value = std.math.maxInt(u64), .want = .invalid_argument },
    };
    for (cases) |cs| {
        s.ub.prepRecv(0, 0);
        switch (cs.width) {
            4 => w(u32, s.ub.args[cs.field..][0..4], @intCast(cs.value), .little),
            else => w(u64, s.ub.args[cs.field..][0..8], cs.value, .little),
        }
        const got = s.ub.call(&s.rig, s.p.b, .recv, .{ ep, A, 0, 0, 0, 0 });
        if (@intFromEnum(cs.want) != got) {
            std.debug.print("case '{s}': expected {d}, got {d}\n", .{ cs.name, @intFromEnum(cs.want), got });
            return error.WrongStatus;
        }
        try testing.expectEqual(@as(u32, 1), try s.queued()); // still there
    }
    // read-only buffer is a fault, not a consumed message
    var ro: [4096]u8 = undefined;
    try s.ub.mem.map(0x50000, &ro, false);
    s.ub.prepRecv(0, 0);
    w(u64, s.ub.args[0..8], 0x50000, .little);
    try expectSt(.fault, s.ub.call(&s.rig, s.p.b, .recv, .{ ep, A, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(u32, 1), try s.queued());
    // and the message is still deliverable afterwards
    try expectSt(.ok, s.recv(0, 0));
    try testing.expectEqualStrings("keep", s.ub.rbuf[0..4]);
}

test "SYS-09 endpoint_stats and handle_release" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    var buf: [64]u8 = undefined;
    try expectSt(.ok, s.send(c.msg(&buf, 1, "12345678", 0), &.{}, 0, 0));
    try expectSt(.ok, s.ub.call(&s.rig, s.p.b, .endpoint_stats, .{ @intFromEnum(s.p.b1), c.User.A_MISC, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(u32, 1), std.mem.readInt(u32, s.ub.misc[0..4], .little));
    try testing.expectEqual(@as(u32, 0), std.mem.readInt(u32, s.ub.misc[4..8], .little));
    try testing.expectEqual(@as(u64, 40), std.mem.readInt(u64, s.ub.misc[8..16], .little));
    try expectSt(.ok, s.ua.call(&s.rig, s.p.a, .endpoint_close, .{ @intFromEnum(s.p.a0), 0, 0, 0, 0, 0 }));
    try expectSt(.ok, s.ub.call(&s.rig, s.p.b, .endpoint_stats, .{ @intFromEnum(s.p.b1), c.User.A_MISC, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(u32, 2), std.mem.readInt(u32, s.ub.misc[4..8], .little)); // peer closed
    try expectSt(.fault, s.ub.call(&s.rig, s.p.b, .endpoint_stats, .{ @intFromEnum(s.p.b1), 0x9000_0000, 0, 0, 0, 0 }));
    // releasing the last capability to an end destroys it
    try expectSt(.ok, s.ub.call(&s.rig, s.p.b, .handle_release, .{ @intFromEnum(s.p.b1), 0, 0, 0, 0, 0 }));
    try expectSt(.invalid_handle, s.ub.call(&s.rig, s.p.b, .handle_release, .{ @intFromEnum(s.p.b1), 0, 0, 0, 0, 0 }));
}

test "SYS-10 endpoint_create: user-memory faults leave nothing behind" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    const a = s.p.a;
    const before_h = try s.rig.k.spaceHandleCount(a);
    const before_c = try s.rig.k.spaceUsed(a, .channels);
    // unmapped, null and overflowing output pointers: rejected BEFORE anything is created
    for ([_]u64{ 0, 0x9000_0000, std.math.maxInt(u64) - 3 }) |bad| {
        try expectSt(.fault, s.ua.call(&s.rig, a, .endpoint_create, .{ bad, 0, 0, 0, 0, 0 }));
    }
    try testing.expectEqual(before_h, try s.rig.k.spaceHandleCount(a));
    // fault AFTER creation (the copy-out fails): the new endpoints are taken back
    s.ua.mem.fail_copy_out_at = s.ua.mem.copy_out_calls;
    try expectSt(.fault, s.ua.call(&s.rig, a, .endpoint_create, .{ c.User.A_MISC, 0, 0, 0, 0, 0 }));
    try testing.expectEqual(before_h, try s.rig.k.spaceHandleCount(a));
    try testing.expectEqual(before_c, try s.rig.k.spaceUsed(a, .channels));
    try testing.expectEqual(@as(usize, 2), s.rig.x.ipc.registered()); // only the pair's two ends
    try testing.expectEqual(@as(u64, 1), s.rig.x.ledger.snapshot().delivery_faults);
    try s.rig.consistent();
}

test "SYS-11 recv: a fault while copying out after the message was consumed releases its capabilities" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    const k = s.rig.k;
    var tr = c.Tracker{};
    const m = try k.createObject(s.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
    const id = try c.idOf(k, s.p.a, m, .memory_object);
    var buf: [64]u8 = undefined;
    try expectSt(.ok, s.send(c.msg(&buf, 1, "f", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, 0, 0));
    const before = try k.spaceHandleCount(s.p.b);
    s.ub.mem.fail_copy_out_at = s.ub.mem.copy_out_calls + 1; // payload copy ok, result copy fails
    try expectSt(.fault, s.recv(0, 0));
    try testing.expectEqual(before, try k.spaceHandleCount(s.p.b)); // the installed capability was released
    try testing.expectEqual(@as(u8, 1), tr.count(id)); // exactly once
    try testing.expectEqual(@as(u32, 0), try s.queued()); // consumed (documented)
    try testing.expectEqual(@as(u64, 1), s.rig.x.ledger.snapshot().delivery_faults);
    try testing.expectEqual(@as(u64, 0), try k.spaceUsed(s.p.a, .queued_bytes));
    try s.rig.consistent();
}

fn closeDestHook(ctx: ?*anyopaque, point: c.ipc4.HookPoint, index: usize) void {
    _ = index;
    const s: *S = @ptrCast(@alignCast(ctx.?));
    if (point == .send_before_commit) s.rig.x.ipc.closeEnd(s.p.b, s.p.b1) catch {};
}

test "SYS-12 failed send rolls a moved capability back and reports its new handle value to the caller" {
    var s: S = undefined;
    try s.init(.{});
    defer s.deinit() catch @panic("leak");
    const k = s.rig.k;
    const m = try k.createObject(s.p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    s.rig.x.ipc.hook = .{ .func = closeDestHook, .ctx = &s };
    var buf: [64]u8 = undefined;
    try expectSt(.peer_dead, s.send(c.msg(&buf, 1, "r", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, 0, 0));
    const now = s.ua.getDescHandle(0);
    try testing.expect(now != @intFromEnum(m)); // moved back under a new value
    _ = try k.lookup(s.p.a, c.Handle.fromRaw(now), .memory_object, c.rights(c.R_XFER)); // and it works
    try testing.expectError(error.InvalidHandle, k.lookup(s.p.a, m, .memory_object, c.Rights.none));
    try testing.expectEqual(@as(u64, 0), try k.spaceUsed(s.p.a, .queued_bytes));
    try s.rig.consistent();
}
