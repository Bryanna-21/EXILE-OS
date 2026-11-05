//! Distinct identifier and handle types (Batch 2).
//!
//! Each type is its own non-exhaustive enum, so values of different kinds cannot
//! be mixed without an explicit `fromRaw`/`raw` conversion at the boundary.
//!
//! IMPORTANT: none of these values confers authority.
//!   * `ObjectId`  - kernel-internal identity of an object. Never handed to user
//!                   space as an authority.
//!   * `ProcessId` - kernel-internal diagnostic identity. Batch 1 (C.3) specifies
//!                   handle-based identity, NOT global PIDs, in the user ABI.
//!   * `Handle`    - the user-visible token: an index plus a generation into ONE
//!                   process's capability table. A handle is only meaningful
//!                   together with the kernel-owned table that issued it; the
//!                   table checks index range, slot state, generation, object
//!                   type and rights on every use (see capabilities.zig).
//!                   Handle values are not secrets.
//!
//! Handle layout (PROVISIONAL, refines Batch 1 A.2 "index+generation, 0 invalid"):
//!   bits  0..15  index      (u16)
//!   bits 16..31  generation (u16, never 0 for a live handle)
//! Because a live generation is never 0, no live handle equals 0, so 0 is
//! always invalid as Batch 1 requires.

const std = @import("std");
const OmniError = @import("error.zig").OmniError;

pub const ObjectId = enum(u64) {
    invalid = 0,
    _,

    pub fn fromRaw(value: u64) ObjectId {
        return @enumFromInt(value);
    }
    pub fn raw(self: ObjectId) u64 {
        return @intFromEnum(self);
    }
    pub fn isValid(self: ObjectId) bool {
        return self != .invalid;
    }
};

pub const ProcessId = enum(u64) {
    invalid = 0,
    _,

    pub fn fromRaw(value: u64) ProcessId {
        return @enumFromInt(value);
    }
    pub fn raw(self: ProcessId) u64 {
        return @intFromEnum(self);
    }
    pub fn isValid(self: ProcessId) bool {
        return self != .invalid;
    }
};

pub const Handle = enum(u32) {
    invalid = 0,
    _,

    pub const index_bits = 16;

    /// Build a handle from parts. Generation 0 is reserved and rejected.
    pub fn make(idx: u16, generation_value: u16) OmniError!Handle {
        if (generation_value == 0) return error.InvalidArgument;
        const packed_value: u32 = (@as(u32, generation_value) << index_bits) | @as(u32, idx);
        return @enumFromInt(packed_value);
    }

    /// Wrap an untrusted raw value. Any u32 is representable; whether it names a
    /// live capability is decided only by the owning table.
    pub fn fromRaw(value: u32) Handle {
        return @enumFromInt(value);
    }
    pub fn raw(self: Handle) u32 {
        return @intFromEnum(self);
    }
    pub fn index(self: Handle) u16 {
        return @truncate(@intFromEnum(self));
    }
    pub fn generation(self: Handle) u16 {
        return @truncate(@intFromEnum(self) >> index_bits);
    }
};

/// Monotonic ObjectId source for the hosted model. Never returns `.invalid`.
/// Single-threaded (no atomics): see docs/CORE-INTERFACES.md, concurrency.
pub const ObjectIdAllocator = struct {
    next_raw: u64 = 1,

    pub fn next(self: *ObjectIdAllocator) OmniError!ObjectId {
        const current = self.next_raw;
        // Compute the successor first so a failure leaves state unchanged.
        const successor = std.math.add(u64, current, 1) catch return error.Overflow;
        self.next_raw = successor;
        return ObjectId.fromRaw(current);
    }
};
