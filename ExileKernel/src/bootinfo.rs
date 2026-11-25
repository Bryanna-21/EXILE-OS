//! EUK BootInfo — versioned, architecture-neutral loader-to-kernel handoff ABI.
//! Contract: docs/BOOT_CONTRACT.md.
#![allow(dead_code)]

pub const MAGIC: u32 = 0x424B_5545;
pub const VERSION: u32 = 1;
pub const ARCH_X86_64: u32 = 1;
pub const ARCH_AARCH64: u32 = 2;
pub const FLAG_FRAMEBUFFER: u32 = 1 << 0;
pub const FLAG_ACPI_RSDP: u32 = 1 << 1;
pub const KNOWN_FLAGS: u32 = FLAG_FRAMEBUFFER | FLAG_ACPI_RSDP;
pub const MAX_REGIONS: u64 = 256;

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemRegion {
    pub start: u64,
    pub size: u64,
    pub kind: u32,
    pub _pad: u32,
}
pub const MEM_REGION_SIZE: usize = 24;

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootInfo {
    pub magic: u32,
    pub version: u32,
    pub total_size: u32,
    pub arch: u32,
    pub flags: u32,
    pub memmap_paddr: u64,
    pub memmap_count: u64,
    pub kernel_phys_start: u64,
    pub kernel_phys_end: u64,
    pub kernel_entry: u64,
    pub framebuffer_paddr: u64,
    pub framebuffer_size: u64,
    pub rsdp_paddr: u64,
}
pub const SIZE: usize = 88;

const _: () = {
    assert!(core::mem::size_of::<BootInfo>() == SIZE);
    assert!(core::mem::size_of::<MemRegion>() == MEM_REGION_SIZE);
    assert!(core::mem::align_of::<BootInfo>() == 8);
    assert!(core::mem::align_of::<MemRegion>() == 8);
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootInfoError {
    BadMagic,
    UnsupportedVersion(u32),
    BadArch(u32),
    UnknownFlags(u32),
    BadTotalSize,
    EmptyMemmap,
    TooManyRegions,
    NullMemmap,
    MisalignedMemmap,
    RangeOverflow,
    KernelRangeInvalid,
    KernelRangeMisaligned,
    KernelEntryInvalid,
    FramebufferMismatch,
    RsdpMismatch,
}

impl BootInfo {
    pub fn validate(&self) -> Result<(), BootInfoError> {
        if self.magic != MAGIC { return Err(BootInfoError::BadMagic); }
        if self.version != VERSION { return Err(BootInfoError::UnsupportedVersion(self.version)); }
        if self.arch != ARCH_X86_64 && self.arch != ARCH_AARCH64 { return Err(BootInfoError::BadArch(self.arch)); }
        let unknown = self.flags & !KNOWN_FLAGS;
        if unknown != 0 { return Err(BootInfoError::UnknownFlags(unknown)); }
        let array_bytes = self.memmap_count.checked_mul(MEM_REGION_SIZE as u64).ok_or(BootInfoError::RangeOverflow)?;
        let total = (SIZE as u64).checked_add(array_bytes).ok_or(BootInfoError::RangeOverflow)?;
        if total > u32::MAX as u64 || self.total_size != total as u32 { return Err(BootInfoError::BadTotalSize); }
        if self.memmap_count == 0 { return Err(BootInfoError::EmptyMemmap); }
        if self.memmap_count > MAX_REGIONS { return Err(BootInfoError::TooManyRegions); }
        if self.memmap_paddr == 0 { return Err(BootInfoError::NullMemmap); }
        if self.memmap_paddr % 8 != 0 { return Err(BootInfoError::MisalignedMemmap); }
        self.memmap_paddr.checked_add(array_bytes).ok_or(BootInfoError::RangeOverflow)?;
        if self.kernel_phys_start % 4096 != 0 || self.kernel_phys_end % 4096 != 0 { return Err(BootInfoError::KernelRangeMisaligned); }
        if self.kernel_phys_start >= self.kernel_phys_end { return Err(BootInfoError::KernelRangeInvalid); }
        if self.kernel_entry == 0 { return Err(BootInfoError::KernelEntryInvalid); }
        let framebuffer = (self.flags & FLAG_FRAMEBUFFER) != 0;
        if framebuffer != (self.framebuffer_size != 0) { return Err(BootInfoError::FramebufferMismatch); }
        if framebuffer {
            if self.framebuffer_paddr == 0 || self.framebuffer_paddr % 4096 != 0 { return Err(BootInfoError::FramebufferMismatch); }
            self.framebuffer_paddr.checked_add(self.framebuffer_size).ok_or(BootInfoError::RangeOverflow)?;
        }
        let acpi = (self.flags & FLAG_ACPI_RSDP) != 0;
        if acpi != (self.rsdp_paddr != 0) { return Err(BootInfoError::RsdpMismatch); }
        Ok(())
    }
}

pub fn validate_regions(regions: &[MemRegion]) -> Result<(), BootInfoError> {
    if regions.is_empty() { return Err(BootInfoError::EmptyMemmap); }
    if regions.len() as u64 > MAX_REGIONS { return Err(BootInfoError::TooManyRegions); }
    for region in regions {
        if region.size == 0 { return Err(BootInfoError::EmptyMemmap); }
        region.start.checked_add(region.size).ok_or(BootInfoError::RangeOverflow)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info(count: usize) -> BootInfo {
        BootInfo { magic: MAGIC, version: VERSION, total_size: (SIZE + count * MEM_REGION_SIZE) as u32, arch: ARCH_X86_64, flags: 0, memmap_paddr: 0x8000, memmap_count: count as u64, kernel_phys_start: 0x10_0000, kernel_phys_end: 0x20_0000, kernel_entry: 0x10_0020, framebuffer_paddr: 0, framebuffer_size: 0, rsdp_paddr: 0 }
    }
    #[test] fn abi_layout_is_pinned() { assert_eq!(core::mem::size_of::<BootInfo>(), 88); assert_eq!(core::mem::size_of::<MemRegion>(), 24); }
    #[test] fn valid_architectures_pass() { assert_eq!(info(3).validate(), Ok(())); let mut b = info(3); b.arch = ARCH_AARCH64; assert_eq!(b.validate(), Ok(())); }
    #[test] fn malformed_header_is_rejected() {
        let mut b = info(3); b.magic = 0; assert_eq!(b.validate(), Err(BootInfoError::BadMagic));
        let mut b = info(3); b.version = 2; assert_eq!(b.validate(), Err(BootInfoError::UnsupportedVersion(2)));
        let mut b = info(3); b.flags = 1 << 31; assert_eq!(b.validate(), Err(BootInfoError::UnknownFlags(1 << 31)));
        let mut b = info(3); b.memmap_paddr = 0; assert_eq!(b.validate(), Err(BootInfoError::NullMemmap));
        let mut b = info(3); b.kernel_entry = 0; assert_eq!(b.validate(), Err(BootInfoError::KernelEntryInvalid));
    }
    #[test] fn counts_sizes_and_overflow_fail_closed() {
        let mut b = info(3); b.total_size += 1; assert_eq!(b.validate(), Err(BootInfoError::BadTotalSize));
        let mut b = info(3); b.memmap_count = 0; b.total_size = SIZE as u32; assert_eq!(b.validate(), Err(BootInfoError::EmptyMemmap));
        let mut b = info(3); b.memmap_count = MAX_REGIONS + 1; assert_eq!(b.validate(), Err(BootInfoError::TooManyRegions));
        let mut b = info(3); b.memmap_count = u64::MAX; assert_eq!(b.validate(), Err(BootInfoError::RangeOverflow));
    }
    #[test] fn ranges_and_optional_objects_are_checked() {
        let mut b = info(3); b.kernel_phys_start = 0x10_004; assert_eq!(b.validate(), Err(BootInfoError::KernelRangeMisaligned));
        let mut b = info(3); b.kernel_phys_start = b.kernel_phys_end; assert_eq!(b.validate(), Err(BootInfoError::KernelRangeInvalid));
        let mut b = info(3); b.flags = FLAG_FRAMEBUFFER; assert_eq!(b.validate(), Err(BootInfoError::FramebufferMismatch)); b.framebuffer_paddr = 0x8000_0000; b.framebuffer_size = 0x1000; assert_eq!(b.validate(), Ok(()));
        let mut b = info(3); b.flags = FLAG_ACPI_RSDP; assert_eq!(b.validate(), Err(BootInfoError::RsdpMismatch)); b.rsdp_paddr = 0x10000; assert_eq!(b.validate(), Ok(()));
    }
    #[test] fn regions_reject_empty_zero_and_overflow() {
        assert_eq!(validate_regions(&[]), Err(BootInfoError::EmptyMemmap));
        assert_eq!(validate_regions(&[MemRegion { start: 1, size: 0, kind: 1, _pad: 0 }]), Err(BootInfoError::EmptyMemmap));
        assert_eq!(validate_regions(&[MemRegion { start: u64::MAX, size: 2, kind: 1, _pad: 0 }]), Err(BootInfoError::RangeOverflow));
    }
}
