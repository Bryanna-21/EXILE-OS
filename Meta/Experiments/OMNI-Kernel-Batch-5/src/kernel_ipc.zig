//! Kernel-facing IPC interface (Batch 5).
//!
//! HOSTED, KERNEL-FACING. This layer is the narrow boundary between an eventual
//! syscall front end and the object model. It is NOT linked into a bootable
//! kernel. Everything below the boundary is Batch 4 (`ipc.zig`) on top of the
//! Batch 3 capability kernel; nothing below is modified.
//!
//! Responsibilities added here:
//!   * a uniform, bounded blocking model (`wait_queue.zig`) in place of the Batch 4
//!     condition variable: every Batch 4 call is made NON-BLOCKING (timeout 0) and
//!     this layer waits, so wake-up, cancellation and timeout are decided in one place;
//!   * policy: finite waits only (an unbounded wait requires a cancellation handle),
//!     bounded blocked callers per space and in total;
//!   * wake-up on every state change made through this interface;
//!   * counters and an audit over every layer.
//!
//! Lock ordering (docs/SCHEDULER-CONTRACT.md): `WaitQueue.mutex` and `Ledger.mutex`
//! are leaves. This layer never holds either while calling Batch 4 or Batch 3, never
//! nests the two, and Batch 4 / Batch 3 never call back into this layer.
//!
//! KNOWN LIMIT: state changes made by calling Batch 3 / Batch 4 DIRECTLY (for example
//! `Kernel.closeSpace` or `Kernel.release` of the last end capability) do not notify
//! waiters of this layer; those waiters are released by their deadline. Use the
//! wrappers below (`spaceClose`, `releaseHandle`, `revoke`) so wake-ups are immediate.

const std = @import("std");
const Allocator = std.mem.Allocator;
const ipc4 = @import("ipc4");
const wq_mod = @import("wait_queue.zig");
const acct = @import("accounting.zig");

pub const OmniError = ipc4.OmniError;
pub const Kernel = ipc4.Kernel;
pub const SpaceId = ipc4.SpaceId;
pub const Handle = ipc4.Handle;
pub const Desc = ipc4.Desc;
pub const SendReport = ipc4.SendReport;
pub const Received = ipc4.Received;
pub const EndStats = ipc4.EndStats;
pub const wait_forever = acct.wait_forever;

pub const Config = struct {
    ipc: ipc4.Config = .{},
    policy: acct.Policy = .{},
};

/// How long a call may block. `timeout_ns == 0` polls (`WouldBlock`); a finite value up
/// to `Policy.max_wait_ns` waits at most that long (`TimedOut`); `wait_forever` waits
/// until an event and requires `cancel`.
pub const Wait = struct {
    timeout_ns: u64 = 0,
    cancel: ?Handle = null,
};

pub const KernelIpc = struct {
    allocator: Allocator,
    kernel: *Kernel,
    ipc: *ipc4.Ipc,
    wq: wq_mod.WaitQueue,
    ledger: acct.Ledger,
    config: Config,

    pub fn create(allocator: Allocator, kernel: *Kernel, config: Config) OmniError!*KernelIpc {
        try config.policy.validate();
        const inner = try ipc4.Ipc.create(allocator, kernel, config.ipc);
        errdefer _ = inner.destroy();
        const self = allocator.create(KernelIpc) catch return error.QuotaExceeded;
        self.* = .{
            .allocator = allocator,
            .kernel = kernel,
            .ipc = inner,
            .wq = wq_mod.WaitQueue.init(config.policy.max_waiters_total),
            .ledger = acct.Ledger.init(allocator, config.policy),
            .config = config,
        };
        return self;
    }

    /// Single-threaded teardown. Call AFTER `Kernel.destroy()`. Returns the number of
    /// leaked inner registry entries; 0 means everything was reclaimed.
    pub fn destroy(self: *KernelIpc) usize {
        const leftover = self.ipc.destroy();
        self.ledger.deinit();
        const allocator = self.allocator;
        allocator.destroy(self);
        return leftover;
    }

    // ------------------------------------------------------------ validation

    /// Deterministic wait-argument check (no clamping, no truncation).
    pub fn validateWait(self: *KernelIpc, w: Wait) OmniError!void {
        if (w.timeout_ns == wait_forever) {
            if (w.cancel == null) return error.InvalidArgument; // unbounded wait needs a way out
        } else if (w.timeout_ns > self.config.policy.max_wait_ns) {
            return error.InvalidArgument;
        }
    }

    // ------------------------------------------------------------ endpoints

    pub fn endpointCreate(self: *KernelIpc, caller: SpaceId) OmniError![2]Handle {
        return self.ipc.createChannel(caller);
    }

    /// Close the end named by `ep` (needs ADMIN). Wakes every blocked caller.
    pub fn endpointClose(self: *KernelIpc, caller: SpaceId, ep: Handle) OmniError!void {
        defer self.wq.notifyAll();
        return self.ipc.closeEnd(caller, ep);
    }

    pub fn endpointStats(self: *KernelIpc, caller: SpaceId, ep: Handle) OmniError!EndStats {
        return self.ipc.endStats(caller, ep);
    }

    // ------------------------------------------------------------ cancellation

    pub fn cancelCreate(self: *KernelIpc, caller: SpaceId) OmniError!Handle {
        return self.ipc.createCancel(caller);
    }

    /// Signal cancellation (needs SIGNAL). One-shot; wakes blocked callers.
    pub fn cancelSignal(self: *KernelIpc, caller: SpaceId, h: Handle) OmniError!void {
        defer self.wq.notifyAll();
        return self.ipc.signalCancel(caller, h);
    }

    // ------------------------------------------------------------ handle wrappers

    /// Release a handle and wake waiters (the release may destroy an endpoint).
    pub fn releaseHandle(self: *KernelIpc, caller: SpaceId, h: Handle) OmniError!void {
        defer self.wq.notifyAll();
        return self.kernel.release(caller, h);
    }

    /// Revoke below `h` and wake waiters so they re-check their authority.
    pub fn revoke(self: *KernelIpc, caller: SpaceId, h: Handle) OmniError!usize {
        defer self.wq.notifyAll();
        return self.kernel.revoke(caller, h);
    }

    /// Close a whole space and wake waiters (its endpoints may die).
    pub fn spaceClose(self: *KernelIpc, space: SpaceId) OmniError!void {
        defer self.wq.notifyAll();
        return self.kernel.closeSpace(space);
    }

    // ------------------------------------------------------------ send / recv

    /// Send through `ep` (needs WRITE). Semantics of Batch 4 `send`, with waiting as
    /// described in the module comment. On any error nothing was published.
    pub fn send(
        self: *KernelIpc,
        caller: SpaceId,
        ep: Handle,
        bytes: []const u8,
        descs: []const Desc,
        w: Wait,
        report: ?*SendReport,
    ) OmniError!void {
        try self.validateWait(w);
        var dl = wq_mod.Deadline.init(finiteOrNull(w.timeout_ns));
        var holding = false;
        defer if (holding) self.ledger.releaseWaiter(caller);
        var last_try = false;
        while (true) {
            const ticket = self.wq.prepare();
            const r = self.ipc.send(caller, ep, bytes, descs, .{ .timeout_ns = 0, .cancel = w.cancel }, report);
            if (r) |_| {
                self.ledger.record(.send_ok);
                self.wq.notifyAll();
                return;
            } else |e| {
                if (e != error.WouldBlock or w.timeout_ns == 0) {
                    if (e != error.WouldBlock) self.wq.notifyAll();
                    self.ledger.record(.send_failed);
                    return e;
                }
                if (last_try) {
                    self.ledger.record(.send_failed);
                    return error.TimedOut;
                }
                self.beginWait(caller, &holding) catch |be| {
                    self.ledger.record(.send_failed);
                    return be;
                };
                const o = self.wq.wait(ticket, &dl) catch return self.waitRejected(.send_failed);
                if (o == .timed_out) last_try = true; // one more attempt: arrival beats timeout
            }
        }
    }

    /// Receive from `ep` (needs READ). On success `out` owns a payload buffer
    /// (`out.deinit(ipc)` via `deinitReceived`).
    pub fn recv(self: *KernelIpc, caller: SpaceId, ep: Handle, w: Wait, out: *Received) OmniError!void {
        try self.validateWait(w);
        var dl = wq_mod.Deadline.init(finiteOrNull(w.timeout_ns));
        var holding = false;
        defer if (holding) self.ledger.releaseWaiter(caller);
        var last_try = false;
        while (true) {
            const ticket = self.wq.prepare();
            const r = self.ipc.recv(caller, ep, .{ .timeout_ns = 0, .cancel = w.cancel }, out);
            if (r) |_| {
                self.ledger.record(.recv_ok);
                self.wq.notifyAll();
                return;
            } else |e| {
                if (e != error.WouldBlock or w.timeout_ns == 0) {
                    // A failed receive may have released a claim another receiver is waiting for.
                    if (e != error.WouldBlock) self.wq.notifyAll();
                    self.ledger.record(.recv_failed);
                    return e;
                }
                // WouldBlock on an empty inbox whose peer is alive: wait.
                if (last_try) {
                    self.ledger.record(.recv_failed);
                    return error.TimedOut;
                }
                self.beginWait(caller, &holding) catch |be| {
                    self.ledger.record(.recv_failed);
                    return be;
                };
                const o = self.wq.wait(ticket, &dl) catch return self.waitRejected(.recv_failed);
                if (o == .timed_out) last_try = true;
            }
        }
    }

    pub fn deinitReceived(self: *KernelIpc, out: *Received) void {
        out.deinit(self.ipc);
    }

    fn finiteOrNull(timeout_ns: u64) ?u64 {
        return if (timeout_ns == wait_forever) null else timeout_ns;
    }

    fn beginWait(self: *KernelIpc, caller: SpaceId, holding: *bool) OmniError!void {
        if (holding.*) return;
        try self.ledger.acquireWaiter(caller);
        holding.* = true;
    }

    fn waitRejected(self: *KernelIpc, ev: acct.Ledger.Event) OmniError {
        self.ledger.record(ev);
        return error.QuotaExceeded;
    }

    // ------------------------------------------------------------ audit

    /// Check every layer. Null means consistent. Must be called when no operation is
    /// in flight for the "quiescent" checks done by callers; the checks here are
    /// valid at any time.
    pub fn audit(self: *KernelIpc) ?[]const u8 {
        if (self.kernel.checkInvariants()) |why| return why;
        if (self.ipc.checkInvariants()) |why| return why;
        if (self.ledger.audit()) |why| return why;
        if (self.wq.waiterCount() > self.config.policy.max_waiters_total) return "wait queue over its bound";
        return null;
    }
};
