//! Endpoints, queues, message validation, accounting (areas 1-6).
const std = @import("std");
const c = @import("common.zig");
const testing = std.testing;
const ipc = c.ipc;

fn send1(rig: *c.Rig, sp: c.SpaceId, h: c.Handle, payload: []const u8) !void {
    var buf: [4096]u8 = undefined;
    try rig.ipc.send(sp, h, c.msg(&buf, 1, payload, 0), &.{}, .{}, null);
}

test "IPC-01 channel round trip is FIFO" {
    var rig = try c.Rig.init(testing.allocator, .{});
    const sp = try rig.space();
    const ends = try rig.ipc.createChannel(sp);
    var buf: [128]u8 = undefined;
    try rig.ipc.send(sp, ends[0], c.msg(&buf, 1, "one", 0), &.{}, .{}, null);
    try rig.ipc.send(sp, ends[0], c.msg(&buf, 2, "two", 0), &.{}, .{}, null);
    var out: ipc.Received = undefined;
    try rig.ipc.recv(sp, ends[1], .{}, &out);
    try testing.expectEqualStrings("one", out.payload);
    try testing.expectEqual(@as(u16, 1), out.header.op);
    out.deinit(rig.ipc);
    try rig.ipc.recv(sp, ends[1], .{}, &out);
    try testing.expectEqualStrings("two", out.payload);
    out.deinit(rig.ipc);
    try testing.expectError(error.WouldBlock, rig.ipc.recv(sp, ends[1], .{}, &out));
    try rig.consistent();
    try testing.expectEqual(@as(usize, 0), rig.deinit());
}

test "IPC-02 both directions are independent queues" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const sp = try rig.space();
    const e = try rig.ipc.createChannel(sp);
    try send1(&rig, sp, e[0], "to1");
    try send1(&rig, sp, e[1], "to0");
    var out: ipc.Received = undefined;
    try rig.ipc.recv(sp, e[0], .{}, &out);
    try testing.expectEqualStrings("to0", out.payload);
    out.deinit(rig.ipc);
    try rig.ipc.recv(sp, e[1], .{}, &out);
    try testing.expectEqualStrings("to1", out.payload);
    out.deinit(rig.ipc);
}

test "IPC-03 end rights are enforced (WRITE to send, READ to recv, ADMIN to close, INSPECT to stat)" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const sp = try rig.space();
    const e = try rig.ipc.createChannel(sp);
    const ro = try rig.k.derive(sp, e[0], c.rights(c.R_READ));
    const wo = try rig.k.derive(sp, e[0], c.rights(c.R_WRITE));
    var buf: [64]u8 = undefined;
    var out: ipc.Received = undefined;
    try testing.expectError(error.AccessDenied, rig.ipc.send(sp, ro, c.msg(&buf, 1, "x", 0), &.{}, .{}, null));
    try testing.expectError(error.AccessDenied, rig.ipc.recv(sp, wo, .{}, &out));
    try testing.expectError(error.AccessDenied, rig.ipc.closeEnd(sp, ro));
    try testing.expectError(error.AccessDenied, rig.ipc.endStats(sp, wo));
    try rig.ipc.send(sp, wo, c.msg(&buf, 1, "x", 0), &.{}, .{}, null);
    try rig.ipc.recv(sp, e[1], .{}, &out);
    out.deinit(rig.ipc);
    try rig.consistent();
}

test "IPC-04 wrong object type and stale handle are InvalidHandle" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const sp = try rig.space();
    const e = try rig.ipc.createChannel(sp);
    const mem = try rig.k.createObject(sp, .memory_object, c.rights(c.R_ALL), .{});
    var buf: [64]u8 = undefined;
    try testing.expectError(error.InvalidHandle, rig.ipc.send(sp, mem, c.msg(&buf, 1, "x", 0), &.{}, .{}, null));
    const d = try rig.k.derive(sp, e[0], c.rights(c.R_ALL & ~@as(u32, 4)));
    try rig.k.release(sp, d);
    try testing.expectError(error.InvalidHandle, rig.ipc.send(sp, d, c.msg(&buf, 1, "x", 0), &.{}, .{}, null));
    try testing.expectError(error.InvalidHandle, rig.ipc.send(sp, .invalid, c.msg(&buf, 1, "x", 0), &.{}, .{}, null));
}

test "IPC-05 queue message limit is deterministic: WouldBlock, no overflow, nothing lost" {
    var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 3 });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const sp = try rig.space();
    const e = try rig.ipc.createChannel(sp);
    try send1(&rig, sp, e[0], "a");
    try send1(&rig, sp, e[0], "b");
    try send1(&rig, sp, e[0], "c");
    try testing.expectError(error.WouldBlock, send1(&rig, sp, e[0], "d"));
    try testing.expectError(error.WouldBlock, send1(&rig, sp, e[0], "e"));
    const st = try rig.ipc.endStats(sp, e[1]);
    try testing.expectEqual(@as(u32, 3), st.queued_messages);
    try testing.expectEqual(@as(u32, 0), st.reserved_messages);
    var out: ipc.Received = undefined;
    for ("abc") |ch| {
        try rig.ipc.recv(sp, e[1], .{}, &out);
        try testing.expectEqual(ch, out.payload[0]);
        out.deinit(rig.ipc);
    }
    try send1(&rig, sp, e[0], "d"); // room again
    try rig.consistent();
}

test "IPC-06 queue byte limit is deterministic" {
    var rig = try c.Rig.init(testing.allocator, .{ .max_messages = 8, .max_queue_bytes = 4096 });
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const sp = try rig.space();
    const e = try rig.ipc.createChannel(sp);
    var payload: [1000]u8 = undefined;
    @memset(&payload, 7);
    try send1(&rig, sp, e[0], &payload);
    try send1(&rig, sp, e[0], &payload);
    try send1(&rig, sp, e[0], &payload); // 3 * 1032 = 3096
    try testing.expectError(error.WouldBlock, send1(&rig, sp, e[0], &payload)); // 4128 > 4096
    try send1(&rig, sp, e[0], "x"); // small one still fits
    const st = try rig.ipc.endStats(sp, e[1]);
    try testing.expectEqual(@as(u64, 3096 + 33), st.queued_bytes);
    try rig.consistent();
}

test "IPC-07 wire validation: truncated, bad version, bad count, oversize" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const sp = try rig.space();
    const e = try rig.ipc.createChannel(sp);
    var buf: [128]u8 = undefined;
    const good = c.msg(&buf, 1, "hello", 0);
    try testing.expectError(error.InvalidArgument, rig.ipc.send(sp, e[0], good[0 .. good.len - 1], &.{}, .{}, null));
    try testing.expectError(error.InvalidArgument, rig.ipc.send(sp, e[0], good[0..10], &.{}, .{}, null));
    var bad = buf;
    std.mem.writeInt(u16, bad[4..6], 0x0002, .little);
    try testing.expectError(error.BadVersion, rig.ipc.send(sp, e[0], bad[0..good.len], &.{}, .{}, null));
    // header claims one capability but none is supplied
    const claims = c.msg(&buf, 1, "hello", 1);
    try testing.expectError(error.InvalidArgument, rig.ipc.send(sp, e[0], claims, &.{}, .{}, null));
    // nothing was published by any failure
    const st = try rig.ipc.endStats(sp, e[1]);
    try testing.expectEqual(@as(u32, 0), st.queued_messages);
    try testing.expectEqual(@as(u32, 0), st.reserved_messages);
}

test "IPC-08 config limits are validated" {
    try testing.expectError(error.InvalidArgument, (ipc.Config{ .max_messages = 0 }).validate());
    try testing.expectError(error.InvalidArgument, (ipc.Config{ .max_queue_bytes = 100 }).validate());
    try testing.expectError(error.InvalidArgument, (ipc.Config{ .wire = .{ .max_handles = 9 } }).validate());
    try testing.expectError(error.InvalidArgument, (ipc.Config{ .wire = .{ .max_message_bytes = 8 } }).validate());
    try (ipc.Config{}).validate();
    try testing.expectEqual(@as(u64, 9) * std.math.maxInt(u32), try (ipc.Config{ .max_messages = std.math.maxInt(u32) }).holdHandles());
}

test "IPC-09 queued bytes are charged to the sender and refunded on receive" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    const sp = try rig.space();
    const e = try rig.ipc.createChannel(sp);
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(sp, .queued_bytes));
    try send1(&rig, sp, e[0], "12345678"); // 40 bytes
    try testing.expectEqual(@as(u64, 40), try rig.k.spaceUsed(sp, .queued_bytes));
    try send1(&rig, sp, e[0], "1");
    try testing.expectEqual(@as(u64, 40 + 33), try rig.k.spaceUsed(sp, .queued_bytes));
    var out: ipc.Received = undefined;
    try rig.ipc.recv(sp, e[1], .{}, &out);
    out.deinit(rig.ipc);
    try testing.expectEqual(@as(u64, 33), try rig.k.spaceUsed(sp, .queued_bytes));
    try rig.ipc.recv(sp, e[1], .{}, &out);
    out.deinit(rig.ipc);
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(sp, .queued_bytes));
    try rig.consistent();
}

test "IPC-10 sender quota exhaustion fails the send with nothing queued, then recovers" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    var budget = c.space_budget;
    budget.queued_bytes = 80;
    const sp = try rig.k.createSpace(budget);
    const e = try rig.ipc.createChannel(sp);
    try send1(&rig, sp, e[0], "12345678"); // 40
    try send1(&rig, sp, e[0], "12345678"); // 80
    try testing.expectError(error.QuotaExceeded, send1(&rig, sp, e[0], "1"));
    const st = try rig.ipc.endStats(sp, e[1]);
    try testing.expectEqual(@as(u32, 2), st.queued_messages);
    try testing.expectEqual(@as(u32, 0), st.reserved_messages);
    var out: ipc.Received = undefined;
    try rig.ipc.recv(sp, e[1], .{}, &out);
    out.deinit(rig.ipc);
    try send1(&rig, sp, e[0], "12345678");
    try rig.consistent();
}

test "IPC-11 channel creation is charged and all-or-nothing" {
    var rig = try c.Rig.init(testing.allocator, .{});
    defer testing.expectEqual(@as(usize, 0), rig.deinit()) catch @panic("leak");
    var budget = c.space_budget;
    budget.channels = 3; // room for one channel (2 ends), not two
    const sp = try rig.k.createSpace(budget);
    const e = try rig.ipc.createChannel(sp);
    try testing.expectEqual(@as(u64, 2), try rig.k.spaceUsed(sp, .channels));
    try testing.expectError(error.QuotaExceeded, rig.ipc.createChannel(sp));
    try testing.expectEqual(@as(u64, 2), try rig.k.spaceUsed(sp, .channels));
    try rig.k.release(sp, e[0]);
    try rig.k.release(sp, e[1]);
    try testing.expectEqual(@as(u64, 0), try rig.k.spaceUsed(sp, .channels));
    _ = try rig.ipc.createChannel(sp); // works again
    try rig.consistent();
}
