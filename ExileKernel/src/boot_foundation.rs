//! Section A(i) boot-foundation validation façade.
//! SPDX-License-Identifier: Apache-2.0
//!
//! This module deliberately performs no raw-pointer dereference. The loader-facing
//! caller must first copy/parse the physical objects into bounded slices, then pass
//! those slices here. A successful `validate` result is the only authority for
//! handing normalized usable ranges to the early allocator.
#![allow(dead_code)]

use crate::bootinfo::{BootInfo, BootInfoError, MemRegion};
use crate::memmap_norm::{self, Error as NormalizeError, Interval, MAX_OUT};
use crate::range::PhysRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    BootInfo(BootInfoError),
    Regions(BootInfoError),
    Normalize(NormalizeError),
    InvalidNormalizedMap(&'static str),
    NoUsableMemory,
}

impl From<BootInfoError> for Error {
    fn from(value: BootInfoError) -> Self { Self::BootInfo(value) }
}

pub struct ValidatedBoot<'a> {
    pub info: &'a BootInfo,
    pub regions: &'a [MemRegion],
    pub normalized: &'a [Interval],
}

impl<'a> ValidatedBoot<'a> {
    pub fn validate(
        info: &'a BootInfo,
        regions: &'a [MemRegion],
        output: &'a mut [Interval],
    ) -> Result<Self, Error> {
        info.validate().map_err(Error::BootInfo)?;
        if info.memmap_count != regions.len() as u64 {
            return Err(Error::Regions(BootInfoError::BadTotalSize));
        }
        crate::bootinfo::validate_regions(regions).map_err(Error::Regions)?;
        if output.len() < MAX_OUT.min(regions.len().saturating_mul(2)) {
            return Err(Error::Normalize(NormalizeError::TooMany));
        }
        let count = memmap_norm::normalize_mem_regions(regions, output)
            .map_err(Error::Normalize)?;
        let normalized = &output[..count];
        memmap_norm::check_normalized(normalized)
            .map_err(Error::InvalidNormalizedMap)?;
        if normalized
            .iter()
            .filter(|interval| interval.kind == memmap_norm::Kind::Usable)
            .filter_map(Interval::range)
            .next()
            .is_none()
        {
            return Err(Error::NoUsableMemory);
        }
        Ok(Self { info, regions, normalized })
    }

    pub fn usable_ranges(&self) -> impl Iterator<Item = PhysRange> + '_ {
        memmap_norm::usable_ranges(self.normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootinfo::{ARCH_X86_64, SIZE};

    fn info(count: usize) -> BootInfo {
        BootInfo {
            magic: crate::bootinfo::MAGIC,
            version: crate::bootinfo::VERSION,
            total_size: (SIZE + count * crate::bootinfo::MEM_REGION_SIZE) as u32,
            arch: ARCH_X86_64,
            flags: 0,
            memmap_paddr: 0x8000,
            memmap_count: count as u64,
            kernel_phys_start: 0x10_0000,
            kernel_phys_end: 0x20_0000,
            kernel_entry: 0x10_0020,
            framebuffer_paddr: 0,
            framebuffer_size: 0,
            rsdp_paddr: 0,
        }
    }

    fn region(start: u64, size: u64, kind: u32) -> MemRegion {
        MemRegion { start, size, kind, _pad: 0 }
    }

    #[test]
    fn validates_and_normalizes_before_exposing_usable_ranges() {
        let regions = [region(0x1000, 0x9000, 1), region(0x3000, 0x1000, 2)];
        let mut output = [Interval::EMPTY; MAX_OUT];
        let boot = ValidatedBoot::validate(&info(regions.len()), &regions, &mut output).unwrap();
        let ranges: Vec<_> = boot.usable_ranges().collect();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start(), 0x1000);
        assert_eq!(ranges[0].end(), 0x3000);
        assert_eq!(ranges[1].start(), 0x4000);
    }

    #[test]
    fn rejects_malformed_and_unsupported_handoffs() {
        let regions = [region(0x1000, 0x1000, 1)];
        let mut output = [Interval::EMPTY; MAX_OUT];
        let mut bad = info(1);
        bad.version = 99;
        assert!(matches!(
            ValidatedBoot::validate(&bad, &regions, &mut output),
            Err(Error::BootInfo(BootInfoError::UnsupportedVersion(99)))
        ));
        let mut bad = info(1);
        bad.memmap_count = 2;
        assert!(matches!(
            ValidatedBoot::validate(&bad, &regions, &mut output),
            Err(Error::BootInfo(BootInfoError::BadTotalSize))
                | Err(Error::Regions(BootInfoError::BadTotalSize))
        ));
    }

    #[test]
    fn rejects_overflow_and_reserved_only_maps() {
        let regions = [region(u64::MAX, 2, 1)];
        let mut output = [Interval::EMPTY; MAX_OUT];
        assert!(matches!(
            ValidatedBoot::validate(&info(1), &regions, &mut output),
            Err(Error::Regions(BootInfoError::RangeOverflow))
        ));
        let regions = [region(0x1000, 0x1000, 2)];
        let mut output = [Interval::EMPTY; MAX_OUT];
        assert_eq!(
            ValidatedBoot::validate(&info(1), &regions, &mut output),
            Err(Error::NoUsableMemory)
        );
    }
}
