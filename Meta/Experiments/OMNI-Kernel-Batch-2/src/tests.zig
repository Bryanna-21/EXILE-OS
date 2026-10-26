//! Hosted, deterministic tests for the Batch 2 core abstractions.
//!
//! Every test name starts with a requirement ID (ERR-, ID-, CAP-, RES-, MEM-,
//! IPC-, ARCH-, X-) that docs/TEST-PLAN.md maps back to the requirement it
//! covers. All randomness uses fixed seeds, so runs are repeatable.
//!
//! These tests run in an ordinary process. They check logic and rejection rules.
//! They say nothing about hardware isolation, privilege, or real kernel behaviour.

const std = @import("std");
const testing = std.testing;

const err_mod = @import("core/error.zig");
const ids = @import("core/ids.zig");
const caps = @import("core/capabilities.zig");
const res = @import("core/resources.zig");
const mem = @import("core/memory.zig");
const ipc = @import("core/ipc.zig");
const arch = @import("arch/interface.zig");

const OmniError = err_mod.OmniError;
const Status = err_mod.Status;
const Handle = ids.Handle;
const ObjectId = ids.ObjectId;
const Rights = caps.Rights;
const ObjectType = caps.ObjectType;

const Table4 = caps.CapabilityTable(4);
const Table16 = caps.CapabilityTable(16);

fn oid(n: u64) ObjectId {
    return ObjectId.fromRaw(n);
}

fn rw() Rights {
    return .{ .read = true, .write = true };
}

// ---------------------------------------------------------------- errors ----

test "ERR-01 every error has a unique, negative provisional status" {
    const errors = @typeInfo(OmniError).error_set.?;
    var seen = std.AutoHashMap(i32, void).init(testing.allocator);
    defer seen.deinit();
    inline for (errors) |e| {
        const status = err_mod.toStatus(@field(OmniError, e.name));
        const code = @intFromEnum(status);
        try testing.expect(code < 0);
        const gop = try seen.getOrPut(code);
        try testing.expect(!gop.found_existing);
    }
    try testing.expectEqual(@as(usize, errors.len), seen.count());
}

test "ERR-02 toStatus and fromStatus round-trip for every error" {
    inline for (@typeInfo(OmniError).error_set.?) |e| {
        const original = @field(OmniError, e.name);
        try testing.expectError(original, err_mod.fromStatus(err_mod.toStatus(original)));
    }
}

test "ERR-03 ok status is success" {
    try err_mod.fromStatus(.ok);
    try testing.expectEqual(@as(i32, 0), @intFromEnum(Status.ok));
}

test "ERR-04 unknown status values are rejected, never treated as success" {
    const bogus = [_]i32{ 1, 2, -18, -999, std.math.maxInt(i32), std.math.minInt(i32) };
    for (bogus) |v| {
        const s: Status = @enumFromInt(v);
        try testing.expectError(error.InvalidArgument, err_mod.fromStatus(s));
    }
}

// ------------------------------------------------------------------- ids ----

test "ID-01 identifier and handle types are distinct" {
    try testing.expect(comptime ids.ObjectId != ids.ProcessId);
    try testing.expect(comptime ids.ObjectId != ids.Handle);
    try testing.expect(comptime ids.ProcessId != ids.Handle);
}

test "ID-02 the zero value of each type is invalid" {
    try testing.expect(!ObjectId.fromRaw(0).isValid());
    try testing.expect(!ids.ProcessId.fromRaw(0).isValid());
    try testing.expectEqual(Handle.invalid, Handle.fromRaw(0));
    try testing.expect(ObjectId.fromRaw(1).isValid());
}

test "ID-03 handle packs index and generation; generation 0 is rejected" {
    const h = try Handle.make(0x1234, 0xBEEF);
    try testing.expectEqual(@as(u16, 0x1234), h.index());
    try testing.expectEqual(@as(u16, 0xBEEF), h.generation());
    try testing.expectEqual(@as(u32, 0xBEEF1234), h.raw());
    try testing.expectError(error.InvalidArgument, Handle.make(7, 0));
    // A live handle is never 0, even at index 0.
    try testing.expect((try Handle.make(0, 1)) != Handle.invalid);
}

test "ID-04 object id allocator never yields the invalid id" {
    var a = ids.ObjectIdAllocator{};
    const first = try a.next();
    const second = try a.next();
    try testing.expect(first.isValid() and second.isValid());
    try testing.expect(first != second);
}

test "ID-05 object id allocator reports overflow and keeps its state" {
    var a = ids.ObjectIdAllocator{ .next_raw = std.math.maxInt(u64) };
    try testing.expectError(error.Overflow, a.next());
    try testing.expectEqual(@as(u64, std.math.maxInt(u64)), a.next_raw);
}

// ----------------------------------------------------------- capabilities ----

test "CAP-01 a valid handle with the required right succeeds" {
    var t = Table4.init(null);
    const h = try t.insert(oid(10), .channel, rw());
    const r = try t.lookup(h, .channel, .{ .read = true });
    try testing.expectEqual(oid(10), r.object);
    try testing.expectEqual(ObjectType.channel, r.otype);
    try testing.expect(r.rights.write);
    // Requiring nothing also succeeds for a live handle of the right type.
    _ = try t.lookup(h, .channel, .none);
}

test "CAP-02 a missing handle is rejected" {
    var t = Table4.init(null);
    const h = try t.insert(oid(1), .channel, rw());
    try testing.expectError(error.InvalidHandle, t.lookup(.invalid, .channel, .none));
    // Index beyond capacity.
    try testing.expectError(error.InvalidHandle, t.lookup(try Handle.make(4, 1), .channel, .none));
    try testing.expectError(error.InvalidHandle, t.lookup(try Handle.make(0xFFFF, 1), .channel, .none));
    // In range but never issued (free slot).
    try testing.expectError(error.InvalidHandle, t.lookup(try Handle.make(3, 1), .channel, .none));
    // Arbitrary raw garbage.
    try testing.expectError(error.InvalidHandle, t.lookup(Handle.fromRaw(0xFFFFFFFF), .channel, .none));
    _ = h;
}

test "CAP-03 a stale handle is rejected, including after its slot is reused" {
    var t = Table4.init(null);
    const old = try t.insert(oid(1), .channel, rw());
    try t.release(old);
    try testing.expectError(error.InvalidHandle, t.lookup(old, .channel, .none));

    // The slot is reused (lowest free slot) with a new generation.
    const fresh = try t.insert(oid(2), .channel, rw());
    try testing.expectEqual(old.index(), fresh.index());
    try testing.expect(old.generation() != fresh.generation());
    try testing.expect(old != fresh);
    try testing.expectError(error.InvalidHandle, t.lookup(old, .channel, .none));
    _ = try t.lookup(fresh, .channel, .none);
}

test "CAP-04 a handle lacking the required right is rejected" {
    var t = Table4.init(null);
    const h = try t.insert(oid(1), .channel, .{ .read = true });
    try testing.expectError(error.AccessDenied, t.lookup(h, .channel, .{ .write = true }));
    try testing.expectError(error.AccessDenied, t.lookup(h, .channel, .{ .read = true, .write = true }));
    _ = try t.lookup(h, .channel, .{ .read = true });
}

test "CAP-05 an object-type mismatch is rejected as an invalid handle" {
    var t = Table4.init(null);
    const h = try t.insert(oid(1), .channel, rw());
    try testing.expectError(error.InvalidHandle, t.lookup(h, .memory_object, .none));
}

test "CAP-06 unknown or reserved rights bits are rejected everywhere" {
    // Rights.fromRaw
    try testing.expectError(error.InvalidArgument, Rights.fromRaw(1 << 9));
    try testing.expectError(error.InvalidArgument, Rights.fromRaw(1 << 31));
    try testing.expectError(error.InvalidArgument, Rights.fromRaw(0xFFFFFFFF));
    _ = try Rights.fromRaw(Rights.valid_mask);
    _ = try Rights.fromRaw(0);
    var bit: u5 = 9;
    while (true) : (bit += 1) {
        try testing.expectError(error.InvalidArgument, Rights.fromRaw(@as(u32, 1) << bit));
        if (bit == 31) break;
    }

    var t = Table4.init(null);
    const h = try t.insert(oid(1), .channel, .{ .read = true, .duplicate = true });

    // lookupRaw
    try testing.expectError(error.InvalidArgument, t.lookupRaw(h, .channel, 1 << 9));
    // deriveRaw
    try testing.expectError(error.InvalidArgument, t.deriveRaw(h, 1 << 20));
    // A Rights value forged with @bitCast still cannot be inserted.
    const forged: Rights = @bitCast(@as(u32, 1 << 12));
    try testing.expect(!forged.isValid());
    try testing.expectError(error.InvalidArgument, t.insert(oid(2), .channel, forged));
    try testing.expectError(error.InvalidArgument, t.lookup(h, .channel, forged));
    try testing.expectEqual(@as(u32, 1), t.liveCount());
}

test "CAP-07 derivation cannot increase rights" {
    var t = Table4.init(null);
    const src = try t.insert(oid(1), .channel, .{ .read = true, .duplicate = true });

    // Asking for a right the source lacks is denied.
    try testing.expectError(error.AccessDenied, t.derive(src, .{ .read = true, .write = true }));
    try testing.expectError(error.AccessDenied, t.derive(src, .{ .admin = true }));
    try testing.expectError(error.AccessDenied, t.deriveRaw(src, Rights.valid_mask));

    // A source without DUPLICATE cannot be derived from at all.
    const nodup = try t.insert(oid(2), .channel, .{ .read = true });
    try testing.expectError(error.AccessDenied, t.derive(nodup, .{ .read = true }));
    try testing.expectError(error.AccessDenied, t.derive(nodup, .none));

    // The permitted derivation is a subset.
    const child = try t.derive(src, .{ .read = true });
    const r = try t.lookup(child, .channel, .{ .read = true });
    try testing.expect(!r.rights.duplicate);
    try testing.expect(!r.rights.write);
    // The child lacks DUPLICATE, so it cannot regain anything.
    try testing.expectError(error.AccessDenied, t.derive(child, .{ .read = true }));
}

test "CAP-08 derivation keeps the same object and type, and is independent of the source" {
    var t = Table4.init(null);
    const src = try t.insert(oid(77), .memory_object, .{ .read = true, .write = true, .duplicate = true });
    const child = try t.derive(src, .{ .read = true });
    const r = try t.lookup(child, .memory_object, .{ .read = true });
    try testing.expectEqual(oid(77), r.object);
    try testing.expect(child != src);
    // Releasing the source does not (in this model) revoke the child. This is
    // documented: revocation is NOT implemented.
    try t.release(src);
    _ = try t.lookup(child, .memory_object, .{ .read = true });
}

test "CAP-09 release invalidates the handle, and a second release fails" {
    var t = Table4.init(null);
    const h = try t.insert(oid(1), .channel, rw());
    try t.release(h);
    try testing.expectError(error.InvalidHandle, t.lookup(h, .channel, .none));
    try testing.expectError(error.InvalidHandle, t.release(h));
    try testing.expectEqual(@as(u32, 0), t.liveCount());
}

test "CAP-10 failed operations leave the table and account unchanged" {
    var acct = try res.Account.create(.{ .handles = 3 }, null);
    var t = Table4.init(&acct);
    const a = try t.insert(oid(1), .channel, .{ .read = true, .duplicate = true });
    const b = try t.insert(oid(2), .channel, .{ .read = true });
    try t.release(b);

    const table_before = t;
    const acct_before = acct;

    try testing.expectError(error.InvalidHandle, t.lookup(.invalid, .channel, .none));
    try testing.expectError(error.AccessDenied, t.lookup(a, .channel, .{ .admin = true }));
    try testing.expectError(error.InvalidArgument, t.lookupRaw(a, .channel, 1 << 30));
    try testing.expectError(error.InvalidHandle, t.release(b)); // stale
    try testing.expectError(error.InvalidHandle, t.release(.invalid));
    try testing.expectError(error.AccessDenied, t.derive(a, .{ .write = true }));
    try testing.expectError(error.InvalidArgument, t.deriveRaw(a, 1 << 15));
    try testing.expectError(error.InvalidArgument, t.insert(.invalid, .channel, .none));

    try testing.expectEqualDeep(table_before, t);
    try testing.expectEqualDeep(acct_before, acct);
}

test "CAP-11 a full table fails with QuotaExceeded and stays unchanged" {
    var t = Table4.init(null);
    var hs: [4]Handle = undefined;
    for (&hs, 0..) |*h, i| h.* = try t.insert(oid(i + 1), .channel, .{ .read = true, .duplicate = true });
    const before = t;
    try testing.expectError(error.QuotaExceeded, t.insert(oid(99), .channel, .none));
    try testing.expectError(error.QuotaExceeded, t.derive(hs[0], .{ .read = true }));
    try testing.expectEqualDeep(before, t);
    // Freeing one slot makes room again.
    try t.release(hs[2]);
    _ = try t.insert(oid(100), .channel, .none);
}

test "CAP-12 a slot whose generation would wrap is retired, not reused" {
    var t = Table4.init(null);
    t.testSetGeneration(0, std.math.maxInt(u16));
    const h = try t.insert(oid(1), .channel, rw());
    try testing.expectEqual(@as(u16, 0), h.index());
    try testing.expectEqual(@as(u16, std.math.maxInt(u16)), h.generation());
    try t.release(h);
    try testing.expectEqual(@as(u32, 1), t.retiredCount());
    try testing.expectError(error.InvalidHandle, t.lookup(h, .channel, .none));
    // The next insert must use a different slot.
    const h2 = try t.insert(oid(2), .channel, rw());
    try testing.expect(h2.index() != 0);
    try testing.expectError(error.InvalidHandle, t.lookup(h, .channel, .none));
}

test "CAP-13 handle quota is charged on insert and returned on release" {
    var acct = try res.Account.create(.{ .handles = 2 }, null);
    var t = Table4.init(&acct);
    const a = try t.insert(oid(1), .channel, .none);
    const b = try t.insert(oid(2), .channel, .none);
    try testing.expectEqual(@as(u64, 2), acct.used(.handles));

    const table_before = t;
    try testing.expectError(error.QuotaExceeded, t.insert(oid(3), .channel, .none));
    try testing.expectEqualDeep(table_before, t);
    try testing.expectEqual(@as(u64, 2), acct.used(.handles));

    try t.release(a);
    try testing.expectEqual(@as(u64, 1), acct.used(.handles));
    _ = try t.insert(oid(3), .channel, .none);
    try testing.expectEqual(@as(u64, 2), acct.used(.handles));
    try t.release(b);
}

test "CAP-14 derive is charged like insert and fails cleanly at the quota" {
    var acct = try res.Account.create(.{ .handles = 1 }, null);
    var t = Table4.init(&acct);
    const a = try t.insert(oid(1), .channel, .{ .read = true, .duplicate = true });
    const table_before = t;
    try testing.expectError(error.QuotaExceeded, t.derive(a, .{ .read = true }));
    try testing.expectEqualDeep(table_before, t);
    try testing.expectEqual(@as(u64, 1), acct.used(.handles));
}

test "CAP-15 deterministic model check: 6000 random operations agree with a reference model" {
    const Live = struct { h: Handle, obj: ObjectId, ot: ObjectType, rights: Rights };
    var prng = std.Random.DefaultPrng.init(0x4F4D4E49);
    const rnd = prng.random();

    var table = Table16.init(null);
    var live: [16]Live = undefined;
    var live_n: usize = 0;
    var dead: [64]Handle = undefined;
    var dead_n: usize = 0;
    var dead_next: usize = 0;
    var next_obj: u64 = 1;

    const all_types = std.meta.tags(ObjectType);

    var step: usize = 0;
    while (step < 6000) : (step += 1) {
        switch (rnd.uintLessThan(u8, 4)) {
            0 => { // insert
                const rights = try Rights.fromRaw(rnd.int(u32) & Rights.valid_mask);
                const ot = all_types[rnd.uintLessThan(usize, all_types.len)];
                const obj = oid(next_obj);
                if (live_n == live.len) {
                    try testing.expectError(error.QuotaExceeded, table.insert(obj, ot, rights));
                } else {
                    const h = try table.insert(obj, ot, rights);
                    next_obj += 1;
                    for (live[0..live_n]) |l| try testing.expect(l.h != h);
                    live[live_n] = .{ .h = h, .obj = obj, .ot = ot, .rights = rights };
                    live_n += 1;
                }
            },
            1 => { // release
                if (live_n == 0) continue;
                const i = rnd.uintLessThan(usize, live_n);
                try table.release(live[i].h);
                dead[dead_next] = live[i].h;
                dead_next = (dead_next + 1) % dead.len;
                if (dead_n < dead.len) dead_n += 1;
                live[i] = live[live_n - 1];
                live_n -= 1;
            },
            2 => { // derive with untrusted bits (some invalid on purpose)
                if (live_n == 0) continue;
                const src = live[rnd.uintLessThan(usize, live_n)];
                const bits = rnd.int(u32) & (Rights.valid_mask | (1 << 9) | (1 << 17));
                if ((bits & ~Rights.valid_mask) != 0) {
                    try testing.expectError(error.InvalidArgument, table.deriveRaw(src.h, bits));
                } else {
                    const want = try Rights.fromRaw(bits);
                    if (!src.rights.duplicate or !src.rights.contains(want)) {
                        try testing.expectError(error.AccessDenied, table.deriveRaw(src.h, bits));
                    } else if (live_n == live.len) {
                        try testing.expectError(error.QuotaExceeded, table.deriveRaw(src.h, bits));
                    } else {
                        const h = try table.deriveRaw(src.h, bits);
                        live[live_n] = .{ .h = h, .obj = src.obj, .ot = src.ot, .rights = want };
                        live_n += 1;
                    }
                }
            },
            else => { // lookup
                if (live_n == 0) continue;
                const l = live[rnd.uintLessThan(usize, live_n)];
                const need = try Rights.fromRaw(rnd.int(u32) & Rights.valid_mask);
                const ot = all_types[rnd.uintLessThan(usize, all_types.len)];
                if (ot != l.ot) {
                    try testing.expectError(error.InvalidHandle, table.lookup(l.h, ot, need));
                } else if (!l.rights.contains(need)) {
                    try testing.expectError(error.AccessDenied, table.lookup(l.h, ot, need));
                } else {
                    const r = try table.lookup(l.h, ot, need);
                    try testing.expectEqual(l.obj, r.object);
                    try testing.expectEqual(l.rights.toRaw(), r.rights.toRaw());
                }
            },
        }
        try testing.expectEqual(@as(u32, @intCast(live_n)), table.liveCount());
        // Spot-check that recently released handles remain invalid.
        if (dead_n > 0) {
            const d = dead[rnd.uintLessThan(usize, dead_n)];
            try testing.expectError(error.InvalidHandle, table.lookup(d, .channel, .none));
        }
    }
}

// -------------------------------------------------------------- resources ----

test "RES-01 a reservation within limits succeeds" {
    var a = try res.Account.create(.{ .pages = 10, .threads = 2 }, null);
    try a.reserve(.pages, 4);
    try a.reserve(.pages, 6); // exactly at the limit
    try a.reserve(.threads, 1);
    try testing.expectEqual(@as(u64, 10), a.used(.pages));
    try testing.expectEqual(@as(u64, 1), a.used(.threads));
}

test "RES-02 a reservation exceeding the limit fails" {
    var a = try res.Account.create(.{ .pages = 10 }, null);
    try a.reserve(.pages, 10);
    try testing.expectError(error.QuotaExceeded, a.reserve(.pages, 1));
    // A class with limit 0 admits nothing.
    try testing.expectError(error.QuotaExceeded, a.reserve(.channels, 1));
    var b = try res.Account.create(.{ .pages = 10 }, null);
    try testing.expectError(error.QuotaExceeded, b.reserve(.pages, 11));
}

test "RES-03 arithmetic overflow is rejected" {
    const max = std.math.maxInt(u64);
    var a = try res.Account.create(.{ .queued_bytes = max }, null);
    try a.reserve(.queued_bytes, max);
    try testing.expectError(error.Overflow, a.reserve(.queued_bytes, 1));
    try testing.expectEqual(max, a.used(.queued_bytes));
    var b = try res.Account.create(.{ .queued_bytes = max }, null);
    try b.reserve(.queued_bytes, 5);
    try testing.expectError(error.Overflow, b.reserve(.queued_bytes, max));
    try testing.expectEqual(@as(u64, 5), b.used(.queued_bytes));
}

test "RES-04 a failed reservation does not modify any counter" {
    var parent = try res.Account.create(.{ .pages = 5, .handles = 5 }, null);
    var child = try res.Account.create(.{ .pages = 5, .handles = 5 }, &parent);
    try parent.reserve(.pages, 4); // parent has 1 page left, child limit is 5

    const p_before = parent;
    const c_before = child;
    // Child limit allows 3, parent does not: nothing may change anywhere.
    try testing.expectError(error.QuotaExceeded, child.reserve(.pages, 3));
    try testing.expectEqualDeep(p_before.usage, parent.usage);
    try testing.expectEqualDeep(c_before.usage, child.usage);

    // Direct failure on a single account.
    var solo = try res.Account.create(.{ .pages = 2 }, null);
    try solo.reserve(.pages, 1);
    const s_before = solo.usage;
    try testing.expectError(error.QuotaExceeded, solo.reserve(.pages, 2));
    try testing.expectEqualDeep(s_before, solo.usage);
}

test "RES-05 release cannot underflow a counter" {
    var a = try res.Account.create(.{ .pages = 10 }, null);
    try testing.expectError(error.InvalidState, a.release(.pages, 1));
    try a.reserve(.pages, 3);
    try testing.expectError(error.InvalidState, a.release(.pages, 4));
    try testing.expectEqual(@as(u64, 3), a.used(.pages));
    try a.release(.pages, 3);
    try testing.expectEqual(@as(u64, 0), a.used(.pages));
    try testing.expectError(error.InvalidState, a.release(.pages, 1));
}

test "RES-06 an invalid release does not corrupt accounting at any level" {
    var parent = try res.Account.create(.{ .pages = 10 }, null);
    var child = try res.Account.create(.{ .pages = 10 }, &parent);
    try child.reserve(.pages, 5);
    // Someone (incorrectly) returns pages directly to the parent.
    try parent.release(.pages, 3);
    const p_before = parent.usage;
    const c_before = child.usage;
    // Now the child's release would underflow the parent: reject, change nothing.
    try testing.expectError(error.InvalidState, child.release(.pages, 5));
    try testing.expectEqualDeep(p_before, parent.usage);
    try testing.expectEqualDeep(c_before, child.usage);
}

test "RES-07 a child charges and releases its ancestors" {
    var root = try res.Account.create(.{ .pages = 10, .threads = 4 }, null);
    var mid = try res.Account.create(.{ .pages = 8, .threads = 4 }, &root);
    var leaf = try res.Account.create(.{ .pages = 6, .threads = 2 }, &mid);
    try leaf.reserve(.pages, 6);
    try testing.expectEqual(@as(u64, 6), leaf.used(.pages));
    try testing.expectEqual(@as(u64, 6), mid.used(.pages));
    try testing.expectEqual(@as(u64, 6), root.used(.pages));
    try leaf.release(.pages, 6);
    try testing.expectEqual(@as(u64, 0), root.used(.pages));
}

test "RES-08 a child limit cannot exceed the parent's remaining budget" {
    var parent = try res.Account.create(.{ .pages = 10 }, null);
    try parent.reserve(.pages, 4);
    try testing.expectError(error.QuotaExceeded, res.Account.create(.{ .pages = 7 }, &parent));
    const ok = try res.Account.create(.{ .pages = 6 }, &parent);
    try testing.expectEqual(@as(u64, 6), ok.limit(.pages));
    // A failed create modified nothing.
    try testing.expectEqual(@as(u64, 4), parent.used(.pages));
}

test "RES-09 zero-sized reservations and releases are harmless" {
    var a = try res.Account.create(.{}, null);
    try a.reserve(.pages, 0);
    try a.release(.pages, 0);
    try testing.expectEqual(@as(u64, 0), a.used(.pages));
}

test "RES-10 resource classes are independent" {
    var a = try res.Account.create(.{ .pages = 1, .handles = 1, .threads = 1, .channels = 1, .queued_bytes = 1 }, null);
    inline for (std.meta.tags(res.Resource)) |r| try a.reserve(r, 1);
    inline for (std.meta.tags(res.Resource)) |r| {
        try testing.expectEqual(@as(u64, 1), a.used(r));
        try testing.expectError(error.QuotaExceeded, a.reserve(r, 1));
    }
}

// ----------------------------------------------------------------- memory ----

test "MEM-01 zero-length ranges are valid up to and including the exact end" {
    _ = try mem.validateWithin(100, 0, 0);
    _ = try mem.validateWithin(100, 50, 0);
    _ = try mem.validateWithin(100, 100, 0);
    try testing.expectError(error.InvalidArgument, mem.validateWithin(100, 101, 0));
    _ = try mem.validateWithin(0, 0, 0);
    try testing.expectError(error.InvalidArgument, mem.validateWithin(0, 1, 0));
}

test "MEM-02 exact-end boundaries" {
    _ = try mem.validateWithin(100, 0, 100);
    _ = try mem.validateWithin(100, 99, 1);
    _ = try mem.validateWithin(100, 100 - 10, 10);
    try testing.expectError(error.InvalidArgument, mem.validateWithin(100, 0, 101));
    try testing.expectError(error.InvalidArgument, mem.validateWithin(100, 99, 2));
    try testing.expectError(error.InvalidArgument, mem.validateWithin(100, 100, 1));
}

test "MEM-03 out-of-range offsets are rejected" {
    try testing.expectError(error.InvalidArgument, mem.validateWithin(100, 1000, 1));
    try testing.expectError(error.InvalidArgument, mem.validateWithin(100, 101, 0));
    try testing.expectError(error.InvalidArgument, mem.validateWithin(0, 0, 1));
}

test "MEM-04 offset + length overflow is detected, even when the bound is huge" {
    const max = std.math.maxInt(u64);
    try testing.expectError(error.Overflow, mem.checkedRange(max, 1));
    try testing.expectError(error.Overflow, mem.checkedRange(1, max));
    try testing.expectError(error.Overflow, mem.checkedRange(max / 2 + 1, max / 2 + 1));
    try testing.expectError(error.Overflow, mem.validateWithin(max, max, 1));
    // Exactly reaching max is not an overflow.
    const r = try mem.checkedRange(max - 5, 5);
    try testing.expectEqual(max, try r.end());
}

test "MEM-05 element-count to byte-length conversion is checked" {
    try testing.expectEqual(@as(u64, 24), try mem.byteLen(3, 8));
    try testing.expectEqual(@as(u64, 0), try mem.byteLen(0, 8));
    try testing.expectError(error.Overflow, mem.byteLen(std.math.maxInt(u64), 2));
    try testing.expectError(error.Overflow, mem.byteLen(1 << 63, 2));
}

test "MEM-06 alignment checks require a power-of-two alignment" {
    try mem.checkAligned(0, 4096);
    try mem.checkAligned(8192, 4096);
    try mem.checkAligned(7, 1);
    try testing.expectError(error.InvalidArgument, mem.checkAligned(4097, 4096));
    try testing.expectError(error.InvalidArgument, mem.checkAligned(4096, 0));
    try testing.expectError(error.InvalidArgument, mem.checkAligned(4096, 3));
    try testing.expectError(error.InvalidArgument, mem.checkAligned(6, 6));
    try testing.expect(mem.isPowerOfTwo(1));
    try testing.expect(mem.isPowerOfTwo(1 << 63));
    try testing.expect(!mem.isPowerOfTwo(0));
    try testing.expect(!mem.isPowerOfTwo(12));
}

test "MEM-07 alignUp rounds up and detects overflow" {
    try testing.expectEqual(@as(u64, 0), try mem.alignUp(0, 4096));
    try testing.expectEqual(@as(u64, 4096), try mem.alignUp(1, 4096));
    try testing.expectEqual(@as(u64, 4096), try mem.alignUp(4096, 4096));
    try testing.expectEqual(@as(u64, 8192), try mem.alignUp(4097, 4096));
    try testing.expectError(error.Overflow, mem.alignUp(std.math.maxInt(u64), 4096));
    try testing.expectError(error.Overflow, mem.alignUp(std.math.maxInt(u64) - 4094, 4096));
    try testing.expectError(error.InvalidArgument, mem.alignUp(10, 0));
    try testing.expectError(error.InvalidArgument, mem.alignUp(10, 12));
}

test "MEM-08 a user window bounds untrusted address spans without touching memory" {
    const w = mem.UserWindow{ .lo = 0x1000, .hi = 0x10000 };
    _ = try w.validateSpan(0x1000, 0x1000);
    _ = try w.validateSpan(0x1000, 0xF000); // exact end
    _ = try w.validateSpan(0x10000, 0); // zero length at the exact end
    try testing.expectError(error.InvalidArgument, w.validateSpan(0x10000, 1));
    try testing.expectError(error.InvalidArgument, w.validateSpan(0xFFF, 1)); // below lo
    try testing.expectError(error.InvalidArgument, w.validateSpan(0, 0)); // null address
    try testing.expectError(error.InvalidArgument, w.validateSpan(0x10001, 0));
    try testing.expectError(error.Overflow, w.validateSpan(std.math.maxInt(u64), 2));
    try testing.expectError(error.InvalidArgument, w.validateSpan(0x2000, 0xF000)); // runs past hi
    const bad = mem.UserWindow{ .lo = 10, .hi = 5 };
    try testing.expectError(error.Internal, bad.validateSpan(6, 0));
}

test "MEM-09 subslice returns exactly the validated bytes" {
    const data = "abcdefghij";
    try testing.expectEqualStrings("cde", try mem.subslice(data, 2, 3));
    try testing.expectEqualStrings("", try mem.subslice(data, 10, 0));
    try testing.expectEqualStrings(data, try mem.subslice(data, 0, 10));
    try testing.expectError(error.InvalidArgument, mem.subslice(data, 8, 3));
    try testing.expectError(error.InvalidArgument, mem.subslice(data, 11, 0));
    try testing.expectError(error.Overflow, mem.subslice(data, std.math.maxInt(u64), 2));
}

// -------------------------------------------------------------------- ipc ----

fn sampleHeader() ipc.Header {
    return .{ .op = 7, .kind = .call, .request_id = 0x1122334455667788, .handle_count = 2 };
}

test "IPC-01 encode then decode round-trips" {
    var buf: [64]u8 = undefined;
    const n = try ipc.encode(&buf, sampleHeader(), "hello", .{});
    try testing.expectEqual(@as(usize, ipc.wire_header_size + 5), n);
    const m = try ipc.decode(buf[0..n], .{});
    try testing.expectEqual(@as(u16, 7), m.header.op);
    try testing.expectEqual(ipc.MsgKind.call, m.header.kind);
    try testing.expectEqual(@as(u64, 0x1122334455667788), m.header.request_id);
    try testing.expectEqual(@as(u32, 2), m.header.handle_count);
    try testing.expectEqualStrings("hello", m.payload);
    // Empty payload is legal.
    const n0 = try ipc.encode(&buf, sampleHeader(), "", .{});
    try testing.expectEqual(@as(usize, 0), (try ipc.decode(buf[0..n0], .{})).payload.len);
}

test "IPC-02 truncated input is rejected" {
    var buf: [64]u8 = undefined;
    const n = try ipc.encode(&buf, sampleHeader(), "hello", .{});
    var len: usize = 0;
    while (len < n) : (len += 1) {
        try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..len], .{}));
    }
}

test "IPC-03 an unsupported ABI version is rejected with BadVersion" {
    var buf: [64]u8 = undefined;
    var h = sampleHeader();
    h.abi_ver = 0x0002;
    const n = try ipc.encode(&buf, h, "x", .{});
    try testing.expectError(error.BadVersion, ipc.decode(buf[0..n], .{}));
    h.abi_ver = 0x0101;
    const n2 = try ipc.encode(&buf, h, "x", .{});
    try testing.expectError(error.BadVersion, ipc.decode(buf[0..n2], .{}));
    h.abi_ver = 0;
    const n3 = try ipc.encode(&buf, h, "x", .{});
    try testing.expectError(error.BadVersion, ipc.decode(buf[0..n3], .{}));
}

test "IPC-04 unknown flags, non-zero reserved bytes and op 0 are rejected" {
    var buf: [64]u8 = undefined;
    const n = try ipc.encode(&buf, sampleHeader(), "x", .{});
    var copy: [64]u8 = undefined;

    @memcpy(copy[0..n], buf[0..n]);
    copy[8] = 1; // flags
    try testing.expectError(error.InvalidArgument, ipc.decode(copy[0..n], .{}));

    @memcpy(copy[0..n], buf[0..n]);
    copy[14] = 1; // reserved
    try testing.expectError(error.InvalidArgument, ipc.decode(copy[0..n], .{}));

    @memcpy(copy[0..n], buf[0..n]);
    copy[6] = 0;
    copy[7] = 0; // op = 0
    try testing.expectError(error.InvalidArgument, ipc.decode(copy[0..n], .{}));

    var h = sampleHeader();
    h.op = 0;
    try testing.expectError(error.InvalidArgument, ipc.encode(&buf, h, "x", .{}));
}

test "IPC-05 unknown message kinds are rejected" {
    var buf: [64]u8 = undefined;
    const n = try ipc.encode(&buf, sampleHeader(), "x", .{});
    const bad_kinds = [_]u16{ 0, 4, 255, 0x8000, 0xFFFF };
    for (bad_kinds) |k| {
        std.mem.writeInt(u16, buf[12..14], k, .little);
        try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..n], .{}));
    }
    inline for (std.meta.tags(ipc.MsgKind)) |k| {
        std.mem.writeInt(u16, buf[12..14], @intFromEnum(k), .little);
        _ = try ipc.decode(buf[0..n], .{});
    }
}

test "IPC-06 a declared size that disagrees with the input or the limit is rejected" {
    var buf: [128]u8 = undefined;
    const n = try ipc.encode(&buf, sampleHeader(), "hello", .{});

    // size larger than the bytes supplied
    std.mem.writeInt(u32, buf[0..4], @intCast(n + 1), .little);
    try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..n], .{}));
    // size smaller than the bytes supplied
    std.mem.writeInt(u32, buf[0..4], @intCast(n - 1), .little);
    try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..n], .{}));
    // size below the fixed header
    std.mem.writeInt(u32, buf[0..4], 4, .little);
    try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..n], .{}));
    // size over the configured maximum
    std.mem.writeInt(u32, buf[0..4], @intCast(n), .little);
    try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..n], .{ .max_message_bytes = @intCast(n - 1) }));
    _ = try ipc.decode(buf[0..n], .{ .max_message_bytes = @intCast(n) });
}

test "IPC-07 a payload length that disagrees with the size is rejected" {
    var buf: [128]u8 = undefined;
    const n = try ipc.encode(&buf, sampleHeader(), "hello", .{});
    const bad = [_]u32{ 0, 4, 6, 100, 0x7FFFFFFF, 0xFFFFFFFF };
    for (bad) |p| {
        std.mem.writeInt(u32, buf[24..28], p, .little);
        try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..n], .{}));
    }
}

test "IPC-08 a handle count over the limit is rejected" {
    var buf: [64]u8 = undefined;
    var h = sampleHeader();
    h.handle_count = 8;
    const n = try ipc.encode(&buf, h, "x", .{});
    _ = try ipc.decode(buf[0..n], .{});
    try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..n], .{ .max_handles = 7 }));
    h.handle_count = 9;
    try testing.expectError(error.InvalidArgument, ipc.encode(&buf, h, "x", .{}));
    std.mem.writeInt(u32, buf[28..32], 0xFFFFFFFF, .little);
    try testing.expectError(error.InvalidArgument, ipc.decode(buf[0..n], .{}));
}

test "IPC-09 total size computation is overflow-checked" {
    try testing.expectEqual(@as(u32, 32), try ipc.totalSize(0));
    try testing.expectEqual(@as(u32, 132), try ipc.totalSize(100));
    try testing.expectError(error.Overflow, ipc.totalSize(std.math.maxInt(u64)));
    try testing.expectError(error.Overflow, ipc.totalSize(std.math.maxInt(u32)));
    try testing.expectError(error.Overflow, ipc.totalSize(std.math.maxInt(u32) - 31));
    try testing.expectEqual(@as(u32, std.math.maxInt(u32)), try ipc.totalSize(std.math.maxInt(u32) - 32));
}

test "IPC-10 encode rejects oversized payloads and undersized output buffers" {
    var buf: [64]u8 = undefined;
    try testing.expectError(error.InvalidArgument, ipc.encode(&buf, sampleHeader(), "0123456789", .{ .max_message_bytes = 40 }));
    try testing.expectError(error.InvalidArgument, ipc.encode(buf[0..35], sampleHeader(), "hello", .{}));
    _ = try ipc.encode(buf[0..37], sampleHeader(), "hello", .{});
}

test "IPC-11 transfer lists are validated against the capability table" {
    var t = Table16.init(null);
    const ok1 = try t.insert(oid(1), .channel, .{ .transfer = true });
    const ok2 = try t.insert(oid(2), .memory_object, .{ .transfer = true, .read = true });
    const no_xfer = try t.insert(oid(3), .channel, .{ .read = true });
    const stale = try t.insert(oid(4), .channel, .{ .transfer = true });
    try t.release(stale);

    const before = t;

    try ipc.validateTransferList(&t, &.{}, 0);
    try ipc.validateTransferList(&t, &.{ ok1, ok2 }, 2);

    try testing.expectError(error.InvalidArgument, ipc.validateTransferList(&t, &.{ok1}, 2)); // count mismatch
    try testing.expectError(error.InvalidArgument, ipc.validateTransferList(&t, &.{ ok1, ok2 }, 1));
    try testing.expectError(error.AccessDenied, ipc.validateTransferList(&t, &.{ ok1, no_xfer }, 2));
    try testing.expectError(error.InvalidHandle, ipc.validateTransferList(&t, &.{stale}, 1));
    try testing.expectError(error.InvalidHandle, ipc.validateTransferList(&t, &.{.invalid}, 1));
    try testing.expectError(error.InvalidArgument, ipc.validateTransferList(&t, &.{ ok1, ok2, ok1 }, 3)); // duplicate

    // Validation only reads: the table is bit-for-bit what it was.
    try testing.expectEqualDeep(before, t);
}

test "IPC-12 deterministic fuzz: decode never misbehaves and accepted messages satisfy the invariants" {
    var prng = std.Random.DefaultPrng.init(0x49504321);
    const rnd = prng.random();
    const limits = ipc.Limits{};

    var accepted: usize = 0;
    var rejected: usize = 0;

    // Pure random buffers.
    var buf: [200]u8 = undefined;
    var i: usize = 0;
    while (i < 20000) : (i += 1) {
        const len = rnd.uintLessThan(usize, buf.len + 1);
        rnd.bytes(buf[0..len]);
        if (ipc.decode(buf[0..len], limits)) |m| {
            accepted += 1;
            try checkAccepted(buf[0..len], m, limits);
        } else |_| rejected += 1;
    }

    // Valid messages with 1-3 random byte mutations.
    var valid: [200]u8 = undefined;
    var j: usize = 0;
    while (j < 20000) : (j += 1) {
        const plen = rnd.uintLessThan(usize, 100);
        var payload: [100]u8 = undefined;
        rnd.bytes(payload[0..plen]);
        var h = sampleHeader();
        h.handle_count = rnd.uintLessThan(u32, 9);
        h.op = rnd.intRangeAtMost(u16, 1, 50);
        h.kind = std.meta.tags(ipc.MsgKind)[rnd.uintLessThan(usize, 3)];
        const n = try ipc.encode(&valid, h, payload[0..plen], limits);
        const mutations = rnd.uintLessThan(usize, 4);
        var k: usize = 0;
        while (k < mutations) : (k += 1) {
            valid[rnd.uintLessThan(usize, n)] = rnd.int(u8);
        }
        const cut = if (rnd.boolean()) n else rnd.uintLessThan(usize, n + 1);
        if (ipc.decode(valid[0..cut], limits)) |m| {
            accepted += 1;
            try checkAccepted(valid[0..cut], m, limits);
        } else |_| rejected += 1;
    }

    try testing.expect(accepted > 0);
    try testing.expect(rejected > 0);
}

fn checkAccepted(bytes: []const u8, m: ipc.Message, limits: ipc.Limits) !void {
    try testing.expectEqual(ipc.abi_version, m.header.abi_ver);
    try testing.expect(m.header.op != 0);
    try testing.expectEqual(@as(u32, 0), m.header.flags);
    try testing.expect(m.header.handle_count <= limits.max_handles);
    try testing.expect(bytes.len <= limits.max_message_bytes);
    try testing.expectEqual(bytes.len, ipc.wire_header_size + m.payload.len);
    // The payload really is a window into the input.
    try testing.expect(@intFromPtr(m.payload.ptr) >= @intFromPtr(bytes.ptr));
    try testing.expect(@intFromPtr(m.payload.ptr) + m.payload.len <= @intFromPtr(bytes.ptr) + bytes.len);
}

// ------------------------------------------------------------------- arch ----

const MockArch = struct {
    pub const name: []const u8 = "hosted-mock";
    pub const page_size: u64 = 4096;
    pub const user_va_lo: u64 = 0x1000;
    pub const user_va_hi: u64 = 0x0000_7fff_ffff_f000;

    var irq_enabled: bool = true;

    pub fn irqSaveDisable() bool {
        const prev = irq_enabled;
        irq_enabled = false;
        return prev;
    }
    pub fn irqRestore(prev: bool) void {
        irq_enabled = prev;
    }
    pub fn fullBarrier() void {}
    pub fn cpuRelax() void {}
};

test "ARCH-01 a conforming backend satisfies the contract and its facts validate" {
    try testing.expect(arch.implements(MockArch));
    comptime arch.assertImplements(MockArch);
    const w = try arch.userWindow(MockArch);
    try testing.expectEqual(@as(u64, 0x1000), w.lo);
    _ = try w.validateSpan(0x2000, 0x1000);

    // The mock behaves as a save/restore pair.
    try testing.expect(MockArch.irq_enabled);
    const prev = MockArch.irqSaveDisable();
    try testing.expect(prev);
    try testing.expect(!MockArch.irq_enabled);
    MockArch.irqRestore(prev);
    try testing.expect(MockArch.irq_enabled);
}

test "ARCH-02 incomplete or mistyped backends are detected" {
    const Empty = struct {};
    const MissingFn = struct {
        pub const name: []const u8 = "x";
        pub const page_size: u64 = 4096;
        pub const user_va_lo: u64 = 0;
        pub const user_va_hi: u64 = 1;
        pub fn irqSaveDisable() bool {
            return false;
        }
        pub fn irqRestore(_: bool) void {}
        pub fn fullBarrier() void {}
        // cpuRelax missing
    };
    const WrongSig = struct {
        pub const name: []const u8 = "x";
        pub const page_size: u64 = 4096;
        pub const user_va_lo: u64 = 0;
        pub const user_va_hi: u64 = 1;
        pub fn irqSaveDisable() bool {
            return false;
        }
        pub fn irqRestore(_: u8) void {} // wrong parameter type
        pub fn fullBarrier() void {}
        pub fn cpuRelax() void {}
    };
    const WrongConstType = struct {
        pub const name: []const u8 = "x";
        pub const page_size = 4096; // comptime_int, not u64
        pub const user_va_lo: u64 = 0;
        pub const user_va_hi: u64 = 1;
        pub fn irqSaveDisable() bool {
            return false;
        }
        pub fn irqRestore(_: bool) void {}
        pub fn fullBarrier() void {}
        pub fn cpuRelax() void {}
    };
    try testing.expect(!arch.implements(Empty));
    try testing.expect(!arch.implements(MissingFn));
    try testing.expect(!arch.implements(WrongSig));
    try testing.expect(!arch.implements(WrongConstType));
}

test "ARCH-03 backend facts are validated" {
    _ = try arch.validateFacts(4096, 0x1000, 0x8000);
    try testing.expectError(error.InvalidArgument, arch.validateFacts(0, 0x1000, 0x8000)); // no page size
    try testing.expectError(error.InvalidArgument, arch.validateFacts(4000, 0x1000, 0x8000)); // not a power of two
    try testing.expectError(error.InvalidArgument, arch.validateFacts(4096, 0x8000, 0x8000)); // empty window
    try testing.expectError(error.InvalidArgument, arch.validateFacts(4096, 0x9000, 0x8000)); // inverted
    try testing.expectError(error.InvalidArgument, arch.validateFacts(4096, 0x1001, 0x8000)); // lo misaligned
    try testing.expectError(error.InvalidArgument, arch.validateFacts(4096, 0x1000, 0x8001)); // hi misaligned
}

// ------------------------------------------------------------------ cross ----

test "X-01 table, account and IPC validation compose: a transfer message is accepted, then a stale handle is not" {
    var acct = try res.Account.create(.{ .handles = 8, .queued_bytes = 4096 }, null);
    var t = Table16.init(&acct);
    const a = try t.insert(oid(1), .channel, .{ .transfer = true, .read = true });
    const b = try t.insert(oid(2), .memory_object, .{ .transfer = true });

    var buf: [128]u8 = undefined;
    var hdr = sampleHeader();
    hdr.handle_count = 2;
    const n = try ipc.encode(&buf, hdr, "payload", .{});

    // The sender charges queued bytes for the message, as Batch 1 B.5 describes.
    try acct.reserve(.queued_bytes, n);
    const m = try ipc.decode(buf[0..n], .{});
    try ipc.validateTransferList(&t, &.{ a, b }, m.header.handle_count);

    // Sender releases one handle before the transfer completes: now it is stale.
    try t.release(b);
    try testing.expectError(error.InvalidHandle, ipc.validateTransferList(&t, &.{ a, b }, m.header.handle_count));

    try acct.release(.queued_bytes, n);
    try testing.expectEqual(@as(u64, 0), acct.used(.queued_bytes));
}
