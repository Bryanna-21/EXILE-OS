//! Syscall boundary (Batch 5): argument decoding, validation and dispatch.
//!
//! KERNEL-FACING, HOSTED. This file implements the validation and dispatch that an
//! architecture-specific entry stub would call. It contains NO machine-specific
//! code: the entry stub is expected to fill `Args` from registers (see
//! docs/SYSCALL-CONTRACT.md for the x86-64 and ARM64 register mappings, which are
//! specified but not implemented here) and to pass the identity of the calling
//! address space, which comes from the kernel and never from user arguments.
//! Nothing here is wired into a bootable kernel.
//!
//! Rules enforced for every call:
//!   * the syscall number is looked up in a closed table; unknown numbers are `no_sys`;
//!   * unused argument registers must be zero;
//!   * handles are 32-bit values; an argument with upper bits set is rejected, never truncated;
//!   * reserved fields and flags in user structures must be zero;
//!   * every length is bounded BEFORE it is used, address arithmetic is checked, and no
//!     user structure is reinterpreted in place: user bytes are copied into kernel memory
//!     and decoded field by field with explicit little-endian offsets;
//!   * output buffers are probed BEFORE an operation that cannot be undone;
//!   * authorization is done by the capability layer on every operation (the rights
//!     column of `table`); a syscall number never confers authority.

const std = @import("std");
const Allocator = std.mem.Allocator;
const ipc4 = @import("ipc4");
const kipc = @import("kernel_ipc.zig");
const b2 = @import("batch2");

pub const KernelIpc = kipc.KernelIpc;
pub const SpaceId = kipc.SpaceId;
pub const Handle = kipc.Handle;
pub const OmniError = kipc.OmniError;

/// PROVISIONAL numbers; the Batch 1 ABI assigned none.
pub const Nr = enum(u32) {
    endpoint_create = 0x100,
    endpoint_close = 0x101,
    send = 0x102,
    recv = 0x103,
    cancel_create = 0x104,
    cancel_signal = 0x105,
    endpoint_stats = 0x106,
    handle_release = 0x107,
};

pub const SysError = OmniError || error{ Fault, NoSys };

/// Wire status: 0 or positive = success (positive carries a value), negative = error.
/// Errors that exist in Batch 2 keep their Batch 2 numbers.
pub const status_fault: i64 = -32;
pub const status_no_sys: i64 = -33;

pub fn statusOf(e: SysError) i64 {
    return switch (e) {
        error.Fault => status_fault,
        error.NoSys => status_no_sys,
        else => |oe| @intFromEnum(b2.err.toStatus(oe)),
    };
}

pub const Args = struct {
    nr: u32,
    a: [6]u64 = [_]u64{0} ** 6,
};

/// Access to the calling process's memory. All functions are all-or-nothing.
pub const UserMemory = struct {
    ctx: *anyopaque,
    copy_in: *const fn (ctx: *anyopaque, addr: u64, dst: []u8) error{Fault}!void,
    copy_out: *const fn (ctx: *anyopaque, addr: u64, src: []const u8) error{Fault}!void,
    /// Check that `[addr, addr+len)` is writable without writing.
    probe_write: *const fn (ctx: *anyopaque, addr: u64, len: u64) error{Fault}!void,
};

pub const OpInfo = struct {
    nr: Nr,
    name: []const u8,
    /// Rights the caller's capability must carry (checked by the capability layer).
    rights: []const u8,
};

pub const table = [_]OpInfo{
    .{ .nr = .endpoint_create, .name = "endpoint_create", .rights = "none (charges channels quota)" },
    .{ .nr = .endpoint_close, .name = "endpoint_close", .rights = "ADMIN on the end" },
    .{ .nr = .send, .name = "send", .rights = "WRITE on the end; TRANSFER (+DUPLICATE for copy) on each enclosed capability; WAIT on cancel" },
    .{ .nr = .recv, .name = "recv", .rights = "READ on the end; WAIT on cancel" },
    .{ .nr = .cancel_create, .name = "cancel_create", .rights = "none" },
    .{ .nr = .cancel_signal, .name = "cancel_signal", .rights = "SIGNAL on the cancellation object" },
    .{ .nr = .endpoint_stats, .name = "endpoint_stats", .rights = "INSPECT on the end" },
    .{ .nr = .handle_release, .name = "handle_release", .rights = "none (releases the caller's own handle)" },
};

// ---------------------------------------------------------------- user layouts

pub const send_args_size = 48;
pub const desc_size = 16;
pub const recv_args_size = 48;
pub const recv_result_size = 88;
pub const stats_size = 32;
pub const create_out_size = 8;

const rd = std.mem.readInt;
const wr = std.mem.writeInt;

fn handleArg(v: u64) SysError!Handle {
    if (v > std.math.maxInt(u32)) return error.InvalidArgument;
    return Handle.fromRaw(@intCast(v));
}

fn optHandle(v: u32) ?Handle {
    return if (v == 0) null else Handle.fromRaw(v);
}

fn requireZero(a: *const [6]u64, from: usize) SysError!void {
    for (a[from..]) |v| if (v != 0) return error.InvalidArgument;
}

/// Run one system call for `caller`. Returns the wire status: negative on error,
/// otherwise the call's value (0 unless stated).
pub fn dispatch(k: *KernelIpc, mem: UserMemory, caller: SpaceId, args: Args) i64 {
    if (run(k, mem, caller, args)) |v| {
        return @intCast(v);
    } else |e| {
        return statusOf(e);
    }
}

fn run(k: *KernelIpc, mem: UserMemory, caller: SpaceId, args: Args) SysError!u32 {
    const nr = std.meta.intToEnum(Nr, args.nr) catch return error.NoSys;
    const a = &args.a;
    switch (nr) {
        .endpoint_create => {
            try requireZero(a, 1);
            try mem.probe_write(mem.ctx, a[0], create_out_size);
            const ends = try k.endpointCreate(caller);
            var out: [create_out_size]u8 = undefined;
            wr(u32, out[0..4], @intFromEnum(ends[0]), .little);
            wr(u32, out[4..8], @intFromEnum(ends[1]), .little);
            mem.copy_out(mem.ctx, a[0], &out) catch {
                // The caller can never learn the handles: take them back.
                k.releaseHandle(caller, ends[0]) catch {};
                k.releaseHandle(caller, ends[1]) catch {};
                k.ledger.record(.delivery_fault);
                return error.Fault;
            };
            return 0;
        },
        .endpoint_close => {
            try requireZero(a, 1);
            try k.endpointClose(caller, try handleArg(a[0]));
            return 0;
        },
        .cancel_create => {
            try requireZero(a, 0);
            return @intFromEnum(try k.cancelCreate(caller));
        },
        .cancel_signal => {
            try requireZero(a, 1);
            try k.cancelSignal(caller, try handleArg(a[0]));
            return 0;
        },
        .handle_release => {
            try requireZero(a, 1);
            try k.releaseHandle(caller, try handleArg(a[0]));
            return 0;
        },
        .endpoint_stats => {
            try requireZero(a, 2);
            const h = try handleArg(a[0]);
            try mem.probe_write(mem.ctx, a[1], stats_size);
            const st = try k.endpointStats(caller, h);
            var out = [_]u8{0} ** stats_size;
            wr(u32, out[0..4], st.queued_messages, .little);
            wr(u32, out[4..8], (@as(u32, @intFromBool(st.closed))) | (@as(u32, @intFromBool(st.peer_closed)) << 1), .little);
            wr(u64, out[8..16], st.queued_bytes, .little);
            wr(u32, out[16..20], st.reserved_messages, .little);
            wr(u32, out[20..24], st.users, .little);
            wr(u64, out[24..32], st.reserved_bytes, .little);
            mem.copy_out(mem.ctx, a[1], &out) catch return error.Fault;
            return 0;
        },
        .send => {
            try requireZero(a, 2);
            return try doSend(k, mem, caller, try handleArg(a[0]), a[1]);
        },
        .recv => {
            try requireZero(a, 2);
            return try doRecv(k, mem, caller, try handleArg(a[0]), a[1]);
        },
    }
}

fn doSend(k: *KernelIpc, mem: UserMemory, caller: SpaceId, ep: Handle, args_ptr: u64) SysError!u32 {
    var ab: [send_args_size]u8 = undefined;
    try mem.copy_in(mem.ctx, args_ptr, &ab);
    const msg_ptr = rd(u64, ab[0..8], .little);
    const msg_len = rd(u32, ab[8..12], .little);
    const desc_count = rd(u32, ab[12..16], .little);
    const desc_ptr = rd(u64, ab[16..24], .little);
    const timeout_ns = rd(u64, ab[24..32], .little);
    const cancel = rd(u32, ab[32..36], .little);
    const flags = rd(u32, ab[36..40], .little);
    const reserved = rd(u64, ab[40..48], .little);
    if (flags != 0 or reserved != 0) return error.InvalidArgument;

    const cfg = &k.config.ipc;
    if (msg_len < b2.ipc.wire_header_size or msg_len > cfg.wire.max_message_bytes) return error.InvalidArgument;
    if (desc_count > cfg.wire.max_handles or desc_count > ipc4.max_desc) return error.InvalidArgument;
    const w = kipc.Wait{ .timeout_ns = timeout_ns, .cancel = optHandle(cancel) };
    try k.validateWait(w);

    const buf = k.allocator.alloc(u8, msg_len) catch return error.QuotaExceeded;
    defer k.allocator.free(buf);
    try mem.copy_in(mem.ctx, msg_ptr, buf);

    var descs: [ipc4.max_desc]ipc4.Desc = undefined;
    const n: usize = desc_count;
    if (n > 0) {
        var db: [ipc4.max_desc * desc_size]u8 = undefined;
        try mem.copy_in(mem.ctx, desc_ptr, db[0 .. n * desc_size]);
        for (0..n) |i| {
            const o = i * desc_size;
            const handle = rd(u32, db[o..][0..4], .little);
            const otype_raw = db[o + 4];
            const mode_raw = db[o + 5];
            const res16 = rd(u16, db[o + 6 ..][0..2], .little);
            const rights_raw = rd(u32, db[o + 8 ..][0..4], .little);
            const res32 = rd(u32, db[o + 12 ..][0..4], .little);
            if (res16 != 0 or res32 != 0) return error.InvalidArgument;
            descs[i] = .{
                .handle = Handle.fromRaw(handle),
                .otype = std.meta.intToEnum(ipc4.ObjectType, otype_raw) catch return error.InvalidArgument,
                .mode = std.meta.intToEnum(ipc4.Mode, mode_raw) catch return error.InvalidArgument,
                .rights = try b2.caps.Rights.fromRaw(rights_raw),
            };
        }
    }

    var report: ipc4.SendReport = .{};
    k.send(caller, ep, buf, descs[0..n], w, &report) catch |e| {
        return sendFailed(k, mem, desc_ptr, descs[0..n], &report, e);
    };
    return 0;
}

fn sendFailed(k: *KernelIpc, mem: UserMemory, desc_ptr: u64, descs: []const ipc4.Desc, report: *const ipc4.SendReport, e: OmniError) SysError {
    _ = k;
    // A rolled-back MOVE returns under a new handle value: tell the caller where it is.
    for (report.rebound[0..report.rebound_count]) |rb| {
        for (descs, 0..) |d, i| {
            if (d.handle != rb.old) continue;
            var v: [4]u8 = undefined;
            wr(u32, &v, @intFromEnum(rb.new), .little);
            const addr = std.math.add(u64, desc_ptr, @as(u64, i) * desc_size) catch break;
            mem.copy_out(mem.ctx, addr, &v) catch {};
            break;
        }
    }
    return e;
}

fn doRecv(k: *KernelIpc, mem: UserMemory, caller: SpaceId, ep: Handle, args_ptr: u64) SysError!u32 {
    var ab: [recv_args_size]u8 = undefined;
    try mem.copy_in(mem.ctx, args_ptr, &ab);
    const buf_ptr = rd(u64, ab[0..8], .little);
    const buf_cap = rd(u32, ab[8..12], .little);
    const flags = rd(u32, ab[12..16], .little);
    const result_ptr = rd(u64, ab[16..24], .little);
    const timeout_ns = rd(u64, ab[24..32], .little);
    const cancel = rd(u32, ab[32..36], .little);
    const res32 = rd(u32, ab[36..40], .little);
    const res64 = rd(u64, ab[40..48], .little);
    if (flags != 0 or res32 != 0 or res64 != 0) return error.InvalidArgument;

    // The buffer must hold the largest possible payload, so a message is never
    // truncated and never consumed without being deliverable.
    const max_payload: u32 = k.config.ipc.wire.max_message_bytes - @as(u32, b2.ipc.wire_header_size);
    if (buf_cap < max_payload) return error.InvalidArgument;
    const w = kipc.Wait{ .timeout_ns = timeout_ns, .cancel = optHandle(cancel) };
    try k.validateWait(w);
    // Probe before consuming anything.
    try mem.probe_write(mem.ctx, buf_ptr, max_payload);
    try mem.probe_write(mem.ctx, result_ptr, recv_result_size);

    var out: ipc4.Received = undefined;
    try k.recv(caller, ep, w, &out);
    defer k.deinitReceived(&out);

    var rb = [_]u8{0} ** recv_result_size;
    wr(u32, rb[0..4], @intCast(out.payload.len), .little);
    wr(u16, rb[4..6], out.header.op, .little);
    wr(u16, rb[6..8], @intFromEnum(out.header.kind), .little);
    wr(u64, rb[8..16], out.header.request_id, .little);
    wr(u32, rb[16..20], @intFromBool(out.partial), .little);
    wr(u32, rb[20..24], out.count, .little);
    for (0..out.count) |i| {
        wr(u32, rb[24 + i * 8 ..][0..4], @as(u32, @intFromEnum(out.slots[i].status)), .little);
        wr(u32, rb[28 + i * 8 ..][0..4], @intFromEnum(out.slots[i].handle), .little);
    }
    const ok = blk: {
        mem.copy_out(mem.ctx, buf_ptr, out.payload) catch break :blk false;
        mem.copy_out(mem.ctx, result_ptr, &rb) catch break :blk false;
        break :blk true;
    };
    if (!ok) {
        // The message was consumed and cannot be returned. Do not leave authority the
        // caller can never name: release what was installed. See FAILURE-MODEL.md.
        for (out.slots[0..out.count]) |s| {
            if (s.status == .delivered) k.releaseHandle(caller, s.handle) catch {};
        }
        k.ledger.record(.delivery_fault);
        return error.Fault;
    }
    return 0;
}
