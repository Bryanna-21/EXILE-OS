//! Generation-checked slot arena (Batch 3).
//!
//! One mechanism, used for every table in the hosted model (handle tables, object
//! registry, capability nodes, operation pins, address-space slots), so the
//! "generation-safe reuse" rules are implemented and tested exactly once.
//!
//! Rules (identical to the Batch 2 capability table, generalised):
//!   * A slot has a generation that starts at 1 and is never 0.
//!   * Freeing a live slot increments its generation, so every `Ref` issued
//!     before the free no longer matches (stale references are rejected).
//!   * EXHAUSTION: if the generation would wrap past its maximum, the slot is
//!     RETIRED permanently instead of being reused. A retired slot can never
//!     again be handed out, so no stale reference can ever become valid again
//!     (no ABA after wraparound). The cost is that capacity shrinks by one slot.
//!   * Index `none` (the maximum value of `Index`) is reserved as a sentinel, so
//!     at most `none` slots can exist.
//!   * Growth is bounded by `limit`. Growth may allocate and may fail with
//!     `OutOfMemory`; reaching `limit` with no free slot is `QuotaExceeded`.
//!
//! Two-step allocation (`reserveOne` then `allocAssumeCapacity`) lets a caller do
//! every FALLIBLE step of an operation before it mutates anything, so a failed
//! operation has nothing to roll back in this structure.
//!
//! POINTER STABILITY: `reserveOne` may reallocate the backing array. Any pointer
//! returned by `get`/`ptrAt` before a `reserveOne` on the same arena is invalid
//! afterwards. Callers must hold indices or `Ref`s across such calls, not pointers.
//!
//! THREAD SAFETY: none. The owning `Kernel` serialises all access with its mutex.

const std = @import("std");
const Allocator = std.mem.Allocator;

pub fn GenArena(comptime T: type, comptime Index: type, comptime Gen: type) type {
    comptime {
        std.debug.assert(@typeInfo(Index) == .int and @typeInfo(Index).int.signedness == .unsigned);
        std.debug.assert(@typeInfo(Gen) == .int and @typeInfo(Gen).int.signedness == .unsigned);
    }

    return struct {
        const Self = @This();

        /// Reserved sentinel index; never a valid slot.
        pub const none: Index = std.math.maxInt(Index);
        pub const max_gen: Gen = std.math.maxInt(Gen);

        pub const State = enum(u8) { free, live, retired };

        pub const Ref = struct {
            index: Index,
            gen: Gen,

            pub fn eql(a: Ref, b: Ref) bool {
                return a.index == b.index and a.gen == b.gen;
            }
        };

        pub const FreeOutcome = enum { reusable, retired };

        const Slot = struct {
            state: State = .free,
            gen: Gen = 1,
            next_free: Index = none,
            value: T = undefined,
        };

        slots: std.ArrayListUnmanaged(Slot) = .empty,
        free_head: Index = none,
        live_count: usize = 0,
        retired_count: usize = 0,
        limit: usize,

        pub fn init(limit: usize) Self {
            std.debug.assert(limit <= none);
            return .{ .limit = limit };
        }

        pub fn deinit(self: *Self, allocator: Allocator) void {
            self.slots.deinit(allocator);
            self.* = undefined;
        }

        pub fn liveCount(self: *const Self) usize {
            return self.live_count;
        }

        pub fn retiredCount(self: *const Self) usize {
            return self.retired_count;
        }

        /// Number of slots ever created (live + free + retired).
        pub fn slotCount(self: *const Self) usize {
            return self.slots.items.len;
        }

        /// Make the next `allocAssumeCapacity` infallible. Never changes any slot.
        /// `QuotaExceeded`: no free slot and `limit` slots already exist.
        pub fn reserveOne(self: *Self, allocator: Allocator) error{ OutOfMemory, QuotaExceeded }!void {
            if (self.free_head != none) return;
            if (self.slots.items.len >= self.limit) return error.QuotaExceeded;
            try self.slots.ensureUnusedCapacity(allocator, 1);
        }

        /// Allocate a slot. Only valid after a successful `reserveOne` with no
        /// intervening allocation or growth on this arena.
        pub fn allocAssumeCapacity(self: *Self, value: T) Ref {
            if (self.free_head != none) {
                const index = self.free_head;
                const slot = &self.slots.items[index];
                std.debug.assert(slot.state == .free);
                self.free_head = slot.next_free;
                slot.state = .live;
                slot.next_free = none;
                slot.value = value;
                self.live_count += 1;
                return .{ .index = index, .gen = slot.gen };
            }
            std.debug.assert(self.slots.items.len < self.limit);
            std.debug.assert(self.slots.items.len < self.slots.capacity);
            const index: Index = @intCast(self.slots.items.len);
            self.slots.appendAssumeCapacity(.{ .state = .live, .gen = 1, .value = value });
            self.live_count += 1;
            return .{ .index = index, .gen = 1 };
        }

        fn slotFor(self: *Self, ref: Ref) ?*Slot {
            if (@as(usize, ref.index) >= self.slots.items.len) return null;
            const slot = &self.slots.items[ref.index];
            if (slot.state != .live or slot.gen != ref.gen) return null;
            return slot;
        }

        /// Live slot whose generation matches, else null.
        pub fn get(self: *Self, ref: Ref) ?*T {
            const slot = self.slotFor(ref) orelse return null;
            return &slot.value;
        }

        pub fn getConst(self: *const Self, ref: Ref) ?*const T {
            return @constCast(self).get(ref);
        }

        /// Live slot at `index` regardless of generation (for internal links that
        /// are maintained exactly under the owner's lock).
        pub fn ptrAt(self: *Self, index: usize) ?*T {
            if (index >= self.slots.items.len) return null;
            const slot = &self.slots.items[index];
            if (slot.state != .live) return null;
            return &slot.value;
        }

        /// The current reference of the live slot at `index`.
        pub fn refOf(self: *const Self, index: usize) ?Ref {
            if (index >= self.slots.items.len) return null;
            const slot = &self.slots.items[index];
            if (slot.state != .live) return null;
            return .{ .index = @intCast(index), .gen = slot.gen };
        }

        pub fn stateAt(self: *const Self, index: usize) ?State {
            if (index >= self.slots.items.len) return null;
            return self.slots.items[index].state;
        }

        /// Free a live slot. `InvalidRef` if `ref` is not the current live ref.
        /// Increments the generation, or retires the slot on exhaustion.
        pub fn free(self: *Self, ref: Ref) error{InvalidRef}!FreeOutcome {
            const slot = self.slotFor(ref) orelse return error.InvalidRef;
            slot.value = undefined;
            self.live_count -= 1;
            if (slot.gen == max_gen) {
                slot.state = .retired;
                slot.next_free = none;
                self.retired_count += 1;
                return .retired;
            }
            slot.gen += 1;
            slot.state = .free;
            slot.next_free = self.free_head;
            self.free_head = ref.index;
            return .reusable;
        }

        /// Internal consistency of counters and the free list.
        pub fn isConsistent(self: *const Self) bool {
            var live: usize = 0;
            var retired: usize = 0;
            var free_slots: usize = 0;
            for (self.slots.items) |slot| {
                switch (slot.state) {
                    .live => live += 1,
                    .retired => retired += 1,
                    .free => free_slots += 1,
                }
                if (slot.gen == 0) return false;
            }
            if (live != self.live_count or retired != self.retired_count) return false;
            var walked: usize = 0;
            var cursor = self.free_head;
            while (cursor != none) {
                if (@as(usize, cursor) >= self.slots.items.len) return false;
                const slot = self.slots.items[cursor];
                if (slot.state != .free) return false;
                walked += 1;
                if (walked > free_slots) return false; // cycle or duplicate
                cursor = slot.next_free;
            }
            return walked == free_slots;
        }

        /// TEST HOOK: force the generation of a FREE slot so exhaustion can be
        /// exercised without billions of cycles. Not for production use.
        pub fn testSetGen(self: *Self, index: usize, gen: Gen) void {
            std.debug.assert(gen != 0);
            const slot = &self.slots.items[index];
            std.debug.assert(slot.state == .free);
            slot.gen = gen;
        }
    };
}
