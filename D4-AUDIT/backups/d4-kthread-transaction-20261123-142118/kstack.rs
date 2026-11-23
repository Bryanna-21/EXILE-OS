//! Kernel stack pool with real reclamation. SPDX-License-Identifier: Apache-2.0
//!
//! `early_alloc` is alloc-only, so stacks cannot be returned to it. Instead one
//! contiguous region is obtained from it once (boot-lifetime) and handed to this
//! pool, which carves it into equal slots and reuses them.
//!
//! Slot layout (low to high address):
//! ```text
//! [ guard: GUARD_BYTES ][ stack: stack_bytes ]   <- top (16-byte aligned)
//! ```
//! The guard region is RESERVED here but not made inaccessible: no page-table
//! manager exists yet, so overflow into it is not trapped. `classify` lets a
//! future page-fault handler identify guard hits. Do not claim overflow
//! protection until the guard pages are unmapped by the VM layer.
//!
//! Ownership: `alloc` hands out a `Stack` token that is neither `Copy` nor
//! `Clone`. `release` consumes it, so a token cannot be released twice through
//! safe code; the pool additionally checks its in-use bit and a per-slot
//! generation, so forged or foreign tokens are rejected rather than trusted.
//!
//! This module only manages address ranges. It never reads or writes the memory.

pub const PAGE: u64 = 4096;
pub const GUARD_BYTES: u64 = PAGE;
pub const MIN_STACK_BYTES: u64 = 4 * PAGE;
pub const MAX_STACK_BYTES: u64 = 1024 * 1024;
pub const MAX_SLOTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Base not page aligned, zero length, overflow, or beyond `limit`.
    BadRegion,
    /// Stack size not a page multiple or outside MIN..=MAX.
    BadStackSize,
    /// Region cannot hold even one slot.
    TooSmall,
    /// Every slot is in use (or retired).
    Exhausted,
    /// Token is foreign, stale, or already released.
    NotOwned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Guard,
    Stack,
    Outside,
}

/// Exclusive ownership of one stack slot. Not `Clone`/`Copy`.
#[derive(Debug, PartialEq, Eq)]
pub struct Stack {
    pool_base: u64,
    slot: u32,
    generation: u32,
    base: u64,
    top: u64,
}

impl Stack {
    /// Lowest usable stack address (just above the guard).
    pub fn base(&self) -> u64 {
        self.base
    }
    /// One past the last usable byte; 16-byte aligned.
    pub fn top(&self) -> u64 {
        self.top
    }
    pub fn guard_base(&self) -> u64 {
        self.base - GUARD_BYTES
    }
    pub fn size(&self) -> u64 {
        self.top - self.base
    }
    pub fn slot(&self) -> usize {
        self.slot as usize
    }
}

pub struct StackPool {
    base: u64,
    stack_bytes: u64,
    slot_bytes: u64,
    slots: usize,
    used: u64,
    retired: u64,
    generation: [u32; MAX_SLOTS],
}

impl StackPool {
    /// `base`/`total_bytes` must come from a validated usable physical range
    /// (for example `EarlyAlloc::alloc`). `limit` is the exclusive end of memory
    /// the kernel can address (the identity-mapped window); the region is
    /// rejected if it does not fit below it.
    pub fn new(base: u64, total_bytes: u64, stack_bytes: u64, limit: u64) -> Result<Self, Error> {
        if base == 0 || base % PAGE != 0 || total_bytes == 0 {
            return Err(Error::BadRegion);
        }
        let end = base.checked_add(total_bytes).ok_or(Error::BadRegion)?;
        if end > limit {
            return Err(Error::BadRegion);
        }
        if stack_bytes % PAGE != 0 || !(MIN_STACK_BYTES..=MAX_STACK_BYTES).contains(&stack_bytes) {
            return Err(Error::BadStackSize);
        }
        let slot_bytes = stack_bytes + GUARD_BYTES;
        let slots = ((total_bytes / slot_bytes) as usize).min(MAX_SLOTS);
        if slots == 0 {
            return Err(Error::TooSmall);
        }
        Ok(Self {
            base,
            stack_bytes,
            slot_bytes,
            slots,
            used: 0,
            retired: 0,
            generation: [1; MAX_SLOTS],
        })
    }

    pub fn capacity(&self) -> usize {
        self.slots
    }
    pub fn in_use(&self) -> usize {
        self.used.count_ones() as usize
    }
    pub fn stack_bytes(&self) -> u64 {
        self.stack_bytes
    }
    /// Bytes of the region actually covered by slots.
    pub fn span(&self) -> (u64, u64) {
        (self.base, self.base + self.slots as u64 * self.slot_bytes)
    }

    fn slot_start(&self, slot: usize) -> u64 {
        self.base + slot as u64 * self.slot_bytes
    }

    pub fn alloc(&mut self) -> Result<Stack, Error> {
        let slot = (0..self.slots)
            .find(|&i| (self.used | self.retired) & (1u64 << i) == 0)
            .ok_or(Error::Exhausted)?;
        self.used |= 1u64 << slot;
        let start = self.slot_start(slot);
        Ok(Stack {
            pool_base: self.base,
            slot: slot as u32,
            generation: self.generation[slot],
            base: start + GUARD_BYTES,
            top: start + self.slot_bytes,
        })
    }

    /// Returns a slot to the pool. The caller must guarantee the stack is no
    /// longer executing (see `kthread`: only after the switch away completed).
    pub fn release(&mut self, stack: Stack) -> Result<(), Error> {
        let slot = stack.slot as usize;
        if stack.pool_base != self.base
            || slot >= self.slots
            || self.used & (1u64 << slot) == 0
            || self.generation[slot] != stack.generation
            || stack.base != self.slot_start(slot) + GUARD_BYTES
            || stack.top != self.slot_start(slot) + self.slot_bytes
        {
            return Err(Error::NotOwned);
        }
        self.used &= !(1u64 << slot);
        match self.generation[slot].checked_add(1) {
            Some(g) => self.generation[slot] = g,
            None => self.retired |= 1u64 << slot, // never reuse: avoids token ABA
        }
        Ok(())
    }

    pub fn classify(&self, addr: u64) -> Region {
        let (lo, hi) = self.span();
        if addr < lo || addr >= hi {
            return Region::Outside;
        }
        if (addr - self.base) % self.slot_bytes < GUARD_BYTES {
            Region::Guard
        } else {
            Region::Stack
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x20_0000;
    const STACK: u64 = 16 * 1024;
    const SLOT: u64 = STACK + GUARD_BYTES;
    const LIMIT: u64 = 1 << 30;

    /// Test-only duplicate, to model a forged or stale token.
    fn dup(s: &Stack) -> Stack {
        Stack {
            pool_base: s.pool_base,
            slot: s.slot,
            generation: s.generation,
            base: s.base,
            top: s.top,
        }
    }

    fn pool(slots: u64) -> StackPool {
        StackPool::new(BASE, slots * SLOT, STACK, LIMIT).unwrap()
    }

    #[test]
    fn validates_region_and_size_parameters() {
        let n = |b, t, s, l| StackPool::new(b, t, s, l).err();
        assert_eq!(n(0, SLOT, STACK, LIMIT), Some(Error::BadRegion));
        assert_eq!(n(BASE + 1, SLOT, STACK, LIMIT), Some(Error::BadRegion));
        assert_eq!(n(BASE, 0, STACK, LIMIT), Some(Error::BadRegion));
        assert_eq!(
            n(u64::MAX - PAGE + 1, SLOT, STACK, u64::MAX),
            Some(Error::BadRegion)
        );
        assert_eq!(
            n(BASE, 4 * SLOT, STACK, BASE + SLOT),
            Some(Error::BadRegion)
        );
        assert_eq!(n(BASE, SLOT, STACK + 1, LIMIT), Some(Error::BadStackSize));
        assert_eq!(n(BASE, SLOT, PAGE, LIMIT), Some(Error::BadStackSize));
        assert_eq!(
            n(BASE, SLOT, MAX_STACK_BYTES + PAGE, LIMIT),
            Some(Error::BadStackSize)
        );
        assert_eq!(n(BASE, SLOT - 1, STACK, LIMIT), Some(Error::TooSmall));
    }

    #[test]
    fn slots_are_aligned_disjoint_and_guarded() {
        let mut p = pool(4);
        let s: Vec<_> = (0..4).map(|_| p.alloc().unwrap()).collect();
        for (i, st) in s.iter().enumerate() {
            assert_eq!(st.top() % 16, 0);
            assert_eq!(st.base() % PAGE, 0);
            assert_eq!(st.size(), STACK);
            assert_eq!(st.guard_base(), BASE + i as u64 * SLOT);
            assert_eq!(p.classify(st.guard_base()), Region::Guard);
            assert_eq!(p.classify(st.base()), Region::Stack);
            assert_eq!(p.classify(st.top() - 1), Region::Stack);
        }
        for w in s.windows(2) {
            assert_eq!(w[0].top(), w[1].guard_base(), "slots abut, never overlap");
        }
        assert_eq!(p.classify(BASE - 1), Region::Outside);
        assert_eq!(p.classify(p.span().1), Region::Outside);
    }

    #[test]
    fn exhaustion_is_an_error_and_release_makes_room() {
        let mut p = pool(2);
        let a = p.alloc().unwrap();
        let _b = p.alloc().unwrap();
        assert_eq!(p.alloc(), Err(Error::Exhausted));
        assert_eq!(p.in_use(), 2);
        let slot = a.slot();
        p.release(a).unwrap();
        assert_eq!(p.in_use(), 1);
        assert_eq!(p.alloc().unwrap().slot(), slot, "freed slot is reused");
    }

    #[test]
    fn release_rejects_stale_forged_and_foreign_tokens() {
        let mut p = pool(2);
        let a = p.alloc().unwrap();
        let forged = dup(&a);
        p.release(a).unwrap();
        assert_eq!(p.release(forged), Err(Error::NotOwned), "double release");

        let mut other = StackPool::new(BASE + 0x100_0000, 2 * SLOT, STACK, LIMIT).unwrap();
        let foreign = other.alloc().unwrap();
        assert_eq!(p.release(foreign), Err(Error::NotOwned));
        assert_eq!(other.in_use(), 1, "failed release must not change state");
    }

    #[test]
    fn stale_token_cannot_free_a_reused_slot() {
        let mut p = pool(1);
        let a = p.alloc().unwrap();
        let stale = dup(&a);
        p.release(a).unwrap();
        let b = p.alloc().unwrap();
        assert_eq!(p.release(stale), Err(Error::NotOwned));
        assert_eq!(p.in_use(), 1);
        p.release(b).unwrap();
    }

    #[test]
    fn generation_exhaustion_retires_the_slot() {
        let mut p = pool(2);
        p.generation[0] = u32::MAX;
        let a = p.alloc().unwrap();
        assert_eq!(a.slot(), 0);
        p.release(a).unwrap();
        assert_eq!(p.alloc().unwrap().slot(), 1, "retired slot skipped");
        assert_eq!(p.alloc(), Err(Error::Exhausted));
    }

    #[test]
    fn capacity_is_capped_and_region_tail_is_unused() {
        let p = StackPool::new(BASE, 200 * SLOT + SLOT / 2, STACK, LIMIT).unwrap();
        assert_eq!(p.capacity(), MAX_SLOTS);
        let q = StackPool::new(BASE, 3 * SLOT + SLOT / 2, STACK, LIMIT).unwrap();
        assert_eq!(q.capacity(), 3);
        assert_eq!(q.span().1, BASE + 3 * SLOT);
    }
}
