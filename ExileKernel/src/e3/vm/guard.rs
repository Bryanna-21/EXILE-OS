//! Explicit guard-page reservations checked before normal mappings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardRange {
    pub start: u64,
    pub end: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardError {
    Unaligned,
    Empty,
    Overlap,
    Capacity,
    NotFound,
}
pub struct GuardSet<const N: usize> {
    items: [Option<GuardRange>; N],
    len: usize,
}
impl<const N: usize> GuardSet<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub fn reserve(&mut self, start: u64, end: u64) -> Result<(), GuardError> {
        if start >= end {
            return Err(GuardError::Empty);
        }
        if start & 4095 != 0 || end & 4095 != 0 {
            return Err(GuardError::Unaligned);
        }
        if self
            .items
            .iter()
            .flatten()
            .any(|g| start < g.end && g.start < end)
        {
            return Err(GuardError::Overlap);
        }
        let s = self
            .items
            .iter_mut()
            .find(|x| x.is_none())
            .ok_or(GuardError::Capacity)?;
        *s = Some(GuardRange { start, end });
        self.len += 1;
        Ok(())
    }
    pub fn blocks(&self, start: u64, end: u64) -> bool {
        self.items
            .iter()
            .flatten()
            .any(|g| start < g.end && g.start < end)
    }
    pub fn remove(&mut self, start: u64) -> Result<(), GuardError> {
        let s = self
            .items
            .iter_mut()
            .find(|x| x.is_some_and(|g| g.start == start))
            .ok_or(GuardError::NotFound)?;
        *s = None;
        self.len -= 1;
        Ok(())
    }
    pub fn iter(&self) -> impl Iterator<Item = &GuardRange> {
        self.items.iter().filter_map(Option::as_ref)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guard_cannot_be_overmapped() {
        let mut g = GuardSet::<2>::new();
        g.reserve(0x8000, 0x9000).unwrap();
        assert!(g.blocks(0x7000, 0x9000));
        assert!(!g.blocks(0x9000, 0xa000));
        assert_eq!(g.reserve(0x8000, 0x9000), Err(GuardError::Overlap));
    }
}
