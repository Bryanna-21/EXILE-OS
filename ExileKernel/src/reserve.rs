//! Physical reservation tracking (Mission A2, file 3 of 5).
//! SPDX-License-Identifier: Apache-2.0
//!
//! A `ReservationSet` records every physical range the early allocator must never
//! hand out: the kernel image, boot information and memory-map storage,
//! bootloader/firmware/platform regions, allocator metadata, bootstrap allocations
//! and anything else explicitly protected.
//!
//! Invariants (checked by `check_invariants` and by the tests after every change):
//!   * entries are sorted by address, pairwise disjoint and NOT adjacent: ranges
//!     that overlap or touch are merged, so the set never grows because of
//!     fragmentation;
//!   * merging never loses protection: the merged range covers the union of the
//!     inputs, and keeps the union of their `ReserveKind` bits;
//!   * a failed `add` (capacity) leaves the set exactly as it was;
//!   * storage is a fixed array: no heap, no allocation.
//! The same type also logs live allocations (kind `Allocated`) so that adjacent
//! allocations coalesce into one entry.
#![allow(dead_code)]
use crate::bootinfo::{self, BootInfo, BootInfoError};
use crate::range::{PAGE_SIZE, PhysRange, RangeError};

/// Why a range is protected. Kinds are bit flags inside a merged entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReserveKind {
    KernelImage = 0,
    BootInfo = 1,
    MemoryMap = 2,
    Bootloader = 3,
    Firmware = 4,
    AllocatorMeta = 5,
    Bootstrap = 6,
    Mmio = 7,
    Explicit = 8,
    /// A live allocation made by the early allocator (not a boot reservation).
    Allocated = 9,
}

impl ReserveKind {
    pub const fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// One merged protected range and the set of reasons behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reservation {
    range: PhysRange,
    kinds: u16,
}

impl Reservation {
    const UNUSED: Reservation = Reservation {
        range: PhysRange::UNUSED,
        kinds: 0,
    };

    pub const fn range(&self) -> PhysRange {
        self.range
    }
    /// Bit mask of `ReserveKind::bit()` values.
    pub const fn kinds(&self) -> u16 {
        self.kinds
    }
    pub const fn has(&self, kind: ReserveKind) -> bool {
        self.kinds & kind.bit() != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveError {
    /// The set has no room for the entry this `add` would create.
    CapacityExceeded,
    Range(RangeError),
    BootInfo(BootInfoError),
}

impl From<RangeError> for ReserveError {
    fn from(e: RangeError) -> Self {
        ReserveError::Range(e)
    }
}

/// Where an `add` would put its (merged) entry. Computed without mutating so that
/// `can_add` and `add` can never disagree.
struct Plan {
    first: usize,
    last: usize, // one past the last merged entry
    merged: PhysRange,
    kinds: u16,
    new_len: usize,
}

#[derive(Debug, Clone)]
pub struct ReservationSet<const N: usize> {
    items: [Reservation; N],
    len: usize,
}

impl<const N: usize> Default for ReservationSet<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ReservationSet<N> {
    pub const fn new() -> Self {
        Self {
            items: [Reservation::UNUSED; N],
            len: 0,
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// The live entries, sorted by address.
    pub fn as_slice(&self) -> &[Reservation] {
        &self.items[..self.len]
    }

    fn plan(&self, range: PhysRange, kind: ReserveKind) -> Plan {
        let mut first = 0;
        while first < self.len && self.items[first].range.end() < range.start() {
            first += 1; // strictly below and not touching
        }
        let mut last = first;
        let mut merged = range;
        let mut kinds = kind.bit();
        while last < self.len && self.items[last].range.start() <= range.end() {
            let it = &self.items[last];
            merged = PhysRange::new(
                merged.start().min(it.range.start()),
                merged.end().max(it.range.end()),
            )
            .unwrap_or(merged); // cannot fail: min < max by construction
            kinds |= it.kinds;
            last += 1;
        }
        Plan {
            first,
            last,
            merged,
            kinds,
            new_len: self.len - (last - first) + 1,
        }
    }

    /// Would `add(range, _)` succeed?
    pub fn can_add(&self, range: PhysRange) -> bool {
        self.plan(range, ReserveKind::Explicit).new_len <= N
    }

    /// Protects `range`. Overlapping and adjacent entries are merged, so no
    /// protected byte is ever dropped. On `CapacityExceeded` nothing changed.
    pub fn add(&mut self, range: PhysRange, kind: ReserveKind) -> Result<(), ReserveError> {
        let p = self.plan(range, kind);
        if p.new_len > N {
            return Err(ReserveError::CapacityExceeded);
        }
        // Move the tail so that exactly one slot (index `first`) holds the merge.
        self.items.copy_within(p.last..self.len, p.first + 1);
        self.items[p.first] = Reservation {
            range: p.merged,
            kinds: p.kinds,
        };
        self.len = p.new_len;
        Ok(())
    }

    pub fn add_len(&mut self, start: u64, len: u64, kind: ReserveKind) -> Result<(), ReserveError> {
        self.add(PhysRange::from_len(start, len)?, kind)
    }

    /// Does any protected byte lie inside `range`?
    pub fn overlaps(&self, range: &PhysRange) -> bool {
        self.as_slice().iter().any(|r| r.range.overlaps(range))
    }

    /// Is `range` entirely protected? (Merging guarantees a covered range sits
    /// inside ONE entry.)
    pub fn covers(&self, range: &PhysRange) -> bool {
        self.as_slice().iter().any(|r| r.range.contains(range))
    }

    /// Total protected bytes (saturating; the set is disjoint so this is exact
    /// unless the set covers the whole 64-bit space).
    pub fn total_bytes(&self) -> u64 {
        self.as_slice()
            .iter()
            .fold(0u64, |acc, r| acc.saturating_add(r.range.len()))
    }

    /// Calls `emit` for every maximal sub-range of `within` that is NOT protected,
    /// in ascending order. Stops at the first error from `emit`.
    pub fn for_each_gap<E>(
        &self,
        within: &PhysRange,
        mut emit: impl FnMut(PhysRange) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut cursor = within.start();
        for it in self.as_slice() {
            let r = it.range;
            if r.end() <= cursor {
                continue;
            }
            if r.start() >= within.end() {
                break;
            }
            let gap_end = r.start().min(within.end());
            if let Ok(gap) = PhysRange::new(cursor, gap_end) {
                emit(gap)?;
            }
            cursor = cursor.max(r.end());
        }
        if let Ok(gap) = PhysRange::new(cursor, within.end()) {
            emit(gap)?;
        }
        Ok(())
    }

    /// Reserves everything a validated `BootInfo` (the loader ABI) makes the kernel
    /// responsible for: the kernel image, the `BootInfo` object itself at
    /// `info_paddr`, the memory-map array, the framebuffer and the page holding the
    /// ACPI RSDP. All-or-nothing.
    pub fn add_boot_objects(
        &mut self,
        info_paddr: u64,
        info: &BootInfo,
    ) -> Result<(), ReserveError> {
        info.validate().map_err(ReserveError::BootInfo)?;
        let mut t = self.clone();
        t.add(
            PhysRange::new(info.kernel_phys_start, info.kernel_phys_end)?,
            ReserveKind::KernelImage,
        )?;
        t.add_len(info_paddr, bootinfo::SIZE as u64, ReserveKind::BootInfo)?;
        let map_bytes = info
            .memmap_count
            .checked_mul(bootinfo::MEM_REGION_SIZE as u64)
            .ok_or(RangeError::Overflow)?;
        t.add_len(info.memmap_paddr, map_bytes, ReserveKind::MemoryMap)?;
        if info.flags & bootinfo::FLAG_FRAMEBUFFER != 0 {
            t.add_len(
                info.framebuffer_paddr,
                info.framebuffer_size,
                ReserveKind::Mmio,
            )?;
        }
        if info.flags & bootinfo::FLAG_ACPI_RSDP != 0 {
            let page = info.rsdp_paddr & !(PAGE_SIZE - 1);
            t.add_len(page, PAGE_SIZE, ReserveKind::Firmware)?;
        }
        *self = t;
        Ok(())
    }

    /// Structural audit. `Ok` means every invariant in the module docs holds.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        if self.len > N {
            return Err("length exceeds capacity");
        }
        let mut prev_end: Option<u64> = None;
        for it in self.as_slice() {
            if it.range.start() >= it.range.end() {
                return Err("empty or inverted range");
            }
            if it.kinds == 0 {
                return Err("entry with no kind");
            }
            if let Some(pe) = prev_end {
                if it.range.start() <= pe {
                    return Err("entries overlap, touch or are unsorted");
                }
            }
            prev_end = Some(it.range.end());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootinfo::{ARCH_X86_64, MAGIC, VERSION};

    fn r(s: u64, e: u64) -> PhysRange {
        PhysRange::new(s, e).unwrap()
    }
    fn spans<const N: usize>(s: &ReservationSet<N>) -> Vec<(u64, u64)> {
        s.as_slice()
            .iter()
            .map(|x| (x.range().start(), x.range().end()))
            .collect()
    }

    #[test]
    fn add_keeps_entries_sorted() {
        let mut s = ReservationSet::<8>::new();
        s.add(r(300, 400), ReserveKind::KernelImage).unwrap();
        s.add(r(100, 200), ReserveKind::BootInfo).unwrap();
        s.add(r(500, 600), ReserveKind::MemoryMap).unwrap();
        assert_eq!(spans(&s), [(100, 200), (300, 400), (500, 600)]);
        s.check_invariants().unwrap();
    }

    #[test]
    fn overlapping_reservations_merge_and_keep_every_kind() {
        let mut s = ReservationSet::<8>::new();
        s.add(r(100, 300), ReserveKind::KernelImage).unwrap();
        s.add(r(200, 500), ReserveKind::BootInfo).unwrap();
        assert_eq!(spans(&s), [(100, 500)]);
        let e = s.as_slice()[0];
        assert!(e.has(ReserveKind::KernelImage) && e.has(ReserveKind::BootInfo));
        assert!(!e.has(ReserveKind::Firmware));
        assert!(s.covers(&r(100, 500)));
        s.check_invariants().unwrap();
    }

    #[test]
    fn adjacent_reservations_merge_without_losing_coverage() {
        let mut s = ReservationSet::<8>::new();
        s.add(r(100, 200), ReserveKind::KernelImage).unwrap();
        s.add(r(200, 300), ReserveKind::Firmware).unwrap();
        s.add(r(50, 100), ReserveKind::Mmio).unwrap();
        assert_eq!(spans(&s), [(50, 300)]);
        assert!(s.covers(&r(50, 300)));
        assert_eq!(s.total_bytes(), 250);
    }

    #[test]
    fn a_one_byte_gap_keeps_entries_separate() {
        let mut s = ReservationSet::<8>::new();
        s.add(r(100, 200), ReserveKind::KernelImage).unwrap();
        s.add(r(201, 300), ReserveKind::KernelImage).unwrap();
        assert_eq!(spans(&s), [(100, 200), (201, 300)]);
        assert!(!s.covers(&r(100, 300)));
        assert!(!s.overlaps(&r(200, 201)), "the gap byte is unprotected");
    }

    #[test]
    fn one_add_can_swallow_many_entries() {
        let mut s = ReservationSet::<8>::new();
        for i in 0..5u64 {
            s.add(r(i * 100, i * 100 + 10), ReserveKind::Explicit)
                .unwrap();
        }
        assert_eq!(s.len(), 5);
        s.add(r(5, 400), ReserveKind::Bootloader).unwrap();
        assert_eq!(spans(&s), [(0, 410)]);
        assert!(s.as_slice()[0].has(ReserveKind::Explicit));
        s.check_invariants().unwrap();
    }

    #[test]
    fn duplicate_and_contained_adds_do_not_grow_the_set() {
        let mut s = ReservationSet::<2>::new();
        s.add(r(0, 100), ReserveKind::KernelImage).unwrap();
        s.add(r(0, 100), ReserveKind::KernelImage).unwrap();
        s.add(r(10, 20), ReserveKind::Bootstrap).unwrap();
        assert_eq!(spans(&s), [(0, 100)]);
    }

    #[test]
    fn capacity_failure_changes_nothing() {
        let mut s = ReservationSet::<2>::new();
        s.add(r(0, 10), ReserveKind::KernelImage).unwrap();
        s.add(r(100, 110), ReserveKind::KernelImage).unwrap();
        let before = s.clone();
        assert!(!s.can_add(r(500, 510)));
        assert_eq!(
            s.add(r(500, 510), ReserveKind::Explicit),
            Err(ReserveError::CapacityExceeded)
        );
        assert_eq!(spans(&s), spans(&before));
        assert_eq!(s.len(), 2);
        // An add that MERGES still fits at full capacity.
        assert!(s.can_add(r(10, 20)));
        s.add(r(10, 20), ReserveKind::Explicit).unwrap();
        assert_eq!(spans(&s), [(0, 20), (100, 110)]);
        s.check_invariants().unwrap();
    }

    #[test]
    fn zero_capacity_set_rejects_everything() {
        let mut s = ReservationSet::<0>::new();
        assert_eq!(
            s.add(r(0, 1), ReserveKind::Explicit),
            Err(ReserveError::CapacityExceeded)
        );
        assert!(s.is_empty());
    }

    #[test]
    fn invalid_ranges_are_range_errors() {
        let mut s = ReservationSet::<4>::new();
        assert_eq!(
            s.add_len(10, 0, ReserveKind::Explicit),
            Err(ReserveError::Range(RangeError::Empty))
        );
        assert_eq!(
            s.add_len(u64::MAX, 2, ReserveKind::Explicit),
            Err(ReserveError::Range(RangeError::Overflow))
        );
        assert!(s.is_empty());
    }

    #[test]
    fn gaps_are_the_complement_of_the_reservations() {
        let mut s = ReservationSet::<8>::new();
        s.add(r(100, 200), ReserveKind::KernelImage).unwrap();
        s.add(r(300, 350), ReserveKind::KernelImage).unwrap();
        let mut gaps = Vec::new();
        s.for_each_gap::<()>(&r(0, 1000), |g| {
            gaps.push((g.start(), g.end()));
            Ok(())
        })
        .unwrap();
        assert_eq!(gaps, [(0, 100), (200, 300), (350, 1000)]);
        let mut inside = Vec::new();
        s.for_each_gap::<()>(&r(150, 320), |g| {
            inside.push((g.start(), g.end()));
            Ok(())
        })
        .unwrap();
        assert_eq!(inside, [(200, 300)]);
        let mut none = Vec::new();
        s.for_each_gap::<()>(&r(120, 180), |g| {
            none.push(g);
            Ok(())
        })
        .unwrap();
        assert!(none.is_empty(), "a fully protected window has no gaps");
    }

    #[test]
    fn gap_iteration_stops_on_the_first_error() {
        let s = ReservationSet::<4>::new();
        let mut calls = 0;
        let res = s.for_each_gap(&r(0, 10), |_| {
            calls += 1;
            Err("stop")
        });
        assert_eq!(res, Err("stop"));
        assert_eq!(calls, 1);
    }

    fn info(kernel: (u64, u64), map: (u64, u64)) -> BootInfo {
        BootInfo {
            magic: MAGIC,
            version: VERSION,
            total_size: (bootinfo::SIZE as u64 + map.1 * bootinfo::MEM_REGION_SIZE as u64) as u32,
            arch: ARCH_X86_64,
            flags: 0,
            memmap_paddr: map.0,
            memmap_count: map.1,
            kernel_phys_start: kernel.0,
            kernel_phys_end: kernel.1,
            kernel_entry: kernel.0,
            framebuffer_paddr: 0,
            framebuffer_size: 0,
            rsdp_paddr: 0,
        }
    }

    #[test]
    fn boot_objects_from_a_validated_bootinfo_are_reserved() {
        let mut bi = info((0x10_0000, 0x20_0000), (0x8000, 3));
        bi.flags = bootinfo::FLAG_FRAMEBUFFER | bootinfo::FLAG_ACPI_RSDP;
        bi.framebuffer_paddr = 0xe000_0000;
        bi.framebuffer_size = 0x10_0000;
        bi.rsdp_paddr = 0xf_5a10;
        let mut s = ReservationSet::<16>::new();
        s.add_boot_objects(0x7000, &bi).unwrap();
        for need in [
            r(0x10_0000, 0x20_0000),
            r(0x7000, 0x7000 + bootinfo::SIZE as u64),
            r(0x8000, 0x8000 + 3 * 24),
            r(0xe000_0000, 0xe010_0000),
            r(0xf_5000, 0xf_6000),
        ] {
            assert!(s.covers(&need), "{need:?} not covered");
        }
        s.check_invariants().unwrap();
    }

    #[test]
    fn boot_objects_are_all_or_nothing() {
        let bi = info((0x10_0000, 0x20_0000), (0x8000, 3));
        let mut s = ReservationSet::<1>::new();
        s.add(r(0, 10), ReserveKind::Explicit).unwrap();
        let before = spans(&s);
        assert_eq!(
            s.add_boot_objects(0x7000, &bi),
            Err(ReserveError::CapacityExceeded)
        );
        assert_eq!(
            spans(&s),
            before,
            "partial reservations must be rolled back"
        );
    }

    #[test]
    fn invalid_bootinfo_reserves_nothing() {
        let mut bi = info((0x10_0000, 0x20_0000), (0x8000, 3));
        bi.magic = 0;
        let mut s = ReservationSet::<8>::new();
        assert_eq!(
            s.add_boot_objects(0x7000, &bi),
            Err(ReserveError::BootInfo(BootInfoError::BadMagic))
        );
        assert!(s.is_empty());
    }

    #[test]
    fn invariants_hold_over_a_pseudo_random_workload() {
        // xorshift64*: deterministic.
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        };
        let mut s = ReservationSet::<12>::new();
        let mut oracle = vec![false; 2048];
        for _ in 0..4000 {
            let a = next() % 2000;
            let l = next() % 40 + 1;
            let rng = r(a, a + l);
            if s.can_add(rng) {
                s.add(rng, ReserveKind::Explicit).unwrap();
                for b in a..a + l {
                    oracle[b as usize] = true;
                }
            } else {
                let before = spans(&s);
                assert_eq!(
                    s.add(rng, ReserveKind::Explicit),
                    Err(ReserveError::CapacityExceeded)
                );
                assert_eq!(spans(&s), before);
            }
            s.check_invariants().unwrap();
            // Exact coverage equivalence with the byte-level oracle.
            let mut model = vec![false; 2048];
            for e in s.as_slice() {
                for b in e.range().start()..e.range().end() {
                    model[b as usize] = true;
                }
            }
            assert_eq!(model, oracle);
        }
    }
}
