//! OMNI-Kernel Batch 4: hosted IPC core (channels, bounded queues, capability
//! transfer, cancellation, closure) on top of the Batch 3 capability kernel.
//!
//! HOSTED PROTOTYPE. Nothing here is a kernel, enforces isolation, or is a
//! production ABI. Semantics are specified in docs/IPC-MODEL.md,
//! docs/CAPABILITY-TRANSFER.md, docs/CANCELLATION-AND-CLOSURE.md and
//! docs/CONCURRENCY.md; comments here state invariants and point at them.
//!
//! LAYERING
//!   * Batch 3 `Kernel` owns spaces, objects and capabilities. This module never
//!     touches its internals; it uses only its public API.
//!   * A *channel* has two *ends*. Each end is a Batch 3 object of type `channel`.
//!     A message sent through end X is queued in the INBOX of the other end.
//!   * Each end owns a private *hold space* (a Batch 3 space). Capabilities and
//!     accounting objects of queued messages live there, so in-flight authority
//!     is owned by the destination end, is accounted (handles), stays inside the
//!     derivation tree (revocable), and is released exactly once when the end is
//!     torn down (Batch 1 B.5).
//!   * Queued bytes are charged by creating one small Batch 3 object per queued
//!     message in the SENDER's space (resource `queued_bytes`) and moving its
//!     capability into the hold space. Receipt, drop or teardown releases that
//!     capability, and Batch 3's exact-once destruction refunds the sender.
//!
//! SYNCHRONISATION (docs/CONCURRENCY.md)
//!   * One mutex, `Ipc.mutex`, and one condition variable guard all IPC state.
//!   * `Ipc.mutex` is NEVER held while calling into the Batch 3 kernel, and the
//!     kernel never calls this module while holding its own lock (its destructor
//!     callbacks run unlocked). The two locks are therefore never nested and no
//!     lock order is required.
//!   * Operations that need both IPC state and kernel state use RESERVE ->
//!     WORK -> COMMIT: a reservation (counters and a `users` reference) is taken
//!     under the lock, kernel work runs unlocked, and a short critical section
//!     publishes or aborts. Failure paths undo exactly what was done.

const std = @import("std");
const Allocator = std.mem.Allocator;
const b2 = @import("batch2");
pub const k3 = @import("kernel3");
pub const wire = b2.ipc;

pub const OmniError = k3.OmniError;
pub const Kernel = k3.Kernel;
pub const SpaceId = k3.SpaceId;
pub const Handle = k3.Handle;
pub const Rights = k3.Rights;
pub const ObjectType = k3.ObjectType;
pub const ObjectId = k3.ObjectId;
pub const OpToken = k3.OpToken;
pub const Counters = k3.Counters;

/// Hard upper bound on capabilities per message (array sizes).
pub const max_desc = 8;

/// How a capability is handed over (docs/CAPABILITY-TRANSFER.md).
pub const Mode = enum(u8) {
    /// Sender loses the capability on success. Needs TRANSFER. The receiver gets
    /// the same rights (a move cannot attenuate).
    move = 1,
    /// Sender keeps its capability. Needs TRANSFER and DUPLICATE. The receiver
    /// gets `rights`, a subset of the sender's rights that must include TRANSFER.
    copy = 2,
};

pub const Desc = struct {
    handle: Handle,
    /// The type the sender believes the capability has (type confusion is
    /// `InvalidHandle`, as in Batch 1 B.3).
    otype: ObjectType,
    mode: Mode,
    rights: Rights,
};

pub const Config = struct {
    /// Messages one inbox may hold, including messages being published.
    max_messages: u32 = 8,
    /// Total wire bytes one inbox may hold, including messages being published.
    max_queue_bytes: u32 = 16 * 1024,
    /// Wire limits handed to the Batch 2 decoder.
    wire: wire.Limits = .{},

    pub fn validate(self: Config) OmniError!void {
        if (self.max_messages == 0) return error.InvalidArgument;
        if (self.wire.max_handles > max_desc) return error.InvalidArgument;
        if (self.wire.max_message_bytes < wire.wire_header_size) return error.InvalidArgument;
        if (self.max_queue_bytes < self.wire.max_message_bytes) return error.InvalidArgument;
        _ = try self.holdHandles();
    }

    /// Handle budget of one hold space: per message one accounting capability
    /// plus the enclosed capabilities. Checked, so absurd configs are rejected.
    pub fn holdHandles(self: Config) OmniError!u64 {
        const per = std.math.add(u64, 1, self.wire.max_handles) catch return error.Overflow;
        return std.math.mul(u64, self.max_messages, per) catch return error.Overflow;
    }
};

/// Waiting behaviour of `send` and `recv`.
pub const WaitOptions = struct {
    /// 0 = never wait (`WouldBlock`), null = wait without a deadline (still
    /// cancellable), n = wait at most n nanoseconds (`TimedOut`).
    timeout_ns: ?u64 = 0,
    /// Optional cancellation capability (needs WAIT, type `cancellation`).
    cancel: ?Handle = null,
};

pub const Rebound = struct { old: Handle, new: Handle };

/// Filled by `send` on failure; describes the (rare) cases in which a failed
/// MOVE could not leave the sender's handle value unchanged.
pub const SendReport = struct {
    /// A moved capability was returned to the sender under a NEW handle value.
    rebound: [max_desc]Rebound = undefined,
    rebound_count: u8 = 0,
    /// A moved capability could not be returned and was released instead. Only
    /// possible if the sender's own space ran out of slots or quota, or closed,
    /// during the rollback (docs/CAPABILITY-TRANSFER.md section 6).
    lost_count: u8 = 0,
};

pub const SlotStatus = enum(u8) {
    /// Unused slot (index >= count).
    none = 0,
    /// Capability installed in the receiver's space; `handle` is valid.
    delivered,
    /// The capability was revoked while the message was queued; `handle` is invalid.
    revoked,
    /// Partial delivery only: the capability could not be installed and was released.
    dropped,
};

pub const Slot = struct {
    status: SlotStatus = .none,
    handle: Handle = .invalid,
};

/// Result of a successful `recv`. The payload buffer is owned by the caller and
/// must be released with `deinit`.
pub const Received = struct {
    header: wire.Header,
    payload: []const u8,
    count: u8,
    slots: [max_desc]Slot,
    /// True if some capabilities were `dropped` (see `recv`).
    partial: bool,
    buf: []u8,

    pub fn deinit(self: *Received, ipc: *Ipc) void {
        ipc.allocator.free(self.buf);
        self.buf = &.{};
        self.payload = &.{};
    }
};

pub const EndStats = struct {
    queued_messages: u32,
    queued_bytes: u64,
    reserved_messages: u32,
    reserved_bytes: u64,
    users: u32,
    closed: bool,
    peer_closed: bool,
};

pub const IpcCounters = struct {
    sent: u64 = 0,
    received: u64 = 0,
    dropped_on_close: u64 = 0,
    would_block: u64 = 0,
    timed_out: u64 = 0,
    cancelled: u64 = 0,
    partial_receives: u64 = 0,
    send_rollbacks: u64 = 0,
    rebounds: u64 = 0,
    lost_on_rollback: u64 = 0,
    revoked_in_flight: u64 = 0,
};

/// Test-only instrumentation: called with NO lock held at fixed points.
pub const HookPoint = enum {
    send_reserved,
    /// After capability `index` was moved into the hold space.
    send_moved,
    send_before_commit,
    send_committed,
    recv_claimed,
    /// After capability `index` was installed in the receiver.
    recv_installed,
    recv_before_commit,
};

pub const Hook = struct {
    func: *const fn (ctx: ?*anyopaque, point: HookPoint, index: usize) void,
    ctx: ?*anyopaque = null,
};

// ------------------------------------------------------------------ internals

const InFlight = struct {
    /// Handle in the hold space; `.invalid` once revoked or consumed.
    hold_handle: Handle = .invalid,
    otype: ObjectType = .channel,
    mode: Mode = .move,
};

const Message = struct {
    prev: ?*Message = null,
    next: ?*Message = null,
    /// Full wire bytes, owned.
    buf: []u8,
    header: wire.Header,
    size: u32,
    claimed: bool = false,
    /// Accounting capability (in the hold space).
    acct: Handle = .invalid,
    ndesc: u8 = 0,
    descs: [max_desc]InFlight = [_]InFlight{.{}} ** max_desc,
};

const End = struct {
    ipc: *Ipc,
    channel: *Channel,
    index: u1,
    hold: SpaceId = undefined,
    object_id: ObjectId = ObjectId.fromRaw(0),
    // Everything below is guarded by Ipc.mutex.
    closed: bool = false,
    /// The Batch 3 object still exists (its destructor has not run).
    obj_alive: bool = true,
    torn_down: bool = false,
    teardown_done: bool = false,
    /// In-flight operations that touch this end's queue or hold space.
    users: u32 = 0,
    q_head: ?*Message = null,
    q_tail: ?*Message = null,
    q_len: u32 = 0,
    q_bytes: u64 = 0,
    res_msgs: u32 = 0,
    res_bytes: u64 = 0,
};

const Channel = struct {
    ipc: *Ipc,
    ends: [2]End,
    /// `createChannel` has not finished; the channel must not be freed yet.
    creating: bool = true,
    freed: bool = false,
};

const CancelState = struct {
    ipc: *Ipc,
    signaled: bool = false,
};

const Entry = union(enum) {
    end: *End,
    cancel: *CancelState,
};

/// What a caller must do after leaving the critical section.
const Plan = struct {
    teardown: bool = false,
};

const PinnedEnd = struct { tok: OpToken, end: *End };

const PinnedCancel = struct {
    tok: ?OpToken = null,
    state: ?*CancelState = null,
};

const Deadline = struct {
    timer: ?std.time.Timer,
    limit: ?u64,

    fn init(timeout_ns: ?u64) Deadline {
        if (timeout_ns) |t| {
            return .{ .timer = std.time.Timer.start() catch null, .limit = t };
        }
        return .{ .timer = null, .limit = null };
    }
};

pub const Ipc = struct {
    allocator: Allocator,
    kernel: *Kernel,
    config: Config,
    mutex: std.Thread.Mutex = .{},
    cond: std.Thread.Condition = .{},
    registry: std.AutoHashMapUnmanaged(u64, Entry) = .empty,
    counters: IpcCounters = .{},
    /// Test instrumentation; set before any thread uses the `Ipc`.
    hook: ?Hook = null,

    pub fn create(allocator: Allocator, kernel: *Kernel, config: Config) OmniError!*Ipc {
        try config.validate();
        const self = allocator.create(Ipc) catch return error.QuotaExceeded;
        self.* = .{ .allocator = allocator, .kernel = kernel, .config = config };
        return self;
    }

    /// Single-threaded teardown. Call AFTER `Kernel.destroy()`, which runs every
    /// remaining object destructor (and so tears down every end). Returns the
    /// number of registry entries that were still present; 0 means everything
    /// was reclaimed. A non-zero value means structures leaked.
    pub fn destroy(self: *Ipc) usize {
        const leftover = self.registry.count();
        self.registry.deinit(self.allocator);
        const allocator = self.allocator;
        allocator.destroy(self);
        return leftover;
    }

    fn fire(self: *Ipc, point: HookPoint, index: usize) void {
        if (self.hook) |h| h.func(h.ctx, point, index);
    }

    // ---------------------------------------------------------------- channels

    /// Create a channel and return capabilities to its two ends, both in `space`
    /// with rights read, write, duplicate, transfer, wait, signal, admin, inspect.
    /// Each end is charged 1 `channels` to `space`. All-or-nothing.
    pub fn createChannel(self: *Ipc, space: SpaceId) OmniError![2]Handle {
        const hold_budget = try self.config.holdHandles();
        const ch = self.allocator.create(Channel) catch return error.QuotaExceeded;
        ch.* = .{ .ipc = self, .ends = .{ .{ .ipc = self, .channel = ch, .index = 0 }, .{ .ipc = self, .channel = ch, .index = 1 } } };

        const limits = Counters{ .handles = hold_budget };
        var holds: usize = 0;
        var handles: [2]Handle = .{ .invalid, .invalid };
        var made: usize = 0; // capabilities created so far
        var err: ?OmniError = null;

        for (&ch.ends) |*e| {
            e.hold = self.kernel.createSpace(limits) catch |ex| {
                err = ex;
                break;
            };
            holds += 1;
        }
        if (err == null) {
            for (&ch.ends, 0..) |*e, i| {
                handles[i] = self.kernel.createObject(space, .channel, endRights(), .{
                    .charge_resource = .channels,
                    .charge_amount = 1,
                    .destroy_fn = endDestructor,
                    .destroy_ctx = e,
                }) catch |ex| {
                    err = ex;
                    break;
                };
                made += 1;
            }
        }
        if (err == null) {
            // Learn the object ids and register the ends.
            for (&ch.ends, 0..) |*e, i| {
                const r = self.kernel.lookup(space, handles[i], .channel, Rights.none) catch |ex| {
                    err = ex;
                    break;
                };
                e.object_id = r.object;
            }
        }
        if (err == null) {
            self.mutex.lock();
            for (&ch.ends) |*e| {
                self.registry.put(self.allocator, e.object_id.raw(), .{ .end = e }) catch {
                    err = error.QuotaExceeded;
                    break;
                };
            }
            if (err != null) {
                for (&ch.ends) |*e| _ = self.registry.remove(e.object_id.raw());
            }
            self.mutex.unlock();
        }

        if (err) |e| {
            self.abortCreate(ch, space, &handles, made, holds);
            return e;
        }

        var plan_free = false;
        {
            self.mutex.lock();
            ch.creating = false;
            plan_free = self.planFree(ch);
            self.mutex.unlock();
        }
        if (plan_free) self.allocator.destroy(ch);
        return handles;
    }

    fn endRights() Rights {
        return Rights.fromRaw(0x1FF & ~@as(u32, 4)) catch unreachable; // everything but `execute`
    }

    /// Undo a partially built channel. `made` capabilities exist in `space`.
    fn abortCreate(self: *Ipc, ch: *Channel, space: SpaceId, handles: *[2]Handle, made: usize, holds: usize) void {
        // Ends whose object was never created are closed by hand.
        var plans: [2]Plan = .{ .{}, .{} };
        self.mutex.lock();
        for (&ch.ends, 0..) |*e, i| {
            if (i >= made) {
                e.obj_alive = false;
                e.closed = true;
                if (i >= holds) {
                    // No hold space was ever created for this end.
                    e.torn_down = true;
                    e.teardown_done = true;
                } else {
                    plans[i] = self.planTeardown(e);
                }
            }
        }
        self.mutex.unlock();
        for (&ch.ends, 0..) |*e, i| {
            if (plans[i].teardown) self.teardownEnd(e);
        }
        // Releasing the created capabilities runs their destructors (which
        // unregister and tear down those ends).
        var i: usize = 0;
        while (i < made) : (i += 1) self.kernel.release(space, handles[i]) catch {};
        var plan_free = false;
        {
            self.mutex.lock();
            ch.creating = false;
            plan_free = self.planFree(ch);
            self.mutex.unlock();
        }
        if (plan_free) self.allocator.destroy(ch);
    }

    /// Destructor of an end's Batch 3 object. Runs with no kernel lock held, exactly once.
    fn endDestructor(ctx: ?*anyopaque, id: ObjectId) void {
        const end: *End = @ptrCast(@alignCast(ctx.?));
        end.ipc.onEndObjectDestroyed(end, id);
    }

    fn onEndObjectDestroyed(self: *Ipc, end: *End, id: ObjectId) void {
        var plan = Plan{};
        var free_chan = false;
        const ch = end.channel;
        {
            self.mutex.lock();
            _ = self.registry.remove(id.raw());
            end.obj_alive = false;
            if (!end.closed) {
                end.closed = true;
                self.cond.broadcast();
            }
            plan = self.planTeardown(end);
            free_chan = self.planFree(ch);
            self.mutex.unlock();
        }
        if (plan.teardown) self.teardownEnd(end);
        if (free_chan) self.allocator.destroy(ch);
    }

    /// Caller holds the mutex. If the end is closed and nobody uses it, mark it
    /// for teardown; the caller then runs `teardownEnd` after unlocking.
    fn planTeardown(self: *Ipc, end: *End) Plan {
        _ = self;
        if (end.closed and end.users == 0 and !end.torn_down) {
            end.torn_down = true;
            return .{ .teardown = true };
        }
        return .{};
    }

    /// Caller holds the mutex. True exactly once, when the channel may be freed.
    fn planFree(self: *Ipc, ch: *Channel) bool {
        _ = self;
        if (ch.freed or ch.creating) return false;
        for (ch.ends) |e| {
            if (!(e.teardown_done and !e.obj_alive)) return false;
        }
        ch.freed = true;
        return true;
    }

    /// Runs outside the lock, exactly once per end (guarded by `torn_down`).
    /// Drops queued messages, releases every in-flight capability and accounting
    /// object by closing the hold space, and frees the message memory.
    fn teardownEnd(self: *Ipc, end: *End) void {
        var head: ?*Message = null;
        {
            self.mutex.lock();
            head = end.q_head;
            end.q_head = null;
            end.q_tail = null;
            end.q_len = 0;
            end.q_bytes = 0;
            self.mutex.unlock();
        }
        // Closing the hold space releases the accounting capabilities (refunding
        // the senders) and every enclosed capability, exactly once.
        self.kernel.closeSpace(end.hold) catch {};
        var dropped: u64 = 0;
        var m = head;
        while (m) |msg| {
            const next = msg.next;
            self.allocator.free(msg.buf);
            self.allocator.destroy(msg);
            dropped += 1;
            m = next;
        }
        var free_chan = false;
        const ch = end.channel;
        {
            self.mutex.lock();
            self.counters.dropped_on_close += dropped;
            end.teardown_done = true;
            free_chan = self.planFree(ch);
            self.mutex.unlock();
        }
        if (free_chan) self.allocator.destroy(ch);
    }

    /// Mark an end closed (explicit close). Needs ADMIN. The Batch 3 capability
    /// stays valid; operations through it fail with `PeerDead`.
    pub fn closeEnd(self: *Ipc, space: SpaceId, h: Handle) OmniError!void {
        const pin = try self.pinEnd(space, h, rightsOf(.{ .admin = true }));
        var plan = Plan{};
        {
            self.mutex.lock();
            if (!pin.end.closed) {
                pin.end.closed = true;
                self.cond.broadcast();
            }
            plan = self.planTeardown(pin.end);
            self.mutex.unlock();
        }
        if (plan.teardown) self.teardownEnd(pin.end);
        self.kernel.endOp(pin.tok) catch {};
    }

    pub fn endStats(self: *Ipc, space: SpaceId, h: Handle) OmniError!EndStats {
        const pin = try self.pinEnd(space, h, rightsOf(.{ .inspect = true }));
        defer self.kernel.endOp(pin.tok) catch {};
        self.mutex.lock();
        defer self.mutex.unlock();
        const e = pin.end;
        return .{
            .queued_messages = e.q_len,
            .queued_bytes = e.q_bytes,
            .reserved_messages = e.res_msgs,
            .reserved_bytes = e.res_bytes,
            .users = e.users,
            .closed = e.closed,
            .peer_closed = e.channel.ends[1 - @as(usize, e.index)].closed,
        };
    }

    // ------------------------------------------------------------------ cancel

    /// Create a cancellation object. Rights: wait, signal, duplicate, transfer.
    pub fn createCancel(self: *Ipc, space: SpaceId) OmniError!Handle {
        const state = self.allocator.create(CancelState) catch return error.QuotaExceeded;
        state.* = .{ .ipc = self };
        const h = self.kernel.createObject(space, .cancellation, rightsOf(.{
            .wait = true,
            .signal = true,
            .duplicate = true,
            .transfer = true,
        }), .{ .destroy_fn = cancelDestructor, .destroy_ctx = state }) catch |e| {
            self.allocator.destroy(state);
            return e;
        };
        const r = self.kernel.lookup(space, h, .cancellation, Rights.none) catch |e| {
            self.kernel.release(space, h) catch {};
            return e;
        };
        self.mutex.lock();
        const put = self.registry.put(self.allocator, r.object.raw(), .{ .cancel = state });
        self.mutex.unlock();
        put catch {
            self.kernel.release(space, h) catch {};
            return error.QuotaExceeded;
        };
        return h;
    }

    fn cancelDestructor(ctx: ?*anyopaque, id: ObjectId) void {
        const state: *CancelState = @ptrCast(@alignCast(ctx.?));
        const self = state.ipc;
        self.mutex.lock();
        _ = self.registry.remove(id.raw());
        self.mutex.unlock();
        self.allocator.destroy(state);
    }

    /// Signal cancellation. One-shot and idempotent. Needs SIGNAL.
    pub fn signalCancel(self: *Ipc, space: SpaceId, h: Handle) OmniError!void {
        const tok = try self.kernel.beginOp(space, h, .cancellation, rightsOf(.{ .signal = true }));
        defer self.kernel.endOp(tok) catch {};
        self.mutex.lock();
        defer self.mutex.unlock();
        const entry = self.registry.get(tok.object.raw()) orelse return error.InvalidHandle;
        switch (entry) {
            .cancel => |c| {
                c.signaled = true;
                self.cond.broadcast();
            },
            else => return error.InvalidHandle,
        }
    }

    fn pinCancel(self: *Ipc, space: SpaceId, h: ?Handle) OmniError!PinnedCancel {
        const handle = h orelse return .{};
        const tok = try self.kernel.beginOp(space, handle, .cancellation, rightsOf(.{ .wait = true }));
        self.mutex.lock();
        const entry = self.registry.get(tok.object.raw());
        self.mutex.unlock();
        if (entry) |e| switch (e) {
            .cancel => |c| return .{ .tok = tok, .state = c },
            else => {},
        };
        self.kernel.endOp(tok) catch {};
        return error.InvalidHandle;
    }

    fn unpinCancel(self: *Ipc, p: PinnedCancel) void {
        if (p.tok) |t| self.kernel.endOp(t) catch {};
    }

    // -------------------------------------------------------------------- pins

    fn rightsOf(r: Rights) Rights {
        return r;
    }

    fn pinEnd(self: *Ipc, space: SpaceId, h: Handle, required: Rights) OmniError!PinnedEnd {
        const tok = try self.kernel.beginOp(space, h, .channel, required);
        self.mutex.lock();
        const entry = self.registry.get(tok.object.raw());
        self.mutex.unlock();
        if (entry) |e| switch (e) {
            .end => |end| return .{ .tok = tok, .end = end },
            else => {},
        };
        self.kernel.endOp(tok) catch {};
        return error.InvalidHandle;
    }

    // -------------------------------------------------------------------- send

    /// Send one message through the end named by `ep` (needs WRITE).
    ///
    /// `bytes` is a Batch 2 wire message (validated here); `descs` lists the
    /// capabilities to hand over, one per `header.handle_count`. On success the
    /// message is in the peer's inbox and every `move` capability has left the
    /// sender. On failure nothing was published and the sender keeps its
    /// authority; the only exception is reported through `report`.
    ///
    /// Check order (deterministic): endpoint capability; wire validation;
    /// descriptor validation; cancellation already signalled; PeerDead; room in
    /// the queue (WouldBlock / TimedOut / Cancelled); allocation; accounting;
    /// capability transfer; publication.
    pub fn send(
        self: *Ipc,
        space: SpaceId,
        ep: Handle,
        bytes: []const u8,
        descs: []const Desc,
        opts: WaitOptions,
        report: ?*SendReport,
    ) OmniError!void {
        if (report) |r| r.* = .{};
        const pin = try self.pinEnd(space, ep, rightsOf(.{ .write = true }));
        defer self.kernel.endOp(pin.tok) catch {};
        const cancel = try self.pinCancel(space, opts.cancel);
        defer self.unpinCancel(cancel);

        const src = pin.end;
        const dst = &src.channel.ends[1 - @as(usize, src.index)];

        // 1. Wire validation (Batch 2): size, version, flags, kind, lengths.
        const msg = try wire.decode(bytes, self.config.wire);
        if (msg.header.handle_count != descs.len) return error.InvalidArgument;
        const size: u32 = @intCast(bytes.len);

        // 2. Descriptor validation. Reads only; changes nothing.
        try self.validateDescs(space, dst, descs);

        // 3. Reserve room in the destination inbox.
        try self.reserve(src, dst, size, opts, cancel, pin.tok);
        self.fire(.send_reserved, 0);

        // 4. Allocate the queue entry and copy the bytes (nothing published yet).
        const m = self.allocator.create(Message) catch {
            self.abortReservation(dst, size);
            return error.QuotaExceeded;
        };
        const buf = self.allocator.dupe(u8, bytes) catch {
            self.allocator.destroy(m);
            self.abortReservation(dst, size);
            return error.QuotaExceeded;
        };
        m.* = .{ .buf = buf, .header = msg.header, .size = size, .ndesc = @intCast(descs.len) };

        // 5. Account the queued bytes to the sender, then hand the accounting
        //    capability to the destination's hold space.
        self.chargeMessage(space, dst, m) catch |e| {
            self.allocator.free(buf);
            self.allocator.destroy(m);
            self.abortReservation(dst, size);
            return e;
        };

        // 6. Transfer the enclosed capabilities into the hold space.
        var moved: usize = 0;
        var failure: ?OmniError = null;
        for (descs, 0..) |d, i| {
            const held = self.stageCap(space, dst, d) catch |e| {
                failure = e;
                break;
            };
            m.descs[i] = .{ .hold_handle = held, .otype = d.otype, .mode = d.mode };
            moved += 1;
            self.fire(.send_moved, i);
        }
        if (failure) |e| {
            self.rollbackSend(space, dst, m, descs, moved, report);
            self.allocator.free(buf);
            self.allocator.destroy(m);
            self.abortReservation(dst, size);
            return e;
        }

        // 7. Publish.
        self.fire(.send_before_commit, 0);
        var closed = false;
        {
            self.mutex.lock();
            if (dst.closed or src.closed) {
                closed = true;
            } else {
                dst.res_msgs -= 1;
                dst.res_bytes -= size;
                self.appendMessage(dst, m);
                dst.users -= 1;
                self.counters.sent += 1;
                self.cond.broadcast();
            }
            self.mutex.unlock();
        }
        if (closed) {
            self.rollbackSend(space, dst, m, descs, moved, report);
            self.allocator.free(buf);
            self.allocator.destroy(m);
            self.abortReservation(dst, size);
            return error.PeerDead;
        }
        self.fire(.send_committed, 0);
    }

    fn validateDescs(self: *Ipc, space: SpaceId, dst: *End, descs: []const Desc) OmniError!void {
        for (descs, 0..) |d, i| {
            if (!d.rights.isValid()) return error.InvalidArgument;
            for (descs[i + 1 ..]) |o| {
                if (o.handle == d.handle) return error.InvalidArgument; // duplicate (Batch 2 rule)
            }
        }
        for (descs) |d| {
            const need: Rights = switch (d.mode) {
                .move => .{ .transfer = true },
                .copy => .{ .transfer = true, .duplicate = true },
            };
            const r = try self.kernel.lookup(space, d.handle, d.otype, need);
            if (r.object.raw() == dst.object_id.raw()) return error.InvalidArgument; // direct cycle
            switch (d.mode) {
                .move => {
                    if (d.rights.toRaw() != r.rights.toRaw()) return error.InvalidArgument;
                },
                .copy => {
                    if (!d.rights.transfer) return error.InvalidArgument;
                    if (!r.rights.contains(d.rights)) return error.AccessDenied; // escalation
                },
            }
        }
    }

    /// Reserve one message slot and `size` bytes in `dst`, waiting if allowed.
    fn reserve(self: *Ipc, src: *End, dst: *End, size: u32, opts: WaitOptions, cancel: PinnedCancel, tok: OpToken) OmniError!void {
        var dl = Deadline.init(opts.timeout_ns);
        var timed_out = false;
        self.mutex.lock();
        if (cancel.state) |c| {
            if (c.signaled) {
                self.counters.cancelled += 1;
                self.mutex.unlock();
                return error.Cancelled;
            }
        }
        while (true) {
            if (src.closed or dst.closed) {
                self.mutex.unlock();
                return error.PeerDead;
            }
            if (self.hasRoom(dst, size)) break;
            if (opts.timeout_ns) |t| {
                if (t == 0) {
                    self.counters.would_block += 1;
                    self.mutex.unlock();
                    return error.WouldBlock;
                }
            }
            if (cancel.state) |c| {
                if (c.signaled) {
                    self.counters.cancelled += 1;
                    self.mutex.unlock();
                    return error.Cancelled;
                }
            }
            if (timed_out) {
                self.counters.timed_out += 1;
                self.mutex.unlock();
                return error.TimedOut;
            }
            if (self.waitStep(&dl)) timed_out = true;
            // Authority is re-checked outside the lock after every wake-up.
            self.mutex.unlock();
            const still = self.kernel.opStillAuthorized(tok);
            self.mutex.lock();
            if (!still) {
                self.mutex.unlock();
                return error.InvalidHandle;
            }
        }
        dst.res_msgs += 1;
        dst.res_bytes += size;
        dst.users += 1;
        self.mutex.unlock();
    }

    fn hasRoom(self: *Ipc, dst: *End, size: u32) bool {
        const msgs = std.math.add(u32, dst.q_len, dst.res_msgs) catch return false;
        if (msgs >= self.config.max_messages) return false;
        const used = std.math.add(u64, dst.q_bytes, dst.res_bytes) catch return false;
        const need = std.math.add(u64, used, size) catch return false;
        return need <= self.config.max_queue_bytes;
    }

    /// Caller holds the mutex. Returns true if the wait timed out.
    fn waitStep(self: *Ipc, dl: *Deadline) bool {
        if (dl.limit) |limit| {
            const elapsed: u64 = if (dl.timer) |*t| t.read() else 0;
            if (elapsed >= limit) return true;
            self.cond.timedWait(&self.mutex, limit - elapsed) catch return true;
            return false;
        }
        self.cond.wait(&self.mutex);
        return false;
    }

    /// Give back a reservation made by `reserve` (nothing was published).
    fn abortReservation(self: *Ipc, dst: *End, size: u32) void {
        var plan = Plan{};
        {
            self.mutex.lock();
            dst.res_msgs -= 1;
            dst.res_bytes -= size;
            dst.users -= 1;
            self.cond.broadcast();
            plan = self.planTeardown(dst);
            self.mutex.unlock();
        }
        if (plan.teardown) self.teardownEnd(dst);
    }

    /// Create the accounting object in the sender's space and move its capability
    /// into the destination hold space. On error nothing remains.
    fn chargeMessage(self: *Ipc, space: SpaceId, dst: *End, m: *Message) OmniError!void {
        const h = try self.kernel.createObject(space, .memory_object, rightsOf(.{ .transfer = true }), .{
            .charge_resource = .queued_bytes,
            .charge_amount = m.size,
        });
        const held = self.kernel.transfer(space, h, dst.hold) catch |e| {
            self.kernel.release(space, h) catch {};
            return e;
        };
        m.acct = held;
    }

    /// Move or copy one capability into the hold space; returns its hold handle.
    fn stageCap(self: *Ipc, space: SpaceId, dst: *End, d: Desc) OmniError!Handle {
        switch (d.mode) {
            .move => return self.kernel.transfer(space, d.handle, dst.hold),
            .copy => {
                const child = try self.kernel.derive(space, d.handle, d.rights);
                return self.kernel.transfer(space, child, dst.hold) catch |e| {
                    self.kernel.release(space, child) catch {};
                    return e;
                };
            },
        }
    }

    /// Undo the first `moved` capability stagings and the accounting capability.
    fn rollbackSend(self: *Ipc, space: SpaceId, dst: *End, m: *Message, descs: []const Desc, moved: usize, report: ?*SendReport) void {
        var rebound: u64 = 0;
        var lost: u64 = 0;
        var i = moved;
        while (i > 0) {
            i -= 1;
            const held = m.descs[i].hold_handle;
            switch (descs[i].mode) {
                .copy => self.kernel.release(dst.hold, held) catch {},
                .move => {
                    if (self.kernel.transfer(dst.hold, held, space)) |back| {
                        rebound += 1;
                        if (report) |r| {
                            r.rebound[r.rebound_count] = .{ .old = descs[i].handle, .new = back };
                            r.rebound_count += 1;
                        }
                    } else |e| {
                        // Revoked meanwhile: it no longer exists, nothing is lost.
                        // Otherwise the sender cannot take it back (its space is
                        // closed or exhausted): release rather than leave it in flight.
                        if (e != error.InvalidHandle) {
                            self.kernel.release(dst.hold, held) catch {};
                            lost += 1;
                            if (report) |r| r.lost_count += 1;
                        }
                    }
                },
            }
        }
        self.kernel.release(dst.hold, m.acct) catch {};
        self.mutex.lock();
        self.counters.send_rollbacks += 1;
        self.counters.rebounds += rebound;
        self.counters.lost_on_rollback += lost;
        self.mutex.unlock();
    }

    fn appendMessage(self: *Ipc, end: *End, m: *Message) void {
        _ = self;
        m.prev = end.q_tail;
        m.next = null;
        if (end.q_tail) |t| t.next = m else end.q_head = m;
        end.q_tail = m;
        end.q_len += 1;
        end.q_bytes += m.size;
    }

    fn unlinkMessage(self: *Ipc, end: *End, m: *Message) void {
        _ = self;
        if (m.prev) |p| p.next = m.next else end.q_head = m.next;
        if (m.next) |n| n.prev = m.prev else end.q_tail = m.prev;
        m.prev = null;
        m.next = null;
        end.q_len -= 1;
        end.q_bytes -= m.size;
    }

    // -------------------------------------------------------------------- recv

    /// Receive the oldest unclaimed message from the end named by `ep` (needs READ).
    ///
    /// Capabilities are installed in `space`. If a capability cannot be installed
    /// (the receiver's quota or memory), the receive fails, the message stays
    /// queued and every capability installed so far is returned to the message.
    /// If that return fails too (the receiver's own space was exhausted
    /// concurrently), the message is delivered PARTIALLY (`partial = true`): the
    /// installed capabilities are kept, the rest are released and marked
    /// `dropped`. Capabilities revoked while queued are marked `revoked`.
    ///
    /// Waiting order: authority; closed; message available; peer closed;
    /// non-blocking; cancelled; timed out. A message that is available when a
    /// waiter wakes is delivered even if cancellation was signalled at the same
    /// time. Once a message is claimed it is delivered; cancellation is no longer
    /// consulted.
    pub fn recv(self: *Ipc, space: SpaceId, ep: Handle, opts: WaitOptions, out: *Received) OmniError!void {
        const pin = try self.pinEnd(space, ep, rightsOf(.{ .read = true }));
        defer self.kernel.endOp(pin.tok) catch {};
        const cancel = try self.pinCancel(space, opts.cancel);
        defer self.unpinCancel(cancel);
        const end = pin.end;
        const peer = &end.channel.ends[1 - @as(usize, end.index)];

        // Claim a message.
        var dl = Deadline.init(opts.timeout_ns);
        var timed_out = false;
        var m: *Message = undefined;
        self.mutex.lock();
        if (cancel.state) |c| {
            if (c.signaled) {
                self.counters.cancelled += 1;
                self.mutex.unlock();
                return error.Cancelled;
            }
        }
        while (true) {
            if (end.closed) {
                self.mutex.unlock();
                return error.PeerDead;
            }
            if (firstUnclaimed(end)) |found| {
                found.claimed = true;
                end.users += 1;
                m = found;
                break;
            }
            if (peer.closed) {
                self.mutex.unlock();
                return error.PeerDead;
            }
            if (opts.timeout_ns) |t| {
                if (t == 0) {
                    self.counters.would_block += 1;
                    self.mutex.unlock();
                    return error.WouldBlock;
                }
            }
            if (cancel.state) |c| {
                if (c.signaled) {
                    self.counters.cancelled += 1;
                    self.mutex.unlock();
                    return error.Cancelled;
                }
            }
            if (timed_out) {
                self.counters.timed_out += 1;
                self.mutex.unlock();
                return error.TimedOut;
            }
            if (self.waitStep(&dl)) timed_out = true;
            self.mutex.unlock();
            const still = self.kernel.opStillAuthorized(pin.tok);
            self.mutex.lock();
            if (!still) {
                self.mutex.unlock();
                return error.InvalidHandle;
            }
        }
        self.mutex.unlock();
        self.fire(.recv_claimed, 0);

        // Install the enclosed capabilities.
        var slots = [_]Slot{.{}} ** max_desc;
        var installed: usize = 0; // descriptors processed so far
        var failure: ?OmniError = null;
        var i: usize = 0;
        while (i < m.ndesc) : (i += 1) {
            const held = m.descs[i].hold_handle;
            if (held == .invalid) {
                slots[i] = .{ .status = .revoked };
                installed += 1;
                continue;
            }
            if (self.kernel.transfer(end.hold, held, space)) |nh| {
                slots[i] = .{ .status = .delivered, .handle = nh };
                installed += 1;
                self.fire(.recv_installed, i);
            } else |e| switch (e) {
                // InvalidHandle means either "the capability was revoked while queued"
                // or "the receiver's space no longer exists". Only the first is a
                // revocation: the capability is gone from the hold space. If it is
                // still there, the destination is dead and the receive fails.
                error.InvalidHandle => {
                    if (self.kernel.lookup(end.hold, held, m.descs[i].otype, Rights.none)) |_| {
                        failure = error.PeerDead;
                        break;
                    } else |_| {
                        m.descs[i].hold_handle = .invalid;
                        slots[i] = .{ .status = .revoked };
                        installed += 1;
                    }
                },
                else => {
                    failure = e;
                    break;
                },
            }
        }

        var partial = false;
        var final_err: ?OmniError = null;
        if (failure) |e| {
            if (e == error.PeerDead) {
                // The receiver's own space was closed: everything it had installed
                // is already gone. Discard the rest of the message.
                final_err = e;
                var k = i;
                while (k < m.ndesc) : (k += 1) {
                    const held = m.descs[k].hold_handle;
                    if (held != .invalid) self.kernel.release(end.hold, held) catch {};
                    m.descs[k].hold_handle = .invalid;
                }
                var q: usize = 0;
                while (q < i) : (q += 1) {
                    slots[q] = .{};
                }
            } else {
                // Return what was installed to the hold space.
                var stuck = false;
                var j = installed;
                while (j > 0) {
                    j -= 1;
                    if (slots[j].status != .delivered) continue;
                    if (self.kernel.transfer(space, slots[j].handle, end.hold)) |back| {
                        m.descs[j].hold_handle = back;
                        slots[j] = .{};
                    } else |re| switch (re) {
                        // Gone (revoked after installation): nothing to return.
                        error.InvalidHandle => {
                            m.descs[j].hold_handle = .invalid;
                            slots[j] = .{ .status = .revoked };
                        },
                        else => stuck = true,
                    }
                }
                if (!stuck) {
                    var plan = Plan{};
                    {
                        self.mutex.lock();
                        m.claimed = false;
                        end.users -= 1;
                        self.cond.broadcast();
                        plan = self.planTeardown(end);
                        self.mutex.unlock();
                    }
                    if (plan.teardown) self.teardownEnd(end);
                    return e;
                }
                // Partial delivery: keep what is installed, release the rest.
                partial = true;
                var k = installed;
                while (k < m.ndesc) : (k += 1) {
                    const held = m.descs[k].hold_handle;
                    if (held != .invalid) self.kernel.release(end.hold, held) catch {};
                    m.descs[k].hold_handle = .invalid;
                    slots[k] = .{ .status = .dropped };
                }
                var q: usize = 0;
                while (q < installed) : (q += 1) {
                    if (slots[q].status == .none) {
                        const held = m.descs[q].hold_handle;
                        if (held != .invalid) self.kernel.release(end.hold, held) catch {};
                        m.descs[q].hold_handle = .invalid;
                        slots[q] = .{ .status = .dropped };
                    }
                }
            }
        }

        // Commit: release the accounting capability, unlink, hand over the buffer.
        self.fire(.recv_before_commit, 0);
        self.kernel.release(end.hold, m.acct) catch {};
        var plan = Plan{};
        {
            self.mutex.lock();
            self.unlinkMessage(end, m);
            end.users -= 1;
            self.counters.received += 1;
            if (partial) self.counters.partial_receives += 1;
            var rev: u64 = 0;
            for (slots[0..m.ndesc]) |s| {
                if (s.status == .revoked) rev += 1;
            }
            self.counters.revoked_in_flight += rev;
            self.cond.broadcast();
            plan = self.planTeardown(end);
            self.mutex.unlock();
        }
        if (plan.teardown) self.teardownEnd(end);

        if (final_err) |e| {
            self.allocator.free(m.buf);
            self.allocator.destroy(m);
            return e;
        }
        out.* = .{
            .header = m.header,
            .payload = m.buf[wire.wire_header_size..],
            .count = m.ndesc,
            .slots = slots,
            .partial = partial,
            .buf = m.buf,
        };
        self.allocator.destroy(m);
    }

    fn firstUnclaimed(end: *End) ?*Message {
        var cur = end.q_head;
        while (cur) |m| : (cur = m.next) {
            if (!m.claimed) return m;
        }
        return null;
    }

    // ---------------------------------------------------------------- inspection

    pub fn stats(self: *Ipc) IpcCounters {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.counters;
    }

    /// Number of registered objects (ends and cancellation objects).
    pub fn registered(self: *Ipc) usize {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.registry.count();
    }

    /// Verify IPC-internal invariants. Null means they hold. Takes the mutex.
    pub fn checkInvariants(self: *Ipc) ?[]const u8 {
        self.mutex.lock();
        defer self.mutex.unlock();
        var it = self.registry.iterator();
        while (it.next()) |kv| {
            switch (kv.value_ptr.*) {
                .cancel => {},
                .end => |e| {
                    var n: u32 = 0;
                    var bytes: u64 = 0;
                    var claimed: u32 = 0;
                    var prev: ?*Message = null;
                    var cur = e.q_head;
                    while (cur) |m| : (cur = m.next) {
                        if (m.prev != prev) return "queue back-link broken";
                        if (m.ndesc != m.header.handle_count) return "descriptor count differs from header";
                        if (m.buf.len != m.size) return "message size differs from buffer";
                        n += 1;
                        bytes += m.size;
                        if (m.claimed) claimed += 1;
                        prev = m;
                        if (n > self.config.max_messages) return "queue longer than limit";
                    }
                    if (e.q_tail != prev) return "queue tail wrong";
                    if (n != e.q_len) return "queue length counter wrong";
                    if (bytes != e.q_bytes) return "queue byte counter wrong";
                    if (@as(u64, e.q_len) + e.res_msgs > self.config.max_messages) return "messages over limit";
                    if (e.q_bytes + e.res_bytes > self.config.max_queue_bytes) return "bytes over limit";
                    if (claimed > e.users) return "claimed messages exceed users";
                    if (e.torn_down and !(e.closed and e.users == 0)) return "torn down while open or in use";
                    if (e.closed and !e.obj_alive and e.torn_down and e.q_len != 0) return "torn-down end still queues messages";
                },
            }
        }
        return null;
    }
};
