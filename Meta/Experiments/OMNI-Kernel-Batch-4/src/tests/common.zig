//! Shared helpers for the Batch 4 tests.

const std = @import("std");
pub const ipc = @import("../ipc.zig");
pub const k3 = @import("kernel3");
pub const Ipc = ipc.Ipc;
pub const Kernel = k3.Kernel;
pub const SpaceId = k3.SpaceId;
pub const Handle = k3.Handle;
pub const ObjectId = k3.ObjectId;
pub const Rights = k3.Rights;
pub const ObjectType = k3.ObjectType;
pub const Counters = k3.Counters;
pub const Resource = k3.Resource;
pub const OmniError = k3.OmniError;

pub const R_READ: u32 = 1;
pub const R_WRITE: u32 = 2;
pub const R_DUP: u32 = 8;
pub const R_XFER: u32 = 16;
pub const R_WAIT: u32 = 32;
pub const R_SIGNAL: u32 = 64;
pub const R_ADMIN: u32 = 128;
pub const R_INSPECT: u32 = 256;
pub const R_ALL: u32 = 0x1FF;

pub fn rights(bits: u32) Rights {
    return Rights.fromRaw(bits) catch @panic("test bug: invalid rights bits");
}

pub const big: Counters = .{ .pages = 1_000_000, .handles = 100_000, .threads = 1000, .channels = 1000, .queued_bytes = 1 << 30 };
pub const space_budget: Counters = .{ .pages = 10_000, .handles = 10_000, .threads = 10, .channels = 10, .queued_bytes = 1 << 20 };

pub fn newKernel(allocator: std.mem.Allocator) !*Kernel {
    return Kernel.create(allocator, .{ .max_spaces = 256, .max_objects = 4096, .max_nodes = 16384, .max_pins = 4096, .root = big });
}

/// Kernel + Ipc pair with orderly teardown: kernel first, then Ipc (leak check).
pub const Rig = struct {
    k: *Kernel,
    ipc: *Ipc,

    pub fn init(allocator: std.mem.Allocator, cfg: ipc.Config) !Rig {
        return initWith(allocator, allocator, cfg);
    }

    /// Separate allocators so tests can inject failures into one layer only.
    pub fn initWith(kernel_alloc: std.mem.Allocator, ipc_alloc: std.mem.Allocator, cfg: ipc.Config) !Rig {
        const k = try newKernel(kernel_alloc);
        errdefer k.destroy();
        const x = try Ipc.create(ipc_alloc, k, cfg);
        return .{ .k = k, .ipc = x };
    }

    /// Returns the number of leaked registry entries (expected 0).
    pub fn deinit(r: *Rig) usize {
        r.k.destroy();
        return r.ipc.destroy();
    }

    pub fn space(r: *Rig) !SpaceId {
        return r.k.createSpace(space_budget);
    }

    pub fn consistent(r: *Rig) !void {
        if (r.k.checkInvariants()) |why| {
            std.debug.print("KERNEL INVARIANT VIOLATED: {s}\n", .{why});
            return error.InvariantViolated;
        }
        if (r.ipc.checkInvariants()) |why| {
            std.debug.print("IPC INVARIANT VIOLATED: {s}\n", .{why});
            return error.InvariantViolated;
        }
    }
};

/// Encode a message with `n_handles` declared and `payload`.
pub fn msg(buf: []u8, op: u16, payload: []const u8, n_handles: u32) []const u8 {
    const len = ipc.wire.encode(buf, .{ .op = op, .kind = .send, .handle_count = n_handles }, payload, .{}) catch @panic("test bug: encode");
    return buf[0..len];
}

pub fn moveDesc(h: Handle, t: ObjectType, r: u32) ipc.Desc {
    return .{ .handle = h, .otype = t, .mode = .move, .rights = rights(r) };
}

pub fn copyDesc(h: Handle, t: ObjectType, r: u32) ipc.Desc {
    return .{ .handle = h, .otype = t, .mode = .copy, .rights = rights(r) };
}

/// Counts destructor calls per object (for exactly-once checks).
pub const Tracker = struct {
    pub const max_ids = 16384;
    lock: std.Thread.Mutex = .{},
    counts: [max_ids]u8 = [_]u8{0} ** max_ids,
    total: u32 = 0,

    pub fn dtor(ctx: ?*anyopaque, id: ObjectId) void {
        const t: *Tracker = @ptrCast(@alignCast(ctx.?));
        t.lock.lock();
        defer t.lock.unlock();
        const i: usize = @intCast(id.raw());
        if (i < max_ids) t.counts[i] +|= 1;
        t.total += 1;
    }

    pub fn count(t: *Tracker, id: ObjectId) u8 {
        t.lock.lock();
        defer t.lock.unlock();
        return t.counts[@intCast(id.raw())];
    }

    pub fn opts(t: *Tracker) k3.CreateOptions {
        return .{ .destroy_fn = dtor, .destroy_ctx = t };
    }
};

pub fn idOf(k: *Kernel, space: SpaceId, h: Handle, t: ObjectType) !ObjectId {
    return (try k.lookup(space, h, t, Rights.none)).object;
}
