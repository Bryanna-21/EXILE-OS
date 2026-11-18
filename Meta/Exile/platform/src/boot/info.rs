//! Versioned, offset-based boot-info handoff structure (bootloader -> kernel).
//!
//! Layout is explicit little-endian bytes, not a `repr(C)` cast, so there is no
//! alignment/UB hazard and no `unsafe`. Integrity is CRC-32 (detects accidental
//! corruption only; it is NOT authentication).

/// Magic: "EXBI".
pub const MAGIC: u32 = 0x4942_5845;
/// Layout major version. A different major is incompatible.
pub const VERSION_MAJOR: u16 = 1;
/// Layout minor version. Readers accept any minor >= what they need.
pub const VERSION_MINOR: u16 = 0;
/// Header size in bytes.
pub const HEADER_LEN: usize = 48;

/// CRC-32 (IEEE 802.3, reflected, init/xorout 0xFFFFFFFF).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Boot flags.
pub mod flags {
    /// Booted via UEFI.
    pub const UEFI: u32 = 1;
    /// Boot services have been exited (memory map is final).
    pub const EXITED_BOOT_SERVICES: u32 = 2;
    /// Recovery / rescue mode requested.
    pub const RECOVERY: u32 = 4;
}

/// Errors from building or parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoError {
    /// Too short.
    Truncated,
    /// Magic mismatch.
    BadMagic,
    /// Major version differs.
    IncompatibleVersion,
    /// Total length field wrong or beyond buffer.
    BadLength,
    /// CRC mismatch.
    BadCrc,
    /// A (offset,len) pair points outside the structure or overlaps the header.
    BadRange,
    /// Destination buffer too small.
    NoSpace,
}

/// Parsed, validated view over a boot-info blob.
#[derive(Debug, Clone, Copy)]
pub struct BootInfo<'a> {
    blob: &'a [u8],
    /// Version minor found in the blob.
    pub minor: u16,
    /// Boot flags.
    pub flags: u32,
    /// Physical address of the ACPI RSDP, or 0.
    pub rsdp: u64,
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn rd64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

/// Header layout (LE): 0 magic, 4 major(u16), 6 minor(u16), 8 total_len, 12 crc32 (computed with
/// this field zero), 16 flags, 20 reserved, 24 rsdp(u64), 32 memmap_off, 36 memmap_len,
/// 40 cmdline_off, 44 cmdline_len.
pub fn build(
    buf: &mut [u8],
    flags: u32,
    rsdp: u64,
    memmap: &[u8],
    cmdline: &[u8],
) -> Result<usize, InfoError> {
    let mm_off = HEADER_LEN;
    let cl_off = mm_off + memmap.len();
    let total = cl_off + cmdline.len();
    if buf.len() < total {
        return Err(InfoError::NoSpace);
    }
    buf[..total].fill(0);
    buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
    buf[4..6].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
    buf[6..8].copy_from_slice(&VERSION_MINOR.to_le_bytes());
    buf[8..12].copy_from_slice(&(total as u32).to_le_bytes());
    buf[16..20].copy_from_slice(&flags.to_le_bytes());
    buf[24..32].copy_from_slice(&rsdp.to_le_bytes());
    buf[32..36].copy_from_slice(&(mm_off as u32).to_le_bytes());
    buf[36..40].copy_from_slice(&(memmap.len() as u32).to_le_bytes());
    buf[40..44].copy_from_slice(&(cl_off as u32).to_le_bytes());
    buf[44..48].copy_from_slice(&(cmdline.len() as u32).to_le_bytes());
    buf[mm_off..cl_off].copy_from_slice(memmap);
    buf[cl_off..total].copy_from_slice(cmdline);
    let c = crc32(&buf[..total]);
    buf[12..16].copy_from_slice(&c.to_le_bytes());
    Ok(total)
}

impl<'a> BootInfo<'a> {
    /// Validate and wrap a blob. Every offset/length is range-checked before use.
    pub fn parse(b: &'a [u8]) -> Result<Self, InfoError> {
        if b.len() < HEADER_LEN {
            return Err(InfoError::Truncated);
        }
        if rd32(b, 0) != MAGIC {
            return Err(InfoError::BadMagic);
        }
        let major = u16::from_le_bytes([b[4], b[5]]);
        if major != VERSION_MAJOR {
            return Err(InfoError::IncompatibleVersion);
        }
        let total = rd32(b, 8) as usize;
        if total < HEADER_LEN || total > b.len() {
            return Err(InfoError::BadLength);
        }
        let mut tmp = [0u8; 4];
        tmp.copy_from_slice(&b[12..16]);
        let stored = u32::from_le_bytes(tmp);
        // CRC over the blob with the crc field treated as zero.
        let mut c = 0xFFFF_FFFFu32;
        for (i, &x) in b[..total].iter().enumerate() {
            let x = if (12..16).contains(&i) { 0 } else { x };
            c ^= u32::from(x);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
        }
        if !c != stored {
            return Err(InfoError::BadCrc);
        }
        let info = BootInfo {
            blob: &b[..total],
            minor: u16::from_le_bytes([b[6], b[7]]),
            flags: rd32(b, 16),
            rsdp: rd64(b, 24),
        };
        info.range(32)?;
        info.range(40)?;
        Ok(info)
    }
    fn range(&self, hdr_off: usize) -> Result<&'a [u8], InfoError> {
        let off = rd32(self.blob, hdr_off) as usize;
        let len = rd32(self.blob, hdr_off + 4) as usize;
        let end = off.checked_add(len).ok_or(InfoError::BadRange)?;
        if off < HEADER_LEN || end > self.blob.len() {
            return Err(InfoError::BadRange);
        }
        Ok(&self.blob[off..end])
    }
    /// Raw memory-map bytes (format given by `flags::UEFI`).
    pub fn memmap(&self) -> &'a [u8] {
        self.range(32).unwrap_or(&[])
    }
    /// Kernel command line bytes.
    pub fn cmdline(&self) -> &'a [u8] {
        self.range(40).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_check_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }
    #[test]
    fn round_trip() {
        let mut buf = [0u8; 256];
        let n = build(
            &mut buf,
            flags::UEFI | flags::EXITED_BOOT_SERVICES,
            0xE_0000,
            &[1, 2, 3, 4],
            b"root=slotA",
        )
        .unwrap();
        let i = BootInfo::parse(&buf[..n]).unwrap();
        assert_eq!(i.flags, 3);
        assert_eq!(i.rsdp, 0xE_0000);
        assert_eq!(i.memmap(), &[1, 2, 3, 4]);
        assert_eq!(i.cmdline(), b"root=slotA");
    }
    #[test]
    fn every_single_bit_flip_is_detected() {
        let mut buf = [0u8; 128];
        let n = build(&mut buf, 1, 2, &[9; 8], b"x").unwrap();
        for byte in 0..n {
            for bit in 0..8 {
                let mut c = buf;
                c[byte] ^= 1 << bit;
                assert!(
                    BootInfo::parse(&c[..n]).is_err(),
                    "undetected flip at byte {byte} bit {bit}"
                );
            }
        }
    }
    #[test]
    fn version_and_size_rules() {
        let mut buf = [0u8; 128];
        let n = build(&mut buf, 0, 0, &[], b"").unwrap();
        let mut v = buf;
        v[4] = 2; // major 2
        assert_eq!(
            BootInfo::parse(&v[..n]).unwrap_err(),
            InfoError::IncompatibleVersion
        );
        assert_eq!(
            BootInfo::parse(&buf[..10]).unwrap_err(),
            InfoError::Truncated
        );
        assert_eq!(
            BootInfo::parse(&buf[..n - 1]).unwrap_err(),
            InfoError::Truncated
        ); // shorter than header
        let mut p = [0u8; 128];
        let pn = build(&mut p, 0, 0, &[1, 2, 3], b"").unwrap();
        assert_eq!(
            BootInfo::parse(&p[..pn - 1]).unwrap_err(),
            InfoError::BadLength
        );
        assert_eq!(
            build(&mut [0u8; 10], 0, 0, &[], b"").unwrap_err(),
            InfoError::NoSpace
        );
        let mut m = buf;
        m[0] = 0;
        assert_eq!(BootInfo::parse(&m[..n]).unwrap_err(), InfoError::BadMagic);
    }
    #[test]
    fn newer_minor_is_accepted() {
        let mut buf = [0u8; 128];
        let n = build(&mut buf, 0, 0, &[], b"").unwrap();
        buf[6] = 9;
        buf[12..16].fill(0);
        let c = crc32(&buf[..n]);
        buf[12..16].copy_from_slice(&c.to_le_bytes());
        assert_eq!(BootInfo::parse(&buf[..n]).unwrap().minor, 9);
    }
    #[test]
    fn malicious_ranges_with_valid_crc_are_rejected() {
        let mut buf = [0u8; 128];
        let n = build(&mut buf, 0, 0, &[1, 2], b"ab").unwrap();
        for (field, val) in [
            (32usize, 4u32),
            (36, 0xFFFF_FFFF),
            (40, 1000),
            (44, 0xFFFF_FFF0),
        ] {
            let mut c = buf;
            c[field..field + 4].copy_from_slice(&val.to_le_bytes());
            c[12..16].fill(0);
            let crc = crc32(&c[..n]);
            c[12..16].copy_from_slice(&crc.to_le_bytes());
            assert_eq!(
                BootInfo::parse(&c[..n]).unwrap_err(),
                InfoError::BadRange,
                "field {field}"
            );
        }
    }
}
