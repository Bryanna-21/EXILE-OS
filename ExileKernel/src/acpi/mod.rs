//! ACPI table discovery: RSDP, RSDT/XSDT, generic SDT validation (Mission C3.3).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Firmware hands the kernel physical addresses; none of them is trusted. Every byte comes
//! through [`PhysReader`], which refuses ranges outside the physical window the kernel has
//! actually mapped (today: the 1 GiB bootstrap identity map). Tables are parsed from small
//! bounded copies, never by casting a firmware pointer to a Rust reference, and every length
//! is checked before it is used. Nothing here allocates.
//!
//! Supported: ACPI 1.0 (RSDT) and 2.0+ (XSDT, preferred, with RSDT fallback) as reachable in
//! the Multiboot v1 environment, where the RSDP must be found by scanning the EBDA and
//! 0xE0000..0xFFFFF. A loader-provided RSDP address can be passed to [`parse_rsdp_at`] directly.
//! Not supported: tables above the mapped window (reported as `Unreadable`, never touched).
#![allow(dead_code)]
#![allow(
    clippy::manual_is_multiple_of,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::identity_op,
    clippy::unusual_byte_groupings
)]

pub mod madt;

/// Largest table accepted. Real MADT/HPET tables are a few hundred bytes to a few KiB.
pub const MAX_TABLE_LEN: u32 = 1 << 20;
/// Most RSDT/XSDT entries examined.
pub const MAX_ROOT_ENTRIES: usize = 512;
pub const SDT_HEADER_LEN: usize = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    /// The physical range is outside the readable window.
    Unreadable(u64),
    BadSignature,
    BadChecksum,
    BadLength,
    /// An entry runs past the end of its table.
    Truncated,
    NotFound,
    TooLarge,
    UnsupportedRevision,
    /// Structurally invalid content (zero-length entry, wrong entry size, duplicate IDs, ...).
    Malformed,
    /// More entries than the fixed-size result can hold.
    TooMany,
    InvalidAddress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadError;

/// Bounded read access to physical memory.
pub trait PhysReader {
    /// Fills `out` from `phys`, or fails without reading anything outside the window.
    fn read(&self, phys: u64, out: &mut [u8]) -> Result<(), ReadError>;
}

/// Reads through the bootstrap identity map. `phys + len` must not exceed `limit`.
pub struct IdentityReader {
    limit: u64,
}

impl IdentityReader {
    /// # Safety
    /// `0..limit` must be identity-mapped, readable memory (or at least free of side-effecting
    /// device registers) for as long as the reader is used.
    pub const unsafe fn new(limit: u64) -> Self {
        Self { limit }
    }
}

impl PhysReader for IdentityReader {
    fn read(&self, phys: u64, out: &mut [u8]) -> Result<(), ReadError> {
        let end = phys.checked_add(out.len() as u64).ok_or(ReadError)?;
        if end > self.limit {
            return Err(ReadError);
        }
        for (i, b) in out.iter_mut().enumerate() {
            // SAFETY: `phys + i < end <= limit`, which `IdentityReader::new` guarantees is
            // mapped. Volatile because firmware memory may change under us.
            *b = unsafe { core::ptr::read_volatile((phys + i as u64) as *const u8) };
        }
        Ok(())
    }
}

fn read_exact<R: PhysReader + ?Sized>(r: &R, phys: u64, out: &mut [u8]) -> Result<(), AcpiError> {
    r.read(phys, out).map_err(|_| AcpiError::Unreadable(phys))
}

pub fn le16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
pub fn le32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
pub fn le64(b: &[u8], off: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(a)
}

/// Byte sum modulo 256. A valid ACPI structure sums to zero.
pub fn sum_bytes(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

/// Streams `len` bytes from `phys` and returns their byte sum, without a `len`-sized buffer.
pub fn checksum_stream<R: PhysReader + ?Sized>(
    r: &R,
    phys: u64,
    len: u32,
) -> Result<u8, AcpiError> {
    let mut sum = 0u8;
    let mut done = 0u32;
    let mut chunk = [0u8; 64];
    while done < len {
        let n = (len - done).min(64) as usize;
        read_exact(r, phys + done as u64, &mut chunk[..n])?;
        sum = sum.wrapping_add(sum_bytes(&chunk[..n]));
        done += n as u32;
    }
    Ok(sum)
}

// ---------------------------------------------------------------------------------------------
// RSDP
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsdp {
    pub revision: u8,
    pub rsdt: u32,
    /// Zero for ACPI 1.0.
    pub xsdt: u64,
}

pub const RSDP_SIGNATURE: [u8; 8] = *b"RSD PTR ";
const RSDP_V1_LEN: usize = 20;
const RSDP_V2_LEN: usize = 36;

/// Parses and validates an RSDP image. `bytes` must hold at least the structure.
pub fn parse_rsdp(bytes: &[u8]) -> Result<Rsdp, AcpiError> {
    if bytes.len() < RSDP_V1_LEN {
        return Err(AcpiError::Truncated);
    }
    if bytes[..8] != RSDP_SIGNATURE {
        return Err(AcpiError::BadSignature);
    }
    if sum_bytes(&bytes[..RSDP_V1_LEN]) != 0 {
        return Err(AcpiError::BadChecksum);
    }
    let revision = bytes[15];
    let rsdt = le32(bytes, 16);
    match revision {
        0 => Ok(Rsdp {
            revision,
            rsdt,
            xsdt: 0,
        }),
        1 => Err(AcpiError::UnsupportedRevision), // not defined by any ACPI version
        _ => {
            if bytes.len() < RSDP_V2_LEN {
                return Err(AcpiError::Truncated);
            }
            let length = le32(bytes, 20) as usize;
            if length < RSDP_V2_LEN || length > bytes.len() {
                return Err(AcpiError::BadLength);
            }
            if sum_bytes(&bytes[..length]) != 0 {
                return Err(AcpiError::BadChecksum);
            }
            Ok(Rsdp {
                revision,
                rsdt,
                xsdt: le64(bytes, 24),
            })
        }
    }
}

/// Reads and validates the RSDP at `phys`.
pub fn parse_rsdp_at<R: PhysReader + ?Sized>(r: &R, phys: u64) -> Result<Rsdp, AcpiError> {
    let mut buf = [0u8; 64];
    // Read the v1 part first so a short readable region still validates a v1 RSDP.
    read_exact(r, phys, &mut buf[..RSDP_V1_LEN])?;
    if buf[..8] != RSDP_SIGNATURE {
        return Err(AcpiError::BadSignature);
    }
    if buf[15] >= 2 {
        read_exact(r, phys, &mut buf[..RSDP_V2_LEN])?;
        let length = le32(&buf, 20) as usize;
        if (RSDP_V2_LEN..=buf.len()).contains(&length) {
            read_exact(r, phys, &mut buf[..length])?;
            return parse_rsdp(&buf[..length]);
        }
        return Err(AcpiError::BadLength);
    }
    parse_rsdp(&buf[..RSDP_V1_LEN])
}

/// Scans `[start, end)` on 16-byte boundaries for a valid RSDP. Invalid candidates (bad
/// checksum, unreadable) are skipped. At most 128 KiB is scanned.
pub fn scan_for_rsdp<R: PhysReader + ?Sized>(r: &R, start: u64, end: u64) -> Option<(u64, Rsdp)> {
    let end = end.min(start.saturating_add(128 * 1024));
    let mut addr = start.checked_add(15)? & !15;
    while addr.checked_add(RSDP_V1_LEN as u64)? <= end {
        let mut sig = [0u8; 8];
        if r.read(addr, &mut sig).is_ok() && sig == RSDP_SIGNATURE {
            if let Ok(rsdp) = parse_rsdp_at(r, addr) {
                return Some((addr, rsdp));
            }
        }
        addr += 16;
    }
    None
}

/// Legacy BIOS search: the first KiB of the EBDA (segment at 0x40E), then 0xE0000..0x100000.
pub fn find_rsdp_bios<R: PhysReader + ?Sized>(r: &R) -> Option<(u64, Rsdp)> {
    let mut seg = [0u8; 2];
    if r.read(0x40E, &mut seg).is_ok() {
        let ebda = (u16::from_le_bytes(seg) as u64) << 4;
        // The EBDA lives in conventional memory, just below 0xA0000.
        if (0x8_0000..0xA_0000).contains(&ebda) {
            if let Some(found) = scan_for_rsdp(r, ebda, ebda + 1024) {
                return Some(found);
            }
        }
    }
    scan_for_rsdp(r, 0xE_0000, 0x10_0000)
}

// ---------------------------------------------------------------------------------------------
// SDT header, RSDT/XSDT
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub oem_id: [u8; 6],
}

pub fn parse_header(b: &[u8; SDT_HEADER_LEN]) -> SdtHeader {
    let mut signature = [0u8; 4];
    signature.copy_from_slice(&b[0..4]);
    let mut oem_id = [0u8; 6];
    oem_id.copy_from_slice(&b[10..16]);
    SdtHeader {
        signature,
        length: le32(b, 4),
        revision: b[8],
        oem_id,
    }
}

pub fn read_header<R: PhysReader + ?Sized>(r: &R, phys: u64) -> Result<SdtHeader, AcpiError> {
    let mut b = [0u8; SDT_HEADER_LEN];
    read_exact(r, phys, &mut b)?;
    Ok(parse_header(&b))
}

/// Reads the header at `phys` and checks length bounds and the whole-table checksum.
pub fn validate_table<R: PhysReader + ?Sized>(r: &R, phys: u64) -> Result<SdtHeader, AcpiError> {
    let h = read_header(r, phys)?;
    if (h.length as usize) < SDT_HEADER_LEN {
        return Err(AcpiError::BadLength);
    }
    if h.length > MAX_TABLE_LEN {
        return Err(AcpiError::TooLarge);
    }
    if checksum_stream(r, phys, h.length)? != 0 {
        return Err(AcpiError::BadChecksum);
    }
    Ok(h)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Root {
    pub phys: u64,
    pub xsdt: bool,
    pub header: SdtHeader,
}

impl Root {
    pub fn entry_size(&self) -> usize {
        if self.xsdt { 8 } else { 4 }
    }
    pub fn entry_count(&self) -> usize {
        (self.header.length as usize - SDT_HEADER_LEN) / self.entry_size()
    }
}

fn validate_root<R: PhysReader + ?Sized>(
    r: &R,
    phys: u64,
    sig: [u8; 4],
    xsdt: bool,
) -> Result<Root, AcpiError> {
    if phys == 0 {
        return Err(AcpiError::InvalidAddress);
    }
    let header = validate_table(r, phys)?;
    if header.signature != sig {
        return Err(AcpiError::BadSignature);
    }
    Ok(Root { phys, xsdt, header })
}

/// XSDT when the RSDP is ACPI 2.0+ and the XSDT validates, otherwise the RSDT.
pub fn root_table<R: PhysReader + ?Sized>(r: &R, rsdp: &Rsdp) -> Result<Root, AcpiError> {
    let mut first_err = None;
    if rsdp.revision >= 2 && rsdp.xsdt != 0 {
        match validate_root(r, rsdp.xsdt, *b"XSDT", true) {
            Ok(root) => return Ok(root),
            Err(e) => first_err = Some(e),
        }
    }
    match validate_root(r, rsdp.rsdt as u64, *b"RSDT", false) {
        Ok(root) => Ok(root),
        Err(e) => Err(first_err.unwrap_or(e)),
    }
}

/// Finds the first table with `signature` listed in the root table and fully validates it.
/// Entries that are zero or unreadable are skipped (firmware may list tables above the mapped
/// window); if nothing else matches, the first unreadable one is reported instead of `NotFound`.
pub fn find_table<R: PhysReader + ?Sized>(
    r: &R,
    rsdp: &Rsdp,
    signature: [u8; 4],
) -> Result<(u64, SdtHeader), AcpiError> {
    let root = root_table(r, rsdp)?;
    let mut first_unreadable = None;
    for i in 0..root.entry_count().min(MAX_ROOT_ENTRIES) {
        let at = root.phys + SDT_HEADER_LEN as u64 + (i * root.entry_size()) as u64;
        let mut e = [0u8; 8];
        read_exact(r, at, &mut e[..root.entry_size()])?;
        let addr = if root.xsdt {
            u64::from_le_bytes(e)
        } else {
            le32(&e, 0) as u64
        };
        if addr == 0 {
            continue;
        }
        let header = match read_header(r, addr) {
            Ok(h) => h,
            Err(AcpiError::Unreadable(a)) => {
                first_unreadable.get_or_insert(a);
                continue;
            }
            Err(e) => return Err(e),
        };
        if header.signature == signature {
            let header = validate_table(r, addr)?;
            return Ok((addr, header));
        }
    }
    Err(match first_unreadable {
        Some(a) => AcpiError::Unreadable(a),
        None => AcpiError::NotFound,
    })
}

/// Finds a table starting from the legacy BIOS RSDP search.
pub fn find_table_bios<R: PhysReader + ?Sized>(
    r: &R,
    signature: [u8; 4],
) -> Result<(u64, SdtHeader), AcpiError> {
    let (_, rsdp) = find_rsdp_bios(r).ok_or(AcpiError::NotFound)?;
    find_table(r, &rsdp, signature)
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// Sparse fake physical memory: a list of (base, bytes) regions; reads must fall inside one.
    #[derive(Default)]
    pub struct FakeMem {
        pub regions: Vec<(u64, Vec<u8>)>,
        pub reads: std::cell::Cell<u64>,
    }

    impl FakeMem {
        pub fn put(&mut self, phys: u64, bytes: &[u8]) {
            self.regions.push((phys, bytes.to_vec()));
        }
    }

    impl PhysReader for FakeMem {
        fn read(&self, phys: u64, out: &mut [u8]) -> Result<(), ReadError> {
            self.reads.set(self.reads.get() + 1);
            let end = phys.checked_add(out.len() as u64).ok_or(ReadError)?;
            for (base, data) in self.regions.iter().rev() {
                if phys >= *base && end <= base + data.len() as u64 {
                    let o = (phys - base) as usize;
                    out.copy_from_slice(&data[o..o + out.len()]);
                    return Ok(());
                }
            }
            Err(ReadError)
        }
    }

    pub fn fix_checksum(t: &mut [u8], at: usize, span: usize) {
        t[at] = 0;
        let s = sum_bytes(&t[..span]);
        t[at] = 0u8.wrapping_sub(s);
    }

    /// A valid SDT: header + `body`, checksum fixed.
    pub fn sdt(sig: &[u8; 4], rev: u8, body: &[u8]) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(sig);
        t.extend_from_slice(&((SDT_HEADER_LEN + body.len()) as u32).to_le_bytes());
        t.push(rev);
        t.push(0); // checksum
        t.extend_from_slice(b"EUKTST");
        t.extend_from_slice(b"EUKTABLE");
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(b"EUK ");
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(body);
        let n = t.len();
        fix_checksum(&mut t, 9, n);
        t
    }

    pub fn rsdp_v1(rsdt: u32) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(b"RSD PTR ");
        t.push(0); // checksum
        t.extend_from_slice(b"EUKTST");
        t.push(0); // revision
        t.extend_from_slice(&rsdt.to_le_bytes());
        fix_checksum(&mut t, 8, 20);
        t
    }

    pub fn rsdp_v2(rsdt: u32, xsdt: u64) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend_from_slice(b"RSD PTR ");
        t.push(0);
        t.extend_from_slice(b"EUKTST");
        t.push(2);
        t.extend_from_slice(&rsdt.to_le_bytes());
        t.extend_from_slice(&36u32.to_le_bytes());
        t.extend_from_slice(&xsdt.to_le_bytes());
        t.push(0); // extended checksum
        t.extend_from_slice(&[0, 0, 0]);
        fix_checksum(&mut t, 8, 20);
        let n = t.len();
        fix_checksum(&mut t, 32, n);
        // The v1 checksum covers bytes 0..20 only and is unaffected by byte 32.
        assert_eq!(sum_bytes(&t[..20]), 0);
        t
    }

    /// RSDT/XSDT listing `addrs`.
    pub fn root(xsdt: bool, addrs: &[u64]) -> Vec<u8> {
        let mut body = Vec::new();
        for a in addrs {
            if xsdt {
                body.extend_from_slice(&a.to_le_bytes());
            } else {
                body.extend_from_slice(&(*a as u32).to_le_bytes());
            }
        }
        sdt(if xsdt { b"XSDT" } else { b"RSDT" }, 1, &body)
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn checksum_helpers() {
        assert_eq!(sum_bytes(&[]), 0);
        assert_eq!(sum_bytes(&[0xFF, 0x01]), 0);
        assert_eq!(sum_bytes(&[0x80, 0x80, 0x01]), 1);
        let mut m = FakeMem::default();
        let t = sdt(b"TEST", 1, &[7u8; 300]);
        m.put(0x1000, &t);
        assert_eq!(checksum_stream(&m, 0x1000, t.len() as u32), Ok(0));
        assert_eq!(
            checksum_stream(&m, 0x1000, t.len() as u32 + 1),
            Err(AcpiError::Unreadable(0x1000 + 64 * 5))
        );
        assert_eq!(checksum_stream(&m, 0x1000, 0), Ok(0));
    }

    #[test]
    fn rsdp_v1_and_v2_parse() {
        let v1 = rsdp_v1(0x5000);
        assert_eq!(
            parse_rsdp(&v1),
            Ok(Rsdp {
                revision: 0,
                rsdt: 0x5000,
                xsdt: 0
            })
        );
        let v2 = rsdp_v2(0x5000, 0x6000);
        assert_eq!(
            parse_rsdp(&v2),
            Ok(Rsdp {
                revision: 2,
                rsdt: 0x5000,
                xsdt: 0x6000
            })
        );
    }

    #[test]
    fn rsdp_rejections() {
        let good = rsdp_v2(0x5000, 0x6000);
        let mut bad = good.clone();
        bad[0] = b'X';
        assert_eq!(parse_rsdp(&bad), Err(AcpiError::BadSignature));
        let mut bad = good.clone();
        bad[16] ^= 1; // v1 checksum
        assert_eq!(parse_rsdp(&bad), Err(AcpiError::BadChecksum));
        let mut bad = good.clone();
        bad[24] ^= 1; // extended area only: v1 checksum still fine
        assert_eq!(parse_rsdp(&bad), Err(AcpiError::BadChecksum));
        assert_eq!(parse_rsdp(&good[..19]), Err(AcpiError::Truncated));
        assert_eq!(parse_rsdp(&good[..30]), Err(AcpiError::Truncated));
        let mut rev1 = rsdp_v1(0x5000);
        rev1[15] = 1;
        fix_checksum(&mut rev1, 8, 20);
        assert_eq!(parse_rsdp(&rev1), Err(AcpiError::UnsupportedRevision));
        // Length field smaller than the structure, and larger than the buffer.
        let mut bad = good.clone();
        bad[20..24].copy_from_slice(&20u32.to_le_bytes());
        fix_checksum(&mut bad, 8, 20);
        assert_eq!(parse_rsdp(&bad), Err(AcpiError::BadLength));
        let mut bad = good.clone();
        bad[20..24].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        fix_checksum(&mut bad, 8, 20);
        assert_eq!(parse_rsdp(&bad), Err(AcpiError::BadLength));
    }

    #[test]
    fn rsdp_at_reads_through_the_reader_and_rejects_unreadable() {
        let mut m = FakeMem::default();
        m.put(0xE_0010, &rsdp_v2(0x5000, 0x6000));
        assert_eq!(parse_rsdp_at(&m, 0xE_0010).unwrap().xsdt, 0x6000);
        assert_eq!(
            parse_rsdp_at(&m, 0x9_0000),
            Err(AcpiError::Unreadable(0x9_0000))
        );
        // A v1 RSDP at the very end of the readable region still works.
        let mut m = FakeMem::default();
        m.put(0xF_FFEC, &rsdp_v1(0x5000));
        assert_eq!(parse_rsdp_at(&m, 0xF_FFEC).unwrap().revision, 0);
    }

    #[test]
    fn scan_finds_aligned_valid_rsdp_and_skips_corrupt_ones() {
        let mut m = FakeMem::default();
        let mut img = vec![0u8; 0x2_0000];
        // A corrupt candidate first (right signature, bad checksum), then a valid one.
        let mut corrupt = rsdp_v1(0x1111);
        corrupt[9] ^= 0xFF;
        img[0x100..0x100 + 20].copy_from_slice(&corrupt);
        let good = rsdp_v2(0x5000, 0x6000);
        img[0x2000..0x2000 + good.len()].copy_from_slice(&good);
        // An unaligned copy must not be found.
        img[0x3009..0x3009 + 20].copy_from_slice(&rsdp_v1(0x2222));
        m.put(0xE_0000, &img);
        let (addr, rsdp) = scan_for_rsdp(&m, 0xE_0000, 0x10_0000).unwrap();
        assert_eq!(addr, 0xE_2000);
        assert_eq!(rsdp.rsdt, 0x5000);
        // Only the unaligned one is present: nothing found.
        let mut m = FakeMem::default();
        let mut img = vec![0u8; 0x1000];
        img[9..29].copy_from_slice(&rsdp_v1(1));
        m.put(0xE_0000, &img);
        assert!(scan_for_rsdp(&m, 0xE_0000, 0xE_1000).is_none());
    }

    #[test]
    fn scan_is_bounded_and_tolerates_unreadable_memory() {
        let m = FakeMem::default(); // nothing readable
        assert!(scan_for_rsdp(&m, 0xE_0000, 0x10_0000).is_none());
        // 128 KiB / 16 candidates at most, regardless of the requested end.
        m.reads.set(0);
        assert!(scan_for_rsdp(&m, 0, u64::MAX).is_none());
        assert!(m.reads.get() <= 128 * 1024 / 16 + 1);
        // Overflowing start.
        assert!(scan_for_rsdp(&m, u64::MAX - 3, u64::MAX).is_none());
    }

    #[test]
    fn bios_search_prefers_the_ebda_then_the_bios_area() {
        let mut m = FakeMem::default();
        m.put(0x40E, &0x9FC0u16.to_le_bytes());
        let mut ebda = vec![0u8; 1024];
        ebda[0x40..0x40 + 20].copy_from_slice(&rsdp_v1(0xAAAA));
        m.put(0x9_FC00, &ebda);
        m.put(0xE_0000, &{
            let mut v = vec![0u8; 0x100];
            v[..20].copy_from_slice(&rsdp_v1(0xBBBB));
            v
        });
        let (addr, rsdp) = find_rsdp_bios(&m).unwrap();
        assert_eq!((addr, rsdp.rsdt), (0x9_FC40, 0xAAAA));
        // No EBDA pointer: falls back to the BIOS area.
        let mut m = FakeMem::default();
        m.put(0xE_0000, &{
            let mut v = vec![0u8; 0x100];
            v[0x10..0x10 + 20].copy_from_slice(&rsdp_v1(0xBBBB));
            v
        });
        assert_eq!(find_rsdp_bios(&m).unwrap().0, 0xE_0010);
        // A wild EBDA pointer is not scanned.
        let mut m = FakeMem::default();
        m.put(0x40E, &0xFFFFu16.to_le_bytes());
        m.reads.set(0);
        assert!(find_rsdp_bios(&m).is_none());
    }

    fn world(xsdt_addr: u64, rsdt_addr: u64) -> (FakeMem, Rsdp) {
        let mut m = FakeMem::default();
        m.put(0x7000, &sdt(b"APIC", 3, &[1, 2, 3, 4]));
        m.put(0x7100, &sdt(b"HPET", 1, &[5; 24]));
        m.put(0x6000, &root(true, &[0x7000, 0x7100]));
        m.put(0x5000, &root(false, &[0x7100, 0x7000]));
        (
            m,
            Rsdp {
                revision: 2,
                rsdt: rsdt_addr as u32,
                xsdt: xsdt_addr,
            },
        )
    }

    #[test]
    fn find_table_via_xsdt_and_rsdt() {
        let (m, rsdp) = world(0x6000, 0x5000);
        assert!(root_table(&m, &rsdp).unwrap().xsdt);
        let (addr, h) = find_table(&m, &rsdp, *b"HPET").unwrap();
        assert_eq!((addr, h.length), (0x7100, 36 + 24));
        assert_eq!(find_table(&m, &rsdp, *b"APIC").unwrap().0, 0x7000);
        assert_eq!(find_table(&m, &rsdp, *b"FACP"), Err(AcpiError::NotFound));
        // ACPI 1.0: RSDT only.
        let v1 = Rsdp {
            revision: 0,
            rsdt: 0x5000,
            xsdt: 0,
        };
        assert!(!root_table(&m, &v1).unwrap().xsdt);
        assert_eq!(find_table(&m, &v1, *b"APIC").unwrap().0, 0x7000);
    }

    #[test]
    fn xsdt_failure_falls_back_to_rsdt_and_reports_the_xsdt_error_if_both_fail() {
        // XSDT address is above the readable window: use the RSDT.
        let (m, rsdp) = world(0x1_0000_0000, 0x5000);
        assert!(!root_table(&m, &rsdp).unwrap().xsdt);
        assert_eq!(find_table(&m, &rsdp, *b"APIC").unwrap().0, 0x7000);
        // Both bad: the more specific (XSDT) error is reported.
        let (m, rsdp) = world(0x1_0000_0000, 0x4000);
        assert_eq!(
            root_table(&m, &rsdp),
            Err(AcpiError::Unreadable(0x1_0000_0000))
        );
        // Zero addresses.
        let rsdp = Rsdp {
            revision: 0,
            rsdt: 0,
            xsdt: 0,
        };
        assert_eq!(root_table(&m, &rsdp), Err(AcpiError::InvalidAddress));
    }

    #[test]
    fn corrupt_root_and_table_are_rejected() {
        let (mut m, rsdp) = world(0x6000, 0x5000);
        let mut bad = root(true, &[0x7000]);
        bad[40] ^= 1; // payload corrupted, checksum stale
        m.put(0x6000, &bad);
        assert!(
            !root_table(&m, &rsdp).unwrap().xsdt,
            "falls back; RSDT is intact"
        );
        // Wrong signature on the root.
        let (mut m, _) = world(0x6000, 0x5000);
        m.put(0x6000, &sdt(b"NOPE", 1, &[]));
        let only_x = Rsdp {
            revision: 2,
            rsdt: 0,
            xsdt: 0x6000,
        };
        assert_eq!(root_table(&m, &only_x), Err(AcpiError::BadSignature));
        // A listed table with a bad checksum is an error, not silently skipped.
        let (mut m, rsdp) = world(0x6000, 0x5000);
        let mut t = sdt(b"APIC", 3, &[1, 2, 3, 4]);
        t[36] ^= 0xFF;
        m.put(0x7000, &t);
        assert_eq!(find_table(&m, &rsdp, *b"APIC"), Err(AcpiError::BadChecksum));
    }

    #[test]
    fn table_length_bounds() {
        let mut m = FakeMem::default();
        let mut t = sdt(b"TEST", 1, &[]);
        t[4..8].copy_from_slice(&35u32.to_le_bytes());
        m.put(0x1000, &t);
        assert_eq!(validate_table(&m, 0x1000), Err(AcpiError::BadLength));
        t[4..8].copy_from_slice(&(MAX_TABLE_LEN + 1).to_le_bytes());
        m.put(0x1000, &t);
        assert_eq!(validate_table(&m, 0x1000), Err(AcpiError::TooLarge));
        t[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        m.put(0x1000, &t);
        assert_eq!(validate_table(&m, 0x1000), Err(AcpiError::TooLarge));
        // Declared longer than the readable memory: reported, not read past.
        t[4..8].copy_from_slice(&4096u32.to_le_bytes());
        m.put(0x1000, &t);
        assert!(matches!(
            validate_table(&m, 0x1000),
            Err(AcpiError::Unreadable(_))
        ));
    }

    #[test]
    fn unreadable_and_zero_entries_are_skipped_but_reported() {
        let mut m = FakeMem::default();
        m.put(0x6000, &root(true, &[0, 0x9_9999_0000, 0x7000]));
        m.put(0x7000, &sdt(b"APIC", 1, &[]));
        let rsdp = Rsdp {
            revision: 2,
            rsdt: 0,
            xsdt: 0x6000,
        };
        assert_eq!(find_table(&m, &rsdp, *b"APIC").unwrap().0, 0x7000);
        // Only unreadable candidates exist: the first unreadable address is reported.
        let mut m = FakeMem::default();
        m.put(0x6000, &root(true, &[0x9_9999_0000]));
        assert_eq!(
            find_table(&m, &rsdp, *b"APIC"),
            Err(AcpiError::Unreadable(0x9_9999_0000))
        );
    }

    #[test]
    fn identity_reader_enforces_its_limit_and_overflow() {
        // Safe to construct and probe with out-of-range requests: they are rejected before any
        // memory is touched.
        let r = unsafe { IdentityReader::new(0x1000) };
        let mut b = [0u8; 4];
        assert_eq!(r.read(0x1000, &mut b), Err(ReadError));
        assert_eq!(r.read(0xFFD, &mut b), Err(ReadError));
        assert_eq!(r.read(u64::MAX - 1, &mut b), Err(ReadError));
        assert_eq!(
            r.read(0xFEE0_0000, &mut b),
            Err(ReadError),
            "LAPIC MMIO is outside the window"
        );
        let mut empty: [u8; 0] = [];
        assert!(
            r.read(0x1000, &mut empty).is_ok(),
            "zero-length read at the limit touches nothing"
        );
    }

    #[test]
    fn find_table_bios_end_to_end() {
        let (mut m, _) = world(0x6000, 0x5000);
        m.put(0xE_0000, &{
            let mut v = vec![0u8; 64];
            v[..36].copy_from_slice(&rsdp_v2(0x5000, 0x6000));
            v
        });
        assert_eq!(find_table_bios(&m, *b"APIC").unwrap().0, 0x7000);
        let empty = FakeMem::default();
        assert_eq!(find_table_bios(&empty, *b"APIC"), Err(AcpiError::NotFound));
    }
}
