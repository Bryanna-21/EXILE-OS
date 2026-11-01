//! Shared helpers for the Batch 6 tests. Everything here is HOSTED and SIMULATED: there is
//! no hardware, no MMU and no scheduler; see the module headers of the code under test.

const std = @import("std");
pub const k3 = @import("kernel3");
pub const b2 = @import("batch2");
pub const ipc4 = @import("ipc4");
pub const omni5 = @import("omni5");
pub const arch = @import("../arch.zig");
pub const sched = @import("../sched.zig");
pub const ts = @import("../thread_state.zig");
pub const um = @import("../user_memory.zig");
pub const ips = @import("../ipc_sched.zig");
pub const entry = @import("../entry.zig");
pub const ctxsw = @import("../context.zig");

pub const kipc = omni5.kernel_ipc;
pub const sys = omni5.syscall;
pub const Kernel = k3.Kernel;
pub const SpaceId = k3.SpaceId;
pub const Handle = k3.Handle;
pub const Rights = k3.Rights;
pub const Sched = sched.Sched;
pub const ThreadId = sched.ThreadId;
pub const PageModel = um.PageModel;
pub const ms = std.time.ns_per_ms;

pub const R_READ: u32 = 1;
pub const R_WRITE: u32 = 2;
pub const R_DUP: u32 = 8;
pub const R_XFER: u32 = 16;
pub const R_ALL: u32 = 0x1FF;

pub fn rights(bits: u32) Rights {
    return Rights.fromRaw(bits) catch @panic("test bug: invalid rights bits");
}

pub const big: k3.Counters = .{ .pages = 1_000_000, .handles = 100_000, .threads = 1000, .channels = 1000, .queued_bytes = 1 << 30 };
pub const space_budget: k3.Counters = .{ .pages = 10_000, .handles = 10_000, .threads = 10, .channels = 10, .queued_bytes = 1 << 20 };

// User addresses used by the helpers (one page each).
pub const A_ARGS: u64 = 0x1000;
pub const A_MSG: u64 = 0x2000;
pub const A_DESC: u64 = 0x10000;
pub const A_RBUF: u64 = 0x20000;
pub const A_RRES: u64 = 0x30000;
pub const A_MISC: u64 = 0x40000;

pub const Proc = struct { space: SpaceId, model: *PageModel };

/// Kernel + KernelIpc + scheduler model + syscall entry state, heap allocated.
pub const Rig = struct {
    alloc: std.mem.Allocator,
    k: *Kernel,
    x: *kipc.KernelIpc,
    s: Sched = .{},
    ip: ips.IpcSched = undefined,
    sysm: entry.System = undefined,

    pub fn create(alloc: std.mem.Allocator, cfg: kipc.Config) !*Rig {
        return createWith(alloc, alloc, cfg, .x86_64, .omni_proposed);
    }

    pub fn createWith(alloc: std.mem.Allocator, ipc_alloc: std.mem.Allocator, cfg: kipc.Config, a: arch.Arch, conv: arch.Convention) !*Rig {
        const r = try alloc.create(Rig);
        errdefer alloc.destroy(r);
        const k = try Kernel.create(alloc, .{ .max_spaces = 256, .max_objects = 4096, .max_nodes = 16384, .max_pins = 4096, .root = big });
        errdefer k.destroy();
        const x = try kipc.KernelIpc.create(ipc_alloc, k, cfg);
        r.* = .{ .alloc = alloc, .k = k, .x = x };
        r.ip = ips.IpcSched.init(x, &r.s);
        r.sysm = .{ .x = x, .s = &r.s, .arch = a, .conv = conv, .user_top = if (a == .x86_64) um.user_top_x86_64 else um.user_top_aarch64 };
        return r;
    }

    /// Returns the number of leaked Batch 4 registry entries (expected 0).
    pub fn destroy(r: *Rig) usize {
        const alloc = r.alloc;
        r.k.destroy();
        const leaked = r.x.destroy();
        alloc.destroy(r);
        return leaked;
    }

    pub fn audit(r: *Rig) !void {
        if (r.x.audit()) |why| {
            std.debug.print("AUDIT VIOLATION (ipc): {s}\n", .{why});
            return error.InvariantViolated;
        }
        if (r.s.audit()) |why| {
            std.debug.print("AUDIT VIOLATION (sched): {s}\n", .{why});
            return error.InvariantViolated;
        }
    }

    /// A new address space with its page model mapped (args, msg, desc, rbuf, rres, misc).
    pub fn newProc(r: *Rig) !Proc {
        const space = try r.k.createSpace(space_budget);
        const top = r.sysm.user_top;
        const model = try r.alloc.create(PageModel);
        errdefer r.alloc.destroy(model);
        model.* = PageModel.init(space, top);
        for ([_]u64{ A_ARGS, A_MSG, A_DESC, A_RBUF, A_RRES, A_MISC }) |a| try model.map(a, 4096, true, true);
        try r.sysm.memory.register(space, model);
        return .{ .space = space, .model = model };
    }

    pub fn freeProc(r: *Rig, p: Proc) void {
        r.sysm.memory.unregister(p.space);
        r.alloc.destroy(p.model);
    }

    /// Create a thread in `p` and make it running (no CPU).
    pub fn spawn(r: *Rig, p: Proc) !ThreadId {
        const t = try r.s.create(p.space);
        try r.s.dispatch(t, null);
        return t;
    }

    pub fn spawnOn(r: *Rig, p: Proc, cpu: u8) !ThreadId {
        const t = try r.s.create(p.space);
        try r.s.dispatch(t, cpu);
        return t;
    }
};

pub fn msg(buf: []u8, op: u16, payload: []const u8, n_handles: u32) []const u8 {
    const len = b2.ipc.encode(buf, .{ .op = op, .kind = .send, .handle_count = n_handles }, payload, .{}) catch @panic("test bug: encode");
    return buf[0..len];
}

pub fn moveDesc(h: Handle, t: k3.ObjectType, r: u32) ipc4.Desc {
    return .{ .handle = h, .otype = t, .mode = .move, .rights = rights(r) };
}

/// Two processes and a channel: end 0 in A, end 1 moved to B.
pub const Pair = struct { a: Proc, b: Proc, a0: Handle, b1: Handle };

pub fn pair(r: *Rig) !Pair {
    const a = try r.newProc();
    const b = try r.newProc();
    const e = try r.x.endpointCreate(a.space);
    const b1 = try r.k.transfer(a.space, e[1], b.space);
    return .{ .a = a, .b = b, .a0 = e[0], .b1 = b1 };
}

// ------------------------------------------------------------------ user structures

const w = std.mem.writeInt;

pub fn prepSend(p: Proc, wire: []const u8, ndesc: u32, timeout_ns: u64, cancel: u32) !void {
    try p.model.userStore(A_MSG, wire);
    var a: [48]u8 = [_]u8{0} ** 48;
    w(u64, a[0..8], A_MSG, .little);
    w(u32, a[8..12], @intCast(wire.len), .little);
    w(u32, a[12..16], ndesc, .little);
    w(u64, a[16..24], A_DESC, .little);
    w(u64, a[24..32], timeout_ns, .little);
    w(u32, a[32..36], cancel, .little);
    try p.model.userStore(A_ARGS, &a);
}

pub fn putDesc(p: Proc, i: usize, d: ipc4.Desc) !void {
    var b: [16]u8 = [_]u8{0} ** 16;
    w(u32, b[0..4], @intFromEnum(d.handle), .little);
    b[4] = @intFromEnum(d.otype);
    b[5] = @intFromEnum(d.mode);
    w(u32, b[8..12], d.rights.toRaw(), .little);
    try p.model.userStore(A_DESC + i * 16, &b);
}

pub fn prepRecv(p: Proc, timeout_ns: u64, cancel: u32) !void {
    var a: [48]u8 = [_]u8{0} ** 48;
    w(u64, a[0..8], A_RBUF, .little);
    w(u32, a[8..12], 4096, .little);
    w(u64, a[16..24], A_RRES, .little);
    w(u64, a[24..32], timeout_ns, .little);
    w(u32, a[32..36], cancel, .little);
    try p.model.userStore(A_ARGS, &a);
}

pub fn resU32(p: Proc, off: usize) !u32 {
    var b: [4]u8 = undefined;
    try p.model.userLoad(A_RRES + off, &b);
    return std.mem.readInt(u32, &b, .little);
}

// ------------------------------------------------------------------ register states

pub const nr_endpoint_create: u64 = @intFromEnum(sys.Nr.endpoint_create);

/// A VALID syscall entry state for `a` using convention `conv`: number `nr`, arguments `args`
/// (only those the convention carries are placed; the rest of `args` is ignored).
pub fn regsFor(a: arch.Arch, conv: arch.Convention, nr: u64, args: [6]u64) arch.Regs {
    switch (a) {
        .x86_64 => {
            var r = arch.X86Regs{ .rax = nr, .cs = 0x23, .ss = 0x1b, .rflags = 0x202, .rip = 0x40_0000, .rsp = 0x7fff_0000 };
            switch (conv) {
                .omni_proposed => {
                    r.rdi = args[0];
                    r.rsi = args[1];
                    r.rdx = args[2];
                    r.r10 = args[3];
                    r.r8 = args[4];
                    r.r9 = args[5];
                },
                .kernel_existing => {
                    r.rdx = args[0];
                    r.rdi = args[1];
                    r.rbx = args[2];
                    r.rsi = args[3];
                },
            }
            return .{ .x86_64 = r };
        },
        .aarch64 => {
            var r = arch.ArmRegs{ .esr = (arch.aarch64_ec_svc64 << 26) | (1 << 25), .pstate = 0, .pc = 0x40_0000, .sp = 0x7fff_0000 };
            r.x[8] = nr;
            switch (conv) {
                .omni_proposed => for (0..6) |i| {
                    r.x[i] = args[i];
                },
                .kernel_existing => for (0..4) |i| {
                    r.x[i + 1] = args[i];
                },
            }
            return .{ .aarch64 = r };
        },
    }
}

/// Run one syscall for the thread current on `cpu`; the test fails on a FATAL entry.
pub fn call(r: *Rig, cpu: u8, nr: sys.Nr, args: [6]u64) !i64 {
    var regs = regsFor(r.sysm.arch, r.sysm.conv, @intFromEnum(nr), args);
    return switch (r.sysm.syscallEntry(cpu, .{}, &regs)) {
        .returned => |v| v,
        .fatal => |e| {
            std.debug.print("unexpected fatal entry: {s}\n", .{@errorName(e)});
            return error.UnexpectedFatal;
        },
    };
}

pub fn expectFatal(res: entry.EntryResult, want: entry.Fatal) !void {
    switch (res) {
        .returned => |v| {
            std.debug.print("expected fatal {s}, got status {d}\n", .{ @errorName(want), v });
            return error.ExpectedFatal;
        },
        .fatal => |e| try std.testing.expectEqual(want, e),
    }
}

/// Poll until `tid` is in `want` (or fail after ~2 s).
pub fn waitState(s: *Sched, tid: ThreadId, want: ts.State) !void {
    var i: u32 = 0;
    while (i < 2000) : (i += 1) {
        if (s.stateOf(tid)) |st| {
            if (st == want) return;
        }
        std.Thread.sleep(1 * ms);
    }
    return error.StateNotReached;
}
