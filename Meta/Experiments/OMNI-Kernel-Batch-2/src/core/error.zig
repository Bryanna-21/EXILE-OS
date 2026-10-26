//! Kernel error and result conventions (OMNI-ABI 0.1 draft, Batch 2).
//!
//! Design (PROVISIONAL, not a finalized ABI):
//!   * Internal code uses a single native Zig error set, `OmniError`.
//!     Functions return `OmniError!T` (alias: `Result(T)`); narrower inferred
//!     sets coerce into it.
//!   * Each error corresponds to one Batch 1 code (ABI-AND-SECURITY.md, A.4),
//!     written there as UPPER_SNAKE and here as CamelCase.
//!   * `INVALID_STATE` is NEW in Batch 2 (Batch 1 had no code for "operation is
//!     valid in form but not in the object's current state", e.g. releasing
//!     more than was reserved). Migration note: add it to A.4 in the next
//!     ABI draft; while the ABI major version is 0 no compatibility is promised.
//!   * `Status` gives each error a negative i32 for the eventual syscall
//!     boundary. Batch 1 deliberately assigned no numbers; the numbers below are
//!     a Batch 2 PROVISIONAL assignment and may change.
//!
//! This module has no state and no allocation.

const std = @import("std");

/// All errors the core abstractions can report.
pub const OmniError = error{
    /// Malformed value, unknown flag/enum/rights bit, length out of range.
    InvalidArgument,
    /// Handle not present, stale generation, or wrong object type.
    InvalidHandle,
    /// Handle valid but lacks a required right.
    AccessDenied,
    NotFound,
    AlreadyExists,
    /// A resource limit (or fixed table capacity) would be exceeded.
    QuotaExceeded,
    WouldBlock,
    TimedOut,
    Cancelled,
    PeerDead,
    NotSupported,
    /// No mutually supported ABI version.
    BadVersion,
    /// Arithmetic or size overflow detected.
    Overflow,
    Busy,
    IoError,
    /// Unexpected internal failure (an invariant did not hold).
    Internal,
    /// NEW in Batch 2: operation not valid in the current state.
    InvalidState,
};

/// Conventional result type for core operations.
pub fn Result(comptime T: type) type {
    return OmniError!T;
}

/// Provisional wire status: 0 = ok, negative = error. Non-exhaustive so that
/// untrusted values can be represented and rejected rather than trapping.
pub const Status = enum(i32) {
    ok = 0,
    invalid_argument = -1,
    invalid_handle = -2,
    access_denied = -3,
    not_found = -4,
    already_exists = -5,
    quota_exceeded = -6,
    would_block = -7,
    timed_out = -8,
    cancelled = -9,
    peer_dead = -10,
    not_supported = -11,
    bad_version = -12,
    overflow = -13,
    busy = -14,
    io_error = -15,
    internal = -16,
    invalid_state = -17,
    _,
};

/// Map an error to its provisional wire status. Exhaustive: adding an error to
/// `OmniError` without a mapping is a compile error.
pub fn toStatus(e: OmniError) Status {
    return switch (e) {
        error.InvalidArgument => .invalid_argument,
        error.InvalidHandle => .invalid_handle,
        error.AccessDenied => .access_denied,
        error.NotFound => .not_found,
        error.AlreadyExists => .already_exists,
        error.QuotaExceeded => .quota_exceeded,
        error.WouldBlock => .would_block,
        error.TimedOut => .timed_out,
        error.Cancelled => .cancelled,
        error.PeerDead => .peer_dead,
        error.NotSupported => .not_supported,
        error.BadVersion => .bad_version,
        error.Overflow => .overflow,
        error.Busy => .busy,
        error.IoError => .io_error,
        error.Internal => .internal,
        error.InvalidState => .invalid_state,
    };
}

/// Convert a (possibly untrusted) status back to a result. `.ok` is success.
/// Any value that is not a known status is rejected with `InvalidArgument`;
/// it is never mapped to success.
pub fn fromStatus(s: Status) OmniError!void {
    return switch (s) {
        .ok => {},
        .invalid_argument => error.InvalidArgument,
        .invalid_handle => error.InvalidHandle,
        .access_denied => error.AccessDenied,
        .not_found => error.NotFound,
        .already_exists => error.AlreadyExists,
        .quota_exceeded => error.QuotaExceeded,
        .would_block => error.WouldBlock,
        .timed_out => error.TimedOut,
        .cancelled => error.Cancelled,
        .peer_dead => error.PeerDead,
        .not_supported => error.NotSupported,
        .bad_version => error.BadVersion,
        .overflow => error.Overflow,
        .busy => error.Busy,
        .io_error => error.IoError,
        .internal => error.Internal,
        .invalid_state => error.InvalidState,
        _ => error.InvalidArgument,
    };
}
