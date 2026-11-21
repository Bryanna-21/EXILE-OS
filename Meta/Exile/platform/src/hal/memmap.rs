//! Physical memory map acquisition and validation (Multiboot 1 and UEFI inputs).
//!
//! Decoding and validation are pure and host-tested. Whether a map reported by firmware is *true* (the "usable" RAM
//! really exists) cannot be established in software; see `docs/THREAT_MODEL.md` of EUK. This code rejects maps that are
//! self-inconsistent, which is the most it can do.

/// What a physical range may be used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    /// Free RAM.
    Usable,
    /// Firmware, device or otherwise off limits.
    Reserved,
    /// ACPI tables that may be reclaimed after they are parsed.
    AcpiReclaimable,
    /// ACPI non-volatile storage.
    AcpiNvs,
    /// Reported defective.
    Bad,
    /// Memory-mapped I/O.
    Mmio,
    /// Bootloader memory reusable once the handoff structures are consumed (UEFI boot services / loader data).
    BootloaderReclaimable,
}

/// One physical range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    /// First byte.
    pub base: u64,
    /// Length in bytes.
    pub len: u64,
    /// Classification.
    pub kind: RegionKind,
}

impl Region {
    /// One past the last byte, or `None` if it does not fit in 64 bits.
    pub fn end(&self) -> Option<u64> {
        self.base.checked_add(self.len)
    }
}

/// Why a map was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    /// `base + len` overflows.
    Overflow,
    /// A region has length zero.
    ZeroLength,
    /// More regions than the fixed capacity.
    Capacity,
    /// Regions `a` and `b` (indices after sorting) overlap.
    Overlap {
        /// Lower-addressed region.
        a: usize,
        /// Higher-addressed region.
        b: usize,
    },
    /// The input ended in the middle of an entry.
    Truncated,
    /// An entry or descriptor size is smaller than the minimum.
    BadEntrySize,
}

/// A fixed-capacity memory map.
#[derive(Clone, Debug)]
pub struct MemoryMap<const N: usize> {
    regions: [Region; N],
    len: usize,
}

const EMPTY: Region = Region {
    base: 0,
    len: 0,
    kind: RegionKind::Reserved,
};

impl<const N: usize> Default for MemoryMap<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> MemoryMap<N> {
    /// An empty map.
    pub const fn new() -> Self {
        Self {
            regions: [EMPTY; N],
            len: 0,
        }
    }

    /// Appends a region after checking it individually.
    pub fn push(&mut self, r: Region) -> Result<(), MapError> {
        if r.len == 0 {
            return Err(MapError::ZeroLength);
        }
        if r.end().is_none() {
            return Err(MapError::Overflow);
        }
        if self.len == N {
            return Err(MapError::Capacity);
        }
        self.regions[self.len] = r;
        self.len += 1;
        Ok(())
    }

    /// The regions in their current order.
    pub fn regions(&self) -> &[Region] {
        &self.regions[..self.len]
    }

    /// Sorts by base address (insertion sort: no allocation, N is small).
    pub fn sort(&mut self) {
        for i in 1..self.len {
            let mut j = i;
            while j > 0 && self.regions[j - 1].base > self.regions[j].base {
                self.regions.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    /// Sorts, then rejects any overlap. Adjacent regions are allowed. Firmware maps that overlap are not trusted.
    pub fn validate(&mut self) -> Result<(), MapError> {
        self.sort();
        for i in 1..self.len {
            let prev = &self.regions[i - 1];
            // `push` guarantees `end()` is Some.
            if prev.base + prev.len > self.regions[i].base {
                return Err(MapError::Overlap { a: i - 1, b: i });
            }
        }
        Ok(())
    }

    /// Merges adjacent regions of the same kind (call after `validate`).
    pub fn coalesce(&mut self) {
        if self.len == 0 {
            return;
        }
        let mut out = 0;
        for i in 1..self.len {
            let cur = self.regions[i];
            let last = self.regions[out];
            if last.kind == cur.kind
                && last.base + last.len == cur.base
                && let Some(total) = last.len.checked_add(cur.len)
            {
                self.regions[out].len = total;
                continue;
            }
            out += 1;
            self.regions[out] = cur;
        }
        self.len = out + 1;
    }

    /// Usable ranges shrunk inward to `align`-byte boundaries (`align` must be a power of two); empty results are skipped.
    pub fn usable_aligned(&self, align: u64) -> UsableIter<'_> {
        debug_assert!(align.is_power_of_two());
        UsableIter {
            regions: self.regions(),
            idx: 0,
            align,
        }
    }

    /// Total bytes in usable regions after page alignment.
    pub fn usable_bytes(&self, align: u64) -> u64 {
        self.usable_aligned(align).map(|(b, e)| e - b).sum()
    }
}

/// Iterator returned by [`MemoryMap::usable_aligned`].
pub struct UsableIter<'a> {
    regions: &'a [Region],
    idx: usize,
    align: u64,
}

impl Iterator for UsableIter<'_> {
    type Item = (u64, u64);
    fn next(&mut self) -> Option<(u64, u64)> {
        while self.idx < self.regions.len() {
            let r = self.regions[self.idx];
            self.idx += 1;
            if r.kind != RegionKind::Usable {
                continue;
            }
            let mask = self.align - 1;
            let start = r.base.checked_add(mask)? & !mask;
            let end = (r.base + r.len) & !mask;
            if start < end {
                return Some((start, end));
            }
        }
        None
    }
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn le64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// Parses a Multiboot 1 memory map (`mmap_addr`/`mmap_length` contents). Each entry is
/// `size:u32` (not counting itself), then `base:u64, len:u64, type:u32`, possibly followed by more bytes.
/// Zero-length entries are skipped (firmware emits them); everything else is validated by [`MemoryMap::validate`].
pub fn parse_multiboot1<const N: usize>(bytes: &[u8]) -> Result<MemoryMap<N>, MapError> {
    let mut map = MemoryMap::new();
    let mut off = 0usize;
    while off < bytes.len() {
        if bytes.len() - off < 4 {
            return Err(MapError::Truncated);
        }
        let size = le32(bytes, off) as usize;
        if size < 20 {
            return Err(MapError::BadEntrySize);
        }
        let total = size.checked_add(4).ok_or(MapError::Truncated)?;
        if bytes.len() - off < total {
            return Err(MapError::Truncated);
        }
        let base = le64(bytes, off + 4);
        let len = le64(bytes, off + 12);
        let kind = match le32(bytes, off + 20) {
            1 => RegionKind::Usable,
            3 => RegionKind::AcpiReclaimable,
            4 => RegionKind::AcpiNvs,
            5 => RegionKind::Bad,
            _ => RegionKind::Reserved,
        };
        if len != 0 {
            map.push(Region { base, len, kind })?;
        }
        off += total;
    }
    map.validate()?;
    Ok(map)
}

/// Parses a UEFI memory map (array of `EFI_MEMORY_DESCRIPTOR` with stride `descriptor_size`, which may exceed 40).
///
/// Boot-services and loader memory is reported as [`RegionKind::BootloaderReclaimable`]; that is only correct **after**
/// `ExitBootServices`, so a bootloader must call this on the map it obtained for that call, not an earlier one.
pub fn parse_uefi<const N: usize>(
    bytes: &[u8],
    descriptor_size: usize,
) -> Result<MemoryMap<N>, MapError> {
    if descriptor_size < 40 {
        return Err(MapError::BadEntrySize);
    }
    let mut map = MemoryMap::new();
    let mut off = 0usize;
    while off < bytes.len() {
        if bytes.len() - off < descriptor_size {
            return Err(MapError::Truncated);
        }
        let ty = le32(bytes, off);
        let phys = le64(bytes, off + 8);
        let pages = le64(bytes, off + 24);
        let len = pages.checked_mul(4096).ok_or(MapError::Overflow)?;
        let kind = match ty {
            7 => RegionKind::Usable,
            1..=4 => RegionKind::BootloaderReclaimable,
            9 => RegionKind::AcpiReclaimable,
            10 => RegionKind::AcpiNvs,
            11 | 12 => RegionKind::Mmio,
            8 => RegionKind::Bad,
            _ => RegionKind::Reserved,
        };
        if len != 0 {
            map.push(Region {
                base: phys,
                len,
                kind,
            })?;
        }
        off += descriptor_size;
    }
    map.validate()?;
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn mb(entries: &[(u64, u64, u32)], extra: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for &(b, l, t) in entries {
            v.extend_from_slice(&((20 + extra) as u32).to_le_bytes());
            v.extend_from_slice(&b.to_le_bytes());
            v.extend_from_slice(&l.to_le_bytes());
            v.extend_from_slice(&t.to_le_bytes());
            v.extend(core::iter::repeat_n(0xAA, extra));
        }
        v
    }

    fn efi(entries: &[(u32, u64, u64)], stride: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for &(t, p, pages) in entries {
            let mut d = std::vec![0u8; stride];
            d[0..4].copy_from_slice(&t.to_le_bytes());
            d[8..16].copy_from_slice(&p.to_le_bytes());
            d[24..32].copy_from_slice(&pages.to_le_bytes());
            v.extend_from_slice(&d);
        }
        v
    }

    #[test]
    fn multiboot_typical_map() {
        let b = mb(
            &[
                (0, 0x9FC00, 1),
                (0x9FC00, 0x400, 2),
                (0xF0000, 0x10000, 2),
                (0x100000, 0x7EE0000, 1),
                (0x7FE0000, 0x20000, 2),
            ],
            0,
        );
        let m: MemoryMap<16> = parse_multiboot1(&b).unwrap();
        assert_eq!(m.regions().len(), 5);
        assert_eq!(m.regions()[0].kind, RegionKind::Usable);
        assert_eq!(m.usable_bytes(4096), 0x9F000 + 0x7EE0000);
    }

    #[test]
    fn multiboot_entry_with_extra_bytes_uses_stride() {
        let b = mb(&[(0x100000, 0x100000, 1), (0x200000, 0x1000, 3)], 8);
        let m: MemoryMap<4> = parse_multiboot1(&b).unwrap();
        assert_eq!(m.regions()[1].kind, RegionKind::AcpiReclaimable);
    }

    #[test]
    fn multiboot_rejects_bad_input() {
        assert_eq!(
            parse_multiboot1::<4>(&[1, 0]).unwrap_err(),
            MapError::Truncated
        );
        let mut b = mb(&[(0, 4096, 1)], 0);
        b[0] = 10;
        assert_eq!(
            parse_multiboot1::<4>(&b).unwrap_err(),
            MapError::BadEntrySize
        );
        let b = mb(&[(0, 4096, 1)], 0);
        assert_eq!(
            parse_multiboot1::<4>(&b[..20]).unwrap_err(),
            MapError::Truncated
        );
        let b = mb(&[(u64::MAX - 10, 100, 1)], 0);
        assert_eq!(parse_multiboot1::<4>(&b).unwrap_err(), MapError::Overflow);
        let b = mb(&[(0, 4096, 1), (0x1000, 4096, 1), (0x2000, 4096, 1)], 0);
        assert_eq!(parse_multiboot1::<2>(&b).unwrap_err(), MapError::Capacity);
    }

    #[test]
    fn overlap_is_rejected_after_sorting() {
        let b = mb(&[(0x2000, 0x2000, 1), (0x0, 0x3000, 1)], 0);
        assert_eq!(
            parse_multiboot1::<4>(&b).unwrap_err(),
            MapError::Overlap { a: 0, b: 1 }
        );
    }

    #[test]
    fn adjacent_is_allowed_and_zero_length_skipped() {
        let b = mb(&[(0, 0x1000, 1), (0x1000, 0, 2), (0x1000, 0x1000, 1)], 0);
        let mut m: MemoryMap<4> = parse_multiboot1(&b).unwrap();
        assert_eq!(m.regions().len(), 2);
        m.coalesce();
        assert_eq!(
            m.regions(),
            &[Region {
                base: 0,
                len: 0x2000,
                kind: RegionKind::Usable
            }]
        );
    }

    #[test]
    fn coalesce_keeps_different_kinds_apart() {
        let mut m: MemoryMap<4> = MemoryMap::new();
        m.push(Region {
            base: 0,
            len: 0x1000,
            kind: RegionKind::Usable,
        })
        .unwrap();
        m.push(Region {
            base: 0x1000,
            len: 0x1000,
            kind: RegionKind::Reserved,
        })
        .unwrap();
        m.push(Region {
            base: 0x2000,
            len: 0x1000,
            kind: RegionKind::Usable,
        })
        .unwrap();
        m.validate().unwrap();
        m.coalesce();
        assert_eq!(m.regions().len(), 3);
    }

    #[test]
    fn usable_is_shrunk_inward_to_pages() {
        let mut m: MemoryMap<4> = MemoryMap::new();
        m.push(Region {
            base: 0x1800,
            len: 0x3000,
            kind: RegionKind::Usable,
        })
        .unwrap(); // 0x1800..0x4800
        m.push(Region {
            base: 0x10000,
            len: 0x800,
            kind: RegionKind::Usable,
        })
        .unwrap(); // smaller than a page: dropped
        m.push(Region {
            base: 0x20000,
            len: 0x1000,
            kind: RegionKind::Reserved,
        })
        .unwrap();
        m.validate().unwrap();
        let v: Vec<_> = m.usable_aligned(4096).collect();
        assert_eq!(v, std::vec![(0x2000, 0x4000)]);
    }

    #[test]
    fn push_rejects_zero_and_overflow() {
        let mut m: MemoryMap<2> = MemoryMap::new();
        assert_eq!(
            m.push(Region {
                base: 0,
                len: 0,
                kind: RegionKind::Usable
            }),
            Err(MapError::ZeroLength)
        );
        assert_eq!(
            m.push(Region {
                base: u64::MAX,
                len: 2,
                kind: RegionKind::Usable
            }),
            Err(MapError::Overflow)
        );
    }

    #[test]
    fn uefi_map_with_larger_descriptor_stride() {
        let b = efi(
            &[
                (3, 0x0, 16),
                (7, 0x100000, 256),
                (9, 0x200000, 4),
                (11, 0xFEE00000, 1),
                (8, 0x300000, 1),
                (5, 0x400000, 2),
            ],
            48,
        );
        let m: MemoryMap<16> = parse_uefi(&b, 48).unwrap();
        let k: Vec<_> = m.regions().iter().map(|r| r.kind).collect();
        assert_eq!(
            k,
            std::vec![
                RegionKind::BootloaderReclaimable,
                RegionKind::Usable,
                RegionKind::AcpiReclaimable,
                RegionKind::Bad,
                RegionKind::Reserved,
                RegionKind::Mmio
            ]
        );
        assert_eq!(m.regions()[1].len, 256 * 4096);
    }

    #[test]
    fn uefi_rejects_bad_input() {
        assert_eq!(
            parse_uefi::<4>(&[], 39).unwrap_err(),
            MapError::BadEntrySize
        );
        assert_eq!(
            parse_uefi::<4>(&efi(&[(7, 0, 1)], 40)[..39], 40).unwrap_err(),
            MapError::Truncated
        );
        assert_eq!(
            parse_uefi::<4>(&efi(&[(7, 0, u64::MAX)], 40), 40).unwrap_err(),
            MapError::Overflow
        );
        assert_eq!(
            parse_uefi::<4>(&efi(&[(7, 0, 4), (7, 0x2000, 4)], 40), 40).unwrap_err(),
            MapError::Overlap { a: 0, b: 1 }
        );
    }
}
