//! Four-level x86-64 paging geometry (48-bit canonical virtual addresses).
use crate::virt_addr::VirtAddr;
use core::fmt;
pub const PAGE_SIZE: u64 = 4096;
pub const HUGE_2M: u64 = 2 * 1024 * 1024;
pub const ENTRIES_PER_TABLE: usize = 512;
pub const PHYS_ADDR_BITS: u8 = 52;
pub const PHYS_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;
pub const USER_CANONICAL_END: u64 = 0x0000_8000_0000_0000;
pub const KERNEL_CANONICAL_START: u64 = 0xffff_8000_0000_0000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageMode {
    FourLevel,
    FiveLevel,
    Legacy32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    UnsupportedMode,
    NonCanonical,
    Unaligned,
    Overflow,
    InvalidLevel,
    ZeroPages,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indices {
    pub pml4: usize,
    pub pdpt: usize,
    pub pd: usize,
    pub pt: usize,
    pub offset: u16,
}
pub fn is_canonical(v: u64) -> bool {
    VirtAddr::new(v).is_ok()
}
pub fn indices(mode: PageMode, v: u64) -> Result<Indices, GeometryError> {
    if mode != PageMode::FourLevel {
        return Err(GeometryError::UnsupportedMode);
    }
    if !is_canonical(v) {
        return Err(GeometryError::NonCanonical);
    }
    Ok(Indices {
        pml4: ((v >> 39) & 0x1ff) as usize,
        pdpt: ((v >> 30) & 0x1ff) as usize,
        pd: ((v >> 21) & 0x1ff) as usize,
        pt: ((v >> 12) & 0x1ff) as usize,
        offset: (v & 0xfff) as u16,
    })
}
pub const fn page_size_for_level(level: u8) -> Option<u64> {
    match level {
        1 => Some(PAGE_SIZE),
        2 => Some(HUGE_2M),
        3 => Some(1 << 30),
        _ => None,
    }
}
pub fn checked_page_end(start: u64, pages: u64) -> Result<u64, GeometryError> {
    if pages == 0 {
        return Err(GeometryError::ZeroPages);
    }
    if start & (PAGE_SIZE - 1) != 0 {
        return Err(GeometryError::Unaligned);
    }
    start
        .checked_add(
            pages
                .checked_mul(PAGE_SIZE)
                .ok_or(GeometryError::Overflow)?,
        )
        .ok_or(GeometryError::Overflow)
}
impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_level_indices_and_edges() {
        let i = indices(PageMode::FourLevel, 0xffff_8123_4567_8abc).unwrap();
        assert_eq!(
            (i.pml4, i.pdpt, i.pd, i.pt, i.offset),
            (0x102, 0x08d, 0x02b, 0x078, 0xabc)
        );
        assert!(is_canonical(0x0000_7fff_ffff_ffff));
        assert!(is_canonical(0xffff_8000_0000_0000));
        assert!(!is_canonical(0x0000_8000_0000_0000));
        assert!(!is_canonical(0xffff_7fff_ffff_ffff));
    }
    #[test]
    fn checked_ranges() {
        assert_eq!(
            indices(PageMode::FiveLevel, 0x1000),
            Err(GeometryError::UnsupportedMode)
        );
        assert_eq!(checked_page_end(0x1000, 2), Ok(0x3000));
        assert_eq!(checked_page_end(1, 1), Err(GeometryError::Unaligned));
        assert_eq!(
            checked_page_end(0x1000, u64::MAX),
            Err(GeometryError::Overflow)
        );
    }
}
