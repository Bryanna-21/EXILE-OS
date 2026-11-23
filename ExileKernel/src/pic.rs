//! Legacy 8259 PIC pair (Mission C3.2). SPDX-License-Identifier: Apache-2.0
//!
//! Usable as the fallback interrupt controller. All port I/O goes through [`PortIo`], so the
//! protocol is host-tested against a behavioural model of two cascaded 8259s and no privileged
//! instruction runs on the host. `HwPorts` is the real `in`/`out` implementation.
//!
//! Initialisation order the caller must respect (checked where it can be, documented where not):
//! 1. interrupts disabled (`cli`) and the IDT loaded and complete, because the remap moves IRQs
//!    to vectors that must already have gates;
//! 2. `Pic::new` (validates the base), then `init`: every line ends up masked except the
//!    cascade (IRQ2), so nothing fires until a driver unmasks its own line;
//! 3. drivers register handlers (`irq.rs`) and only then `unmask_irq` their line;
//! 4. only then `sti`.
//!
//! Remap: IRQ0..7 -> `base`..`base+7`, IRQ8..15 -> `base+8`..`base+15`. The default base is
//! `irq::LEGACY_BASE` (0x20), clear of the CPU exception vectors (0..31) which the BIOS default
//! mapping (0x08..0x0F) would collide with.
//!
//! Spurious interrupts: when an IRQ is withdrawn between INTR and INTA, the PIC delivers its
//! lowest line (IRQ7 on the master, IRQ15 on the slave) with the in-service bit *clear*. Those
//! are detected by reading ISR (OCW3). A spurious IRQ7 gets no EOI; a spurious IRQ15 gets an EOI
//! to the master only (the master did see the cascade line), never to the slave.
#![allow(dead_code)]
#![allow(
    clippy::manual_is_multiple_of,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::identity_op,
    clippy::unusual_byte_groupings
)]

use crate::irq::{ControllerError, InterruptController, LEGACY_BASE};

pub const MASTER_CMD: u16 = 0x20;
pub const MASTER_DATA: u16 = 0x21;
pub const SLAVE_CMD: u16 = 0xA0;
pub const SLAVE_DATA: u16 = 0xA1;
/// Unused port; a write to it takes about a microsecond on real hardware (classic `io_wait`).
pub const IO_WAIT_PORT: u16 = 0x80;

const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;
const OCW2_EOI: u8 = 0x20;
const OCW3_READ_IRR: u8 = 0x0A;
const OCW3_READ_ISR: u8 = 0x0B;
/// IRQ2 on the master is wired to the slave's INT output.
pub const CASCADE_IRQ: u8 = 2;
pub const IRQ_COUNT: u8 = 16;

/// Byte port I/O. `&mut self` because a port read can have side effects.
pub trait PortIo {
    fn outb(&mut self, port: u16, value: u8);
    fn inb(&mut self, port: u16) -> u8;
    fn io_wait(&mut self) {
        self.outb(IO_WAIT_PORT, 0);
    }
}

/// The real thing.
#[cfg(target_arch = "x86_64")]
pub struct HwPorts(());

#[cfg(target_arch = "x86_64")]
impl HwPorts {
    /// # Safety
    /// Ring 0 (or an I/O permission bitmap that allows the ports used). The caller guarantees
    /// nothing else drives these ports concurrently.
    pub const unsafe fn new() -> Self {
        Self(())
    }
}

#[cfg(target_arch = "x86_64")]
impl PortIo for HwPorts {
    fn outb(&mut self, port: u16, value: u8) {
        // SAFETY: `HwPorts::new` required ring 0 and exclusive use of the ports.
        unsafe {
            core::arch::asm!("out dx, al", in("dx") port, in("al") value,
                options(nomem, nostack, preserves_flags));
        }
    }
    fn inb(&mut self, port: u16) -> u8 {
        let v: u8;
        // SAFETY: as above.
        unsafe {
            core::arch::asm!("in al, dx", in("dx") port, out("al") v,
                options(nomem, nostack, preserves_flags));
        }
        v
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PicError {
    /// Base must be a multiple of 8, at least 32 (above the exceptions) and at most 0xF0.
    InvalidBase,
    InvalidIrq,
    /// IRQ2 is the cascade; masking it would silence the whole slave.
    CascadeLine,
    NotInitialized,
    AlreadyInitialized,
}

pub struct Pic<P: PortIo> {
    io: P,
    base: u8,
    mask: u16,
    initialized: bool,
    spurious_master: u64,
    spurious_slave: u64,
}

impl<P: PortIo> Pic<P> {
    pub fn new(io: P, base: u8) -> Result<Self, PicError> {
        if base < 32 || base % 8 != 0 || base > 0xF0 {
            return Err(PicError::InvalidBase);
        }
        Ok(Self {
            io,
            base,
            mask: 0xFFFF,
            initialized: false,
            spurious_master: 0,
            spurious_slave: 0,
        })
    }

    pub fn with_default_base(io: P) -> Self {
        // LEGACY_BASE is a compile-time constant satisfying `new`.
        match Self::new(io, LEGACY_BASE) {
            Ok(p) => p,
            Err(_) => unreachable!(),
        }
    }

    pub fn base(&self) -> u8 {
        self.base
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Cached mask, bit n set = IRQ n masked.
    pub fn masks(&self) -> u16 {
        self.mask
    }

    pub fn spurious_counts(&self) -> (u64, u64) {
        (self.spurious_master, self.spurious_slave)
    }

    /// Remaps both PICs and leaves every line masked except the cascade. Interrupts must be
    /// disabled by the caller (module docs). Re-running is refused so a live configuration is
    /// never reprogrammed by accident.
    pub fn init(&mut self) -> Result<(), PicError> {
        if self.initialized {
            return Err(PicError::AlreadyInitialized);
        }
        // Mask everything first so nothing is delivered mid-sequence.
        self.io.outb(MASTER_DATA, 0xFF);
        self.io.outb(SLAVE_DATA, 0xFF);
        // ICW1: begin initialisation, edge triggered, cascade mode, ICW4 follows.
        self.io.outb(MASTER_CMD, ICW1_INIT | ICW1_ICW4);
        self.io.io_wait();
        self.io.outb(SLAVE_CMD, ICW1_INIT | ICW1_ICW4);
        self.io.io_wait();
        // ICW2: vector offsets.
        self.io.outb(MASTER_DATA, self.base);
        self.io.io_wait();
        self.io.outb(SLAVE_DATA, self.base + 8);
        self.io.io_wait();
        // ICW3: master has a slave on IRQ2 (bit mask); slave's cascade identity is 2.
        self.io.outb(MASTER_DATA, 1 << CASCADE_IRQ);
        self.io.io_wait();
        self.io.outb(SLAVE_DATA, CASCADE_IRQ);
        self.io.io_wait();
        // ICW4: 8086 mode, normal (non-auto) EOI.
        self.io.outb(MASTER_DATA, ICW4_8086);
        self.io.io_wait();
        self.io.outb(SLAVE_DATA, ICW4_8086);
        self.io.io_wait();
        // OCW1: everything masked except the cascade.
        self.mask = 0xFFFF & !(1 << CASCADE_IRQ);
        self.write_masks();
        self.initialized = true;
        Ok(())
    }

    fn write_masks(&mut self) {
        self.io.outb(MASTER_DATA, self.mask as u8);
        self.io.outb(SLAVE_DATA, (self.mask >> 8) as u8);
    }

    fn check(&self, irq: u8) -> Result<(), PicError> {
        if !self.initialized {
            return Err(PicError::NotInitialized);
        }
        if irq >= IRQ_COUNT {
            return Err(PicError::InvalidIrq);
        }
        Ok(())
    }

    pub fn mask_irq(&mut self, irq: u8) -> Result<(), PicError> {
        self.check(irq)?;
        if irq == CASCADE_IRQ {
            return Err(PicError::CascadeLine);
        }
        self.mask |= 1 << irq;
        self.write_masks();
        Ok(())
    }

    pub fn unmask_irq(&mut self, irq: u8) -> Result<(), PicError> {
        self.check(irq)?;
        self.mask &= !(1 << irq);
        self.write_masks();
        Ok(())
    }

    /// Masks all 16 lines including the cascade. Use before handing interrupt delivery to the
    /// APIC, or at shutdown. The PIC stays remapped, so a late spurious interrupt lands on a
    /// vector that has a gate.
    pub fn disable_all(&mut self) -> Result<(), PicError> {
        if !self.initialized {
            return Err(PicError::NotInitialized);
        }
        self.mask = 0xFFFF;
        self.write_masks();
        Ok(())
    }

    fn read_reg(&mut self, ocw3: u8) -> u16 {
        self.io.outb(MASTER_CMD, ocw3);
        self.io.outb(SLAVE_CMD, ocw3);
        let m = self.io.inb(MASTER_CMD) as u16;
        let s = self.io.inb(SLAVE_CMD) as u16;
        m | (s << 8)
    }

    /// In-service register, both chips (slave in the high byte).
    pub fn read_isr(&mut self) -> u16 {
        self.read_reg(OCW3_READ_ISR)
    }

    pub fn read_irr(&mut self) -> u16 {
        self.read_reg(OCW3_READ_IRR)
    }

    /// True if `irq` is IRQ7/IRQ15 delivered with its in-service bit clear. IRQ15 additionally
    /// gets the master-only EOI described in the module docs. Any other IRQ is never spurious.
    pub fn check_spurious_irq(&mut self, irq: u8) -> bool {
        match irq {
            7 => {
                if self.read_isr() & (1 << 7) == 0 {
                    self.spurious_master += 1;
                    true
                } else {
                    false
                }
            }
            15 => {
                if self.read_isr() & (1 << 15) == 0 {
                    self.spurious_slave += 1;
                    // The master latched the cascade line; release it, but leave the slave alone.
                    self.io.outb(MASTER_CMD, OCW2_EOI);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Non-specific EOI. IRQ8..15 acknowledge the slave first, then the master (for the
    /// cascade line).
    pub fn eoi_irq(&mut self, irq: u8) {
        if !self.initialized || irq >= IRQ_COUNT {
            return;
        }
        if irq >= 8 {
            self.io.outb(SLAVE_CMD, OCW2_EOI);
        }
        self.io.outb(MASTER_CMD, OCW2_EOI);
    }

    fn irq_of(&self, vector: u8) -> Option<u8> {
        let off = vector.checked_sub(self.base)?;
        (off < IRQ_COUNT).then_some(off)
    }

    #[cfg(test)]
    pub(crate) fn io_for_test(&mut self) -> &mut P {
        &mut self.io
    }

    /// Releases the ports (for tests and handover).
    pub fn into_io(self) -> P {
        self.io
    }
}

impl<P: PortIo> InterruptController for Pic<P> {
    fn name(&self) -> &'static str {
        "8259 PIC"
    }

    fn is_spurious(&mut self, vector: u8) -> bool {
        match self.irq_of(vector) {
            Some(irq) if self.initialized => self.check_spurious_irq(irq),
            _ => false,
        }
    }

    fn eoi(&mut self, vector: u8) {
        if let Some(irq) = self.irq_of(vector) {
            self.eoi_irq(irq);
        }
    }

    fn mask(&mut self, vector: u8) -> Result<(), ControllerError> {
        let irq = self.irq_of(vector).ok_or(ControllerError::InvalidVector)?;
        self.mask_irq(irq).map_err(|e| match e {
            PicError::NotInitialized => ControllerError::NotInitialized,
            PicError::CascadeLine => ControllerError::Unsupported,
            _ => ControllerError::InvalidVector,
        })
    }

    fn unmask(&mut self, vector: u8) -> Result<(), ControllerError> {
        let irq = self.irq_of(vector).ok_or(ControllerError::InvalidVector)?;
        self.unmask_irq(irq).map_err(|e| match e {
            PicError::NotInitialized => ControllerError::NotInitialized,
            _ => ControllerError::InvalidVector,
        })
    }
}

#[cfg(test)]
pub(crate) mod model {
    //! Behavioural model of two cascaded 8259s, precise enough to validate the init sequence
    //! and the EOI/ISR protocol. It records every port write.
    use super::*;

    #[derive(Default, Clone)]
    pub struct Chip {
        pub step: u8, // 0 = idle, 2 = expect ICW2, 3 = ICW3, 4 = ICW4
        pub need_icw4: bool,
        pub base: u8,
        pub icw3: u8,
        pub icw4: u8,
        pub imr: u8,
        pub isr: u8,
        pub irr: u8,
        pub read_isr: bool,
        pub icw1_count: u32,
        pub nonspecific_eois: u32,
    }

    #[derive(Default)]
    pub struct Model {
        pub chips: [Chip; 2],
        pub log: Vec<(u16, u8, bool)>, // port, value, was_write
        pub waits: u32,
    }

    impl Model {
        fn chip(port: u16) -> (usize, bool) {
            match port {
                MASTER_CMD => (0, true),
                MASTER_DATA => (0, false),
                SLAVE_CMD => (1, true),
                SLAVE_DATA => (1, false),
                _ => panic!("unexpected port {port:#x}"),
            }
        }
    }

    impl PortIo for Model {
        fn outb(&mut self, port: u16, v: u8) {
            if port == IO_WAIT_PORT {
                self.waits += 1;
                return;
            }
            self.log.push((port, v, true));
            let (i, is_cmd) = Self::chip(port);
            let c = &mut self.chips[i];
            if is_cmd {
                if v & 0x10 != 0 {
                    // ICW1: resets the IMR and in-service state, starts the sequence.
                    c.need_icw4 = v & 1 != 0;
                    c.imr = 0;
                    c.isr = 0;
                    c.step = 2;
                    c.icw1_count += 1;
                } else if v & 0x08 != 0 {
                    // OCW3
                    if v & 0x02 != 0 {
                        c.read_isr = v & 1 != 0;
                    }
                } else if v & 0xE0 == 0x20 {
                    // Non-specific EOI: clear the highest-priority in-service bit.
                    c.nonspecific_eois += 1;
                    if c.isr != 0 {
                        c.isr &= !(1 << c.isr.trailing_zeros());
                    }
                }
            } else {
                match c.step {
                    2 => {
                        c.base = v;
                        c.step = 3;
                    }
                    3 => {
                        c.icw3 = v;
                        c.step = if c.need_icw4 { 4 } else { 0 };
                    }
                    4 => {
                        c.icw4 = v;
                        c.step = 0;
                    }
                    _ => c.imr = v,
                }
            }
        }
        fn inb(&mut self, port: u16) -> u8 {
            self.log.push((port, 0, false));
            let (i, is_cmd) = Self::chip(port);
            let c = &self.chips[i];
            if is_cmd {
                if c.read_isr { c.isr } else { c.irr }
            } else {
                c.imr
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::model::Model;
    use super::*;
    use crate::irq::{self, IrqTable, OWNER_TEST, Outcome};

    fn ready() -> Pic<Model> {
        let mut p = Pic::with_default_base(Model::default());
        p.init().unwrap();
        p
    }

    #[test]
    fn base_validation() {
        for bad in [0u8, 8, 0x10, 0x18, 0x21, 0x2C, 0xF8, 0xFF] {
            assert_eq!(
                Pic::new(Model::default(), bad).err(),
                Some(PicError::InvalidBase),
                "{bad:#x}"
            );
        }
        for ok in [0x20u8, 0x28, 0x30, 0x70, 0xF0] {
            assert!(Pic::new(Model::default(), ok).is_ok(), "{ok:#x}");
        }
    }

    #[test]
    fn init_sequence_leaves_the_documented_state() {
        let p = ready();
        assert_eq!(p.masks(), 0xFFFB);
        assert!(p.is_initialized());
        let m = p.into_io();
        let (ms, sl) = (&m.chips[0], &m.chips[1]);
        assert_eq!(
            (ms.base, sl.base),
            (0x20, 0x28),
            "remapped above the exceptions"
        );
        assert_eq!((ms.icw3, sl.icw3), (0x04, 0x02), "cascade on IRQ2");
        assert_eq!((ms.icw4, sl.icw4), (1, 1), "8086 mode");
        assert_eq!((ms.step, sl.step), (0, 0), "sequence completed");
        assert_eq!((ms.imr, sl.imr), (0xFB, 0xFF), "only the cascade is open");
        assert_eq!((ms.icw1_count, sl.icw1_count), (1, 1));
        assert!(m.waits >= 8, "io_wait between ICWs");
    }

    #[test]
    fn nothing_is_unmasked_at_any_point_during_init() {
        let p = ready();
        let m = p.into_io();
        // The very first writes mask both chips, before ICW1.
        assert_eq!(m.log[0], (MASTER_DATA, 0xFF, true));
        assert_eq!(m.log[1], (SLAVE_DATA, 0xFF, true));
        let first_icw1 = m.log.iter().position(|e| e.0 == MASTER_CMD).unwrap();
        assert!(first_icw1 >= 2);
    }

    #[test]
    fn custom_base_is_programmed() {
        let mut p = Pic::new(Model::default(), 0x70).unwrap();
        p.init().unwrap();
        let m = p.into_io();
        assert_eq!((m.chips[0].base, m.chips[1].base), (0x70, 0x78));
    }

    #[test]
    fn init_twice_is_refused_and_ops_before_init_fail() {
        let mut p = Pic::with_default_base(Model::default());
        assert_eq!(p.mask_irq(1), Err(PicError::NotInitialized));
        assert_eq!(p.unmask_irq(1), Err(PicError::NotInitialized));
        assert_eq!(p.disable_all(), Err(PicError::NotInitialized));
        p.init().unwrap();
        let before = p.io.log.len();
        assert_eq!(p.init(), Err(PicError::AlreadyInitialized));
        assert_eq!(
            p.io.log.len(),
            before,
            "a refused init must not touch the ports"
        );
    }

    #[test]
    fn mask_unmask_update_the_right_chip() {
        let mut p = ready();
        p.unmask_irq(0).unwrap();
        p.unmask_irq(11).unwrap();
        assert_eq!(p.masks(), 0xFFFB & !1 & !(1 << 11));
        let m = &p.io;
        assert_eq!((m.chips[0].imr, m.chips[1].imr), (0xFA, 0xF7));
        p.mask_irq(0).unwrap();
        assert_eq!(p.io.chips[0].imr, 0xFB);
        assert_eq!(p.unmask_irq(16), Err(PicError::InvalidIrq));
        assert_eq!(p.mask_irq(200), Err(PicError::InvalidIrq));
    }

    #[test]
    fn the_cascade_line_cannot_be_masked_individually() {
        let mut p = ready();
        assert_eq!(p.mask_irq(CASCADE_IRQ), Err(PicError::CascadeLine));
        assert_eq!(p.io.chips[0].imr & 4, 0);
        p.disable_all().unwrap();
        assert_eq!((p.io.chips[0].imr, p.io.chips[1].imr), (0xFF, 0xFF));
    }

    #[test]
    fn eoi_sequences_for_master_and_slave_lines() {
        let mut p = ready();
        p.io.log.clear();
        p.eoi_irq(3);
        assert_eq!(
            p.io.log,
            vec![(MASTER_CMD, 0x20, true)],
            "master line: master EOI only"
        );
        p.io.log.clear();
        p.eoi_irq(11);
        assert_eq!(
            p.io.log,
            vec![(SLAVE_CMD, 0x20, true), (MASTER_CMD, 0x20, true)],
            "slave line: slave first, then master"
        );
        p.io.log.clear();
        p.eoi_irq(16);
        assert!(p.io.log.is_empty());
    }

    #[test]
    fn eoi_clears_the_in_service_bit() {
        let mut p = ready();
        p.io.chips[0].isr = 1 << 4 | 1 << 2; // slave in service on cascade, plus IRQ4
        p.io.chips[1].isr = 1 << 3; // IRQ11
        p.eoi_irq(11);
        assert_eq!(p.io.chips[1].isr, 0);
        assert_eq!(p.io.chips[0].isr, 1 << 4, "highest priority (IRQ2) cleared");
    }

    #[test]
    fn spurious_irq7_is_detected_only_when_isr_bit_is_clear_and_gets_no_eoi() {
        let mut p = ready();
        p.io.log.clear();
        assert!(p.check_spurious_irq(7));
        assert_eq!(p.spurious_counts(), (1, 0));
        assert!(
            p.io.log.iter().all(|e| !(e.1 == 0x20 && e.2)),
            "no EOI may be sent for a spurious IRQ7"
        );
        // Genuine IRQ7: ISR bit 7 set.
        p.io.chips[0].isr = 1 << 7;
        assert!(!p.check_spurious_irq(7));
        assert_eq!(p.spurious_counts(), (1, 0));
    }

    #[test]
    fn spurious_irq15_acks_only_the_master() {
        let mut p = ready();
        p.io.log.clear();
        assert!(p.check_spurious_irq(15));
        assert_eq!(p.spurious_counts(), (0, 1));
        let eois: Vec<_> = p.io.log.iter().filter(|e| e.2 && e.1 == 0x20).collect();
        assert_eq!(eois.len(), 1);
        assert_eq!(
            eois[0].0, MASTER_CMD,
            "master only; the slave must not be acked"
        );
        assert_eq!(
            (
                p.io.chips[0].nonspecific_eois,
                p.io.chips[1].nonspecific_eois
            ),
            (1, 0)
        );
        // Genuine IRQ15.
        p.io.chips[1].isr = 1 << 7;
        assert!(!p.check_spurious_irq(15));
    }

    #[test]
    fn only_irq7_and_irq15_can_be_spurious() {
        let mut p = ready();
        for irq in (0..16).filter(|i| *i != 7 && *i != 15) {
            assert!(!p.check_spurious_irq(irq), "irq {irq}");
        }
    }

    #[test]
    fn controller_trait_maps_vectors_and_rejects_foreign_ones() {
        let mut p = ready();
        let c: &mut dyn InterruptController = &mut p;
        assert_eq!(c.unmask(0x20), Ok(()));
        assert_eq!(c.unmask(0x2F), Ok(()));
        assert_eq!(c.unmask(0x30), Err(ControllerError::InvalidVector));
        assert_eq!(c.unmask(0x1F), Err(ControllerError::InvalidVector));
        assert_eq!(c.mask(0x22), Err(ControllerError::Unsupported));
        assert!(
            !c.is_spurious(0x45),
            "vectors outside the PIC range are never PIC-spurious"
        );
        let mut q = Pic::with_default_base(Model::default());
        assert_eq!(q.mask(0x21), Err(ControllerError::NotInitialized));
    }

    #[test]
    fn dispatch_through_the_pic_acks_correctly_end_to_end() {
        fn h(_: u8, _: usize) -> irq::HandlerResult {
            irq::HandlerResult::Handled
        }
        let t = IrqTable::new();
        t.register(0x2B, OWNER_TEST, h, 0).unwrap(); // IRQ11
        let mut p = ready();
        p.io.chips[1].isr = 1 << 3;
        p.io.chips[0].isr = 1 << 2;
        p.io.log.clear();
        assert_eq!(irq::dispatch(&t, &mut p, 0x2B), Outcome::Handled);
        assert_eq!(
            (p.io.chips[0].isr, p.io.chips[1].isr),
            (0, 0),
            "both chips acknowledged"
        );

        // Spurious IRQ15 through dispatch: counted, no handler, master EOI only.
        p.io.chips[0].isr = 1 << 2;
        assert_eq!(irq::dispatch(&t, &mut p, 0x2F), Outcome::Spurious);
        assert_eq!(t.spurious_count(), 1);
        assert_eq!(p.io.chips[0].isr, 0);

        // Unhandled real IRQ on the master: acked, counted.
        p.io.chips[0].isr = 1 << 5;
        assert_eq!(irq::dispatch(&t, &mut p, 0x25), Outcome::Unhandled);
        assert_eq!(p.io.chips[0].isr, 0);
    }

    #[test]
    fn storm_protection_masks_an_unhandled_pic_line() {
        let t = IrqTable::new();
        let mut p = ready();
        p.unmask_irq(5).unwrap();
        for _ in 0..irq::UNHANDLED_MASK_THRESHOLD {
            p.io.chips[0].isr = 1 << 5;
            irq::dispatch(&t, &mut p, 0x25);
        }
        assert_eq!(
            p.masks() & (1 << 5),
            1 << 5,
            "line masked after the threshold"
        );
        assert_eq!(p.io.chips[0].imr & (1 << 5), 1 << 5);
    }
}
