//! Resource accounting for the kernel-facing IPC layer (Batch 5).
//!
//! What is accounted where (docs/RESOURCE-ACCOUNTING.md has the full table):
//!   * endpoints            Batch 3 `channels` quota of the creating space
//!   * queued bytes         Batch 3 `queued_bytes` quota of the SENDING space
//!   * in-flight capability Batch 3 `handles` of the receiving end's hold space
//!   * queued messages      per receiving end, hard-bounded by the Batch 4 `Config`
//!   * blocked waiters      THIS module: per-space and total, bounded
//! This module owns only what the kernel-facing layer can observe exactly:
//! blocked waiters (acquire/release are always paired by the caller) and operation
//! counters. All arithmetic is checked; a release without a matching acquire is
//! recorded as `underflows` and never silently ignored.

const std = @import("std");
const Allocator = std.mem.Allocator;
const k3 = @import("kernel3");

pub const Policy = struct {
    /// Longest finite wait a caller may request, in nanoseconds. PROVISIONAL.
    max_wait_ns: u64 = 30 * std.time.ns_per_s,
    /// Blocked callers per calling space. PROVISIONAL.
    max_waiters_per_space: u32 = 16,
    /// Blocked callers in the whole kernel-facing layer. PROVISIONAL.
    max_waiters_total: u32 = 64,

    pub fn validate(self: Policy) error{InvalidArgument}!void {
        if (self.max_wait_ns == 0 or self.max_wait_ns == wait_forever) return error.InvalidArgument;
        if (self.max_waiters_per_space == 0) return error.InvalidArgument;
        if (self.max_waiters_total < self.max_waiters_per_space) return error.InvalidArgument;
    }
};

/// Sentinel timeout meaning "no deadline"; only accepted together with a cancellation handle.
pub const wait_forever: u64 = std.math.maxInt(u64);

pub const Counters = struct {
    sends_ok: u64 = 0,
    recvs_ok: u64 = 0,
    sends_failed: u64 = 0,
    recvs_failed: u64 = 0,
    waits_started: u64 = 0,
    waits_rejected: u64 = 0,
    delivery_faults: u64 = 0,
    /// Releases without a matching acquire. Must stay 0.
    underflows: u64 = 0,
};

pub const Ledger = struct {
    allocator: Allocator,
    policy: Policy,
    mutex: std.Thread.Mutex = .{},
    per_space: std.AutoHashMapUnmanaged(u64, u32) = .empty,
    total_waiters: u32 = 0,
    counters: Counters = .{},

    pub fn init(allocator: Allocator, policy: Policy) Ledger {
        return .{ .allocator = allocator, .policy = policy };
    }

    pub fn deinit(self: *Ledger) void {
        self.per_space.deinit(self.allocator);
    }

    fn key(space: k3.SpaceId) u64 {
        return (@as(u64, space.gen) << 32) | space.index;
    }

    /// Reserve one blocked-waiter slot for `space`. `QuotaExceeded` leaves nothing changed.
    pub fn acquireWaiter(self: *Ledger, space: k3.SpaceId) error{QuotaExceeded}!void {
        self.mutex.lock();
        defer self.mutex.unlock();
        if (self.total_waiters >= self.policy.max_waiters_total) {
            self.counters.waits_rejected += 1;
            return error.QuotaExceeded;
        }
        const gop = self.per_space.getOrPut(self.allocator, key(space)) catch {
            self.counters.waits_rejected += 1;
            return error.QuotaExceeded;
        };
        if (!gop.found_existing) gop.value_ptr.* = 0;
        if (gop.value_ptr.* >= self.policy.max_waiters_per_space) {
            if (gop.value_ptr.* == 0) _ = self.per_space.remove(key(space));
            self.counters.waits_rejected += 1;
            return error.QuotaExceeded;
        }
        gop.value_ptr.* += 1;
        self.total_waiters += 1;
        self.counters.waits_started += 1;
    }

    /// Return a slot taken by `acquireWaiter`. A release with no matching acquire is
    /// counted in `underflows` and changes nothing else.
    pub fn releaseWaiter(self: *Ledger, space: k3.SpaceId) void {
        self.mutex.lock();
        defer self.mutex.unlock();
        const p = self.per_space.getPtr(key(space)) orelse {
            self.counters.underflows += 1;
            return;
        };
        if (p.* == 0 or self.total_waiters == 0) {
            self.counters.underflows += 1;
            return;
        }
        p.* -= 1;
        self.total_waiters -= 1;
        if (p.* == 0) _ = self.per_space.remove(key(space));
    }

    pub fn waitersOf(self: *Ledger, space: k3.SpaceId) u32 {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.per_space.get(key(space)) orelse 0;
    }

    pub fn waitersTotal(self: *Ledger) u32 {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.total_waiters;
    }

    pub fn snapshot(self: *Ledger) Counters {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.counters;
    }

    pub const Event = enum { send_ok, recv_ok, send_failed, recv_failed, delivery_fault };

    pub fn record(self: *Ledger, e: Event) void {
        self.mutex.lock();
        defer self.mutex.unlock();
        switch (e) {
            .send_ok => self.counters.sends_ok += 1,
            .recv_ok => self.counters.recvs_ok += 1,
            .send_failed => self.counters.sends_failed += 1,
            .recv_failed => self.counters.recvs_failed += 1,
            .delivery_fault => self.counters.delivery_faults += 1,
        }
    }

    /// Internal consistency. Null means consistent.
    pub fn audit(self: *Ledger) ?[]const u8 {
        self.mutex.lock();
        defer self.mutex.unlock();
        var sum: u64 = 0;
        var it = self.per_space.iterator();
        while (it.next()) |kv| {
            if (kv.value_ptr.* == 0) return "zero entry left in waiter table";
            if (kv.value_ptr.* > self.policy.max_waiters_per_space) return "per-space waiters over limit";
            sum += kv.value_ptr.*;
        }
        if (sum != self.total_waiters) return "waiter total differs from per-space sum";
        if (self.total_waiters > self.policy.max_waiters_total) return "total waiters over limit";
        if (self.counters.underflows != 0) return "a waiter slot was released twice";
        return null;
    }
};
