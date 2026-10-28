//! Allocation-failure tests (OOM-xx): for every fallible operation, fail the
//! allocator at each possible point and check that nothing changed.

const std = @import("std");
const c = @import("common.zig");
const expect = std.testing.expect;
const expectEqual = std.testing.expectEqual;

const Snapshot = struct {
    stats: kernel_stats,
    root: [5]u64,
    const kernel_stats = @TypeOf(@as(*c.Kernel, undefined).stats());
};

fn snapshot(k: *c.Kernel) Snapshot {
    return .{ .stats = k.stats(), .root = .{
        k.rootUsed(.pages),
        k.rootUsed(.handles),
        k.rootUsed(.threads),
        k.rootUsed(.channels),
        k.rootUsed(.queued_bytes),
    } };
}

/// Sweep the failure point over every allocation the operation makes.
fn sweep(comptime Op: type) !void {
    var k: usize = 0;
    var saw_failure = false;
    while (true) : (k += 1) {
        if (k > 64) return error.TestUnexpectedResult; // the operation allocates more than expected
        var fa = std.testing.FailingAllocator.init(std.testing.allocator, .{});
        const kern = try c.Kernel.create(fa.allocator(), c.bigLimits());
        defer kern.destroy();
        var st = try Op.setup(kern);
        const before = snapshot(kern);
        fa.fail_index = fa.alloc_index + k;
        const result = Op.run(kern, &st);
        fa.fail_index = std.math.maxInt(usize);
        if (result) |_| {
            try expect(!fa.has_induced_failure); // success means no allocation was refused
            try Op.check(kern, &st);
            try c.expectConsistent(kern);
            break;
        } else |e| {
            saw_failure = true;
            try expectEqual(error.QuotaExceeded, e);
            try expect(fa.has_induced_failure);
            const after = snapshot(kern);
            try expectEqual(before, after);
            try c.expectConsistent(kern);
            try Op.check_unchanged(kern, &st);
        }
    }
    try expect(saw_failure); // the sweep really exercised a failure
}

test "OOM-01 createSpace is all-or-nothing under allocation failure" {
    try sweep(struct {
        fn setup(_: *c.Kernel) !void {}
        fn run(kern: *c.Kernel, _: *void) !void {
            _ = try kern.createSpace(c.big);
        }
        fn check(kern: *c.Kernel, _: *void) !void {
            try expectEqual(@as(usize, 1), kern.stats().spaces);
        }
        fn check_unchanged(kern: *c.Kernel, _: *void) !void {
            try expectEqual(@as(usize, 0), kern.stats().spaces);
        }
    });
}

test "OOM-02 createObject is all-or-nothing under allocation failure" {
    try sweep(struct {
        const S = struct { space: c.SpaceId };
        fn setup(kern: *c.Kernel) !S {
            return .{ .space = try kern.createSpace(c.big) };
        }
        fn run(kern: *c.Kernel, s: *S) !void {
            _ = try kern.createObject(s.space, .channel, c.rights(c.R_DUP), .{ .charge_resource = .pages, .charge_amount = 3 });
        }
        fn check(kern: *c.Kernel, s: *S) !void {
            try expectEqual(@as(u64, 3), try kern.spaceUsed(s.space, .pages));
            try expectEqual(@as(usize, 1), kern.stats().objects);
        }
        fn check_unchanged(kern: *c.Kernel, s: *S) !void {
            try expectEqual(@as(u64, 0), try kern.spaceUsed(s.space, .pages));
            try expectEqual(@as(u64, 0), try kern.spaceUsed(s.space, .handles));
            try expectEqual(@as(usize, 0), kern.stats().objects);
        }
    });
}

test "OOM-03 derive is all-or-nothing under allocation failure" {
    try sweep(struct {
        const S = struct { space: c.SpaceId, h: c.Handle };
        fn setup(kern: *c.Kernel) !S {
            const sp = try kern.createSpace(c.big);
            // Arrays grow 0 -> 8 -> 20: filling exactly 8 makes the next derive grow them.
            var i: usize = 0;
            while (i < 7) : (i += 1) _ = try kern.createObject(sp, .timer, c.Rights.none, .{});
            return .{ .space = sp, .h = try kern.createObject(sp, .channel, c.rights(c.R_DUP | c.R_READ), .{}) };
        }
        fn run(kern: *c.Kernel, s: *S) !void {
            _ = try kern.derive(s.space, s.h, c.rights(c.R_READ));
        }
        fn check(kern: *c.Kernel, s: *S) !void {
            try expectEqual(@as(u64, 9), try kern.spaceUsed(s.space, .handles));
        }
        fn check_unchanged(kern: *c.Kernel, s: *S) !void {
            try expectEqual(@as(u64, 8), try kern.spaceUsed(s.space, .handles));
            _ = try kern.lookup(s.space, s.h, .channel, c.Rights.none);
        }
    });
}

test "OOM-04 transfer is all-or-nothing under allocation failure" {
    try sweep(struct {
        const S = struct { a: c.SpaceId, b: c.SpaceId, h: c.Handle };
        fn setup(kern: *c.Kernel) !S {
            const a = try kern.createSpace(c.big);
            const b = try kern.createSpace(c.big);
            return .{ .a = a, .b = b, .h = try kern.createObject(a, .channel, c.rights(c.R_XFER), .{}) };
        }
        fn run(kern: *c.Kernel, s: *S) !void {
            _ = try kern.transfer(s.a, s.h, s.b);
        }
        fn check(kern: *c.Kernel, s: *S) !void {
            try expectEqual(@as(u64, 1), try kern.spaceUsed(s.b, .handles));
            try expectEqual(@as(u64, 0), try kern.spaceUsed(s.a, .handles));
        }
        fn check_unchanged(kern: *c.Kernel, s: *S) !void {
            try expectEqual(@as(u64, 0), try kern.spaceUsed(s.b, .handles));
            _ = try kern.lookup(s.a, s.h, .channel, c.Rights.none); // source still holds it
        }
    });
}

test "OOM-05 beginOp is all-or-nothing under allocation failure" {
    try sweep(struct {
        const S = struct { space: c.SpaceId, h: c.Handle };
        fn setup(kern: *c.Kernel) !S {
            const sp = try kern.createSpace(c.big);
            return .{ .space = sp, .h = try kern.createObject(sp, .channel, c.Rights.none, .{}) };
        }
        fn run(kern: *c.Kernel, s: *S) !void {
            _ = try kern.beginOp(s.space, s.h, .channel, c.Rights.none);
        }
        fn check(kern: *c.Kernel, _: *S) !void {
            try expectEqual(@as(usize, 1), kern.stats().pins);
        }
        fn check_unchanged(kern: *c.Kernel, _: *S) !void {
            try expectEqual(@as(usize, 0), kern.stats().pins);
        }
    });
}

test "OOM-06 release, revoke, endOp and closeSpace never allocate (so they cannot fail for lack of memory)" {
    var fa = std.testing.FailingAllocator.init(std.testing.allocator, .{});
    const k = try c.Kernel.create(fa.allocator(), c.bigLimits());
    defer k.destroy();
    var t = c.Tracker{};
    const s = try k.createSpace(c.big);
    const s2 = try k.createSpace(c.big);
    const root = try k.createObject(s, .channel, c.rights(c.R_DUP | c.R_ADMIN | c.R_XFER), t.opts());
    const kid = try k.derive(s, root, c.rights(c.R_DUP | c.R_XFER));
    const tok = try k.beginOp(s, root, .channel, c.Rights.none);
    const other = try k.createObject(s2, .channel, c.Rights.none, t.opts());

    fa.fail_index = fa.alloc_index; // every allocation from here on fails
    _ = try k.revoke(s, root);
    _ = kid;
    try k.endOp(tok);
    try k.release(s, root);
    try k.release(s2, other);
    try k.closeSpace(s);
    try k.closeSpace(s2);
    try expect(!fa.has_induced_failure);
    try expectEqual(@as(u32, 2), t.total);
    fa.fail_index = std.math.maxInt(usize);
    try c.expectConsistent(k);
}
