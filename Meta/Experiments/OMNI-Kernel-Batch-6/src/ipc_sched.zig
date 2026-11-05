//! Blocking IPC coordinated with the scheduler model (Batch 6).
//!
//! HOSTED. `IpcSched` performs Batch 5 `send` / `recv` for a THREAD: the calling address
//! space is looked up from the thread (trusted scheduler state), the wait is done by the
//! scheduler model (`Sched.blockWait`, state `blocked`), and every state change made
//! through this type wakes the IPC waiters. It uses Batch 5 only through its public,
//! NON-BLOCKING interface (timeout 0) and does not modify it.
//!
//! Responsibilities (docs/SCHEDULER-INTEGRATION.md):
//!   * the IPC layer owns WAIT CONDITIONS: it knows when a state change may satisfy a waiter
//!     and calls `wakeIpc`;
//!   * the scheduler owns THREAD STATE: only `Sched` moves a thread between states;
//!   * the waiter owns its WAITER SLOT (Batch 5 ledger): taken before blocking, returned on
//!     every exit path, including being killed.
//!
//! No lost wake-up: the waiter registers, takes a ticket, THEN tests the condition (a
//! non-blocking Batch 5 call), and blocks only if the ticket is still current. A waker
//! changes the state first, THEN wakes every registered waiter.
//!
//! Known gap (also in REPORT.md): Batch 5 `syscall.dispatch` blocks inside Batch 5's own
//! hosted wait queue, invisible to `Sched`. Routing blocking SYSCALLS through `IpcSched`
//! needs a wait-provider hook in Batch 5 `dispatch`; it is specified, not implemented.
//!
//! Lock order: `reg_mutex` (this file) and `Sched.mutex` are leaves, never nested, and
//! neither is held across a call into Batch 5.

const std = @import("std");
const omni5 = @import("omni5");
const sched_mod = @import("sched.zig");

const kipc = omni5.kernel_ipc;
pub const KernelIpc = kipc.KernelIpc;
pub const Wait = kipc.Wait;
pub const OmniError = kipc.OmniError;
pub const Handle = kipc.Handle;
pub const Desc = kipc.Desc;
pub const SendReport = kipc.SendReport;
pub const Received = kipc.Received;
pub const SpaceId = kipc.SpaceId;
pub const Sched = sched_mod.Sched;
pub const ThreadId = sched_mod.ThreadId;

pub const IpcSched = struct {
    x: *KernelIpc,
    s: *Sched,
    reg_mutex: std.Thread.Mutex = .{},
    registered: [sched_mod.max_threads]ThreadId = [_]ThreadId{.invalid} ** sched_mod.max_threads,
    wakeups_sent: u64 = 0,

    pub fn init(x: *KernelIpc, s: *Sched) IpcSched {
        return .{ .x = x, .s = s };
    }

    // ------------------------------------------------------------ waiter registry

    fn register(self: *IpcSched, tid: ThreadId) void {
        self.reg_mutex.lock();
        defer self.reg_mutex.unlock();
        for (&self.registered) |*r| {
            if (r.* == .invalid) {
                r.* = tid;
                return;
            }
        }
        // Cannot happen: at most `max_threads` threads exist and each registers once.
        unreachable;
    }

    fn unregister(self: *IpcSched, tid: ThreadId) void {
        self.reg_mutex.lock();
        defer self.reg_mutex.unlock();
        for (&self.registered) |*r| {
            if (r.* == tid) {
                r.* = .invalid;
                return;
            }
        }
    }

    pub fn registeredCount(self: *IpcSched) u32 {
        self.reg_mutex.lock();
        defer self.reg_mutex.unlock();
        var n: u32 = 0;
        for (self.registered) |r| {
            if (r != .invalid) n += 1;
        }
        return n;
    }

    /// Wake every registered IPC waiter (they re-test their condition). Call AFTER the
    /// state change that may satisfy a waiter.
    pub fn wakeIpc(self: *IpcSched) void {
        var snapshot: [sched_mod.max_threads]ThreadId = undefined;
        var n: usize = 0;
        {
            self.reg_mutex.lock();
            defer self.reg_mutex.unlock();
            for (self.registered) |r| {
                if (r != .invalid) {
                    snapshot[n] = r;
                    n += 1;
                }
            }
            self.wakeups_sent += n;
        }
        for (snapshot[0..n]) |t| _ = self.s.wake(t, .woken);
    }

    fn spaceOfThread(self: *IpcSched, tid: ThreadId) OmniError!SpaceId {
        return self.s.spaceOf(tid) orelse error.InvalidState;
    }

    fn finiteOrNull(timeout_ns: u64) ?u64 {
        return if (timeout_ns == kipc.wait_forever) null else timeout_ns;
    }

    // ------------------------------------------------------------ blocking operations

    pub fn send(self: *IpcSched, tid: ThreadId, ep: Handle, bytes: []const u8, descs: []const Desc, w: Wait, report: ?*SendReport) OmniError!void {
        try self.x.validateWait(w);
        const space = try self.spaceOfThread(tid);
        var dl = omni5.wait_queue.Deadline.init(finiteOrNull(w.timeout_ns));
        var holding = false;
        defer if (holding) self.x.ledger.releaseWaiter(space);
        self.register(tid);
        defer self.unregister(tid);
        var last_try = false;
        while (true) {
            // A killed thread never starts (or continues) an operation: nothing is consumed.
            if (self.s.killRequested(tid)) return error.Cancelled;
            const ticket = self.s.prepareBlock(tid) catch return error.InvalidState;
            const r = self.x.send(space, ep, bytes, descs, .{ .timeout_ns = 0, .cancel = w.cancel }, report);
            if (r) |_| {
                self.wakeIpc();
                return;
            } else |e| {
                if (e != error.WouldBlock or w.timeout_ns == 0) {
                    if (e != error.WouldBlock) self.wakeIpc();
                    return e;
                }
                if (last_try) return error.TimedOut;
                if (!holding) {
                    self.x.ledger.acquireWaiter(space) catch return error.QuotaExceeded;
                    holding = true;
                }
                const reason = self.s.blockWait(ticket, dl.remaining()) catch return error.InvalidState;
                switch (reason) {
                    .killed => return error.Cancelled,
                    .timeout => last_try = true, // one more attempt: arrival beats timeout
                    else => {},
                }
            }
        }
    }

    pub fn recv(self: *IpcSched, tid: ThreadId, ep: Handle, w: Wait, out: *Received) OmniError!void {
        try self.x.validateWait(w);
        const space = try self.spaceOfThread(tid);
        var dl = omni5.wait_queue.Deadline.init(finiteOrNull(w.timeout_ns));
        var holding = false;
        defer if (holding) self.x.ledger.releaseWaiter(space);
        self.register(tid);
        defer self.unregister(tid);
        var last_try = false;
        while (true) {
            if (self.s.killRequested(tid)) return error.Cancelled;
            const ticket = self.s.prepareBlock(tid) catch return error.InvalidState;
            const r = self.x.recv(space, ep, .{ .timeout_ns = 0, .cancel = w.cancel }, out);
            if (r) |_| {
                self.wakeIpc();
                return;
            } else |e| {
                if (e != error.WouldBlock or w.timeout_ns == 0) {
                    if (e != error.WouldBlock) self.wakeIpc();
                    return e;
                }
                if (last_try) return error.TimedOut;
                if (!holding) {
                    self.x.ledger.acquireWaiter(space) catch return error.QuotaExceeded;
                    holding = true;
                }
                const reason = self.s.blockWait(ticket, dl.remaining()) catch return error.InvalidState;
                switch (reason) {
                    .killed => return error.Cancelled,
                    .timeout => last_try = true,
                    else => {},
                }
            }
        }
    }

    // ------------------------------------------------------------ state changes that wake

    pub fn endpointClose(self: *IpcSched, tid: ThreadId, ep: Handle) OmniError!void {
        const space = try self.spaceOfThread(tid);
        try self.x.endpointClose(space, ep);
        self.wakeIpc();
    }

    pub fn releaseHandle(self: *IpcSched, tid: ThreadId, h: Handle) OmniError!void {
        const space = try self.spaceOfThread(tid);
        try self.x.releaseHandle(space, h);
        self.wakeIpc();
    }

    pub fn revoke(self: *IpcSched, tid: ThreadId, h: Handle) OmniError!usize {
        const space = try self.spaceOfThread(tid);
        const n = try self.x.revoke(space, h);
        self.wakeIpc();
        return n;
    }

    pub fn cancelSignal(self: *IpcSched, tid: ThreadId, h: Handle) OmniError!void {
        const space = try self.spaceOfThread(tid);
        try self.x.cancelSignal(space, h);
        self.wakeIpc();
    }

    // ------------------------------------------------------------ process termination

    /// Terminate every thread of `space` and close the space. Order (docs/SCHEDULER-INTEGRATION.md):
    ///   1. mark every thread killed (sticky) and wake those that are blocked;
    ///   2. close the address space through Batch 5 (releases endpoints, queued messages and
    ///      in-flight capabilities exactly once; wakes peers and any waiter inside Batch 5);
    ///   3. wake IPC waiters again so peers see the closure;
    ///   4. each thread observes `killed`, returns, and exits (`Sched.exit`); the caller then
    ///      waits for the space to have no threads (`Sched.waitSpaceEmpty`).
    /// Returns the number of threads that were marked.
    pub fn terminateSpace(self: *IpcSched, space: SpaceId) OmniError!u32 {
        const n = self.s.killSpace(space);
        self.wakeIpc();
        defer self.wakeIpc();
        try self.x.spaceClose(space);
        return n;
    }
};
