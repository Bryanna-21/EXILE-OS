//! Fixed-capacity frame-range allocator; tracks physical ownership but never dereferences frames.
#![no_std]
#[cfg(test)]
extern crate std;
const PAGE: u64 = 4096;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRange {
    pub start: u64,
    pub pages: u64,
}
impl FrameRange {
    pub fn new(start: u64, pages: u64) -> Result<Self, FrameError> {
        if start == 0 || start & 4095 != 0 || pages == 0 {
            return Err(FrameError::InvalidRange);
        }
        start
            .checked_add(pages.checked_mul(PAGE).ok_or(FrameError::Overflow)?)
            .ok_or(FrameError::Overflow)?;
        Ok(Self { start, pages })
    }
    pub fn end(self) -> u64 {
        self.start + self.pages * PAGE
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    InvalidRange,
    Overflow,
    Capacity,
    OutOfMemory,
    UnknownAllocation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameToken {
    pub start: u64,
    pub pages: u64,
    generation: u32,
}
#[derive(Debug, Clone, Copy)]
struct Used {
    token: FrameToken,
}
pub struct FrameAllocator<const R: usize, const A: usize> {
    free: [Option<FrameRange>; R],
    len: usize,
    used: [Option<Used>; A],
    generation: u32,
}
impl<const R: usize, const A: usize> FrameAllocator<R, A> {
    pub const fn new() -> Self {
        Self {
            free: [None; R],
            len: 0,
            used: [None; A],
            generation: 1,
        }
    }
    pub fn add_range(&mut self, r: FrameRange) -> Result<(), FrameError> {
        let r = FrameRange::new(r.start, r.pages)?;
        if self
            .used
            .iter()
            .flatten()
            .any(|x| r.start < x.token.start + x.token.pages * PAGE && x.token.start < r.end())
        {
            return Err(FrameError::InvalidRange);
        }
        self.insert_free(r)
    }
    fn remove_free(&mut self, i: usize) {
        self.free.copy_within(i + 1..self.len, i);
        self.len -= 1;
        self.free[self.len] = None;
    }
    fn sort(&mut self) {
        for i in 1..self.len {
            let mut j = i;
            while j > 0 && self.free[j].unwrap().start < self.free[j - 1].unwrap().start {
                self.free.swap(j, j - 1);
                j -= 1;
            }
        }
    }
    fn insert_free(&mut self, mut r: FrameRange) -> Result<(), FrameError> {
        let mut i = 0;
        while i < self.len {
            let x = self.free[i].ok_or(FrameError::InvalidRange)?;
            if r.start < x.end() && x.start < r.end() {
                return Err(FrameError::InvalidRange);
            }
            if x.end() == r.start {
                r = FrameRange::new(
                    x.start,
                    x.pages.checked_add(r.pages).ok_or(FrameError::Overflow)?,
                )?;
                self.remove_free(i);
                i = 0;
                continue;
            }
            if r.end() == x.start {
                r = FrameRange::new(
                    r.start,
                    r.pages.checked_add(x.pages).ok_or(FrameError::Overflow)?,
                )?;
                self.remove_free(i);
                i = 0;
                continue;
            }
            i += 1
        }
        if self.len == R {
            return Err(FrameError::Capacity);
        }
        self.free[self.len] = Some(r);
        self.len += 1;
        self.sort();
        Ok(())
    }
    pub fn allocate(&mut self, n: u64) -> Result<FrameToken, FrameError> {
        if n == 0 {
            return Err(FrameError::InvalidRange);
        }
        let slot = self
            .used
            .iter()
            .position(Option::is_none)
            .ok_or(FrameError::Capacity)?;
        let i = self.free[..self.len]
            .iter()
            .position(|r| r.is_some_and(|x| x.pages >= n))
            .ok_or(FrameError::OutOfMemory)?;
        let mut r = self.free[i].unwrap();
        let t = FrameToken {
            start: r.start,
            pages: n,
            generation: self.generation,
        };
        self.generation = self.generation.wrapping_add(1).max(1);
        r.start = r
            .start
            .checked_add(n.checked_mul(PAGE).ok_or(FrameError::Overflow)?)
            .ok_or(FrameError::Overflow)?;
        r.pages -= n;
        if r.pages == 0 {
            self.free.copy_within(i + 1..self.len, i);
            self.len -= 1;
            self.free[self.len] = None
        } else {
            self.free[i] = Some(r)
        }
        self.used[slot] = Some(Used { token: t });
        Ok(t)
    }
    pub fn release(&mut self, t: FrameToken) -> Result<(), FrameError> {
        let i = self
            .used
            .iter()
            .position(|x| x.is_some_and(|a| a.token == t))
            .ok_or(FrameError::UnknownAllocation)?;
        self.insert_free(FrameRange::new(t.start, t.pages)?)?;
        self.used[i] = None;
        Ok(())
    }
    pub fn free_ranges(&self) -> impl Iterator<Item = FrameRange> + '_ {
        self.free[..self.len].iter().filter_map(|x| *x)
    }
}
impl<const R: usize, const A: usize> Default for FrameAllocator<R, A> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allocate_and_coalesce() {
        let mut a = FrameAllocator::<4, 4>::new();
        a.add_range(FrameRange::new(0x1000, 8).unwrap()).unwrap();
        let x = a.allocate(3).unwrap();
        let y = a.allocate(2).unwrap();
        assert_eq!(y.start, 0x4000);
        a.release(x).unwrap();
        a.release(y).unwrap();
        assert_eq!(
            a.free_ranges().collect::<std::vec::Vec<_>>(),
            std::vec![FrameRange {
                start: 0x1000,
                pages: 8
            }]
        );
    }
    #[test]
    fn duplicate_and_stale() {
        let mut a = FrameAllocator::<2, 2>::new();
        a.add_range(FrameRange::new(0x1000, 2).unwrap()).unwrap();
        assert_eq!(
            a.add_range(FrameRange::new(0x2000, 2).unwrap()),
            Err(FrameError::InvalidRange)
        );
        let t = a.allocate(1).unwrap();
        a.release(t).unwrap();
        assert_eq!(a.release(t), Err(FrameError::UnknownAllocation));
    }
    #[test]
    fn coalesces_when_free_metadata_is_full() {
        let mut a = FrameAllocator::<1, 1>::new();
        a.add_range(FrameRange::new(0x1000, 2).unwrap()).unwrap();
        let t = a.allocate(1).unwrap();
        a.add_range(FrameRange::new(0x3000, 1).unwrap()).unwrap();
        a.release(t).unwrap();
        assert_eq!(
            a.free_ranges().collect::<std::vec::Vec<_>>(),
            std::vec![FrameRange {
                start: 0x1000,
                pages: 3
            }]
        );
    }
    #[test]
    fn overflow_and_capacity() {
        assert_eq!(FrameRange::new(0x1000, u64::MAX), Err(FrameError::Overflow));
        let mut a = FrameAllocator::<1, 1>::new();
        a.add_range(FrameRange::new(0x1000, 1).unwrap()).unwrap();
        assert_eq!(a.allocate(2), Err(FrameError::OutOfMemory));
        let t = a.allocate(1).unwrap();
        assert_eq!(a.allocate(1), Err(FrameError::Capacity));
        a.release(t).unwrap();
    }
}
