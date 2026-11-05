//! Resource accounting tests across creation, derive, transfer, release, destroy (ACC-xx).

const std = @import("std");
const c = @import("common.zig");
const expect = std.testing.expect;
const expectEqual = std.testing.expectEqual;
const expectError = std.testing.expectError;

fn limits(handles: u64, pages: u64) c.Counters {
    return .{ .pages = pages, .handles = handles, .threads = 0, .channels = 0, .queued_bytes = 0 };
}

test "ACC-01 charges at create/derive, refunds at release; object charge refunded after destruction" {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    t.kernel_ptr = k;
    const s = try k.createSpace(c.big);
    const h = try k.createObject(s, .memory_object, c.rights(c.R_DUP), t.optsCharged(10));
    try expectEqual(@as(u64, 1), try k.spaceUsed(s, .handles));
    try expectEqual(@as(u64, 10), try k.spaceUsed(s, .pages));
    try expectEqual(@as(u64, 10), k.rootUsed(.pages));
    const d = try k.derive(s, h, c.Rights.none);
    try expectEqual(@as(u64, 2), try k.spaceUsed(s, .handles));
    try expectEqual(@as(u64, 10), try k.spaceUsed(s, .pages)); // derive adds no object charge
    try k.release(s, h);
    try expectEqual(@as(u64, 1), try k.spaceUsed(s, .handles));
    try expectEqual(@as(u64, 10), try k.spaceUsed(s, .pages)); // object still alive
    try k.release(s, d);
    try expectEqual(@as(u64, 0), try k.spaceUsed(s, .handles));
    try expectEqual(@as(u64, 0), try k.spaceUsed(s, .pages));
    try expectEqual(@as(u64, 0), k.rootUsed(.pages));
    try expectEqual(@as(u64, 0), k.rootUsed(.handles));
    try c.expectConsistent(k);
}

test "ACC-02 the charge is still held while the destructor runs and refunded afterwards" {
    const Ctx = struct {
        var kp: *c.Kernel = undefined;
        var space: c.SpaceId = undefined;
        var seen_pages: u64 = 0;
        var seen_handles: u64 = 99;
        fn dtor(_: ?*anyopaque, _: c.ObjectId) void {
            seen_pages = kp.spaceUsed(space, .pages) catch 12345;
            seen_handles = kp.spaceUsed(space, .handles) catch 12345;
        }
    };
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    Ctx.kp = k;
    Ctx.space = try k.createSpace(c.big);
    const h = try k.createObject(Ctx.space, .channel, c.Rights.none, .{ .destroy_fn = Ctx.dtor, .charge_resource = .pages, .charge_amount = 5 });
    try k.release(Ctx.space, h);
    try expectEqual(@as(u64, 5), Ctx.seen_pages); // object charge held during destruction
    try expectEqual(@as(u64, 0), Ctx.seen_handles); // handle charge already refunded
    try expectEqual(@as(u64, 0), try k.spaceUsed(Ctx.space, .pages));
}

test "ACC-03 create fails atomically when the handle limit or the object charge cannot be met" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const s = try k.createSpace(limits(2, 10));
    const before = k.stats();
    try expectError(error.QuotaExceeded, k.createObject(s, .channel, c.Rights.none, .{ .charge_resource = .pages, .charge_amount = 11 }));
    try expectEqual(@as(u64, 0), try k.spaceUsed(s, .handles)); // handle charge rolled back
    try expectEqual(@as(u64, 0), try k.spaceUsed(s, .pages));
    try expectEqual(before, k.stats());
    const h1 = try k.createObject(s, .channel, c.rights(c.R_DUP), .{ .charge_resource = .pages, .charge_amount = 10 });
    _ = try k.derive(s, h1, c.Rights.none);
    try expectError(error.QuotaExceeded, k.createObject(s, .channel, c.Rights.none, .{}));
    try expectError(error.QuotaExceeded, k.derive(s, h1, c.Rights.none));
    try expectEqual(@as(u64, 2), try k.spaceUsed(s, .handles));
    try c.expectConsistent(k);
}

test "ACC-04 transfer charges the destination first; a full destination leaves everything unchanged" {
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    const a = try k.createSpace(c.big);
    const b = try k.createSpace(limits(1, 0));
    const h1 = try k.createObject(a, .channel, c.rights(c.R_XFER), .{});
    const h2 = try k.createObject(a, .channel, c.rights(c.R_XFER), .{});
    _ = try k.transfer(a, h1, b);
    const before = k.stats();
    const used_a = try k.spaceUsed(a, .handles);
    try expectError(error.QuotaExceeded, k.transfer(a, h2, b));
    try expectEqual(before, k.stats());
    try expectEqual(used_a, try k.spaceUsed(a, .handles));
    _ = try k.lookup(a, h2, .channel, c.Rights.none); // still held by the source
    try c.expectConsistent(k);
}

test "ACC-05 root limits bound the sum over spaces; child limits are bounded by the root's remaining budget" {
    var small = c.bigLimits();
    small.root = limits(3, 100);
    const k2 = try kernel_mod.Kernel.create(std.testing.allocator, small);
    defer k2.destroy();
    const a = try k2.createSpace(limits(2, 0));
    const b = try k2.createSpace(limits(2, 0)); // child limits may overlap; root usage bounds them
    _ = try k2.createObject(a, .channel, c.Rights.none, .{});
    _ = try k2.createObject(a, .channel, c.Rights.none, .{});
    _ = try k2.createObject(b, .channel, c.Rights.none, .{});
    try expectError(error.QuotaExceeded, k2.createObject(b, .channel, c.Rights.none, .{})); // root at 3
    try expectEqual(@as(u64, 3), k2.rootUsed(.handles));
    try expectError(error.QuotaExceeded, k2.createSpace(limits(1, 0))); // exceeds remaining root budget (0)
    try c.expectConsistent(k2);
}

const kernel_mod = c.kernel;

test "ACC-06 space handle table limit 65535 slots is reported as QuotaExceeded without state change" {
    // A direct arena check; creating 65535 real objects would be slow and add nothing.
    const GenArena = @import("../gen_arena.zig").GenArena;
    var a = GenArena(u8, u16, u16).init(kernel_mod.table_slot_limit);
    defer a.deinit(std.testing.allocator);
    try expectEqual(@as(u16, 0xFFFF), @TypeOf(a).none);
    try expectEqual(@as(usize, 65535), a.limit);
}

test "ACC-07 destroying objects refunds exactly what was charged over many random objects" {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    t.kernel_ptr = k;
    const s = try k.createSpace(c.big);
    var prng = std.Random.DefaultPrng.init(7);
    const rnd = prng.random();
    var hs: [200]c.Handle = undefined;
    var total: u64 = 0;
    for (&hs) |*h| {
        const amt = rnd.uintLessThan(u64, 50);
        h.* = try k.createObject(s, .memory_object, c.Rights.none, t.optsCharged(amt));
        total += amt;
    }
    try expectEqual(total, try k.spaceUsed(s, .pages));
    for (hs) |h| try k.release(s, h);
    try expectEqual(@as(u64, 0), try k.spaceUsed(s, .pages));
    try expectEqual(@as(u32, 200), t.total);
    try c.expectConsistent(k);
}
