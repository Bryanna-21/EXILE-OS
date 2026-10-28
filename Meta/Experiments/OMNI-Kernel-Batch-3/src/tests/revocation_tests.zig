//! Revocation, in-flight operation and stale-authority tests (REV-xx).

const std = @import("std");
const c = @import("common.zig");
const expect = std.testing.expect;
const expectEqual = std.testing.expectEqual;
const expectError = std.testing.expectError;

const owner_rights = c.R_DUP | c.R_ADMIN | c.R_XFER | c.R_READ | c.R_WRITE;

test "REV-01 revoke removes every descendant, keeps the node, its siblings and ancestors" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(owner_rights), .{});
    const c1 = try k.derive(s, root, c.rights(c.R_DUP | c.R_ADMIN | c.R_READ));
    const c2 = try k.derive(s, root, c.rights(c.R_READ));
    const g1 = try k.derive(s, c1, c.rights(c.R_DUP | c.R_READ));
    const gg = try k.derive(s, g1, c.rights(c.R_READ));
    const used = try k.spaceUsed(s, .handles);
    try expectEqual(@as(u64, 5), used);

    try expectEqual(@as(usize, 2), try k.revoke(s, c1)); // g1 and gg only
    try expectError(error.InvalidHandle, k.lookup(s, g1, .channel, c.Rights.none));
    try expectError(error.InvalidHandle, k.lookup(s, gg, .channel, c.Rights.none));
    _ = try k.lookup(s, c1, .channel, c.Rights.none);
    _ = try k.lookup(s, c2, .channel, c.Rights.none);
    try expectEqual(@as(u64, 3), try k.spaceUsed(s, .handles));
    try c.expectConsistent(k);

    try expectEqual(@as(usize, 2), try k.revoke(s, root));
    try expectError(error.InvalidHandle, k.lookup(s, c1, .channel, c.Rights.none));
    try expectError(error.InvalidHandle, k.lookup(s, c2, .channel, c.Rights.none));
    _ = try k.lookup(s, root, .channel, c.Rights.none);
    try expectEqual(@as(u64, 1), try k.spaceUsed(s, .handles));
    try expectEqual(@as(usize, 0), try k.revoke(s, root)); // nothing left: idempotent
    try c.expectConsistent(k);
}

test "REV-02 revoke requires admin; a failed revoke changes nothing" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(owner_rights), .{});
    const noadmin = try k.derive(s, root, c.rights(c.R_DUP | c.R_READ));
    const kid = try k.derive(s, noadmin, c.rights(c.R_READ));
    const before = k.stats();
    try expectError(error.AccessDenied, k.revoke(s, noadmin));
    try expectError(error.InvalidHandle, k.revoke(s, .invalid));
    try expectEqual(before, k.stats());
    _ = try k.lookup(s, kid, .channel, c.Rights.none);
}

test "REV-03 revocation crosses spaces: descendants moved to other spaces are revoked and refunded" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const a = try k.createSpace(c.big);
    const b = try k.createSpace(c.big);
    const d = try k.createSpace(c.big);
    const root = try k.createObject(a, .channel, c.rights(owner_rights), .{});
    const x = try k.derive(a, root, c.rights(c.R_DUP | c.R_XFER | c.R_READ));
    const xb = try k.transfer(a, x, b);
    const y = try k.derive(b, xb, c.rights(c.R_XFER | c.R_READ));
    const yd = try k.transfer(b, y, d);
    try expectEqual(@as(u64, 1), try k.spaceUsed(b, .handles));
    try expectEqual(@as(u64, 1), try k.spaceUsed(d, .handles));
    try expectEqual(@as(usize, 2), try k.revoke(a, root));
    try expectError(error.InvalidHandle, k.lookup(b, xb, .channel, c.Rights.none));
    try expectError(error.InvalidHandle, k.lookup(d, yd, .channel, c.Rights.none));
    try expectEqual(@as(u64, 0), try k.spaceUsed(b, .handles));
    try expectEqual(@as(u64, 0), try k.spaceUsed(d, .handles));
    try expectEqual(@as(u64, 1), try k.spaceUsed(a, .handles));
    try c.expectConsistent(k);
}

test "REV-04 revoke through a node whose ancestor was released and which became a root" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(owner_rights), .{});
    const mid = try k.derive(s, root, c.rights(owner_rights));
    const leaf = try k.derive(s, mid, c.rights(c.R_READ));
    try k.release(s, root); // mid becomes a root; leaf stays below mid
    try expectEqual(@as(usize, 1), try k.revoke(s, mid));
    try expectError(error.InvalidHandle, k.lookup(s, leaf, .channel, c.Rights.none));
    _ = try k.lookup(s, mid, .channel, c.Rights.none);
    try c.expectConsistent(k);
}

test "REV-05 stale authority: a revoked handle stays invalid after its slot is reused" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(owner_rights), .{});
    const old = try k.derive(s, root, c.rights(c.R_READ | c.R_WRITE));
    _ = try k.revoke(s, root);
    const fresh = try k.derive(s, root, c.rights(c.R_READ));
    try expectEqual(old.index(), fresh.index()); // same table slot, different generation
    try expect(old != fresh);
    try expectError(error.InvalidHandle, k.lookup(s, old, .channel, c.rights(c.R_READ)));
    try expectError(error.InvalidHandle, k.derive(s, old, c.Rights.none));
    try expectError(error.InvalidHandle, k.release(s, old));
    const r = try k.lookup(s, fresh, .channel, c.Rights.none);
    try expectEqual(@as(u32, c.R_READ), r.rights.toRaw()); // no rights inherited from the old holder
}

test "REV-06 in-flight operation: authority is gone after revoke, but the object stays alive until endOp" {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    t.kernel_ptr = k;
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(owner_rights), t.opts());
    const id = try c.idOf(k, s, root, .channel);
    const kid = try k.derive(s, root, c.rights(c.R_READ));
    const tok = try k.beginOp(s, kid, .channel, c.rights(c.R_READ));
    try expect(k.opStillAuthorized(tok));
    try expectEqual(id.raw(), tok.object.raw());
    try expectEqual(@as(usize, 1), try k.revoke(s, root));
    try expect(!k.opStillAuthorized(tok)); // revoked while in flight
    try k.release(s, root); // last capability gone, but the pin remains
    try expectEqual(@as(u8, 0), t.count(id));
    try expectEqual(@as(usize, 1), k.stats().objects);
    try c.expectConsistent(k);
    try k.endOp(tok);
    try expectEqual(@as(u8, 1), t.count(id));
    try expectEqual(@as(usize, 0), k.stats().objects);
    try expectEqual(@as(usize, 0), k.stats().pins);
    try expectError(error.InvalidHandle, k.endOp(tok)); // double end
    try expect(!t.saw_lock_held);
    try c.expectConsistent(k);
}

test "REV-07 beginOp checks type and rights; failures pin nothing; transfer keeps authority" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const a = try k.createSpace(c.big);
    const b = try k.createSpace(c.big);
    const h = try k.createObject(a, .channel, c.rights(c.R_READ | c.R_XFER), .{});
    try expectError(error.InvalidHandle, k.beginOp(a, h, .timer, c.Rights.none));
    try expectError(error.AccessDenied, k.beginOp(a, h, .channel, c.rights(c.R_WRITE)));
    try expectError(error.InvalidHandle, k.beginOp(a, .invalid, .channel, c.Rights.none));
    try expectEqual(@as(usize, 0), k.stats().pins);
    const tok = try k.beginOp(a, h, .channel, c.rights(c.R_READ));
    const moved = try k.transfer(a, h, b);
    try expect(k.opStillAuthorized(tok)); // the capability still exists; only its holder changed
    try k.release(b, moved);
    try expect(!k.opStillAuthorized(tok));
    try k.endOp(tok);
    try c.expectConsistent(k);
}

test "REV-08 closing the holder's space ends in-flight authority" {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const h = try k.createObject(s, .channel, c.Rights.none, t.opts());
    const id = try c.idOf(k, s, h, .channel);
    const tok = try k.beginOp(s, h, .channel, c.Rights.none);
    try k.closeSpace(s);
    try expect(!k.opStillAuthorized(tok));
    try expectEqual(@as(u8, 0), t.count(id)); // pinned
    try k.endOp(tok);
    try expectEqual(@as(u8, 1), t.count(id));
    try expectEqual(@as(usize, 0), k.stats().spaces);
    try c.expectConsistent(k);
}

test "REV-09 a deep derivation chain is revoked iteratively (no recursion limit)" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(c.R_DUP | c.R_ADMIN), .{});
    var cur = root;
    var depth: usize = 0;
    while (depth < 3000) : (depth += 1) cur = try k.derive(s, cur, c.rights(c.R_DUP));
    try c.expectConsistent(k);
    try expectEqual(@as(usize, 3000), try k.revoke(s, root));
    try expectEqual(@as(usize, 1), k.stats().nodes);
    try c.expectConsistent(k);
}

test "REV-10 revoke result of derive-before versus derive-after (sequential linearization)" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(owner_rights), .{});
    const before = try k.derive(s, root, c.rights(c.R_READ));
    _ = try k.revoke(s, root);
    const after = try k.derive(s, root, c.rights(c.R_READ));
    try expectError(error.InvalidHandle, k.lookup(s, before, .channel, c.Rights.none));
    _ = try k.lookup(s, after, .channel, c.Rights.none);
}
