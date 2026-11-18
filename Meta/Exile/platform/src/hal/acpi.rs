//! ACPI table validation and parsing (RSDP, RSDT/XSDT, MADT, MCFG).
//!
//! Status: PARSER ONLY. Operates on byte slices the caller has already mapped;
//! it never dereferences physical addresses itself. Verified against
//! hand-built fixtures, NOT against real firmware tables.

/// Errors from ACPI parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiError {
    /// Buffer shorter than the structure requires.
    Truncated,
    /// Signature mismatch.
    BadSignature,
    /// Byte-sum checksum is non-zero.
    BadChecksum,
    /// Length field inconsistent with the buffer.
    BadLength,
    /// Output capacity exceeded.
    Full,
}

/// Sum of all bytes (wrapping). Valid ACPI structures sum to zero.
pub fn checksum(b: &[u8]) -> u8 {
    b.iter().fold(0u8, |a, x| a.wrapping_add(*x))
}

fn rd32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Parsed RSDP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsdp {
    /// ACPI revision (0 = RSDT only, >=2 = XSDT present).
    pub revision: u8,
    /// Physical address of the RSDT.
    pub rsdt: u32,
    /// Physical address of the XSDT (0 if revision < 2).
    pub xsdt: u64,
}

/// Validate and parse an RSDP (both the 20-byte v1 and 36-byte v2 checksums).
pub fn parse_rsdp(b: &[u8]) -> Result<Rsdp, AcpiError> {
    if b.len() < 20 {
        return Err(AcpiError::Truncated);
    }
    if &b[0..8] != b"RSD PTR " {
        return Err(AcpiError::BadSignature);
    }
    if checksum(&b[0..20]) != 0 {
        return Err(AcpiError::BadChecksum);
    }
    let revision = b[15];
    let rsdt = rd32(b, 16).ok_or(AcpiError::Truncated)?;
    if revision < 2 {
        return Ok(Rsdp {
            revision,
            rsdt,
            xsdt: 0,
        });
    }
    if b.len() < 36 {
        return Err(AcpiError::Truncated);
    }
    let len = rd32(b, 20).ok_or(AcpiError::Truncated)? as usize;
    if len < 36 || len > b.len() {
        return Err(AcpiError::BadLength);
    }
    if checksum(&b[0..len]) != 0 {
        return Err(AcpiError::BadChecksum);
    }
    Ok(Rsdp {
        revision,
        rsdt,
        xsdt: rd64(b, 24).ok_or(AcpiError::Truncated)?,
    })
}

/// Validate a system description table; returns (signature, total length).
pub fn parse_sdt_header(b: &[u8]) -> Result<([u8; 4], usize), AcpiError> {
    if b.len() < 36 {
        return Err(AcpiError::Truncated);
    }
    let len = rd32(b, 4).ok_or(AcpiError::Truncated)? as usize;
    if len < 36 || len > b.len() {
        return Err(AcpiError::BadLength);
    }
    if checksum(&b[..len]) != 0 {
        return Err(AcpiError::BadChecksum);
    }
    Ok(([b[0], b[1], b[2], b[3]], len))
}

/// Read the table pointers from a validated RSDT (`wide=false`, 4-byte) or XSDT (`wide=true`).
pub fn sdt_entries<const N: usize>(b: &[u8], wide: bool) -> Result<([u64; N], usize), AcpiError> {
    let (sig, len) = parse_sdt_header(b)?;
    let want: &[u8; 4] = if wide { b"XSDT" } else { b"RSDT" };
    if &sig != want {
        return Err(AcpiError::BadSignature);
    }
    let w = if wide { 8 } else { 4 };
    let n = (len - 36) / w;
    if n > N {
        return Err(AcpiError::Full);
    }
    let mut out = [0u64; N];
    for (i, slot) in out.iter_mut().enumerate().take(n) {
        let o = 36 + i * w;
        *slot = if wide {
            rd64(b, o)
        } else {
            rd32(b, o).map(u64::from)
        }
        .ok_or(AcpiError::Truncated)?;
    }
    Ok((out, n))
}

/// One enabled processor from the MADT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MadtCpu {
    /// ACPI processor id.
    pub acpi_id: u32,
    /// Local APIC id.
    pub apic_id: u32,
}

/// Summary of a MADT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Madt<const N: usize> {
    /// Local APIC base physical address (low 32-bit field; override entry applied).
    pub lapic_base: u64,
    /// Processors that are enabled (or online-capable).
    pub cpus: [Option<MadtCpu>; N],
    /// Count of enabled processors recorded.
    pub cpu_count: usize,
    /// First I/O APIC (id, address, gsi_base), if any.
    pub ioapic: Option<(u8, u32, u32)>,
}

/// Parse a MADT. Disabled processors are skipped. Entries with length < 2 are rejected
/// (they would loop forever otherwise).
pub fn parse_madt<const N: usize>(b: &[u8]) -> Result<Madt<N>, AcpiError> {
    let (sig, len) = parse_sdt_header(b)?;
    if &sig != b"APIC" {
        return Err(AcpiError::BadSignature);
    }
    if len < 44 {
        return Err(AcpiError::Truncated);
    }
    let mut m = Madt {
        lapic_base: u64::from(rd32(b, 36).ok_or(AcpiError::Truncated)?),
        cpus: [None; N],
        cpu_count: 0,
        ioapic: None,
    };
    let mut o = 44;
    while o < len {
        if o + 2 > len {
            return Err(AcpiError::BadLength);
        }
        let (ty, l) = (b[o], b[o + 1] as usize);
        if l < 2 || o + l > len {
            return Err(AcpiError::BadLength);
        }
        let e = &b[o..o + l];
        match ty {
            0 if l >= 8 => {
                let flags = rd32(e, 4).ok_or(AcpiError::Truncated)?;
                if flags & 0b11 != 0 {
                    if m.cpu_count >= N {
                        return Err(AcpiError::Full);
                    }
                    m.cpus[m.cpu_count] = Some(MadtCpu {
                        acpi_id: u32::from(e[2]),
                        apic_id: u32::from(e[3]),
                    });
                    m.cpu_count += 1;
                }
            }
            1 if l >= 12 && m.ioapic.is_none() => {
                m.ioapic = Some((
                    e[2],
                    rd32(e, 4).ok_or(AcpiError::Truncated)?,
                    rd32(e, 8).ok_or(AcpiError::Truncated)?,
                ));
            }
            5 if l >= 12 => m.lapic_base = rd64(e, 4).ok_or(AcpiError::Truncated)?,
            9 if l >= 16 => {
                let flags = rd32(e, 8).ok_or(AcpiError::Truncated)?;
                if flags & 0b11 != 0 {
                    if m.cpu_count >= N {
                        return Err(AcpiError::Full);
                    }
                    m.cpus[m.cpu_count] = Some(MadtCpu {
                        acpi_id: rd32(e, 12).ok_or(AcpiError::Truncated)?,
                        apic_id: rd32(e, 4).ok_or(AcpiError::Truncated)?,
                    });
                    m.cpu_count += 1;
                }
            }
            _ => {}
        }
        o += l;
    }
    Ok(m)
}

/// A PCIe ECAM window from the MCFG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EcamRegion {
    /// Base physical address.
    pub base: u64,
    /// PCI segment group.
    pub segment: u16,
    /// First bus covered.
    pub start_bus: u8,
    /// Last bus covered.
    pub end_bus: u8,
}

/// Parse an MCFG into at most `N` windows.
pub fn parse_mcfg<const N: usize>(b: &[u8]) -> Result<([Option<EcamRegion>; N], usize), AcpiError> {
    let (sig, len) = parse_sdt_header(b)?;
    if &sig != b"MCFG" {
        return Err(AcpiError::BadSignature);
    }
    let mut out = [None; N];
    let mut n = 0;
    let mut o = 44;
    while o + 16 <= len {
        if n >= N {
            return Err(AcpiError::Full);
        }
        let seg = u16::from_le_bytes([b[o + 8], b[o + 9]]);
        out[n] = Some(EcamRegion {
            base: rd64(b, o).ok_or(AcpiError::Truncated)?,
            segment: seg,
            start_bus: b[o + 10],
            end_bus: b[o + 11],
        });
        n += 1;
        o += 16;
    }
    Ok((out, n))
}

impl EcamRegion {
    /// Physical address of a function's config space, or None if outside this window.
    pub fn config_addr(&self, bus: u8, dev: u8, func: u8) -> Option<u64> {
        if bus < self.start_bus || bus > self.end_bus || dev > 31 || func > 7 {
            return None;
        }
        Some(
            self.base
                + (u64::from(bus - self.start_bus) << 20
                    | u64::from(dev) << 15
                    | u64::from(func) << 12),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn fix(mut v: Vec<u8>, sum_at: usize) -> Vec<u8> {
        v[sum_at] = 0;
        let c = checksum(&v);
        v[sum_at] = 0u8.wrapping_sub(c);
        v
    }
    fn sdt(sig: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(sig);
        v.extend_from_slice(&((36 + body.len()) as u32).to_le_bytes());
        v.extend_from_slice(&[1, 0]);
        v.extend_from_slice(&[0u8; 26]);
        v.extend_from_slice(body);
        fix(v, 9)
    }
    fn rsdp2(xsdt: u64) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RSD PTR ");
        v.extend_from_slice(&[0, b'E', b'X', b'I', b'L', b'E', 0, 2]);
        v.extend_from_slice(&0x1000u32.to_le_bytes());
        v.extend_from_slice(&36u32.to_le_bytes());
        v.extend_from_slice(&xsdt.to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0]);
        let mut a = v.clone();
        a[8] = 0;
        a[8] = 0u8.wrapping_sub(checksum(&a[0..20]));
        a[32] = 0;
        a[32] = 0u8.wrapping_sub(checksum(&a));
        a
    }

    #[test]
    fn rsdp_v2_valid() {
        let r = parse_rsdp(&rsdp2(0xDEAD_B000)).unwrap();
        assert_eq!((r.revision, r.rsdt, r.xsdt), (2, 0x1000, 0xDEAD_B000));
    }
    #[test]
    fn rsdp_rejects_corruption() {
        let mut a = rsdp2(1);
        a[24] ^= 1; // xsdt address flips -> extended checksum fails, base ok
        assert_eq!(parse_rsdp(&a), Err(AcpiError::BadChecksum));
        let mut b = rsdp2(1);
        b[0] = b'X';
        assert_eq!(parse_rsdp(&b), Err(AcpiError::BadSignature));
        assert_eq!(parse_rsdp(&b[..10]), Err(AcpiError::Truncated));
        let mut c = rsdp2(1);
        c[20] = 255; // length beyond buffer
        assert!(parse_rsdp(&c).is_err());
    }
    #[test]
    fn xsdt_and_rsdt_entries() {
        let mut body = Vec::new();
        body.extend_from_slice(&0x1111u64.to_le_bytes());
        body.extend_from_slice(&0x2222u64.to_le_bytes());
        let t = sdt(b"XSDT", &body);
        let (e, n) = sdt_entries::<4>(&t, true).unwrap();
        assert_eq!((n, e[0], e[1]), (2, 0x1111, 0x2222));
        assert_eq!(sdt_entries::<4>(&t, false), Err(AcpiError::BadSignature));
        assert_eq!(sdt_entries::<1>(&t, true), Err(AcpiError::Full));
        let r = sdt(b"RSDT", &0x3333u32.to_le_bytes());
        assert_eq!(sdt_entries::<2>(&r, false).unwrap().0[0], 0x3333);
    }
    #[test]
    fn sdt_checksum_and_length_enforced() {
        let mut t = sdt(b"APIC", &[0u8; 8]);
        t[40] ^= 0xFF;
        assert_eq!(parse_sdt_header(&t), Err(AcpiError::BadChecksum));
        let mut t = sdt(b"APIC", &[0u8; 8]);
        t[4] = 200;
        assert_eq!(parse_sdt_header(&t), Err(AcpiError::BadLength));
    }
    fn madt_body(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        for e in entries {
            b.extend_from_slice(e);
        }
        b
    }
    #[test]
    fn madt_counts_only_enabled_cpus() {
        let lapic = |id, apic, fl: u32| {
            let mut e = vec![0u8, 8, id, apic];
            e.extend_from_slice(&fl.to_le_bytes());
            e
        };
        let mut io = vec![1u8, 12, 7, 0];
        io.extend_from_slice(&0xFEC0_0000u32.to_le_bytes());
        io.extend_from_slice(&0u32.to_le_bytes());
        let t = sdt(
            b"APIC",
            &madt_body(&[lapic(0, 0, 1), lapic(1, 2, 0), lapic(2, 4, 2), io]),
        );
        let m = parse_madt::<8>(&t).unwrap();
        assert_eq!(m.cpu_count, 2);
        assert_eq!(
            m.cpus[1],
            Some(MadtCpu {
                acpi_id: 2,
                apic_id: 4
            })
        );
        assert_eq!(m.ioapic, Some((7, 0xFEC0_0000, 0)));
        assert_eq!(m.lapic_base, 0xFEE0_0000);
    }
    #[test]
    fn madt_override_and_capacity_and_zero_length_entry() {
        let mut ov = vec![5u8, 12, 0, 0];
        ov.extend_from_slice(&0x1_FEE0_0000u64.to_le_bytes());
        let t = sdt(b"APIC", &madt_body(&[ov]));
        assert_eq!(parse_madt::<2>(&t).unwrap().lapic_base, 0x1_FEE0_0000);
        let mk = |a| {
            let mut e = vec![0u8, 8, a, a];
            e.extend_from_slice(&1u32.to_le_bytes());
            e
        };
        let t = sdt(b"APIC", &madt_body(&[mk(0), mk(1), mk(2)]));
        assert_eq!(parse_madt::<2>(&t), Err(AcpiError::Full));
        let t = sdt(b"APIC", &madt_body(&[vec![0u8, 0]]));
        assert_eq!(parse_madt::<2>(&t), Err(AcpiError::BadLength));
    }
    #[test]
    fn mcfg_and_ecam_addressing() {
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&0xE000_0000u64.to_le_bytes());
        body.extend_from_slice(&[0, 0, 0, 0x3F, 0, 0, 0, 0]);
        let t = sdt(b"MCFG", &body);
        let (r, n) = parse_mcfg::<2>(&t).unwrap();
        assert_eq!(n, 1);
        let w = r[0].unwrap();
        assert_eq!((w.start_bus, w.end_bus), (0, 0x3F));
        assert_eq!(
            w.config_addr(1, 2, 3),
            Some(0xE000_0000 + (1 << 20) + (2 << 15) + (3 << 12))
        );
        assert_eq!(w.config_addr(0x40, 0, 0), None);
        assert_eq!(w.config_addr(0, 32, 0), None);
    }
}
