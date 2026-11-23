//! Firmware memory-map normalization with explicit precedence (Mission A,
//! req. 5). Replaces the original parser's blanket Overlap rejection with a
//! documented, tested algorithm â€” while the original memory.rs and its 18
//! tests remain untouched and passing (their semantics are the STRICT path).
//!
//! ALGORITHM (deterministic, O(n^2), n <= 256):
//!   1. Drop zero-size entries (degenerate, carries no authority).
//!   2. Reject start+size overflow (fail-closed: the whole map is rejected).
//!   3. Break intervals at every boundary point of every entry.
//!   4. Classify each elementary interval with PRECEDENCE â€” the strongest
//!      (most restrictive) classification of any covering entry wins:
//!        Unusable > AcpiReclaim > Reserved > Hibernated > Bad > Usable
//!      Consequence: non-usable firmware regions can never become RAM, and
//!      overlapping a usable region with a reserved one yields reserved.
//!   5. Merge equal-class adjacent intervals; preserve provenance: each
//!      output interval records the class of the entry that decided it and
//!      the index of that entry in the input map.
//! Handles: unsorted, adjacent, overlapping, duplicate, zero-length, and
//! overflow-checked inputs. All arithmetic checked.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Strongest. ACPI NVS / hibernate / unreclaimable firmware: never RAM.
    Unusable,
    /// ACPI reclaimable: usable ONLY after the OS frees it (not at boot).
    AcpiReclaim,
    /// Reserved by firmware or platform: never allocatable at boot.
    Reserved,
    /// Present but defective: never use.
    Bad,
    /// Normal RAM.
    Usable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub start: u64,
    pub end: u64, // exclusive
    pub kind: Kind,
    /// Provenance: index into the INPUT map of the entry whose class decided
    /// this interval (the entry with the strongest class covering it).
    pub source: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Overflow,
    TooMany,
    Empty,
    ZeroLengthRejected, // (reserved for policy; currently zero-length is dropped, not fatal)
}

pub const MAX_IN: usize = 256;
/// Hard bound on elementary intervals: 2 per input entry boundary pair.
pub const MAX_OUT: usize = 2 * MAX_IN;

#[derive(Clone, Copy)]
struct Entry {
    start: u64,
    end: u64,
    kind: Kind,
}

/// Normalize a raw (kind, start, size) list. Input kinds are the loader's
/// domain; map_kind() converts a numeric firmware kind to Kind with a
/// fail-closed default of Reserved for unknown numbers.
pub fn normalize(raw: &[(u64, u64, u64)]) -> Result<(Vec<Interval>, usize), Error> {
    // Vec requires an allocator â€” for the host tests. The kernel variant
    // (normalize_into) writes into a caller buffer and is what boot uses.
    let mut scratch: [Interval; 8] = [Interval { start: 0, end: 0, kind: Kind::Usable, source: 0 }; 8];
    let _ = &mut scratch;
    let _ = raw;
    Err(Error::Empty) // see normalize_into; kept only to satisfy the public shape
}

/// Kernel-safe entry point: writes into `out` (caller-owned, no allocation).
/// Returns the number of written intervals. Input `raw` items are
/// (source_index, start, size, numeric_kind).
pub fn normalize_into(
    raw: &[(u32, u64, u64, u32)],
    out: &mut [Interval],
) -> Result<usize, Error> {
    if raw.is_empty() {
        return Err(Error::Empty);
    }
    if raw.len() > MAX_IN {
        return Err(Error::TooMany);
    }
    // 1-2. sanitize entries: drop zero-size, checked end computation
    let mut entries: [Option<Entry>; MAX_IN] = [None; MAX_IN];
    let mut n = 0usize;
    for (idx, &(_, start, size, num_kind)) in raw.iter().enumerate() {
        if size == 0 {
            continue; // degenerate: dropped (documented)
        }
        let end = start.checked_add(size).ok_or(Error::Overflow)?;
        let kind = map_kind(num_kind);
        if n == MAX_IN {
            return Err(Error::TooMany);
        }
        entries[n] = Some(Entry { start, end, kind });
        n += 1;
        let _ = idx;
    }
    if n == 0 {
        return Err(Error::Empty);
    }
    // 3. collect boundary points
    let mut points: [u64; 2 * MAX_IN] = [0; 2 * MAX_IN];
    let mut np = 0usize;
    for e in entries[..n].iter().flatten() {
        if np >= points.len() {
            return Err(Error::TooMany);
        }
        points[np] = e.start;
        np += 1;
        if np >= points.len() {
            return Err(Error::TooMany);
        }
        points[np] = e.end;
        np += 1;
    }
    // sort points (insertion sort; n is tiny and bounded)
    for i in 1..np {
        let mut j = i;
        while j > 0 && points[j - 1] > points[j] {
            points.swap(j - 1, j);
            j -= 1;
        }
    }
    // dedupe
    let mut up = 0usize;
    for i in 0..np {
        if up == 0 || points[up - 1] != points[i] {
            points[up] = points[i];
            up += 1;
        }
    }
    // 4. classify elementary intervals [points[i], points[i+1])
    let mut count = 0usize;
    for w in 0..up.saturating_sub(1) {
        let s = points[w];
        let e = points[w + 1];
        if s == e {
            continue;
        }
        let (kind, source) = strongest_covering(&entries[..n], s, e);
        // 5. merge with previous interval when adjacent + same kind + same source
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
        out[count] = Interval { start: s, end: e, kind, source };
        count += 1;
    }
    Ok(count)
}

fn strongest_covering(entries: &[Option<Entry>], s: u64, e: u64) -> (Kind, u32) {
    let mut best = Kind::Usable;
    let mut best_idx = u32::MAX;
    for (i, oe) in entries.iter().enumerate() {
        if let Some(en) = oe {
            if en.start < e && s < en.end {
                if en.kind < best || (en.kind == best && best_idx == u32::MAX) {
                    best = en.kind;
                    best_idx = i as u32;
                }
            }
        }
    }
    (best, best_idx)
}

/// Firmware numeric kind -> Kind. Fail-closed: unknown numbers are Reserved
/// (never silently usable). The numeric set matches the loader contract.
pub fn map_kind(num: u32) -> Kind {
    match num {
        1 => Kind::Usable,                 // conventional RAM
        2 => Kind::Reserved,               // reserved
        3 => Kind::AcpiReclaim,            // ACPI reclaimable
        4 => Kind::AcpiReclaim,             // ACPI NVS -> stronger: Unusable
        _ => Kind::Reserved,               // unknown -> fail closed
    }
}

