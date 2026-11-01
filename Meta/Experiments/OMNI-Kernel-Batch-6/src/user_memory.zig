//! User-memory integration (Batch 6): the bridge between the Batch 5 `UserMemory`
//! abstraction and kernel-managed user memory.
//!
//! SIMULATION. `PageModel` is a hosted stand-in for one address space's page tables
//! (4 KiB pages, read/write permission, present bit, an upper bound for user addresses).
//! No real MMU, page-table walker, TLB or fault handler is involved. What it does pin
//! down is the CONTRACT a real implementation must meet, and the behaviour the Batch 5
//! syscall layer depends on:
//!   * a user address is never dereferenced: all access goes through the copy functions,
//!     which check the address range with overflow-safe arithmetic first;
//!   * a copy may span pages (unlike the Batch 5 hosted model, which is stricter and never
//!     spans mappings); every page touched must be present and have the right permission;
//!   * `copy_in` writes only into a kernel buffer, so a fault leaves no user-visible effect;
//!   * `copy_out` probes every page first; a fault found by the probe changes nothing;
//!   * a fault that appears AFTER the probe (another thread unmapped a page: the race the
//!     real kernel has too) can leave a PARTIAL write to user memory. It is reported as a
//!     fault and counted (`partial_bytes_written`); the written range is unspecified;
//!   * a bridge is bound to ONE calling address space and refuses any other.

const std = @import("std");
const k3 = @import("kernel3");
const omni5 = @import("omni5");
const sys = omni5.syscall;

pub const SpaceId = k3.SpaceId;
pub const UserMemory = sys.UserMemory;

pub const page_size: u64 = 4096;
pub const max_pages = 64;

/// First address NOT available to user mode, per architecture (documented in
/// docs/USER-MEMORY-INTEGRATION.md): 47-bit lower canonical half on x86-64, 48-bit
/// TTBR0 range on ARM64 with a 48-bit VA configuration.
pub const user_top_x86_64: u64 = 0x0000_8000_0000_0000;
pub const user_top_aarch64: u64 = 0x0001_0000_0000_0000;

const Page = struct {
    used: bool = false,
    vpn: u64 = 0,
    present: bool = false,
    readable: bool = false,
    writable: bool = false,
    data: [page_size]u8 = undefined,
};

pub const MapError = error{ BadRange, Overlap, TooManyPages };

pub const PageModel = struct {
    owner: SpaceId,
    user_top: u64,
    pages: [max_pages]Page = [_]Page{.{}} ** max_pages,
    faults: u64 = 0,
    partial_bytes_written: u64 = 0,
    /// Test hook: runs between the write probe and the write pass of `copy_out`.
    after_probe: ?*const fn (ctx: *anyopaque, m: *PageModel) void = null,
    after_probe_ctx: *anyopaque = undefined,

    pub fn init(owner: SpaceId, user_top: u64) PageModel {
        return .{ .owner = owner, .user_top = user_top };
    }

    fn find(self: *PageModel, vpn: u64) ?*Page {
        for (&self.pages) |*p| {
            if (p.used and p.vpn == vpn) return p;
        }
        return null;
    }

    /// Map `len` bytes (rounded up to whole pages) at page-aligned `addr`, zero-filled.
    pub fn map(self: *PageModel, addr: u64, len: u64, readable: bool, writable: bool) MapError!void {
        if (len == 0 or addr % page_size != 0 or addr == 0) return error.BadRange;
        const end = std.math.add(u64, addr, len) catch return error.BadRange;
        const rounded = std.math.add(u64, end, page_size - 1) catch return error.BadRange;
        const end_page = rounded / page_size;
        if (end_page * page_size > self.user_top) return error.BadRange;
        const first = addr / page_size;
        var free: u64 = 0;
        for (self.pages) |p| {
            if (!p.used) free += 1;
        }
        if (end_page - first > free) return error.TooManyPages;
        var v = first;
        while (v < end_page) : (v += 1) {
            if (self.find(v) != null) return error.Overlap;
        }
        v = first;
        while (v < end_page) : (v += 1) {
            for (&self.pages) |*p| {
                if (p.used) continue;
                p.* = .{ .used = true, .vpn = v, .present = true, .readable = readable, .writable = writable };
                @memset(&p.data, 0);
                break;
            }
        }
    }

    pub fn unmap(self: *PageModel, addr: u64) void {
        if (self.find(addr / page_size)) |p| p.* = .{};
    }

    pub fn setPresent(self: *PageModel, addr: u64, present: bool) void {
        if (self.find(addr / page_size)) |p| p.present = present;
    }

    /// TEST ONLY: user-mode view of memory. Honours nothing but existence, so tests can
    /// prepare and inspect buffers whatever their kernel-side permissions are.
    pub fn userStore(self: *PageModel, addr: u64, src: []const u8) error{Fault}!void {
        var done: usize = 0;
        while (done < src.len) {
            const a = addr + done;
            const p = self.find(a / page_size) orelse return error.Fault;
            const off: usize = @intCast(a % page_size);
            const n = @min(src.len - done, page_size - off);
            @memcpy(p.data[off..][0..n], src[done..][0..n]);
            done += n;
        }
    }

    pub fn userLoad(self: *PageModel, addr: u64, dst: []u8) error{Fault}!void {
        var done: usize = 0;
        while (done < dst.len) {
            const a = addr + done;
            const p = self.find(a / page_size) orelse return error.Fault;
            const off: usize = @intCast(a % page_size);
            const n = @min(dst.len - done, page_size - off);
            @memcpy(dst[done..][0..n], p.data[off..][0..n]);
            done += n;
        }
    }

    // ------------------------------------------------------------ kernel-side access

    fn checkRange(self: *PageModel, addr: u64, len: u64) error{Fault}!void {
        const end = std.math.add(u64, addr, len) catch {
            self.faults += 1;
            return error.Fault;
        };
        if (end > self.user_top) {
            self.faults += 1;
            return error.Fault;
        }
    }

    fn walk(self: *PageModel, addr: u64, len: u64, need_write: bool) error{Fault}!void {
        if (len == 0) return;
        try self.checkRange(addr, len);
        var a = addr;
        const end = addr + len;
        while (a < end) {
            const p = self.find(a / page_size) orelse {
                self.faults += 1;
                return error.Fault;
            };
            const ok = p.present and (if (need_write) p.writable else p.readable);
            if (!ok) {
                self.faults += 1;
                return error.Fault;
            }
            a = (a / page_size + 1) * page_size;
        }
    }

    pub fn copyIn(self: *PageModel, addr: u64, dst: []u8) error{Fault}!void {
        if (dst.len == 0) return;
        try self.walk(addr, dst.len, false);
        var done: usize = 0;
        while (done < dst.len) {
            const a = addr + done;
            const p = self.find(a / page_size).?; // `walk` just proved it exists
            const off: usize = @intCast(a % page_size);
            const n = @min(dst.len - done, page_size - off);
            @memcpy(dst[done..][0..n], p.data[off..][0..n]);
            done += n;
        }
    }

    pub fn probeWrite(self: *PageModel, addr: u64, len: u64) error{Fault}!void {
        try self.walk(addr, len, true);
    }

    pub fn copyOut(self: *PageModel, addr: u64, src: []const u8) error{Fault}!void {
        if (src.len == 0) return;
        try self.walk(addr, src.len, true);
        if (self.after_probe) |f| f(self.after_probe_ctx, self);
        var done: usize = 0;
        while (done < src.len) {
            const a = addr + done;
            const off: usize = @intCast(a % page_size);
            const n = @min(src.len - done, page_size - off);
            const p = self.find(a / page_size) orelse return self.partial(done);
            if (!(p.present and p.writable)) return self.partial(done);
            @memcpy(p.data[off..][0..n], src[done..][0..n]);
            done += n;
        }
    }

    fn partial(self: *PageModel, written: usize) error{Fault} {
        self.faults += 1;
        self.partial_bytes_written += written;
        return error.Fault;
    }
};

// -------------------------------------------------------------------- the bridge

pub const BindError = error{WrongAddressSpace};

/// One syscall's view of user memory: a page model bound to the calling address space.
pub const Binding = struct {
    model: *PageModel,
    caller: SpaceId,

    fn ctxOf(ctx: *anyopaque) *Binding {
        return @ptrCast(@alignCast(ctx));
    }

    fn valid(b: *Binding) bool {
        return b.model.owner.eql(b.caller);
    }

    fn copyIn(ctx: *anyopaque, addr: u64, dst: []u8) error{Fault}!void {
        const b = ctxOf(ctx);
        if (!b.valid()) return error.Fault;
        return b.model.copyIn(addr, dst);
    }

    fn copyOut(ctx: *anyopaque, addr: u64, src: []const u8) error{Fault}!void {
        const b = ctxOf(ctx);
        if (!b.valid()) return error.Fault;
        return b.model.copyOut(addr, src);
    }

    fn probeWrite(ctx: *anyopaque, addr: u64, len: u64) error{Fault}!void {
        const b = ctxOf(ctx);
        if (!b.valid()) return error.Fault;
        return b.model.probeWrite(addr, len);
    }

    /// Bind `model` for `caller`. Fails closed if the model belongs to another space.
    pub fn init(caller: SpaceId, model: *PageModel) BindError!Binding {
        if (!model.owner.eql(caller)) return error.WrongAddressSpace;
        return .{ .model = model, .caller = caller };
    }

    pub fn memory(b: *Binding) UserMemory {
        return .{ .ctx = b, .copy_in = copyIn, .copy_out = copyOut, .probe_write = probeWrite };
    }
};

// -------------------------------------------------------------------- registry

pub const max_spaces = 16;

/// Which page model belongs to which address space. The key is the SPACE; the lookup key
/// always comes from trusted kernel state (the thread's owner), never from user input.
pub const Registry = struct {
    const Entry = struct { space: SpaceId, model: *PageModel };
    entries: [max_spaces]?Entry = [_]?Entry{null} ** max_spaces,

    pub const Error = error{ TooManySpaces, AlreadyRegistered, WrongAddressSpace };

    pub fn register(self: *Registry, space: SpaceId, model: *PageModel) Error!void {
        if (!model.owner.eql(space)) return error.WrongAddressSpace;
        var free: ?usize = null;
        for (self.entries, 0..) |e, i| {
            if (e) |x| {
                if (x.space.eql(space)) return error.AlreadyRegistered;
            } else if (free == null) free = i;
        }
        const slot = free orelse return error.TooManySpaces;
        self.entries[slot] = .{ .space = space, .model = model };
    }

    pub fn unregister(self: *Registry, space: SpaceId) void {
        for (&self.entries) |*e| {
            if (e.*) |x| {
                if (x.space.eql(space)) e.* = null;
            }
        }
    }

    pub fn modelOf(self: *Registry, space: SpaceId) ?*PageModel {
        for (self.entries) |e| {
            if (e) |x| {
                if (x.space.eql(space)) return x.model;
            }
        }
        return null;
    }
};
