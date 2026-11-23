//! I/O APIC driver and the APIC-side interrupt controller (Mission C3.4, part 2).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Register access goes through [`crate::apic::Mmio`] (IOREGSEL/IOWIN indirection), so the
//! redirection-table logic is host-tested against a model of the chip. See `apic.rs` for the
//! mapping constraint: IOAPIC registers (usually 0xFEC00000) are outside the bootstrap map and
//! must not be touched until they are mapped uncached.
//!
//! Routing rules:
//! * A route is created **masked**. The driver registers its handler first, then unmasks.
//! * Only vectors `irq::is_registrable` (legacy/dynamic) can be routed, so an exception vector,
//!   the reserved range, or the spurious vector can never be programmed into a redirection entry.
//! * One GSI -> one vector -> one IOAPIC pin; routing a GSI twice or reusing a vector is refused.
//! * Destination is a physical APIC ID, fixed delivery. Broadcast (0xFF) is refused.
//! * Polarity and trigger must be resolved by the caller (MADT overrides resolved by
//!   [`ApicController::request_isa_irq`]); "bus default" is never written blindly.
#![allow(dead_code)]
#![allow(
    clippy::manual_is_multiple_of,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::identity_op,
    clippy::unusual_byte_groupings
)]

use crate::acpi::madt::{IoApicEntry, Madt, Polarity, Trigger};
use crate::apic::{ApicRegs, LocalApic, Mmio};
use crate::irq::{
    ControllerError, Handler, InterruptController, IrqError, IrqTable, Owner, is_registrable,
};

pub const MAX_IOAPICS: usize = crate::acpi::madt::MAX_IOAPICS;
const REG_SELECT: usize = 0x00;
const REG_WINDOW: usize = 0x10;
/// Window size of the register block.
pub const IOAPIC_WINDOW_LEN: usize = 0x20;

const IOAPIC_ID: u8 = 0x00;
const IOAPIC_VER: u8 = 0x01;
const IOAPIC_REDTBL: u8 = 0x10;

const RTE_MASK: u32 = 1 << 16;
const RTE_LEVEL: u32 = 1 << 15;
const RTE_ACTIVE_LOW: u32 = 1 << 13;
const RTE_DEST_LOGICAL: u32 = 1 << 11;
const RTE_DELIVERY_SHIFT: u32 = 8;
const RTE_DELIVERY_STATUS: u32 = 1 << 12;
const RTE_REMOTE_IRR: u32 = 1 << 14;
const MAX_PINS: u32 = 240;
pub const NO_GSI: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Fixed,
    LowestPriority,
    Smi,
    Nmi,
    Init,
    ExtInt,
    /// Reserved encoding.
    Reserved(u8),
}

impl Delivery {
    fn decode(bits: u32) -> Self {
        match bits & 7 {
            0 => Self::Fixed,
            1 => Self::LowestPriority,
            2 => Self::Smi,
            4 => Self::Nmi,
            5 => Self::Init,
            7 => Self::ExtInt,
            n => Self::Reserved(n as u8),
        }
    }
    fn encode(self) -> u32 {
        match self {
            Self::Fixed => 0,
            Self::LowestPriority => 1,
            Self::Smi => 2,
            Self::Nmi => 4,
            Self::Init => 5,
            Self::ExtInt => 7,
            Self::Reserved(n) => n as u32 & 7,
        }
    }
}

/// A redirection table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rte {
    pub vector: u8,
    pub delivery: Delivery,
    pub dest_logical: bool,
    pub polarity: Polarity,
    pub trigger: Trigger,
    pub masked: bool,
    pub dest: u8,
    /// Read-only status bits, as read back.
    pub delivery_pending: bool,
    pub remote_irr: bool,
}

impl Rte {
    /// A fixed-delivery, physical-destination entry. The read-only bits are zero.
    pub fn fixed(vector: u8, dest: u8, polarity: Polarity, trigger: Trigger, masked: bool) -> Self {
        Self {
            vector,
            delivery: Delivery::Fixed,
            dest_logical: false,
            polarity,
            trigger,
            masked,
            dest,
            delivery_pending: false,
            remote_irr: false,
        }
    }

    pub fn encode(&self) -> (u32, u32) {
        let mut lo = self.vector as u32 | self.delivery.encode() << RTE_DELIVERY_SHIFT;
        if self.dest_logical {
            lo |= RTE_DEST_LOGICAL;
        }
        if self.polarity == Polarity::ActiveLow {
            lo |= RTE_ACTIVE_LOW;
        }
        if self.trigger == Trigger::Level {
            lo |= RTE_LEVEL;
        }
        if self.masked {
            lo |= RTE_MASK;
        }
        (lo, (self.dest as u32) << 24)
    }

    pub fn decode(lo: u32, hi: u32) -> Self {
        Self {
            vector: lo as u8,
            delivery: Delivery::decode(lo >> RTE_DELIVERY_SHIFT),
            dest_logical: lo & RTE_DEST_LOGICAL != 0,
            polarity: if lo & RTE_ACTIVE_LOW != 0 {
                Polarity::ActiveLow
            } else {
                Polarity::ActiveHigh
            },
            trigger: if lo & RTE_LEVEL != 0 {
                Trigger::Level
            } else {
                Trigger::Edge
            },
            masked: lo & RTE_MASK != 0,
            dest: (hi >> 24) as u8,
            delivery_pending: lo & RTE_DELIVERY_STATUS != 0,
            remote_irr: lo & RTE_REMOTE_IRR != 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoApicError {
    /// Version register reads as absent (all ones / zero).
    NoDevice,
    BadPinCount,
    /// `gsi_base + pins` overflows.
    GsiOverflow,
    InvalidPin,
}

pub struct IoApic<M: Mmio> {
    mmio: M,
    id: u8,
    gsi_base: u32,
    pins: u32,
    version: u8,
    routed: [u64; 4],
}

impl<M: Mmio> IoApic<M> {
    /// Probes the chip: reads the version register and derives the pin count from it.
    pub fn new(mut mmio: M, id: u8, gsi_base: u32) -> Result<Self, IoApicError> {
        mmio.write32(REG_SELECT, IOAPIC_VER as u32);
        let raw = mmio.read32(REG_WINDOW);
        if raw == 0 || raw == 0xFFFF_FFFF {
            return Err(IoApicError::NoDevice);
        }
        let version = raw as u8;
        let pins = ((raw >> 16) & 0xFF) + 1;
        if version < 0x10 || pins > MAX_PINS {
            return Err(IoApicError::BadPinCount);
        }
        gsi_base.checked_add(pins).ok_or(IoApicError::GsiOverflow)?;
        Ok(Self {
            mmio,
            id,
            gsi_base,
            pins,
            version,
            routed: [0; 4],
        })
    }

    pub fn id(&self) -> u8 {
        self.id
    }
    pub fn gsi_base(&self) -> u32 {
        self.gsi_base
    }
    pub fn pins(&self) -> u32 {
        self.pins
    }
    pub fn version(&self) -> u8 {
        self.version
    }
    /// First GSI past this chip's range.
    pub fn gsi_end(&self) -> u32 {
        self.gsi_base + self.pins
    }
    pub fn contains(&self, gsi: u32) -> bool {
        gsi >= self.gsi_base && gsi < self.gsi_end()
    }
    pub fn mmio_mut(&mut self) -> &mut M {
        &mut self.mmio
    }

    fn read_reg(&mut self, reg: u8) -> u32 {
        self.mmio.write32(REG_SELECT, reg as u32);
        self.mmio.read32(REG_WINDOW)
    }

    fn write_reg(&mut self, reg: u8, value: u32) {
        self.mmio.write32(REG_SELECT, reg as u32);
        self.mmio.write32(REG_WINDOW, value);
    }

    pub fn hw_id(&mut self) -> u8 {
        (self.read_reg(IOAPIC_ID) >> 24) as u8 & 0x0F
    }

    fn check_pin(&self, pin: u32) -> Result<(), IoApicError> {
        if pin < self.pins {
            Ok(())
        } else {
            Err(IoApicError::InvalidPin)
        }
    }

    pub fn read_rte(&mut self, pin: u32) -> Result<Rte, IoApicError> {
        self.check_pin(pin)?;
        let base = IOAPIC_REDTBL + 2 * pin as u8;
        let lo = self.read_reg(base);
        let hi = self.read_reg(base + 1);
        Ok(Rte::decode(lo, hi))
    }

    /// Programs a pin. The entry is masked while its fields change, so it can never deliver an
    /// interrupt with a half-written vector or destination; the final write applies the
    /// requested mask state.
    pub fn write_rte(&mut self, pin: u32, rte: &Rte) -> Result<(), IoApicError> {
        self.check_pin(pin)?;
        let base = IOAPIC_REDTBL + 2 * pin as u8;
        let (lo, hi) = rte.encode();
        self.write_reg(base, lo | RTE_MASK);
        self.write_reg(base + 1, hi);
        self.write_reg(base, lo);
        Ok(())
    }

    fn set_mask(&mut self, pin: u32, masked: bool) -> Result<(), IoApicError> {
        self.check_pin(pin)?;
        let reg = IOAPIC_REDTBL + 2 * pin as u8;
        let lo = self.read_reg(reg);
        let new = if masked {
            lo | RTE_MASK
        } else {
            lo & !RTE_MASK
        };
        // The delivery-status and remote-IRR bits are read-only; writing them back is ignored.
        self.write_reg(reg, new);
        Ok(())
    }

    pub fn mask_pin(&mut self, pin: u32) -> Result<(), IoApicError> {
        self.set_mask(pin, true)
    }

    pub fn unmask_pin(&mut self, pin: u32) -> Result<(), IoApicError> {
        self.set_mask(pin, false)
    }

    fn routed_bit(&self, pin: u32) -> bool {
        self.routed[(pin / 64) as usize] >> (pin % 64) & 1 != 0
    }
    fn set_routed(&mut self, pin: u32, v: bool) {
        let w = &mut self.routed[(pin / 64) as usize];
        if v {
            *w |= 1 << (pin % 64);
        } else {
            *w &= !(1 << (pin % 64));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    /// Exception, reserved or spurious vector.
    VectorNotAllowed,
    VectorInUse,
    /// No IOAPIC covers the GSI.
    NoIoApic,
    AlreadyRouted,
    NotRouted,
    /// Broadcast (0xFF) is not a valid fixed-delivery physical destination.
    InvalidDest,
    /// The new IOAPIC's GSI range overlaps one already added.
    Overlap,
    TooManyIoApics,
    Irq(IrqError),
    Chip(IoApicError),
}

/// The APIC interrupt controller: the BSP local APIC plus the system's IOAPICs.
pub struct ApicController<R: ApicRegs, M: Mmio> {
    lapic: LocalApic<R>,
    ios: [Option<IoApic<M>>; MAX_IOAPICS],
    count: usize,
    vec_gsi: [u32; 256],
}

impl<R: ApicRegs, M: Mmio> ApicController<R, M> {
    /// `lapic` must already be initialised (`LocalApic::init`).
    pub fn new(lapic: LocalApic<R>) -> Self {
        Self {
            lapic,
            ios: [const { None }; MAX_IOAPICS],
            count: 0,
            vec_gsi: [NO_GSI; 256],
        }
    }

    pub fn lapic_mut(&mut self) -> &mut LocalApic<R> {
        &mut self.lapic
    }

    pub fn ioapic_count(&self) -> usize {
        self.count
    }

    pub fn ioapic(&mut self, index: usize) -> Option<&mut IoApic<M>> {
        self.ios.get_mut(index)?.as_mut()
    }

    /// Adds a probed IOAPIC, refusing GSI ranges that overlap one already present.
    pub fn add_ioapic(&mut self, io: IoApic<M>) -> Result<(), RouteError> {
        if self.count == MAX_IOAPICS {
            return Err(RouteError::TooManyIoApics);
        }
        let overlaps = self
            .ios
            .iter()
            .flatten()
            .any(|o| io.gsi_base() < o.gsi_end() && o.gsi_base() < io.gsi_end());
        if overlaps {
            return Err(RouteError::Overlap);
        }
        self.ios[self.count] = Some(io);
        self.count += 1;
        Ok(())
    }

    /// Probes one chip per MADT entry using `window(entry)` to obtain its register window, and
    /// adds them all. The closure is where the caller guarantees the mapping.
    pub fn add_from_madt(
        &mut self,
        madt: &Madt,
        mut window: impl FnMut(&IoApicEntry) -> Option<M>,
    ) -> Result<(), RouteError> {
        for e in madt.ioapics() {
            let m = window(e).ok_or(RouteError::NoIoApic)?;
            let io = IoApic::new(m, e.id, e.gsi_base).map_err(RouteError::Chip)?;
            self.add_ioapic(io)?;
        }
        Ok(())
    }

    fn find(&mut self, gsi: u32) -> Option<(&mut IoApic<M>, u32)> {
        self.ios
            .iter_mut()
            .flatten()
            .find(|io| io.contains(gsi))
            .map(|io| {
                let pin = gsi - io.gsi_base();
                (io, pin)
            })
    }

    /// Routes `gsi` to `vector` on the CPU with APIC ID `dest`. The entry is left masked.
    pub fn route_gsi(
        &mut self,
        gsi: u32,
        vector: u8,
        dest: u8,
        polarity: Polarity,
        trigger: Trigger,
    ) -> Result<(), RouteError> {
        if !is_registrable(vector) {
            return Err(RouteError::VectorNotAllowed);
        }
        if self.vec_gsi[vector as usize] != NO_GSI {
            return Err(RouteError::VectorInUse);
        }
        if dest == 0xFF {
            return Err(RouteError::InvalidDest);
        }
        let (io, pin) = self.find(gsi).ok_or(RouteError::NoIoApic)?;
        if io.routed_bit(pin) {
            return Err(RouteError::AlreadyRouted);
        }
        io.write_rte(pin, &Rte::fixed(vector, dest, polarity, trigger, true))
            .map_err(RouteError::Chip)?;
        io.set_routed(pin, true);
        self.vec_gsi[vector as usize] = gsi;
        Ok(())
    }

    /// Masks and forgets the route for `gsi`.
    pub fn unroute_gsi(&mut self, gsi: u32) -> Result<(), RouteError> {
        let vector = self
            .vec_gsi
            .iter()
            .position(|g| *g == gsi)
            .ok_or(RouteError::NotRouted)?;
        let (io, pin) = self.find(gsi).ok_or(RouteError::NoIoApic)?;
        io.mask_pin(pin).map_err(RouteError::Chip)?;
        io.set_routed(pin, false);
        self.vec_gsi[vector] = NO_GSI;
        Ok(())
    }

    /// Allocates a dynamic vector, registers `handler` on it, and routes `gsi` to it (masked).
    /// If routing fails the vector is released again, so a failure leaves no residue.
    #[allow(clippy::too_many_arguments)]
    pub fn request_gsi(
        &mut self,
        table: &IrqTable,
        owner: Owner,
        handler: Handler,
        cookie: usize,
        gsi: u32,
        dest: u8,
        polarity: Polarity,
        trigger: Trigger,
    ) -> Result<u8, RouteError> {
        let vector = table
            .register_dynamic(owner, handler, cookie)
            .map_err(RouteError::Irq)?;
        match self.route_gsi(gsi, vector, dest, polarity, trigger) {
            Ok(()) => Ok(vector),
            Err(e) => {
                let _ = table.deregister(vector, owner);
                Err(e)
            }
        }
    }

    /// As [`request_gsi`](Self::request_gsi) for a legacy ISA IRQ, applying the MADT's
    /// interrupt source overrides (remapped GSI, polarity, trigger).
    #[allow(clippy::too_many_arguments)]
    pub fn request_isa_irq(
        &mut self,
        table: &IrqTable,
        madt: &Madt,
        irq: u8,
        owner: Owner,
        handler: Handler,
        cookie: usize,
        dest: u8,
    ) -> Result<u8, RouteError> {
        let r = madt.isa_route(irq);
        self.request_gsi(
            table, owner, handler, cookie, r.gsi, dest, r.polarity, r.trigger,
        )
    }

    pub fn gsi_of_vector(&self, vector: u8) -> Option<u32> {
        let g = self.vec_gsi[vector as usize];
        (g != NO_GSI).then_some(g)
    }

    fn pin_of_vector(&mut self, vector: u8) -> Result<(&mut IoApic<M>, u32), ControllerError> {
        let gsi = self
            .gsi_of_vector(vector)
            .ok_or(ControllerError::InvalidVector)?;
        self.find(gsi).ok_or(ControllerError::InvalidVector)
    }
}

impl<R: ApicRegs, M: Mmio> InterruptController for ApicController<R, M> {
    fn name(&self) -> &'static str {
        "APIC"
    }
    fn is_spurious(&mut self, vector: u8) -> bool {
        InterruptController::is_spurious(&mut self.lapic, vector)
    }
    fn eoi(&mut self, vector: u8) {
        InterruptController::eoi(&mut self.lapic, vector)
    }
    fn mask(&mut self, vector: u8) -> Result<(), ControllerError> {
        let (io, pin) = self.pin_of_vector(vector)?;
        io.mask_pin(pin).map_err(|_| ControllerError::InvalidVector)
    }
    fn unmask(&mut self, vector: u8) -> Result<(), ControllerError> {
        let (io, pin) = self.pin_of_vector(vector)?;
        io.unmask_pin(pin)
            .map_err(|_| ControllerError::InvalidVector)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apic::fakes::*;
    use crate::apic::{REG_EOI, REG_VERSION, SVR_ENABLE, XApicRegs};
    use crate::irq::{self, HandlerResult, OWNER_TEST, OWNER_TIMER, Outcome, SPURIOUS_VECTOR};
    use std::collections::BTreeMap;

    /// Model of an IOAPIC register block: IOREGSEL/IOWIN indirection, ID, VER and a redirection
    /// table whose read-only bits (delivery status, remote IRR) ignore writes.
    pub struct ModelIo {
        sel: u32,
        id: u32,
        ver: u32,
        rt: Vec<u64>,
        pub log: Vec<(u8, u32)>, // register index, value written to the window
        pub window_writes_without_select: u32,
        selected_since_write: bool,
        pub ro: BTreeMap<u32, u64>, // pin -> read-only bits forced on read
    }
    impl ModelIo {
        pub fn new(pins: u32) -> Self {
            Self {
                sel: 0,
                id: 2 << 24,
                ver: 0x11 | (pins - 1) << 16,
                rt: vec![RTE_MASK as u64; pins as usize], // reset state: masked
                log: Vec::new(),
                window_writes_without_select: 0,
                selected_since_write: false,
                ro: BTreeMap::new(),
            }
        }
        pub fn rte(&self, pin: usize) -> Rte {
            Rte::decode(self.rt[pin] as u32, (self.rt[pin] >> 32) as u32)
        }
        pub fn absent() -> Self {
            let mut m = Self::new(24);
            m.ver = 0xFFFF_FFFF;
            m
        }
    }
    impl Mmio for ModelIo {
        fn read32(&mut self, off: usize) -> u32 {
            match off {
                REG_SELECT => self.sel,
                REG_WINDOW => match self.sel as u8 {
                    IOAPIC_ID => self.id,
                    IOAPIC_VER => self.ver,
                    r if r >= IOAPIC_REDTBL
                        && ((r - IOAPIC_REDTBL) / 2) as usize <= self.rt.len() =>
                    {
                        let pin = ((r - IOAPIC_REDTBL) / 2) as usize;
                        let v = self.rt[pin] | *self.ro.get(&(pin as u32)).unwrap_or(&0);
                        if (r - IOAPIC_REDTBL) % 2 == 0 {
                            v as u32
                        } else {
                            (v >> 32) as u32
                        }
                    }
                    _ => 0,
                },
                _ => panic!("bad ioapic read offset {off:#x}"),
            }
        }
        fn write32(&mut self, off: usize, v: u32) {
            match off {
                REG_SELECT => {
                    self.sel = v;
                    self.selected_since_write = true;
                }
                REG_WINDOW => {
                    if !self.selected_since_write {
                        self.window_writes_without_select += 1;
                    }
                    self.selected_since_write = false;
                    let r = self.sel as u8;
                    self.log.push((r, v));
                    if r >= IOAPIC_REDTBL {
                        let pin = ((r - IOAPIC_REDTBL) / 2) as usize;
                        assert!(pin < self.rt.len(), "write past the redirection table");
                        let ro_lo = (RTE_DELIVERY_STATUS | RTE_REMOTE_IRR) as u64;
                        if (r - IOAPIC_REDTBL) % 2 == 0 {
                            self.rt[pin] = (self.rt[pin] & !0xFFFF_FFFF) | (v as u64 & !ro_lo);
                        } else {
                            self.rt[pin] = (self.rt[pin] & 0xFFFF_FFFF) | (v as u64) << 32;
                        }
                    }
                }
                _ => panic!("bad ioapic write offset {off:#x}"),
            }
        }
        fn read64(&mut self, _: usize) -> u64 {
            panic!("IOAPIC registers are 32-bit")
        }
        fn write64(&mut self, _: usize, _: u64) {
            panic!("IOAPIC registers are 32-bit")
        }
    }

    fn handler(_: u8, _: usize) -> HandlerResult {
        HandlerResult::Handled
    }

    fn lapic() -> LocalApic<XApicRegs<FakeMmio>> {
        let mut a = LocalApic::new(xapic(0x0006_0014));
        a.init(SPURIOUS_VECTOR).unwrap();
        a
    }

    fn controller(ios: &[(u32, u32)]) -> ApicController<XApicRegs<FakeMmio>, ModelIo> {
        let mut c = ApicController::new(lapic());
        for (i, (base, pins)) in ios.iter().enumerate() {
            c.add_ioapic(IoApic::new(ModelIo::new(*pins), i as u8, *base).unwrap())
                .unwrap();
        }
        c
    }

    #[test]
    fn rte_encoding_golden_values_and_roundtrip() {
        let r = Rte::fixed(0x30, 1, Polarity::ActiveLow, Trigger::Level, true);
        assert_eq!(r.encode(), (0x0001_A030, 0x0100_0000));
        let r = Rte::fixed(0x41, 0xFE, Polarity::ActiveHigh, Trigger::Edge, false);
        assert_eq!(r.encode(), (0x41, 0xFE00_0000));
        assert_eq!(Rte::decode(0x41, 0xFE00_0000), r);
        for v in [0x20u8, 0x30, 0xEF] {
            for (p, t) in [
                (Polarity::ActiveHigh, Trigger::Edge),
                (Polarity::ActiveLow, Trigger::Level),
                (Polarity::ActiveHigh, Trigger::Level),
                (Polarity::ActiveLow, Trigger::Edge),
            ] {
                for masked in [false, true] {
                    let r = Rte::fixed(v, 3, p, t, masked);
                    let (lo, hi) = r.encode();
                    assert_eq!(Rte::decode(lo, hi), r);
                }
            }
        }
        // Read-only bits decode but are never produced by encode.
        let d = Rte::decode(RTE_DELIVERY_STATUS | RTE_REMOTE_IRR | 0x50, 0);
        assert!(d.delivery_pending && d.remote_irr);
        let (lo, _) = d.encode();
        assert_eq!(lo & (RTE_DELIVERY_STATUS | RTE_REMOTE_IRR), 0);
        assert_eq!(Delivery::decode(0b011), Delivery::Reserved(3));
        assert_eq!(Delivery::decode(0b100), Delivery::Nmi);
    }

    #[test]
    fn probing_reads_version_and_derives_the_pin_count() {
        let io = IoApic::new(ModelIo::new(24), 0, 0).unwrap();
        assert_eq!((io.pins(), io.version(), io.gsi_end()), (24, 0x11, 24));
        let io = IoApic::new(ModelIo::new(1), 0, 100).unwrap();
        assert_eq!((io.pins(), io.gsi_base(), io.gsi_end()), (1, 100, 101));
        assert!(matches!(
            IoApic::new(ModelIo::absent(), 0, 0),
            Err(IoApicError::NoDevice)
        ));
        let mut zero = ModelIo::new(24);
        zero.ver = 0;
        assert!(matches!(
            IoApic::new(zero, 0, 0),
            Err(IoApicError::NoDevice)
        ));
        let mut old = ModelIo::new(24);
        old.ver = 0x05 | 23 << 16;
        assert!(matches!(
            IoApic::new(old, 0, 0),
            Err(IoApicError::BadPinCount)
        ));
        let mut huge = ModelIo::new(24);
        huge.ver = 0x11 | 0xF0 << 16; // 241 pins
        assert!(matches!(
            IoApic::new(huge, 0, 0),
            Err(IoApicError::BadPinCount)
        ));
        assert!(matches!(
            IoApic::new(ModelIo::new(24), 0, u32::MAX - 5),
            Err(IoApicError::GsiOverflow)
        ));
        let mut io = IoApic::new(ModelIo::new(24), 0, 0).unwrap();
        assert_eq!(io.hw_id(), 2);
    }

    #[test]
    fn programming_masks_first_and_ends_with_the_requested_state() {
        let mut io = IoApic::new(ModelIo::new(24), 0, 0).unwrap();
        io.mmio_mut().log.clear();
        io.write_rte(
            5,
            &Rte::fixed(0x33, 2, Polarity::ActiveLow, Trigger::Level, false),
        )
        .unwrap();
        let log = io.mmio_mut().log.clone();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].0, 0x10 + 10, "low dword first, masked");
        assert_ne!(log[0].1 & RTE_MASK, 0);
        assert_eq!(log[1], (0x10 + 11, 2 << 24), "then the destination");
        assert_eq!(log[2].0, 0x10 + 10);
        assert_eq!(log[2].1 & RTE_MASK, 0, "final write unmasked as requested");
        assert_eq!(log[2].1 & 0xFF, 0x33);
        assert_eq!(io.mmio_mut().window_writes_without_select, 0);
        let r = io.read_rte(5).unwrap();
        assert_eq!(
            (r.vector, r.dest, r.polarity, r.trigger, r.masked),
            (0x33, 2, Polarity::ActiveLow, Trigger::Level, false)
        );
        assert_eq!(io.read_rte(24), Err(IoApicError::InvalidPin));
        assert_eq!(io.write_rte(24, &r), Err(IoApicError::InvalidPin));
        assert_eq!(io.mask_pin(100), Err(IoApicError::InvalidPin));
    }

    #[test]
    fn mask_and_unmask_preserve_other_fields_and_read_only_bits_are_not_corrupted() {
        let mut io = IoApic::new(ModelIo::new(24), 0, 0).unwrap();
        io.write_rte(
            3,
            &Rte::fixed(0x44, 7, Polarity::ActiveLow, Trigger::Level, false),
        )
        .unwrap();
        io.mmio_mut()
            .ro
            .insert(3, (RTE_REMOTE_IRR | RTE_DELIVERY_STATUS) as u64);
        io.mask_pin(3).unwrap();
        let r = io.read_rte(3).unwrap();
        assert!(r.masked);
        assert_eq!(
            (r.vector, r.dest, r.polarity, r.trigger),
            (0x44, 7, Polarity::ActiveLow, Trigger::Level)
        );
        io.unmask_pin(3).unwrap();
        assert!(!io.read_rte(3).unwrap().masked);
    }

    #[test]
    fn controller_refuses_overlapping_ioapics_and_enforces_the_chip_limit() {
        let mut c = controller(&[(0, 24)]);
        let overlapping = IoApic::new(ModelIo::new(24), 1, 20).unwrap();
        assert_eq!(c.add_ioapic(overlapping), Err(RouteError::Overlap));
        let before = IoApic::new(ModelIo::new(8), 1, 16).unwrap();
        assert_eq!(
            c.add_ioapic(before),
            Err(RouteError::Overlap),
            "16..24 is inside 0..24"
        );
        let adjacent = IoApic::new(ModelIo::new(24), 1, 24).unwrap();
        assert!(c.add_ioapic(adjacent).is_ok());
        let mut full = controller(&[]);
        for i in 0..MAX_IOAPICS as u32 {
            full.add_ioapic(IoApic::new(ModelIo::new(1), i as u8, i).unwrap())
                .unwrap();
        }
        let extra = IoApic::new(ModelIo::new(1), 9, 100).unwrap();
        assert_eq!(full.add_ioapic(extra), Err(RouteError::TooManyIoApics));
    }

    #[test]
    fn routing_programs_a_masked_entry_and_tracks_the_mapping() {
        let mut c = controller(&[(0, 24), (24, 8)]);
        c.route_gsi(26, 0x45, 0, Polarity::ActiveLow, Trigger::Level)
            .unwrap();
        assert_eq!(c.gsi_of_vector(0x45), Some(26));
        let r = c.ioapic(1).unwrap().read_rte(2).unwrap();
        assert_eq!(
            (r.vector, r.masked, r.polarity, r.trigger),
            (0x45, true, Polarity::ActiveLow, Trigger::Level)
        );
        // The first chip was never written.
        assert!(c.ioapic(0).unwrap().mmio_mut().log.is_empty());
        InterruptController::unmask(&mut c, 0x45).unwrap();
        assert!(!c.ioapic(1).unwrap().read_rte(2).unwrap().masked);
        InterruptController::mask(&mut c, 0x45).unwrap();
        assert!(c.ioapic(1).unwrap().read_rte(2).unwrap().masked);
    }

    #[test]
    fn routing_validation() {
        let mut c = controller(&[(0, 24)]);
        let (p, t) = (Polarity::ActiveHigh, Trigger::Edge);
        for bad in [0u8, 3, 0x1F, 0xF0, 0xFE, SPURIOUS_VECTOR] {
            assert_eq!(
                c.route_gsi(1, bad, 0, p, t),
                Err(RouteError::VectorNotAllowed),
                "{bad:#x}"
            );
        }
        assert_eq!(c.route_gsi(24, 0x30, 0, p, t), Err(RouteError::NoIoApic));
        assert_eq!(
            c.route_gsi(u32::MAX, 0x30, 0, p, t),
            Err(RouteError::NoIoApic)
        );
        assert_eq!(
            c.route_gsi(1, 0x30, 0xFF, p, t),
            Err(RouteError::InvalidDest)
        );
        c.route_gsi(1, 0x30, 0, p, t).unwrap();
        assert_eq!(
            c.route_gsi(1, 0x31, 0, p, t),
            Err(RouteError::AlreadyRouted)
        );
        assert_eq!(c.route_gsi(2, 0x30, 0, p, t), Err(RouteError::VectorInUse));
        // Failed routes wrote nothing and left no mapping.
        assert_eq!(c.gsi_of_vector(0x31), None);
        let untouched = c.ioapic(0).unwrap().read_rte(2).unwrap();
        assert_eq!(
            (untouched.vector, untouched.masked),
            (0, true),
            "refused routes write nothing"
        );
        assert_eq!(
            InterruptController::mask(&mut c, 0x77),
            Err(ControllerError::InvalidVector)
        );
    }

    #[test]
    fn unroute_masks_and_frees_both_the_gsi_and_the_vector() {
        let mut c = controller(&[(0, 24)]);
        let (p, t) = (Polarity::ActiveHigh, Trigger::Edge);
        c.route_gsi(4, 0x50, 0, p, t).unwrap();
        InterruptController::unmask(&mut c, 0x50).unwrap();
        c.unroute_gsi(4).unwrap();
        assert!(c.ioapic(0).unwrap().read_rte(4).unwrap().masked);
        assert_eq!(c.gsi_of_vector(0x50), None);
        assert_eq!(c.unroute_gsi(4), Err(RouteError::NotRouted));
        c.route_gsi(4, 0x50, 0, p, t).unwrap(); // reusable
    }

    #[test]
    fn request_gsi_registers_routes_and_rolls_back_on_failure() {
        let t = irq::IrqTable::new();
        let mut c = controller(&[(0, 24)]);
        let v = c
            .request_gsi(
                &t,
                OWNER_TIMER,
                handler,
                7,
                3,
                0,
                Polarity::ActiveHigh,
                Trigger::Edge,
            )
            .unwrap();
        assert_eq!(v, irq::DYNAMIC_FIRST);
        assert_eq!(t.owner_of(v), Some(OWNER_TIMER));
        // GSI with no IOAPIC: the vector it just claimed must be released again.
        let before = t.owner_of(irq::DYNAMIC_FIRST + 1);
        assert_eq!(
            c.request_gsi(
                &t,
                OWNER_TEST,
                handler,
                0,
                99,
                0,
                Polarity::ActiveHigh,
                Trigger::Edge
            ),
            Err(RouteError::NoIoApic)
        );
        assert_eq!(t.owner_of(irq::DYNAMIC_FIRST + 1), before);
        assert!(before.is_none());
        // Same GSI twice: second fails, first stays intact.
        assert_eq!(
            c.request_gsi(
                &t,
                OWNER_TEST,
                handler,
                0,
                3,
                0,
                Polarity::ActiveHigh,
                Trigger::Edge
            ),
            Err(RouteError::AlreadyRouted)
        );
        assert_eq!(t.owner_of(v), Some(OWNER_TIMER));
        assert_eq!(t.owner_of(v + 1), None);
    }

    #[test]
    fn isa_irq_routing_follows_madt_overrides() {
        use crate::acpi::testutil::*;
        // MADT: IOAPIC at GSI 0, timer IRQ0 -> GSI2, SCI IRQ9 active-low level.
        let mut body = vec![];
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&[1, 12, 0, 0, 0, 0, 0xC0, 0xFE, 0, 0, 0, 0]);
        body.extend_from_slice(&[2, 10, 0, 0, 2, 0, 0, 0, 0, 0]);
        body.extend_from_slice(&[2, 10, 0, 9, 9, 0, 0, 0, 0x0F, 0]);
        let mut mem = FakeMem::default();
        mem.put(0x4000, &sdt(b"APIC", 5, &body));
        let madt = crate::acpi::madt::parse_madt(&mem, 0x4000).unwrap();

        let t = irq::IrqTable::new();
        let mut c = controller(&[(0, 24)]);
        let v0 = c
            .request_isa_irq(&t, &madt, 0, OWNER_TIMER, handler, 0, 0)
            .unwrap();
        assert_eq!(c.gsi_of_vector(v0), Some(2), "IRQ0 lands on GSI 2");
        let r = c.ioapic(0).unwrap().read_rte(2).unwrap();
        assert_eq!(
            (r.polarity, r.trigger, r.vector),
            (Polarity::ActiveHigh, Trigger::Edge, v0)
        );
        let v9 = c
            .request_isa_irq(&t, &madt, 9, OWNER_TEST, handler, 0, 0)
            .unwrap();
        let r = c.ioapic(0).unwrap().read_rte(9).unwrap();
        assert_eq!(
            (r.polarity, r.trigger, r.vector),
            (Polarity::ActiveLow, Trigger::Level, v9)
        );
        // Plain identity route for an un-overridden IRQ.
        let v4 = c
            .request_isa_irq(&t, &madt, 4, OWNER_TEST, handler, 0, 0)
            .unwrap();
        assert_eq!(c.gsi_of_vector(v4), Some(4));
    }

    #[test]
    fn add_from_madt_probes_each_entry_and_reports_unmapped_windows() {
        use crate::acpi::testutil::*;
        let mut body = vec![];
        body.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&[1, 12, 0, 0, 0, 0, 0xC0, 0xFE, 0, 0, 0, 0]);
        body.extend_from_slice(&[1, 12, 1, 0, 0, 0, 0xC1, 0xFE, 24, 0, 0, 0]);
        let mut mem = FakeMem::default();
        mem.put(0x4000, &sdt(b"APIC", 5, &body));
        let madt = crate::acpi::madt::parse_madt(&mem, 0x4000).unwrap();
        let mut c = controller(&[]);
        let mut seen = vec![];
        c.add_from_madt(&madt, |e| {
            seen.push((e.id, e.address, e.gsi_base));
            Some(ModelIo::new(24))
        })
        .unwrap();
        assert_eq!(seen, vec![(0, 0xFEC0_0000, 0), (1, 0xFEC1_0000, 24)]);
        assert_eq!(c.ioapic_count(), 2);
        // The caller declines to provide a window (not mapped): reported, nothing added.
        let mut c = controller(&[]);
        assert_eq!(c.add_from_madt(&madt, |_| None), Err(RouteError::NoIoApic));
        assert_eq!(c.ioapic_count(), 0);
        // Absent hardware behind a window.
        let mut c = controller(&[]);
        assert_eq!(
            c.add_from_madt(&madt, |_| Some(ModelIo::absent())),
            Err(RouteError::Chip(IoApicError::NoDevice))
        );
    }

    #[test]
    fn dispatch_end_to_end_acks_the_local_apic_and_never_acks_spurious() {
        let t = irq::IrqTable::new();
        let mut c = controller(&[(0, 24)]);
        let v = c
            .request_gsi(
                &t,
                OWNER_TEST,
                handler,
                0,
                5,
                0,
                Polarity::ActiveHigh,
                Trigger::Edge,
            )
            .unwrap();
        InterruptController::unmask(&mut c, v).unwrap();
        c.lapic_mut().regs_mut().0.log.clear();
        assert_eq!(irq::dispatch(&t, &mut c, v), Outcome::Handled);
        assert_eq!(
            c.lapic_mut().regs_mut().0.writes(),
            vec![(REG_EOI as usize, 0)]
        );
        c.lapic_mut().regs_mut().0.log.clear();
        assert_eq!(
            irq::dispatch(&t, &mut c, SPURIOUS_VECTOR),
            Outcome::Spurious
        );
        assert!(c.lapic_mut().regs_mut().0.writes().is_empty());
        assert_eq!(
            c.lapic_mut().regs_mut().0.get(REG_VERSION as usize),
            0x0006_0014
        );
        assert_ne!(
            c.lapic_mut()
                .regs_mut()
                .0
                .get(crate::apic::REG_SVR as usize) as u32
                & SVR_ENABLE,
            0
        );
    }

    #[test]
    fn an_unhandled_routed_line_is_masked_at_the_ioapic_after_the_threshold() {
        let t = irq::IrqTable::new();
        let mut c = controller(&[(0, 24)]);
        let v = c
            .request_gsi(
                &t,
                OWNER_TEST,
                handler,
                0,
                6,
                0,
                Polarity::ActiveLow,
                Trigger::Level,
            )
            .unwrap();
        InterruptController::unmask(&mut c, v).unwrap();
        t.deregister(v, OWNER_TEST).unwrap(); // device keeps firing, nobody listens
        for _ in 0..irq::UNHANDLED_MASK_THRESHOLD {
            assert_eq!(irq::dispatch(&t, &mut c, v), Outcome::Unhandled);
        }
        assert!(
            c.ioapic(0).unwrap().read_rte(6).unwrap().masked,
            "storm contained at the source"
        );
    }
}
