//! Mission B handoff (Mission A2, file 5 of 5).
//! SPDX-License-Identifier: Apache-2.0
//!
//! OWNERSHIP AND LIFECYCLE
//!   * While the allocator is `Active`, the early allocator owns every free page.
//!   * `handoff` is the ONE place ownership moves. It first audits the allocator,
//!     then re-validates every free range, then gives the complete list to the
//!     receiver in a single call. Only if the receiver accepts is the allocator
//!     sealed. A refusal leaves the allocator Active and unchanged, so memory is
//!     never owned by both sides and never by neither.
//!   * Pages that are reserved (kernel image, boot info, map storage, firmware,
//!     bootloader, allocator metadata, late reservations) or already allocated are
//!     NEVER offered: they were removed from the free list before this point and
//!     the audit proves the free list does not overlap either set.
//!
//! WHAT THIS FILE IS NOT
//!   Mission B is not present in this repository, so none of its interfaces could be
//!   inspected. This module therefore does not implement or imitate a frame
//!   allocator. It defines the smallest receiving interface (`FrameSink`) that a
//!   Mission B allocator needs to implement, or that a thin adapter must wrap once
//!   its real shape is known. Anything beyond "here are page-aligned, validated,
//!   free physical ranges" is Mission B's contract to define.
#![allow(dead_code)]
use crate::early_alloc::{EarlyAllocator, State};
use crate::range::{self, PAGE_SIZE, PhysRange};

/// A receiver's refusal. The code is the receiver's own; it is passed through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkError(pub u32);

/// The receiving side of the handoff.
pub trait FrameSink {
    /// Takes ownership of ALL `ranges` (ascending, disjoint, page-aligned, never
    /// containing page 0) or of none. Returning `Err` means nothing was taken.
    fn accept_all(&mut self, ranges: &[PhysRange]) -> Result<(), SinkError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffError {
    /// The allocator is not `Active` (never initialized, or already handed off).
    NotActive(State),
    /// `check_invariants` failed; nothing was offered.
    Audit(&'static str),
    /// Free range number `index` failed re-validation; nothing was offered.
    InvalidRange { index: usize, why: &'static str },
    /// The receiver refused; the allocator is still Active and unchanged.
    Sink(SinkError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffReport {
    pub ranges: usize,
    pub bytes: u64,
    /// Bytes the early allocator had already given out (not transferred).
    pub allocated_bytes: u64,
    /// Bytes withdrawn from the free list by late reservations.
    pub withdrawn_bytes: u64,
}

/// Checks one free range independently of the allocator's own bookkeeping.
fn validate_free_range<const R: usize, const A: usize>(
    r: &PhysRange,
    boot: &crate::reserve::ReservationSet<R>,
    allocs: &crate::reserve::ReservationSet<A>,
) -> Result<(), &'static str> {
    if r.start() >= r.end() {
        return Err("empty or inverted");
    }
    if !range::is_aligned(r.start(), PAGE_SIZE) || !range::is_aligned(r.end(), PAGE_SIZE) {
        return Err("not page aligned");
    }
    if r.start() < PAGE_SIZE {
        return Err("includes physical page 0");
    }
    if boot.overlaps(r) {
        return Err("overlaps a reservation");
    }
    if allocs.overlaps(r) {
        return Err("overlaps a live allocation");
    }
    Ok(())
}

/// Hands every remaining free page to `sink`. See the module docs.
pub fn handoff<const F: usize, const R: usize, const A: usize, S: FrameSink>(
    alloc: &mut EarlyAllocator<F, R, A>,
    sink: &mut S,
) -> Result<HandoffReport, HandoffError> {
    if alloc.state() != State::Active {
        return Err(HandoffError::NotActive(alloc.state()));
    }
    alloc.check_invariants().map_err(HandoffError::Audit)?;
    let mut prev_end = 0u64;
    let mut bytes = 0u64;
    for (index, r) in alloc.free_ranges().iter().enumerate() {
        validate_free_range(r, alloc.reservations(), alloc.allocations())
            .map_err(|why| HandoffError::InvalidRange { index, why })?;
        if index > 0 && r.start() <= prev_end {
            return Err(HandoffError::InvalidRange {
                index,
                why: "unsorted or touching",
            });
        }
        prev_end = r.end();
        bytes = bytes
            .checked_add(r.len())
            .ok_or(HandoffError::InvalidRange {
                index,
                why: "byte count overflow",
            })?;
    }
    let report = HandoffReport {
        ranges: alloc.free_ranges().len(),
        bytes,
        allocated_bytes: alloc.allocated_bytes(),
        withdrawn_bytes: alloc.withdrawn_bytes(),
    };
    if report.ranges > 0 {
        sink.accept_all(alloc.free_ranges())
            .map_err(HandoffError::Sink)?;
    }
    alloc.seal_after_transfer();
    Ok(report)
}

/// Fixed-capacity receiver that only records what it is given. It is a reference
/// implementation of the trait for tests and bring-up logging, NOT a frame
/// allocator.
#[derive(Debug, Clone)]
pub struct RecordingSink<const N: usize> {
    ranges: [PhysRange; N],
    len: usize,
}

impl<const N: usize> RecordingSink<N> {
    pub const fn new() -> Self {
        Self {
            ranges: [PhysRange::UNUSED; N],
            len: 0,
        }
    }
    pub fn ranges(&self) -> &[PhysRange] {
        &self.ranges[..self.len]
    }
}

impl<const N: usize> Default for RecordingSink<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> FrameSink for RecordingSink<N> {
    fn accept_all(&mut self, ranges: &[PhysRange]) -> Result<(), SinkError> {
        if self.len != 0 || ranges.len() > N {
            return Err(SinkError(1)); // full or already used: take nothing
        }
        self.ranges[..ranges.len()].copy_from_slice(ranges);
        self.len = ranges.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reserve::{ReservationSet, ReserveKind};

    type Al = EarlyAllocator<8, 8, 16>;
    fn r(s: u64, e: u64) -> PhysRange {
        PhysRange::new(s, e).unwrap()
    }
    fn active() -> Al {
        let mut res = ReservationSet::<8>::new();
        res.add(r(0x8000, 0x9000), ReserveKind::KernelImage)
            .unwrap();
        let mut a = Al::new();
        a.init([r(0, 0x20_000)], res, u64::MAX).unwrap();
        a
    }

    struct Refuse;
    impl FrameSink for Refuse {
        fn accept_all(&mut self, _: &[PhysRange]) -> Result<(), SinkError> {
            Err(SinkError(7))
        }
    }

    #[test]
    fn hands_over_only_free_pages_never_reserved_or_allocated() {
        let mut a = active();
        let got = a.alloc(0x2000, 1).unwrap();
        let mut sink = RecordingSink::<8>::new();
        let rep = handoff(&mut a, &mut sink).unwrap();
        assert_eq!(rep.allocated_bytes, 0x2000);
        for g in sink.ranges() {
            assert!(!g.overlaps(&got), "allocated memory was offered");
            assert!(
                !g.overlaps(&r(0x8000, 0x9000)),
                "reserved memory was offered"
            );
            assert!(g.start() >= PAGE_SIZE);
        }
        assert_eq!(
            sink.ranges().iter().map(|g| g.len()).sum::<u64>(),
            rep.bytes
        );
        assert_eq!(
            rep.bytes,
            0x20_000 - 0x1000 - 0x1000 - 0x2000,
            "page 0, reservation, allocation"
        );
        assert_eq!(a.state(), State::Sealed);
        a.check_invariants().unwrap();
    }

    #[test]
    fn handoff_is_one_shot() {
        let mut a = active();
        let mut s = RecordingSink::<8>::new();
        handoff(&mut a, &mut s).unwrap();
        let mut s2 = RecordingSink::<8>::new();
        assert_eq!(
            handoff(&mut a, &mut s2),
            Err(HandoffError::NotActive(State::Sealed))
        );
        assert!(s2.ranges().is_empty(), "no page may be offered twice");
        assert_eq!(
            a.alloc(4096, 1),
            Err(crate::early_alloc::AllocError::Sealed)
        );
    }

    #[test]
    fn uninitialized_allocator_cannot_be_handed_off() {
        let mut a = Al::new();
        let mut s = RecordingSink::<8>::new();
        assert_eq!(
            handoff(&mut a, &mut s),
            Err(HandoffError::NotActive(State::Uninit))
        );
    }

    #[test]
    fn a_refusing_receiver_leaves_the_allocator_active_and_unchanged() {
        let mut a = active();
        a.alloc(0x1000, 1).unwrap();
        let before = (a.free_ranges().to_vec(), a.allocated_bytes(), a.state());
        assert_eq!(
            handoff(&mut a, &mut Refuse),
            Err(HandoffError::Sink(SinkError(7)))
        );
        assert_eq!(
            (a.free_ranges().to_vec(), a.allocated_bytes(), a.state()),
            before
        );
        a.check_invariants().unwrap();
        // The memory is still ours and still allocatable.
        assert!(a.alloc(0x1000, 1).is_ok());
        // ... and a later, willing receiver gets it.
        let mut s = RecordingSink::<8>::new();
        handoff(&mut a, &mut s).unwrap();
    }

    #[test]
    fn a_too_small_receiver_is_a_refusal_not_a_partial_transfer() {
        let mut a = active(); // free: [0x1000,0x8000) and [0x9000,0x20000): 2 ranges
        let mut s = RecordingSink::<1>::new();
        assert_eq!(
            handoff(&mut a, &mut s),
            Err(HandoffError::Sink(SinkError(1)))
        );
        assert!(s.ranges().is_empty());
        assert_eq!(a.state(), State::Active);
    }

    #[test]
    fn corrupted_allocator_is_not_handed_off() {
        let mut a = active();
        a.alloc(0x1000, 1).unwrap();
        let mut bad = a.clone();
        bad.corrupt_free_for_test(0, r(0x1000, 0x2000)); // overlaps the allocation
        let mut s = RecordingSink::<8>::new();
        assert!(matches!(
            handoff(&mut bad, &mut s),
            Err(HandoffError::Audit(_))
        ));
        assert!(s.ranges().is_empty());
        assert_eq!(bad.state(), State::Active);
    }

    #[test]
    fn exhausted_allocator_hands_over_nothing_but_still_seals() {
        let mut a = Al::new();
        a.init([r(0x1000, 0x3000)], ReservationSet::new(), u64::MAX)
            .unwrap();
        a.alloc(0x2000, 1).unwrap();
        let mut s = RecordingSink::<1>::new();
        let rep = handoff(&mut a, &mut s).unwrap();
        assert_eq!((rep.ranges, rep.bytes), (0, 0));
        assert_eq!(a.state(), State::Sealed);
    }

    #[test]
    fn late_reservations_are_not_offered() {
        let mut a = active();
        a.reserve_late(r(0x10_010, 0x10_020), ReserveKind::AllocatorMeta)
            .unwrap();
        let rep_before = a.withdrawn_bytes();
        let mut s = RecordingSink::<8>::new();
        let rep = handoff(&mut a, &mut s).unwrap();
        assert_eq!(rep.withdrawn_bytes, rep_before);
        assert!(
            s.ranges()
                .iter()
                .all(|g| !g.overlaps(&r(0x10_000, 0x11_000)))
        );
    }

    #[test]
    fn independent_validation_rejects_bad_ranges() {
        let boot = ReservationSet::<4>::new();
        let allocs = ReservationSet::<4>::new();
        assert!(validate_free_range(&r(0x1000, 0x2000), &boot, &allocs).is_ok());
        assert_eq!(
            validate_free_range(&r(0x1001, 0x2000), &boot, &allocs),
            Err("not page aligned")
        );
        assert_eq!(
            validate_free_range(&r(0x1000, 0x2001), &boot, &allocs),
            Err("not page aligned")
        );
        assert_eq!(
            validate_free_range(&r(0, 0x1000), &boot, &allocs),
            Err("includes physical page 0")
        );
        let mut b = ReservationSet::<4>::new();
        b.add(r(0x1800, 0x1900), ReserveKind::Explicit).unwrap();
        assert_eq!(
            validate_free_range(&r(0x1000, 0x2000), &b, &allocs),
            Err("overlaps a reservation")
        );
        let mut al = ReservationSet::<4>::new();
        al.add(r(0x1000, 0x1100), ReserveKind::Allocated).unwrap();
        assert_eq!(
            validate_free_range(&r(0x1000, 0x2000), &boot, &al),
            Err("overlaps a live allocation")
        );
    }
}
