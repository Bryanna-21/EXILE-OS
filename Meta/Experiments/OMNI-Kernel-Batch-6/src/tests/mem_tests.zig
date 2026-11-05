//! User-memory integration tests. SIM: `PageModel` is a hosted page-table stand-in.
const std = @import("std");
const c = @import("common.zig");
const um = c.um;
const testing = std.testing;
const Rig = c.Rig;

const sp1 = c.SpaceId{ .index = 1, .gen = 1 };
const sp2 = c.SpaceId{ .index = 2, .gen = 1 };

fn newModel(space: c.SpaceId) !*c.PageModel {
    const m = try testing.allocator.create(c.PageModel);
    m.* = c.PageModel.init(space, um.user_top_x86_64);
    return m;
}

test "UM-01 SIM mapping rules, multi-page copies and permissions" {
    const m = try newModel(sp1);
    defer testing.allocator.destroy(m);
    try testing.expectError(error.BadRange, m.map(0, 4096, true, true)); // null page
    try testing.expectError(error.BadRange, m.map(0x1001, 4096, true, true)); // unaligned
    try testing.expectError(error.BadRange, m.map(0x1000, 0, true, true));
    try testing.expectError(error.BadRange, m.map(0x1000, std.math.maxInt(u64), true, true)); // overflow
    try testing.expectError(error.BadRange, m.map(um.user_top_x86_64 - 4096, 8192, true, true)); // beyond user space
    try m.map(0x10000, 3 * 4096, true, true);
    try testing.expectError(error.Overlap, m.map(0x11000, 4096, true, true));
    try testing.expectError(error.TooManyPages, m.map(0x100000, 65 * 4096, true, true));

    var pat: [3 * 4096]u8 = undefined;
    for (&pat, 0..) |*b, i| b.* = @truncate(i *% 7);
    try m.userStore(0x10000, &pat);
    // a copy that straddles two page boundaries
    var got: [5000]u8 = undefined;
    try m.copyIn(0x10000 + 4000, &got);
    try testing.expectEqualSlices(u8, pat[4000..9000], &got);
    @memset(&got, 0xEE);
    try m.copyOut(0x10000 + 3000, &got);
    var back: [5000]u8 = undefined;
    try m.userLoad(0x10000 + 3000, &back);
    try testing.expectEqualSlices(u8, &got, &back);

    // permissions
    try m.map(0x20000, 4096, true, false); // read-only
    try m.map(0x30000, 4096, false, true); // write-only
    var one: [8]u8 = undefined;
    try m.copyIn(0x20000, &one);
    try testing.expectError(error.Fault, m.copyOut(0x20000, &one));
    try testing.expectError(error.Fault, m.probeWrite(0x20000, 8));
    try testing.expectError(error.Fault, m.copyIn(0x30000, &one));
    try m.copyOut(0x30000, &one);
    // not present / unmapped
    m.setPresent(0x10000, false);
    try testing.expectError(error.Fault, m.copyIn(0x10000, &one));
    try testing.expectError(error.Fault, m.probeWrite(0x10000, 1));
    m.setPresent(0x10000, true);
    m.unmap(0x20000);
    try testing.expectError(error.Fault, m.copyIn(0x20000, &one));
    // zero-length copies always succeed and touch nothing
    try m.copyIn(0x9999_0000, one[0..0]);
    try m.copyOut(0x9999_0000, one[0..0]);
    try m.probeWrite(0x9999_0000, 0);
}

test "UM-02 SIM kernel addresses, the top of user space and address wrap-around are refused" {
    const m = try newModel(sp1);
    defer testing.allocator.destroy(m);
    try m.map(um.user_top_x86_64 - 4096, 4096, true, true);
    var b: [16]u8 = undefined;
    try m.copyIn(um.user_top_x86_64 - 16, &b); // last 16 bytes of user space are fine
    try testing.expectError(error.Fault, m.copyIn(um.user_top_x86_64 - 8, &b)); // crosses the top
    try testing.expectError(error.Fault, m.copyIn(um.user_top_x86_64, b[0..1]));
    try testing.expectError(error.Fault, m.copyIn(0xffff_8000_0000_0000, b[0..1])); // kernel half
    try testing.expectError(error.Fault, m.copyIn(std.math.maxInt(u64) - 3, &b)); // addr + len wraps
    try testing.expectError(error.Fault, m.copyOut(std.math.maxInt(u64), b[0..2]));
    try testing.expectError(error.Fault, m.probeWrite(std.math.maxInt(u64) - 1, 8));
    try testing.expectError(error.Fault, m.probeWrite(0x1000, std.math.maxInt(u64))); // huge length
    try testing.expect(m.faults >= 7);
}

test "UM-03 SIM copy_out probes every page first: a fault found by the probe changes nothing" {
    const m = try newModel(sp1);
    defer testing.allocator.destroy(m);
    try m.map(0x10000, 4096, true, true);
    try m.map(0x11000, 4096, true, false); // second page read-only
    var data: [200]u8 = undefined;
    @memset(&data, 0xAB);
    try testing.expectError(error.Fault, m.copyOut(0x10000 + 4096 - 100, &data));
    var first: [100]u8 = undefined;
    try m.userLoad(0x10000 + 4096 - 100, &first);
    for (first) |x| try testing.expectEqual(@as(u8, 0), x); // untouched
    try testing.expectEqual(@as(u64, 0), m.partial_bytes_written);
}

const Race = struct { calls: u32 = 0, unmap_at: u32, addr: u64 };

fn raceHook(ctx: *anyopaque, m: *c.PageModel) void {
    const rc: *Race = @ptrCast(@alignCast(ctx));
    rc.calls += 1;
    if (rc.calls == rc.unmap_at) m.unmap(rc.addr);
}

test "UM-04 SIM a page unmapped after the probe: reported as a fault, the partial write is counted" {
    const m = try newModel(sp1);
    defer testing.allocator.destroy(m);
    try m.map(0x10000, 2 * 4096, true, true);
    var rc = Race{ .unmap_at = 1, .addr = 0x11000 };
    m.after_probe = raceHook;
    m.after_probe_ctx = &rc;
    var data: [300]u8 = undefined;
    @memset(&data, 0xCD);
    try testing.expectError(error.Fault, m.copyOut(0x10000 + 4096 - 100, &data));
    try testing.expectEqual(@as(u64, 100), m.partial_bytes_written); // the first page was written
    var w: [100]u8 = undefined;
    try m.userLoad(0x10000 + 4096 - 100, &w);
    try testing.expectEqual(@as(u8, 0xCD), w[0]); // the documented consequence: the range is unspecified
    try testing.expectEqual(@as(u32, 1), rc.calls);
}

test "UM-05 SIM address-space ownership: a binding or registry entry for another space is refused" {
    const m1 = try newModel(sp1);
    defer testing.allocator.destroy(m1);
    try testing.expectError(error.WrongAddressSpace, um.Binding.init(sp2, m1));
    var b = try um.Binding.init(sp1, m1);
    try m1.map(0x1000, 4096, true, true);
    const mem = b.memory();
    var x: [4]u8 = undefined;
    try mem.copy_in(mem.ctx, 0x1000, &x);
    // the page model is re-owned after the bind: every operation now fails closed
    m1.owner = sp2;
    try testing.expectError(error.Fault, mem.copy_in(mem.ctx, 0x1000, &x));
    try testing.expectError(error.Fault, mem.copy_out(mem.ctx, 0x1000, &x));
    try testing.expectError(error.Fault, mem.probe_write(mem.ctx, 0x1000, 4));

    var reg = um.Registry{};
    m1.owner = sp1;
    try reg.register(sp1, m1);
    try testing.expectError(error.AlreadyRegistered, reg.register(sp1, m1));
    try testing.expectError(error.WrongAddressSpace, reg.register(sp2, m1));
    try testing.expectEqual(m1, reg.modelOf(sp1).?);
    try testing.expectEqual(@as(?*c.PageModel, null), reg.modelOf(sp2));
    reg.unregister(sp1);
    try testing.expectEqual(@as(?*c.PageModel, null), reg.modelOf(sp1));
    // capacity
    var models: [um.max_spaces + 1]*c.PageModel = undefined;
    var n: usize = 0;
    defer for (models[0..n]) |mm| testing.allocator.destroy(mm);
    for (0..um.max_spaces + 1) |i| {
        models[i] = try newModel(c.SpaceId{ .index = @intCast(10 + i), .gen = 1 });
        n += 1;
    }
    for (0..um.max_spaces) |i| try reg.register(models[i].owner, models[i]);
    try testing.expectError(error.TooManySpaces, reg.register(models[um.max_spaces].owner, models[um.max_spaces]));
}

fn setArgs(p: c.Proc, msg_ptr: u64, len: u32, nd: u32, desc_ptr: u64) !void {
    var a: [48]u8 = [_]u8{0} ** 48;
    std.mem.writeInt(u64, a[0..8], msg_ptr, .little);
    std.mem.writeInt(u32, a[8..12], len, .little);
    std.mem.writeInt(u32, a[12..16], nd, .little);
    std.mem.writeInt(u64, a[16..24], desc_ptr, .little);
    try p.model.userStore(c.A_ARGS, &a);
}

test "UM-06 SIM buffers that span pages work through the bridge" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    _ = try r.spawnOn(p.a, 0);
    _ = try r.spawnOn(p.b, 1);
    try p.a.model.map(0x3000, 4096, true, true); // the page after A_MSG
    var buf: [64]u8 = undefined;
    const wire = c.msg(&buf, 1, "straddling", 0);
    const start: u64 = 0x2000 + 4096 - 20; // 20 bytes in the first page, the rest in the next
    try p.a.model.userStore(start, wire);
    try setArgs(p.a, start, @intCast(wire.len), 0, c.A_DESC);
    try testing.expectEqual(@as(i64, 0), try c.call(r, 0, .send, .{ @intFromEnum(p.a0), c.A_ARGS, 0, 0, 0, 0 }));
    // receive into a buffer that straddles too
    try p.b.model.map(0x21000, 4096, true, true);
    var a: [48]u8 = [_]u8{0} ** 48;
    std.mem.writeInt(u64, a[0..8], 0x20000 + 4096 - 4, .little);
    std.mem.writeInt(u32, a[8..12], 4096, .little);
    std.mem.writeInt(u64, a[16..24], c.A_RRES, .little);
    try p.b.model.userStore(c.A_ARGS, &a);
    try testing.expectEqual(@as(i64, 0), try c.call(r, 1, .recv, .{ @intFromEnum(p.b1), c.A_ARGS, 0, 0, 0, 0 }));
    var got: [10]u8 = undefined;
    try p.b.model.userLoad(0x20000 + 4096 - 4, &got);
    try testing.expectEqualStrings("straddling", &got);
    try r.audit();
    r.freeProc(p.a);
    r.freeProc(p.b);
    try testing.expectEqual(@as(usize, 0), r.destroy());
}

test "UM-07 SIM copy-in faults are -32 and have no effect; no authority is lost" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    _ = try r.spawnOn(p.a, 0);
    const obj = try r.k.createObject(p.a.space, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    const wire = c.msg(&buf, 1, "x", 1);
    try p.a.model.userStore(c.A_MSG, wire);
    const ep = @intFromEnum(p.a0);

    // argument block unreadable (unmapped, kernel address, wrapping)
    for ([_]u64{ 0x9000_0000, um.user_top_x86_64, std.math.maxInt(u64) - 8, 0 }) |bad| {
        try testing.expectEqual(@as(i64, -32), try c.call(r, 0, .send, .{ ep, bad, 0, 0, 0, 0 }));
    }
    // message pointer unreadable
    try setArgs(p.a, 0x9000_0000, @intCast(wire.len), 1, c.A_DESC);
    try testing.expectEqual(@as(i64, -32), try c.call(r, 0, .send, .{ ep, c.A_ARGS, 0, 0, 0, 0 }));
    // descriptor array unreadable: the capability named in it must survive
    try setArgs(p.a, c.A_MSG, @intCast(wire.len), 1, 0x9000_0000);
    try testing.expectEqual(@as(i64, -32), try c.call(r, 0, .send, .{ ep, c.A_ARGS, 0, 0, 0, 0 }));
    // message length running past the mapped page
    try setArgs(p.a, 0x2000 + 4096 - 8, 4096, 0, c.A_DESC);
    try testing.expectEqual(@as(i64, -32), try c.call(r, 0, .send, .{ ep, c.A_ARGS, 0, 0, 0, 0 }));
    try testing.expectEqual(@as(u32, 0), (try r.x.endpointStats(p.b.space, p.b1)).queued_messages);
    try r.k.release(p.a.space, obj); // still held: nothing was taken
    try r.audit();
    r.freeProc(p.a);
    r.freeProc(p.b);
    try testing.expectEqual(@as(usize, 0), r.destroy());
}

test "UM-08 SIM a fault after the message was consumed releases its capabilities and drops the message" {
    const r = try Rig.create(testing.allocator, .{});
    const p = try c.pair(r);
    const ta = try r.spawn(p.a);
    _ = ta;
    _ = try r.spawnOn(p.b, 1);
    const obj = try r.k.createObject(p.a.space, .memory_object, c.rights(c.R_READ | c.R_XFER), .{});
    var buf: [64]u8 = undefined;
    const tid_a = try r.spawn(p.a);
    try r.ip.send(tid_a, p.a0, c.msg(&buf, 1, "cap", 1), &.{c.moveDesc(obj, .memory_object, c.R_READ | c.R_XFER)}, .{}, null);
    const before = try r.k.spaceHandleCount(p.b.space);

    // the result page disappears between the probe and the second copy-out
    var rc = Race{ .unmap_at = 2, .addr = c.A_RRES };
    p.b.model.after_probe = raceHook;
    p.b.model.after_probe_ctx = &rc;
    try c.prepRecv(p.b, 0, 0);
    try testing.expectEqual(@as(i64, -32), try c.call(r, 1, .recv, .{ @intFromEnum(p.b1), c.A_ARGS, 0, 0, 0, 0 }));
    p.b.model.after_probe = null;
    try testing.expectEqual(before, try r.k.spaceHandleCount(p.b.space)); // delivered handle released
    try testing.expectEqual(@as(u32, 0), (try r.x.endpointStats(p.b.space, p.b1)).queued_messages); // consumed
    try testing.expectEqual(@as(u64, 1), r.x.ledger.snapshot().delivery_faults);
    try r.audit();
    r.freeProc(p.a);
    r.freeProc(p.b);
    try testing.expectEqual(@as(usize, 0), r.destroy());
}
