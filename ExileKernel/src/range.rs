//! Checked physical-address range arithmetic (Mission A2, file 4 of 5).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Everything the early allocator, the reservation set and the Mission B handoff
//! need to reason about address intervals lives here, so that no other module does
//! raw address arithmetic. Rules:
//!   * a `PhysRange` is a half-open interval `[start, end)` that is NEVER empty and
//!     never inverted (the constructors enforce `start < end`);
//!   * every addition, rounding and length computation is checked; an overflow is an
//!     `Err`, never a wrapped value;
//!   * alignments are powers of two no larger than `MAX_ALIGN`.
//! No allocation, no `unsafe`, no `std`.
#![allow(dead_code)]

/// Size of a physical page / frame.
pub const PAGE_SIZE: u64 = 4096;
/// Largest alignment accepted anywhere in the early allocator (1 GiB: the size of the
/// bootstrap identity map).
pub const MAX_ALIGN: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeError {
    /// `start == end`: a range must contain at least one byte.
    Empty,
    /// `start > end`.
    Inverted,
    /// An address or length computation exceeded `u64`.
    Overflow,
    /// Alignment is zero, not a power of two, or larger than `MAX_ALIGN`.
    BadAlignment,
}

/// Is `align` a legal alignment (power of two, `1..=MAX_ALIGN`)?
pub const fn is_valid_align(align: u64) -> bool {
    align.is_power_of_two() && align <= MAX_ALIGN
}

/// Rounds `addr` up to a multiple of `align`.
pub fn align_up(addr: u64, align: u64) -> Result<u64, RangeError> {
    if !is_valid_align(align) {
        return Err(RangeError::BadAlignment);
    }
    let mask = align - 1;
    Ok(addr.checked_add(mask).ok_or(RangeError::Overflow)? & !mask)
}

/// Rounds `addr` down to a multiple of `align`.
pub fn align_down(addr: u64, align: u64) -> Result<u64, RangeError> {
    if !is_valid_align(align) {
        return Err(RangeError::BadAlignment);
    }
    Ok(addr & !(align - 1))
}

/// True when `addr` is a multiple of a LEGAL `align`; an illegal `align` is never
/// "aligned".
pub const fn is_aligned(addr: u64, align: u64) -> bool {
    is_valid_align(align) && addr & (align - 1) == 0
}

/// A non-empty half-open physical address range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysRange {
    start: u64,
    end: u64,
}

impl PhysRange {
    /// Placeholder for unused array slots. It violates the non-empty invariant ON
    /// PURPOSE so it can never be mistaken for data: containers track their own
    /// length and never read a slot beyond it.
    pub(crate) const UNUSED: PhysRange = PhysRange { start: 0, end: 0 };

    pub fn new(start: u64, end: u64) -> Result<Self, RangeError> {
        match start.cmp(&end) {
            core::cmp::Ordering::Less => Ok(Self { start, end }),
            core::cmp::Ordering::Equal => Err(RangeError::Empty),
            core::cmp::Ordering::Greater => Err(RangeError::Inverted),
        }
    }

    /// `[start, start + len)`; `len == 0` is `Empty`, a wrapping end is `Overflow`.
    pub fn from_len(start: u64, len: u64) -> Result<Self, RangeError> {
        if len == 0 {
            return Err(RangeError::Empty);
        }
        let end = start.checked_add(len).ok_or(RangeError::Overflow)?;
        Ok(Self { start, end })
    }

    pub const fn start(&self) -> u64 {
        self.start
    }

    /// Exclusive end.
    pub const fn end(&self) -> u64 {
        self.end
    }

    /// Length in bytes (always >= 1 for a valid range).
    pub const fn len(&self) -> u64 {
        self.end - self.start
    }

    pub const fn contains_addr(&self, addr: u64) -> bool {
        self.start <= addr && addr < self.end
    }

    /// Is `other` entirely inside `self`?
    pub const fn contains(&self, other: &PhysRange) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    /// Do the ranges share at least one byte?
    pub const fn overlaps(&self, other: &PhysRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Do the ranges overlap OR sit directly next to each other (no gap)?
    pub const fn touches(&self, other: &PhysRange) -> bool {
        self.start <= other.end && other.start <= self.end
    }

    pub fn intersection(&self, other: &PhysRange) -> Option<PhysRange> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start < end).then_some(PhysRange { start, end })
    }

    /// The smallest range covering both, only when they touch.
    pub fn union_if_touching(&self, other: &PhysRange) -> Option<PhysRange> {
        self.touches(other).then(|| PhysRange {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        })
    }

    /// `self` minus `hole`: the piece below the hole and the piece above it. Either
    /// may be absent. If the hole does not overlap `self` the result is `(Some(self),
    /// None)`; if it covers `self` the result is `(None, None)`.
    pub fn subtract(&self, hole: &PhysRange) -> (Option<PhysRange>, Option<PhysRange>) {
        if !self.overlaps(hole) {
            return (Some(*self), None);
        }
        let low = (self.start < hole.start).then_some(PhysRange {
            start: self.start,
            end: hole.start,
        });
        let high = (hole.end < self.end).then_some(PhysRange {
            start: hole.end,
            end: self.end,
        });
        (low, high)
    }

    /// Keeps only the part inside `[lo, hi)`.
    pub fn clip(&self, lo: u64, hi: u64) -> Option<PhysRange> {
        let start = self.start.max(lo);
        let end = self.end.min(hi);
        (start < end).then_some(PhysRange { start, end })
    }

    /// Shrinks the range INWARD to whole multiples of `granule` (start rounded up,
    /// end rounded down). `Ok(None)` means no whole granule fits.
    pub fn shrink_to_granule(&self, granule: u64) -> Result<Option<PhysRange>, RangeError> {
        if !is_valid_align(granule) {
            return Err(RangeError::BadAlignment);
        }
        // Rounding the start up can overflow only for a range that ends within one
        // granule of u64::MAX; such a range cannot contain a whole granule at its
        // top, so it shrinks to nothing rather than failing.
        let start = match align_up(self.start, granule) {
            Ok(s) => s,
            Err(RangeError::Overflow) => return Ok(None),
            Err(e) => return Err(e),
        };
        let end = align_down(self.end, granule)?;
        Ok((start < end).then_some(PhysRange { start, end }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: u64, e: u64) -> PhysRange {
        PhysRange::new(s, e).unwrap()
    }

    #[test]
    fn constructors_enforce_non_empty_and_ordered() {
        assert_eq!(PhysRange::new(5, 5), Err(RangeError::Empty));
        assert_eq!(PhysRange::new(6, 5), Err(RangeError::Inverted));
        assert_eq!(PhysRange::new(5, 6).unwrap().len(), 1);
        assert_eq!(PhysRange::from_len(10, 0), Err(RangeError::Empty));
        assert_eq!(PhysRange::from_len(u64::MAX, 1), Err(RangeError::Overflow));
        assert_eq!(
            PhysRange::from_len(u64::MAX - 1, 2),
            Err(RangeError::Overflow)
        );
        assert_eq!(
            PhysRange::from_len(u64::MAX - 1, 1).unwrap().end(),
            u64::MAX
        );
        assert_eq!(PhysRange::from_len(0, u64::MAX).unwrap().len(), u64::MAX);
    }

    #[test]
    fn alignment_helpers_are_checked() {
        assert_eq!(align_up(0, 4096), Ok(0));
        assert_eq!(align_up(1, 4096), Ok(4096));
        assert_eq!(align_up(4096, 4096), Ok(4096));
        assert_eq!(align_up(4097, 4096), Ok(8192));
        assert_eq!(align_up(u64::MAX, 4096), Err(RangeError::Overflow));
        assert_eq!(align_up(u64::MAX - 4094, 4096), Err(RangeError::Overflow));
        assert_eq!(align_up(u64::MAX - 4095, 4096), Ok(u64::MAX - 4095));
        assert_eq!(align_down(4097, 4096), Ok(4096));
        assert_eq!(align_down(u64::MAX, 4096), Ok(u64::MAX - 4095));
        for bad in [0, 3, 6, 4095, MAX_ALIGN << 1, u64::MAX] {
            assert_eq!(align_up(8, bad), Err(RangeError::BadAlignment), "{bad}");
            assert_eq!(align_down(8, bad), Err(RangeError::BadAlignment), "{bad}");
            assert!(!is_aligned(0, bad), "{bad}");
        }
        assert_eq!(align_up(1, 1), Ok(1));
        assert_eq!(align_up(1, MAX_ALIGN), Ok(MAX_ALIGN));
        assert!(is_aligned(8192, 4096) && !is_aligned(8193, 4096));
    }

    #[test]
    fn overlap_touch_contain() {
        let a = r(10, 20);
        assert!(a.overlaps(&r(19, 30)));
        assert!(!a.overlaps(&r(20, 30)), "adjacent is not overlapping");
        assert!(a.touches(&r(20, 30)), "adjacent touches");
        assert!(a.touches(&r(0, 10)));
        assert!(!a.touches(&r(21, 30)), "a one-byte gap does not touch");
        assert!(a.contains(&r(10, 20)) && a.contains(&r(12, 13)));
        assert!(!a.contains(&r(9, 20)) && !a.contains(&r(10, 21)));
        assert!(a.contains_addr(10) && a.contains_addr(19) && !a.contains_addr(20));
    }

    #[test]
    fn intersection_and_union() {
        assert_eq!(r(10, 20).intersection(&r(15, 30)), Some(r(15, 20)));
        assert_eq!(r(10, 20).intersection(&r(20, 30)), None);
        assert_eq!(r(10, 20).intersection(&r(0, 5)), None);
        assert_eq!(r(10, 20).intersection(&r(12, 14)), Some(r(12, 14)));
        assert_eq!(r(10, 20).union_if_touching(&r(20, 30)), Some(r(10, 30)));
        assert_eq!(r(10, 20).union_if_touching(&r(15, 16)), Some(r(10, 20)));
        assert_eq!(r(10, 20).union_if_touching(&r(21, 30)), None);
    }

    #[test]
    fn subtract_covers_every_case() {
        let a = r(10, 40);
        assert_eq!(a.subtract(&r(0, 10)), (Some(a), None), "adjacent below");
        assert_eq!(a.subtract(&r(40, 50)), (Some(a), None), "adjacent above");
        assert_eq!(a.subtract(&r(0, 5)), (Some(a), None), "disjoint");
        assert_eq!(a.subtract(&r(0, 50)), (None, None), "covered");
        assert_eq!(a.subtract(&r(10, 40)), (None, None), "exact");
        assert_eq!(a.subtract(&r(0, 20)), (None, Some(r(20, 40))));
        assert_eq!(a.subtract(&r(30, 50)), (Some(r(10, 30)), None));
        assert_eq!(a.subtract(&r(20, 30)), (Some(r(10, 20)), Some(r(30, 40))));
        assert_eq!(a.subtract(&r(10, 20)), (None, Some(r(20, 40))));
        assert_eq!(a.subtract(&r(30, 40)), (Some(r(10, 30)), None));
    }

    #[test]
    fn subtract_preserves_total_length() {
        for (s, e) in [(0u64, 100u64), (10, 20), (50, 60), (95, 200), (0, 1)] {
            let a = r(0, 100);
            let h = r(s, e);
            let (l, u) = a.subtract(&h);
            let kept = l.map_or(0, |x| x.len()) + u.map_or(0, |x| x.len());
            let removed = a.intersection(&h).map_or(0, |x| x.len());
            assert_eq!(kept + removed, a.len(), "hole {s}..{e}");
        }
    }

    #[test]
    fn clip_and_shrink() {
        assert_eq!(r(10, 20).clip(0, 100), Some(r(10, 20)));
        assert_eq!(r(10, 20).clip(15, 100), Some(r(15, 20)));
        assert_eq!(r(10, 20).clip(0, 15), Some(r(10, 15)));
        assert_eq!(r(10, 20).clip(20, 30), None);
        assert_eq!(r(10, 20).clip(5, 5), None);
        assert_eq!(r(1, 8192).shrink_to_granule(4096), Ok(Some(r(4096, 8192))));
        assert_eq!(r(0, 8192).shrink_to_granule(4096), Ok(Some(r(0, 8192))));
        assert_eq!(r(1, 8191).shrink_to_granule(4096), Ok(None));
        assert_eq!(r(4096, 4097).shrink_to_granule(4096), Ok(None));
        assert_eq!(
            r(100, 200).shrink_to_granule(3),
            Err(RangeError::BadAlignment)
        );
    }

    #[test]
    fn shrink_near_top_of_address_space_does_not_overflow() {
        let top = r(u64::MAX - 100, u64::MAX);
        assert_eq!(top.shrink_to_granule(4096), Ok(None));
        let big = r(u64::MAX - 8191, u64::MAX);
        let s = big.shrink_to_granule(4096).unwrap();
        assert_eq!(s, Some(r(u64::MAX - 8191, u64::MAX - 4095)));
    }

    #[test]
    fn ordering_sorts_by_start_then_end() {
        let mut v = [r(30, 40), r(10, 50), r(10, 20), r(0, 5)];
        v.sort();
        assert_eq!(v, [r(0, 5), r(10, 20), r(10, 50), r(30, 40)]);
    }
}
