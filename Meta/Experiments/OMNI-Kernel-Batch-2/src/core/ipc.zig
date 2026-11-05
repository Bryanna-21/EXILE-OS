//! Minimal IPC message representation and validation (Batch 2).
//! Batch 1 references: A.3 (message header), B.5 (IPC validation), C.4 (omni_ipc).
//!
//! SCOPE: this module encodes, decodes and VALIDATES message bytes and the list
//! of handles a sender wants to transfer. It does not queue, deliver, block,
//! schedule, or move handles between tables. There is no cross-process IPC and
//! no transport in Batch 2.
//!
//! WIRE LAYOUT (PROVISIONAL; little-endian; this is a concrete refinement of the
//! Batch 1 "conceptual" header, and fills its implicit 4 padding bytes with an
//! explicit `kind` and `reserved`):
//!
//!   off  size  field
//!     0    4   size         total message bytes, header+descriptor+payload
//!     4    2   abi_ver      (major << 8) | minor ; must equal abi_version
//!     6    2   op           interface operation number, must be non-zero
//!     8    4   flags        no flags defined in 0.1; must be 0
//!    12    2   kind         MsgKind
//!    14    2   reserved     must be 0
//!    16    8   request_id   caller-chosen, echoed in replies, NOT trusted for
//!                           authorization (Batch 1 A.3)
//!    24    4   payload_len  bytes of payload following the descriptor
//!    28    4   handle_count number of handles being transferred (the handle
//!                           values travel out-of-band, as in Batch 1 C.4)
//!    32  ...   payload
//!
//! VALIDATION ORDER (fixed so rejection is deterministic):
//!   1. bytes.len >= wire_header_size                        else InvalidArgument
//!   2. abi_ver == abi_version                               else BadVersion
//!   3. flags == 0, reserved == 0, op != 0                   else InvalidArgument
//!   4. kind is a known MsgKind                              else InvalidArgument
//!   5. size == bytes.len and size <= limits.max_message_bytes else InvalidArgument
//!   6. payload_len == size - wire_header_size (checked)     else InvalidArgument
//!   7. handle_count <= limits.max_handles                   else InvalidArgument
//! Decoding never indexes outside `bytes` and never allocates.

const std = @import("std");
const OmniError = @import("error.zig").OmniError;
const ids = @import("ids.zig");
const caps = @import("capabilities.zig");

/// OMNI-ABI 0.1 as (major << 8) | minor.
pub const abi_version: u16 = 0x0001;

pub const wire_header_size: usize = 32;

/// Message kinds from Batch 1 C.4 (send / call / reply).
pub const MsgKind = enum(u16) {
    send = 1,
    call = 2,
    reply = 3,
};

/// Limits are parameters, not ABI constants (Batch 1 C.4: "queryable limits").
/// The defaults are PROVISIONAL and unvalidated.
pub const Limits = struct {
    max_message_bytes: u32 = 4096,
    max_handles: u32 = 8,
};

pub const Header = struct {
    abi_ver: u16 = abi_version,
    op: u16,
    flags: u32 = 0,
    kind: MsgKind,
    request_id: u64 = 0,
    handle_count: u32 = 0,
};

/// A validated message. `payload` borrows from the input bytes.
pub const Message = struct {
    header: Header,
    payload: []const u8,
};

/// Total wire size for a payload length, checked. Rejects lengths that cannot
/// be represented in the u32 `size` field.
pub fn totalSize(payload_len: u64) OmniError!u32 {
    const total = std.math.add(u64, payload_len, wire_header_size) catch return error.Overflow;
    return std.math.cast(u32, total) orelse error.Overflow;
}

/// Encode `header` + `payload` into `out`; returns the number of bytes written.
/// Validates against `limits` so only well-formed messages can be produced.
/// On error `out` may have been partially written and must be discarded.
pub fn encode(out: []u8, header: Header, payload: []const u8, limits: Limits) OmniError!usize {
    const size = try totalSize(payload.len);
    if (size > limits.max_message_bytes) return error.InvalidArgument;
    if (out.len < size) return error.InvalidArgument;
    if (header.handle_count > limits.max_handles) return error.InvalidArgument;
    if (header.op == 0) return error.InvalidArgument;

    std.mem.writeInt(u32, out[0..4], size, .little);
    std.mem.writeInt(u16, out[4..6], header.abi_ver, .little);
    std.mem.writeInt(u16, out[6..8], header.op, .little);
    std.mem.writeInt(u32, out[8..12], header.flags, .little);
    std.mem.writeInt(u16, out[12..14], @intFromEnum(header.kind), .little);
    std.mem.writeInt(u16, out[14..16], 0, .little);
    std.mem.writeInt(u64, out[16..24], header.request_id, .little);
    std.mem.writeInt(u32, out[24..28], @intCast(payload.len), .little); // size check above bounds this
    std.mem.writeInt(u32, out[28..32], header.handle_count, .little);
    @memcpy(out[wire_header_size..][0..payload.len], payload);
    return size;
}

/// Decode and validate untrusted bytes. See the validation order above.
pub fn decode(bytes: []const u8, limits: Limits) OmniError!Message {
    if (bytes.len < wire_header_size) return error.InvalidArgument;

    const size = std.mem.readInt(u32, bytes[0..4], .little);
    const abi_ver = std.mem.readInt(u16, bytes[4..6], .little);
    const op = std.mem.readInt(u16, bytes[6..8], .little);
    const flags = std.mem.readInt(u32, bytes[8..12], .little);
    const kind_raw = std.mem.readInt(u16, bytes[12..14], .little);
    const reserved = std.mem.readInt(u16, bytes[14..16], .little);
    const request_id = std.mem.readInt(u64, bytes[16..24], .little);
    const payload_len = std.mem.readInt(u32, bytes[24..28], .little);
    const handle_count = std.mem.readInt(u32, bytes[28..32], .little);

    if (abi_ver != abi_version) return error.BadVersion;
    if (flags != 0 or reserved != 0 or op == 0) return error.InvalidArgument;
    const kind = std.meta.intToEnum(MsgKind, kind_raw) catch return error.InvalidArgument;
    if (size != bytes.len or size > limits.max_message_bytes) return error.InvalidArgument;
    const expected_payload = std.math.sub(u32, size, @intCast(wire_header_size)) catch return error.InvalidArgument;
    if (payload_len != expected_payload) return error.InvalidArgument;
    if (handle_count > limits.max_handles) return error.InvalidArgument;

    return .{
        .header = .{
            .abi_ver = abi_ver,
            .op = op,
            .flags = flags,
            .kind = kind,
            .request_id = request_id,
            .handle_count = handle_count,
        },
        .payload = bytes[wire_header_size..][0..payload_len],
    };
}

/// Validate the handles a sender wants to transfer against its capability table.
///
/// Requirements (all-or-nothing; the table is only read, never modified):
///   * `handles.len` equals the message's declared `handle_count`;
///   * every handle is live and holds the TRANSFER right;
///   * no handle appears twice (duplicate transfer is disallowed in 0.1, a
///     PROVISIONAL decision from Batch 1 C.4).
/// Errors: count mismatch -> InvalidArgument; stale/absent -> InvalidHandle;
/// missing TRANSFER -> AccessDenied; duplicate -> InvalidArgument.
/// This function does NOT move or copy any handle.
pub fn validateTransferList(table: anytype, handles: []const ids.Handle, declared_count: u32) OmniError!void {
    if (handles.len != declared_count) return error.InvalidArgument;
    for (handles) |h| {
        const r = try table.peek(h);
        if (!r.rights.transfer) return error.AccessDenied;
    }
    for (handles, 0..) |a, i| {
        for (handles[i + 1 ..]) |b| {
            if (a == b) return error.InvalidArgument;
        }
    }
}
