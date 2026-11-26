//! Fixed-size slot ownership tracker with generation-checked tokens.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    index: u16,
    generation: u32,
}
impl Slot {
    pub const fn index(self) -> usize {
        self.index as usize
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlabError {
    Capacity,
    InvalidSlot,
    DoubleFree,
}
pub struct Slab<const N: usize> {
    used: [bool; N],
    generation: [u32; N],
    live: usize,
}
impl<const N: usize> Slab<N> {
    pub const fn new() -> Self {
        Self {
            used: [false; N],
            generation: [1; N],
            live: 0,
        }
    }
    pub const fn capacity(&self) -> usize {
        N
    }
    pub const fn live(&self) -> usize {
        self.live
    }
    pub fn allocate(&mut self) -> Result<Slot, SlabError> {
        let i = self
            .used
            .iter()
            .position(|x| !*x)
            .ok_or(SlabError::Capacity)?;
        let index = u16::try_from(i).map_err(|_| SlabError::Capacity)?;
        self.used[i] = true;
        self.live += 1;
        Ok(Slot {
            index,
            generation: self.generation[i],
        })
    }
    pub fn release(&mut self, s: Slot) -> Result<(), SlabError> {
        let i = s.index();
        if i >= N {
            return Err(SlabError::InvalidSlot);
        }
        if !self.used[i] {
            return Err(SlabError::DoubleFree);
        }
        if self.generation[i] != s.generation {
            return Err(SlabError::InvalidSlot);
        }
        self.used[i] = false;
        self.generation[i] = self.generation[i].wrapping_add(1).max(1);
        self.live -= 1;
        Ok(())
    }
    pub fn is_live(&self, s: Slot) -> bool {
        let i = s.index();
        i < N && self.used[i] && self.generation[i] == s.generation
    }
}
impl<const N: usize> Default for Slab<N> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generation_and_double_free() {
        let mut s = Slab::<2>::new();
        let a = s.allocate().unwrap();
        s.release(a).unwrap();
        assert_eq!(s.release(a), Err(SlabError::DoubleFree));
        let b = s.allocate().unwrap();
        assert_ne!(a.generation, b.generation);
        assert!(!s.is_live(a));
    }
    #[test]
    fn capacity_and_bounds() {
        let mut s = Slab::<1>::new();
        let a = s.allocate().unwrap();
        assert_eq!(s.allocate(), Err(SlabError::Capacity));
        assert_eq!(
            s.release(Slot {
                index: 99,
                generation: 1
            }),
            Err(SlabError::InvalidSlot)
        );
        s.release(a).unwrap();
    }
}
