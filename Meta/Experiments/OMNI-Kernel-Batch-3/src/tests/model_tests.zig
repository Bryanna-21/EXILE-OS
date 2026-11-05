//! Randomized tests against an independent reference model (RND-xx).
//!
//! The model is written separately from the kernel and uses plain arrays and
//! linear scans. Every step compares: the result (success or the exact error),
//! every live handle, a sample of dead handles, resource accounting, object
//! counts, destructor counts, and the kernel's own invariant checker.

const std = @import("std");
const c = @import("common.zig");
const expect = std.testing.expect;
const expectEqual = std.testing.expectEqual;

const Node = struct {
    alive: bool = true,
    parent: ?usize,
    space: usize,
    handle: c.Handle,
    rights: u32,
    obj: usize,
};

const Obj = struct {
    id: u64,
    otype: c.ObjectType,
    destroyed: bool = false,
    billed: usize,
    charge: u64,
    refs: u32 = 0,
    pins: u32 = 0,
};

const Pin = struct { tok: c.kernel.OpToken, node: usize, obj: usize, active: bool = true };
const Dead = struct { space: usize, handle: c.Handle };

const types = [_]c.ObjectType{ .channel, .timer, .memory_object };

const Sim = struct {
    gpa: std.mem.Allocator,
    k: *c.Kernel,
    t: *c.Tracker,
    rnd: std.Random,
    sids: std.ArrayListUnmanaged(c.SpaceId) = .empty,
    sopen: std.ArrayListUnmanaged(bool) = .empty,
    nodes: std.ArrayListUnmanaged(Node) = .empty,
    objs: std.ArrayListUnmanaged(Obj) = .empty,
    pins: std.ArrayListUnmanaged(Pin) = .empty,
    dead: std.ArrayListUnmanaged(Dead) = .empty,

    fn deinit(s: *Sim) void {
        s.sids.deinit(s.gpa);
        s.sopen.deinit(s.gpa);
        s.nodes.deinit(s.gpa);
        s.objs.deinit(s.gpa);
        s.pins.deinit(s.gpa);
        s.dead.deinit(s.gpa);
    }

    fn addSpace(s: *Sim) !void {
        try s.sids.append(s.gpa, try s.k.createSpace(c.space_budget));
        try s.sopen.append(s.gpa, true);
    }

    fn pickAlive(s: *Sim) ?usize {
        var count: usize = 0;
        for (s.nodes.items) |n| count += @intFromBool(n.alive);
        if (count == 0) return null;
        var want = s.rnd.uintLessThan(usize, count);
        for (s.nodes.items, 0..) |n, i| {
            if (!n.alive) continue;
            if (want == 0) return i;
            want -= 1;
        }
        unreachable;
    }

    fn pickOpenSpace(s: *Sim) usize {
        while (true) {
            const i = s.rnd.uintLessThan(usize, s.sids.items.len);
            if (s.sopen.items[i]) return i;
        }
    }

    fn billedLive(s: *Sim, space: usize) usize {
        var n: usize = 0;
        for (s.objs.items) |o| n += @intFromBool(!o.destroyed and o.billed == space);
        return n;
    }

    /// Model of `release`: remove one node, re-parent its children, drop its reference.
    fn modelRemove(s: *Sim, n: usize) !void {
        const node = &s.nodes.items[n];
        node.alive = false;
        for (s.nodes.items) |*m| {
            if (m.alive and m.parent == n) m.parent = node.parent;
        }
        try s.dead.append(s.gpa, .{ .space = node.space, .handle = node.handle });
        const o = &s.objs.items[node.obj];
        o.refs -= 1;
        if (o.refs == 0 and o.pins == 0) o.destroyed = true;
    }

    fn isDescendant(s: *Sim, m: usize, of: usize) bool {
        var up = s.nodes.items[m].parent;
        while (up) |p| : (up = s.nodes.items[p].parent) {
            if (p == of) return true;
        }
        return false;
    }

    fn randRights(s: *Sim) u32 {
        const base = s.rnd.uintLessThan(u32, 512);
        return if (s.rnd.uintLessThan(u8, 10) < 6) base | c.R_DUP | c.R_XFER | c.R_ADMIN | c.R_READ else base;
    }

    // ------------------------------------------------------------------ steps

    fn stepCreate(s: *Sim) !void {
        const sp = s.pickOpenSpace();
        const typ = types[s.rnd.uintLessThan(usize, types.len)];
        const bits = s.randRights();
        const charge = s.rnd.uintLessThan(u64, 5);
        const h = try s.k.createObject(s.sids.items[sp], typ, c.rights(bits), s.t.optsCharged(charge));
        const r = try s.k.lookup(s.sids.items[sp], h, typ, c.Rights.none);
        try s.objs.append(s.gpa, .{ .id = r.object.raw(), .otype = typ, .billed = sp, .charge = charge, .refs = 1 });
        try s.nodes.append(s.gpa, .{ .parent = null, .space = sp, .handle = h, .rights = bits, .obj = s.objs.items.len - 1 });
    }

    fn stepDerive(s: *Sim) !void {
        const n = s.pickAlive() orelse return;
        const src = s.nodes.items[n];
        const req: u32 = if (s.rnd.uintLessThan(u8, 10) < 7) (src.rights & s.rnd.uintLessThan(u32, 512)) else s.rnd.uintLessThan(u32, 512);
        const sid = s.sids.items[src.space];
        const result = s.k.derive(sid, src.handle, c.rights(req));
        if ((src.rights & c.R_DUP) == 0 or (req & ~src.rights) != 0) {
            try std.testing.expectError(error.AccessDenied, result);
            return;
        }
        const h = try result;
        s.objs.items[src.obj].refs += 1;
        try s.nodes.append(s.gpa, .{ .parent = n, .space = src.space, .handle = h, .rights = req, .obj = src.obj });
    }

    fn stepRelease(s: *Sim) !void {
        const n = s.pickAlive() orelse return;
        const node = s.nodes.items[n];
        try s.k.release(s.sids.items[node.space], node.handle);
        try s.modelRemove(n);
    }

    fn stepRevoke(s: *Sim) !void {
        const n = s.pickAlive() orelse return;
        const node = s.nodes.items[n];
        const result = s.k.revoke(s.sids.items[node.space], node.handle);
        if ((node.rights & c.R_ADMIN) == 0) {
            try std.testing.expectError(error.AccessDenied, result);
            return;
        }
        const got = try result;
        var expected: usize = 0;
        // Mark first, then remove: descendants are decided against the pre-revoke tree.
        var doomed = try s.gpa.alloc(bool, s.nodes.items.len);
        defer s.gpa.free(doomed);
        for (s.nodes.items, 0..) |m, i| doomed[i] = m.alive and s.isDescendant(i, n);
        for (doomed, 0..) |d, i| {
            if (!d) continue;
            const m = &s.nodes.items[i];
            m.alive = false;
            try s.dead.append(s.gpa, .{ .space = m.space, .handle = m.handle });
            s.objs.items[m.obj].refs -= 1;
            expected += 1;
        }
        try expectEqual(expected, got);
        for (s.objs.items) |*o| {
            if (!o.destroyed and o.refs == 0 and o.pins == 0) o.destroyed = true;
        }
    }

    fn stepTransfer(s: *Sim) !void {
        const n = s.pickAlive() orelse return;
        const node = s.nodes.items[n];
        const dst = s.pickOpenSpace();
        const result = s.k.transfer(s.sids.items[node.space], node.handle, s.sids.items[dst]);
        if (dst == node.space) {
            try std.testing.expectError(error.InvalidArgument, result);
            return;
        }
        if ((node.rights & c.R_XFER) == 0) {
            try std.testing.expectError(error.AccessDenied, result);
            return;
        }
        const nh = try result;
        try s.dead.append(s.gpa, .{ .space = node.space, .handle = node.handle });
        s.nodes.items[n].space = dst;
        s.nodes.items[n].handle = nh;
    }

    fn stepBeginOp(s: *Sim) !void {
        const n = s.pickAlive() orelse return;
        const node = s.nodes.items[n];
        const o = &s.objs.items[node.obj];
        const typ = if (s.rnd.uintLessThan(u8, 10) < 8) o.otype else types[s.rnd.uintLessThan(usize, types.len)];
        const req: u32 = if (s.rnd.boolean()) (node.rights & s.rnd.uintLessThan(u32, 512)) else s.rnd.uintLessThan(u32, 512);
        const result = s.k.beginOp(s.sids.items[node.space], node.handle, typ, c.rights(req));
        if (typ != o.otype) {
            try std.testing.expectError(error.InvalidHandle, result);
            return;
        }
        if ((req & ~node.rights) != 0) {
            try std.testing.expectError(error.AccessDenied, result);
            return;
        }
        const tok = try result;
        o.pins += 1;
        try s.pins.append(s.gpa, .{ .tok = tok, .node = n, .obj = node.obj });
    }

    fn stepEndOp(s: *Sim) !void {
        if (s.pins.items.len == 0) return;
        const i = s.rnd.uintLessThan(usize, s.pins.items.len);
        const p = &s.pins.items[i];
        if (!p.active) {
            try std.testing.expectError(error.InvalidHandle, s.k.endOp(p.tok));
            return;
        }
        try s.k.endOp(p.tok);
        p.active = false;
        const o = &s.objs.items[p.obj];
        o.pins -= 1;
        if (o.refs == 0 and o.pins == 0) o.destroyed = true;
    }

    fn stepClose(s: *Sim) !void {
        var open: usize = 0;
        for (s.sopen.items) |o| open += @intFromBool(o);
        if (open <= 2) return;
        const sp = s.pickOpenSpace();
        try s.k.closeSpace(s.sids.items[sp]);
        for (s.nodes.items, 0..) |n, i| {
            if (n.alive and n.space == sp) try s.modelRemove(i);
        }
        s.sopen.items[sp] = false;
        // Using a closed space fails: PeerDead while it is a zombie, InvalidHandle once freed.
        const err = if (s.billedLive(sp) > 0) error.PeerDead else error.InvalidHandle;
        try std.testing.expectError(err, s.k.createObject(s.sids.items[sp], .timer, c.Rights.none, .{}));
        try s.addSpace();
    }

    // ----------------------------------------------------------------- checks

    fn verify(s: *Sim, full_dead_scan: bool) !void {
        try c.expectConsistent(s.k);
        var alive: usize = 0;
        for (s.nodes.items) |n| {
            if (!n.alive) continue;
            alive += 1;
            const o = s.objs.items[n.obj];
            const r = try s.k.lookup(s.sids.items[n.space], n.handle, o.otype, c.Rights.none);
            try expectEqual(n.rights, r.rights.toRaw());
            try expectEqual(o.id, r.object.raw());
        }
        // Dead handles never resolve.
        const total = s.dead.items.len;
        const scan = if (full_dead_scan) total else @min(total, 40);
        var j: usize = 0;
        while (j < scan) : (j += 1) {
            const d = if (full_dead_scan) s.dead.items[j] else if (j < 10) s.dead.items[total - 1 - @min(j, total - 1)] else s.dead.items[s.rnd.uintLessThan(usize, total)];
            const sid = s.sids.items[d.space];
            const expect_err: anyerror = if (s.sopen.items[d.space]) error.InvalidHandle else if (s.billedLive(d.space) > 0) error.PeerDead else error.InvalidHandle;
            try std.testing.expectError(expect_err, s.k.lookup(sid, d.handle, .channel, c.Rights.none));
        }
        // Accounting and counts.
        for (s.sids.items, 0..) |sid, sp| {
            if (!s.sopen.items[sp] and s.billedLive(sp) == 0) {
                try std.testing.expectError(error.InvalidHandle, s.k.spaceUsed(sid, .handles));
                continue;
            }
            var nodes_here: u64 = 0;
            for (s.nodes.items) |n| nodes_here += @intFromBool(n.alive and n.space == sp);
            var pages: u64 = 0;
            for (s.objs.items) |o| {
                if (!o.destroyed and o.billed == sp) pages += o.charge;
            }
            try expectEqual(nodes_here, try s.k.spaceUsed(sid, .handles));
            try expectEqual(pages, try s.k.spaceUsed(sid, .pages));
        }
        var live_objs: usize = 0;
        var destroyed: u32 = 0;
        for (s.objs.items) |o| {
            live_objs += @intFromBool(!o.destroyed);
            destroyed += @intFromBool(o.destroyed);
        }
        var active_pins: usize = 0;
        for (s.pins.items) |p| {
            active_pins += @intFromBool(p.active);
            const want = p.active and s.nodes.items[p.node].alive;
            try expectEqual(want, s.k.opStillAuthorized(p.tok));
        }
        const st = s.k.stats();
        try expectEqual(alive, st.nodes);
        try expectEqual(live_objs, st.objects);
        try expectEqual(active_pins, st.pins);
        try expectEqual(destroyed, s.t.total);
        if (full_dead_scan) {
            for (s.objs.items) |o| try expectEqual(@as(u8, @intFromBool(o.destroyed)), s.t.counts[@intCast(o.id)]);
        }
    }
};

fn run(seed: u64, steps: usize) !void {
    var t = c.Tracker{};
    const k = try c.newKernel(std.testing.allocator);
    defer k.destroy();
    t.kernel_ptr = k;
    var prng = std.Random.DefaultPrng.init(seed);
    var s = Sim{ .gpa = std.testing.allocator, .k = k, .t = &t, .rnd = prng.random() };
    defer s.deinit();
    for (0..4) |_| try s.addSpace();

    var i: usize = 0;
    while (i < steps) : (i += 1) {
        const roll = s.rnd.uintLessThan(u8, 100);
        if (roll < 14) {
            try s.stepCreate();
        } else if (roll < 36) {
            try s.stepDerive();
        } else if (roll < 54) {
            try s.stepRelease();
        } else if (roll < 63) {
            try s.stepRevoke();
        } else if (roll < 77) {
            try s.stepTransfer();
        } else if (roll < 86) {
            try s.stepBeginOp();
        } else if (roll < 97) {
            try s.stepEndOp();
        } else {
            try s.stepClose();
        }
        try s.verify(i % 50 == 0);
    }
    try s.verify(true);
    try expect(!t.saw_lock_held);
    try expect(s.nodes.items.len > steps / 10); // the run really did something
}

test "RND-01 reference-model run, seed 1" {
    try run(1, 2500);
}

test "RND-02 reference-model run, seed 2" {
    try run(0xC0FFEE, 2500);
}

test "RND-03 reference-model run, seed 3" {
    try run(0xDEADBEEF, 2500);
}
