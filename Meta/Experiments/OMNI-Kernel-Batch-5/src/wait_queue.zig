//! Hosted wait model (Batch 5): an eventcount with bounded waiters and deadlines.
//!
//! HOSTED. This is the model of what a kernel scheduler must provide to the IPC
//! layer; it is built on an OS mutex and condition variable and is NOT a
//! scheduler. docs/SCHEDULER-CONTRACT.md states the contract a real scheduler
//! has to meet to replace it.
//!
//! Protocol (prevents lost wake-ups):
//!   1. waiter:   ticket = prepare()          (reads the event counter)
//!   2. waiter:   re-examine the condition (try the operation)
//!   3. waiter:   if it would block, wait(ticket, deadline)
//!   notifier:    change the state FIRST, then notifyAll() (increments the counter)
//! `wait` returns immediately if the counter moved since `prepare`, so a state
//! change that happens between steps 2 and 3 is never missed, and one that
//! happens before step 1 is seen by step 2.
//!
//! Locking: `mutex` is a leaf lock. No other lock is acquired while holding it,
//! and it is never held while calling out of this module.

const std = @import("std");

pub const WaitError = error{
    /// The bounded number of simultaneous waiters is exhausted.
    TooManyWaiters,
};

pub const Outcome = enum {
    /// The event counter changed (or a notification raced the wait). Re-check the condition.
    notified,
    /// The deadline passed with no notification.
    timed_out,
};

pub const Ticket = struct { seq: u64 };

/// A monotonic deadline. `limit == null` means no deadline.
pub const Deadline = struct {
    timer: ?std.time.Timer,
    limit_ns: ?u64,

    pub fn init(limit_ns: ?u64) Deadline {
        // If no monotonic clock is available `timer` is null and every deadline counts
        // as already expired (fail closed, never an unbounded wait).
        if (limit_ns) |_| return .{ .timer = std.time.Timer.start() catch null, .limit_ns = limit_ns };
        return .{ .timer = null, .limit_ns = null };
    }

    /// Nanoseconds left, or null for "no deadline". 0 means expired.
    pub fn remaining(self: *Deadline) ?u64 {
        const limit = self.limit_ns orelse return null;
        var t = self.timer orelse return 0;
        const el = t.read();
        return if (el >= limit) 0 else limit - el;
    }
};

pub const WaitQueue = struct {
    mutex: std.Thread.Mutex = .{},
    cond: std.Thread.Condition = .{},
    seq: u64 = 0,
    waiters: u32 = 0,
    max_waiters: u32,
    /// Statistics (guarded by `mutex`).
    notifies: u64 = 0,
    wakeups: u64 = 0,
    timeouts: u64 = 0,

    pub fn init(max_waiters: u32) WaitQueue {
        return .{ .max_waiters = max_waiters };
    }

    pub fn prepare(self: *WaitQueue) Ticket {
        self.mutex.lock();
        defer self.mutex.unlock();
        return .{ .seq = self.seq };
    }

    /// Block until notified after `ticket`, or until `dl` expires. Never waits when
    /// the counter already moved. A notification that races a timeout wins.
    pub fn wait(self: *WaitQueue, ticket: Ticket, dl: *Deadline) WaitError!Outcome {
        self.mutex.lock();
        defer self.mutex.unlock();
        if (self.seq != ticket.seq) return .notified;
        if (dl.remaining()) |r| if (r == 0) {
            self.timeouts += 1;
            return .timed_out;
        };
        if (self.waiters >= self.max_waiters) return error.TooManyWaiters;
        self.waiters += 1;
        defer self.waiters -= 1;
        while (true) {
            if (dl.remaining()) |r| {
                if (r == 0) {
                    self.timeouts += 1;
                    return .timed_out;
                }
                self.cond.timedWait(&self.mutex, r) catch {};
            } else {
                self.cond.wait(&self.mutex);
            }
            if (self.seq != ticket.seq) {
                self.wakeups += 1;
                return .notified;
            }
            // Spurious wake-up or timeout: loop re-evaluates the deadline.
        }
    }

    /// Publish an event. Call AFTER the state change that makes a waiter's
    /// condition true (or that a waiter must re-examine).
    pub fn notifyAll(self: *WaitQueue) void {
        self.mutex.lock();
        self.seq +%= 1;
        self.notifies += 1;
        self.mutex.unlock();
        self.cond.broadcast();
    }

    pub fn waiterCount(self: *WaitQueue) u32 {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.waiters;
    }
};
