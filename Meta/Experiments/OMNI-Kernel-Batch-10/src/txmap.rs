//! Transactional page mapping (SEC-14) and page-table tree teardown.
//!
//! Batch 8's `Mapper` can fail half way (no frame, already mapped, ...). It then leaves the
//! leaf entries it already wrote and any intermediate table frames it already linked. This
//! module wraps the Mapper without modifying it:
//!
//!  * every `write_u64` the Mapper performs is journaled with the previous value;
//!  * every frame it allocates is recorded;
//!  * on failure the journal is replayed in reverse (restoring even the USER-bit upgrades of
//!    pre-existing tables) and the recorded frames are returned to the allocator.
//!
//! Capacity is fixed (no heap). A request that could exceed it is refused BEFORE touching
//! anything, so rollback can never be incomplete.
//!
//! Limits: this restores page-table memory and frame ownership only. It does not flush the
//! TLB (the caller does that after a successful change), and it is hosted-tested only.

use omni8::paging::{
    ADDR_MASK, ENTRIES, FrameSource, HUGE, HUGE_SIZE, MapError, Mapper, PAGE_SIZE, PRESENT, Perms,
    PhysMem, UserWindow,
};

/// Frames can be handed back, which `FrameSource` alone cannot do.
pub trait FrameRelease {
    fn free_frame(&mut self, phys: u64);
}

/// Maximum number of mapping units (4 KiB pages or 2 MiB pages) in one transaction.
pub const TX_MAX_UNITS: usize = 64;
// Per unit: at most 1 leaf write + 3 table-link / USER-upgrade writes; at most 3 new frames.
const WRITES_PER_UNIT: usize = 4;
const FRAMES_PER_UNIT: usize = 3;
const W: usize = TX_MAX_UNITS * WRITES_PER_UNIT;
const F: usize = TX_MAX_UNITS * FRAMES_PER_UNIT;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TxError {
    Map(MapError),
    /// More than `TX_MAX_UNITS` units requested. Nothing was changed.
    TooLarge,
}

struct Journal<'m, M: PhysMem> {
    mem: &'m mut M,
    log: [(u64, u64); W],
    n: usize,
    overflowed: bool,
}

impl<M: PhysMem> PhysMem for Journal<'_, M> {
    fn read_u64(&self, phys: u64) -> u64 {
        self.mem.read_u64(phys)
    }
    fn write_u64(&mut self, phys: u64, value: u64) {
        let old = self.mem.read_u64(phys);
        if self.n < W {
            self.log[self.n] = (phys, old);
            self.n += 1;
        } else {
            self.overflowed = true; // unreachable given the up-front size check
        }
        self.mem.write_u64(phys, value);
    }
    fn zero_frame(&mut self, phys: u64) {
        // Only ever called on a frame allocated inside this transaction; it is freed on
        // rollback, so its contents need no restoring.
        self.mem.zero_frame(phys);
    }
}

struct Tracker<'a, A: FrameSource> {
    inner: &'a mut A,
    got: [u64; F],
    n: usize,
    overflowed: bool,
}

impl<A: FrameSource> FrameSource for Tracker<'_, A> {
    fn alloc_frame(&mut self) -> Option<u64> {
        let f = self.inner.alloc_frame()?;
        if self.n < F {
            self.got[self.n] = f;
            self.n += 1;
        } else {
            self.overflowed = true; // unreachable given the up-front size check
        }
        Some(f)
    }
}

/// Number of mapping units `map_range_4k` / `map_range_best` will use, counting stops early
/// once it exceeds `TX_MAX_UNITS`. Mirrors the Mapper's own splitting rule.
fn count_units(virt: u64, phys: u64, len: u64, best: bool) -> usize {
    let mut off = 0;
    let mut n = 0;
    while off < len {
        n += 1;
        if n > TX_MAX_UNITS {
            return n;
        }
        if best
            && (virt + off) % HUGE_SIZE == 0
            && (phys + off) % HUGE_SIZE == 0
            && len - off >= HUGE_SIZE
        {
            off += HUGE_SIZE;
        } else {
            off += PAGE_SIZE;
        }
    }
    n
}

/// Map `[virt, virt+len)` to `[phys, phys+len)` with all-or-nothing semantics.
///
/// `best` selects 2 MiB pages where aligned (`map_range_best`), otherwise 4 KiB pages.
/// On `Err` the page tables and the allocator are exactly as before the call.
#[allow(clippy::too_many_arguments)]
pub fn map_range_atomic<M: PhysMem, A: FrameSource + FrameRelease>(
    mem: &mut M,
    frames: &mut A,
    root: u64,
    window: UserWindow,
    virt: u64,
    phys: u64,
    len: u64,
    perms: Perms,
    best: bool,
) -> Result<(), TxError> {
    if len % PAGE_SIZE != 0 {
        return Err(TxError::Map(MapError::Misaligned));
    }
    virt.checked_add(len)
        .ok_or(TxError::Map(MapError::Overflow))?;
    phys.checked_add(len)
        .ok_or(TxError::Map(MapError::Overflow))?;
    if count_units(virt, phys, len, best) > TX_MAX_UNITS {
        return Err(TxError::TooLarge);
    }
    let mut journal = Journal {
        mem,
        log: [(0, 0); W],
        n: 0,
        overflowed: false,
    };
    let mut tracker = Tracker {
        inner: &mut *frames,
        got: [0; F],
        n: 0,
        overflowed: false,
    };
    let result = {
        let mut mapper =
            Mapper::from_root(&mut journal, &mut tracker, root, window).map_err(TxError::Map)?;
        if best {
            mapper.map_range_best(virt, phys, len, perms)
        } else {
            mapper.map_range_4k(virt, phys, len, perms)
        }
    };
    debug_assert!(!journal.overflowed && !tracker.overflowed);
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            // Undo writes newest first, so an entry written twice ends at its original value.
            let mut i = journal.n;
            while i > 0 {
                i -= 1;
                let (slot, old) = journal.log[i];
                journal.mem.write_u64(slot, old);
            }
            let got = tracker.got;
            let n = tracker.n;
            for f in got.iter().take(n) {
                frames.free_frame(*f);
            }
            Err(TxError::Map(e))
        }
    }
}

/// Free every page-table frame under `root` (PML4, PDPT, PD, PT levels) and the root itself.
/// Leaf data frames are NOT freed: the caller owns them. Returns the number of table frames
/// freed. Present entries that are not valid table pointers are skipped, not followed.
pub fn destroy_tables<M: PhysMem, A: FrameRelease>(
    mem: &mut M,
    frames: &mut A,
    root: u64,
    phys_limit: u64,
) -> usize {
    fn walk<M: PhysMem, A: FrameRelease>(
        mem: &mut M,
        frames: &mut A,
        table: u64,
        level: u8,
        limit: u64,
    ) -> usize {
        let mut freed = 0;
        // Levels: 4 = PML4, 3 = PDPT, 2 = PD, 1 = PT. A PT holds only leaves.
        if level > 1 {
            for i in 0..ENTRIES {
                let e = mem.read_u64(table + i as u64 * 8);
                if e & PRESENT == 0 || e & HUGE != 0 {
                    continue; // absent, or a 1 GiB/2 MiB leaf
                }
                let child = e & ADDR_MASK;
                if child == 0 || child % PAGE_SIZE != 0 || child >= limit {
                    continue;
                }
                freed += walk(mem, frames, child, level - 1, limit);
            }
        }
        frames.free_frame(table);
        freed + 1
    }
    if root == 0 || root % PAGE_SIZE != 0 || root >= phys_limit {
        return 0;
    }
    walk(mem, frames, root, 4, phys_limit)
}
