//! OMNI-Kernel Batch 3: hosted capability lifecycle, revocation and lifetime model.
//!
//! This is a HOSTED model that runs in an ordinary process. It enforces nothing
//! against code in the same process, and it is not a kernel. See
//! docs/CAPABILITY-LIFECYCLE.md, docs/REVOCATION-MODEL.md and docs/CONCURRENCY.md
//! for the rules this file implements; the comments here state invariants and
//! point at those documents rather than repeating them.
//!
//! OWNERSHIP MODEL (summary)
//!   * A `Kernel` owns every table. Callers hold only value types: `SpaceId`,
//!     `Handle`, `OpToken`. No pointer into kernel tables escapes.
//!   * A *capability node* is the kernel record of one capability. It is owned
//!     by exactly one space (its handle table) and sits in a derivation tree.
//!   * An *object* is owned by the kernel registry. It is kept alive by the
//!     number of nodes that refer to it (`cap_refs`) plus the number of in-flight
//!     operation pins (`pins`). It is destroyed exactly once, when both reach zero.
//!   * A space owns its handle table and an account. A closed space stays in
//!     memory (a "zombie") only while objects are still billed to it.
//!
//! SYNCHRONISATION
//!   * One mutex (`Kernel.mutex`) guards ALL kernel state. Every public method
//!     takes it, does its fallible work first (reserve-then-commit), commits with
//!     infallible code, and releases it.
//!   * Object destructors and the final resource refund run AFTER the commit
//!     critical section, with the mutex NOT held (see `finishDestroy`).
//!   * The memory allocator is called while the mutex is held; it must not call
//!     back into the kernel.

const std = @import("std");
const Allocator = std.mem.Allocator;
const b2 = @import("batch2");
const GenArena = @import("gen_arena.zig").GenArena;

pub const OmniError = b2.err.OmniError;
pub const Handle = b2.ids.Handle;
pub const ObjectId = b2.ids.ObjectId;
pub const ObjectIdAllocator = b2.ids.ObjectIdAllocator;
pub const Rights = b2.caps.Rights;
pub const ObjectType = b2.caps.ObjectType;
pub const Resolved = b2.caps.Resolved;
pub const Resource = b2.res.Resource;
pub const Counters = b2.res.Counters;
pub const Account = b2.res.Account;

/// Destructor callback. Runs with the kernel mutex NOT held, exactly once per
/// object, after the last capability and the last pin are gone. It may call back
/// into the kernel (the kernel is consistent and unlocked when it runs).
pub const DestroyFn = *const fn (ctx: ?*anyopaque, id: ObjectId) void;

/// Handle tables hold at most this many slots (16-bit index, 0xFFFF reserved).
pub const table_slot_limit: usize = 65535;

const Object = struct {
    id: ObjectId,
    otype: ObjectType,
    state: enum { live, dying },
    cap_refs: u32,
    pins: u32,
    billed_space: u32,
    charge_res: Resource,
    charge_amount: u64,
    dtor: ?DestroyFn,
    ctx: ?*anyopaque,
    next_dying: u32,
    // Scratch used only by `checkInvariants`.
    chk_refs: u32 = 0,
    chk_pins: u32 = 0,
};

const none_node = std.math.maxInt(u32);

const Node = struct {
    object: u32,
    otype: ObjectType,
    rights: Rights,
    space: u32,
    slot: u16,
    parent: u32,
    first_child: u32,
    prev_sibling: u32,
    next_sibling: u32,
};

const Pin = struct { object: u32 };

const ObjArena = GenArena(Object, u32, u32);
const NodeArena = GenArena(Node, u32, u32);
const PinArena = GenArena(Pin, u32, u32);
const TableArena = GenArena(NodeArena.Ref, u16, u16);

const Space = struct {
    account: Account,
    table: TableArena,
    open: bool = true,
    billed_objects: u64 = 0,
    // Scratch used only by `checkInvariants`.
    chk_nodes: u64 = 0,
    chk_billed: u64 = 0,
    chk_usage: Counters = .{},
};

const SpaceArena = GenArena(*Space, u32, u32);

/// Names an address space (a handle table plus an account). Generation-checked.
pub const SpaceId = SpaceArena.Ref;

pub const Limits = struct {
    max_spaces: u32 = 64,
    max_objects: u32 = 4096,
    max_nodes: u32 = 8192,
    max_pins: u32 = 1024,
    /// Limits of the root account. Every space account is a child of it.
    root: Counters,
};

pub const CreateOptions = struct {
    /// Charged to the creating space's account at creation, refunded when the
    /// object is destroyed. Zero amount means no charge.
    charge_resource: Resource = .pages,
    charge_amount: u64 = 0,
    destroy_fn: ?DestroyFn = null,
    destroy_ctx: ?*anyopaque = null,
};

/// Proof that an operation was authorized at `beginOp`. See `opStillAuthorized`.
pub const OpToken = struct {
    pin: PinArena.Ref,
    node: NodeArena.Ref,
    object: ObjectId,
};

pub const Stats = struct {
    spaces: usize,
    objects: usize,
    dying_objects: usize,
    nodes: usize,
    pins: usize,
    retired_nodes: usize,
    retired_objects: usize,
    retired_pins: usize,
    retired_spaces: usize,
};

/// Singly linked list (through `Object.next_dying`) of objects whose destruction
/// is owed after the current critical section. Per operation, on the stack.
const DestroyList = struct {
    head: u32 = ObjArena.none,
};

fn counterPtr(c: *Counters, r: Resource) *u64 {
    return switch (r) {
        .pages => &c.pages,
        .handles => &c.handles,
        .threads => &c.threads,
        .channels => &c.channels,
        .queued_bytes => &c.queued_bytes,
    };
}

fn mustRelease(acct: *Account, r: Resource, amount: u64) void {
    acct.release(r, amount) catch @panic("OMNI Batch 3: accounting underflow (broken invariant)");
}

fn mustFree(arena: anytype, ref: anytype) void {
    _ = arena.free(ref) catch @panic("OMNI Batch 3: free of a non-live slot (broken invariant)");
}

pub const Kernel = struct {
    allocator: Allocator,
    mutex: std.Thread.Mutex = .{},
    root_account: Account,
    spaces: SpaceArena,
    objects: ObjArena,
    nodes: NodeArena,
    pins: PinArena,
    ids: ObjectIdAllocator = .{},

    /// Heap-allocates the kernel so that account pointers stay stable.
    pub fn create(allocator: Allocator, limits: Limits) OmniError!*Kernel {
        const k = allocator.create(Kernel) catch return error.QuotaExceeded;
        k.* = .{
            .allocator = allocator,
            .root_account = .{ .limits = limits.root },
            .spaces = SpaceArena.init(limits.max_spaces),
            .objects = ObjArena.init(limits.max_objects),
            .nodes = NodeArena.init(limits.max_nodes),
            .pins = PinArena.init(limits.max_pins),
        };
        return k;
    }

    /// Single-threaded teardown: no other thread may use the kernel. Closes every
    /// space, runs the destructor of every remaining object (including objects
    /// still pinned: pins do not survive teardown), and frees all memory.
    pub fn destroy(self: *Kernel) void {
        var i: usize = 0;
        while (i < self.spaces.slotCount()) : (i += 1) {
            const ref = blk: {
                self.mutex.lock();
                defer self.mutex.unlock();
                const r = self.spaces.refOf(i) orelse break :blk null;
                if (!self.spaces.ptrAt(i).?.*.open) break :blk null;
                break :blk r;
            };
            if (ref) |r| self.closeSpace(r) catch {};
        }
        var dl = DestroyList{};
        {
            self.mutex.lock();
            defer self.mutex.unlock();
            var j: usize = 0;
            while (j < self.objects.slotCount()) : (j += 1) {
                if (self.objects.ptrAt(j)) |o| {
                    if (o.state == .live) {
                        o.state = .dying;
                        o.pins = 0;
                        o.next_dying = dl.head;
                        dl.head = @intCast(j);
                    }
                }
            }
        }
        self.finishDestroy(&dl);
        const allocator = self.allocator;
        self.spaces.deinit(allocator);
        self.objects.deinit(allocator);
        self.nodes.deinit(allocator);
        self.pins.deinit(allocator);
        allocator.destroy(self);
    }

    // ------------------------------------------------------------------ spaces

    /// Create a space whose account is a child of the root account. Errors:
    /// `QuotaExceeded` (limits exceed the root's remaining budget, space table
    /// full, or allocation failed). On error nothing changed.
    pub fn createSpace(self: *Kernel, limits: Counters) OmniError!SpaceId {
        self.mutex.lock();
        defer self.mutex.unlock();
        const account = try Account.create(limits, &self.root_account);
        const sp = self.allocator.create(Space) catch return error.QuotaExceeded;
        errdefer self.allocator.destroy(sp);
        self.spaces.reserveOne(self.allocator) catch return error.QuotaExceeded;
        sp.* = .{ .account = account, .table = TableArena.init(table_slot_limit) };
        return self.spaces.allocAssumeCapacity(sp);
    }

    /// Close a space: release every capability it holds (children of released
    /// nodes are re-parented, as in `release`), then mark it closed. Objects whose
    /// last reference this was are destroyed after the critical section. A closed
    /// space rejects every operation with `PeerDead`. Closing twice: `PeerDead`.
    pub fn closeSpace(self: *Kernel, id: SpaceId) OmniError!void {
        var dl = DestroyList{};
        const result: OmniError!void = blk: {
            self.mutex.lock();
            defer self.mutex.unlock();
            break :blk self.closeSpaceLocked(id, &dl);
        };
        self.finishDestroy(&dl);
        return result;
    }

    fn closeSpaceLocked(self: *Kernel, id: SpaceId, dl: *DestroyList) OmniError!void {
        const sp = try self.openSpace(id);
        sp.open = false;
        var i: usize = 0;
        while (i < sp.table.slotCount()) : (i += 1) {
            if (sp.table.ptrAt(i)) |nref| self.removeNode(nref.index, dl);
        }
        if (sp.billed_objects == 0) self.freeSpace(id.index);
    }

    fn openSpace(self: *Kernel, id: SpaceId) OmniError!*Space {
        const slot = self.spaces.get(id) orelse return error.InvalidHandle;
        if (!slot.*.open) return error.PeerDead;
        return slot.*;
    }

    fn freeSpace(self: *Kernel, index: u32) void {
        const sp = self.spaces.ptrAt(index).?.*;
        std.debug.assert(sp.table.liveCount() == 0);
        std.debug.assert(sp.billed_objects == 0);
        const ref = self.spaces.refOf(index).?;
        mustFree(&self.spaces, ref);
        sp.table.deinit(self.allocator);
        self.allocator.destroy(sp);
    }

    // ------------------------------------------------------------ handle lookup

    fn resolveNode(self: *Kernel, sp: *Space, h: Handle) OmniError!u32 {
        if (h == .invalid) return error.InvalidHandle;
        const nref = sp.table.get(.{ .index = h.index(), .gen = h.generation() }) orelse
            return error.InvalidHandle;
        if (self.nodes.get(nref.*) == null) return error.Internal;
        return nref.index;
    }

    fn authorize(self: *Kernel, sp: *Space, h: Handle, expected: ObjectType, required: Rights) OmniError!u32 {
        if (!required.isValid()) return error.InvalidArgument;
        const ni = try self.resolveNode(sp, h);
        const n = self.nodes.ptrAt(ni).?;
        if (n.otype != expected) return error.InvalidHandle;
        if (!n.rights.contains(required)) return error.AccessDenied;
        return ni;
    }

    /// Check a handle against an expected type and required rights. Same error
    /// rules as Batch 2 `CapabilityTable.lookup`, plus `PeerDead` for a closed
    /// space and `InvalidHandle` for a stale `SpaceId`.
    pub fn lookup(self: *Kernel, space: SpaceId, h: Handle, expected: ObjectType, required: Rights) OmniError!Resolved {
        self.mutex.lock();
        defer self.mutex.unlock();
        const sp = try self.openSpace(space);
        const ni = try self.authorize(sp, h, expected, required);
        const n = self.nodes.ptrAt(ni).?;
        return .{ .object = self.objects.ptrAt(n.object).?.id, .otype = n.otype, .rights = n.rights };
    }

    // ---------------------------------------------------------------- creation

    /// Create an object and its first (root) capability in `space`. Charges one
    /// `handles` unit plus `opts.charge_*` to the space's account. All-or-nothing.
    pub fn createObject(self: *Kernel, space: SpaceId, otype: ObjectType, rights: Rights, opts: CreateOptions) OmniError!Handle {
        self.mutex.lock();
        defer self.mutex.unlock();
        const sp = try self.openSpace(space);
        if (!rights.isValid()) return error.InvalidArgument;

        // Phase 1: fallible work that changes no kernel table contents.
        self.objects.reserveOne(self.allocator) catch return error.QuotaExceeded;
        self.nodes.reserveOne(self.allocator) catch return error.QuotaExceeded;
        sp.table.reserveOne(self.allocator) catch return error.QuotaExceeded;
        var next_ids = self.ids;
        const oid = try next_ids.next();
        // Phase 2: reversible account charges.
        try sp.account.reserve(.handles, 1);
        if (opts.charge_amount > 0) {
            sp.account.reserve(opts.charge_resource, opts.charge_amount) catch |e| {
                mustRelease(&sp.account, .handles, 1);
                return e;
            };
        }
        // Phase 3: infallible commit.
        self.ids = next_ids;
        const oref = self.objects.allocAssumeCapacity(.{
            .id = oid,
            .otype = otype,
            .state = .live,
            .cap_refs = 1,
            .pins = 0,
            .billed_space = space.index,
            .charge_res = opts.charge_resource,
            .charge_amount = opts.charge_amount,
            .dtor = opts.destroy_fn,
            .ctx = opts.destroy_ctx,
            .next_dying = ObjArena.none,
        });
        const nref = self.nodes.allocAssumeCapacity(.{
            .object = oref.index,
            .otype = otype,
            .rights = rights,
            .space = space.index,
            .slot = 0,
            .parent = none_node,
            .first_child = none_node,
            .prev_sibling = none_node,
            .next_sibling = none_node,
        });
        const tref = sp.table.allocAssumeCapacity(nref);
        self.nodes.ptrAt(nref.index).?.slot = tref.index;
        sp.billed_objects += 1;
        return Handle.make(tref.index, tref.gen) catch @panic("OMNI Batch 3: generation 0 issued");
    }

    // ------------------------------------------------------------- derive etc.

    /// Create a child capability in the same space with a SUBSET of the source's
    /// rights. Requires `duplicate` on the source. Rights cannot grow.
    pub fn derive(self: *Kernel, space: SpaceId, source: Handle, requested: Rights) OmniError!Handle {
        self.mutex.lock();
        defer self.mutex.unlock();
        const sp = try self.openSpace(space);
        if (!requested.isValid()) return error.InvalidArgument;
        const si = try self.resolveNode(sp, source);
        const src = self.nodes.ptrAt(si).?;
        if (!src.rights.duplicate) return error.AccessDenied;
        if (!src.rights.contains(requested)) return error.AccessDenied;
        const object_index = src.object;
        const otype = src.otype;

        // `src` must not be used after this point: reserveOne may reallocate.
        self.nodes.reserveOne(self.allocator) catch return error.QuotaExceeded;
        sp.table.reserveOne(self.allocator) catch return error.QuotaExceeded;
        try sp.account.reserve(.handles, 1);

        const nref = self.nodes.allocAssumeCapacity(.{
            .object = object_index,
            .otype = otype,
            .rights = requested,
            .space = space.index,
            .slot = 0,
            .parent = none_node,
            .first_child = none_node,
            .prev_sibling = none_node,
            .next_sibling = none_node,
        });
        const tref = sp.table.allocAssumeCapacity(nref);
        self.nodes.ptrAt(nref.index).?.slot = tref.index;
        self.pushChild(si, nref.index);
        self.objects.ptrAt(object_index).?.cap_refs += 1;
        return Handle.make(tref.index, tref.gen) catch @panic("OMNI Batch 3: generation 0 issued");
    }

    /// Release one capability. Its children are re-parented to its parent (or
    /// become roots), so ancestors keep the ability to revoke them. A second
    /// release of the same handle fails with `InvalidHandle`.
    pub fn release(self: *Kernel, space: SpaceId, h: Handle) OmniError!void {
        var dl = DestroyList{};
        const result: OmniError!void = blk: {
            self.mutex.lock();
            defer self.mutex.unlock();
            const sp = self.openSpace(space) catch |e| break :blk e;
            const ni = self.resolveNode(sp, h) catch |e| break :blk e;
            self.removeNode(ni, &dl);
            break :blk {};
        };
        self.finishDestroy(&dl);
        return result;
    }

    /// Revoke every DESCENDANT of the capability `h` (children, grandchildren, in
    /// any space). `h` itself survives. Requires `admin` on `h`. Returns the
    /// number of capabilities removed. Atomic: one critical section is the
    /// linearization point (docs/REVOCATION-MODEL.md).
    pub fn revoke(self: *Kernel, space: SpaceId, h: Handle) OmniError!usize {
        var dl = DestroyList{};
        const result: OmniError!usize = blk: {
            self.mutex.lock();
            defer self.mutex.unlock();
            const sp = self.openSpace(space) catch |e| break :blk e;
            const ni = self.resolveNode(sp, h) catch |e| break :blk e;
            if (!self.nodes.ptrAt(ni).?.rights.admin) break :blk error.AccessDenied;
            break :blk self.revokeBelow(ni, &dl);
        };
        self.finishDestroy(&dl);
        return result;
    }

    fn revokeBelow(self: *Kernel, root: u32, dl: *DestroyList) usize {
        var count: usize = 0;
        var cur = root;
        while (true) {
            const c = self.nodes.ptrAt(cur).?;
            if (c.first_child != none_node) {
                cur = c.first_child;
                continue;
            }
            if (cur == root) break;
            const parent = c.parent;
            self.removeNode(cur, dl);
            count += 1;
            cur = parent;
        }
        return count;
    }

    /// MOVE a capability to another space. Requires `transfer`. The source handle
    /// becomes invalid; the destination gets a new handle with the same rights;
    /// the node keeps its place in the derivation tree. Charges the destination
    /// account first (`QuotaExceeded` leaves everything unchanged). Source and
    /// destination must differ (`InvalidArgument`). The destination being closed
    /// is `PeerDead`.
    pub fn transfer(self: *Kernel, src_space: SpaceId, h: Handle, dst_space: SpaceId) OmniError!Handle {
        self.mutex.lock();
        defer self.mutex.unlock();
        const ssp = try self.openSpace(src_space);
        const dsp = try self.openSpace(dst_space);
        if (src_space.eql(dst_space)) return error.InvalidArgument;
        const ni = try self.resolveNode(ssp, h);
        const n = self.nodes.ptrAt(ni).?;
        if (!n.rights.transfer) return error.AccessDenied;

        dsp.table.reserveOne(self.allocator) catch return error.QuotaExceeded;
        try dsp.account.reserve(.handles, 1);

        // Infallible commit.
        const old = ssp.table.refOf(n.slot).?;
        mustFree(&ssp.table, old);
        const tref = dsp.table.allocAssumeCapacity(self.nodes.refOf(ni).?);
        n.space = dst_space.index;
        n.slot = tref.index;
        mustRelease(&ssp.account, .handles, 1);
        return Handle.make(tref.index, tref.gen) catch @panic("OMNI Batch 3: generation 0 issued");
    }

    // ------------------------------------------------------- in-flight operations

    /// Authorize an operation and pin its object so the object cannot be
    /// destroyed until `endOp`. Authority is checked ONCE, here (the operation's
    /// linearization point). Long operations may re-check with `opStillAuthorized`.
    pub fn beginOp(self: *Kernel, space: SpaceId, h: Handle, expected: ObjectType, required: Rights) OmniError!OpToken {
        self.mutex.lock();
        defer self.mutex.unlock();
        const sp = try self.openSpace(space);
        const ni = try self.authorize(sp, h, expected, required);
        const object_index = self.nodes.ptrAt(ni).?.object;
        self.pins.reserveOne(self.allocator) catch return error.QuotaExceeded;
        const pref = self.pins.allocAssumeCapacity(.{ .object = object_index });
        const o = self.objects.ptrAt(object_index).?;
        o.pins += 1;
        return .{ .pin = pref, .node = self.nodes.refOf(ni).?, .object = o.id };
    }

    /// True while the capability that authorized the operation still exists.
    /// Tied to the capability node, not to the handle value, so it stays true
    /// across a `transfer` and becomes false after release, revoke, or the
    /// holder's space closing, even if the slot has since been reused.
    pub fn opStillAuthorized(self: *Kernel, tok: OpToken) bool {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.pins.get(tok.pin) != null and self.nodes.get(tok.node) != null;
    }

    /// Drop the pin. A second `endOp` with the same token is `InvalidHandle`.
    /// If this was the last reference to the object, it is destroyed here.
    pub fn endOp(self: *Kernel, tok: OpToken) OmniError!void {
        var dl = DestroyList{};
        const result: OmniError!void = blk: {
            self.mutex.lock();
            defer self.mutex.unlock();
            const pin = self.pins.get(tok.pin) orelse break :blk error.InvalidHandle;
            const oi = pin.object;
            mustFree(&self.pins, tok.pin);
            self.objects.ptrAt(oi).?.pins -= 1;
            self.maybeDying(oi, &dl);
            break :blk {};
        };
        self.finishDestroy(&dl);
        return result;
    }

    // ---------------------------------------------------------------- internals

    /// Infallible. Detaches `ni`, re-parents its children to its parent, frees its
    /// table slot, refunds its handle charge and drops its object reference.
    /// Must be called with the mutex held. Allocates nothing, so pointers into
    /// the arenas stay valid inside this function.
    fn removeNode(self: *Kernel, ni: u32, dl: *DestroyList) void {
        const n = self.nodes.ptrAt(ni).?;
        const parent = n.parent;
        while (n.first_child != none_node) {
            const c = n.first_child;
            self.unlinkFromParent(c);
            self.pushChild(parent, c);
        }
        self.unlinkFromParent(ni);

        const sp = self.spaces.ptrAt(n.space).?.*;
        mustFree(&sp.table, sp.table.refOf(n.slot).?);
        mustRelease(&sp.account, .handles, 1);

        const oi = n.object;
        self.objects.ptrAt(oi).?.cap_refs -= 1;
        mustFree(&self.nodes, self.nodes.refOf(ni).?);
        self.maybeDying(oi, dl);
    }

    fn unlinkFromParent(self: *Kernel, ni: u32) void {
        const n = self.nodes.ptrAt(ni).?;
        if (n.prev_sibling != none_node) {
            self.nodes.ptrAt(n.prev_sibling).?.next_sibling = n.next_sibling;
        } else if (n.parent != none_node) {
            self.nodes.ptrAt(n.parent).?.first_child = n.next_sibling;
        }
        if (n.next_sibling != none_node) {
            self.nodes.ptrAt(n.next_sibling).?.prev_sibling = n.prev_sibling;
        }
        n.parent = none_node;
        n.prev_sibling = none_node;
        n.next_sibling = none_node;
    }

    /// Make `child` (currently unlinked) the first child of `parent`; a `none_node`
    /// parent makes it a root.
    fn pushChild(self: *Kernel, parent: u32, child: u32) void {
        const c = self.nodes.ptrAt(child).?;
        c.parent = parent;
        c.prev_sibling = none_node;
        if (parent == none_node) {
            c.next_sibling = none_node;
            return;
        }
        const p = self.nodes.ptrAt(parent).?;
        c.next_sibling = p.first_child;
        if (p.first_child != none_node) self.nodes.ptrAt(p.first_child).?.prev_sibling = child;
        p.first_child = child;
    }

    /// The live -> dying transition happens at most once per object because a
    /// reference can only be created from an existing reference (a live node or a
    /// pin), so nothing can resurrect an object whose counts are both zero.
    fn maybeDying(self: *Kernel, oi: u32, dl: *DestroyList) void {
        const o = self.objects.ptrAt(oi).?;
        if (o.state == .live and o.cap_refs == 0 and o.pins == 0) {
            o.state = .dying;
            o.next_dying = dl.head;
            dl.head = oi;
        }
    }

    /// Run owed destructors with the mutex NOT held, then finalize each object
    /// (refund its charge, free its registry slot) under the mutex. The object
    /// slot stays occupied (state `dying`) during the destructor, so its resources
    /// remain charged until it has finished.
    fn finishDestroy(self: *Kernel, dl: *DestroyList) void {
        var cur = dl.head;
        dl.head = ObjArena.none;
        while (cur != ObjArena.none) {
            self.mutex.lock();
            const o = self.objects.ptrAt(cur).?;
            const f = o.dtor;
            const ctx = o.ctx;
            const id = o.id;
            const next = o.next_dying;
            self.mutex.unlock();

            if (f) |func| func(ctx, id);

            self.mutex.lock();
            self.finalizeObject(cur);
            self.mutex.unlock();
            cur = next;
        }
    }

    fn finalizeObject(self: *Kernel, oi: u32) void {
        const o = self.objects.ptrAt(oi).?;
        std.debug.assert(o.state == .dying);
        const space_index = o.billed_space;
        const sp = self.spaces.ptrAt(space_index).?.*;
        if (o.charge_amount > 0) mustRelease(&sp.account, o.charge_res, o.charge_amount);
        sp.billed_objects -= 1;
        mustFree(&self.objects, self.objects.refOf(oi).?);
        if (!sp.open and sp.billed_objects == 0) self.freeSpace(space_index);
    }

    // --------------------------------------------------------------- inspection

    pub fn stats(self: *Kernel) Stats {
        self.mutex.lock();
        defer self.mutex.unlock();
        var dying: usize = 0;
        var i: usize = 0;
        while (i < self.objects.slotCount()) : (i += 1) {
            if (self.objects.ptrAt(i)) |o| {
                if (o.state == .dying) dying += 1;
            }
        }
        return .{
            .spaces = self.spaces.liveCount(),
            .objects = self.objects.liveCount(),
            .dying_objects = dying,
            .nodes = self.nodes.liveCount(),
            .pins = self.pins.liveCount(),
            .retired_nodes = self.nodes.retiredCount(),
            .retired_objects = self.objects.retiredCount(),
            .retired_pins = self.pins.retiredCount(),
            .retired_spaces = self.spaces.retiredCount(),
        };
    }

    /// Usage of one resource class in a space's account. Works on a zombie space.
    pub fn spaceUsed(self: *Kernel, space: SpaceId, r: Resource) OmniError!u64 {
        self.mutex.lock();
        defer self.mutex.unlock();
        const slot = self.spaces.get(space) orelse return error.InvalidHandle;
        return slot.*.account.used(r);
    }

    pub fn rootUsed(self: *Kernel, r: Resource) u64 {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.root_account.used(r);
    }

    /// Number of live capabilities held in a space.
    pub fn spaceHandleCount(self: *Kernel, space: SpaceId) OmniError!usize {
        self.mutex.lock();
        defer self.mutex.unlock();
        const slot = self.spaces.get(space) orelse return error.InvalidHandle;
        return slot.*.table.liveCount();
    }

    /// TEST HOOK: force the generation of a free handle-table slot.
    pub fn testSetTableGen(self: *Kernel, space: SpaceId, slot_index: usize, gen: u16) void {
        self.mutex.lock();
        defer self.mutex.unlock();
        self.spaces.get(space).?.*.table.testSetGen(slot_index, gen);
    }

    /// TEST HOOK: force the generation of a free node-arena slot.
    pub fn testSetNodeGen(self: *Kernel, slot_index: usize, gen: u32) void {
        self.mutex.lock();
        defer self.mutex.unlock();
        self.nodes.testSetGen(slot_index, gen);
    }

    /// Verify every structural and accounting invariant. Returns null when all
    /// hold, else a static description of the first violation. Takes the mutex.
    /// Allocates nothing. Intended for tests and debug builds: O(total size).
    pub fn checkInvariants(self: *Kernel) ?[]const u8 {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.checkLocked();
    }

    fn checkLocked(self: *Kernel) ?[]const u8 {
        if (!self.spaces.isConsistent()) return "space arena inconsistent";
        if (!self.objects.isConsistent()) return "object arena inconsistent";
        if (!self.nodes.isConsistent()) return "node arena inconsistent";
        if (!self.pins.isConsistent()) return "pin arena inconsistent";

        // Reset scratch.
        var i: usize = 0;
        while (i < self.objects.slotCount()) : (i += 1) {
            if (self.objects.ptrAt(i)) |o| {
                o.chk_refs = 0;
                o.chk_pins = 0;
            }
        }
        i = 0;
        while (i < self.spaces.slotCount()) : (i += 1) {
            if (self.spaces.ptrAt(i)) |sp| {
                if (!sp.*.table.isConsistent()) return "handle table inconsistent";
                sp.*.chk_nodes = 0;
                sp.*.chk_billed = 0;
                sp.*.chk_usage = .{};
            }
        }

        // Nodes.
        i = 0;
        while (i < self.nodes.slotCount()) : (i += 1) {
            const n = self.nodes.ptrAt(i) orelse continue;
            const idx: u32 = @intCast(i);
            const o = self.objects.ptrAt(n.object) orelse return "node refers to a free object slot";
            if (o.state != .live) return "node refers to a dying object";
            if (o.otype != n.otype) return "node type differs from object type";
            if (!n.rights.isValid()) return "node has reserved rights bits";
            const spp = self.spaces.ptrAt(n.space) orelse return "node refers to a free space slot";
            const sp = spp.*;
            if (!sp.open) return "node held by a closed space";
            const tv = sp.table.ptrAt(n.slot) orelse return "node's handle slot is not live";
            const own = self.nodes.refOf(i).?;
            if (tv.index != own.index or tv.gen != own.gen) return "handle slot does not point back to node";
            if (n.parent != none_node) {
                const p = self.nodes.ptrAt(n.parent) orelse return "parent is not a live node";
                if (p.object != n.object) return "child and parent refer to different objects";
                if (!p.rights.contains(n.rights)) return "child rights exceed parent rights";
                if (n.prev_sibling == none_node and p.first_child != idx) return "first child not linked from parent";
            }
            if (n.prev_sibling != none_node) {
                const ps = self.nodes.ptrAt(n.prev_sibling) orelse return "prev sibling not live";
                if (ps.next_sibling != idx or ps.parent != n.parent) return "sibling links asymmetric (prev)";
            }
            if (n.next_sibling != none_node) {
                const ns = self.nodes.ptrAt(n.next_sibling) orelse return "next sibling not live";
                if (ns.prev_sibling != idx or ns.parent != n.parent) return "sibling links asymmetric (next)";
            }
            if (n.first_child != none_node) {
                const c = self.nodes.ptrAt(n.first_child) orelse return "first child not live";
                if (c.parent != idx or c.prev_sibling != none_node) return "first child does not point back";
            }
            // No cycle: a parent chain is no longer than the node count.
            var steps: usize = 0;
            var up = n.parent;
            while (up != none_node) {
                steps += 1;
                if (steps > self.nodes.liveCount()) return "cycle in derivation tree";
                up = self.nodes.ptrAt(up).?.parent;
            }
            o.chk_refs += 1;
            sp.chk_nodes += 1;
        }

        // Pins.
        i = 0;
        while (i < self.pins.slotCount()) : (i += 1) {
            const p = self.pins.ptrAt(i) orelse continue;
            const o = self.objects.ptrAt(p.object) orelse return "pin refers to a free object slot";
            if (o.state != .live) return "pin refers to a dying object";
            o.chk_pins += 1;
        }

        // Objects.
        i = 0;
        while (i < self.objects.slotCount()) : (i += 1) {
            const o = self.objects.ptrAt(i) orelse continue;
            if (!o.id.isValid() or o.id.raw() >= self.ids.next_raw) return "object id out of range";
            const spp = self.spaces.ptrAt(o.billed_space) orelse return "object billed to a free space slot";
            const sp = spp.*;
            sp.chk_billed += 1;
            if (o.charge_amount > 0) counterPtr(&sp.chk_usage, o.charge_res).* += o.charge_amount;
            switch (o.state) {
                .live => {
                    if (o.cap_refs != o.chk_refs) return "object cap_refs differs from node count";
                    if (o.pins != o.chk_pins) return "object pins differs from pin count";
                    if (o.cap_refs == 0 and o.pins == 0) return "live object with no references";
                },
                .dying => {
                    if (o.chk_refs != 0 or o.chk_pins != 0) return "dying object still referenced";
                },
            }
        }

        // Spaces and accounts.
        var sum: Counters = .{};
        i = 0;
        while (i < self.spaces.slotCount()) : (i += 1) {
            const spp = self.spaces.ptrAt(i) orelse continue;
            const sp = spp.*;
            if (sp.table.liveCount() != sp.chk_nodes) return "table live count differs from node count";
            if (sp.billed_objects != sp.chk_billed) return "billed object count mismatch";
            if (!sp.open and sp.billed_objects == 0) return "closed space with nothing billed was not freed";
            if (!sp.open and sp.table.liveCount() != 0) return "closed space still holds capabilities";
            inline for (std.meta.tags(Resource)) |r| {
                var expected = counterPtr(&sp.chk_usage, r).*;
                if (r == .handles) expected += sp.chk_nodes;
                if (sp.account.used(r) != expected) return "account usage differs from recomputed usage";
                counterPtr(&sum, r).* += sp.account.used(r);
            }
        }
        inline for (std.meta.tags(Resource)) |r| {
            if (self.root_account.used(r) != counterPtr(&sum, r).*) return "root usage differs from sum of spaces";
        }
        return null;
    }
};
