//! Bounded virtual-region policy metadata; not a second mapping implementation.
use crate::page_perms::Perms;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Reserved,
    Mapped,
    Guard,
    Unmapped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub kind: RegionKind,
    pub permissions: Option<Perms>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    Empty,
    Overflow,
    Unaligned,
    Overlap,
    Capacity,
    InvalidPermissions,
    NotFound,
    NonCanonical,
}
pub struct RegionSet<const N: usize> {
    items: [Option<Region>; N],
    len: usize,
}
impl<const N: usize> RegionSet<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub fn insert(&mut self, r: Region) -> Result<(), RegionError> {
        if r.start >= r.end {
            return Err(RegionError::Empty);
        }
        if r.start & 4095 != 0 || r.end & 4095 != 0 {
            return Err(RegionError::Unaligned);
        }
        if !super::geometry::is_canonical(r.start)
            || !super::geometry::is_canonical(r.end - 1)
            || (r.start < super::geometry::USER_CANONICAL_END
                && r.end > super::geometry::USER_CANONICAL_END)
        {
            return Err(RegionError::NonCanonical);
        }
        if r.permissions.is_some_and(|p| p.validate().is_err()) {
            return Err(RegionError::InvalidPermissions);
        }
        if self
            .items
            .iter()
            .flatten()
            .any(|x| r.start < x.end && x.start < r.end)
        {
            return Err(RegionError::Overlap);
        }
        let s = self
            .items
            .iter_mut()
            .find(|x| x.is_none())
            .ok_or(RegionError::Capacity)?;
        *s = Some(r);
        self.len += 1;
        Ok(())
    }
    pub fn find(&self, a: u64) -> Option<Region> {
        self.items
            .iter()
            .flatten()
            .find(|r| r.start <= a && a < r.end)
            .copied()
    }
    pub fn remove(&mut self, start: u64) -> Result<Region, RegionError> {
        let s = self
            .items
            .iter_mut()
            .find(|x| x.is_some_and(|r| r.start == start))
            .ok_or(RegionError::NotFound)?;
        self.len -= 1;
        s.take().ok_or(RegionError::NotFound)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Region> {
        self.items.iter().filter_map(Option::as_ref)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_overlap_and_tracks_kind() {
        let mut s = RegionSet::<2>::new();
        let r = Region {
            start: 0x1000,
            end: 0x3000,
            kind: RegionKind::Guard,
            permissions: None,
        };
        s.insert(r).unwrap();
        assert_eq!(
            s.insert(Region {
                start: 0x2000,
                end: 0x4000,
                ..r
            }),
            Err(RegionError::Overlap)
        );
        assert_eq!(s.find(0x2000).unwrap().kind, RegionKind::Guard);
        s.remove(0x1000).unwrap();
        assert_eq!(s.len(), 0);
        assert_eq!(
            s.insert(Region {
                start: 0x0000_8000_0000_0000,
                end: 0x0000_8000_0000_1000,
                kind: RegionKind::Mapped,
                permissions: None
            }),
            Err(RegionError::NonCanonical)
        );
    }
}
