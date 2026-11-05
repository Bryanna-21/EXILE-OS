//! Capability lifecycle tests (LC-xx) and generation/handle-reuse tests (GEN-xx).

const std = @import("std");
const c = @import("common.zig");
const expect = std.testing.expect;
const expectEqual = std.testing.expectEqual;
const expectError = std.testing.expectError;

const full = c.R_ALL;

test "LC-01 create and lookup: success, type confusion, missing right, bad bits, zero and garbage handles" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const h = try k.createObject(s, .channel, c.rights(c.R_READ | c.R_WRITE), .{});
    try expect(h != .invalid);
    const got = try k.lookup(s, h, .channel, c.rights(c.R_READ));
    try expect(got.object.isValid());
    try expectEqual(c.ObjectType.channel, got.otype);
    try expectError(error.InvalidHandle, k.lookup(s, h, .memory_object, c.Rights.none));
    try expectError(error.AccessDenied, k.lookup(s, h, .channel, c.rights(c.R_EXEC)));
    const forged: c.Rights = @bitCast(@as(u32, 0x8000_0001));
    try expectError(error.InvalidArgument, k.lookup(s, h, .channel, forged));
    try expectError(error.InvalidArgument, k.createObject(s, .channel, forged, .{}));
    try expectError(error.InvalidHandle, k.lookup(s, .invalid, .channel, c.Rights.none));
    try expectError(error.InvalidHandle, k.lookup(s, c.Handle.fromRaw(0xFFFF_FFFF), .channel, c.Rights.none));
    try expectError(error.InvalidHandle, k.lookup(s, c.Handle.fromRaw(h.raw() +% 1), .channel, c.Rights.none));
    try c.expectConsistent(k);
}

test "LC-02 derive attenuates rights; escalation and missing duplicate are rejected; failures change nothing" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .memory_object, c.rights(c.R_READ | c.R_WRITE | c.R_DUP), .{});
    const child = try k.derive(s, root, c.rights(c.R_READ | c.R_DUP));
    const r = try k.lookup(s, child, .memory_object, c.Rights.none);
    try expectEqual(@as(u32, c.R_READ | c.R_DUP), r.rights.toRaw());
    try expectEqual(c.ObjectType.memory_object, r.otype);

    const before = k.stats();
    const used_before = try k.spaceUsed(s, .handles);
    try expectError(error.AccessDenied, k.derive(s, child, c.rights(c.R_READ | c.R_WRITE))); // escalation
    try expectError(error.AccessDenied, k.derive(s, child, c.rights(c.R_ADMIN)));
    const grandchild = try k.derive(s, child, c.rights(c.R_DUP));
    try expectError(error.AccessDenied, k.derive(s, grandchild, c.rights(c.R_READ))); // re-derivation cannot regain
    const nodup = try k.derive(s, grandchild, c.Rights.none);
    try expectError(error.AccessDenied, k.derive(s, nodup, c.Rights.none)); // no duplicate right
    const forged: c.Rights = @bitCast(@as(u32, 0x200));
    try expectError(error.InvalidArgument, k.derive(s, root, forged));
    try expectError(error.InvalidHandle, k.derive(s, .invalid, c.Rights.none));
    try expectEqual(before.nodes + 2, k.stats().nodes); // only the two successful derives
    try expectEqual(used_before + 2, try k.spaceUsed(s, .handles));
    try c.expectConsistent(k);
}

test "LC-03 release invalidates, double release fails, object survives until the last capability" {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    t.kernel_ptr = k;
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(c.R_DUP | c.R_READ), t.opts());
    const id = try c.idOf(k, s, root, .channel);
    const child = try k.derive(s, root, c.rights(c.R_READ));
    try k.release(s, root);
    try expectError(error.InvalidHandle, k.release(s, root));
    try expectError(error.InvalidHandle, k.lookup(s, root, .channel, c.Rights.none));
    try expectEqual(@as(u8, 0), t.count(id));
    try expectEqual(@as(usize, 1), k.stats().objects);
    _ = try k.lookup(s, child, .channel, c.rights(c.R_READ)); // derived cap survives its source's release
    try k.release(s, child);
    try expectEqual(@as(u8, 1), t.count(id));
    try expectEqual(@as(usize, 0), k.stats().objects);
    try expectEqual(@as(usize, 0), k.stats().nodes);
    try expect(!t.saw_lock_held);
    try c.expectConsistent(k);
}

test "LC-04 releasing a middle node re-parents its children so the ancestor can still revoke them" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const a = try k.createObject(s, .channel, c.rights(c.R_DUP | c.R_ADMIN | c.R_READ), .{});
    const b = try k.derive(s, a, c.rights(c.R_DUP | c.R_READ));
    const cc = try k.derive(s, b, c.rights(c.R_READ));
    try k.release(s, b);
    try c.expectConsistent(k);
    _ = try k.lookup(s, cc, .channel, c.rights(c.R_READ));
    try expectEqual(@as(usize, 1), try k.revoke(s, a));
    try expectError(error.InvalidHandle, k.lookup(s, cc, .channel, c.Rights.none));
    _ = try k.lookup(s, a, .channel, c.Rights.none);
    try c.expectConsistent(k);
}

test "LC-05 transfer moves a capability; source invalid; rights and tree position kept" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const a = try k.createSpace(c.big);
    const b = try k.createSpace(c.big);
    const root = try k.createObject(a, .channel, c.rights(c.R_DUP | c.R_ADMIN | c.R_XFER | c.R_READ), .{});
    const child = try k.derive(a, root, c.rights(c.R_XFER | c.R_READ));
    const used_a = try k.spaceUsed(a, .handles);
    const moved = try k.transfer(a, child, b);
    try expectError(error.InvalidHandle, k.lookup(a, child, .channel, c.Rights.none));
    const r = try k.lookup(b, moved, .channel, c.Rights.none);
    try expectEqual(@as(u32, c.R_XFER | c.R_READ), r.rights.toRaw());
    try expectEqual(used_a - 1, try k.spaceUsed(a, .handles));
    try expectEqual(@as(u64, 1), try k.spaceUsed(b, .handles));
    try expectEqual(@as(usize, 1), try k.spaceHandleCount(b));
    // The moved capability is still revocable by its ancestor in the source space.
    try expectEqual(@as(usize, 1), try k.revoke(a, root));
    try expectError(error.InvalidHandle, k.lookup(b, moved, .channel, c.Rights.none));
    try expectEqual(@as(u64, 0), try k.spaceUsed(b, .handles));
    try c.expectConsistent(k);
}

test "LC-06 transfer errors: no transfer right, same space, closed (zombie) or stale destination; nothing changes" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const a = try k.createSpace(c.big);
    const b = try k.createSpace(c.big);
    const stale = try k.createSpace(c.big);
    try k.closeSpace(stale); // freed at once: nothing billed to it
    const zombie = try k.createSpace(c.big);
    const zh = try k.createObject(zombie, .channel, c.rights(c.R_XFER), .{});
    _ = try k.transfer(zombie, zh, b); // object stays billed to `zombie`
    try k.closeSpace(zombie);
    const h = try k.createObject(a, .channel, c.rights(c.R_XFER | c.R_DUP), .{});
    const noxfer = try k.derive(a, h, c.Rights.none);
    const before = k.stats();
    try expectError(error.AccessDenied, k.transfer(a, noxfer, b));
    try expectError(error.InvalidArgument, k.transfer(a, h, a));
    try expectError(error.InvalidHandle, k.transfer(a, .invalid, b));
    try expectError(error.InvalidHandle, k.transfer(a, h, stale));
    try expectError(error.PeerDead, k.transfer(a, h, zombie));
    try expectEqual(before, k.stats());
    _ = try k.lookup(a, h, .channel, c.Rights.none);
    try expectEqual(@as(u64, 1), try k.spaceUsed(b, .handles));
    try c.expectConsistent(k);
}

test "LC-07 closeSpace releases everything, destroys unreferenced objects, rejects later use" {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    t.kernel_ptr = k;
    const s = try k.createSpace(c.big);
    const h1 = try k.createObject(s, .channel, c.rights(c.R_DUP), t.opts());
    const h2 = try k.createObject(s, .channel, c.rights(c.R_DUP), t.opts());
    _ = try k.derive(s, h1, c.Rights.none);
    try k.closeSpace(s);
    try expectEqual(@as(u32, 2), t.total);
    try expectEqual(@as(usize, 0), k.stats().objects);
    try expectEqual(@as(usize, 0), k.stats().spaces); // nothing billed elsewhere: freed immediately
    try expectError(error.InvalidHandle, k.lookup(s, h2, .channel, c.Rights.none)); // stale SpaceId
    try expectError(error.InvalidHandle, k.closeSpace(s));
    try expectEqual(@as(u64, 0), k.rootUsed(.handles));
    try expect(!t.saw_lock_held);
    try c.expectConsistent(k);
}

test "LC-08 zombie space: closing the billed space keeps its charge until the object is destroyed" {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    t.kernel_ptr = k;
    const a = try k.createSpace(c.big);
    const b = try k.createSpace(c.big);
    const h = try k.createObject(a, .channel, c.rights(c.R_XFER | c.R_DUP), t.optsCharged(7));
    const id = try c.idOf(k, a, h, .channel);
    const moved = try k.transfer(a, h, b);
    try k.closeSpace(a);
    try expectEqual(@as(usize, 2), k.stats().spaces); // `a` is a zombie
    try expectEqual(@as(u64, 7), try k.spaceUsed(a, .pages));
    try expectError(error.PeerDead, k.createObject(a, .channel, c.Rights.none, .{}));
    try expectError(error.PeerDead, k.closeSpace(a));
    _ = try k.lookup(b, moved, .channel, c.Rights.none);
    try c.expectConsistent(k);
    try k.release(b, moved);
    try expectEqual(@as(u8, 1), t.count(id));
    try expectEqual(@as(usize, 1), k.stats().spaces); // zombie freed
    try expectError(error.InvalidHandle, k.spaceUsed(a, .pages));
    try expectEqual(@as(u64, 0), k.rootUsed(.pages));
    try c.expectConsistent(k);
}

test "LC-09 destructor runs outside the lock, may re-enter the kernel, and runs exactly once" {
    const Ctx = struct {
        var kp: *c.Kernel = undefined;
        var space: c.SpaceId = undefined;
        var other: c.Handle = .invalid;
        var ran: u32 = 0;
        var held: bool = false;
        var reentered: bool = false;
        fn dtor(_: ?*anyopaque, _: c.ObjectId) void {
            ran += 1;
            if (kp.mutex.tryLock()) kp.mutex.unlock() else held = true;
            // Re-enter: release another capability, whose destruction nests.
            kp.release(space, other) catch return;
            _ = kp.createObject(space, .timer, c.Rights.none, .{}) catch return;
            _ = kp.stats();
            reentered = true;
        }
    };
    var inner = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    inner.kernel_ptr = k;
    Ctx.kp = k;
    Ctx.space = try k.createSpace(c.big);
    Ctx.other = try k.createObject(Ctx.space, .channel, c.Rights.none, inner.opts());
    const outer = try k.createObject(Ctx.space, .channel, c.Rights.none, .{ .destroy_fn = Ctx.dtor });
    try k.release(Ctx.space, outer);
    try expectEqual(@as(u32, 1), Ctx.ran);
    try expect(!Ctx.held);
    try expect(Ctx.reentered);
    try expectEqual(@as(u32, 1), inner.total);
    try expect(!inner.saw_lock_held);
    try expectEqual(@as(usize, 1), k.stats().objects); // only the timer created by the destructor
    try c.expectConsistent(k);
}

test "LC-10 kernel teardown destroys every remaining object once, including pinned ones" {
    var t = c.Tracker{};
    {
        const k = try c.newKernel(std.testing.allocator);
        const s = try k.createSpace(c.big);
        const h1 = try k.createObject(s, .channel, c.Rights.none, t.opts());
        _ = try k.createObject(s, .channel, c.Rights.none, t.opts());
        _ = try k.beginOp(s, h1, .channel, c.Rights.none);
        k.destroy(); // std.testing.allocator reports any leak
    }
    try expectEqual(@as(u32, 2), t.total);
}

test "LC-11 object ids are never reused and never invalid" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    var seen: [100]u64 = undefined;
    for (&seen) |*slot| {
        const h = try k.createObject(s, .timer, c.Rights.none, .{});
        const id = try c.idOf(k, s, h, .timer);
        try expect(id.isValid());
        slot.* = id.raw();
        try k.release(s, h);
    }
    for (seen, 0..) |v, i| for (seen[i + 1 ..]) |w| try expect(v != w);
}

test "GEN-01 handle slot reuse changes the generation; stale handles never match" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    var stale: [50]c.Handle = undefined;
    var prev = try k.createObject(s, .timer, c.Rights.none, .{});
    for (&stale) |*slot| {
        slot.* = prev;
        try k.release(s, prev);
        const next = try k.createObject(s, .timer, c.Rights.none, .{});
        try expectEqual(prev.index(), next.index());
        try expect(next.generation() != prev.generation());
        try expect(next != .invalid);
        prev = next;
    }
    for (stale) |h| try expectError(error.InvalidHandle, k.lookup(s, h, .timer, c.Rights.none));
    _ = try k.lookup(s, prev, .timer, c.Rights.none);
}

test "GEN-02 handle-table generation exhaustion retires the slot (documented behaviour)" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const h0 = try k.createObject(s, .timer, c.Rights.none, .{});
    try expectEqual(@as(u16, 0), h0.index());
    try k.release(s, h0);
    k.testSetTableGen(s, 0, 0xFFFF);
    const h1 = try k.createObject(s, .timer, c.Rights.none, .{});
    try expectEqual(@as(u16, 0), h1.index());
    try expectEqual(@as(u16, 0xFFFF), h1.generation());
    _ = try k.lookup(s, h1, .timer, c.Rights.none);
    try k.release(s, h1); // generation would wrap: the slot is retired instead
    try expectError(error.InvalidHandle, k.lookup(s, h1, .timer, c.Rights.none));
    try expectError(error.InvalidHandle, k.lookup(s, h0, .timer, c.Rights.none));
    const h2 = try k.createObject(s, .timer, c.Rights.none, .{});
    try expectEqual(@as(u16, 1), h2.index()); // slot 0 is never reused
    try expectError(error.InvalidHandle, k.lookup(s, h1, .timer, c.Rights.none));
    try c.expectConsistent(k);
}

test "GEN-03 node generation exhaustion retires the node slot; stale operation tokens stay invalid" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .timer, c.rights(c.R_DUP), .{});
    const child = try k.derive(s, root, c.Rights.none); // node slot 1
    const tok = try k.beginOp(s, child, .timer, c.Rights.none);
    try expect(k.opStillAuthorized(tok));
    try k.release(s, child);
    try expect(!k.opStillAuthorized(tok));
    k.testSetNodeGen(1, std.math.maxInt(u32));
    const child2 = try k.derive(s, root, c.Rights.none); // reuses node slot 1 at max generation
    try expect(!k.opStillAuthorized(tok)); // slot reuse does not revive the old authority
    try k.release(s, child2); // retires the slot
    try expectEqual(@as(usize, 1), k.stats().retired_nodes);
    const child3 = try k.derive(s, root, c.Rights.none);
    _ = try k.lookup(s, child3, .timer, c.Rights.none);
    try expect(!k.opStillAuthorized(tok));
    try k.endOp(tok);
    try c.expectConsistent(k);
}

test "GEN-04 stale SpaceId after the slot is reused is rejected" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s1 = try k.createSpace(c.big);
    try k.closeSpace(s1);
    const s2 = try k.createSpace(c.big);
    try expectEqual(s1.index, s2.index);
    try expect(s1.gen != s2.gen);
    try expectError(error.InvalidHandle, k.createObject(s1, .timer, c.Rights.none, .{}));
    _ = try k.createObject(s2, .timer, c.Rights.none, .{});
}

test "GEN-05 no issued handle is ever zero across many create/release cycles" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    var i: usize = 0;
    while (i < 3000) : (i += 1) {
        const h = try k.createObject(s, .timer, c.Rights.none, .{});
        try expect(h.raw() != 0 and h.generation() != 0);
        try k.release(s, h);
    }
}
