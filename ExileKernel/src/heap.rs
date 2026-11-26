//! Offset-based bounded heap arena; does not dereference backing memory or install a global allocator.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    pub offset: usize,
    pub size: usize,
    generation: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapError {
    ZeroSize,
    BadAlignment,
    Overflow,
    OutOfMemory,
    MetadataFull,
    InvalidFree,
    AlreadyInitialized,
}
#[derive(Debug, Clone, Copy)]
struct Span {
    offset: usize,
    size: usize,
}
pub struct Heap<const N: usize> {
    capacity: usize,
    free: [Option<Span>; N],
    free_len: usize,
    live: [Option<Allocation>; N],
    generation: u32,
    initialized: bool,
}
impl<const N: usize> Heap<N> {
    pub const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            free: [None; N],
            free_len: 0,
            live: [None; N],
            generation: 1,
            initialized: false,
        }
    }
    pub fn init(&mut self) -> Result<(), HeapError> {
        if self.initialized {
            return Err(HeapError::AlreadyInitialized);
        }
        if self.capacity == 0 {
            return Err(HeapError::ZeroSize);
        }
        if N == 0 {
            return Err(HeapError::MetadataFull);
        }
        self.free[0] = Some(Span {
            offset: 0,
            size: self.capacity,
        });
        self.free_len = 1;
        self.initialized = true;
        Ok(())
    }
    pub fn allocate(&mut self, n: usize, align: usize) -> Result<Allocation, HeapError> {
        if n == 0 {
            return Err(HeapError::ZeroSize);
        }
        if align == 0 || !align.is_power_of_two() {
            return Err(HeapError::BadAlignment);
        }
        let slot = self
            .live
            .iter()
            .position(Option::is_none)
            .ok_or(HeapError::MetadataFull)?;
        for i in 0..self.free_len {
            let s = self.free[i].unwrap();
            let a = s.offset.checked_add(align - 1).ok_or(HeapError::Overflow)? & !(align - 1);
            let e = a.checked_add(n).ok_or(HeapError::Overflow)?;
            let se = s.offset.checked_add(s.size).ok_or(HeapError::Overflow)?;
            if e > se {
                continue;
            }
            let l = a - s.offset;
            let r = se - e;
            if l > 0 && r > 0 && self.free_len == N {
                return Err(HeapError::MetadataFull);
            }
            let out = Allocation {
                offset: a,
                size: n,
                generation: self.generation,
            };
            self.generation = self.generation.wrapping_add(1).max(1);
            match (l > 0, r > 0) {
                (true, true) => {
                    self.free[i] = Some(Span {
                        offset: s.offset,
                        size: l,
                    });
                    self.free.copy_within(i + 1..self.free_len, i + 2);
                    self.free[i + 1] = Some(Span { offset: e, size: r });
                    self.free_len += 1
                }
                (true, false) => {
                    self.free[i] = Some(Span {
                        offset: s.offset,
                        size: l,
                    })
                }
                (false, true) => self.free[i] = Some(Span { offset: e, size: r }),
                (false, false) => {
                    self.free.copy_within(i + 1..self.free_len, i);
                    self.free_len -= 1;
                    self.free[self.free_len] = None
                }
            }
            self.live[slot] = Some(out);
            return Ok(out);
        }
        Err(HeapError::OutOfMemory)
    }
    fn remove_free(&mut self, i: usize) {
        self.free.copy_within(i + 1..self.free_len, i);
        self.free_len -= 1;
        self.free[self.free_len] = None;
    }
    fn insert_free(&mut self, mut span: Span) -> Result<(), HeapError> {
        let mut i = 0;
        while i < self.free_len {
            let x = self.free[i].ok_or(HeapError::InvalidFree)?;
            let xe = x.offset.checked_add(x.size).ok_or(HeapError::Overflow)?;
            let se = span
                .offset
                .checked_add(span.size)
                .ok_or(HeapError::Overflow)?;
            if span.offset < xe && x.offset < se {
                return Err(HeapError::InvalidFree);
            }
            if xe == span.offset {
                span = Span {
                    offset: x.offset,
                    size: x.size.checked_add(span.size).ok_or(HeapError::Overflow)?,
                };
                self.remove_free(i);
                i = 0;
                continue;
            }
            if se == x.offset {
                span = Span {
                    offset: span.offset,
                    size: span.size.checked_add(x.size).ok_or(HeapError::Overflow)?,
                };
                self.remove_free(i);
                i = 0;
                continue;
            }
            i += 1
        }
        if self.free_len == N {
            return Err(HeapError::MetadataFull);
        }
        self.free[self.free_len] = Some(span);
        self.free_len += 1;
        for i in 1..self.free_len {
            let mut j = i;
            while j > 0 && self.free[j].unwrap().offset < self.free[j - 1].unwrap().offset {
                self.free.swap(j, j - 1);
                j -= 1
            }
        }
        Ok(())
    }
    pub fn deallocate(&mut self, a: Allocation) -> Result<(), HeapError> {
        let i = self
            .live
            .iter()
            .position(|x| *x == Some(a))
            .ok_or(HeapError::InvalidFree)?;
        self.insert_free(Span {
            offset: a.offset,
            size: a.size,
        })?;
        self.live[i] = None;
        Ok(())
    }
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coalesce() {
        let mut h = Heap::<8>::new(1024);
        h.init().unwrap();
        assert_eq!(h.init(), Err(HeapError::AlreadyInitialized));
        let a = h.allocate(100, 64).unwrap();
        let b = h.allocate(200, 16).unwrap();
        assert_eq!(a.offset % 64, 0);
        h.deallocate(a).unwrap();
        h.deallocate(b).unwrap();
        assert_eq!(h.allocate(1024, 1).unwrap().offset, 0);
    }
    #[test]
    fn invalid_and_double_free() {
        let mut h = Heap::<4>::new(128);
        h.init().unwrap();
        assert_eq!(h.allocate(4, 3), Err(HeapError::BadAlignment));
        let a = h.allocate(8, 8).unwrap();
        h.deallocate(a).unwrap();
        assert_eq!(h.deallocate(a), Err(HeapError::InvalidFree));
    }
    #[test]
    fn coalesces_when_free_metadata_is_full() {
        let mut h = Heap::<1>::new(128);
        h.init().unwrap();
        let a = h.allocate(64, 1).unwrap();
        assert_eq!(h.allocate(1, 1), Err(HeapError::MetadataFull));
        h.deallocate(a).unwrap();
        assert_eq!(h.allocate(128, 1).unwrap().offset, 0);
    }
    #[test]
    fn overflow() {
        let mut h = Heap::<2>::new(usize::MAX);
        h.init().unwrap();
        h.allocate(1, 1).unwrap();
        assert_eq!(h.allocate(usize::MAX, 2), Err(HeapError::Overflow));
    }
}
