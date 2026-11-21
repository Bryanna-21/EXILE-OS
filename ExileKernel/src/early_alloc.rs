//! Bounded early physical allocator (Mission A, req. 6).
//! Scope is deliberately narrow (mission rule: no scheduler/frames here):
//!   * operates ONLY on normalized Usable intervals, minus required
  //!     reservations (kernel image, boot info, modules, tables, MMIO...);
//!   * alloc-only: NO free() at this stage â€” pages handed out during boot
//!     are boot-lifetime-owned. Release support arrives with the real frame
//!     allocator (documented handoff point: frames.rs from later missions or
//!     the WO-2 tree; this module exposes `take_remaining()` for it);
//!   * fail-closed: exhaustion returns None; misaligned/huge requests Err;
//!   * 2 MiB allocation granularity by default (page-table-friendly),
//!     4 KiB granularity optional; caller states alignment explicitly;
//!   * checked arithmetic everywhere; no unbounded state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotInitialized,
    InvalidAlignment,
    OutOfBounds,
    Exhausted,
}

pub const GRAN_2M: u64 = 2 * 1024 * 1024;
pub const GRAN_4K: u64 = 4096;

/// Reservations the caller must apply BEFORE init (Mission A, req. 5).
#[derive(Clone, Copy)]
pub struct Reservation {
    pub start: u64,
    pub end: u64,
}

pub struct EarlyAlloc {
    intervals: [Interval; 32],
    count: usize,
    cursor: usize,     // current interval index
    next: u64,         // next candidate base within the current interval
}

#[derive(Clone, Copy)]
struct Interval {
    start: u64,
    end: u64, // exclusive
}

impl EarlyAlloc {
    /// Build from normalized Usable intervals with reservations subtracted.
    /// The caller passes ONLY Kind::Usable intervals; reservations must be
    /// subtracted by the caller (usually by splitting them out of the map).
    pub fn new(usable: &[(u64, u64)], reservations: &[Reservation]) -> Result<Self, Error> {
        if usable.is_empty() {
            return Err(Error::NotInitialized);
        }
        let mut ints: [Interval; 32] = [Interval { start: 0, end: 0 }; 32];
        let mut count = 0usize;
        for &(s, e) in usable {
            if s >= e {
                continue;
            }
            if count == ints.len() {
                return Err(Error::OutOfBounds);
            }
            ints[count] = Interval { start: s, end: e };
            count += 1;
        }
        let mut a = EarlyAlloc { intervals: ints, count, cursor: 0, next: if count > 0 { ints[0].start } else { 0 } };
        // subtract reservations: split intervals around each reservation
        for r in reservations {
            if r.start >= r.end {
                continue; // degenerate reservation: ignored
            }
            a.subtract(r.start, r.end)?;
        }
        // reset cursor to the first interval that still has space
        a.cursor = a.first_nonempty().ok_or(Error::Exhausted)?;
        a.next = a.intervals[a.cursor].start;
        Ok(a)
    }

    fn first_nonempty(&self) -> Option<usize> {
        for i in 0..self.count {
            if self.intervals[i].start < self.intervals[i].end {
                return Some(i);
            }
        }
        None
    }

    fn subtract(&mut self, rs: u64, re: u64) -> Result<(), Error> {
        for i in 0..self.count {
            let s = self.intervals[i].start;
            let e = self.intervals[i].end;
            if re <= s || rs >= e {
                continue; // disjoint
            }
            if rs <= s && re >= e {
                self.intervals[i] = Interval { start: s, end: s }; // fully removed
            } else if rs <= s {
                self.intervals[i] = Interval { start: re, end: e };
            } else if re >= e {
                self.intervals[i] = Interval { start: s, end: rs };
            } else {
                // splits into two: right side needs a free slot
                if self.count == self.intervals.len() {
                    return Err(Error::OutOfBounds);
                }
                self.intervals[self.count] = Interval { start: re, end: e };
                self.count += 1;
                self.intervals[i] = Interval { start: s, end: rs };
            }
        }
        Ok(())
    }

    /// Allocate `size` bytes aligned to `align` (power of two). Returns the
    /// physical base. Fail-closed on exhaustion and invalid inputs.
    pub fn alloc(&mut self, size: u64, align: u64) -> Result<u64, Error> {
        if size == 0 || !align.is_power_of_two() || align > GRAN_2M {
            return Err(Error::InvalidAlignment);
        }
        let mut tried = 0usize;
        loop {
            let cur = self.intervals[self.cursor];
            let base = self.next.checked_add(align - 1).ok_or(Error::Exhausted)? & !(align - 1);
            let end = base.checked_add(size).ok_or(Error::Exhausted)?;
            if end <= cur.end {
                self.next = end;
                return Ok(base);
            }
            // advance to next interval with space
            self.cursor += 1;
            if self.cursor >= self.count {
                self.cursor = 0;
                tried += 1;
                if tried > 1 {
                    return Err(Error::Exhausted);
                }
            }
            self.next = self.intervals[self.cursor].start;
        }
    }

    /// Remaining bytes across all intervals (for handoff to the real frame
    /// allocator: later missions consume `remaining_intervals()`).
    pub fn remaining(&self) -> u64 {
        let mut total = 0u64;
        for i in 0..self.count {
            let s = if i == self.cursor { self.next } else { self.intervals[i].start };
            if s < self.intervals[i].end {
                total += self.intervals[i].end - s;
            }
        }
        total
    }

    /// Handoff point: remaining usable intervals after early boot allocations.
    pub fn remaining_intervals(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        (0..self.count).filter_map(move |i| {
            let s = if i == self.cursor { self.next } else { self.intervals[i].start };
            if s < self.intervals[i].end { Some((s, self.intervals[i].end)) } else { None }
        })
    }
}
