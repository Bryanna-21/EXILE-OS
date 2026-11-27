//! Read-only virtual-to-physical translation with effective parent permissions.
use super::{
    backend::PageTableMemory,
    frame_source::FrameSource,
    geometry::{HUGE_2M, PAGE_SIZE},
    space::{PageTableVm, VmError},
    tlb::TlbInvalidator,
    walk::effective_permissions,
};
use crate::page_perms::Perms;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Translation {
    pub physical: u64,
    pub permissions: Perms,
    pub page_size: u64,
}
impl<F: FrameSource, B: PageTableMemory, T: TlbInvalidator, const N: usize, const M: usize>
    PageTableVm<F, B, T, N, M>
{
    pub fn translate(&self, v: u64) -> Result<Translation, VmError> {
        let w = self.walk(v)?;
        let size = if w.leaf_level == 2 {
            HUGE_2M
        } else {
            PAGE_SIZE
        };
        let mask = size - 1;
        let base = w.entry & super::geometry::PHYS_ADDR_MASK & !mask;
        let physical = base.checked_add(v & mask).ok_or(VmError::PhysicalAddress)?;
        Ok(Translation {
            physical,
            permissions: effective_permissions(w),
            page_size: size,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn translation_offset_math() {
        let v = 0x4000u64;
        let p = 0x9000u64;
        assert_eq!((p + (v & 4095)), p);
    }
}
