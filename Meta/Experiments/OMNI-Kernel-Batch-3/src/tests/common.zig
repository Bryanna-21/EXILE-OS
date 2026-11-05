//! Shared helpers for the Batch 3 tests.

const std = @import("std");
pub const kernel = @import("../kernel.zig");
pub const Kernel = kernel.Kernel;
pub const SpaceId = kernel.SpaceId;
pub const Handle = kernel.Handle;
pub const ObjectId = kernel.ObjectId;
pub const Rights = kernel.Rights;
pub const ObjectType = kernel.ObjectType;
pub const Counters = kernel.Counters;
pub const Resource = kernel.Resource;

pub const R_READ: u32 = 1;
pub const R_WRITE: u32 = 2;
pub const R_EXEC: u32 = 4;
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

/// A space budget small enough that many spaces fit under the root budget at any time.
pub const space_budget: Counters = .{ .pages = 10_000, .handles = 10_000, .threads = 10, .channels = 10, .queued_bytes = 1 << 20 };

pub fn bigLimits() kernel.Limits {
    return .{ .max_spaces = 64, .max_objects = 4096, .max_nodes = 16384, .max_pins = 4096, .root = big };
}

pub fn newKernel(allocator: std.mem.Allocator) !*Kernel {
    return Kernel.create(allocator, bigLimits());
}

/// Fails the test, printing the violation text, if any invariant is broken.
pub fn expectConsistent(k: *Kernel) !void {
    if (k.checkInvariants()) |why| {
        std.debug.print("INVARIANT VIOLATED: {s}\n", .{why});
        return error.InvariantViolated;
    }
}

/// Destructor bookkeeping: counts calls per object id and can probe the mutex.
pub const Tracker = struct {
    pub const max_ids = 16384;
    lock: std.Thread.Mutex = .{},
    counts: [max_ids]u8 = [_]u8{0} ** max_ids,
    total: u32 = 0,
    kernel_ptr: ?*Kernel = null,
    /// Set to true if a destructor ever ran while the kernel mutex was held.
    saw_lock_held: bool = false,
    probes: u32 = 0,
    /// Optional extra action run inside the destructor, after probing.
    extra: ?*const fn (t: *Tracker, id: ObjectId) void = null,

    pub fn dtor(ctx: ?*anyopaque, id: ObjectId) void {
        const t: *Tracker = @ptrCast(@alignCast(ctx.?));
        if (t.kernel_ptr) |k| {
            if (k.mutex.tryLock()) {
                k.mutex.unlock();
            } else {
                t.saw_lock_held = true;
            }
        }
        {
            t.lock.lock();
            defer t.lock.unlock();
            const i: usize = @intCast(id.raw());
            if (i < max_ids) {
                t.counts[i] +|= 1;
            }
            t.total += 1;
            t.probes += 1;
        }
        if (t.extra) |f| f(t, id);
    }

    pub fn count(t: *Tracker, id: ObjectId) u8 {
        t.lock.lock();
        defer t.lock.unlock();
        return t.counts[@intCast(id.raw())];
    }

    pub fn opts(t: *Tracker) kernel.CreateOptions {
        return .{ .destroy_fn = dtor, .destroy_ctx = t };
    }

    pub fn optsCharged(t: *Tracker, amount: u64) kernel.CreateOptions {
        return .{ .destroy_fn = dtor, .destroy_ctx = t, .charge_resource = .pages, .charge_amount = amount };
    }
};

pub fn idOf(k: *Kernel, space: SpaceId, h: Handle, t: ObjectType) !ObjectId {
    const r = try k.lookup(space, h, t, Rights.none);
    return r.object;
}
