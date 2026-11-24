//! Early physical page allocator (Mission A2, file 1 of 5).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Hands out physical pages before any heap or frame allocator exists. Heap
//! independent, `no_std`, fixed-capacity metadata, no `unsafe`, alloc-only (there is
//! no free: the early allocator exists only until Mission B takes over).
//!
//! LIFECYCLE
//!   Uninit --init--> Active --handoff (phys_handoff.rs)--> Sealed
//!   * `init` consumes the validated usable ranges and the boot reservations. It
//!     fails with `AlreadyInitialized` once the allocator is Active or Sealed, so a
//!     second call can never discard live reservations or allocations.
//!   * `alloc` works only while Active.
//!   * Sealing hands the REMAINING free pages to the next owner and empties this
//!     allocator's free list; from then on the pages are not ours, `alloc` fails
//!     with `Sealed`, and nothing is returned twice.
//!
//! POLICY
//!   * Only pages inside the validated usable ranges, below `ceiling`, above
//!     physical page 0 and outside every reservation are ever managed. Usable
//!     memory is trimmed INWARD to whole pages; a page that is only partly usable
//!     or only partly reserved is never handed out.
//!   * Sizes are rounded up to whole pages and every allocation is at least
//!     page-aligned. Larger power-of-two alignments (up to `MAX_ALIGN`) are honoured.
//!   * First fit, lowest address first: deterministic.
//!   * Every mutating operation is all-or-nothing: on `Err` the allocator is
//!     byte-for-byte unchanged (capacity is checked BEFORE anything is modified).
//!   * Allocations are recorded in `allocs`; free ranges can never overlap boot
//!     reservations or allocations (`check_invariants`).
#![allow(dead_code)]
use crate::range::{self, PAGE_SIZE, PhysRange, RangeError};
use crate::reserve::{ReservationSet, ReserveError, ReserveKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Uninit,
    Active,
    Sealed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocError {
    NotInitialized,
    AlreadyInitialized,
    Sealed,
    ZeroSize,
    BadAlignment,
    SizeOverflow,
    /// No free range can satisfy the request.
    OutOfMemory,
    /// Space exists but the fixed metadata tables have no room to record it.
    MetadataFull,
    /// `init` found no managed page after applying every exclusion.
    NoUsableMemory,
    /// `init` would need more free-list slots than the allocator has.
    TooManyRanges,
    Range(RangeError),
    Reserve(ReserveError),
}

impl From<RangeError> for AllocError {
    fn from(e: RangeError) -> Self {
        AllocError::Range(e)
    }
}
impl From<ReserveError> for AllocError {
    fn from(e: ReserveError) -> Self {
        AllocError::Reserve(e)
    }
}

/// `F` free-list slots, `R` boot-reservation slots, `A` allocation-log slots.
#[derive(Debug, Clone)]
pub struct EarlyAllocator<const F: usize, const R: usize, const A: usize> {
    state: State,
    free: [PhysRange; F],
    free_len: usize,
    boot: ReservationSet<R>,
    allocs: ReservationSet<A>,
    managed_bytes: u64,
    allocated_bytes: u64,
    withdrawn_bytes: u64,
    transferred_bytes: u64,
}

impl<const F: usize, const R: usize, const A: usize> Default for EarlyAllocator<F, R, A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const F: usize, const R: usize, const A: usize> EarlyAllocator<F, R, A> {
    pub const fn new() -> Self {
        Self {
            state: State::Uninit,
            free: [PhysRange::UNUSED; F],
            free_len: 0,
            boot: ReservationSet::new(),
            allocs: ReservationSet::new(),
            managed_bytes: 0,
            allocated_bytes: 0,
            withdrawn_bytes: 0,
            transferred_bytes: 0,
        }
    }

    pub const fn state(&self) -> State {
        self.state
    }
    /// Free ranges, ascending, page-aligned, pairwise separated.
    pub fn free_ranges(&self) -> &[PhysRange] {
        &self.free[..self.free_len]
    }
    pub fn free_bytes(&self) -> u64 {
        self.free_ranges()
            .iter()
            .fold(0u64, |a, r| a.saturating_add(r.len()))
    }
    pub fn largest_free(&self) -> u64 {
        self.free_ranges()
            .iter()
            .map(|r| r.len())
            .max()
            .unwrap_or(0)
    }
    pub const fn managed_bytes(&self) -> u64 {
        self.managed_bytes
    }
    pub const fn allocated_bytes(&self) -> u64 {
        self.allocated_bytes
    }
    pub const fn withdrawn_bytes(&self) -> u64 {
        self.withdrawn_bytes
    }
    pub const fn transferred_bytes(&self) -> u64 {
        self.transferred_bytes
    }
    pub fn reservations(&self) -> &ReservationSet<R> {
        &self.boot
    }
    pub fn allocations(&self) -> &ReservationSet<A> {
        &self.allocs
    }

    /// Initializes the allocator.
    ///
    /// `usable` need not be sorted, disjoint or page-aligned; it must contain only
    /// ranges that validation already classified as usable. `reservations` is every
    /// range that must stay out of the allocator (kernel image, boot info, memory
    /// map storage, firmware, bootloader, allocator metadata, ...). Memory at or
    /// above `ceiling` and physical page 0 are never managed.
    /// On error the allocator is unchanged (still `Uninit`).
    pub fn init(
        &mut self,
        usable: impl IntoIterator<Item = PhysRange>,
        reservations: ReservationSet<R>,
        ceiling: u64,
    ) -> Result<(), AllocError> {
        if self.state != State::Uninit {
            return Err(AllocError::AlreadyInitialized);
        }
        // 1. Union of the usable input (overlap and adjacency merged), clipped.
        let mut union = ReservationSet::<F>::new();
        for r in usable {
            if let Some(c) = r.clip(PAGE_SIZE, ceiling) {
                union
                    .add(c, ReserveKind::Explicit)
                    .map_err(|_| AllocError::TooManyRanges)?;
            }
        }
        // 2. Subtract reservations, trim inward to whole pages.
        let mut free = [PhysRange::UNUSED; F];
        let mut len = 0usize;
        let mut total = 0u64;
        for u in union.as_slice() {
            reservations.for_each_gap(&u.range(), |gap| {
                match gap.shrink_to_granule(PAGE_SIZE) {
                    Ok(Some(g)) => {
                        if len == F {
                            return Err(AllocError::TooManyRanges);
                        }
                        free[len] = g;
                        len += 1;
                        total = total.checked_add(g.len()).ok_or(AllocError::SizeOverflow)?;
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err(e) => Err(AllocError::Range(e)),
                }
            })?;
        }
        if len == 0 {
            return Err(AllocError::NoUsableMemory);
        }
        // 3. Commit.
        *self = Self {
            state: State::Active,
            free,
            free_len: len,
            boot: reservations,
            allocs: ReservationSet::new(),
            managed_bytes: total,
            allocated_bytes: 0,
            withdrawn_bytes: 0,
            transferred_bytes: 0,
        };
        Ok(())
    }

    fn require_active(&self) -> Result<(), AllocError> {
        match self.state {
            State::Active => Ok(()),
            State::Uninit => Err(AllocError::NotInitialized),
            State::Sealed => Err(AllocError::Sealed),
        }
    }

    /// Allocates `size` bytes (rounded up to whole pages) aligned to `align`
    /// (a power of two up to `MAX_ALIGN`; at least page alignment is always given).
    /// Returns the allocated range. Never returns reserved, overlapping or
    /// out-of-region memory. On `Err` nothing changed.
    pub fn alloc(&mut self, size: u64, align: u64) -> Result<PhysRange, AllocError> {
        self.require_active()?;
        if size == 0 {
            return Err(AllocError::ZeroSize);
        }
        if !range::is_valid_align(align) {
            return Err(AllocError::BadAlignment);
        }
        let size = range::align_up(size, PAGE_SIZE).map_err(|_| AllocError::SizeOverflow)?;
        let align = align.max(PAGE_SIZE);
        let mut meta_blocked = false;
        for i in 0..self.free_len {
            let r = self.free[i];
            let Ok(a) = range::align_up(r.start(), align) else {
                continue;
            };
            let Some(e) = a.checked_add(size) else {
                continue;
            };
            if e > r.end() {
                continue;
            }
            let Ok(taken) = PhysRange::new(a, e) else {
                continue;
            };
            let (left, right) = r.subtract(&taken);
            let needs_slot = left.is_some() && right.is_some();
            if (needs_slot && self.free_len == F) || !self.allocs.can_add(taken) {
                meta_blocked = true;
                continue;
            }
            // Capacity proven above: the log add cannot fail. Do it first so that
            // a (theoretical) failure still leaves the free list untouched.
            if self.allocs.add(taken, ReserveKind::Allocated).is_err() {
                return Err(AllocError::MetadataFull);
            }
            self.replace_free(i, left, right);
            self.allocated_bytes += size; // bounded by managed_bytes
            return Ok(taken);
        }
        Err(if meta_blocked {
            AllocError::MetadataFull
        } else {
            AllocError::OutOfMemory
        })
    }

    pub fn alloc_pages(&mut self, pages: u64) -> Result<PhysRange, AllocError> {
        let size = pages
            .checked_mul(PAGE_SIZE)
            .ok_or(AllocError::SizeOverflow)?;
        self.alloc(size, PAGE_SIZE)
    }

    /// Replaces free slot `i` by its surviving pieces. Caller guarantees room.
    fn replace_free(&mut self, i: usize, left: Option<PhysRange>, right: Option<PhysRange>) {
        match (left, right) {
            (Some(l), Some(r)) => {
                self.free.copy_within(i + 1..self.free_len, i + 2);
                self.free[i] = l;
                self.free[i + 1] = r;
                self.free_len += 1;
            }
            (Some(p), None) | (None, Some(p)) => self.free[i] = p,
            (None, None) => {
                self.free.copy_within(i + 1..self.free_len, i);
                self.free_len -= 1;
            }
        }
    }

    /// Protects `range` after init (for example allocator metadata or a late
    /// discovery). Every free page that overlaps it, even partly, is withdrawn.
    /// Memory already allocated is unaffected. All-or-nothing.
    pub fn reserve_late(&mut self, range: PhysRange, kind: ReserveKind) -> Result<(), AllocError> {
        self.require_active()?;
        let mut boot = self.boot.clone();
        boot.add(range, kind)?;
        let outward = PhysRange::new(
            range::align_down(range.start(), PAGE_SIZE)?,
            range::align_up(range.end(), PAGE_SIZE)?,
        )?;
        let mut free = [PhysRange::UNUSED; F];
        let mut len = 0usize;
        let mut removed = 0u64;
        for r in self.free_ranges() {
            if !r.overlaps(&outward) {
                free[len] = *r; // len <= index, so always in bounds
                len += 1;
                continue;
            }
            let (l, rt) = r.subtract(&outward);
            let kept = l.map_or(0, |p| p.len()) + rt.map_or(0, |p| p.len());
            removed += r.len() - kept;
            for p in [l, rt].into_iter().flatten() {
                if len == F {
                    return Err(AllocError::TooManyRanges);
                }
                free[len] = p;
                len += 1;
            }
        }
        self.boot = boot;
        self.free = free;
        self.free_len = len;
        self.withdrawn_bytes += removed;
        Ok(())
    }

    /// Marks the allocator Sealed after the free pages were accepted by their new
    /// owner. Only `phys_handoff::handoff` should call this.
    pub(crate) fn seal_after_transfer(&mut self) {
        if self.state != State::Active {
            return;
        }
        self.transferred_bytes = self.free_bytes();
        self.free = [PhysRange::UNUSED; F];
        self.free_len = 0;
        self.state = State::Sealed;
    }

    /// Test-only fault injection used by the handoff tests.
    #[cfg(test)]
    pub(crate) fn corrupt_free_for_test(&mut self, i: usize, r: PhysRange) {
        self.free[i] = r;
    }

    /// Full structural and accounting audit.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        if self.state == State::Uninit {
            return if self.free_len == 0 && self.managed_bytes == 0 {
                Ok(())
            } else {
                Err("uninitialized allocator holds state")
            };
        }
        self.boot.check_invariants()?;
        self.allocs.check_invariants()?;
        let mut prev_end: Option<u64> = None;
        for r in self.free_ranges() {
            if r.start() >= r.end() {
                return Err("empty or inverted free range");
            }
            if !range::is_aligned(r.start(), PAGE_SIZE) || !range::is_aligned(r.end(), PAGE_SIZE) {
                return Err("free range not page aligned");
            }
            if r.start() < PAGE_SIZE {
                return Err("free range includes physical page 0");
            }
            if let Some(pe) = prev_end {
                if r.start() <= pe {
                    return Err("free ranges unsorted, overlapping or adjacent");
                }
            }
            prev_end = Some(r.end());
            if self.boot.overlaps(r) {
                return Err("free range overlaps a reservation");
            }
            if self.allocs.overlaps(r) {
                return Err("free range overlaps a live allocation");
            }
        }
        if self.allocs.total_bytes() != self.allocated_bytes {
            return Err("allocation log disagrees with allocated byte count");
        }
        let sum = self
            .free_bytes()
            .checked_add(self.allocated_bytes)
            .and_then(|v| v.checked_add(self.withdrawn_bytes))
            .and_then(|v| v.checked_add(self.transferred_bytes))
            .ok_or("accounting overflow")?;
        if sum != self.managed_bytes {
            return Err("byte conservation violated");
        }
        if self.state == State::Sealed && self.free_len != 0 {
            return Err("sealed allocator still holds free pages");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type A = EarlyAllocator<8, 8, 16>;

    fn r(s: u64, e: u64) -> PhysRange {
        PhysRange::new(s, e).unwrap()
    }
    fn resv(items: &[(u64, u64)]) -> ReservationSet<8> {
        let mut s = ReservationSet::new();
        for &(a, b) in items {
            s.add(r(a, b), ReserveKind::Explicit).unwrap();
        }
        s
    }
    fn mk(usable: &[(u64, u64)], reserved: &[(u64, u64)], ceiling: u64) -> A {
        let mut a = A::new();
        a.init(
            usable.iter().map(|&(s, e)| r(s, e)),
            resv(reserved),
            ceiling,
        )
        .unwrap();
        a.check_invariants().unwrap();
        a
    }
    fn free(a: &A) -> Vec<(u64, u64)> {
        a.free_ranges()
            .iter()
            .map(|x| (x.start(), x.end()))
            .collect()
    }

    #[test]
    fn uninit_allocator_refuses_to_allocate() {
        let mut a = A::new();
        assert_eq!(a.alloc(4096, 4096), Err(AllocError::NotInitialized));
        assert_eq!(a.state(), State::Uninit);
        a.check_invariants().unwrap();
    }

    #[test]
    fn init_excludes_reservations_page_zero_and_ceiling() {
        let a = mk(&[(0, 0x10_0000)], &[(0x4000, 0x6000)], 0x8_0000);
        assert_eq!(free(&a), [(0x1000, 0x4000), (0x6000, 0x8_0000)]);
        assert_eq!(a.managed_bytes(), 0x3000 + 0x7_a000);
    }

    #[test]
    fn partly_reserved_and_partly_usable_pages_are_never_managed() {
        // Usable [0x1800, 0x5800); reservation [0x3010, 0x3020) inside page 0x3000.
        let a = mk(&[(0x1800, 0x5800)], &[(0x3010, 0x3020)], u64::MAX);
        assert_eq!(free(&a), [(0x2000, 0x3000), (0x4000, 0x5000)]);
    }

    #[test]
    fn unsorted_overlapping_and_adjacent_usable_input_is_unioned() {
        let a = mk(
            &[
                (0x8000, 0xa000),
                (0x1000, 0x3000),
                (0x2000, 0x4000),
                (0xa000, 0xb000),
            ],
            &[],
            u64::MAX,
        );
        assert_eq!(free(&a), [(0x1000, 0x4000), (0x8000, 0xb000)]);
    }

    #[test]
    fn sub_page_usable_fragments_vanish() {
        let mut a = A::new();
        let err = a
            .init([r(0x1000, 0x1fff)], ReservationSet::new(), u64::MAX)
            .unwrap_err();
        assert_eq!(err, AllocError::NoUsableMemory);
        assert_eq!(a.state(), State::Uninit);
    }

    #[test]
    fn fully_reserved_or_empty_input_fails_and_stays_uninit() {
        let mut a = A::new();
        assert_eq!(
            a.init([r(0x1000, 0x5000)], resv(&[(0, 0x6000)]), u64::MAX),
            Err(AllocError::NoUsableMemory)
        );
        assert_eq!(
            a.init(core::iter::empty(), ReservationSet::new(), u64::MAX),
            Err(AllocError::NoUsableMemory)
        );
        assert_eq!(
            a.init([r(0, 0x1000)], ReservationSet::new(), u64::MAX),
            Err(AllocError::NoUsableMemory),
            "page 0 only"
        );
        assert_eq!(a.state(), State::Uninit);
        a.check_invariants().unwrap();
    }

    #[test]
    fn too_many_ranges_is_reported_and_leaves_state_unchanged() {
        let mut a = EarlyAllocator::<2, 8, 8>::new();
        let usable = [r(0x1000, 0x2000), r(0x3000, 0x4000), r(0x5000, 0x6000)];
        assert_eq!(
            a.init(usable, ReservationSet::new(), u64::MAX),
            Err(AllocError::TooManyRanges)
        );
        assert_eq!(a.state(), State::Uninit);
        // Splitting by a reservation can also overflow the free list.
        let mut b = EarlyAllocator::<2, 8, 8>::new();
        let res = resv(&[(0x2000, 0x3000), (0x4000, 0x5000)]);
        assert_eq!(
            b.init([r(0x1000, 0x8000)], res, u64::MAX),
            Err(AllocError::TooManyRanges)
        );
        assert_eq!(b.state(), State::Uninit);
    }

    #[test]
    fn allocation_is_lowest_address_first_fit() {
        let mut a = mk(&[(0x1000, 0x3000), (0x10_0000, 0x20_0000)], &[], u64::MAX);
        assert_eq!(a.alloc(4096, 1).unwrap(), r(0x1000, 0x2000));
        assert_eq!(a.alloc(4096, 1).unwrap(), r(0x2000, 0x3000));
        assert_eq!(a.alloc(4096, 1).unwrap(), r(0x10_0000, 0x10_1000));
        a.check_invariants().unwrap();
    }

    #[test]
    fn sizes_round_up_to_pages_and_alignment_is_honoured() {
        let mut a = mk(&[(0x1000, 0x40_0000)], &[], u64::MAX);
        let x = a.alloc(1, 1).unwrap();
        assert_eq!(x.len(), 4096);
        let y = a.alloc(5000, 1).unwrap();
        assert_eq!(y.len(), 8192);
        let z = a.alloc(4096, 0x10_0000).unwrap();
        assert_eq!(z.start() % 0x10_0000, 0);
        let w = a.alloc(4096, 0x20_0000).unwrap();
        assert_eq!(w.start(), 0x20_0000);
        a.check_invariants().unwrap();
        // The gap skipped for alignment stays allocatable.
        let g = a.alloc(4096, 1).unwrap();
        assert!(g.start() < 0x10_0000);
    }

    #[test]
    fn invalid_requests_are_rejected_without_change() {
        let mut a = mk(&[(0x1000, 0x10_0000)], &[], u64::MAX);
        let snap = a.clone();
        assert_eq!(a.alloc(0, 4096), Err(AllocError::ZeroSize));
        for bad in [0u64, 3, 6, 4097, (1 << 30) + 1, 1 << 31, u64::MAX] {
            assert_eq!(
                a.alloc(4096, bad),
                Err(AllocError::BadAlignment),
                "align {bad}"
            );
        }
        assert_eq!(a.alloc(u64::MAX, 4096), Err(AllocError::SizeOverflow));
        assert_eq!(
            a.alloc(u64::MAX - 4094, 4096),
            Err(AllocError::SizeOverflow)
        );
        assert_eq!(a.alloc_pages(u64::MAX), Err(AllocError::SizeOverflow));
        assert_eq!(a.alloc_pages(0), Err(AllocError::ZeroSize));
        assert_eq!(free(&a), free(&snap));
        assert_eq!(a.allocated_bytes(), 0);
        a.check_invariants().unwrap();
    }

    #[test]
    fn exhaustion_fails_cleanly_and_is_repeatable() {
        let mut a = mk(&[(0x1000, 0x4000)], &[], u64::MAX);
        a.alloc(0x3000, 1).unwrap();
        let snap = a.clone();
        for _ in 0..3 {
            assert_eq!(a.alloc(4096, 1), Err(AllocError::OutOfMemory));
        }
        assert_eq!(free(&a), free(&snap));
        assert_eq!(a.allocated_bytes(), 0x3000);
        a.check_invariants().unwrap();
    }

    #[test]
    fn a_request_larger_than_every_free_range_fails_without_change() {
        let mut a = mk(&[(0x1000, 0x3000), (0x5000, 0x7000)], &[], u64::MAX);
        let before = free(&a);
        assert_eq!(a.alloc(0x3000, 1), Err(AllocError::OutOfMemory));
        assert_eq!(free(&a), before);
        // The sum of free memory is irrelevant: ranges are not contiguous.
        assert_eq!(a.free_bytes(), 0x4000);
    }

    #[test]
    fn alignment_failure_due_to_range_shape_is_out_of_memory() {
        let mut a = mk(&[(0x1000, 0x3000)], &[], u64::MAX);
        assert_eq!(a.alloc(4096, 0x10_0000), Err(AllocError::OutOfMemory));
        a.check_invariants().unwrap();
    }

    #[test]
    fn metadata_exhaustion_is_reported_and_changes_nothing() {
        // One free slot: allocating from the middle would need a second one.
        let mut a = EarlyAllocator::<1, 4, 4>::new();
        a.init([r(0x1000, 0x10_0000)], ReservationSet::new(), u64::MAX)
            .unwrap();
        let snap = (a.free_ranges().to_vec(), a.allocated_bytes());
        // Alignment forces the allocation off the start of the range: a split.
        assert_eq!(a.alloc(4096, 0x2_0000), Err(AllocError::MetadataFull));
        assert_eq!((a.free_ranges().to_vec(), a.allocated_bytes()), snap);
        a.check_invariants().unwrap();
        // The start-of-range allocation needs no new slot and still works.
        assert!(a.alloc(4096, 1).is_ok());
    }

    #[test]
    fn allocation_log_capacity_is_checked_before_mutation() {
        let mut a = EarlyAllocator::<8, 4, 2>::new();
        a.init([r(0x1000, 0x10_0000)], ReservationSet::new(), u64::MAX)
            .unwrap();
        // Create two NON-adjacent allocations, filling the 2-slot log.
        a.alloc(4096, 1).unwrap(); // 0x1000
        a.alloc(4096, 0x4000).unwrap(); // 0x4000 (skips 0x2000..0x4000)
        let snap = (free_of(&a), a.allocated_bytes());
        // A third, non-adjacent to both, cannot be recorded.
        assert_eq!(a.alloc(4096, 0x8000), Err(AllocError::MetadataFull));
        assert_eq!((free_of(&a), a.allocated_bytes()), snap);
        a.check_invariants().unwrap();
        // One that merges into an existing log entry is fine.
        assert_eq!(a.alloc(4096, 1).unwrap().start(), 0x2000);
        a.check_invariants().unwrap();
    }
    fn free_of<const F: usize, const R: usize, const Q: usize>(
        a: &EarlyAllocator<F, R, Q>,
    ) -> Vec<(u64, u64)> {
        a.free_ranges()
            .iter()
            .map(|x| (x.start(), x.end()))
            .collect()
    }

    #[test]
    fn reinit_is_rejected_and_preserves_live_state() {
        let mut a = mk(&[(0x1000, 0x10_0000)], &[(0x8000, 0x9000)], u64::MAX);
        let got = a.alloc(0x2000, 1).unwrap();
        let before = (free(&a), a.allocated_bytes(), a.reservations().len());
        assert_eq!(
            a.init([r(0x1000, 0x20_0000)], ReservationSet::new(), u64::MAX),
            Err(AllocError::AlreadyInitialized)
        );
        assert_eq!(
            (free(&a), a.allocated_bytes(), a.reservations().len()),
            before
        );
        assert!(
            a.allocations().covers(&got),
            "live allocation still recorded"
        );
        assert!(a.reservations().covers(&r(0x8000, 0x9000)));
        a.check_invariants().unwrap();
    }

    #[test]
    fn reserve_late_withdraws_overlapping_pages_only() {
        let mut a = mk(&[(0x1000, 0x10_000)], &[], u64::MAX);
        let first = a.alloc(0x1000, 1).unwrap(); // 0x1000
        a.reserve_late(r(0x5010, 0x5020), ReserveKind::AllocatorMeta)
            .unwrap();
        assert!(
            !free(&a).iter().any(|&(s, e)| s < 0x6000 && e > 0x5000),
            "whole page withdrawn"
        );
        assert_eq!(a.withdrawn_bytes(), 0x1000);
        assert!(a.allocations().covers(&first));
        a.check_invariants().unwrap();
        // Reserving something already allocated or already reserved is harmless.
        a.reserve_late(r(0x1000, 0x2000), ReserveKind::Explicit)
            .unwrap();
        a.reserve_late(r(0x5010, 0x5020), ReserveKind::Explicit)
            .unwrap();
        assert_eq!(a.withdrawn_bytes(), 0x1000);
        a.check_invariants().unwrap();
    }

    #[test]
    fn reserve_late_failure_changes_nothing() {
        let mut a = EarlyAllocator::<2, 1, 4>::new();
        a.init([r(0x1000, 0x10_000)], ReservationSet::new(), u64::MAX)
            .unwrap();
        a.reserve_late(r(0x2000, 0x3000), ReserveKind::Explicit)
            .unwrap(); // free: 2 slots, boot: 1
        let snap = (free_of(&a), a.withdrawn_bytes());
        // Boot-table capacity (R = 1): a non-adjacent reservation does not fit.
        assert_eq!(
            a.reserve_late(r(0x8000, 0x9000), ReserveKind::Explicit),
            Err(AllocError::Reserve(ReserveError::CapacityExceeded))
        );
        assert_eq!((free_of(&a), a.withdrawn_bytes()), snap);
        a.check_invariants().unwrap();
        // Free-list capacity (F = 2): splitting a third range does not fit.
        let mut b = EarlyAllocator::<2, 4, 4>::new();
        b.init([r(0x1000, 0x10_000)], ReservationSet::new(), u64::MAX)
            .unwrap();
        b.reserve_late(r(0x2000, 0x3000), ReserveKind::Explicit)
            .unwrap();
        let snap = (free_of(&b), b.withdrawn_bytes());
        assert_eq!(
            b.reserve_late(r(0x8000, 0x9000), ReserveKind::Explicit),
            Err(AllocError::TooManyRanges)
        );
        assert_eq!((free_of(&b), b.withdrawn_bytes()), snap);
        b.check_invariants().unwrap();
    }

    #[test]
    fn sealing_transfers_everything_and_blocks_further_use() {
        let mut a = mk(&[(0x1000, 0x10_000)], &[], u64::MAX);
        a.alloc(0x2000, 1).unwrap();
        let remaining = a.free_bytes();
        a.seal_after_transfer();
        assert_eq!(a.state(), State::Sealed);
        assert_eq!(a.transferred_bytes(), remaining);
        assert_eq!(a.free_bytes(), 0);
        assert_eq!(a.alloc(4096, 1), Err(AllocError::Sealed));
        assert_eq!(
            a.init([r(0x1000, 0x2000)], ReservationSet::new(), u64::MAX),
            Err(AllocError::AlreadyInitialized)
        );
        assert_eq!(
            a.reserve_late(r(0x1000, 0x2000), ReserveKind::Explicit),
            Err(AllocError::Sealed)
        );
        a.check_invariants().unwrap();
    }

    #[test]
    fn audit_detects_corruption() {
        let mut a = mk(&[(0x1000, 0x10_000)], &[(0x4000, 0x5000)], u64::MAX);
        a.alloc(0x1000, 1).unwrap();
        let good = a.clone();
        let mut bad = good.clone();
        bad.free[0] = r(0x1000, 0x2000); // overlaps the live allocation
        assert!(bad.check_invariants().is_err());
        let mut bad = good.clone();
        bad.managed_bytes += 1;
        assert_eq!(bad.check_invariants(), Err("byte conservation violated"));
        let mut bad = good.clone();
        bad.free[1] = r(0x4000, 0x5000); // inside a reservation
        assert!(bad.check_invariants().is_err());
        let mut bad = good.clone();
        bad.free[0] = r(0x2001, 0x4000); // not page aligned
        assert_eq!(bad.check_invariants(), Err("free range not page aligned"));
    }

    /// Deterministic randomized sequences with a page-level oracle.
    #[test]
    fn random_sequences_never_overlap_or_leave_the_regions() {
        let mut x = 0xdead_beef_cafe_f00du64;
        let mut next = move || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        };
        const PAGES: u64 = 1024;
        for round in 0..60 {
            // Random usable windows and reservations inside a 1024-page world.
            let mut usable = Vec::new();
            let mut reserved = ReservationSet::<8>::new();
            for _ in 0..(next() % 4 + 1) {
                let s = next() % (PAGES - 8) * 4096 + (next() % 4096);
                let l = (next() % 300 + 1) * 4096;
                usable.push(r(s, s + l));
            }
            for _ in 0..(next() % 5) {
                let s = next() % (PAGES - 4) * 4096 + (next() % 4096);
                let l = next() % 20000 + 1;
                let _ = reserved.add(r(s, s + l), ReserveKind::Explicit);
            }
            let ceiling = if round % 3 == 0 {
                (PAGES - 100) * 4096
            } else {
                PAGES * 4096 + 0x10_0000
            };
            let mut al = EarlyAllocator::<16, 8, 32>::new();
            if al
                .init(usable.iter().copied(), reserved.clone(), ceiling)
                .is_err()
            {
                assert_eq!(al.state(), State::Uninit);
                continue;
            }
            // Oracle: which pages may ever be handed out.
            let eligible = |p: u64| {
                let (s, e) = (p * 4096, p * 4096 + 4096);
                p != 0
                    && e <= ceiling
                    && usable.iter().any(|u| u.start() <= s && e <= u.end())
                    && !reserved.overlaps(&r(s, e))
            };
            let mut taken = vec![false; (PAGES + 600) as usize];
            for _ in 0..300 {
                let before = al.clone();
                let size = if next() % 8 == 0 {
                    next() % 200_000
                } else {
                    next() % 20_000
                };
                let align = 1u64 << (next() % 18);
                match al.alloc(size, align) {
                    Ok(got) => {
                        assert!(size > 0);
                        assert!(
                            got.len() >= size && got.len() % 4096 == 0 && got.len() < size + 4096
                        );
                        assert_eq!(got.start() % align.max(4096), 0);
                        for p in got.start() / 4096..got.end() / 4096 {
                            assert!(eligible(p), "round {round}: page {p} not eligible");
                            assert!(
                                !taken[p as usize],
                                "round {round}: page {p} handed out twice"
                            );
                            taken[p as usize] = true;
                        }
                    }
                    Err(_) => {
                        assert_eq!(
                            al.free_ranges(),
                            before.free_ranges(),
                            "failure changed the free list"
                        );
                        assert_eq!(al.allocated_bytes(), before.allocated_bytes());
                        assert_eq!(al.allocations().as_slice(), before.allocations().as_slice());
                    }
                }
                al.check_invariants().unwrap();
            }
            let n_taken = taken.iter().filter(|&&t| t).count() as u64;
            assert_eq!(al.allocated_bytes(), n_taken * 4096);
            let n_free: u64 = al.free_ranges().iter().map(|f| f.len() / 4096).sum();
            let n_eligible = (0..PAGES + 600).filter(|&p| eligible(p)).count() as u64;
            assert_eq!(
                n_free + n_taken,
                n_eligible,
                "round {round}: pages lost or invented"
            );
        }
    }
}
