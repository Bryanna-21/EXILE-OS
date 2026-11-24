//! EUK BootInfo â€” versioned, architecture-neutral loader->kernel handoff ABI.
//! Contract: docs/BOOT_CONTRACT.md. Design rules (Mission A, req. 3):
//!   * fixed-width types, explicit C layout, 8-byte alignment;
//!   * NO raw pointers: out-of-struct objects are referenced by physical
//!     address + count and interpreted by the kernel after validation;
//!   * fail-closed: any inconsistency is an error; boot must not continue;
//!   * unsupported versions are rejected (no silent guessing).
//! Division of responsibility: BootInfo validation is the STRICT ABI check
//! (the loader promises well-formed input). memmap_norm tolerates malformed
//! firmware maps defensively. Both layers are applied.

pub const MAGIC: u32 = 0x424B_5545; // "EUKB" little-endian
pub const VERSION: u32 = 1;

pub const ARCH_X86_64: u32 = 1;
pub const ARCH_AARCH64: u32 = 2;

pub const FLAG_FRAMEBUFFER: u32 = 1 << 0;
pub const FLAG_ACPI_RSDP: u32 = 1 << 1;

/// Hard ABI bound on the memory-map array. Larger handoffs are malformed.
pub const MAX_REGIONS: u64 = 256;

/// One memory-map entry as delivered by the loader.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemRegion {
    pub start: u64,
    pub size: u64,
    pub kind: u32,
    pub _pad: u32,
}

pub const MEM_REGION_SIZE: usize = 24;

/// Loader -> kernel handoff header. Physical addresses everywhere.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootInfo {
    pub magic: u32,             // MAGIC
    pub version: u32,           // VERSION (exact match required)
    pub total_size: u32,        // header + regions array, in bytes
    pub arch: u32,              // ARCH_X86_64 | ARCH_AARCH64
    pub flags: u32,             // FLAG_* bits; each flag must match its object
    pub memmap_paddr: u64,      // physical address of the MemRegion array
    pub memmap_count: u64,      // number of entries in the array
    pub kernel_phys_start: u64, // page-aligned, inclusive
    pub kernel_phys_end: u64,   // page-aligned, exclusive
    pub kernel_entry: u64,      // kernel virtual entry address
    pub framebuffer_paddr: u64, // valid only with FLAG_FRAMEBUFFER
    pub framebuffer_size: u64,  // valid only with FLAG_FRAMEBUFFER
    pub rsdp_paddr: u64,        // valid only with FLAG_ACPI_RSDP
}

pub const SIZE: usize = 88;

// Compile-time ABI pinning: layout drift is a build error, not a runtime bug.
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
    BadTotalSize,
    EmptyMemmap,
    TooManyRegions,
    MisalignedMemmap,
    RangeOverflow,
    KernelRangeInvalid,
    KernelRangeMisaligned,
    FramebufferMismatch,
    RsdpMismatch,
}

impl BootInfo {
    /// Structural validation. Fail-closed per Mission A rule 10: the caller
    /// must abort boot on any Err.
    pub fn validate(&self) -> Result<(), BootInfoError> {
        if self.magic != MAGIC {
            return Err(BootInfoError::BadMagic);
        }
        if self.version != VERSION {
            return Err(BootInfoError::UnsupportedVersion(self.version));
        }
        if self.arch != ARCH_X86_64 && self.arch != ARCH_AARCH64 {
            return Err(BootInfoError::BadArch(self.arch));
        }
        let array_bytes = self
            .memmap_count
            .checked_mul(MEM_REGION_SIZE as u64)
            .ok_or(BootInfoError::RangeOverflow)?;
        let total = (SIZE as u64)
            .checked_add(array_bytes)
            .ok_or(BootInfoError::RangeOverflow)?;
        if total > u32::MAX as u64 {
            return Err(BootInfoError::RangeOverflow);
        }
        if self.total_size != total as u32 {
            return Err(BootInfoError::BadTotalSize);
        }
        if self.memmap_count == 0 {
            return Err(BootInfoError::EmptyMemmap);
        }
        if self.memmap_count > MAX_REGIONS {
            return Err(BootInfoError::TooManyRegions);
        }
        if self.memmap_paddr % 8 != 0 {
            return Err(BootInfoError::MisalignedMemmap);
        }
        self.memmap_paddr
            .checked_add(array_bytes)
            .ok_or(BootInfoError::RangeOverflow)?;
        if self.kernel_phys_start % 4096 != 0 || self.kernel_phys_end % 4096 != 0 {
            return Err(BootInfoError::KernelRangeMisaligned);
        }
        if self.kernel_phys_start >= self.kernel_phys_end {
            return Err(BootInfoError::KernelRangeInvalid);
        }
        self.kernel_phys_end
            .checked_sub(1)
            .ok_or(BootInfoError::KernelRangeInvalid)?;
        let fb = (self.flags & FLAG_FRAMEBUFFER) != 0;
        if fb == (self.framebuffer_size == 0) {
            return Err(BootInfoError::FramebufferMismatch);
        }
        if fb {
            if self.framebuffer_paddr % 4096 != 0 {
                return Err(BootInfoError::FramebufferMismatch);
            }
            self.framebuffer_paddr
                .checked_add(self.framebuffer_size)
                .ok_or(BootInfoError::RangeOverflow)?;
        }
        let acpi = (self.flags & FLAG_ACPI_RSDP) != 0;
        if acpi != (self.rsdp_paddr != 0) {
            return Err(BootInfoError::RsdpMismatch);
        }
        Ok(())
    }
}

/// Strict region-array validation (the loader-side ABI promise).
pub fn validate_regions(regions: &[MemRegion]) -> Result<(), BootInfoError> {
    if regions.is_empty() {
        return Err(BootInfoError::EmptyMemmap);
    }
    if regions.len() as u64 > MAX_REGIONS {
        return Err(BootInfoError::TooManyRegions);
    }
    for r in regions {
        if r.size == 0 {
            return Err(BootInfoError::EmptyMemmap);
        }
        r.start
            .checked_add(r.size)
            .ok_or(BootInfoError::RangeOverflow)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions() -> [MemRegion; 3] {
        [
            MemRegion {
                start: 0,
                size: 4096,
                kind: 1,
                _pad: 0,
            },
            MemRegion {
                start: 4096,
                size: 8192,
                kind: 2,
                _pad: 0,
            },
            MemRegion {
                start: 0x10_0000,
                size: 0x1000_0000,
                kind: 1,
                _pad: 0,
            },
        ]
    }

    fn info(arch: u32) -> BootInfo {
        BootInfo {
            magic: MAGIC,
            version: VERSION,
            total_size: (SIZE + 3 * MEM_REGION_SIZE) as u32,
            arch,
            flags: 0,
            memmap_paddr: 0x8000,
            memmap_count: 3,
            kernel_phys_start: 0x10_0000,
            kernel_phys_end: 0x20_0000,
            kernel_entry: 0x10_0020,
            framebuffer_paddr: 0,
            framebuffer_size: 0,
            rsdp_paddr: 0,
        }
    }

    #[test]
    fn abi_sizes_are_pinned() {
        assert_eq!(core::mem::size_of::<BootInfo>(), 88);
        assert_eq!(core::mem::size_of::<MemRegion>(), 24);
    }

    #[test]
    fn valid_passes() {
        assert_eq!(info(ARCH_X86_64).validate(), Ok(()));
        assert_eq!(info(ARCH_AARCH64).validate(), Ok(()));
    }

    #[test]
    fn bad_magic_rejected() {
        let mut b = info(ARCH_X86_64);
        b.magic = 0xDEAD_BEEF;
        assert_eq!(b.validate(), Err(BootInfoError::BadMagic));
    }

    #[test]
    fn unsupported_version_rejected_with_the_version() {
        let mut b = info(ARCH_X86_64);
        b.version = 2;
        assert_eq!(b.validate(), Err(BootInfoError::UnsupportedVersion(2)));
        b.version = 0;
        assert_eq!(b.validate(), Err(BootInfoError::UnsupportedVersion(0)));
    }

    #[test]
    fn unknown_arch_rejected() {
        let mut b = info(ARCH_X86_64);
        b.arch = 3;
        assert_eq!(b.validate(), Err(BootInfoError::BadArch(3)));
    }

    #[test]
    fn total_size_mismatch_and_overflow_rejected() {
        let mut b = info(ARCH_X86_64);
        b.total_size += 1;
        assert_eq!(b.validate(), Err(BootInfoError::BadTotalSize));
        let mut b = info(ARCH_X86_64);
        b.memmap_count = u32::MAX as u64; // count * 24 overflows u64-safe path
        assert_eq!(b.validate(), Err(BootInfoError::RangeOverflow));
    }

    #[test]
    fn empty_and_oversized_memmaps_rejected() {
        let mut b = info(ARCH_X86_64);
        b.memmap_count = 0;
        b.total_size = SIZE as u32;
        assert_eq!(b.validate(), Err(BootInfoError::EmptyMemmap));
        let mut b = info(ARCH_X86_64);
        b.memmap_count = MAX_REGIONS + 1;
        b.total_size = (SIZE + (MAX_REGIONS as usize + 1) * MEM_REGION_SIZE) as u32;
        assert_eq!(b.validate(), Err(BootInfoError::TooManyRegions));
    }

    #[test]
    fn misaligned_memmap_rejected() {
        let mut b = info(ARCH_X86_64);
        b.memmap_paddr = 0x8004;
        assert_eq!(b.validate(), Err(BootInfoError::MisalignedMemmap));
    }

    #[test]
    fn kernel_range_checks() {
        let mut b = info(ARCH_X86_64);
        b.kernel_phys_start = b.kernel_phys_end;
        assert_eq!(b.validate(), Err(BootInfoError::KernelRangeInvalid));
        let mut b = info(ARCH_X86_64);
        b.kernel_phys_start = 0x10_0004; // unaligned
        assert_eq!(b.validate(), Err(BootInfoError::KernelRangeMisaligned));
        let mut b = info(ARCH_X86_64);
        b.kernel_phys_start = 0; // 0-1 underflow in checked_sub
        b.kernel_phys_end = 0;
        assert_eq!(b.validate(), Err(BootInfoError::KernelRangeInvalid));
    }

    #[test]
    fn framebuffer_flag_must_match_object() {
        let mut b = info(ARCH_X86_64);
        b.flags = FLAG_FRAMEBUFFER;
        assert_eq!(b.validate(), Err(BootInfoError::FramebufferMismatch));
        b.framebuffer_size = 0x1000;
        b.framebuffer_paddr = 0x8000_0000;
        assert_eq!(b.validate(), Ok(()));
        b.framebuffer_paddr = 0x8000_0004; // unaligned
        assert_eq!(b.validate(), Err(BootInfoError::FramebufferMismatch));
    }

    #[test]
    fn rsdp_flag_must_match_object() {
        let mut b = info(ARCH_X86_64);
        b.flags = FLAG_ACPI_RSDP;
        assert_eq!(b.validate(), Err(BootInfoError::RsdpMismatch));
        b.rsdp_paddr = 0x1_0000;
        assert_eq!(b.validate(), Ok(()));
        b.flags = 0;
        assert_eq!(b.validate(), Err(BootInfoError::RsdpMismatch));
    }

    #[test]
    fn region_array_strict_validation() {
        assert!(validate_regions(&regions()).is_ok());
        assert_eq!(
            validate_regions(&[MemRegion {
                start: 1,
                size: 0,
                kind: 1,
                _pad: 0
            }]),
            Err(BootInfoError::EmptyMemmap)
        );
        assert_eq!(
            validate_regions(&[MemRegion {
                start: u64::MAX,
                size: 2,
                kind: 1,
                _pad: 0
            }]),
            Err(BootInfoError::RangeOverflow)
        );
        assert_eq!(validate_regions(&[]), Err(BootInfoError::EmptyMemmap));
    }
}
