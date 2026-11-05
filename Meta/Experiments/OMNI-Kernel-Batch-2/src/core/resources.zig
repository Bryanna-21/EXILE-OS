//! Bounded resource accounting (Batch 2). Batch 1 reference: B.6.
//!
//! Model:
//!   * An `Account` has a fixed limit and a usage counter per `Resource`.
//!   * `reserve` charges the account AND every ancestor. It is all-or-nothing:
//!     every level is checked first, and counters are modified only if every
//!     level passes. A failed reserve changes nothing.
//!   * `release` is likewise all-or-nothing and can never underflow a counter.
//!     Releasing more than is reserved is reported as `InvalidState` and changes
//!     nothing.
//!   * All arithmetic is checked (`error.Overflow`).
//!
//! CONCURRENCY ASSUMPTION: this implementation is SINGLE-THREADED. It uses plain
//! integers with no locks or atomics and is NOT safe for concurrent use. A kernel
//! version needs per-account locking or atomics plus a defined lock order for the
//! parent chain; that is deferred (docs/IMPLEMENTATION-PLAN.md).
//!
//! LIFETIME: a child holds a pointer to its parent. The parent must outlive the
//! child and must not be moved or copied while children exist. This module cannot
//! enforce that; callers must keep accounts at stable addresses.
//!
//! Charging policy (shared objects, Batch 1 D5) is outside this module: it only
//! counts what callers tell it to count.

const std = @import("std");
const OmniError = @import("error.zig").OmniError;

pub const Resource = enum {
    pages,
    handles,
    threads,
    channels,
    queued_bytes,
};

/// One u64 per resource class. Used for both limits and usage.
pub const Counters = struct {
    pages: u64 = 0,
    handles: u64 = 0,
    threads: u64 = 0,
    channels: u64 = 0,
    queued_bytes: u64 = 0,

    pub fn get(self: Counters, r: Resource) u64 {
        return switch (r) {
            .pages => self.pages,
            .handles => self.handles,
            .threads => self.threads,
            .channels => self.channels,
            .queued_bytes => self.queued_bytes,
        };
    }

    fn ptr(self: *Counters, r: Resource) *u64 {
        return switch (r) {
            .pages => &self.pages,
            .handles => &self.handles,
            .threads => &self.threads,
            .channels => &self.channels,
            .queued_bytes => &self.queued_bytes,
        };
    }
};

pub const Account = struct {
    limits: Counters,
    usage: Counters = .{},
    parent: ?*Account = null,

    /// Create an account. With a parent, each child limit must not exceed the
    /// parent's REMAINING budget (limit - usage) at creation time (Batch 1 C.3,
    /// proc_set_limits; the exact rule is PROVISIONAL). On error nothing is
    /// modified.
    pub fn create(limits: Counters, parent: ?*Account) OmniError!Account {
        if (parent) |p| {
            inline for (std.meta.tags(Resource)) |r| {
                const remaining = std.math.sub(u64, p.limits.get(r), p.usage.get(r)) catch
                    return error.Internal; // usage above limit means a broken invariant
                if (limits.get(r) > remaining) return error.QuotaExceeded;
            }
        }
        return .{ .limits = limits, .parent = parent };
    }

    pub fn used(self: *const Account, r: Resource) u64 {
        return self.usage.get(r);
    }

    pub fn limit(self: *const Account, r: Resource) u64 {
        return self.limits.get(r);
    }

    /// Charge `amount` of `r` to this account and all ancestors, or change nothing.
    pub fn reserve(self: *Account, r: Resource, amount: u64) OmniError!void {
        // Pass 1: verify every level. No mutation.
        var it: ?*Account = self;
        while (it) |acc| : (it = acc.parent) {
            const new_usage = std.math.add(u64, acc.usage.get(r), amount) catch
                return error.Overflow;
            if (new_usage > acc.limits.get(r)) return error.QuotaExceeded;
        }
        // Pass 2: commit. Cannot overflow; pass 1 proved it for every level.
        it = self;
        while (it) |acc| : (it = acc.parent) {
            acc.usage.ptr(r).* += amount;
        }
    }

    /// Return `amount` of `r` to this account and all ancestors, or change nothing.
    /// Underflow at any level yields `InvalidState` with no counter modified.
    pub fn release(self: *Account, r: Resource, amount: u64) OmniError!void {
        var it: ?*Account = self;
        while (it) |acc| : (it = acc.parent) {
            if (amount > acc.usage.get(r)) return error.InvalidState;
        }
        it = self;
        while (it) |acc| : (it = acc.parent) {
            acc.usage.ptr(r).* -= amount;
        }
    }
};
