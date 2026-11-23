//! HPET: ACPI table discovery, capability validation, counter access and comparator
//! programming (Mission C3.5). SPDX-License-Identifier: Apache-2.0
//!
//! Registers go through [`crate::apic::Mmio`], so the driver is host-tested against a model of
//! the chip. The HPET block (usually 0xFED00000) is outside the bootstrap identity map; the
//! kernel must not create a `VolatileMmio` for it until it is mapped uncached
//! (`timer::choose_source` falls back to the PIT otherwise).
//!
//! Periodic mode follows the HPET specification's quirk: with `VAL_SET_CNF` set, the first
//! write to a periodic comparator sets the comparator itself and the second sets the period
//! accumulator, so both are written. The main counter must be halted while a timer is
//! programmed (the driver refuses otherwise) so the first expiry is exactly one period after
//! the counter starts.
//!
//! Interrupt routing: either the "legacy replacement" mapping (timer 0 -> IRQ0, timer 1 ->
//! IRQ8, which also works with the 8259) or an IOAPIC GSI taken from the timer's route
//! capability mask. Level-triggered mode is not used.
#![allow(dead_code)]
#![allow(clippy::unnecessary_cast)]

use crate::acpi::{
    AcpiError, PhysReader, Rsdp, SdtHeader, find_table, find_table_bios, le16, le32, le64,
    read_header,
};
use crate::apic::Mmio;
use crate::timer::{TickRate, TimerError, TimerSource};

pub const HPET_SIGNATURE: [u8; 4] = *b"HPET";
const TABLE_LEN: usize = 56;
/// Size of the register block.
pub const HPET_WINDOW_LEN: usize = 0x400;

const REG_CAP: usize = 0x00;
const REG_CONFIG: usize = 0x10;
const REG_INT_STATUS: usize = 0x20;
const REG_COUNTER: usize = 0xF0;
const fn timer_cfg(n: u32) -> usize {
    0x100 + 0x20 * n as usize
}
const fn timer_cmp(n: u32) -> usize {
    0x108 + 0x20 * n as usize
}

const CFG_ENABLE: u64 = 1 << 0;
const CFG_LEGACY: u64 = 1 << 1;

const TCFG_LEVEL: u64 = 1 << 1;
const TCFG_INT_ENABLE: u64 = 1 << 2;
const TCFG_PERIODIC: u64 = 1 << 3;
const TCFG_PERIODIC_CAP: u64 = 1 << 4;
const TCFG_SIZE64: u64 = 1 << 5;
const TCFG_VAL_SET: u64 = 1 << 6;
const TCFG_FORCE32: u64 = 1 << 8;
const TCFG_ROUTE_SHIFT: u32 = 9;
const TCFG_ROUTE_MASK: u64 = 0x1F << TCFG_ROUTE_SHIFT;
const TCFG_FSB_ENABLE: u64 = 1 << 14;

/// The spec bounds the counter clock period at 100 ns (0x05F5E100 femtoseconds).
pub const MAX_PERIOD_FS: u32 = 0x05F5_E100;
const FS_PER_SEC: u64 = 1_000_000_000_000_000;
/// Refuse periods shorter than this many counts: a comparator that close to the counter can be
/// missed by the hardware and the timer would then wait a full counter wrap.
pub const MIN_PERIOD_COUNTS: u64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HpetTable {
    pub header: SdtHeader,
    pub base: u64,
    pub event_timer_block_id: u32,
    pub hpet_number: u8,
    /// Minimum clock ticks for periodic mode without lost interrupts.
    pub min_tick: u16,
    pub page_protection: u8,
}

/// Parses an already checksum-validated HPET table image.
pub fn parse_table(header: SdtHeader, b: &[u8]) -> Result<HpetTable, AcpiError> {
    if header.signature != HPET_SIGNATURE {
        return Err(AcpiError::BadSignature);
    }
    if b.len() < TABLE_LEN {
        return Err(AcpiError::Truncated);
    }
    // Generic Address Structure: space id 0 = system memory.
    let space_id = b[40];
    let base = le64(b, 44);
    if space_id != 0 {
        return Err(AcpiError::Malformed);
    }
    if base == 0 || base & 7 != 0 {
        return Err(AcpiError::InvalidAddress);
    }
    Ok(HpetTable {
        header,
        base,
        event_timer_block_id: le32(b, 36),
        hpet_number: b[52],
        min_tick: le16(b, 53),
        page_protection: b[55],
    })
}

fn load<R: PhysReader + ?Sized>(
    r: &R,
    phys: u64,
    header: SdtHeader,
) -> Result<HpetTable, AcpiError> {
    let mut buf = [0u8; TABLE_LEN];
    if (header.length as usize) < TABLE_LEN {
        return Err(AcpiError::BadLength);
    }
    r.read(phys, &mut buf)
        .map_err(|_| AcpiError::Unreadable(phys))?;
    // Re-read the header from the same bytes so what is parsed is what was validated.
    let _ = read_header(r, phys)?;
    parse_table(header, &buf)
}

/// Finds the HPET table through a validated RSDP.
pub fn discover<R: PhysReader + ?Sized>(r: &R, rsdp: &Rsdp) -> Result<HpetTable, AcpiError> {
    let (phys, header) = find_table(r, rsdp, HPET_SIGNATURE)?;
    load(r, phys, header)
}

pub fn discover_bios<R: PhysReader + ?Sized>(r: &R) -> Result<HpetTable, AcpiError> {
    let (phys, header) = find_table_bios(r, HPET_SIGNATURE)?;
    load(r, phys, header)
}

/// Where a timer's interrupt goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Legacy replacement: timer 0 -> IRQ0, timer 1 -> IRQ8. Requires the chip to support it.
    Legacy,
    /// IOAPIC input `gsi` (0..=31), which must be set in the timer's route-capability mask.
    Gsi(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpetError {
    /// Capability register reads as absent (all ones / zero).
    NoDevice,
    /// Revision 0, period 0 or above the architectural maximum.
    BadCapabilities,
    /// The main counter does not advance while enabled.
    CounterStuck,
    InvalidTimer,
    NotPeriodicCapable,
    LegacyUnsupported,
    /// Route not allowed by the timer's capability mask, or a bad legacy timer.
    BadRoute,
    InvalidFrequency,
    /// The operation requires the main counter halted (or running).
    BadState,
}

impl From<HpetError> for TimerError {
    fn from(e: HpetError) -> Self {
        match e {
            HpetError::InvalidFrequency => TimerError::InvalidFrequency,
            HpetError::NotPeriodicCapable | HpetError::LegacyUnsupported | HpetError::BadRoute => {
                TimerError::Unsupported
            }
            HpetError::BadState => TimerError::BadState,
            _ => TimerError::Hardware,
        }
    }
}

pub struct Hpet<M: Mmio> {
    mmio: M,
    period_fs: u32,
    timers: u32,
    counter64: bool,
    legacy_capable: bool,
    revision: u8,
    min_tick: u16,
}

impl<M: Mmio> Hpet<M> {
    /// Reads and validates the capabilities. `min_tick` comes from the ACPI table (0 if
    /// unknown). Does not enable anything.
    pub fn new(mut mmio: M, min_tick: u16) -> Result<Self, HpetError> {
        let cap = mmio.read64(REG_CAP);
        if cap == 0 || cap == u64::MAX {
            return Err(HpetError::NoDevice);
        }
        let revision = cap as u8;
        let period_fs = (cap >> 32) as u32;
        if revision == 0 || period_fs == 0 || period_fs > MAX_PERIOD_FS {
            return Err(HpetError::BadCapabilities);
        }
        Ok(Self {
            mmio,
            period_fs,
            timers: ((cap >> 8) as u32 & 0x1F) + 1,
            counter64: cap & (1 << 13) != 0,
            legacy_capable: cap & (1 << 15) != 0,
            revision,
            min_tick,
        })
    }

    pub fn mmio_mut(&mut self) -> &mut M {
        &mut self.mmio
    }
    pub fn period_fs(&self) -> u32 {
        self.period_fs
    }
    /// Counter frequency in Hz (1e15 / period_fs), rounded down.
    pub fn frequency_hz(&self) -> u64 {
        FS_PER_SEC / self.period_fs as u64
    }
    pub fn timer_count(&self) -> u32 {
        self.timers
    }
    pub fn counter_is_64bit(&self) -> bool {
        self.counter64
    }
    pub fn legacy_capable(&self) -> bool {
        self.legacy_capable
    }
    pub fn revision(&self) -> u8 {
        self.revision
    }

    fn config(&mut self) -> u64 {
        self.mmio.read64(REG_CONFIG)
    }

    pub fn is_enabled(&mut self) -> bool {
        self.config() & CFG_ENABLE != 0
    }

    /// Main counter. A 32-bit counter returns its low 32 bits.
    pub fn counter(&mut self) -> u64 {
        if self.counter64 {
            self.mmio.read64(REG_COUNTER)
        } else {
            self.mmio.read32(REG_COUNTER) as u64
        }
    }

    pub fn counter_bits(&self) -> u32 {
        if self.counter64 { 64 } else { 32 }
    }

    /// Starts the main counter and verifies that it advances.
    pub fn enable(&mut self) -> Result<(), HpetError> {
        let cfg = self.config();
        self.mmio.write64(REG_CONFIG, cfg | CFG_ENABLE);
        let start = self.counter();
        for _ in 0..1000 {
            if self.counter() != start {
                return Ok(());
            }
        }
        // Leave the chip as we found it.
        self.mmio.write64(REG_CONFIG, cfg & !CFG_ENABLE);
        Err(HpetError::CounterStuck)
    }

    /// Halts the main counter (timers stop generating interrupts as their comparator can no
    /// longer match).
    pub fn disable(&mut self) {
        let cfg = self.config();
        self.mmio.write64(REG_CONFIG, cfg & !CFG_ENABLE);
    }

    /// Selects legacy replacement routing. Only with the counter halted.
    pub fn enable_legacy_replacement(&mut self) -> Result<(), HpetError> {
        if !self.legacy_capable {
            return Err(HpetError::LegacyUnsupported);
        }
        let cfg = self.config();
        if cfg & CFG_ENABLE != 0 {
            return Err(HpetError::BadState);
        }
        self.mmio.write64(REG_CONFIG, cfg | CFG_LEGACY);
        Ok(())
    }

    fn check_timer(&self, timer: u32) -> Result<(), HpetError> {
        if timer < self.timers {
            Ok(())
        } else {
            Err(HpetError::InvalidTimer)
        }
    }

    pub fn timer_is_periodic_capable(&mut self, timer: u32) -> Result<bool, HpetError> {
        self.check_timer(timer)?;
        Ok(self.mmio.read64(timer_cfg(timer)) & TCFG_PERIODIC_CAP != 0)
    }

    /// Bit n set: the timer can be routed to IOAPIC input n.
    pub fn timer_route_mask(&mut self, timer: u32) -> Result<u32, HpetError> {
        self.check_timer(timer)?;
        Ok((self.mmio.read64(timer_cfg(timer)) >> 32) as u32)
    }

    /// Programs `timer` for periodic interrupts at about `hz`; returns the tick definition.
    /// The main counter must be halted; call [`enable`](Self::enable) afterwards.
    pub fn program_periodic(
        &mut self,
        timer: u32,
        hz: u32,
        route: Route,
    ) -> Result<TickRate, HpetError> {
        self.check_timer(timer)?;
        if hz == 0 {
            return Err(HpetError::InvalidFrequency);
        }
        let cfg = self.config();
        if cfg & CFG_ENABLE != 0 {
            return Err(HpetError::BadState);
        }
        let tcfg = self.mmio.read64(timer_cfg(timer));
        if tcfg & TCFG_PERIODIC_CAP == 0 {
            return Err(HpetError::NotPeriodicCapable);
        }
        let route_bits = match route {
            Route::Legacy => {
                if cfg & CFG_LEGACY == 0 || !(timer == 0 || timer == 1) {
                    return Err(HpetError::BadRoute);
                }
                0 // ignored while legacy replacement is active
            }
            Route::Gsi(g) => {
                if g > 31 || (tcfg >> 32) as u32 >> g & 1 == 0 {
                    return Err(HpetError::BadRoute);
                }
                (g as u64) << TCFG_ROUTE_SHIFT
            }
        };
        let freq = self.frequency_hz();
        let period = freq / hz as u64;
        let min = MIN_PERIOD_COUNTS.max(self.min_tick as u64);
        let is64 = tcfg & TCFG_SIZE64 != 0;
        if period < min || (!is64 && period > u32::MAX as u64) {
            return Err(HpetError::InvalidFrequency);
        }
        // Edge-triggered, periodic, interrupt enabled, next comparator write is the value.
        let mut new = tcfg & !(TCFG_LEVEL | TCFG_ROUTE_MASK | TCFG_FSB_ENABLE | TCFG_FORCE32);
        new |= TCFG_INT_ENABLE | TCFG_PERIODIC | TCFG_VAL_SET | route_bits;
        if is64 {
            // 64-bit comparators are used in 64-bit mode.
        } else {
            new |= TCFG_FORCE32;
        }
        self.mmio.write64(timer_cfg(timer), new);
        // Counter is halted: restart from zero so the first expiry is one period away.
        if self.counter64 {
            self.mmio.write64(REG_COUNTER, 0);
        } else {
            self.mmio.write32(REG_COUNTER, 0);
        }
        // First write: comparator. Second write (VAL_SET semantics): period accumulator.
        self.write_comparator(timer, is64, period);
        self.write_comparator(timer, is64, period);
        Ok(TickRate {
            counts_per_tick: period,
            counter_hz: freq,
        })
    }

    fn write_comparator(&mut self, timer: u32, is64: bool, v: u64) {
        if is64 {
            self.mmio.write64(timer_cmp(timer), v);
        } else {
            self.mmio.write32(timer_cmp(timer), v as u32);
        }
    }

    /// Stops interrupt generation from `timer` and clears its pending status.
    pub fn stop_timer(&mut self, timer: u32) -> Result<(), HpetError> {
        self.check_timer(timer)?;
        let t = self.mmio.read64(timer_cfg(timer));
        self.mmio.write64(
            timer_cfg(timer),
            t & !(TCFG_INT_ENABLE | TCFG_PERIODIC | TCFG_VAL_SET),
        );
        self.mmio.write64(REG_INT_STATUS, 1 << timer); // write-1-to-clear
        Ok(())
    }

    /// Full shutdown: every timer's interrupt disabled, counter halted, legacy routing off.
    pub fn shutdown(&mut self) {
        for t in 0..self.timers {
            let _ = self.stop_timer(t);
        }
        let cfg = self.config();
        self.mmio
            .write64(REG_CONFIG, cfg & !(CFG_ENABLE | CFG_LEGACY));
    }
}

/// HPET timer 0 in legacy-replacement mode as a [`TimerSource`]. Interrupts arrive on IRQ0
/// (PIC) or the IOAPIC input the platform wires IRQ0 to.
pub struct HpetTimer<M: Mmio> {
    hpet: Hpet<M>,
    running: bool,
}

impl<M: Mmio> HpetTimer<M> {
    pub fn new(hpet: Hpet<M>) -> Self {
        Self {
            hpet,
            running: false,
        }
    }
    pub fn hpet_mut(&mut self) -> &mut Hpet<M> {
        &mut self.hpet
    }
    pub fn into_inner(self) -> Hpet<M> {
        self.hpet
    }
}

impl<M: Mmio> TimerSource for HpetTimer<M> {
    fn name(&self) -> &'static str {
        "HPET timer 0"
    }

    fn start_periodic(&mut self, hz: u32) -> Result<TickRate, TimerError> {
        if self.running {
            return Err(TimerError::BadState);
        }
        self.hpet.enable_legacy_replacement()?;
        let rate = self.hpet.program_periodic(0, hz, Route::Legacy)?;
        if let Err(e) = self.hpet.enable() {
            self.hpet.shutdown();
            return Err(e.into());
        }
        self.running = true;
        Ok(rate)
    }

    fn stop(&mut self) {
        self.hpet.shutdown();
        self.running = false;
    }
}

#[cfg(test)]
pub(crate) mod model {
    use super::*;
    use std::collections::BTreeMap;

    /// HPET register-file model: capability register, general config, per-timer config with
    /// read-only capability bits, comparator VAL_SET semantics, and a counter that advances by
    /// `step` on every read while enabled.
    pub struct ModelHpet {
        pub cap: u64,
        pub config: u64,
        pub int_status: u64,
        pub counter: u64,
        pub step: u64,
        pub tcfg: Vec<u64>,
        pub cmp: Vec<u64>,
        pub period: Vec<u64>,
        pub val_set_pending: Vec<bool>,
        pub writes: Vec<(usize, u64, u8)>,
        pub tcfg_ro: u64,
        _unused: BTreeMap<u8, u8>,
    }

    pub const GOOD_CAP: u64 =
        (69_841_279u64 << 32) | (0x8086 << 16) | (1 << 15) | (1 << 13) | (2 << 8) | 1;

    impl ModelHpet {
        pub fn with_cap(cap: u64) -> Self {
            let n = ((cap >> 8) as usize & 0x1F) + 1;
            let ro = TCFG_PERIODIC_CAP | TCFG_SIZE64 | (0xF4u64 << 32); // routes 2,4,5,6,7
            Self {
                cap,
                config: 0,
                int_status: 0,
                counter: 0,
                step: 100,
                tcfg: vec![ro; n],
                cmp: vec![0; n],
                period: vec![0; n],
                val_set_pending: vec![false; n],
                writes: vec![],
                tcfg_ro: ro,
                _unused: BTreeMap::new(),
            }
        }
        pub fn good() -> Self {
            Self::with_cap(GOOD_CAP)
        }
    }

    impl Mmio for ModelHpet {
        fn read32(&mut self, off: usize) -> u32 {
            match off {
                REG_COUNTER => {
                    self.advance();
                    self.counter as u32
                }
                _ => panic!("32-bit read at {off:#x} not modelled"),
            }
        }
        fn write32(&mut self, off: usize, v: u32) {
            self.writes.push((off, v as u64, 4));
            match off {
                REG_COUNTER => {
                    assert_eq!(self.config & CFG_ENABLE, 0, "counter written while running");
                    self.counter = v as u64;
                }
                o if (0x108..0x108 + 0x20 * 32).contains(&o) && (o - 0x108) % 0x20 == 0 => {
                    self.cmp_write(((o - 0x108) / 0x20) as usize, v as u64);
                }
                _ => panic!("32-bit write at {off:#x} not modelled"),
            }
        }
        fn read64(&mut self, off: usize) -> u64 {
            match off {
                REG_CAP => self.cap,
                REG_CONFIG => self.config,
                REG_INT_STATUS => self.int_status,
                REG_COUNTER => {
                    self.advance();
                    self.counter
                }
                o if (0x100..0x100 + 0x20 * 32).contains(&o) && (o - 0x100) % 0x20 == 0 => {
                    self.tcfg[((o - 0x100) / 0x20) as usize]
                }
                o if (0x108..0x108 + 0x20 * 32).contains(&o) && (o - 0x108) % 0x20 == 0 => {
                    self.cmp[((o - 0x108) / 0x20) as usize]
                }
                _ => panic!("64-bit read at {off:#x} not modelled"),
            }
        }
        fn write64(&mut self, off: usize, v: u64) {
            self.writes.push((off, v, 8));
            match off {
                REG_CONFIG => self.config = v & (CFG_ENABLE | CFG_LEGACY),
                REG_INT_STATUS => self.int_status &= !v,
                REG_COUNTER => {
                    assert_eq!(self.config & CFG_ENABLE, 0, "counter written while running");
                    self.counter = v;
                }
                o if (0x100..0x100 + 0x20 * 32).contains(&o) && (o - 0x100) % 0x20 == 0 => {
                    let i = ((o - 0x100) / 0x20) as usize;
                    let writable = !self.tcfg_ro;
                    self.tcfg[i] = (self.tcfg[i] & self.tcfg_ro) | (v & writable);
                    self.val_set_pending[i] = v & TCFG_VAL_SET != 0;
                    self.tcfg[i] &= !TCFG_VAL_SET; // reads back as zero; the model tracks it separately
                }
                o if (0x108..0x108 + 0x20 * 32).contains(&o) && (o - 0x108) % 0x20 == 0 => {
                    self.cmp_write(((o - 0x108) / 0x20) as usize, v);
                }
                _ => panic!("64-bit write at {off:#x} not modelled"),
            }
        }
    }

    impl ModelHpet {
        fn advance(&mut self) {
            if self.config & CFG_ENABLE != 0 {
                self.counter = self.counter.wrapping_add(self.step);
            }
        }
        fn cmp_write(&mut self, i: usize, v: u64) {
            if self.tcfg[i] & TCFG_PERIODIC != 0 && !self.val_set_pending[i] {
                self.period[i] = v; // accumulator
            } else {
                self.cmp[i] = v;
                self.val_set_pending[i] = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::model::*;
    use super::*;
    use crate::acpi::testutil::*;
    use crate::acpi::{Rsdp, parse_header};

    fn hpet() -> Hpet<ModelHpet> {
        Hpet::new(ModelHpet::good(), 0).unwrap()
    }

    // ---- ACPI table --------------------------------------------------------------------

    fn table_body(space: u8, base: u64, number: u8, min_tick: u16) -> Vec<u8> {
        let mut b = vec![];
        b.extend_from_slice(&0x8086_A201u32.to_le_bytes()); // event timer block id
        b.extend_from_slice(&[space, 64, 0, 0]); // GAS: space, width, offset, access
        b.extend_from_slice(&base.to_le_bytes());
        b.push(number);
        b.extend_from_slice(&min_tick.to_le_bytes());
        b.push(0); // page protection
        b
    }

    fn world(body: &[u8]) -> (FakeMem, Rsdp) {
        let mut m = FakeMem::default();
        m.put(0x7000, &sdt(b"HPET", 1, body));
        m.put(0x6000, &root(true, &[0x7000]));
        (
            m,
            Rsdp {
                revision: 2,
                rsdt: 0,
                xsdt: 0x6000,
            },
        )
    }

    #[test]
    fn table_discovery_and_decode() {
        let (m, rsdp) = world(&table_body(0, 0xFED0_0000, 0, 0x80));
        let t = discover(&m, &rsdp).unwrap();
        assert_eq!(t.base, 0xFED0_0000);
        assert_eq!(
            (t.hpet_number, t.min_tick, t.event_timer_block_id),
            (0, 0x80, 0x8086_A201)
        );
        assert_eq!(t.header.length, 36 + 20);
    }

    #[test]
    fn table_rejections() {
        for (body, want) in [
            (table_body(1, 0xFED0_0000, 0, 0), AcpiError::Malformed), // I/O port space
            (table_body(0, 0, 0, 0), AcpiError::InvalidAddress),
            (table_body(0, 0xFED0_0004, 0, 0), AcpiError::InvalidAddress), // unaligned
        ] {
            let (m, rsdp) = world(&body);
            assert_eq!(discover(&m, &rsdp).err(), Some(want));
        }
        // Too short for the structure (checksum is valid; length is the problem).
        let (m, rsdp) = world(&table_body(0, 0xFED0_0000, 0, 0)[..10]);
        assert_eq!(discover(&m, &rsdp).err(), Some(AcpiError::BadLength));
        // Wrong signature handed to the parser directly.
        let t = sdt(b"APIC", 1, &table_body(0, 0xFED0_0000, 0, 0));
        let mut h = [0u8; 36];
        h.copy_from_slice(&t[..36]);
        assert_eq!(
            parse_table(parse_header(&h), &t),
            Err(AcpiError::BadSignature)
        );
        // No HPET at all.
        let mut m = FakeMem::default();
        m.put(0x6000, &root(true, &[]));
        let rsdp = Rsdp {
            revision: 2,
            rsdt: 0,
            xsdt: 0x6000,
        };
        assert_eq!(discover(&m, &rsdp).err(), Some(AcpiError::NotFound));
    }

    // ---- capabilities ------------------------------------------------------------------

    #[test]
    fn capabilities_are_decoded() {
        let mut h = hpet();
        assert_eq!(h.period_fs(), 69_841_279);
        assert_eq!(h.frequency_hz(), 14_318_179);
        assert_eq!(h.timer_count(), 3);
        assert!(h.counter_is_64bit() && h.legacy_capable());
        assert_eq!(h.revision(), 1);
        assert_eq!(h.counter_bits(), 64);
        assert!(!h.is_enabled());
        assert_eq!(h.timer_is_periodic_capable(0), Ok(true));
        assert_eq!(h.timer_route_mask(1), Ok(0xF4));
        assert_eq!(h.timer_is_periodic_capable(3), Err(HpetError::InvalidTimer));
    }

    #[test]
    fn bad_capabilities_are_rejected() {
        let bad_period_zero = GOOD_CAP & 0xFFFF_FFFF;
        let bad_period_big = (GOOD_CAP & 0xFFFF_FFFF) | ((MAX_PERIOD_FS as u64 + 1) << 32);
        let bad_rev = GOOD_CAP & !0xFF;
        assert!(matches!(
            Hpet::new(ModelHpet::with_cap(u64::MAX), 0),
            Err(HpetError::NoDevice)
        ));
        assert!(matches!(
            Hpet::new(ModelHpet::with_cap(0), 0),
            Err(HpetError::NoDevice)
        ));
        for cap in [bad_period_zero, bad_period_big, bad_rev] {
            assert!(
                matches!(
                    Hpet::new(ModelHpet::with_cap(cap), 0),
                    Err(HpetError::BadCapabilities)
                ),
                "{cap:#x}"
            );
        }
        // The architectural maximum period is accepted.
        let edge = (GOOD_CAP & 0xFFFF_FFFF) | ((MAX_PERIOD_FS as u64) << 32);
        assert!(Hpet::new(ModelHpet::with_cap(edge), 0).is_ok());
    }

    // ---- counter -----------------------------------------------------------------------

    #[test]
    fn enable_verifies_the_counter_advances_and_disable_stops_it() {
        let mut h = hpet();
        h.enable().unwrap();
        assert!(h.is_enabled());
        let a = h.counter();
        let b = h.counter();
        assert!(b > a);
        h.disable();
        assert!(!h.is_enabled());
        let c = h.counter();
        assert_eq!(h.counter(), c, "a halted counter does not move");
    }

    #[test]
    fn a_stuck_counter_is_reported_and_the_chip_is_left_disabled() {
        let mut m = ModelHpet::good();
        m.step = 0;
        let mut h = Hpet::new(m, 0).unwrap();
        assert_eq!(h.enable(), Err(HpetError::CounterStuck));
        assert!(!h.is_enabled());
    }

    #[test]
    fn a_32_bit_counter_reads_32_bits_and_wraps() {
        let cap = GOOD_CAP & !(1 << 13);
        let mut m = ModelHpet::with_cap(cap);
        m.counter = 0xFFFF_FFF0;
        m.config = CFG_ENABLE;
        let mut h = Hpet::new(m, 0).unwrap();
        assert_eq!(h.counter_bits(), 32);
        let a = h.counter();
        assert_eq!(a, 0x54, "0xFFFF_FFF0 + 100 wraps to 0x54 in 32 bits");
        assert_eq!(
            crate::timer::counter_delta(0xFFFF_FFF0, a, h.counter_bits()),
            100
        );
    }

    // ---- periodic programming ----------------------------------------------------------

    #[test]
    fn periodic_programming_via_legacy_replacement() {
        let mut h = hpet();
        h.enable_legacy_replacement().unwrap();
        let rate = h.program_periodic(0, 1000, Route::Legacy).unwrap();
        assert_eq!(
            rate,
            TickRate {
                counts_per_tick: 14_318,
                counter_hz: 14_318_179
            }
        );
        let m = h.mmio_mut();
        assert_eq!(
            m.cmp[0], 14_318,
            "first expiry one period after the counter starts"
        );
        assert_eq!(
            m.period[0], 14_318,
            "accumulator set by the second comparator write"
        );
        let c = m.tcfg[0];
        assert_ne!(c & TCFG_INT_ENABLE, 0);
        assert_ne!(c & TCFG_PERIODIC, 0);
        assert_eq!(c & TCFG_LEVEL, 0, "edge triggered");
        assert_eq!(m.counter, 0);
        assert_eq!(m.config & CFG_LEGACY, CFG_LEGACY);
        // Exactly two comparator writes, in order, after the config write.
        let cmp_writes: Vec<_> = m.writes.iter().filter(|w| w.0 == 0x108).collect();
        assert_eq!(cmp_writes.len(), 2);
    }

    #[test]
    fn periodic_programming_via_ioapic_route_uses_only_capable_inputs() {
        let mut h = hpet();
        let r = h.program_periodic(1, 100, Route::Gsi(5)).unwrap();
        assert_eq!(r.counts_per_tick, 143_181);
        assert_eq!(
            (h.mmio_mut().tcfg[1] & TCFG_ROUTE_MASK) >> TCFG_ROUTE_SHIFT,
            5
        );
        // GSI 3 is not in the capability mask 0xF4 (bits 2,4-7): rejected, nothing written.
        let before = h.mmio_mut().writes.len();
        assert_eq!(
            h.program_periodic(1, 100, Route::Gsi(3)),
            Err(HpetError::BadRoute)
        );
        assert_eq!(
            h.program_periodic(1, 100, Route::Gsi(32)),
            Err(HpetError::BadRoute)
        );
        assert_eq!(h.mmio_mut().writes.len(), before);
    }

    #[test]
    fn programming_preconditions() {
        let mut h = hpet();
        assert_eq!(
            h.program_periodic(0, 100, Route::Legacy),
            Err(HpetError::BadRoute),
            "legacy not enabled"
        );
        assert_eq!(
            h.program_periodic(5, 100, Route::Gsi(5)),
            Err(HpetError::InvalidTimer)
        );
        assert_eq!(
            h.program_periodic(0, 0, Route::Gsi(5)),
            Err(HpetError::InvalidFrequency)
        );
        // Period below the safe minimum (14.3 MHz / 20 kHz = 715 counts < 1000).
        assert_eq!(
            h.program_periodic(0, 20_000, Route::Gsi(5)),
            Err(HpetError::InvalidFrequency)
        );
        assert!(h.program_periodic(0, 14_000, Route::Gsi(5)).is_ok());
        // Legacy routing is for timers 0 and 1 only.
        h.enable_legacy_replacement().unwrap();
        assert_eq!(
            h.program_periodic(2, 100, Route::Legacy),
            Err(HpetError::BadRoute)
        );
        // Programming while the counter runs is refused.
        let mut h = hpet();
        h.enable().unwrap();
        assert_eq!(
            h.program_periodic(0, 100, Route::Gsi(5)),
            Err(HpetError::BadState)
        );
        assert_eq!(h.enable_legacy_replacement(), Err(HpetError::BadState));
        // Chips without the capability.
        let mut nolegacy = Hpet::new(ModelHpet::with_cap(GOOD_CAP & !(1 << 15)), 0).unwrap();
        assert_eq!(
            nolegacy.enable_legacy_replacement(),
            Err(HpetError::LegacyUnsupported)
        );
        // Timers that are not periodic capable.
        let mut m = ModelHpet::good();
        m.tcfg[0] &= !TCFG_PERIODIC_CAP;
        m.tcfg_ro &= !TCFG_PERIODIC_CAP;
        let mut h = Hpet::new(m, 0).unwrap();
        assert_eq!(
            h.program_periodic(0, 100, Route::Gsi(5)),
            Err(HpetError::NotPeriodicCapable)
        );
    }

    #[test]
    fn acpi_min_tick_raises_the_minimum_period() {
        let mut h = Hpet::new(ModelHpet::good(), 5000).unwrap();
        // 14.3 MHz / 3000 Hz = 4772 counts < 5000: refused only because of the table's min_tick.
        assert_eq!(
            h.program_periodic(0, 3000, Route::Gsi(5)),
            Err(HpetError::InvalidFrequency)
        );
        assert!(h.program_periodic(0, 1000, Route::Gsi(5)).is_ok());
    }

    #[test]
    fn a_32_bit_comparator_rejects_periods_that_do_not_fit() {
        let mut m = ModelHpet::good();
        m.tcfg[0] &= !TCFG_SIZE64;
        m.tcfg_ro &= !TCFG_SIZE64;
        let mut h = Hpet::new(m, 0).unwrap();
        // 14.3 MHz / 1 Hz = 14_318_179 fits in 32 bits.
        assert!(h.program_periodic(0, 1, Route::Gsi(5)).is_ok());
        let m = h.mmio_mut();
        assert_ne!(m.tcfg[0] & TCFG_FORCE32, 0);
        assert!(
            m.writes.iter().any(|w| w.0 == 0x108 && w.2 == 4),
            "32-bit comparator writes"
        );
        assert!(!m.writes.iter().any(|w| w.0 == 0x108 && w.2 == 8));
    }

    // ---- shutdown and the TimerSource ---------------------------------------------------

    #[test]
    fn shutdown_disables_every_timer_the_counter_and_legacy_mode() {
        let mut h = hpet();
        h.enable_legacy_replacement().unwrap();
        h.program_periodic(0, 1000, Route::Legacy).unwrap();
        h.program_periodic(1, 100, Route::Gsi(5)).unwrap();
        h.enable().unwrap();
        h.mmio_mut().int_status = 0b11;
        h.shutdown();
        let m = h.mmio_mut();
        assert_eq!(m.config & (CFG_ENABLE | CFG_LEGACY), 0);
        for t in 0..3 {
            assert_eq!(
                m.tcfg[t] & (TCFG_INT_ENABLE | TCFG_PERIODIC),
                0,
                "timer {t}"
            );
        }
        assert_eq!(m.int_status, 0, "pending status cleared");
    }

    #[test]
    fn timer_source_lifecycle() {
        let mut t = HpetTimer::new(hpet());
        assert_eq!(t.name(), "HPET timer 0");
        let rate = t.start_periodic(1000).unwrap();
        assert_eq!(rate.counts_per_tick, 14_318);
        assert!(t.hpet_mut().is_enabled());
        assert_eq!(
            t.start_periodic(1000),
            Err(TimerError::BadState),
            "already running"
        );
        t.stop();
        assert!(!t.hpet_mut().is_enabled());
        // Restart after stop works (legacy flag was cleared and is set again).
        assert!(t.start_periodic(500).is_ok());
    }

    #[test]
    fn timer_source_failure_leaves_the_chip_quiet() {
        // Counter that never advances.
        let mut m = ModelHpet::good();
        m.step = 0;
        let mut t = HpetTimer::new(Hpet::new(m, 0).unwrap());
        assert_eq!(t.start_periodic(1000), Err(TimerError::Hardware));
        let h = t.hpet_mut();
        assert!(!h.is_enabled());
        assert_eq!(h.mmio_mut().config & (CFG_ENABLE | CFG_LEGACY), 0);
        assert_eq!(
            t.start_periodic(1000),
            Err(TimerError::Hardware),
            "and it can be retried"
        );
        // No legacy support -> Unsupported, not Hardware.
        let mut t =
            HpetTimer::new(Hpet::new(ModelHpet::with_cap(GOOD_CAP & !(1 << 15)), 0).unwrap());
        assert_eq!(t.start_periodic(1000), Err(TimerError::Unsupported));
        let mut t = HpetTimer::new(hpet());
        assert_eq!(t.start_periodic(0), Err(TimerError::InvalidFrequency));
    }

    #[test]
    fn register_map_offsets() {
        assert!(timer_cmp(2) + 8 <= HPET_WINDOW_LEN);
        assert_eq!(timer_cfg(0), 0x100);
        assert_eq!(timer_cmp(2), 0x148);
    }
}
