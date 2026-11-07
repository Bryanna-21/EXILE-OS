//! Physical frame allocator: two bitmaps (pool membership and free state) over the first
//! `MAX_FRAMES` frames (1 GiB). First-fit with a rolling hint. Not thread-safe by itself;
//! callers serialise (the boot path is single-CPU with interrupts masked).

use crate::paging::FrameSource;

pub const FRAME_SIZE: u64 = 4096;
pub const MAX_FRAMES: usize = 262_144;
const WORDS: usize = MAX_FRAMES / 64;
/// Highest physical address (exclusive) this allocator can manage.
pub const CEILING: u64 = MAX_FRAMES as u64 * FRAME_SIZE;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameError {
    OutOfRange,
    Misaligned,
    /// The frame is not currently allocated (already free, or never part of the pool).
    NotAllocated,
}

pub struct FrameAllocator {
    pool: [u64; WORDS],
    free: [u64; WORDS],
    free_count: usize,
    hint: usize,
}

impl Default for FrameAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameAllocator {
    pub const fn new() -> Self {
        FrameAllocator {
            pool: [0; WORDS],
            free: [0; WORDS],
            free_count: 0,
            hint: 0,
        }
    }

    fn bit(words: &[u64; WORDS], i: usize) -> bool {
        words[i / 64] & (1 << (i % 64)) != 0
    }
    fn set(words: &mut [u64; WORDS], i: usize) {
        words[i / 64] |= 1 << (i % 64);
    }
    fn clear(words: &mut [u64; WORDS], i: usize) {
        words[i / 64] &= !(1 << (i % 64));
    }

    /// Add `[start, end)` to the pool, narrowed inward to frame boundaries and clipped to
    /// `CEILING`. Frames already in the pool are left as they are. Returns frames added.
    pub fn add_range(&mut self, start: u64, end: u64) -> usize {
        let start = match start.checked_add(FRAME_SIZE - 1) {
            Some(s) => s / FRAME_SIZE,
            None => return 0,
        };
        let end = end.min(CEILING) / FRAME_SIZE;
        let mut added = 0;
        let mut i = start;
        while i < end {
            let f = i as usize;
            if !Self::bit(&self.pool, f) {
                Self::set(&mut self.pool, f);
                Self::set(&mut self.free, f);
                self.free_count += 1;
                added += 1;
            }
            i += 1;
        }
        added
    }

    /// Remove `[start, end)`, widened outward to frame boundaries, from the pool. Frames that
    /// were free stop being free; allocated frames simply leave the pool. Returns the
    /// number of frames removed from the pool.
    pub fn reserve_range(&mut self, start: u64, end: u64) -> usize {
        if start >= end {
            return 0;
        }
        let first = start / FRAME_SIZE;
        let last = match end.checked_add(FRAME_SIZE - 1) {
            Some(e) => (e / FRAME_SIZE).min(MAX_FRAMES as u64),
            None => MAX_FRAMES as u64,
        };
        let mut removed = 0;
        let mut i = first;
        while i < last {
            let f = i as usize;
            if Self::bit(&self.pool, f) {
                if Self::bit(&self.free, f) {
                    Self::clear(&mut self.free, f);
                    self.free_count -= 1;
                }
                Self::clear(&mut self.pool, f);
                removed += 1;
            }
            i += 1;
        }
        removed
    }

    pub fn free_frames(&self) -> usize {
        self.free_count
    }

    pub fn pool_frames(&self) -> usize {
        self.pool.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn is_free(&self, addr: u64) -> bool {
        let f = (addr / FRAME_SIZE) as usize;
        addr % FRAME_SIZE == 0 && f < MAX_FRAMES && Self::bit(&self.free, f)
    }

    pub fn alloc(&mut self) -> Option<u64> {
        if self.free_count == 0 {
            return None;
        }
        for step in 0..MAX_FRAMES {
            let f = (self.hint + step) % MAX_FRAMES;
            if Self::bit(&self.free, f) {
                Self::clear(&mut self.free, f);
                self.free_count -= 1;
                self.hint = f + 1;
                return Some(f as u64 * FRAME_SIZE);
            }
        }
        None
    }

    pub fn free(&mut self, addr: u64) -> Result<(), FrameError> {
        if addr % FRAME_SIZE != 0 {
            return Err(FrameError::Misaligned);
        }
        let f = (addr / FRAME_SIZE) as usize;
        if f >= MAX_FRAMES {
            return Err(FrameError::OutOfRange);
        }
        if !Self::bit(&self.pool, f) || Self::bit(&self.free, f) {
            return Err(FrameError::NotAllocated);
        }
        Self::set(&mut self.free, f);
        self.free_count += 1;
        if f < self.hint {
            self.hint = f;
        }
        Ok(())
    }
}

impl FrameSource for FrameAllocator {
    fn alloc_frame(&mut self) -> Option<u64> {
        self.alloc()
    }
}
