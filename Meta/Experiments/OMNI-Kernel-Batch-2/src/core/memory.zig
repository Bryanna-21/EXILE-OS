//! Checked range, alignment and buffer-validation helpers (Batch 2).
//! Batch 1 reference: B.3 (validation of user-controlled input).
//!
//! These helpers do ARITHMETIC ONLY. They never dereference an address.
//! Passing validation means "this (offset, len) is well-formed and inside the
//! stated bounds"; it does NOT mean a user pointer is safe to access. A kernel
//! must additionally (a) check that the pages are mapped with suitable
//! permission, (b) copy-in before validating contents (TOCTOU, B.3 rule 6), and
//! (c) handle faults. Those steps need architecture support and are deferred.
//!
//! Conventions (PROVISIONAL):
//!   * A range is half-open: [offset, offset + len).
//!   * A zero-length range is valid iff its offset is <= the upper bound
//!     (so offset == bound, "exact end", is valid; offset > bound is not).
//!   * Overflow of offset + len is `Overflow`; a well-formed range that falls
//!     outside the bounds is `InvalidArgument`.

const std = @import("std");
const OmniError = @import("error.zig").OmniError;

pub const Range = struct {
    offset: u64,
    len: u64,

    /// Exclusive end, checked.
    pub fn end(self: Range) OmniError!u64 {
        return std.math.add(u64, self.offset, self.len) catch error.Overflow;
    }
};

/// Build a range, rejecting offset + len overflow.
pub fn checkedRange(offset: u64, len: u64) OmniError!Range {
    const r = Range{ .offset = offset, .len = len };
    _ = try r.end();
    return r;
}

/// Validate that [offset, offset+len) lies within [0, total).
pub fn validateWithin(total: u64, offset: u64, len: u64) OmniError!Range {
    const r = try checkedRange(offset, len);
    if ((try r.end()) > total) return error.InvalidArgument;
    return r;
}

/// Checked element-count -> byte-length conversion.
pub fn byteLen(count: u64, elem_size: u64) OmniError!u64 {
    return std.math.mul(u64, count, elem_size) catch error.Overflow;
}

pub fn isPowerOfTwo(value: u64) bool {
    return value != 0 and (value & (value - 1)) == 0;
}

/// Require `alignment` to be a power of two and `value` to be a multiple of it.
pub fn checkAligned(value: u64, alignment: u64) OmniError!void {
    if (!isPowerOfTwo(alignment)) return error.InvalidArgument;
    if ((value & (alignment - 1)) != 0) return error.InvalidArgument;
}

/// Round `value` up to a multiple of `alignment` (power of two), checked.
pub fn alignUp(value: u64, alignment: u64) OmniError!u64 {
    if (!isPowerOfTwo(alignment)) return error.InvalidArgument;
    const mask = alignment - 1;
    const bumped = std.math.add(u64, value, mask) catch return error.Overflow;
    return bumped & ~mask;
}

/// An address-space window [lo, hi) in which user addresses may lie. The values
/// are supplied by the architecture layer (see arch/interface.zig); this struct
/// carries no knowledge of any real architecture.
pub const UserWindow = struct {
    lo: u64,
    hi: u64,

    /// Validate an untrusted (addr, len) pair against the window. Returns the
    /// validated range. No memory is accessed.
    pub fn validateSpan(self: UserWindow, addr: u64, len: u64) OmniError!Range {
        if (self.lo > self.hi) return error.Internal; // misconfigured window
        const r = try checkedRange(addr, len);
        if (addr < self.lo) return error.InvalidArgument;
        if ((try r.end()) > self.hi) return error.InvalidArgument;
        return r;
    }
};

/// Return the sub-slice `bytes[offset .. offset+len]` after full validation.
/// Works only on slices the caller already owns, so it never touches an
/// arbitrary address. u64 values that do not fit `usize` are rejected.
pub fn subslice(bytes: []const u8, offset: u64, len: u64) OmniError![]const u8 {
    const r = try validateWithin(bytes.len, offset, len);
    const start = std.math.cast(usize, r.offset) orelse return error.Overflow;
    const length = std.math.cast(usize, r.len) orelse return error.Overflow;
    return bytes[start..][0..length];
}
