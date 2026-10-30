//! Capability transfer (areas 7-11).
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const ipc = c.ipc;

const Pair = struct {
    a: c.SpaceId,
    b: c.SpaceId,
    a0: c.Handle, // sender end, in A
    b1: c.Handle, // receiver end, in B
};

fn pair(rig: *c.Rig) !Pair {
    const a = try rig.space();
    const b = try rig.space();
    const e = try rig.ipc.createChannel(a);
    const b1 = try rig.k.transfer(a, e[1], b);
    return .{ .a = a, .b = b, .a0 = e[0], .b1 = b1 };
}

fn recvOne(rig: *c.Rig, sp: c.SpaceId, h: c.Handle) !ipc.Received {
    var out: ipc.Received = undefined;
    try rig.ipc.recv(sp, h, .{}, &out);
    return out;
}

test "CAP-01 move: sender loses the capability, receiver gets the same rights" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const m = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_WRITE | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "cap", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_WRITE | c.R_XFER)}, .{}, null);
    try testing.expectError(error.InvalidHandle, rig.k.lookup(p.a, m, .memory_object, c.Rights.none));
    var out = try recvOne(&rig, p.b, p.b1);
    defer out.deinit(rig.ipc);
    try testing.expectEqual(@as(u8, 1), out.count);
    try testing.expectEqual(ipc.SlotStatus.delivered, out.slots[0].status);
    const r = try rig.k.lookup(p.b, out.slots[0].handle, .memory_object, c.Rights.none);
    try testing.expectEqual(c.rights(c.R_READ | c.R_WRITE | c.R_XFER).toRaw(), r.rights.toRaw());
    try rig.consistent();
}

test "CAP-02 copy: sender keeps authority, receiver gets an attenuated subset" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const m = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_WRITE | c.R_DUP | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "cap", 1), &.{c.copyDesc(m, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    _ = try rig.k.lookup(p.a, m, .memory_object, c.rights(c.R_WRITE)); // sender intact
    var out = try recvOne(&rig, p.b, p.b1);
    defer out.deinit(rig.ipc);
    const r = try rig.k.lookup(p.b, out.slots[0].handle, .memory_object, c.Rights.none);
    try testing.expectEqual(c.rights(c.R_READ | c.R_XFER).toRaw(), r.rights.toRaw());
    try testing.expectError(error.AccessDenied, rig.k.lookup(p.b, out.slots[0].handle, .memory_object, c.rights(c.R_WRITE)));
}

test "CAP-03 escalation and malformed descriptors are rejected; sender unchanged" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const m = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER), .{});
    const noxfer = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP), .{});
    const nodup = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    const one = c.msg(&buf, 1, "x", 1);
    const s = &rig.ipc;
    // copy asks for WRITE the sender does not have
    try testing.expectError(error.AccessDenied, s.*.send(p.a, p.a0, one, &.{c.copyDesc(m, .memory_object, c.R_READ | c.R_WRITE | c.R_XFER)}, .{}, null));
    // move cannot attenuate
    try testing.expectError(error.InvalidArgument, s.*.send(p.a, p.a0, one, &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, .{}, null));
    // copy must keep TRANSFER (otherwise the receiver could never pass it on)
    try testing.expectError(error.InvalidArgument, s.*.send(p.a, p.a0, one, &.{c.copyDesc(m, .memory_object, c.R_READ)}, .{}, null));
    // reserved right bits
    try testing.expectError(error.InvalidArgument, s.*.send(p.a, p.a0, one, &.{.{ .handle = m, .otype = .memory_object, .mode = .copy, .rights = @bitCast(@as(u32, 0x8000_0010)) }}, .{}, null));
    // no TRANSFER right on the capability
    try testing.expectError(error.AccessDenied, s.*.send(p.a, p.a0, one, &.{c.moveDesc(noxfer, .memory_object, c.R_READ | c.R_DUP)}, .{}, null));
    // copy needs DUPLICATE
    try testing.expectError(error.AccessDenied, s.*.send(p.a, p.a0, one, &.{c.copyDesc(nodup, .memory_object, c.R_READ | c.R_XFER)}, .{}, null));
    // wrong claimed type
    try testing.expectError(error.InvalidHandle, s.*.send(p.a, p.a0, one, &.{c.copyDesc(m, .channel, c.R_READ | c.R_XFER)}, .{}, null));
    // the same handle twice
    const two = c.msg(&buf, 1, "x", 2);
    try testing.expectError(error.InvalidArgument, s.*.send(p.a, p.a0, two, &.{ c.copyDesc(m, .memory_object, c.R_READ | c.R_XFER), c.copyDesc(m, .memory_object, c.R_READ | c.R_XFER) }, .{}, null));
    // sending the destination end through itself
    const e = try rig.ipc.createChannel(p.a);
    try testing.expectError(error.InvalidArgument, s.*.send(p.a, e[0], one, &.{c.moveDesc(e[1], .channel, c.R_ALL & ~@as(u32, 4))}, .{}, null));
    // after all that: nothing queued, sender still owns everything
    const st = try rig.ipc.endStats(p.b, p.b1);
    try testing.expectEqual(@as(u32, 0), st.queued_messages);
    _ = try rig.k.lookup(p.a, m, .memory_object, c.rights(c.R_DUP | c.R_XFER));
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
    try rig.consistent();
}

test "CAP-04 stale handle in a descriptor is rejected" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const m = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    try rig.k.release(p.a, m);
    var buf: [64]u8 = undefined;
    try testing.expectError(error.InvalidHandle, rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "x", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, .{}, null));
    // a handle already moved away is stale too (no double send of one authority)
    const m2 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    const d = c.moveDesc(m2, .memory_object, c.R_READ | c.R_XFER);
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "x", 1), &.{d}, .{}, null);
    try testing.expectError(error.InvalidHandle, rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "x", 1), &.{d}, .{}, null));
}

test "CAP-05 slot exhaustion: a retired sender slot never makes an old handle valid" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const first = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    const idx = first.index();
    try rig.k.release(p.a, first);
    rig.k.testSetTableGen(p.a, idx, 0xFFFF); // next use of this slot is its last
    const last = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    try testing.expectEqual(idx, last.index());
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "x", 1), &.{c.moveDesc(last, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    try testing.expectError(error.InvalidHandle, rig.k.lookup(p.a, last, .memory_object, c.Rights.none));
    // the slot is retired; fresh objects use other slots and the dead value stays dead
    const next = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ), .{});
    try testing.expect(next.index() != idx);
    try testing.expectError(error.InvalidHandle, rig.k.lookup(p.a, last, .memory_object, c.Rights.none));
    var out = try recvOne(&rig, p.b, p.b1);
    out.deinit(rig.ipc);
    try rig.consistent();
}

test "CAP-06 receiver quota exhaustion: recv fails, message stays queued, retry succeeds" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const a = try rig.space();
    var budget = c.space_budget;
    budget.handles = 4;
    const b = try rig.k.createSpace(budget);
    const e = try rig.ipc.createChannel(a);
    const b1 = try rig.k.transfer(a, e[1], b); // 1 handle used in B
    const dummy = try rig.k.createObject(b, .memory_object, c.rights(c.R_READ), .{}); // 2 used, 2 free
    var objs: [3]c.Handle = undefined;
    var descs: [3]ipc.Desc = undefined;
    for (&objs, &descs) |*o, *d| {
        o.* = try rig.k.createObject(a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        d.* = c.moveDesc(o.*, .memory_object, c.R_READ | c.R_XFER);
    }
    var buf: [64]u8 = undefined;
    try rig.ipc.send(a, e[0], c.msg(&buf, 9, "three", 3), &descs, .{}, null);
    var out: ipc.Received = undefined;
    try testing.expectError(error.QuotaExceeded, rig.ipc.recv(b, b1, .{}, &out));
    var st = try rig.ipc.endStats(b, b1);
    try testing.expectEqual(@as(u32, 1), st.queued_messages);
    try testing.expectEqual(@as(u32, 2), try rig.k.spaceHandleCount(b)); // nothing leaked into B
    try rig.consistent();
    try rig.k.release(b, dummy);
    try rig.ipc.recv(b, b1, .{}, &out);
    defer out.deinit(rig.ipc);
    try testing.expect(!out.partial);
    try testing.expectEqual(@as(u8, 3), out.count);
    for (out.slots[0..3]) |s| try testing.expectEqual(ipc.SlotStatus.delivered, s.status);
    st = try rig.ipc.endStats(b, b1);
    try testing.expectEqual(@as(u32, 0), st.queued_messages);
}

test "CAP-07 capability moved across two hops keeps its rights" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const cspace = try rig.space();
    const e2 = try rig.ipc.createChannel(p.b);
    const c1 = try rig.k.transfer(p.b, e2[1], cspace);
    const m = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "h", 1), &.{c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    var out = try recvOne(&rig, p.b, p.b1);
    const hb = out.slots[0].handle;
    out.deinit(rig.ipc);
    try rig.ipc.send(p.b, e2[0], c.msg(&buf, 1, "h", 1), &.{c.moveDesc(hb, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    out = try recvOne(&rig, cspace, c1);
    defer out.deinit(rig.ipc);
    const r = try rig.k.lookup(cspace, out.slots[0].handle, .memory_object, c.Rights.none);
    try testing.expectEqual(c.rights(c.R_READ | c.R_XFER).toRaw(), r.rights.toRaw());
}

test "CAP-08 a capability sent over a channel can be a channel end" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const inner = try rig.ipc.createChannel(p.a);
    const full = c.R_ALL & ~@as(u32, 4);
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "e", 1), &.{c.moveDesc(inner[1], .channel, full)}, .{}, null);
    var out = try recvOne(&rig, p.b, p.b1);
    defer out.deinit(rig.ipc);
    const got = out.slots[0].handle;
    try rig.ipc.send(p.a, inner[0], c.msg(&buf, 5, "via", 0), &.{}, .{}, null);
    var o2 = try recvOne(&rig, p.b, got);
    defer o2.deinit(rig.ipc);
    try testing.expectEqualStrings("via", o2.payload);
}

// ---- revocation races -------------------------------------------------------

test "REV-01 revoked while queued: recv reports the slot as revoked, message still delivered" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const parent = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER | c.R_ADMIN), .{});
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "r", 1), &.{c.copyDesc(parent, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    _ = try rig.k.revoke(p.a, parent); // removes the in-flight copy
    var out = try recvOne(&rig, p.b, p.b1);
    defer out.deinit(rig.ipc);
    try testing.expectEqual(ipc.SlotStatus.revoked, out.slots[0].status);
    try testing.expectEqual(c.Handle.invalid, out.slots[0].handle);
    try testing.expectEqualStrings("r", out.payload);
    try testing.expectEqual(@as(u64, 1), rig.ipc.stats().revoked_in_flight);
    try rig.consistent();
}

test "REV-02 revoked after installation: the receiver handle dies like any other" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const parent = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER | c.R_ADMIN), .{});
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "r", 1), &.{c.copyDesc(parent, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    var out = try recvOne(&rig, p.b, p.b1);
    defer out.deinit(rig.ipc);
    const h = out.slots[0].handle;
    _ = try rig.k.lookup(p.b, h, .memory_object, c.Rights.none);
    _ = try rig.k.revoke(p.a, parent);
    try testing.expectError(error.InvalidHandle, rig.k.lookup(p.b, h, .memory_object, c.Rights.none));
}

const RevokeCtx = struct {
    rig: *c.Rig,
    space: c.SpaceId,
    victim: c.Handle,
    at: ipc.HookPoint,
    fired: bool = false,
};

fn revokeHook(ctx: ?*anyopaque, point: ipc.HookPoint, index: usize) void {
    _ = index;
    const rc: *RevokeCtx = @ptrCast(@alignCast(ctx.?));
    if (point == rc.at and !rc.fired) {
        rc.fired = true;
        _ = rc.rig.k.revoke(rc.space, rc.victim) catch {};
    }
}

test "REV-03 revoke during send (after staging, before commit): send succeeds, receiver sees revoked" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const parent = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER | c.R_ADMIN), .{});
    var rc = RevokeCtx{ .rig = &rig, .space = p.a, .victim = parent, .at = .send_before_commit };
    rig.ipc.hook = .{ .func = revokeHook, .ctx = &rc };
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "r", 1), &.{c.copyDesc(parent, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    try testing.expect(rc.fired);
    var out = try recvOne(&rig, p.b, p.b1);
    defer out.deinit(rig.ipc);
    try testing.expectEqual(ipc.SlotStatus.revoked, out.slots[0].status);
    try rig.consistent();
}

test "REV-04 revoke between claim and install: slot reported revoked, others delivered" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const parent = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER | c.R_ADMIN), .{});
    const other = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "r", 2), &.{ c.copyDesc(parent, .memory_object, c.R_READ | c.R_XFER), c.moveDesc(other, .memory_object, c.R_READ | c.R_XFER) }, .{}, null);
    var rc = RevokeCtx{ .rig = &rig, .space = p.a, .victim = parent, .at = .recv_claimed };
    rig.ipc.hook = .{ .func = revokeHook, .ctx = &rc };
    var out = try recvOne(&rig, p.b, p.b1);
    defer out.deinit(rig.ipc);
    try testing.expect(rc.fired);
    try testing.expectEqual(ipc.SlotStatus.revoked, out.slots[0].status);
    try testing.expectEqual(ipc.SlotStatus.delivered, out.slots[1].status);
    try rig.consistent();
}

// ---- rollback and allocation-failure injection --------------------------------

/// Occupy `n` slots in each space so that array growth lands on different operations.
fn warmUp(rig: *c.Rig, p: Pair, n: usize) !void {
    var i: usize = 0;
    while (i < n) : (i += 1) {
        _ = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ), .{});
        _ = try rig.k.createObject(p.b, .memory_object, c.rights(c.R_READ), .{});
    }
}

fn expectOwned(rig: *c.Rig, sp: c.SpaceId, d: ipc.Desc, report: ipc.SendReport) !void {
    if (rig.k.lookup(sp, d.handle, d.otype, c.Rights.none)) |_| return else |_| {}
    for (report.rebound[0..report.rebound_count]) |rb| {
        if (rb.old == d.handle) {
            _ = try rig.k.lookup(sp, rb.new, d.otype, c.Rights.none);
            return;
        }
    }
    return error.SenderLostAuthority;
}

test "ALLOC-01 kernel allocation failure at every point of send: no leak, no loss, no partial publication" {
    var failures: u32 = 0;
    var successes: u32 = 0;
    var idx: usize = 0;
    while (idx < 10 * 24) : (idx += 1) {
        const warm = idx / 24; // shifts where array growth falls
        const n = idx % 24;
        var fa = std.testing.FailingAllocator.init(testing.allocator, .{});
        var rig = try c.Rig.initWith(fa.allocator(), testing.allocator, .{});
        const p = try pair(&rig);
        try warmUp(&rig, p, warm);
        const m1 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_DUP | c.R_XFER), .{});
        const m2 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        const descs = [_]ipc.Desc{ c.copyDesc(m1, .memory_object, c.R_READ | c.R_XFER), c.moveDesc(m2, .memory_object, c.R_READ | c.R_XFER) };
        var buf: [64]u8 = undefined;
        const bytes = c.msg(&buf, 1, "inject", 2);
        fa.fail_index = fa.alloc_index + n;
        var report: ipc.SendReport = .{};
        const res = rig.ipc.send(p.a, p.a0, bytes, &descs, .{}, &report);
        fa.fail_index = std.math.maxInt(usize);
        if (res) |_| {
            successes += 1;
            var out = try recvOne(&rig, p.b, p.b1);
            try testing.expectEqual(@as(u8, 2), out.count);
            out.deinit(rig.ipc);
        } else |e| {
            failures += 1;
            try testing.expectEqual(error.QuotaExceeded, e);
            try testing.expectEqual(@as(u8, 0), report.lost_count);
            const st = try rig.ipc.endStats(p.b, p.b1);
            try testing.expectEqual(@as(u32, 0), st.queued_messages);
            try testing.expectEqual(@as(u32, 0), st.reserved_messages);
            try testing.expectEqual(@as(u64, 0), st.queued_bytes);
            try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
            for (descs) |d| try expectOwned(&rig, p.a, d, report);
            // the system still works afterwards
            try testing.expectError(error.InvalidArgument, rig.ipc.send(p.a, p.a0, bytes, &.{}, .{}, null));
            var buf2: [64]u8 = undefined;
            try rig.ipc.send(p.a, p.a0, c.msg(&buf2, 1, "ok", 0), &.{}, .{}, null);
            var out = try recvOne(&rig, p.b, p.b1);
            out.deinit(rig.ipc);
        }
        try rig.consistent();
        try testing.expectEqual(@as(usize, 0), rig.deinit());
    }
    try testing.expect(failures > 5);
    try testing.expect(successes > 5);
}

test "ALLOC-02 ipc allocation failure in send, recv and createChannel: no leak, consistent" {
    var n: usize = 0;
    var failures: u32 = 0;
    while (n < 40) : (n += 1) {
        var fa = std.testing.FailingAllocator.init(testing.allocator, .{});
        var rig = try c.Rig.initWith(testing.allocator, fa.allocator(), .{});
        const a = try rig.space();
        fa.fail_index = fa.alloc_index + n;
        const made = rig.ipc.createChannel(a);
        fa.fail_index = std.math.maxInt(usize);
        if (made) |e| {
            fa.fail_index = fa.alloc_index + n % 5;
            var buf: [64]u8 = undefined;
            const r = rig.ipc.send(a, e[0], c.msg(&buf, 1, "p", 0), &.{}, .{}, null);
            fa.fail_index = std.math.maxInt(usize);
            if (r) |_| {} else |er| {
                failures += 1;
                try testing.expectEqual(error.QuotaExceeded, er);
                try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(a, .queued_bytes));
                try rig.ipc.send(a, e[0], c.msg(&buf, 1, "p", 0), &.{}, .{}, null);
            }
            var out: ipc.Received = undefined;
            try rig.ipc.recv(a, e[1], .{}, &out);
            out.deinit(rig.ipc);
        } else |er| {
            failures += 1;
            try testing.expectEqual(error.QuotaExceeded, er);
            try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(a, .channels));
            try testing.expectEqual(@as(usize, 0), try rig.k.spaceHandleCount(a));
        }
        try rig.consistent();
        try testing.expectEqual(@as(usize, 0), rig.deinit());
    }
    try testing.expect(failures > 3);
}

test "ALLOC-03 kernel allocation failure at every point of recv: message not lost, no leak" {
    var failures: u32 = 0;
    var idx: usize = 0;
    while (idx < 10 * 24) : (idx += 1) {
        const warm = idx / 24;
        const n = idx % 24;
        var fa = std.testing.FailingAllocator.init(testing.allocator, .{});
        var rig = try c.Rig.initWith(fa.allocator(), testing.allocator, .{});
        const p = try pair(&rig);
        try warmUp(&rig, p, warm);
        const m1 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        const m2 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
        var buf: [64]u8 = undefined;
        try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "rx", 2), &.{ c.moveDesc(m1, .memory_object, c.R_READ | c.R_XFER), c.moveDesc(m2, .memory_object, c.R_READ | c.R_XFER) }, .{}, null);
        const before = try rig.k.spaceHandleCount(p.b);
        fa.fail_index = fa.alloc_index + n;
        var out: ipc.Received = undefined;
        const r = rig.ipc.recv(p.b, p.b1, .{}, &out);
        fa.fail_index = std.math.maxInt(usize);
        if (r) |_| {
            // delivered completely or (only if a rollback failed) partially; never silently short
            for (out.slots[0..2]) |s| try testing.expect(s.status != .none);
            if (!out.partial) for (out.slots[0..2]) |s| try testing.expectEqual(ipc.SlotStatus.delivered, s.status);
            out.deinit(rig.ipc);
        } else |e| {
            failures += 1;
            try testing.expectEqual(error.QuotaExceeded, e);
            const st = try rig.ipc.endStats(p.b, p.b1);
            try testing.expectEqual(@as(u32, 1), st.queued_messages);
            try testing.expectEqual(before, try rig.k.spaceHandleCount(p.b)); // nothing leaked into B
            // retry delivers it intact
            try rig.ipc.recv(p.b, p.b1, .{}, &out);
            try testing.expect(!out.partial);
            try testing.expectEqual(ipc.SlotStatus.delivered, out.slots[1].status);
            out.deinit(rig.ipc);
        }
        try rig.consistent();
        try testing.expectEqual(@as(usize, 0), rig.deinit());
    }
    try testing.expect(failures > 0);
}

const CloseCtx = struct {
    rig: *c.Rig,
    space: c.SpaceId,
    end: c.Handle,
    fired: bool = false,
};

fn closeHook(ctx: ?*anyopaque, point: ipc.HookPoint, index: usize) void {
    _ = index;
    const cc: *CloseCtx = @ptrCast(@alignCast(ctx.?));
    if (point == .send_before_commit and !cc.fired) {
        cc.fired = true;
        cc.rig.ipc.closeEnd(cc.space, cc.end) catch {};
    }
}

test "CLOSE-ROLLBACK peer closes during send: PeerDead, moved capabilities return to the sender" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    const m = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    var cc = CloseCtx{ .rig = &rig, .space = p.b, .end = p.b1 };
    rig.ipc.hook = .{ .func = closeHook, .ctx = &cc };
    var buf: [64]u8 = undefined;
    var report: ipc.SendReport = .{};
    const d = c.moveDesc(m, .memory_object, c.R_READ | c.R_XFER);
    try testing.expectError(error.PeerDead, rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "x", 1), &.{d}, .{}, &report));
    try testing.expect(cc.fired);
    try expectOwned(&rig, p.a, d, report);
    try testing.expectEqual(@as(u8, 1), report.rebound_count); // returned under a new handle value
    try testing.expect(report.rebound[0].old != report.rebound[0].new);
    try testing.expectEqual(@as(u8, 0), report.lost_count);
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
    try rig.consistent();
}

const SpaceCloseCtx = struct { rig: *c.Rig, space: c.SpaceId, fired: bool = false };

fn closeSpaceHook(ctx: ?*anyopaque, point: ipc.HookPoint, index: usize) void {
    const cc: *SpaceCloseCtx = @ptrCast(@alignCast(ctx.?));
    if (point == .recv_installed and index == 0 and !cc.fired) {
        cc.fired = true;
        cc.rig.k.closeSpace(cc.space) catch {};
    }
}

test "RECV-ZOMBIE receiver space closed (but not freed) mid-install: recv fails with PeerDead, every capability released once, no leak" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    var tr = c.Tracker{};
    const m1 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
    const m2 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
    const id1 = try c.idOf(rig.k, p.a, m1, .memory_object);
    const id2 = try c.idOf(rig.k, p.a, m2, .memory_object);
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "p", 2), &.{ c.moveDesc(m1, .memory_object, c.R_READ | c.R_XFER), c.moveDesc(m2, .memory_object, c.R_READ | c.R_XFER) }, .{}, null);
    // Keep the receiver space a closed-but-not-freed "zombie": an object it paid for lives on in A.
    const keep = try rig.k.createObject(p.b, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    _ = try rig.k.transfer(p.b, keep, p.a);
    var cc = SpaceCloseCtx{ .rig = &rig, .space = p.b };
    rig.ipc.hook = .{ .func = closeSpaceHook, .ctx = &cc };
    var out: ipc.Received = undefined;
    try testing.expectError(error.PeerDead, rig.ipc.recv(p.b, p.b1, .{}, &out));
    try testing.expect(cc.fired);
    try testing.expectEqual(@as(u64, 0), rig.ipc.stats().revoked_in_flight);
    try testing.expectEqual(@as(u8, 1), tr.count(id1)); // died with the receiver space
    try testing.expectEqual(@as(u8, 1), tr.count(id2)); // released from the hold space
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
    try rig.consistent();
}

test "RECV-DEAD-SPACE receiver space destroyed mid-install: recv fails with PeerDead, nothing is reported as revoked, no leak" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const p = try pair(&rig);
    var tr = c.Tracker{};
    const m1 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
    const m2 = try rig.k.createObject(p.a, .memory_object, c.rights(c.R_READ | c.R_XFER), tr.opts());
    const id1 = try c.idOf(rig.k, p.a, m1, .memory_object);
    const id2 = try c.idOf(rig.k, p.a, m2, .memory_object);
    var buf: [64]u8 = undefined;
    try rig.ipc.send(p.a, p.a0, c.msg(&buf, 1, "p", 2), &.{ c.moveDesc(m1, .memory_object, c.R_READ | c.R_XFER), c.moveDesc(m2, .memory_object, c.R_READ | c.R_XFER) }, .{}, null);
    var cc = SpaceCloseCtx{ .rig = &rig, .space = p.b }; // no zombie: the slot is freed at once
    rig.ipc.hook = .{ .func = closeSpaceHook, .ctx = &cc };
    var out: ipc.Received = undefined;
    try testing.expectError(error.PeerDead, rig.ipc.recv(p.b, p.b1, .{}, &out));
    try testing.expect(cc.fired);
    try testing.expectEqual(@as(u64, 0), rig.ipc.stats().revoked_in_flight);
    // the queued message died with the end (the end's only capability was in the dead space)
    try testing.expectEqual(@as(u8, 1), tr.count(id1));
    try testing.expectEqual(@as(u8, 1), tr.count(id2));
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(p.a, .queued_bytes));
    try rig.consistent();
}
