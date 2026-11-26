//! Bounded virtual mapping metadata; deliberately does not install hardware tables.
#![no_std]
#[cfg(test)]
extern crate std;
use crate::page_table::Permissions;
use crate::virt_addr::{VirtAddr, VirtRange};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mapping {
    pub virtual_range: VirtRange,
    pub physical_start: u64,
    pub permissions: Permissions,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceError {
    Capacity,
    Unaligned,
    Overflow,
    NonCanonical,
    Overlap,
    NotFound,
    BadPermissions,
}
pub struct AddressSpace<const N: usize> {
    items: [Option<Mapping>; N],
    len: usize,
}
impl<const N: usize> AddressSpace<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub fn mappings(&self) -> impl Iterator<Item = &Mapping> {
        self.items.iter().filter_map(Option::as_ref)
    }
    pub fn map(
        &mut self,
        v: VirtAddr,
        p: u64,
        n: u64,
        perm: Permissions,
    ) -> Result<(), AddressSpaceError> {
        perm.validate()
            .map_err(|_| AddressSpaceError::BadPermissions)?;
        if n == 0 || n & 4095 != 0 || v.get() & 4095 != 0 || p == 0 || p & 4095 != 0 {
            return Err(AddressSpaceError::Unaligned);
        }
        let r = VirtRange::from_len(v, n).map_err(|e| match e {
            crate::virt_addr::VirtError::NonCanonical => AddressSpaceError::NonCanonical,
            _ => AddressSpaceError::Overflow,
        })?;
        p.checked_add(n).ok_or(AddressSpaceError::Overflow)?;
        if self.mappings().any(|m| m.virtual_range.overlaps(r)) {
            return Err(AddressSpaceError::Overlap);
        }
        let s = self
            .items
            .iter_mut()
            .find(|x| x.is_none())
            .ok_or(AddressSpaceError::Capacity)?;
        *s = Some(Mapping {
            virtual_range: r,
            physical_start: p,
            permissions: perm,
        });
        self.len += 1;
        Ok(())
    }
    pub fn unmap(&mut self, v: VirtAddr) -> Result<Mapping, AddressSpaceError> {
        let s = self
            .items
            .iter_mut()
            .find(|x| x.as_ref().is_some_and(|m| m.virtual_range.start() == v))
            .ok_or(AddressSpaceError::NotFound)?;
        let m = s.take().ok_or(AddressSpaceError::NotFound)?;
        self.len -= 1;
        Ok(m)
    }
    pub fn translate(&self, v: VirtAddr) -> Option<(u64, Permissions)> {
        self.mappings()
            .find(|m| m.virtual_range.contains(v))
            .and_then(|m| {
                m.physical_start
                    .checked_add(v.get() - m.virtual_range.start().get())
                    .map(|p| (p, m.permissions))
            })
    }
}
impl<const N: usize> Default for AddressSpace<N> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle() {
        let mut a = AddressSpace::<1>::new();
        let v = VirtAddr::new(0x4000).unwrap();
        a.map(v, 0x8000, 8192, Permissions::KERNEL_RW).unwrap();
        assert_eq!(
            a.translate(VirtAddr::new(0x4abc).unwrap()).unwrap().0,
            0x8abc
        );
        assert_eq!(
            a.map(v, 0xa000, 4096, Permissions::KERNEL_RW),
            Err(AddressSpaceError::Overlap)
        );
        a.unmap(v).unwrap();
        assert_eq!(a.len(), 0);
    }
    #[test]
    fn rejects_noncanonical_mapping() {
        let mut a = AddressSpace::<1>::new();
        let v = VirtAddr::new(0x0000_7fff_ffff_f000).unwrap();
        assert_eq!(
            a.map(v, 0x2000, 8192, Permissions::KERNEL_RW),
            Err(AddressSpaceError::NonCanonical)
        );
    }
    #[test]
    fn bad_permissions_overflow() {
        let mut a = AddressSpace::<2>::new();
        let wx = Permissions {
            execute: true,
            ..Permissions::KERNEL_RW
        };
        assert_eq!(
            a.map(VirtAddr::new(0x1000).unwrap(), 0x2000, 4096, wx),
            Err(AddressSpaceError::BadPermissions)
        );
        assert_eq!(
            a.map(
                VirtAddr::new(0x1000).unwrap(),
                u64::MAX - 4095,
                4096,
                Permissions::KERNEL_RW
            ),
            Err(AddressSpaceError::Overflow)
        );
    }
}
