//! ACPI MADT ("APIC" table) parsing (Mission C3.3). SPDX-License-Identifier: Apache-2.0
//!
//! The table is validated (header, length bounds, checksum over the whole table) and then
//! walked entry by entry through a bounded reader. Each entry is length-checked before its body
//! is read: a length below 2, an entry past the end of the table, or a known entry with the
//! wrong size is `Malformed`/`Truncated`, never skipped over or trusted. Unknown entry types are
//! skipped by their stated length and counted. Results go into fixed-size arrays; running out of
//! room is `TooMany`, not silent truncation.
//!
//! Decoded here: local APIC address (with the 64-bit override), processor local APICs and
//! x2APICs, IOAPICs, interrupt source overrides with polarity/trigger decoding, local APIC NMI
//! entries. The caller decides what to do with them; nothing here touches hardware.
#![allow(dead_code)]

use super::{AcpiError, PhysReader, SDT_HEADER_LEN, find_table, le16, le32, le64, validate_table};
use super::{Rsdp, find_table_bios};

pub const MAX_CPUS: usize = 64;
pub const MAX_IOAPICS: usize = 8;
pub const MAX_OVERRIDES: usize = 16;
pub const MAX_NMIS: usize = 16;
pub const MADT_SIGNATURE: [u8; 4] = *b"APIC";
const FIXED_LEN: usize = SDT_HEADER_LEN + 8;

/// MADT flag: the system also has dual 8259 PICs.
pub const FLAG_PCAT_COMPAT: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    ActiveHigh,
    ActiveLow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Edge,
    Level,
}

/// Decodes the 2-bit MPS INTI polarity field. `Ok(None)` = "conforms to the bus default".
pub fn decode_polarity(bits: u16) -> Result<Option<Polarity>, AcpiError> {
    match bits & 3 {
        0 => Ok(None),
        1 => Ok(Some(Polarity::ActiveHigh)),
        3 => Ok(Some(Polarity::ActiveLow)),
        _ => Err(AcpiError::Malformed), // 0b10 is reserved
    }
}

pub fn decode_trigger(bits: u16) -> Result<Option<Trigger>, AcpiError> {
    match bits & 3 {
        0 => Ok(None),
        1 => Ok(Some(Trigger::Edge)),
        3 => Ok(Some(Trigger::Level)),
        _ => Err(AcpiError::Malformed),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuEntry {
    pub acpi_id: u32,
    pub apic_id: u32,
    pub enabled: bool,
    pub online_capable: bool,
    /// Came from a Processor Local x2APIC entry.
    pub x2apic: bool,
}

impl CpuEntry {
    pub const EMPTY: Self = Self {
        acpi_id: 0,
        apic_id: 0,
        enabled: false,
        online_capable: false,
        x2apic: false,
    };
    /// Enabled now, or could be brought online later.
    pub fn usable(&self) -> bool {
        self.enabled || self.online_capable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoApicEntry {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

impl IoApicEntry {
    pub const EMPTY: Self = Self {
        id: 0,
        address: 0,
        gsi_base: 0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceOverride {
    pub bus: u8,
    /// ISA IRQ number.
    pub source: u8,
    pub gsi: u32,
    /// `None` = conforms to the ISA default (active high).
    pub polarity: Option<Polarity>,
    /// `None` = conforms to the ISA default (edge).
    pub trigger: Option<Trigger>,
}

impl SourceOverride {
    pub const EMPTY: Self = Self {
        bus: 0,
        source: 0,
        gsi: 0,
        polarity: None,
        trigger: None,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LapicNmi {
    /// 0xFFFF_FFFF: all processors.
    pub acpi_id: u32,
    pub lint: u8,
    pub polarity: Option<Polarity>,
    pub trigger: Option<Trigger>,
}

impl LapicNmi {
    pub const EMPTY: Self = Self {
        acpi_id: 0,
        lint: 0,
        polarity: None,
        trigger: None,
    };
}

/// A fully resolved legacy-IRQ route (no "bus default" left).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsaRoute {
    pub gsi: u32,
    pub polarity: Polarity,
    pub trigger: Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Madt {
    /// Physical address of the local APIC registers (after any override).
    pub lapic_address: u64,
    pub flags: u32,
    pub lapic_address_overridden: bool,
    pub cpus: [CpuEntry; MAX_CPUS],
    pub cpu_count: usize,
    pub ioapics: [IoApicEntry; MAX_IOAPICS],
    pub ioapic_count: usize,
    pub overrides: [SourceOverride; MAX_OVERRIDES],
    pub override_count: usize,
    pub nmis: [LapicNmi; MAX_NMIS],
    pub nmi_count: usize,
    /// Entries of types this parser does not decode (skipped by length).
    pub unsupported_entries: u32,
}

impl Madt {
    const fn empty() -> Self {
        Self {
            lapic_address: 0,
            flags: 0,
            lapic_address_overridden: false,
            cpus: [CpuEntry::EMPTY; MAX_CPUS],
            cpu_count: 0,
            ioapics: [IoApicEntry::EMPTY; MAX_IOAPICS],
            ioapic_count: 0,
            overrides: [SourceOverride::EMPTY; MAX_OVERRIDES],
            override_count: 0,
            nmis: [LapicNmi::EMPTY; MAX_NMIS],
            nmi_count: 0,
            unsupported_entries: 0,
        }
    }

    pub fn cpus(&self) -> &[CpuEntry] {
        &self.cpus[..self.cpu_count]
    }
    pub fn ioapics(&self) -> &[IoApicEntry] {
        &self.ioapics[..self.ioapic_count]
    }
    pub fn overrides(&self) -> &[SourceOverride] {
        &self.overrides[..self.override_count]
    }
    pub fn nmis(&self) -> &[LapicNmi] {
        &self.nmis[..self.nmi_count]
    }

    pub fn pcat_compat(&self) -> bool {
        self.flags & FLAG_PCAT_COMPAT != 0
    }

    pub fn usable_cpu_count(&self) -> usize {
        self.cpus().iter().filter(|c| c.usable()).count()
    }

    /// Resolves legacy ISA IRQ `irq` to its global system interrupt, polarity and trigger:
    /// identity-mapped, active high, edge unless an override says otherwise.
    pub fn isa_route(&self, irq: u8) -> IsaRoute {
        match self
            .overrides()
            .iter()
            .find(|o| o.bus == 0 && o.source == irq)
        {
            Some(o) => IsaRoute {
                gsi: o.gsi,
                polarity: o.polarity.unwrap_or(Polarity::ActiveHigh),
                trigger: o.trigger.unwrap_or(Trigger::Edge),
            },
            None => IsaRoute {
                gsi: irq as u32,
                polarity: Polarity::ActiveHigh,
                trigger: Trigger::Edge,
            },
        }
    }

    /// The IOAPIC entry whose base is the highest one not above `gsi`. The caller must still
    /// check `gsi` against that IOAPIC's real redirection-entry count (known only from
    /// hardware).
    pub fn ioapic_candidate(&self, gsi: u32) -> Option<&IoApicEntry> {
        self.ioapics()
            .iter()
            .filter(|io| io.gsi_base <= gsi)
            .max_by_key(|io| io.gsi_base)
    }
}

/// Finds and parses the MADT via an already validated RSDP.
pub fn discover<R: PhysReader + ?Sized>(r: &R, rsdp: &Rsdp) -> Result<Madt, AcpiError> {
    let (phys, _) = find_table(r, rsdp, MADT_SIGNATURE)?;
    parse_madt(r, phys)
}

/// Legacy BIOS RSDP search, then `discover`.
pub fn discover_bios<R: PhysReader + ?Sized>(r: &R) -> Result<Madt, AcpiError> {
    let (phys, _) = find_table_bios(r, MADT_SIGNATURE)?;
    parse_madt(r, phys)
}

/// Parses the MADT at `phys`.
pub fn parse_madt<R: PhysReader + ?Sized>(r: &R, phys: u64) -> Result<Madt, AcpiError> {
    let header = validate_table(r, phys)?;
    if header.signature != MADT_SIGNATURE {
        return Err(AcpiError::BadSignature);
    }
    let total = header.length as usize;
    if total < FIXED_LEN {
        return Err(AcpiError::BadLength);
    }
    let mut fixed = [0u8; 8];
    r.read(phys + SDT_HEADER_LEN as u64, &mut fixed)
        .map_err(|_| AcpiError::Unreadable(phys + SDT_HEADER_LEN as u64))?;

    let mut m = Madt::empty();
    m.lapic_address = le32(&fixed, 0) as u64;
    m.flags = le32(&fixed, 4);

    let mut off = FIXED_LEN;
    let mut buf = [0u8; 255];
    while off < total {
        if total - off < 2 {
            return Err(AcpiError::Truncated);
        }
        let mut head = [0u8; 2];
        r.read(phys + off as u64, &mut head)
            .map_err(|_| AcpiError::Unreadable(phys + off as u64))?;
        let (kind, len) = (head[0], head[1] as usize);
        if len < 2 {
            return Err(AcpiError::Malformed); // would never advance
        }
        if len > total - off {
            return Err(AcpiError::Truncated);
        }
        let body = &mut buf[..len];
        r.read(phys + off as u64, body)
            .map_err(|_| AcpiError::Unreadable(phys + off as u64))?;
        let body = &buf[..len];
        entry(&mut m, kind, body)?;
        off += len;
    }

    if m.lapic_address == 0 || m.lapic_address & 0xFFF != 0 {
        return Err(AcpiError::InvalidAddress);
    }
    Ok(m)
}

fn need(body: &[u8], len: usize) -> Result<(), AcpiError> {
    if body.len() == len {
        Ok(())
    } else {
        Err(AcpiError::Malformed)
    }
}

fn entry(m: &mut Madt, kind: u8, b: &[u8]) -> Result<(), AcpiError> {
    match kind {
        // Processor Local APIC
        0 => {
            need(b, 8)?;
            let flags = le32(b, 4);
            push_cpu(
                m,
                CpuEntry {
                    acpi_id: b[2] as u32,
                    apic_id: b[3] as u32,
                    enabled: flags & 1 != 0,
                    online_capable: flags & 2 != 0,
                    x2apic: false,
                },
            )
        }
        // I/O APIC
        1 => {
            need(b, 12)?;
            let e = IoApicEntry {
                id: b[2],
                address: le32(b, 4),
                gsi_base: le32(b, 8),
            };
            if e.address == 0 || e.address & 3 != 0 {
                return Err(AcpiError::InvalidAddress);
            }
            if m.ioapics()
                .iter()
                .any(|o| o.id == e.id || o.address == e.address)
            {
                return Err(AcpiError::Malformed);
            }
            if m.ioapic_count == MAX_IOAPICS {
                return Err(AcpiError::TooMany);
            }
            m.ioapics[m.ioapic_count] = e;
            m.ioapic_count += 1;
            Ok(())
        }
        // Interrupt Source Override
        2 => {
            need(b, 10)?;
            let flags = le16(b, 8);
            let o = SourceOverride {
                bus: b[2],
                source: b[3],
                gsi: le32(b, 4),
                polarity: decode_polarity(flags)?,
                trigger: decode_trigger(flags >> 2)?,
            };
            if o.bus != 0 {
                // Only the ISA bus (0) is defined by ACPI.
                m.unsupported_entries += 1;
                return Ok(());
            }
            if m.overrides().iter().any(|x| x.source == o.source) {
                return Err(AcpiError::Malformed);
            }
            if m.override_count == MAX_OVERRIDES {
                return Err(AcpiError::TooMany);
            }
            m.overrides[m.override_count] = o;
            m.override_count += 1;
            Ok(())
        }
        // Local APIC NMI
        4 => {
            need(b, 6)?;
            let flags = le16(b, 3);
            let lint = b[5];
            if lint > 1 {
                return Err(AcpiError::Malformed);
            }
            let n = LapicNmi {
                acpi_id: if b[2] == 0xFF {
                    0xFFFF_FFFF
                } else {
                    b[2] as u32
                },
                lint,
                polarity: decode_polarity(flags)?,
                trigger: decode_trigger(flags >> 2)?,
            };
            if m.nmi_count == MAX_NMIS {
                return Err(AcpiError::TooMany);
            }
            m.nmis[m.nmi_count] = n;
            m.nmi_count += 1;
            Ok(())
        }
        // Local APIC Address Override
        5 => {
            need(b, 12)?;
            if m.lapic_address_overridden {
                return Err(AcpiError::Malformed);
            }
            let addr = le64(b, 4);
            if addr == 0 || addr & 0xFFF != 0 {
                return Err(AcpiError::InvalidAddress);
            }
            m.lapic_address = addr;
            m.lapic_address_overridden = true;
            Ok(())
        }
        // Processor Local x2APIC
        9 => {
            need(b, 16)?;
            let flags = le32(b, 8);
            push_cpu(
                m,
                CpuEntry {
                    acpi_id: le32(b, 12),
                    apic_id: le32(b, 4),
                    enabled: flags & 1 != 0,
                    online_capable: flags & 2 != 0,
                    x2apic: true,
                },
            )
        }
        // NMI source, IOSAPIC, local SAPIC, platform interrupt sources, x2APIC NMI, GIC, ...
        // and OEM entries: not decoded.
        _ => {
            m.unsupported_entries += 1;
            Ok(())
        }
    }
}

fn push_cpu(m: &mut Madt, c: CpuEntry) -> Result<(), AcpiError> {
    if m.cpus().iter().any(|o| o.apic_id == c.apic_id) {
        return Err(AcpiError::Malformed);
    }
    if m.cpu_count == MAX_CPUS {
        return Err(AcpiError::TooMany);
    }
    m.cpus[m.cpu_count] = c;
    m.cpu_count += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    fn lapic(acpi: u8, apic: u8, flags: u32) -> Vec<u8> {
        let mut v = vec![0, 8, acpi, apic];
        v.extend_from_slice(&flags.to_le_bytes());
        v
    }
    fn ioapic(id: u8, addr: u32, base: u32) -> Vec<u8> {
        let mut v = vec![1, 12, id, 0];
        v.extend_from_slice(&addr.to_le_bytes());
        v.extend_from_slice(&base.to_le_bytes());
        v
    }
    fn iso(bus: u8, src: u8, gsi: u32, flags: u16) -> Vec<u8> {
        let mut v = vec![2, 10, bus, src];
        v.extend_from_slice(&gsi.to_le_bytes());
        v.extend_from_slice(&flags.to_le_bytes());
        v
    }
    fn nmi(acpi: u8, flags: u16, lint: u8) -> Vec<u8> {
        let mut v = vec![4, 6, acpi];
        v.extend_from_slice(&flags.to_le_bytes());
        v.push(lint);
        v
    }
    fn lapic_override(addr: u64) -> Vec<u8> {
        let mut v = vec![5, 12, 0, 0];
        v.extend_from_slice(&addr.to_le_bytes());
        v
    }
    fn x2apic(id: u32, flags: u32, uid: u32) -> Vec<u8> {
        let mut v = vec![9, 16, 0, 0];
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&flags.to_le_bytes());
        v.extend_from_slice(&uid.to_le_bytes());
        v
    }

    fn madt_bytes(lapic_addr: u32, flags: u32, entries: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&lapic_addr.to_le_bytes());
        body.extend_from_slice(&flags.to_le_bytes());
        for e in entries {
            body.extend_from_slice(e);
        }
        sdt(b"APIC", 5, &body)
    }

    fn parse(entries: &[Vec<u8>]) -> Result<Madt, AcpiError> {
        let mut m = FakeMem::default();
        m.put(0x4000, &madt_bytes(0xFEE0_0000, 1, entries));
        parse_madt(&m, 0x4000)
    }

    #[test]
    fn typical_qemu_like_table() {
        let t = parse(&[
            lapic(0, 0, 1),
            lapic(1, 1, 1),
            ioapic(0, 0xFEC0_0000, 0),
            iso(0, 0, 2, 0),      // timer: ISA IRQ0 -> GSI 2, bus defaults
            iso(0, 5, 5, 0b1101), // IRQ5: active high, level
            iso(0, 9, 9, 0b1111), // IRQ9: active low, level (ACPI SCI)
            nmi(0xFF, 0, 1),
        ])
        .unwrap();
        assert_eq!(t.lapic_address, 0xFEE0_0000);
        assert!(t.pcat_compat());
        assert_eq!(t.cpu_count, 2);
        assert_eq!(
            t.cpus()[1],
            CpuEntry {
                acpi_id: 1,
                apic_id: 1,
                enabled: true,
                online_capable: false,
                x2apic: false
            }
        );
        assert_eq!(
            t.ioapics(),
            &[IoApicEntry {
                id: 0,
                address: 0xFEC0_0000,
                gsi_base: 0
            }]
        );
        assert_eq!(t.override_count, 3);
        assert_eq!(t.nmis()[0].acpi_id, 0xFFFF_FFFF);
        assert_eq!(t.nmis()[0].lint, 1);
        assert_eq!(t.usable_cpu_count(), 2);
        assert_eq!(t.unsupported_entries, 0);
    }

    #[test]
    fn isa_routing_defaults_and_overrides() {
        let t = parse(&[
            lapic(0, 0, 1),
            ioapic(0, 0xFEC0_0000, 0),
            iso(0, 0, 2, 0),
            iso(0, 9, 9, 0b1111),
        ])
        .unwrap();
        // No override: identity, active high, edge.
        assert_eq!(
            t.isa_route(4),
            IsaRoute {
                gsi: 4,
                polarity: Polarity::ActiveHigh,
                trigger: Trigger::Edge
            }
        );
        // Override with bus defaults resolves to the ISA defaults.
        assert_eq!(
            t.isa_route(0),
            IsaRoute {
                gsi: 2,
                polarity: Polarity::ActiveHigh,
                trigger: Trigger::Edge
            }
        );
        // Explicit polarity/trigger.
        assert_eq!(
            t.isa_route(9),
            IsaRoute {
                gsi: 9,
                polarity: Polarity::ActiveLow,
                trigger: Trigger::Level
            }
        );
    }

    #[test]
    fn polarity_and_trigger_decoding_covers_every_encoding() {
        assert_eq!(decode_polarity(0), Ok(None));
        assert_eq!(decode_polarity(1), Ok(Some(Polarity::ActiveHigh)));
        assert_eq!(decode_polarity(3), Ok(Some(Polarity::ActiveLow)));
        assert_eq!(decode_polarity(2), Err(AcpiError::Malformed));
        assert_eq!(
            decode_polarity(0xFFF1),
            Ok(Some(Polarity::ActiveHigh)),
            "upper bits are other fields"
        );
        assert_eq!(decode_trigger(0), Ok(None));
        assert_eq!(decode_trigger(1), Ok(Some(Trigger::Edge)));
        assert_eq!(decode_trigger(3), Ok(Some(Trigger::Level)));
        assert_eq!(decode_trigger(2), Err(AcpiError::Malformed));
        // Through the table: reserved encodings reject the whole table.
        assert_eq!(
            parse(&[iso(0, 1, 1, 0b0010)]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[iso(0, 1, 1, 0b1000)]).err(),
            Some(AcpiError::Malformed)
        );
    }

    #[test]
    fn lapic_address_override_wins_and_is_validated() {
        let t = parse(&[lapic_override(0x1_FEE0_0000)]).unwrap();
        assert_eq!(t.lapic_address, 0x1_FEE0_0000);
        assert!(t.lapic_address_overridden);
        assert_eq!(
            parse(&[lapic_override(0)]).err(),
            Some(AcpiError::InvalidAddress)
        );
        assert_eq!(
            parse(&[lapic_override(0xFEE0_0800)]).err(),
            Some(AcpiError::InvalidAddress)
        );
        assert_eq!(
            parse(&[lapic_override(0xFEE0_0000), lapic_override(0xFEE0_1000)]).err(),
            Some(AcpiError::Malformed)
        );
        // The 32-bit field itself is validated when there is no override.
        let mut m = FakeMem::default();
        m.put(0x4000, &madt_bytes(0, 1, &[]));
        assert_eq!(
            parse_madt(&m, 0x4000).err(),
            Some(AcpiError::InvalidAddress)
        );
        m.put(0x4000, &madt_bytes(0xFEE0_0010, 1, &[]));
        assert_eq!(
            parse_madt(&m, 0x4000).err(),
            Some(AcpiError::InvalidAddress)
        );
        // ...and an override rescues a zero base address.
        m.put(0x4000, &madt_bytes(0, 1, &[lapic_override(0xFEE0_0000)]));
        assert_eq!(parse_madt(&m, 0x4000).unwrap().lapic_address, 0xFEE0_0000);
    }

    #[test]
    fn disabled_and_online_capable_processors() {
        let t = parse(&[
            lapic(0, 0, 1),
            lapic(1, 1, 0),
            lapic(2, 2, 2),
            lapic(3, 3, 3),
        ])
        .unwrap();
        assert_eq!(t.cpu_count, 4);
        assert_eq!(
            t.usable_cpu_count(),
            3,
            "the disabled, not-online-capable one is unusable"
        );
        assert!(!t.cpus()[1].usable());
        assert!(t.cpus()[2].usable() && !t.cpus()[2].enabled && t.cpus()[2].online_capable);
    }

    #[test]
    fn x2apic_entries_are_decoded() {
        let t = parse(&[x2apic(0x100, 1, 7), lapic(0, 0, 1)]).unwrap();
        assert_eq!(
            t.cpus()[0],
            CpuEntry {
                acpi_id: 7,
                apic_id: 0x100,
                enabled: true,
                online_capable: false,
                x2apic: true
            }
        );
        assert_eq!(
            parse(&[x2apic(5, 1, 0), lapic(0, 5, 1)]).err(),
            Some(AcpiError::Malformed),
            "duplicate APIC id"
        );
    }

    #[test]
    fn unsupported_entries_are_skipped_by_length_and_counted() {
        let mut nmi_src = vec![3, 8, 0, 0];
        nmi_src.extend_from_slice(&[0, 0, 0, 0]); // NMI source (type 3), skipped
        let mut oem = vec![0x80, 5, 1, 2, 3];
        let t = parse(&[
            lapic(0, 0, 1),
            nmi_src,
            oem.clone(),
            iso(1, 3, 3, 0),
            lapic(1, 1, 1),
        ])
        .unwrap();
        assert_eq!(
            t.cpu_count, 2,
            "entries after the unknown ones are still parsed"
        );
        assert_eq!(
            t.unsupported_entries, 3,
            "type 3, OEM, and the non-ISA-bus override"
        );
        assert_eq!(t.override_count, 0);
        oem.clear();
    }

    #[test]
    fn structurally_malformed_entries() {
        // Length 0 and 1 must not loop forever or be skipped.
        assert_eq!(parse(&[vec![0, 0, 0, 0]]).err(), Some(AcpiError::Malformed));
        assert_eq!(
            parse(&[vec![0x7F, 1, 0, 0]]).err(),
            Some(AcpiError::Malformed)
        );
        // Unknown and unsupported types with length 0 or 1 would never advance the cursor.
        assert_eq!(
            parse(&[vec![0x7F, 0, 0, 0]]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(parse(&[vec![3, 0, 0, 0]]).err(), Some(AcpiError::Malformed));
        assert_eq!(
            parse(&[vec![0x80, 1, 0, 0]]).err(),
            Some(AcpiError::Malformed)
        );
        // Known types with the wrong size.
        assert_eq!(
            parse(&[vec![0, 7, 0, 0, 0, 0, 0]]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[vec![0, 9, 0, 0, 0, 0, 0, 0, 0]]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[vec![1, 8, 0, 0, 0, 0, 0xC0, 0xFE]]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[vec![2, 6, 0, 0, 0, 0]]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[vec![4, 5, 0xFF, 0, 0]]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[vec![5, 8, 0, 0, 0, 0, 0, 0]]).err(),
            Some(AcpiError::Malformed)
        );
        // LINT out of range.
        assert_eq!(parse(&[nmi(0xFF, 0, 2)]).err(), Some(AcpiError::Malformed));
    }

    #[test]
    fn truncated_tables() {
        // Entry claims more bytes than the table has.
        let mut e = lapic(0, 0, 1);
        e[1] = 20;
        assert_eq!(parse(&[e]).err(), Some(AcpiError::Truncated));
        // One stray byte after the last entry (cannot hold an entry header).
        let mut body = Vec::new();
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&lapic(0, 0, 1));
        body.push(0);
        let mut m = FakeMem::default();
        m.put(0x4000, &sdt(b"APIC", 5, &body));
        assert_eq!(parse_madt(&m, 0x4000).err(), Some(AcpiError::Truncated));
        // Table shorter than the fixed part.
        let mut m = FakeMem::default();
        m.put(0x4000, &sdt(b"APIC", 5, &[0, 0, 0]));
        assert_eq!(parse_madt(&m, 0x4000).err(), Some(AcpiError::BadLength));
        // Header declares more than is readable.
        let mut m = FakeMem::default();
        let t = madt_bytes(0xFEE0_0000, 1, &[lapic(0, 0, 1)]);
        m.put(0x4000, &t[..t.len() - 4]);
        assert!(matches!(
            parse_madt(&m, 0x4000),
            Err(AcpiError::Unreadable(_))
        ));
    }

    #[test]
    fn checksum_and_signature_are_enforced() {
        let mut t = madt_bytes(0xFEE0_0000, 1, &[lapic(0, 0, 1)]);
        let mut m = FakeMem::default();
        t[40] ^= 0x10;
        m.put(0x4000, &t);
        assert_eq!(parse_madt(&m, 0x4000).err(), Some(AcpiError::BadChecksum));
        let mut m = FakeMem::default();
        m.put(0x4000, &sdt(b"HPET", 1, &[0; 16]));
        assert_eq!(parse_madt(&m, 0x4000).err(), Some(AcpiError::BadSignature));
    }

    #[test]
    fn duplicates_are_rejected() {
        assert_eq!(
            parse(&[lapic(0, 3, 1), lapic(1, 3, 1)]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[ioapic(1, 0xFEC0_0000, 0), ioapic(1, 0xFEC1_0000, 24)]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[ioapic(1, 0xFEC0_0000, 0), ioapic(2, 0xFEC0_0000, 24)]).err(),
            Some(AcpiError::Malformed)
        );
        assert_eq!(
            parse(&[iso(0, 1, 1, 0), iso(0, 1, 5, 0)]).err(),
            Some(AcpiError::Malformed)
        );
        // IOAPIC register bases must be non-null and 4-byte aligned.
        assert_eq!(
            parse(&[ioapic(0, 0, 0)]).err(),
            Some(AcpiError::InvalidAddress)
        );
        assert_eq!(
            parse(&[ioapic(0, 0xFEC0_0002, 0)]).err(),
            Some(AcpiError::InvalidAddress)
        );
    }

    #[test]
    fn capacity_limits_are_reported_not_truncated() {
        let cpus: Vec<_> = (0..MAX_CPUS as u16)
            .map(|i| lapic(i as u8, i as u8, 1))
            .collect();
        assert_eq!(parse(&cpus).unwrap().cpu_count, MAX_CPUS);
        let mut over = cpus.clone();
        over.push(x2apic(0x1000, 1, 99));
        assert_eq!(parse(&over).err(), Some(AcpiError::TooMany));
        let ios: Vec<_> = (0..=MAX_IOAPICS as u32)
            .map(|i| ioapic(i as u8, 0xFEC0_0000 + i * 0x1000, i * 24))
            .collect();
        assert_eq!(
            parse(&ios[..MAX_IOAPICS]).unwrap().ioapic_count,
            MAX_IOAPICS
        );
        assert_eq!(parse(&ios).err(), Some(AcpiError::TooMany));
        let isos: Vec<_> = (0..=MAX_OVERRIDES as u8)
            .map(|i| iso(0, i, i as u32, 0))
            .collect();
        assert_eq!(parse(&isos).err(), Some(AcpiError::TooMany));
        let nmis: Vec<_> = (0..=MAX_NMIS).map(|_| nmi(0xFF, 0, 0)).collect();
        assert_eq!(parse(&nmis).err(), Some(AcpiError::TooMany));
    }

    #[test]
    fn ioapic_candidate_picks_the_highest_base_not_above_the_gsi() {
        let t = parse(&[
            ioapic(0, 0xFEC0_0000, 0),
            ioapic(1, 0xFEC1_0000, 24),
            ioapic(2, 0xFEC2_0000, 48),
        ])
        .unwrap();
        assert_eq!(t.ioapic_candidate(0).unwrap().id, 0);
        assert_eq!(t.ioapic_candidate(23).unwrap().id, 0);
        assert_eq!(t.ioapic_candidate(24).unwrap().id, 1);
        assert_eq!(t.ioapic_candidate(1000).unwrap().id, 2);
        let none = parse(&[ioapic(1, 0xFEC1_0000, 24)]).unwrap();
        assert!(none.ioapic_candidate(3).is_none());
    }

    #[test]
    fn discovery_through_the_root_table_end_to_end() {
        let mut m = FakeMem::default();
        m.put(
            0x4000,
            &madt_bytes(0xFEE0_0000, 1, &[lapic(0, 0, 1), ioapic(0, 0xFEC0_0000, 0)]),
        );
        m.put(0x6000, &root(true, &[0x4000]));
        m.put(0xE_0000, &{
            let mut v = vec![0u8; 64];
            v[..36].copy_from_slice(&rsdp_v2(0, 0x6000));
            v
        });
        let t = discover_bios(&m).unwrap();
        assert_eq!((t.cpu_count, t.ioapic_count), (1, 1));
        let rsdp = Rsdp {
            revision: 2,
            rsdt: 0,
            xsdt: 0x6000,
        };
        assert_eq!(discover(&m, &rsdp).unwrap().lapic_address, 0xFEE0_0000);
        // Mapped-window violation: the table lives where the reader cannot reach.
        let mut m2 = FakeMem::default();
        m2.put(0x6000, &root(true, &[0x1_0000_0000]));
        assert_eq!(
            discover(&m2, &rsdp).err(),
            Some(AcpiError::Unreadable(0x1_0000_0000))
        );
    }
}
