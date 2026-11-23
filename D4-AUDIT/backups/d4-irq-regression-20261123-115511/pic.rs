//! 8259 PIC and 8253/8254 PIT programming. SPDX-License-Identifier: Apache-2.0
//!
//! All hardware access goes through the `PortIo` trait. The kernel uses `HwPorts`
//! (real `in`/`out`); host tests use a recording fake, so the exact sequence of
//! port writes is checked without touching hardware. Whether the sequence is
//! accepted by a PIC/PIT is proven only by the QEMU tests (`pc` and `q35`).
//!
//! Scope: legacy PIC + PIT only. No APIC/IOAPIC, no HPET, no calibration.
#![allow(dead_code)]
use crate::timer;

pub const MASTER_CMD: u16 = 0x20;
pub const MASTER_DATA: u16 = 0x21;
pub const SLAVE_CMD: u16 = 0xa0;
pub const SLAVE_DATA: u16 = 0xa1;
pub const PIT_CH0: u16 = 0x40;
pub const PIT_CMD: u16 = 0x43;
const IO_WAIT_PORT: u16 = 0x80;

const ICW1_INIT_ICW4: u8 = 0x11;
const ICW4_8086: u8 = 0x01;
const OCW2_EOI: u8 = 0x20;
const OCW3_READ_ISR: u8 = 0x0b;
/// PIT: channel 0, access lobyte/hibyte, mode 2 (rate generator), binary.
const PIT_CH0_MODE2: u8 = 0x34;

/// First vector used for the PIC's 16 IRQ lines.
pub const IRQ_BASE: u8 = 32;
/// Vector of IRQ0 (the PIT).
pub const TIMER_VECTOR: u8 = IRQ_BASE;
/// Vector of the master PIC's spurious IRQ 7.
pub const SPURIOUS_MASTER_VECTOR: u8 = IRQ_BASE + 7;
/// Vector of the slave PIC's spurious IRQ 15.
pub const SPURIOUS_SLAVE_VECTOR: u8 = IRQ_BASE + 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A vector offset is below 32 (would collide with CPU exceptions), not a
    /// multiple of 8, or overlaps the other PIC's range.
    BadOffset,
    /// IRQ number is not 0..=15.
    BadIrq,
    Timer(timer::Error),
}

impl From<timer::Error> for Error {
    fn from(e: timer::Error) -> Self {
        Error::Timer(e)
    }
}

pub trait PortIo {
    fn outb(&mut self, port: u16, value: u8);
    fn inb(&mut self, port: u16) -> u8;
    /// Short delay after a PIC write (some real chips need it). Default: none.
    fn io_wait(&mut self) {}
}

/// Real port access. Only valid in ring 0.
pub struct HwPorts;

impl PortIo for HwPorts {
    fn outb(&mut self, port: u16, value: u8) {
        // SAFETY: `out` to an I/O port; ring 0. The ports used by this module are
        // the PIC, PIT and the 0x80 delay port, which have no memory effects.
        unsafe {
            core::arch::asm!("out dx, al", in("dx") port, in("al") value,
                options(nomem, nostack, preserves_flags));
        }
    }
    fn inb(&mut self, port: u16) -> u8 {
        let v: u8;
        // SAFETY: `in` from an I/O port; ring 0.
        unsafe {
            core::arch::asm!("in al, dx", in("dx") port, out("al") v,
                options(nomem, nostack, preserves_flags));
        }
        v
    }
    fn io_wait(&mut self) {
        self.outb(IO_WAIT_PORT, 0);
    }
}

fn write<P: PortIo>(p: &mut P, port: u16, v: u8) {
    p.outb(port, v);
    p.io_wait();
}

/// Remaps the PIC so IRQ0..7 -> `master_off..+8` and IRQ8..15 -> `slave_off..+8`,
/// then sets the interrupt masks (bit set = line masked).
pub fn init_pic<P: PortIo>(
    p: &mut P,
    master_off: u8,
    slave_off: u8,
    master_mask: u8,
    slave_mask: u8,
) -> Result<(), Error> {
    let ok = |o: u8| o >= 32 && o % 8 == 0;
    let overlap = master_off.abs_diff(slave_off) < 8;
    if !ok(master_off) || !ok(slave_off) || overlap {
        return Err(Error::BadOffset);
    }
    if master_off > 248 || slave_off > 248 {
        return Err(Error::BadOffset);
    }
    write(p, MASTER_CMD, ICW1_INIT_ICW4);
    write(p, SLAVE_CMD, ICW1_INIT_ICW4);
    write(p, MASTER_DATA, master_off);
    write(p, SLAVE_DATA, slave_off);
    write(p, MASTER_DATA, 0x04); // slave on IRQ2
    write(p, SLAVE_DATA, 0x02); // slave cascade identity
    write(p, MASTER_DATA, ICW4_8086);
    write(p, SLAVE_DATA, ICW4_8086);
    write(p, MASTER_DATA, master_mask);
    write(p, SLAVE_DATA, slave_mask);
    Ok(())
}

/// End-of-interrupt for `irq` (0..=15). IRQs 8..15 also acknowledge the master.
pub fn eoi<P: PortIo>(p: &mut P, irq: u8) -> Result<(), Error> {
    if irq > 15 {
        return Err(Error::BadIrq);
    }
    if irq >= 8 {
        p.outb(SLAVE_CMD, OCW2_EOI);
    }
    p.outb(MASTER_CMD, OCW2_EOI);
    Ok(())
}

/// True if a vector-39 interrupt was a spurious IRQ 7 (the in-service bit is
/// clear). A spurious IRQ must NOT be acknowledged.
pub fn is_spurious_irq7<P: PortIo>(p: &mut P) -> bool {
    p.outb(MASTER_CMD, OCW3_READ_ISR);
    p.inb(MASTER_CMD) & 0x80 == 0
}

/// True if a vector-47 interrupt was a spurious IRQ 15.
/// The in-service bit is read from the slave PIC.
pub fn is_spurious_irq15<P: PortIo>(p: &mut P) -> bool {
    p.outb(SLAVE_CMD, OCW3_READ_ISR);
    p.inb(SLAVE_CMD) & 0x80 == 0
}

/// A spurious IRQ 15 must acknowledge the master cascade only.
/// The slave PIC did not place IRQ 15 in service, so it gets no EOI.
pub fn acknowledge_spurious_irq15<P: PortIo>(p: &mut P) {
    p.outb(MASTER_CMD, OCW2_EOI);
}

/// Programs PIT channel 0 as a rate generator with the divisor for `hz`.
/// Returns the divisor used.
pub fn program_pit<P: PortIo>(p: &mut P, hz: u32) -> Result<u16, Error> {
    let divisor = timer::pit_divisor(hz)?;
    p.outb(PIT_CMD, PIT_CH0_MODE2);
    p.outb(PIT_CH0, (divisor & 0xff) as u8);
    p.outb(PIT_CH0, (divisor >> 8) as u8);
    Ok(divisor)
}

/// Remaps the PIC with only IRQ0 (timer) unmasked and starts the PIT at `hz`.
/// Interrupts stay disabled at the CPU; the caller enables them later.
pub fn init_timer_irq<P: PortIo>(p: &mut P, hz: u32) -> Result<u16, Error> {
    // Validate the rate before touching the PIC so a bad argument changes nothing.
    timer::pit_divisor(hz)?;
    init_pic(p, IRQ_BASE, IRQ_BASE + 8, 0xfe, 0xff)?;
    program_pit(p, hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        writes: Vec<(u16, u8)>,
        reads: Vec<u16>,
        isr: u8,
        waits: usize,
    }
    impl PortIo for Fake {
        fn outb(&mut self, port: u16, value: u8) {
            self.writes.push((port, value));
        }
        fn inb(&mut self, port: u16) -> u8 {
            self.reads.push(port);
            self.isr
        }
        fn io_wait(&mut self) {
            self.waits += 1;
        }
    }

    #[test]
    fn pic_init_sequence_is_exact() {
        let mut f = Fake::default();
        init_pic(&mut f, 32, 40, 0xfe, 0xff).unwrap();
        assert_eq!(
            f.writes,
            vec![
                (0x20, 0x11),
                (0xa0, 0x11),
                (0x21, 32),
                (0xa1, 40),
                (0x21, 0x04),
                (0xa1, 0x02),
                (0x21, 0x01),
                (0xa1, 0x01),
                (0x21, 0xfe),
                (0xa1, 0xff),
            ]
        );
        assert_eq!(f.waits, 10, "every PIC write is followed by a delay");
    }

    #[test]
    fn pic_init_rejects_bad_offsets_without_writing() {
        for (m, s) in [
            (0, 8),
            (8, 16),
            (33, 40),
            (32, 36),
            (32, 32),
            (32, 39),
            (248, 255),
            (0xf8, 0x100u16 as u8),
        ] {
            let mut f = Fake::default();
            assert_eq!(
                init_pic(&mut f, m, s, 0, 0),
                Err(Error::BadOffset),
                "{m} {s}"
            );
            assert!(f.writes.is_empty(), "{m} {s}: nothing may be written");
        }
    }

    #[test]
    fn eoi_master_only_for_irq0_to_7() {
        let mut f = Fake::default();
        eoi(&mut f, 0).unwrap();
        eoi(&mut f, 7).unwrap();
        assert_eq!(f.writes, vec![(0x20, 0x20), (0x20, 0x20)]);
    }

    #[test]
    fn eoi_slave_then_master_for_irq8_to_15() {
        let mut f = Fake::default();
        eoi(&mut f, 11).unwrap();
        assert_eq!(f.writes, vec![(0xa0, 0x20), (0x20, 0x20)]);
    }

    #[test]
    fn eoi_rejects_irq_above_15() {
        let mut f = Fake::default();
        assert_eq!(eoi(&mut f, 16), Err(Error::BadIrq));
        assert!(f.writes.is_empty());
    }

    #[test]
    fn spurious_irq7_detection_reads_isr() {
        let mut f = Fake::default();
        f.isr = 0x00;
        assert!(is_spurious_irq7(&mut f));
        assert_eq!(f.writes, vec![(0x20, 0x0b)]);
        let mut g = Fake::default();
        g.isr = 0x80;
        assert!(!is_spurious_irq7(&mut g), "real IRQ7 has its ISR bit set");
        let mut h = Fake::default();
        h.isr = 0x7f;
        assert!(
            is_spurious_irq7(&mut h),
            "other in-service bits do not make IRQ7 real"
        );
    }

    #[test]
    fn pit_program_sequence() {
        let mut f = Fake::default();
        let d = program_pit(&mut f, 100).unwrap();
        assert_eq!(d, 11_932);
        assert_eq!(
            f.writes,
            vec![
                (0x43, 0x34),
                (0x40, (11_932 & 0xff) as u8),
                (0x40, (11_932 >> 8) as u8)
            ]
        );
    }

    #[test]
    fn pit_rejects_out_of_range_rate_without_writing() {
        let mut f = Fake::default();
        assert_eq!(
            program_pit(&mut f, 5),
            Err(Error::Timer(timer::Error::HzOutOfRange))
        );
        assert!(f.writes.is_empty());
    }

    #[test]
    fn init_timer_irq_validates_before_touching_hardware() {
        let mut f = Fake::default();
        assert!(init_timer_irq(&mut f, 0).is_err());
        assert!(f.writes.is_empty(), "bad rate must not remap the PIC");
        let mut g = Fake::default();
        assert_eq!(init_timer_irq(&mut g, 100), Ok(11_932));
        // PIC remap + masks first, then the PIT.
        assert_eq!(g.writes.len(), 10 + 3);
        assert_eq!(g.writes[8], (0x21, 0xfe), "only IRQ0 unmasked");
        assert_eq!(g.writes[9], (0xa1, 0xff), "slave fully masked");
        assert_eq!(g.writes[10], (0x43, 0x34));
    }

    #[test]
    fn vector_constants_do_not_collide_with_exceptions() {
        assert!(TIMER_VECTOR >= 32);
        assert_eq!(SPURIOUS_MASTER_VECTOR, 39);
    }
}
