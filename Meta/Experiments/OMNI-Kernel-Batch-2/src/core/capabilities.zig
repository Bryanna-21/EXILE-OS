//! Capability table and rights checking (Batch 2, hosted model).
//! Batch 1 references: A.6 (rights), B.1 (default deny), B.2 (capabilities).
//!
//! WHAT THIS IS: a fixed-capacity, allocation-free model of the per-process,
//! KERNEL-OWNED table that a real kernel would keep. User space would hold only
//! a `Handle` (index + generation). On every use the table checks, in order:
//!   1. the handle is not 0 and its index is within the table,
//!   2. the slot is live (not free, not retired),
//!   3. the handle's generation equals the slot's generation (stale -> reject),
//!   4. the object type matches what the caller expects,
//!   5. the entry's rights contain every required right.
//! Steps 1-4 fail with `InvalidHandle`; step 5 fails with `AccessDenied`.
//!
//! WHAT THIS IS NOT: a hosted table runs in an ordinary process. It provides NO
//! hardware-enforced privilege or memory isolation, and nothing stops a hosted
//! caller from touching the struct directly. It models the checking rules so
//! they can be tested deterministically; it is not a security boundary.
//!
//! RULES (tested):
//!   * Default deny: a handle conveys only the rights stored in its entry.
//!   * Unknown/reserved rights bits are rejected (`InvalidArgument`), never
//!     masked or ignored.
//!   * Derivation can only keep or REMOVE rights. It needs the DUPLICATE right on
//!     the source and the requested rights must be a subset of the source's.
//!   * Release invalidates the handle. The slot's generation is incremented; a
//!     slot whose generation would wrap is RETIRED permanently, so a stale
//!     handle can never become valid again (no ABA after wrap).
//!   * Every mutating operation is all-or-nothing: on error, table and account
//!     are unchanged.
//!
//! NOT IMPLEMENTED (deferred): revocation of derived handles (Batch 1 B.2
//! options R1-R3), handle transfer between tables, per-object reference counts,
//! destruction of the underlying object, concurrency control. The table is
//! SINGLE-THREADED.

const std = @import("std");
const OmniError = @import("error.zig").OmniError;
const ids = @import("ids.zig");
const resources = @import("resources.zig");

pub const Handle = ids.Handle;
pub const ObjectId = ids.ObjectId;

/// Rights from Batch 1 A.6. Bits 9..31 are reserved and must be zero.
pub const Rights = packed struct(u32) {
    read: bool = false,
    write: bool = false,
    execute: bool = false,
    duplicate: bool = false,
    transfer: bool = false,
    wait: bool = false,
    signal: bool = false,
    admin: bool = false,
    inspect: bool = false,
    _reserved: u23 = 0,

    pub const none: Rights = .{};
    pub const valid_mask: u32 = 0x1FF;

    /// Parse untrusted bits. Any bit outside `valid_mask` is rejected.
    pub fn fromRaw(bits: u32) OmniError!Rights {
        if ((bits & ~valid_mask) != 0) return error.InvalidArgument;
        return @bitCast(bits);
    }

    pub fn toRaw(self: Rights) u32 {
        return @bitCast(self);
    }

    /// True if no reserved bit is set (guards values built with @bitCast).
    pub fn isValid(self: Rights) bool {
        return (self.toRaw() & ~valid_mask) == 0;
    }

    /// True if `self` includes every right in `required`.
    pub fn contains(self: Rights, required: Rights) bool {
        return (required.toRaw() & ~self.toRaw()) == 0;
    }
};

/// Kernel object kinds referenced by capabilities (from Batch 1 C.3-C.7).
pub const ObjectType = enum(u8) {
    channel = 1,
    memory_object,
    notification,
    cancellation,
    process,
    thread,
    timer,
    service_endpoint,
};

/// Result of a successful check.
pub const Resolved = struct {
    object: ObjectId,
    otype: ObjectType,
    rights: Rights,
};

pub fn CapabilityTable(comptime capacity: u16) type {
    comptime std.debug.assert(capacity > 0);

    return struct {
        const Self = @This();

        const State = enum(u8) { free, live, retired };

        const Entry = struct {
            state: State = .free,
            /// Generation expected in handles for this slot. Never 0.
            generation: u16 = 1,
            object: ObjectId = .invalid,
            otype: ObjectType = .channel,
            rights: Rights = .none,
        };

        entries: [capacity]Entry = [_]Entry{.{}} ** capacity,
        live_count: u32 = 0,
        /// Optional resource account charged one `handles` unit per live entry.
        account: ?*resources.Account = null,

        pub fn init(account: ?*resources.Account) Self {
            return .{ .account = account };
        }

        pub fn liveCount(self: *const Self) u32 {
            return self.live_count;
        }

        /// Number of slots permanently retired by generation exhaustion.
        pub fn retiredCount(self: *const Self) u32 {
            var n: u32 = 0;
            for (self.entries) |e| {
                if (e.state == .retired) n += 1;
            }
            return n;
        }

        /// Create a new capability. Fails with `QuotaExceeded` when the table is
        /// full or the account's handle limit would be exceeded.
        pub fn insert(self: *Self, object: ObjectId, otype: ObjectType, rights: Rights) OmniError!Handle {
            if (!object.isValid()) return error.InvalidArgument;
            if (!rights.isValid()) return error.InvalidArgument;

            // 1. Find a slot without mutating anything.
            const slot = self.findFree() orelse return error.QuotaExceeded;
            // 2. Charge the account (all-or-nothing, may fail).
            if (self.account) |a| try a.reserve(.handles, 1);
            // 3. Commit. Cannot fail.
            const e = &self.entries[slot];
            e.state = .live;
            e.object = object;
            e.otype = otype;
            e.rights = rights;
            self.live_count += 1;
            return Handle.make(@intCast(slot), e.generation) catch unreachable; // generation != 0 by invariant
        }

        /// Check a handle against an expected type and required rights.
        pub fn lookup(self: *const Self, h: Handle, expected: ObjectType, required: Rights) OmniError!Resolved {
            if (!required.isValid()) return error.InvalidArgument;
            const e = try self.resolve(h);
            if (e.otype != expected) return error.InvalidHandle;
            if (!e.rights.contains(required)) return error.AccessDenied;
            return .{ .object = e.object, .otype = e.otype, .rights = e.rights };
        }

        /// Like `lookup`, but `required_bits` is untrusted. Unknown bits are
        /// rejected with `InvalidArgument` BEFORE the handle is examined.
        pub fn lookupRaw(self: *const Self, h: Handle, expected: ObjectType, required_bits: u32) OmniError!Resolved {
            const required = try Rights.fromRaw(required_bits);
            return self.lookup(h, expected, required);
        }

        /// Kernel-internal: return what a handle refers to WITHOUT requiring any
        /// rights or a specific type. Used by IPC validation, which then applies
        /// its own right checks. Must never be exposed as a user-facing operation.
        pub fn peek(self: *const Self, h: Handle) OmniError!Resolved {
            const e = try self.resolve(h);
            return .{ .object = e.object, .otype = e.otype, .rights = e.rights };
        }

        /// Create a new handle to the same object with a SUBSET of the source's
        /// rights. Requires DUPLICATE on the source. Cannot increase rights.
        pub fn derive(self: *Self, source: Handle, requested: Rights) OmniError!Handle {
            if (!requested.isValid()) return error.InvalidArgument;
            const e = try self.resolve(source);
            if (!e.rights.duplicate) return error.AccessDenied;
            if (!e.rights.contains(requested)) return error.AccessDenied;
            // Copy out before insert: insert may not alias `e`, and on failure
            // the table is unchanged (insert is all-or-nothing).
            const object = e.object;
            const otype = e.otype;
            return self.insert(object, otype, requested);
        }

        /// `derive` with untrusted rights bits. Unknown bits -> `InvalidArgument`.
        pub fn deriveRaw(self: *Self, source: Handle, requested_bits: u32) OmniError!Handle {
            const requested = try Rights.fromRaw(requested_bits);
            return self.derive(source, requested);
        }

        /// Invalidate a handle and free its slot. A second release of the same
        /// handle fails with `InvalidHandle`.
        pub fn release(self: *Self, h: Handle) OmniError!void {
            const e = try self.resolveMut(h);
            // Return the account charge first: if it fails (broken invariant),
            // nothing has been modified yet.
            if (self.account) |a| try a.release(.handles, 1);
            if (e.generation == std.math.maxInt(u16)) {
                e.state = .retired; // never reused: stale handles stay invalid
            } else {
                e.generation += 1;
                e.state = .free;
            }
            e.object = .invalid;
            e.rights = .none;
            self.live_count -= 1;
        }

        // ---- internals ----

        fn findFree(self: *const Self) ?usize {
            for (self.entries, 0..) |e, i| {
                if (e.state == .free) return i;
            }
            return null;
        }

        fn resolve(self: *const Self, h: Handle) OmniError!*const Entry {
            if (h == .invalid) return error.InvalidHandle;
            const idx: usize = h.index();
            if (idx >= capacity) return error.InvalidHandle;
            const e = &self.entries[idx];
            if (e.state != .live) return error.InvalidHandle;
            if (e.generation != h.generation()) return error.InvalidHandle;
            return e;
        }

        fn resolveMut(self: *Self, h: Handle) OmniError!*Entry {
            const e = try self.resolve(h);
            return @constCast(e);
        }

        /// Test hook: force a slot's generation so wraparound can be exercised
        /// without 65535 release cycles. Only valid for a free slot.
        pub fn testSetGeneration(self: *Self, slot: u16, generation: u16) void {
            std.debug.assert(self.entries[slot].state == .free);
            std.debug.assert(generation != 0);
            self.entries[slot].generation = generation;
        }
    };
}
