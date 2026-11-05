//! Hosted model of user memory (Batch 5).
//!
//! HOSTED. A real kernel validates user addresses against the caller's address space
//! and copies with fault handling; this model captures the behaviour the syscall
//! layer depends on: a copy is all-or-nothing, never spans two mappings, never
//! writes read-only memory, and fails (never wraps) on address arithmetic overflow.
//! It also supports fault injection for failure-recovery tests.

const std = @import("std");
const syscall = @import("syscall.zig");

pub const max_segments = 8;

pub const Segment = struct {
    base: u64,
    data: []u8,
    writable: bool,
};

pub const HostedUserMemory = struct {
    segments: [max_segments]?Segment = [_]?Segment{null} ** max_segments,
    /// Fail the copy-out with this zero-based index (counting every `copy_out` call), once.
    fail_copy_out_at: ?u32 = null,
    copy_out_calls: u32 = 0,
    /// Fail the next `probe_write` calls with this count (decrementing), for tests.
    fail_probes: u32 = 0,

    pub const MapError = error{ Overlap, TooManySegments, BadRange };

    pub fn map(self: *HostedUserMemory, base: u64, data: []u8, writable: bool) MapError!void {
        if (base == 0) return error.BadRange;
        const end = std.math.add(u64, base, data.len) catch return error.BadRange;
        var free: ?usize = null;
        for (self.segments, 0..) |maybe, i| {
            if (maybe) |s| {
                const s_end = s.base + s.data.len; // cannot overflow: checked when mapped
                if (base < s_end and s.base < end) return error.Overlap;
            } else if (free == null) free = i;
        }
        const slot = free orelse return error.TooManySegments;
        self.segments[slot] = .{ .base = base, .data = data, .writable = writable };
    }

    pub fn unmap(self: *HostedUserMemory, base: u64) void {
        for (&self.segments) |*maybe| {
            if (maybe.*) |s| if (s.base == base) {
                maybe.* = null;
            };
        }
    }

    fn find(self: *HostedUserMemory, addr: u64, len: u64, need_write: bool) error{Fault}![]u8 {
        if (len == 0) return &.{};
        if (addr == 0) return error.Fault;
        const end = std.math.add(u64, addr, len) catch return error.Fault;
        for (self.segments) |maybe| {
            const s = maybe orelse continue;
            const s_end = s.base + s.data.len;
            if (addr >= s.base and end <= s_end) {
                if (need_write and !s.writable) return error.Fault;
                const off: usize = @intCast(addr - s.base);
                return s.data[off..][0..@as(usize, @intCast(len))];
            }
        }
        return error.Fault;
    }

    fn copyIn(ctx: *anyopaque, addr: u64, dst: []u8) error{Fault}!void {
        const self: *HostedUserMemory = @ptrCast(@alignCast(ctx));
        const src = try self.find(addr, dst.len, false);
        @memcpy(dst, src);
    }

    fn copyOut(ctx: *anyopaque, addr: u64, src: []const u8) error{Fault}!void {
        const self: *HostedUserMemory = @ptrCast(@alignCast(ctx));
        const n = self.copy_out_calls;
        self.copy_out_calls += 1;
        if (self.fail_copy_out_at) |at| if (at == n) {
            self.fail_copy_out_at = null;
            return error.Fault;
        };
        const dst = try self.find(addr, src.len, true);
        @memcpy(dst, src);
    }

    fn probeWrite(ctx: *anyopaque, addr: u64, len: u64) error{Fault}!void {
        const self: *HostedUserMemory = @ptrCast(@alignCast(ctx));
        if (self.fail_probes > 0) {
            self.fail_probes -= 1;
            return error.Fault;
        }
        _ = try self.find(addr, len, true);
    }

    pub fn memory(self: *HostedUserMemory) syscall.UserMemory {
        return .{ .ctx = self, .copy_in = copyIn, .copy_out = copyOut, .probe_write = probeWrite };
    }
};
