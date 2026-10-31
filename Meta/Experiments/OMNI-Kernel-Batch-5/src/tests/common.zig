//! Shared helpers for the Batch 5 tests.

const std = @import("std");
pub const kipc = @import("../kernel_ipc.zig");
pub const sys = @import("../syscall.zig");
pub const hum = @import("../hosted_user_memory.zig");
pub const wq = @import("../wait_queue.zig");
pub const acct = @import("../accounting.zig");
pub const ipc4 = @import("ipc4");
pub const k3 = @import("kernel3");
pub const b2 = @import("batch2");

pub const Kernel = k3.Kernel;
pub const SpaceId = k3.SpaceId;
pub const Handle = k3.Handle;
pub const Rights = k3.Rights;
pub const OmniError = k3.OmniError;
pub const KernelIpc = kipc.KernelIpc;
pub const ms = std.time.ns_per_ms;

pub const R_READ: u32 = 1;
pub const R_WRITE: u32 = 2;
pub const R_DUP: u32 = 8;
pub const R_XFER: u32 = 16;
pub const R_WAIT: u32 = 32;
pub const R_SIGNAL: u32 = 64;
pub const R_ADMIN: u32 = 128;
pub const R_INSPECT: u32 = 256;
pub const R_ALL: u32 = 0x1FF;
pub const R_END: u32 = 0x1FB; // everything but EXECUTE: the rights of a fresh endpoint

pub fn rights(bits: u32) Rights {
    return Rights.fromRaw(bits) catch @panic("test bug: invalid rights bits");
}

pub const big: k3.Counters = .{ .pages = 1_000_000, .handles = 100_000, .threads = 1000, .channels = 1000, .queued_bytes = 1 << 30 };
pub const space_budget: k3.Counters = .{ .pages = 10_000, .handles = 10_000, .threads = 10, .channels = 10, .queued_bytes = 1 << 20 };

/// Kernel + KernelIpc with orderly teardown: kernel first, then KernelIpc (leak check).
pub const Rig = struct {
    k: *Kernel,
    x: *KernelIpc,

    pub fn init(allocator: std.mem.Allocator, cfg: kipc.Config) !Rig {
        return initWith(allocator, allocator, cfg);
    }

    /// Separate allocators so tests can inject failures into one layer only.
    pub fn initWith(kernel_alloc: std.mem.Allocator, ipc_alloc: std.mem.Allocator, cfg: kipc.Config) !Rig {
        const k = try Kernel.create(kernel_alloc, .{ .max_spaces = 256, .max_objects = 4096, .max_nodes = 16384, .max_pins = 4096, .root = big });
        errdefer k.destroy();
        const x = try KernelIpc.create(ipc_alloc, k, cfg);
        return .{ .k = k, .x = x };
    }

    /// Returns the number of leaked registry entries (expected 0).
    pub fn deinit(r: *Rig) usize {
        r.k.destroy();
        return r.x.destroy();
    }

    pub fn space(r: *Rig) !SpaceId {
        return r.k.createSpace(space_budget);
    }

    pub fn consistent(r: *Rig) !void {
        if (r.x.audit()) |why| {
            std.debug.print("AUDIT VIOLATION: {s}\n", .{why});
            return error.InvariantViolated;
        }
    }
};

/// Encode a wire message.
pub fn msg(buf: []u8, op: u16, payload: []const u8, n_handles: u32) []const u8 {
    const len = b2.ipc.encode(buf, .{ .op = op, .kind = .send, .handle_count = n_handles }, payload, .{}) catch @panic("test bug: encode");
    return buf[0..len];
}

pub fn moveDesc(h: Handle, t: k3.ObjectType, r: u32) ipc4.Desc {
    return .{ .handle = h, .otype = t, .mode = .move, .rights = rights(r) };
}

pub fn copyDesc(h: Handle, t: k3.ObjectType, r: u32) ipc4.Desc {
    return .{ .handle = h, .otype = t, .mode = .copy, .rights = rights(r) };
}

pub const Pair = struct { a: SpaceId, b: SpaceId, a0: Handle, b1: Handle };

/// Two spaces, a channel created in A, its second end moved to B.
pub fn pair(rig: *Rig) !Pair {
    const a = try rig.space();
    const b = try rig.space();
    const e = try rig.x.endpointCreate(a);
    const b1 = try rig.k.transfer(a, e[1], b);
    return .{ .a = a, .b = b, .a0 = e[0], .b1 = b1 };
}

pub fn sendPoll(rig: *Rig, sp: SpaceId, h: Handle, payload: []const u8) !void {
    var buf: [4096]u8 = undefined;
    try rig.x.send(sp, h, msg(&buf, 1, payload, 0), &.{}, .{}, null);
}

/// Counts destructor calls per object (exact-once checks).
pub const Tracker = struct {
    pub const max_ids = 16384;
    lock: std.Thread.Mutex = .{},
    counts: [max_ids]u8 = [_]u8{0} ** max_ids,

    pub fn dtor(ctx: ?*anyopaque, id: k3.ObjectId) void {
        const t: *Tracker = @ptrCast(@alignCast(ctx.?));
        t.lock.lock();
        defer t.lock.unlock();
        const i: usize = @intCast(id.raw());
        if (i < max_ids) t.counts[i] +|= 1;
    }

    pub fn count(t: *Tracker, id: k3.ObjectId) u8 {
        t.lock.lock();
        defer t.lock.unlock();
        return t.counts[@intCast(id.raw())];
    }

    pub fn opts(t: *Tracker) k3.CreateOptions {
        return .{ .destroy_fn = dtor, .destroy_ctx = t };
    }
};

pub fn idOf(k: *Kernel, space: SpaceId, h: Handle, t: k3.ObjectType) !k3.ObjectId {
    return (try k.lookup(space, h, t, Rights.none)).object;
}

/// A simulated user process: memory plus helpers that build syscall structures.
pub const User = struct {
    mem: hum.HostedUserMemory = .{},
    // One page each: arguments, message, descriptors, receive buffer, receive result, misc.
    args: [64]u8 align(8) = undefined,
    msgb: [4096]u8 = undefined,
    descs: [8 * sys.desc_size]u8 = undefined,
    rbuf: [4096]u8 = undefined,
    rres: [sys.recv_result_size]u8 = undefined,
    misc: [64]u8 = undefined,

    pub const A_ARGS: u64 = 0x1000;
    pub const A_MSG: u64 = 0x2000;
    pub const A_DESC: u64 = 0x10000;
    pub const A_RBUF: u64 = 0x20000;
    pub const A_RRES: u64 = 0x30000;
    pub const A_MISC: u64 = 0x40000;

    pub fn setup(u: *User) !void {
        @memset(&u.args, 0);
        @memset(&u.msgb, 0);
        @memset(&u.descs, 0);
        @memset(&u.rbuf, 0);
        @memset(&u.rres, 0);
        @memset(&u.misc, 0);
        try u.mem.map(A_ARGS, &u.args, true);
        try u.mem.map(A_MSG, &u.msgb, true);
        try u.mem.map(A_DESC, &u.descs, true);
        try u.mem.map(A_RBUF, &u.rbuf, true);
        try u.mem.map(A_RRES, &u.rres, true);
        try u.mem.map(A_MISC, &u.misc, true);
    }

    pub fn call(u: *User, rig: *Rig, caller: SpaceId, nr: sys.Nr, a: [6]u64) i64 {
        return sys.dispatch(rig.x, u.mem.memory(), caller, .{ .nr = @intFromEnum(nr), .a = a });
    }

    pub fn callRaw(u: *User, rig: *Rig, caller: SpaceId, nr: u32, a: [6]u64) i64 {
        return sys.dispatch(rig.x, u.mem.memory(), caller, .{ .nr = nr, .a = a });
    }

    /// Fill the send argument block at A_ARGS and the message at A_MSG.
    pub fn prepSend(u: *User, wire: []const u8, ndesc: u32, timeout_ns: u64, cancel: u32) void {
        @memcpy(u.msgb[0..wire.len], wire);
        const w = std.mem.writeInt;
        w(u64, u.args[0..8], A_MSG, .little);
        w(u32, u.args[8..12], @intCast(wire.len), .little);
        w(u32, u.args[12..16], ndesc, .little);
        w(u64, u.args[16..24], A_DESC, .little);
        w(u64, u.args[24..32], timeout_ns, .little);
        w(u32, u.args[32..36], cancel, .little);
        w(u32, u.args[36..40], 0, .little);
        w(u64, u.args[40..48], 0, .little);
    }

    pub fn putDesc(u: *User, i: usize, d: ipc4.Desc) void {
        const w = std.mem.writeInt;
        const o = i * sys.desc_size;
        w(u32, u.descs[o..][0..4], @intFromEnum(d.handle), .little);
        u.descs[o + 4] = @intFromEnum(d.otype);
        u.descs[o + 5] = @intFromEnum(d.mode);
        w(u16, u.descs[o + 6 ..][0..2], 0, .little);
        w(u32, u.descs[o + 8 ..][0..4], d.rights.toRaw(), .little);
        w(u32, u.descs[o + 12 ..][0..4], 0, .little);
    }

    pub fn getDescHandle(u: *User, i: usize) u32 {
        return std.mem.readInt(u32, u.descs[i * sys.desc_size ..][0..4], .little);
    }

    pub fn prepRecv(u: *User, timeout_ns: u64, cancel: u32) void {
        const w = std.mem.writeInt;
        w(u64, u.args[0..8], A_RBUF, .little);
        w(u32, u.args[8..12], u.rbuf.len, .little);
        w(u32, u.args[12..16], 0, .little);
        w(u64, u.args[16..24], A_RRES, .little);
        w(u64, u.args[24..32], timeout_ns, .little);
        w(u32, u.args[32..36], cancel, .little);
        w(u32, u.args[36..40], 0, .little);
        w(u64, u.args[40..48], 0, .little);
    }

    pub fn resPayloadLen(u: *User) u32 {
        return std.mem.readInt(u32, u.rres[0..4], .little);
    }
    pub fn resCount(u: *User) u32 {
        return std.mem.readInt(u32, u.rres[20..24], .little);
    }
    pub fn resSlotStatus(u: *User, i: usize) u32 {
        return std.mem.readInt(u32, u.rres[24 + i * 8 ..][0..4], .little);
    }
    pub fn resSlotHandle(u: *User, i: usize) u32 {
        return std.mem.readInt(u32, u.rres[28 + i * 8 ..][0..4], .little);
    }
};

pub fn ok(v: i64) !void {
    if (v < 0) {
        std.debug.print("unexpected syscall status {d}\n", .{v});
        return error.UnexpectedStatus;
    }
}

pub fn st(e: sys.SysError) i64 {
    return sys.statusOf(e);
}

pub const Env = struct {
    rig: Rig,
    p: Pair,
    ua: User = .{},
    ub: User = .{},

    pub fn init(self: *Env, cfg: kipc.Config) !void {
        try self.initWith(std.testing.allocator, std.testing.allocator, cfg);
    }

    pub fn initWith(self: *Env, ka: std.mem.Allocator, ia: std.mem.Allocator, cfg: kipc.Config) !void {
        self.rig = try Rig.initWith(ka, ia, cfg);
        self.p = try pair(&self.rig);
        self.ua = .{};
        self.ub = .{};
        try self.ua.setup();
        try self.ub.setup();
    }

    pub fn deinit(self: *Env) !void {
        try std.testing.expectEqual(@as(usize, 0), self.rig.deinit());
    }

    pub fn send(self: *Env, wire: []const u8, descs: []const ipc4.Desc, timeout: u64, cancel: u32) i64 {
        for (descs, 0..) |d, i| self.ua.putDesc(i, d);
        self.ua.prepSend(wire, @intCast(descs.len), timeout, cancel);
        return self.ua.call(&self.rig, self.p.a, .send, .{ @intFromEnum(self.p.a0), User.A_ARGS, 0, 0, 0, 0 });
    }

    pub fn recv(self: *Env, timeout: u64, cancel: u32) i64 {
        self.ub.prepRecv(timeout, cancel);
        return self.ub.call(&self.rig, self.p.b, .recv, .{ @intFromEnum(self.p.b1), User.A_ARGS, 0, 0, 0, 0 });
    }

    pub fn queued(self: *Env) !u32 {
        return (try self.rig.x.endpointStats(self.p.b, self.p.b1)).queued_messages;
    }
};
