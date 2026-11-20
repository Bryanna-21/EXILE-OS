//! Original bounded Multiboot map algorithms. No allocation or pointer dereferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    RecordSize,
    Overflow,
    Empty,
    TooMany,
    Overlap,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub kind: u32,
}
impl Region {
    pub fn new(start: u64, length: u64, kind: u32) -> Result<Self, Error> {
        if length == 0 {
            return Err(Error::Empty);
        }
        Ok(Self {
            start,
            end: start.checked_add(length).ok_or(Error::Overflow)?,
            kind,
        })
    }
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([
        b[i],
        b[i + 1],
        b[i + 2],
        b[i + 3],
        b[i + 4],
        b[i + 5],
        b[i + 6],
        b[i + 7],
    ])
}
pub fn parse_map(bytes: &[u8], out: &mut [Region]) -> Result<usize, Error> {
    let mut offset = 0usize;
    let mut count = 0;
    while offset < bytes.len() {
        if bytes.len() - offset < 4 {
            return Err(Error::Truncated);
        }
        let size = u32_at(bytes, offset) as usize;
        if size < 20 {
            return Err(Error::RecordSize);
        }
        let next = offset
            .checked_add(4)
            .and_then(|x| x.checked_add(size))
            .ok_or(Error::Overflow)?;
        if next > bytes.len() {
            return Err(Error::Truncated);
        }
        let region = Region::new(
            u64_at(bytes, offset + 4),
            u64_at(bytes, offset + 12),
            u32_at(bytes, offset + 20),
        )?;
        if count == out.len() {
            return Err(Error::TooMany);
        }
        if out[..count]
            .iter()
            .any(|r| region.start < r.end && r.start < region.end)
        {
            return Err(Error::Overlap);
        }
        out[count] = region;
        count += 1;
        offset = next;
    }
    if count == 0 {
        return Err(Error::Empty);
    }
    Ok(count)
}
/// Returns an aligned usable range, clipped above a reserved prefix and below a ceiling.
/// Not a live allocator: loader/kernel/module reservations must be accounted for before use.
pub fn usable_pages(region: Region, reserved_end: u64, ceiling: u64) -> Option<(u64, u64)> {
    if region.kind != 1 {
        return None;
    }
    let start = region.start.max(reserved_end).checked_add(4095)? & !4095;
    let end = region.end.min(ceiling) & !4095;
    if start >= end {
        return None;
    }
    Some((start, (end - start) / 4096))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record(start: u64, len: u64, kind: u32) -> Vec<u8> {
        let mut v = 20u32.to_le_bytes().to_vec();
        v.extend(start.to_le_bytes());
        v.extend(len.to_le_bytes());
        v.extend(kind.to_le_bytes());
        v
    }
    fn parse(b: &[u8]) -> Result<usize, Error> {
        parse_map(
            b,
            &mut [Region {
                start: 0,
                end: 0,
                kind: 0,
            }; 4],
        )
    }
    #[test]
    fn valid() {
        assert_eq!(parse(&record(4096, 8192, 1)), Ok(1));
    }
    #[test]
    fn empty_map() {
        assert_eq!(parse(&[]), Err(Error::Empty));
    }
    #[test]
    fn short_header() {
        assert_eq!(parse(&[0; 3]), Err(Error::Truncated));
    }
    #[test]
    fn short_record() {
        assert_eq!(parse(&record(0, 4096, 1)[..23]), Err(Error::Truncated));
    }
    #[test]
    fn small_record() {
        assert_eq!(parse(&19u32.to_le_bytes()), Err(Error::RecordSize));
    }
    #[test]
    fn huge_record() {
        assert_eq!(parse(&u32::MAX.to_le_bytes()), Err(Error::Truncated));
    }
    #[test]
    fn zero_range() {
        assert_eq!(parse(&record(0, 0, 1)), Err(Error::Empty));
    }
    #[test]
    fn overflowing_range() {
        assert_eq!(parse(&record(u64::MAX, 1, 1)), Err(Error::Overflow));
    }
    #[test]
    fn overlap() {
        let mut b = record(0, 8192, 1);
        b.extend(record(4096, 8192, 2));
        assert_eq!(parse(&b), Err(Error::Overlap));
    }
    #[test]
    fn adjacent() {
        let mut b = record(0, 4096, 1);
        b.extend(record(4096, 4096, 2));
        assert_eq!(parse(&b), Ok(2));
    }
    #[test]
    fn unordered() {
        let mut b = record(8192, 4096, 1);
        b.extend(record(0, 4096, 2));
        assert_eq!(parse(&b), Ok(2));
    }
    #[test]
    fn bounded_capacity() {
        assert_eq!(parse_map(&record(0, 4096, 1), &mut []), Err(Error::TooMany));
    }
    #[test]
    fn extended_record() {
        let mut b = record(0, 4096, 1);
        b[..4].copy_from_slice(&24u32.to_le_bytes());
        b.extend([0; 4]);
        assert_eq!(parse(&b), Ok(1));
    }
    #[test]
    fn reserved_kind() {
        assert_eq!(
            usable_pages(Region::new(0, 8192, 2).unwrap(), 0, 8192),
            None
        );
    }
    #[test]
    fn page_alignment() {
        assert_eq!(
            usable_pages(Region::new(1, 12287, 1).unwrap(), 0, 12288),
            Some((4096, 2))
        );
    }
    #[test]
    fn reserved_prefix() {
        assert_eq!(
            usable_pages(Region::new(0, 16384, 1).unwrap(), 8192, 16384),
            Some((8192, 2))
        );
    }
    #[test]
    fn ceiling() {
        assert_eq!(
            usable_pages(Region::new(0, 16384, 1).unwrap(), 0, 8192),
            Some((0, 2))
        );
    }
    #[test]
    fn alignment_overflow() {
        assert_eq!(
            usable_pages(Region::new(u64::MAX - 1, 1, 1).unwrap(), 0, u64::MAX),
            None
        );
    }
}

// -----------------------------------------------------------------------------
// Bounded physical frame allocator
// -----------------------------------------------------------------------------
// Tracks physical memory below 1 GiB. Bit 1 means a frame belongs to the
// usable pool; bit 1 in `free` means it is currently available for allocation.
// Frame zero is never allocated.

pub const FRAME_SIZE: u64 = 4096;
pub const FRAME_LIMIT: u64 = 1024 * 1024 * 1024;
pub const MAX_FRAMES: usize = (FRAME_LIMIT / FRAME_SIZE) as usize;
const BITMAP_WORDS: usize = MAX_FRAMES / 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    InvalidRange,
    Unmanaged,
    DoubleFree,
}

pub struct FrameAllocator {
    pool: [u64; BITMAP_WORDS],
    free: [u64; BITMAP_WORDS],
    hint: usize,
    free_count: usize,
    pool_count: usize,
}

impl FrameAllocator {
    pub const fn new() -> Self {
        Self {
            pool: [0; BITMAP_WORDS],
            free: [0; BITMAP_WORDS],
            hint: 1,
            free_count: 0,
            pool_count: 0,
        }
    }

    #[inline]
    fn bit(index: usize) -> (usize, u64) {
        (index / 64, 1u64 << (index % 64))
    }

    #[inline]
    fn has(bitmap: &[u64; BITMAP_WORDS], index: usize) -> bool {
        let (word, mask) = Self::bit(index);
        bitmap[word] & mask != 0
    }

    #[inline]
    fn set(bitmap: &mut [u64; BITMAP_WORDS], index: usize) {
        let (word, mask) = Self::bit(index);
        bitmap[word] |= mask;
    }

    #[inline]
    fn clear(bitmap: &mut [u64; BITMAP_WORDS], index: usize) {
        let (word, mask) = Self::bit(index);
        bitmap[word] &= !mask;
    }

    /// Adds only complete, page-aligned usable frames within the allocator limit.
    /// Repeated or overlapping additions do not inflate the counters.
    pub fn add_region(&mut self, region: Region) {
        if region.kind != 1 {
            return;
        }

        let Some(start) = region.start.checked_add(FRAME_SIZE - 1) else {
            return;
        };
        let start = start & !(FRAME_SIZE - 1);
        let end = region.end.min(FRAME_LIMIT) & !(FRAME_SIZE - 1);

        if start >= end {
            return;
        }

        let first = (start / FRAME_SIZE) as usize;
        let last = (end / FRAME_SIZE) as usize;

        for index in first.max(1)..last.min(MAX_FRAMES) {
            if !Self::has(&self.pool, index) {
                Self::set(&mut self.pool, index);
                Self::set(&mut self.free, index);
                self.pool_count += 1;
                self.free_count += 1;
            }
        }
    }

    /// Reserves every frame touched by [start, start + length).
    /// This is deliberately rounded outward so partial pages are never exposed.
    pub fn reserve_range(
        &mut self,
        start: u64,
        length: u64,
    ) -> Result<(), FrameError> {
        if length == 0 {
            return Err(FrameError::InvalidRange);
        }

        let end = start
            .checked_add(length)
            .ok_or(FrameError::InvalidRange)?;

        let rounded_end = end
            .checked_add(FRAME_SIZE - 1)
            .ok_or(FrameError::InvalidRange)?
            & !(FRAME_SIZE - 1);

        let first = (start / FRAME_SIZE) as usize;
        let last = (rounded_end.min(FRAME_LIMIT) / FRAME_SIZE) as usize;

        for index in first.max(1)..last.min(MAX_FRAMES) {
            if Self::has(&self.pool, index) {
                Self::clear(&mut self.pool, index);
                self.pool_count -= 1;

                if Self::has(&self.free, index) {
                    Self::clear(&mut self.free, index);
                    self.free_count -= 1;
                }
            }
        }

        Ok(())
    }

    /// Allocates one 4 KiB frame, returning its physical address.
    pub fn alloc(&mut self) -> Option<u64> {
        if self.free_count == 0 {
            return None;
        }

        let start = self.hint.max(1).min(MAX_FRAMES - 1);

        for index in (start..MAX_FRAMES).chain(1..start) {
            if Self::has(&self.free, index) {
                Self::clear(&mut self.free, index);
                self.free_count -= 1;
                self.hint = if index + 1 < MAX_FRAMES {
                    index + 1
                } else {
                    1
                };
                return Some((index as u64) * FRAME_SIZE);
            }
        }

        // Defensive consistency check: the counter must never claim that
        // frames are available when the bitmap contains none.
        self.free_count = 0;
        None
    }

    /// Returns a previously allocated frame to the pool.
    pub fn free(&mut self, address: u64) -> Result<(), FrameError> {
        if address == 0
            || address >= FRAME_LIMIT
            || address & (FRAME_SIZE - 1) != 0
        {
            return Err(FrameError::InvalidRange);
        }

        let index = (address / FRAME_SIZE) as usize;

        if !Self::has(&self.pool, index) {
            return Err(FrameError::Unmanaged);
        }
        if Self::has(&self.free, index) {
            return Err(FrameError::DoubleFree);
        }

        Self::set(&mut self.free, index);
        self.free_count += 1;
        self.hint = self.hint.min(index);
        Ok(())
    }

    pub const fn free_frames(&self) -> usize {
        self.free_count
    }

    pub const fn pool_frames(&self) -> usize {
        self.pool_count
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    fn usable(start: u64, length: u64) -> Region {
        Region::new(start, length, 1).unwrap()
    }

    #[test]
    fn allocator_starts_empty() {
        let mut allocator = FrameAllocator::new();
        assert_eq!(allocator.pool_frames(), 0);
        assert_eq!(allocator.free_frames(), 0);
        assert_eq!(allocator.alloc(), None);
    }

    #[test]
    fn adds_aligned_usable_frames() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(1, 3 * FRAME_SIZE - 1));
        assert_eq!(allocator.pool_frames(), 2);
        assert_eq!(allocator.free_frames(), 2);
    }

    #[test]
    fn ignores_non_usable_regions() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(Region::new(0, 8 * FRAME_SIZE, 2).unwrap());
        assert_eq!(allocator.pool_frames(), 0);
    }

    #[test]
    fn never_allocates_frame_zero() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(0, 3 * FRAME_SIZE));
        assert_eq!(allocator.alloc(), Some(FRAME_SIZE));
    }

    #[test]
    fn allocations_are_unique_and_counted() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(FRAME_SIZE, 3 * FRAME_SIZE));

        let first = allocator.alloc().unwrap();
        let second = allocator.alloc().unwrap();

        assert_ne!(first, second);
        assert_eq!(allocator.free_frames(), 1);
    }

    #[test]
    fn exhausted_allocator_returns_none() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(FRAME_SIZE, FRAME_SIZE));

        assert_eq!(allocator.alloc(), Some(FRAME_SIZE));
        assert_eq!(allocator.alloc(), None);
    }

    #[test]
    fn reservation_rounds_outward() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(FRAME_SIZE, 4 * FRAME_SIZE));

        allocator.reserve_range(FRAME_SIZE + 1, 1).unwrap();

        assert_eq!(allocator.pool_frames(), 3);
        assert_eq!(allocator.free_frames(), 3);
    }

    #[test]
    fn reservation_removes_already_allocated_frame() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(FRAME_SIZE, 2 * FRAME_SIZE));

        let allocated = allocator.alloc().unwrap();
        allocator.reserve_range(allocated, FRAME_SIZE).unwrap();

        assert_eq!(allocator.pool_frames(), 1);
        assert_eq!(allocator.free_frames(), 1);
    }

    #[test]
    fn free_restores_frame() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(FRAME_SIZE, FRAME_SIZE));

        let frame = allocator.alloc().unwrap();
        allocator.free(frame).unwrap();

        assert_eq!(allocator.free_frames(), 1);
    }

    #[test]
    fn rejects_double_free() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(FRAME_SIZE, FRAME_SIZE));

        let frame = allocator.alloc().unwrap();
        allocator.free(frame).unwrap();

        assert_eq!(allocator.free(frame), Err(FrameError::DoubleFree));
    }

    #[test]
    fn rejects_unmanaged_free() {
        let mut allocator = FrameAllocator::new();
        assert_eq!(
            allocator.free(8 * FRAME_SIZE),
            Err(FrameError::Unmanaged)
        );
    }

    #[test]
    fn rejects_unaligned_and_zero_free() {
        let mut allocator = FrameAllocator::new();
        assert_eq!(allocator.free(0), Err(FrameError::InvalidRange));
        assert_eq!(allocator.free(FRAME_SIZE + 1), Err(FrameError::InvalidRange));
    }

    #[test]
    fn rejects_reservation_overflow() {
        let mut allocator = FrameAllocator::new();
        assert_eq!(
            allocator.reserve_range(u64::MAX - 10, 20),
            Err(FrameError::InvalidRange)
        );
    }

    #[test]
    fn duplicate_region_does_not_inflate_counts() {
        let mut allocator = FrameAllocator::new();
        let region = usable(FRAME_SIZE, 4 * FRAME_SIZE);

        allocator.add_region(region);
        allocator.add_region(region);

        assert_eq!(allocator.pool_frames(), 4);
        assert_eq!(allocator.free_frames(), 4);
    }

    #[test]
    fn clips_regions_at_one_gibibyte() {
        let mut allocator = FrameAllocator::new();
        allocator.add_region(usable(FRAME_LIMIT - FRAME_SIZE, 4 * FRAME_SIZE));

        assert_eq!(allocator.pool_frames(), 1);
        assert_eq!(allocator.alloc(), Some(FRAME_LIMIT - FRAME_SIZE));
    }
}
