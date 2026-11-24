//! Firmware memory-map normalization with explicit precedence (Mission A1,
//! extended by Mission A2 as file 2 of 5).
//! SPDX-License-Identifier: Apache-2.0
//!
//! The strict parser in `memory.rs` (rejects any overlap) is untouched. This module
//! is the tolerant path: it turns an arbitrary list of firmware entries into a
//! sorted list of disjoint, classified intervals that the early allocator can trust.
//!
//! ALGORITHM (deterministic, O(n^2), n <= `MAX_IN`, no heap):
//!   1. Drop zero-size entries (degenerate, carry no authority).
//!   2. Reject `start + size` overflow and inverted `Region`s: the WHOLE map is
//!      rejected (fail closed).
//!   3. Break the address space at every boundary of every entry.
//!   4. Classify each elementary interval by PRECEDENCE; the most restrictive
//!      covering entry wins:
//!        Unusable > AcpiReclaim > Reserved > Bad > Usable
//!      so a usable entry overlapping anything else can never leak RAM. Ties keep
//!      the entry that appears first in the input.
//!   5. Intervals that NO entry covers are GAPS. They are never emitted, therefore
//!      never usable: absence of a firmware entry is not permission to use memory.
//!   6. Merge adjacent intervals with the same kind AND the same source tag.
//!
//! Numeric kinds (Multiboot / loader ABI): 1 usable, 2 reserved, 3 ACPI reclaimable,
//! 4 ACPI NVS (never RAM), 5 bad RAM. Anything else is Reserved, never usable.
//!
//! Provenance: `Interval::source` is the caller-supplied tag of the entry that
//! decided the interval (the index in the original map when the caller passes
//! `enumerate()` order, as the adapters below do).
#![allow(dead_code)]
use crate::bootinfo::MemRegion;
use crate::memory::Region;
use crate::range::PhysRange;

/// Classification, ordered strongest (lowest) to weakest (highest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// ACPI NVS and similar: never RAM, never reclaimable.
    Unusable,
    /// ACPI reclaimable: usable only after the OS has consumed the tables.
    AcpiReclaim,
    /// Reserved by firmware or platform, and every unknown numeric kind.
    Reserved,
    /// Present but defective.
    Bad,
    /// Normal RAM.
    Usable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub start: u64,
    pub end: u64, // exclusive
    pub kind: Kind,
    /// Tag of the input entry that decided this interval.
    pub source: u32,
}

impl Interval {
    pub const EMPTY: Interval = Interval {
        start: 0,
        end: 0,
        kind: Kind::Reserved,
        source: 0,
    };

    pub fn range(&self) -> Option<PhysRange> {
        PhysRange::new(self.start, self.end).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// `start + size` exceeds 64 bits.
    Overflow,
    /// A `Region` with `end < start`.
    Inverted,
    /// More than `MAX_IN` entries, or the output buffer is too small.
    TooMany,
    /// No entry with a non-zero size.
    Empty,
}

pub const MAX_IN: usize = 256;
/// Upper bound on emitted intervals: `2 * MAX_IN - 1` elementary intervals.
pub const MAX_OUT: usize = 2 * MAX_IN;

#[derive(Clone, Copy)]
struct Entry {
    start: u64,
    end: u64,
    kind: Kind,
    source: u32,
}

/// Numeric firmware kind to `Kind`. Fail closed: unknown numbers are `Reserved`.
pub fn map_kind(num: u32) -> Kind {
    match num {
        1 => Kind::Usable,
        2 => Kind::Reserved,
        3 => Kind::AcpiReclaim,
        4 => Kind::Unusable,
        5 => Kind::Bad,
        _ => Kind::Reserved,
    }
}

/// Core routine. `raw` yields `(source_tag, start, size, numeric_kind)`.
/// Returns the number of intervals written to `out`. On error `out` may hold
/// partial scratch data and MUST be ignored; the count is never returned.
pub fn normalize_iter(
    raw: impl Iterator<Item = Result<(u32, u64, u64, u32), Error>>,
    out: &mut [Interval],
) -> Result<usize, Error> {
    let mut entries = [Entry {
        start: 0,
        end: 0,
        kind: Kind::Reserved,
        source: 0,
    }; MAX_IN];
    let mut n = 0usize;
    let mut seen = 0usize;
    for item in raw {
        let (source, start, size, num_kind) = item?;
        seen += 1;
        if seen > MAX_IN {
            return Err(Error::TooMany);
        }
        if size == 0 {
            continue;
        }
        let end = start.checked_add(size).ok_or(Error::Overflow)?;
        entries[n] = Entry {
            start,
            end,
            kind: map_kind(num_kind),
            source,
        };
        n += 1;
    }
    if n == 0 {
        return Err(Error::Empty);
    }
    let entries = &entries[..n];

    // Boundary points, sorted and deduplicated (insertion sort: n is bounded).
    let mut points = [0u64; 2 * MAX_IN];
    let mut np = 0usize;
    for e in entries {
        points[np] = e.start;
        points[np + 1] = e.end;
        np += 2;
    }
    for i in 1..np {
        let mut j = i;
        while j > 0 && points[j - 1] > points[j] {
            points.swap(j - 1, j);
            j -= 1;
        }
    }
    let mut up = 0usize;
    for i in 0..np {
        if up == 0 || points[up - 1] != points[i] {
            points[up] = points[i];
            up += 1;
        }
    }

    let mut count = 0usize;
    for w in 0..up.saturating_sub(1) {
        let (s, e) = (points[w], points[w + 1]);
        let Some((kind, source)) = strongest_covering(entries, s, e) else {
            continue; // gap: no entry covers it, so it is never emitted
        };
        if count > 0 {
            let last = out[count - 1];
            if last.end == s && last.kind == kind && last.source == source {
                out[count - 1].end = e;
                continue;
            }
        }
        if count >= out.len() {
            return Err(Error::TooMany);
        }
        out[count] = Interval {
            start: s,
            end: e,
            kind,
            source,
        };
        count += 1;
    }
    Ok(count)
}

/// Strongest class among entries covering `[s, e)`; `None` if none covers it.
fn strongest_covering(entries: &[Entry], s: u64, e: u64) -> Option<(Kind, u32)> {
    let mut best: Option<(Kind, u32)> = None;
    for en in entries {
        if en.start < e && s < en.end {
            match best {
                Some((k, _)) if k <= en.kind => {} // tie or weaker: keep the first
                _ => best = Some((en.kind, en.source)),
            }
        }
    }
    best
}

/// Slice entry point. `raw` items are `(source_tag, start, size, numeric_kind)`.
pub fn normalize_into(raw: &[(u32, u64, u64, u32)], out: &mut [Interval]) -> Result<usize, Error> {
    if raw.is_empty() {
        return Err(Error::Empty);
    }
    normalize_iter(raw.iter().map(|&t| Ok(t)), out)
}

/// Normalizes the strict parser's `Region`s (`end` exclusive). Source = index.
pub fn normalize_regions(regions: &[Region], out: &mut [Interval]) -> Result<usize, Error> {
    if regions.is_empty() {
        return Err(Error::Empty);
    }
    normalize_iter(
        regions.iter().enumerate().map(|(i, r)| {
            let size = r.end.checked_sub(r.start).ok_or(Error::Inverted)?;
            Ok((i as u32, r.start, size, r.kind))
        }),
        out,
    )
}

/// Normalizes the loader ABI's `MemRegion`s. Source = index.
pub fn normalize_mem_regions(regions: &[MemRegion], out: &mut [Interval]) -> Result<usize, Error> {
    if regions.is_empty() {
        return Err(Error::Empty);
    }
    normalize_iter(
        regions
            .iter()
            .enumerate()
            .map(|(i, r)| Ok((i as u32, r.start, r.size, r.kind))),
        out,
    )
}

/// The usable intervals of a normalized map, as validated ranges, ascending.
pub fn usable_ranges(intervals: &[Interval]) -> impl Iterator<Item = PhysRange> + '_ {
    intervals
        .iter()
        .filter(|i| i.kind == Kind::Usable)
        .filter_map(|i| i.range())
}

/// Structural audit of a normalized map: sorted, disjoint, non-empty.
pub fn check_normalized(intervals: &[Interval]) -> Result<(), &'static str> {
    let mut prev_end = 0u64;
    for (idx, i) in intervals.iter().enumerate() {
        if i.start >= i.end {
            return Err("empty or inverted interval");
        }
        if idx > 0 && i.start < prev_end {
            return Err("intervals overlap or are unsorted");
        }
        prev_end = i.end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(raw: &[(u32, u64, u64, u32)]) -> Vec<Interval> {
        let mut out = [Interval::EMPTY; MAX_OUT];
        let n = normalize_into(raw, &mut out).unwrap();
        check_normalized(&out[..n]).unwrap();
        out[..n].to_vec()
    }
    fn spans(v: &[Interval]) -> Vec<(u64, u64, Kind)> {
        v.iter().map(|i| (i.start, i.end, i.kind)).collect()
    }

    #[test]
    fn numeric_kinds_map_fail_closed() {
        assert_eq!(map_kind(1), Kind::Usable);
        assert_eq!(map_kind(2), Kind::Reserved);
        assert_eq!(map_kind(3), Kind::AcpiReclaim);
        assert_eq!(map_kind(4), Kind::Unusable, "ACPI NVS is never RAM");
        assert_eq!(map_kind(5), Kind::Bad);
        for k in [0u32, 6, 7, 100, u32::MAX] {
            assert_eq!(map_kind(k), Kind::Reserved, "unknown kind {k}");
        }
    }

    #[test]
    fn simple_map_is_passed_through() {
        let v = run(&[(0, 0, 0x9f000, 1), (1, 0x10_0000, 0x3ff0_0000, 1)]);
        assert_eq!(
            spans(&v),
            [
                (0, 0x9f000, Kind::Usable),
                (0x10_0000, 0x4000_0000, Kind::Usable)
            ]
        );
    }

    #[test]
    fn unsorted_input_is_sorted() {
        let v = run(&[
            (0, 0x3000, 0x1000, 1),
            (1, 0x1000, 0x1000, 2),
            (2, 0x2000, 0x1000, 1),
        ]);
        assert_eq!(
            spans(&v),
            [
                (0x1000, 0x2000, Kind::Reserved),
                (0x2000, 0x3000, Kind::Usable),
                (0x3000, 0x4000, Kind::Usable)
            ]
        );
    }

    #[test]
    fn holes_between_entries_are_not_usable() {
        // Nothing describes [0x2000, 0x3000): it must not appear at all.
        let v = run(&[(0, 0x1000, 0x1000, 1), (1, 0x3000, 0x1000, 1)]);
        assert_eq!(
            spans(&v),
            [
                (0x1000, 0x2000, Kind::Usable),
                (0x3000, 0x4000, Kind::Usable)
            ]
        );
        assert!(usable_ranges(&v).all(|r| !(r.start() < 0x3000 && r.end() > 0x2000)));
    }

    #[test]
    fn reserved_beats_usable_where_they_overlap() {
        let v = run(&[(0, 0, 0x4000, 1), (1, 0x1000, 0x1000, 2)]);
        assert_eq!(
            spans(&v),
            [
                (0, 0x1000, Kind::Usable),
                (0x1000, 0x2000, Kind::Reserved),
                (0x2000, 0x4000, Kind::Usable)
            ]
        );
        assert_eq!(v[1].source, 1, "provenance names the deciding entry");
    }

    #[test]
    fn full_precedence_chain() {
        // Same window described five ways; the strongest wins in every order.
        let kinds = [1u32, 5, 2, 3, 4];
        for rot in 0..kinds.len() {
            let raw: Vec<_> = (0..kinds.len())
                .map(|i| (i as u32, 0u64, 0x1000u64, kinds[(i + rot) % kinds.len()]))
                .collect();
            let v = run(&raw);
            assert_eq!(spans(&v), [(0, 0x1000, Kind::Unusable)], "rot {rot}");
        }
        let v = run(&[(0, 0, 0x1000, 1), (1, 0, 0x1000, 5)]);
        assert_eq!(v[0].kind, Kind::Bad);
        let v = run(&[(0, 0, 0x1000, 5), (1, 0, 0x1000, 2)]);
        assert_eq!(v[0].kind, Kind::Reserved);
        let v = run(&[(0, 0, 0x1000, 2), (1, 0, 0x1000, 3)]);
        assert_eq!(v[0].kind, Kind::AcpiReclaim);
        let v = run(&[(0, 0, 0x1000, 3), (1, 0, 0x1000, 4)]);
        assert_eq!(v[0].kind, Kind::Unusable);
    }

    #[test]
    fn nvs_is_never_usable_and_never_reclaimable() {
        let v = run(&[(0, 0, 0x2000, 4)]);
        assert_eq!(v[0].kind, Kind::Unusable);
        assert_eq!(usable_ranges(&v).count(), 0);
    }

    #[test]
    fn duplicates_and_zero_length_entries() {
        let v = run(&[
            (0, 0x1000, 0x1000, 1),
            (1, 0x1000, 0x1000, 1),
            (2, 0x5000, 0, 2),
        ]);
        assert_eq!(spans(&v), [(0x1000, 0x2000, Kind::Usable)]);
        assert_eq!(v[0].source, 0, "ties keep the first entry");
        let mut out = [Interval::EMPTY; 4];
        assert_eq!(normalize_into(&[(0, 5, 0, 1)], &mut out), Err(Error::Empty));
    }

    #[test]
    fn adjacent_same_kind_same_source_merge_only_within_one_entry() {
        let v = run(&[(0, 0, 0x1000, 1), (1, 0x1000, 0x1000, 1)]);
        assert_eq!(v.len(), 2, "different sources stay distinct for provenance");
        let v = run(&[(0, 0, 0x3000, 1), (1, 0x1000, 0x1000, 1)]);
        assert_eq!(spans(&v), [(0, 0x3000, Kind::Usable)]);
    }

    #[test]
    fn overflow_rejects_the_whole_map() {
        let mut out = [Interval::EMPTY; 8];
        assert_eq!(
            normalize_into(
                &[(0, 0, 0x1000, 1), (1, u64::MAX - 0xff, 0x1000, 1)],
                &mut out
            ),
            Err(Error::Overflow)
        );
        assert_eq!(
            normalize_into(&[(0, u64::MAX, 1, 1)], &mut out),
            Err(Error::Overflow)
        );
        assert!(normalize_into(&[(0, u64::MAX - 1, 1, 1)], &mut out).is_ok());
    }

    #[test]
    fn too_many_entries_and_small_output_are_errors() {
        let big: Vec<_> = (0..MAX_IN as u32 + 1)
            .map(|i| (i, u64::from(i) * 0x2000, 0x1000, 1))
            .collect();
        let mut out = [Interval::EMPTY; MAX_OUT];
        assert_eq!(normalize_into(&big, &mut out), Err(Error::TooMany));
        let ok = &big[..MAX_IN];
        assert_eq!(normalize_into(ok, &mut out), Ok(MAX_IN));
        let mut small = [Interval::EMPTY; 3];
        assert_eq!(normalize_into(ok, &mut small), Err(Error::TooMany));
        assert_eq!(normalize_into(&[], &mut out), Err(Error::Empty));
    }

    #[test]
    fn worst_case_overlap_fits_the_output_bound() {
        // Staggered overlapping entries produce the most elementary intervals.
        let raw: Vec<_> = (0..MAX_IN as u32)
            .map(|i| {
                (
                    i,
                    u64::from(i) * 0x1000,
                    0x1_0000,
                    if i % 2 == 0 { 1 } else { 2 },
                )
            })
            .collect();
        let mut out = [Interval::EMPTY; MAX_OUT];
        let n = normalize_into(&raw, &mut out).unwrap();
        assert!(n <= MAX_OUT);
        check_normalized(&out[..n]).unwrap();
    }

    #[test]
    fn region_and_memregion_adapters_agree_with_the_slice_path() {
        let regions = [
            Region {
                start: 0x1000,
                end: 0x3000,
                kind: 1,
            },
            Region {
                start: 0x2000,
                end: 0x4000,
                kind: 2,
            },
        ];
        let mem = [
            MemRegion {
                start: 0x1000,
                size: 0x2000,
                kind: 1,
                _pad: 0,
            },
            MemRegion {
                start: 0x2000,
                size: 0x2000,
                kind: 2,
                _pad: 0,
            },
        ];
        let raw = [(0, 0x1000, 0x2000, 1), (1, 0x2000, 0x2000, 2)];
        let (mut a, mut b, mut c) = (
            [Interval::EMPTY; 8],
            [Interval::EMPTY; 8],
            [Interval::EMPTY; 8],
        );
        let na = normalize_regions(&regions, &mut a).unwrap();
        let nb = normalize_mem_regions(&mem, &mut b).unwrap();
        let nc = normalize_into(&raw, &mut c).unwrap();
        assert_eq!((na, nb), (nc, nc));
        assert_eq!(&a[..na], &c[..nc]);
        assert_eq!(&b[..nb], &c[..nc]);
        let inverted = [Region {
            start: 10,
            end: 5,
            kind: 1,
        }];
        assert_eq!(normalize_regions(&inverted, &mut a), Err(Error::Inverted));
    }

    #[test]
    fn matches_a_byte_level_oracle_on_random_maps() {
        let mut x = 0x1234_5678_9abc_def1u64;
        let mut next = move || {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        };
        for _ in 0..500 {
            let cnt = (next() % 12 + 1) as usize;
            let raw: Vec<(u32, u64, u64, u32)> = (0..cnt)
                .map(|i| {
                    (
                        i as u32,
                        (next() % 60) * 16,
                        (next() % 8) * 16,
                        (next() % 8) as u32,
                    )
                })
                .collect();
            let mut out = [Interval::EMPTY; MAX_OUT];
            let n = match normalize_into(&raw, &mut out) {
                Ok(n) => n,
                Err(Error::Empty) => {
                    assert!(raw.iter().all(|r| r.2 == 0));
                    continue;
                }
                Err(e) => panic!("{e:?}"),
            };
            check_normalized(&out[..n]).unwrap();
            for byte in 0..(70 * 16u64) {
                let mut want: Option<Kind> = None;
                for &(_, s, z, k) in &raw {
                    if z != 0 && byte >= s && byte < s + z {
                        let kk = map_kind(k);
                        want = Some(want.map_or(kk, |w| w.min(kk)));
                    }
                }
                let got = out[..n]
                    .iter()
                    .find(|i| byte >= i.start && byte < i.end)
                    .map(|i| i.kind);
                assert_eq!(got, want, "byte {byte} of {raw:?}");
            }
        }
    }
}
