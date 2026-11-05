//! Unit tests for the generation-checked arena (ARN-xx).

const std = @import("std");
const GenArena = @import("../gen_arena.zig").GenArena;

const A = GenArena(u32, u16, u16);

test "ARN-01 alloc, get, free, stale ref rejected" {
    var a = A.init(10);
    defer a.deinit(std.testing.allocator);
    try a.reserveOne(std.testing.allocator);
    const r1 = a.allocAssumeCapacity(11);
    try std.testing.expectEqual(@as(u16, 1), r1.gen);
    try std.testing.expectEqual(@as(u32, 11), a.get(r1).?.*);
    try std.testing.expectEqual(A.FreeOutcome.reusable, try a.free(r1));
    try std.testing.expect(a.get(r1) == null);
    try std.testing.expectError(error.InvalidRef, a.free(r1));
    try a.reserveOne(std.testing.allocator);
    const r2 = a.allocAssumeCapacity(22);
    try std.testing.expectEqual(r1.index, r2.index);
    try std.testing.expectEqual(@as(u16, 2), r2.gen);
    try std.testing.expect(a.get(r1) == null);
    try std.testing.expect(a.get(r2) != null);
    try std.testing.expect(a.isConsistent());
}

test "ARN-02 out-of-range and wrong-generation refs are rejected" {
    var a = A.init(10);
    defer a.deinit(std.testing.allocator);
    try a.reserveOne(std.testing.allocator);
    const r = a.allocAssumeCapacity(1);
    try std.testing.expect(a.get(.{ .index = 5, .gen = 1 }) == null);
    try std.testing.expect(a.get(.{ .index = A.none, .gen = 1 }) == null);
    try std.testing.expect(a.get(.{ .index = r.index, .gen = r.gen + 1 }) == null);
    try std.testing.expect(a.get(.{ .index = r.index, .gen = 0 }) == null);
}

test "ARN-03 generation exhaustion retires the slot; capacity shrinks; stale refs stay invalid" {
    var a = A.init(2);
    defer a.deinit(std.testing.allocator);
    try a.reserveOne(std.testing.allocator);
    const r1 = a.allocAssumeCapacity(1);
    _ = try a.free(r1);
    a.testSetGen(0, A.max_gen);
    try a.reserveOne(std.testing.allocator);
    const r2 = a.allocAssumeCapacity(2);
    try std.testing.expectEqual(A.max_gen, r2.gen);
    try std.testing.expectEqual(A.FreeOutcome.retired, try a.free(r2));
    try std.testing.expectEqual(@as(usize, 1), a.retiredCount());
    try std.testing.expect(a.get(r2) == null);
    try std.testing.expect(a.get(r1) == null);
    try std.testing.expectEqual(A.State.retired, a.stateAt(0).?);
    // A retired slot is never handed out again.
    try a.reserveOne(std.testing.allocator);
    const r3 = a.allocAssumeCapacity(3);
    try std.testing.expectEqual(@as(u16, 1), r3.index);
    try std.testing.expect(a.isConsistent());
    // Limit is 2 slots and one is retired: with slot 1 live, no capacity remains.
    try std.testing.expectError(error.QuotaExceeded, a.reserveOne(std.testing.allocator));
    _ = try a.free(r3);
    try a.reserveOne(std.testing.allocator); // slot 1 reusable
    _ = a.allocAssumeCapacity(4);
    try std.testing.expectError(error.QuotaExceeded, a.reserveOne(std.testing.allocator));
}

test "ARN-04 generation never 0 across a full wrap of one slot" {
    var a = GenArena(u8, u8, u8).init(4);
    defer a.deinit(std.testing.allocator);
    var cycles: usize = 0;
    var last_gen: u8 = 0;
    while (true) {
        a.reserveOne(std.testing.allocator) catch break;
        const r = a.allocAssumeCapacity(0);
        try std.testing.expect(r.gen != 0);
        if (r.index == 0) {
            try std.testing.expect(r.gen > last_gen);
            last_gen = r.gen;
        }
        _ = try a.free(r);
        cycles += 1;
        if (a.retiredCount() >= 4) break;
        if (cycles > 5000) return error.TestUnexpectedResult;
    }
    // 4 slots, each used for 255 generations before retiring, then no capacity.
    try std.testing.expectEqual(@as(usize, 4), a.retiredCount());
    try std.testing.expectEqual(@as(usize, 4 * 255), cycles);
    try std.testing.expect(a.isConsistent());
}

test "ARN-05 reserveOne reports QuotaExceeded at the limit without changing state" {
    var a = A.init(2);
    defer a.deinit(std.testing.allocator);
    try a.reserveOne(std.testing.allocator);
    _ = a.allocAssumeCapacity(1);
    try a.reserveOne(std.testing.allocator);
    _ = a.allocAssumeCapacity(2);
    try std.testing.expectError(error.QuotaExceeded, a.reserveOne(std.testing.allocator));
    try std.testing.expectEqual(@as(usize, 2), a.liveCount());
    try std.testing.expect(a.isConsistent());
}

test "ARN-06 allocation failure in reserveOne changes nothing" {
    var fa = std.testing.FailingAllocator.init(std.testing.allocator, .{ .fail_index = 0 });
    var a = A.init(4);
    defer a.deinit(fa.allocator());
    try std.testing.expectError(error.OutOfMemory, a.reserveOne(fa.allocator()));
    try std.testing.expectEqual(@as(usize, 0), a.slotCount());
    try std.testing.expectEqual(@as(usize, 0), a.liveCount());
    try std.testing.expect(a.isConsistent());
}

test "ARN-07 randomized against a model; free list stays consistent" {
    var prng = std.Random.DefaultPrng.init(0xA11CE);
    const rnd = prng.random();
    var a = A.init(64);
    defer a.deinit(std.testing.allocator);
    var live: [64]?A.Ref = [_]?A.Ref{null} ** 64;
    var dead: [256]A.Ref = undefined;
    var ndead: usize = 0;
    var steps: usize = 0;
    while (steps < 5000) : (steps += 1) {
        const i = rnd.uintLessThan(usize, live.len);
        if (live[i]) |r| {
            _ = try a.free(r);
            live[i] = null;
            dead[ndead % dead.len] = r;
            ndead += 1;
        } else {
            a.reserveOne(std.testing.allocator) catch continue;
            live[i] = a.allocAssumeCapacity(@intCast(i));
        }
        for (dead[0..@min(ndead, dead.len)]) |d| try std.testing.expect(a.get(d) == null);
        for (live) |maybe| {
            if (maybe) |r| try std.testing.expect(a.get(r) != null);
        }
        try std.testing.expect(a.isConsistent());
    }
}
