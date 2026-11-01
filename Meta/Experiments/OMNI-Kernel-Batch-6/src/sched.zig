//! Hosted scheduler model for blocking IPC (Batch 6).
//!
//! SIMULATION. This is NOT a scheduler: it picks no thread, preempts nothing and switches
//! no context. It models exactly the part of a scheduler that blocking IPC depends on:
//! a bounded thread table, the thread-state machine (`thread_state.zig`), and the
//! block / wake / kill protocol with its no-lost-wake-up guarantee. A hosted OS thread
//! stands for each simulated kernel thread, and `blockWait` is the hosted stand-in for
//! "yield to the scheduler until unblocked".
//!
//! Ownership rules (docs/SCHEDULER-INTEGRATION.md):
//!   * the scheduler is the ONLY owner of thread state: nothing else changes it;
//!   * a waker never edits a thread, it asks: `wake(tid, reason)`;
//!   * the blocker decides to block only through a `Ticket` taken BEFORE it checked its
//!     wait condition (eventcount): a wake that lands after `prepareBlock` is never lost;
//!   * `kill` is sticky: a killed thread never blocks again.
//!
//! Lock: one mutex (`mutex`) models `g_scheduler_lock`. It is a leaf lock: no function in
//! this file calls out while holding it.

const std = @import("std");
const k3 = @import("kernel3");
const ts = @import("thread_state.zig");

pub const SpaceId = k3.SpaceId;
pub const State = ts.State;

pub const max_threads = 64;
pub const max_cpus = 4;

pub const ThreadId = enum(u32) {
    invalid = 0,
    _,

    fn make(idx: usize, gen: u16) ThreadId {
        return @enumFromInt((@as(u32, gen) << 16) | @as(u32, @intCast(idx + 1)));
    }

    fn index(self: ThreadId) ?usize {
        const low = @intFromEnum(self) & 0xFFFF;
        if (low == 0 or low > max_threads) return null;
        return low - 1;
    }

    fn generation(self: ThreadId) u16 {
        return @truncate(@intFromEnum(self) >> 16);
    }
};

/// Why a blocked (or about to block) thread was woken. Ordered by precedence: when several
/// wake-ups coincide the highest wins; `killed` outranks everything.
pub const WakeReason = enum(u8) { none, timeout, woken, cancelled, killed };

pub const SchedError = error{
    NoSuchThread,
    InvalidState,
    InvalidTransition,
    TooManyThreads,
    CpuBusy,
    BadCpu,
};

pub const Ticket = struct { tid: ThreadId, seq: u64 };

pub const BlockResult = union(enum) {
    /// The thread did NOT block: a wake-up (or a kill) was already pending.
    immediate: WakeReason,
    /// The thread is now `blocked`; it stays so until `wake`.
    blocked,
};

pub const WakeResult = enum { made_runnable, recorded, ignored };

pub const Stats = struct {
    created: u64 = 0,
    blocks: u64 = 0,
    immediate: u64 = 0,
    made_runnable: u64 = 0,
    recorded: u64 = 0,
    ignored: u64 = 0,
    exited: u64 = 0,
};

const Thread = struct {
    used: bool = false,
    gen: u16 = 0,
    state: State = .invalid,
    space: ?SpaceId = null,
    /// Eventcount: incremented by every wake attempt on this thread.
    seq: u64 = 0,
    pending: WakeReason = .none,
    kill: bool = false,
    cpu: ?u8 = null,
};

pub const Sched = struct {
    mutex: std.Thread.Mutex = .{},
    cond: std.Thread.Condition = .{},
    threads: [max_threads]Thread = [_]Thread{.{}} ** max_threads,
    cpu_current: [max_cpus]ThreadId = [_]ThreadId{.invalid} ** max_cpus,
    stats: Stats = .{},

    // ------------------------------------------------------------ lookup (lock held)

    fn find(self: *Sched, tid: ThreadId) SchedError!*Thread {
        const i = tid.index() orelse return error.NoSuchThread;
        const th = &self.threads[i];
        if (!th.used or th.gen != tid.generation()) return error.NoSuchThread;
        return th;
    }

    fn setState(self: *Sched, th: *Thread, to: State) SchedError!void {
        _ = self;
        ts.check(th.state, to) catch return error.InvalidTransition;
        th.state = to;
    }

    fn releaseCpu(self: *Sched, th: *Thread, tid: ThreadId) void {
        if (th.cpu) |c| {
            if (self.cpu_current[c] == tid) self.cpu_current[c] = .invalid;
            th.cpu = null;
        }
    }

    // ------------------------------------------------------------ lifecycle

    /// Create a thread owned by `space`, in state `runnable`. The owner is fixed for the
    /// thread's life: it is the only source of "the calling address space".
    pub fn create(self: *Sched, space: SpaceId) SchedError!ThreadId {
        self.mutex.lock();
        defer self.mutex.unlock();
        for (&self.threads, 0..) |*th, i| {
            if (th.used) continue;
            th.gen +%= 1;
            if (th.gen == 0) th.gen = 1;
            th.used = true;
            th.state = .invalid;
            th.space = space;
            th.seq = 0;
            th.pending = .none;
            th.kill = false;
            th.cpu = null;
            try self.setState(th, .runnable);
            self.stats.created += 1;
            return ThreadId.make(i, th.gen);
        }
        return error.TooManyThreads;
    }

    /// Model of "the scheduler picked this thread": runnable -> running, optionally on `cpu`.
    pub fn dispatch(self: *Sched, tid: ThreadId, cpu: ?u8) SchedError!void {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.dispatchLocked(tid, cpu);
    }

    fn dispatchLocked(self: *Sched, tid: ThreadId, cpu: ?u8) SchedError!void {
        const th = try self.find(tid);
        if (cpu) |c| {
            if (c >= max_cpus) return error.BadCpu;
            if (self.cpu_current[c] != .invalid) return error.CpuBusy;
        }
        try self.setState(th, .running);
        if (cpu) |c| {
            self.cpu_current[c] = tid;
            th.cpu = c;
        }
    }

    /// running -> runnable (time slice ended or voluntary yield).
    pub fn yieldRunning(self: *Sched, tid: ThreadId) SchedError!void {
        self.mutex.lock();
        defer self.mutex.unlock();
        const th = try self.find(tid);
        if (th.state != .running) return error.InvalidState;
        try self.setState(th, .runnable);
        self.releaseCpu(th, tid);
    }

    /// Begin dying. From `running`, `runnable` or `stopped` only: a `blocked` thread must
    /// be woken first (the state machine refuses blocked -> dying).
    pub fn beginDeath(self: *Sched, tid: ThreadId) SchedError!void {
        self.mutex.lock();
        defer self.mutex.unlock();
        const th = try self.find(tid);
        try self.setState(th, .dying);
        self.releaseCpu(th, tid);
    }

    /// dying -> dead, and the slot is freed (its generation will change on reuse).
    pub fn finish(self: *Sched, tid: ThreadId) SchedError!void {
        self.mutex.lock();
        defer self.mutex.unlock();
        const th = try self.find(tid);
        try self.setState(th, .dead);
        th.used = false;
        th.state = .invalid;
        th.space = null;
        self.stats.exited += 1;
        self.cond.broadcast();
    }

    /// HOSTED join: wait until `space` has no live threads, or `timeout_ns` elapses.
    /// Returns true if the space is empty.
    pub fn waitSpaceEmpty(self: *Sched, space: SpaceId, timeout_ns: u64) bool {
        var timer = std.time.Timer.start() catch return self.liveThreads(space) == 0;
        self.mutex.lock();
        defer self.mutex.unlock();
        while (true) {
            var n: u32 = 0;
            for (self.threads) |th| {
                if (!th.used) continue;
                const sp = th.space orelse continue;
                if (sp.eql(space)) n += 1;
            }
            if (n == 0) return true;
            const el = timer.read();
            if (el >= timeout_ns) return false;
            self.cond.timedWait(&self.mutex, timeout_ns - el) catch {};
        }
    }

    /// running -> dying -> dead for the calling thread.
    pub fn exit(self: *Sched, tid: ThreadId) SchedError!void {
        try self.beginDeath(tid);
        try self.finish(tid);
    }

    // ------------------------------------------------------------ block / wake

    /// Take a ticket BEFORE testing the wait condition. Only a running thread may do so.
    /// Clears any stale (non-sticky) wake-up recorded earlier.
    pub fn prepareBlock(self: *Sched, tid: ThreadId) SchedError!Ticket {
        self.mutex.lock();
        defer self.mutex.unlock();
        const th = try self.find(tid);
        if (!ts.canBlock(th.state)) return error.InvalidState;
        th.pending = .none;
        return .{ .tid = tid, .seq = th.seq };
    }

    /// Block unless a wake-up arrived since `prepareBlock` (or the thread is being killed).
    pub fn block(self: *Sched, t: Ticket) SchedError!BlockResult {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.blockLocked(t);
    }

    fn blockLocked(self: *Sched, t: Ticket) SchedError!BlockResult {
        const th = try self.find(t.tid);
        if (!ts.canBlock(th.state)) return error.InvalidState;
        if (th.kill) {
            self.stats.immediate += 1;
            return .{ .immediate = .killed };
        }
        if (th.seq != t.seq) {
            const r = if (th.pending == .none) WakeReason.woken else th.pending;
            th.pending = .none;
            self.stats.immediate += 1;
            return .{ .immediate = r };
        }
        try self.setState(th, .blocked);
        self.releaseCpu(th, t.tid);
        self.stats.blocks += 1;
        return .blocked;
    }

    /// Ask for `tid` to be woken. Safe from any thread, any number of times; a wake for a
    /// thread that is gone (stale id, dying, dead) is ignored, never an error.
    pub fn wake(self: *Sched, tid: ThreadId, reason: WakeReason) WakeResult {
        self.mutex.lock();
        defer self.mutex.unlock();
        const r = self.wakeLocked(tid, reason);
        if (r == .made_runnable) self.cond.broadcast();
        return r;
    }

    fn wakeLocked(self: *Sched, tid: ThreadId, reason: WakeReason) WakeResult {
        const th = self.find(tid) catch {
            self.stats.ignored += 1;
            return .ignored;
        };
        switch (th.state) {
            .invalid, .dying, .dead => {
                self.stats.ignored += 1;
                return .ignored;
            },
            else => {},
        }
        th.seq +%= 1;
        if (@intFromEnum(reason) > @intFromEnum(th.pending)) th.pending = reason;
        if (th.state == .blocked) {
            // blocked -> runnable is always a valid transition.
            th.state = .runnable;
            self.stats.made_runnable += 1;
            return .made_runnable;
        }
        self.stats.recorded += 1;
        return .recorded;
    }

    /// The timer fired for `tid` (hosted stand-in for a timeout interrupt).
    pub fn timeoutFire(self: *Sched, tid: ThreadId) WakeResult {
        return self.wake(tid, .timeout);
    }

    /// Mark `tid` as being killed: sticky, and wakes it if it is blocked.
    pub fn kill(self: *Sched, tid: ThreadId) WakeResult {
        self.mutex.lock();
        defer self.mutex.unlock();
        const th = self.find(tid) catch return .ignored;
        switch (th.state) {
            .invalid, .dying, .dead => return .ignored,
            else => {},
        }
        th.kill = true;
        const r = self.wakeLocked(tid, .killed);
        if (r == .made_runnable) self.cond.broadcast();
        return r;
    }

    /// Kill every live thread of `space`. Returns how many threads were marked.
    pub fn killSpace(self: *Sched, space: SpaceId) u32 {
        self.mutex.lock();
        defer self.mutex.unlock();
        var n: u32 = 0;
        for (&self.threads, 0..) |*th, i| {
            if (!th.used) continue;
            const sp = th.space orelse continue;
            if (!sp.eql(space)) continue;
            switch (th.state) {
                .invalid, .dying, .dead => continue,
                else => {},
            }
            th.kill = true;
            _ = self.wakeLocked(ThreadId.make(i, th.gen), .killed);
            n += 1;
        }
        self.cond.broadcast();
        return n;
    }

    /// The woken thread is scheduled again: runnable -> running. Returns the wake reason
    /// (killed if the thread is being killed) and clears the recorded one.
    pub fn resumeThread(self: *Sched, tid: ThreadId, cpu: ?u8) SchedError!WakeReason {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.resumeLocked(tid, cpu);
    }

    fn resumeLocked(self: *Sched, tid: ThreadId, cpu: ?u8) SchedError!WakeReason {
        try self.dispatchLocked(tid, cpu);
        const th = try self.find(tid);
        const r: WakeReason = if (th.kill) .killed else th.pending;
        th.pending = .none;
        return r;
    }

    /// HOSTED blocking wait: block (unless a wake-up is already pending) and sleep the
    /// OS thread until woken or until `timeout_ns` elapses, then run the thread again.
    /// Returns the reason it ran again. No lock is held while the caller tests state.
    pub fn blockWait(self: *Sched, t: Ticket, timeout_ns: ?u64) SchedError!WakeReason {
        var timer: ?std.time.Timer = null;
        if (timeout_ns != null) timer = std.time.Timer.start() catch null;
        self.mutex.lock();
        defer self.mutex.unlock();
        switch (try self.blockLocked(t)) {
            .immediate => |r| return r,
            .blocked => {},
        }
        while (true) {
            const th = try self.find(t.tid);
            if (th.state != .blocked) break;
            if (timeout_ns) |limit| {
                const el: u64 = if (timer) |*tm| tm.read() else limit; // no clock: expired
                if (el >= limit) {
                    _ = self.wakeLocked(t.tid, .timeout);
                    break;
                }
                self.cond.timedWait(&self.mutex, limit - el) catch {};
            } else {
                self.cond.wait(&self.mutex);
            }
        }
        return self.resumeLocked(t.tid, null);
    }

    // ------------------------------------------------------------ queries

    pub fn spaceOf(self: *Sched, tid: ThreadId) ?SpaceId {
        self.mutex.lock();
        defer self.mutex.unlock();
        const th = self.find(tid) catch return null;
        return th.space;
    }

    pub fn stateOf(self: *Sched, tid: ThreadId) ?State {
        self.mutex.lock();
        defer self.mutex.unlock();
        const th = self.find(tid) catch return null;
        return th.state;
    }

    pub fn killRequested(self: *Sched, tid: ThreadId) bool {
        self.mutex.lock();
        defer self.mutex.unlock();
        const th = self.find(tid) catch return false;
        return th.kill;
    }

    pub fn current(self: *Sched, cpu: u8) ?ThreadId {
        if (cpu >= max_cpus) return null;
        self.mutex.lock();
        defer self.mutex.unlock();
        const t = self.cpu_current[cpu];
        return if (t == .invalid) null else t;
    }

    pub fn liveThreads(self: *Sched, space: SpaceId) u32 {
        self.mutex.lock();
        defer self.mutex.unlock();
        var n: u32 = 0;
        for (self.threads) |th| {
            if (!th.used) continue;
            const sp = th.space orelse continue;
            if (sp.eql(space)) n += 1;
        }
        return n;
    }

    pub fn totalThreads(self: *Sched) u32 {
        self.mutex.lock();
        defer self.mutex.unlock();
        var n: u32 = 0;
        for (self.threads) |th| {
            if (th.used) n += 1;
        }
        return n;
    }

    pub fn statsSnapshot(self: *Sched) Stats {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.stats;
    }

    /// Internal consistency. Null means consistent.
    pub fn audit(self: *Sched) ?[]const u8 {
        self.mutex.lock();
        defer self.mutex.unlock();
        for (self.threads, 0..) |th, i| {
            if (!th.used) {
                if (th.state != .invalid) return "unused slot has a state";
                continue;
            }
            if (th.space == null) return "used thread without an owner space";
            if (th.state == .invalid or th.state == .dead) return "used thread in invalid/dead state";
            if (th.state == .blocked and th.pending != .none) return "blocked thread has a pending wake-up";
            if (th.state == .blocked and th.cpu != null) return "blocked thread holds a cpu";
            if (th.cpu) |c| {
                if (th.state != .running) return "non-running thread holds a cpu";
                if (self.cpu_current[c] != ThreadId.make(i, th.gen)) return "cpu table does not match thread";
            }
        }
        for (self.cpu_current, 0..) |t, c| {
            if (t == .invalid) continue;
            const th = self.find(t) catch return "cpu refers to a missing thread";
            if (th.state != .running) return "cpu refers to a non-running thread";
            if (th.cpu == null or th.cpu.? != c) return "cpu/thread mismatch";
        }
        return null;
    }
};
