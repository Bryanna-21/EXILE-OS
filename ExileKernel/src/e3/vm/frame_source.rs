//! Explicit one-frame ownership adapter over the existing physical allocator.
use crate::frame_alloc::{
    FrameAllocator, FrameError as ExistingError, FrameToken as ExistingToken,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLease {
    slot: u16,
    generation: u32,
    phys: u64,
}
impl FrameLease {
    pub const fn physical(self) -> u64 {
        self.phys
    }
    pub const fn generation(self) -> u32 {
        self.generation
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSourceError {
    Exhausted,
    Stale,
    Capacity,
    Overflow,
    Allocator(ExistingError),
    GenerationExhausted,
    InjectedFailure,
}
/// Supplies one owned physical frame per lease. A lease is returned exactly once;
/// implementations must reject stale or duplicate releases.
pub trait FrameSource {
    fn allocate(&mut self) -> Result<FrameLease, FrameSourceError>;
    fn validate(&self, lease: FrameLease) -> bool;
    fn release(&mut self, lease: FrameLease) -> Result<(), FrameSourceError>;
}
struct Slot {
    token: Option<ExistingToken>,
    generation: u32,
    retired: bool,
}
pub struct AllocatorFrameSource<'a, const R: usize, const A: usize, const N: usize> {
    allocator: &'a mut FrameAllocator<R, A>,
    slots: [Slot; N],
}
impl<'a, const R: usize, const A: usize, const N: usize> AllocatorFrameSource<'a, R, A, N> {
    pub fn new(allocator: &'a mut FrameAllocator<R, A>) -> Self {
        Self {
            allocator,
            slots: core::array::from_fn(|_| Slot {
                token: None,
                generation: 1,
                retired: false,
            }),
        }
    }
}
impl<const R: usize, const A: usize, const N: usize> FrameSource
    for AllocatorFrameSource<'_, R, A, N>
{
    fn allocate(&mut self) -> Result<FrameLease, FrameSourceError> {
        let i = self
            .slots
            .iter()
            .take(u16::MAX as usize + 1)
            .position(|s| s.token.is_none() && !s.retired)
            .ok_or(FrameSourceError::Capacity)?;
        let token = self
            .allocator
            .allocate(1)
            .map_err(FrameSourceError::Allocator)?;
        let s = &mut self.slots[i];
        s.token = Some(token);
        Ok(FrameLease {
            slot: u16::try_from(i).map_err(|_| FrameSourceError::Capacity)?,
            generation: s.generation,
            phys: token.start,
        })
    }
    fn validate(&self, l: FrameLease) -> bool {
        self.slots.get(l.slot as usize).is_some_and(|s| {
            s.generation == l.generation && s.token.is_some_and(|t| t.start == l.phys)
        })
    }
    fn release(&mut self, l: FrameLease) -> Result<(), FrameSourceError> {
        let s = self
            .slots
            .get_mut(l.slot as usize)
            .ok_or(FrameSourceError::Stale)?;
        if s.generation != l.generation {
            return Err(FrameSourceError::Stale);
        }
        let token = s.token.ok_or(FrameSourceError::Stale)?;
        if token.start != l.phys {
            return Err(FrameSourceError::Stale);
        }
        self.allocator
            .release(token)
            .map_err(FrameSourceError::Allocator)?;
        s.token = None;
        if let Some(g) = s.generation.checked_add(1) {
            s.generation = g
        } else {
            s.retired = true
        }
        Ok(())
    }
}
#[cfg(test)]
pub struct MockFrames<const N: usize> {
    used: [bool; N],
    generation: [u32; N],
    retired: [bool; N],
    base: u64,
    fail_releases: usize,
}
#[cfg(test)]
impl<const N: usize> MockFrames<N> {
    pub const fn new(base: u64) -> Self {
        Self {
            used: [false; N],
            generation: [1; N],
            retired: [false; N],
            base,
            fail_releases: 0,
        }
    }
    pub fn fail_next_releases(&mut self, count: usize) {
        self.fail_releases = count;
    }
    pub fn live(&self) -> usize {
        self.used.iter().filter(|x| **x).count()
    }
}
#[cfg(test)]
impl<const N: usize> FrameSource for MockFrames<N> {
    fn allocate(&mut self) -> Result<FrameLease, FrameSourceError> {
        let i = self
            .used
            .iter()
            .enumerate()
            .position(|(i, x)| !*x && !self.retired[i])
            .ok_or(FrameSourceError::Exhausted)?;
        let phys = self
            .base
            .checked_add(i as u64 * 4096)
            .ok_or(FrameSourceError::Overflow)?;
        if i > u16::MAX as usize {
            return Err(FrameSourceError::Capacity);
        }
        self.used[i] = true;
        Ok(FrameLease {
            slot: i as u16,
            generation: self.generation[i],
            phys,
        })
    }
    fn validate(&self, l: FrameLease) -> bool {
        let i = l.slot as usize;
        i < N
            && self.used[i]
            && self.generation[i] == l.generation
            && self.base.checked_add(i as u64 * 4096) == Some(l.phys)
    }
    fn release(&mut self, l: FrameLease) -> Result<(), FrameSourceError> {
        let i = l.slot as usize;
        if i >= N
            || !self.used[i]
            || self.generation[i] != l.generation
            || self.base.checked_add(i as u64 * 4096) != Some(l.phys)
        {
            return Err(FrameSourceError::Stale);
        }
        if self.fail_releases > 0 {
            self.fail_releases -= 1;
            return Err(FrameSourceError::InjectedFailure);
        }
        self.used[i] = false;
        if let Some(g) = self.generation[i].checked_add(1) {
            self.generation[i] = g
        } else {
            self.retired[i] = true
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_and_duplicate_leases_are_rejected() {
        let mut f = MockFrames::<2>::new(0x1000);
        let a = f.allocate().unwrap();
        assert!(f.validate(a));
        f.release(a).unwrap();
        assert!(!f.validate(a));
        assert_eq!(f.release(a), Err(FrameSourceError::Stale));
        let b = f.allocate().unwrap();
        assert_ne!(a.generation(), b.generation());
    }
}
#[cfg(test)]
mod allocator_adapter_tests {
    use super::*;
    use crate::frame_alloc::FrameRange;
    #[test]
    fn adapter_tracks_existing_allocator_tokens() {
        let mut allocator = FrameAllocator::<2, 2>::new();
        allocator
            .add_range(FrameRange::new(0x4000, 4).unwrap())
            .unwrap();
        let mut source = AllocatorFrameSource::<2, 2, 2>::new(&mut allocator);
        let token = source.allocate().unwrap();
        assert_eq!(token.physical(), 0x4000);
        assert!(source.validate(token));
        source.release(token).unwrap();
        assert!(!source.validate(token));
        assert_eq!(source.release(token), Err(FrameSourceError::Stale));
    }
}
