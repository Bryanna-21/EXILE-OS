//! Endpoint ownership, rights and lifecycle tests (EP-xx).

const std = @import("std");
const c = @import("common.zig");
const expect = std.testing.expect;
const expectEqual = std.testing.expectEqual;
const expectError = std.testing.expectError;

test "EP-01 createChannel returns two distinct ends and charges the creator" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.space();
    const ends = try env.x.createChannel(a);
    try expect(ends[0] != ends[1]);
    try expectEqual(@as(u64, 2), try env.k.spaceUsed(a, .channels));
    try expectEqual(@as(usize, 2), env.x.registered());
    const s0 = try env.x.endStats(a, ends[0]);
    try expectEqual(@as(u32, 0), s0.queued_messages);
    try expect(!s0.closed and !s0.peer_closed);
    try env.consistent();
    try env.deinit();
}

test "EP-02 createChannel is all-or-nothing when the creator's channel quota is exhausted" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.k.createSpace(.{ .handles = 100, .channels = 1, .queued_bytes = 1000 });
    try expectError(error.QuotaExceeded, env.x.createChannel(a));
    try expectEqual(@as(u64, 0), try env.k.spaceUsed(a, .channels));
    try expectEqual(@as(usize, 0), try env.k.spaceHandleCount(a));
    try expectEqual(@as(usize, 0), env.x.registered());
    try env.consistent();
    try env.deinit();
}

test "EP-03 every endpoint operation checks its own right" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.space();
    const ends = try env.x.createChannel(a);
    const peer = ends[1];
    const only_read = try env.k.derive(a, ends[0], c.rights(c.R_READ));
    const only_write = try env.k.derive(a, ends[0], c.rights(c.R_WRITE));
    const only_inspect = try env.k.derive(a, ends[0], c.rights(c.R_INSPECT));
    const only_admin = try env.k.derive(a, ends[0], c.rights(c.R_ADMIN));

    var buf: [64]u8 = undefined;
    const m = c.msg(&buf, 1, "x", 0);
    try expectError(error.AccessDenied, env.x.send(a, only_read, m, &.{}, .{}, null));
    try env.x.send(a, only_write, m, &.{}, .{}, null);
    var out: c.Received = undefined;
    try expectError(error.AccessDenied, env.x.recv(a, only_write, .{}, &out));
    try expectError(error.AccessDenied, env.x.endStats(a, only_admin));
    _ = try env.x.endStats(a, only_inspect);
    try expectError(error.AccessDenied, env.x.closeEnd(a, only_inspect));
    // The message went to the peer end's inbox.
    var got = try c.recvOne(&env, a, peer, .{});
    got.deinit(env.x);
    try env.x.closeEnd(a, only_admin);
    try env.consistent();
    try env.deinit();
}

test "EP-04 wrong object type, released and invalid handles are rejected" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.space();
    const ends = try env.x.createChannel(a);
    var t = c.Tracker{};
    const mem = try t.make(env.k, a, c.R_ALL);
    var buf: [64]u8 = undefined;
    const m = c.msg(&buf, 1, "x", 0);
    try expectError(error.InvalidHandle, env.x.send(a, mem, m, &.{}, .{}, null));
    try expectError(error.InvalidHandle, env.x.send(a, .invalid, m, &.{}, .{}, null));
    const copy = try env.k.derive(a, ends[0], c.rights(c.R_WRITE));
    try env.k.release(a, copy);
    try expectError(error.InvalidHandle, env.x.send(a, copy, m, &.{}, .{}, null));
    // A cancellation handle is not an endpoint.
    const cancel = try env.x.createCancel(a);
    try expectError(error.InvalidHandle, env.x.send(a, cancel, m, &.{}, .{}, null));
    try env.consistent();
    try env.deinit();
}

test "EP-05 closeEnd: peer sends fail, own ops fail, queued messages stay receivable by the peer" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.space();
    const ends = try env.x.createChannel(a);
    try c.sendBytes(&env, a, ends[0], 1, "one");
    try c.sendBytes(&env, a, ends[0], 2, "two");
    try env.x.closeEnd(a, ends[0]);
    try env.x.closeEnd(a, ends[0]); // idempotent
    try expectError(error.PeerDead, c.sendBytes(&env, a, ends[0], 3, "x"));
    try expectError(error.PeerDead, c.sendBytes(&env, a, ends[1], 3, "x"));
    try expectError(error.PeerDead, c.recvOne(&env, a, ends[0], .{}));
    // The closed side's pending messages were sent TO ends[1] and remain deliverable.
    var m1 = try c.recvOne(&env, a, ends[1], .{});
    try std.testing.expectEqualStrings("one", m1.payload);
    m1.deinit(env.x);
    var m2 = try c.recvOne(&env, a, ends[1], .{});
    m2.deinit(env.x);
    try expectError(error.PeerDead, c.recvOne(&env, a, ends[1], .{}));
    try env.consistent();
    try env.deinit();
}

test "EP-06 releasing the last capability of an end closes it and unregisters it" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.space();
    const b = try env.space();
    const ends = try env.x.createChannel(a);
    const moved = try env.k.transfer(a, ends[1], b);
    try c.sendBytes(&env, a, ends[0], 1, "pending");
    try env.k.release(b, moved); // last capability of end 1
    try expectEqual(@as(usize, 1), env.x.registered());
    try expectError(error.PeerDead, c.sendBytes(&env, a, ends[0], 2, "x"));
    const st = try env.x.endStats(a, ends[0]);
    try expect(st.peer_closed);
    try expectEqual(@as(u64, 1), env.x.stats().dropped_on_close);
    // The dropped message's accounting was refunded.
    try expectEqual(@as(u64, 0), try env.k.spaceUsed(a, .queued_bytes));
    try env.consistent();
    try env.deinit();
}

test "EP-07 destroying the space that holds an end closes the end" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.space();
    const b = try env.space();
    const ends = try env.x.createChannel(a);
    const moved = try env.k.transfer(a, ends[1], b);
    try c.sendBytes(&env, b, moved, 1, "to a");
    try env.k.closeSpace(b);
    try expectError(error.PeerDead, c.sendBytes(&env, a, ends[0], 2, "x"));
    // The message b sent is in a's inbox: still deliverable after b died.
    var got = try c.recvOne(&env, a, ends[0], .{});
    try std.testing.expectEqualStrings("to a", got.payload);
    got.deinit(env.x);
    try expectError(error.PeerDead, c.recvOne(&env, a, ends[0], .{}));
    try expectEqual(@as(u64, 0), env.k.rootUsed(.queued_bytes));
    try env.consistent();
    try env.deinit();
}

test "EP-08 destroying the kernel with queued messages and capabilities leaks nothing" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.space();
    const ends = try env.x.createChannel(a);
    var t = c.Tracker{};
    const cap = try t.make(env.k, a, c.R_ALL);
    const id = try t.idOf(env.k, a, cap);
    var buf: [64]u8 = undefined;
    const m = c.msg(&buf, 1, "x", 1);
    try env.x.send(a, ends[0], m, &.{.{ .handle = cap, .otype = .memory_object, .mode = .move, .rights = c.rights(c.R_ALL) }}, .{}, null);
    try expectEqual(@as(u8, 0), t.count(id));
    try env.deinit(); // testing allocator detects leaks; deinit checks the registry
    try expectEqual(@as(u8, 1), t.count(id));
}

test "EP-09 sending an end through its own channel is rejected" {
    var env = try c.Env.init(std.testing.allocator, .{});
    const a = try env.space();
    const ends = try env.x.createChannel(a);
    var buf: [64]u8 = undefined;
    const m = c.msg(&buf, 1, "x", 1);
    // ends[1] is the destination inbox owner: queueing it into its own inbox is a direct cycle.
    try expectError(error.InvalidArgument, env.x.send(a, ends[0], m, &.{.{ .handle = ends[1], .otype = .channel, .mode = .move, .rights = c.rights(c.R_ALL & ~@as(u32, 4)) }}, .{}, null));
    try env.consistent();
    try env.deinit();
}
