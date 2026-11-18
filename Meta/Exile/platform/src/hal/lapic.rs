//! Local APIC register logic over an abstract `Mmio`.
//!
//! Status: register encoding + sequencing only. Tested against a mock register file. NOT run on a
//! real or emulated LAPIC; timer calibration (which needs a reference clock) is not implemented.

/// 32-bit MMIO window.
pub trait Mmio {
    /// Read a 32-bit register at byte offset.
    fn read32(&mut self, off: usize) -> u32;
    /// Write a 32-bit register at byte offset.
    fn write32(&mut self, off: usize, v: u32);
}

/// Register offsets.
pub mod reg {
    /// APIC ID.
    pub const ID: usize = 0x20;
    /// Version.
    pub const VERSION: usize = 0x30;
    /// End of interrupt.
    pub const EOI: usize = 0x0B0;
    /// Spurious interrupt vector register.
    pub const SVR: usize = 0x0F0;
    /// ICR low.
    pub const ICR_LO: usize = 0x300;
    /// ICR high.
    pub const ICR_HI: usize = 0x310;
    /// LVT timer.
    pub const LVT_TIMER: usize = 0x320;
    /// Timer initial count.
    pub const TIMER_INIT: usize = 0x380;
    /// Timer divide configuration.
    pub const TIMER_DIV: usize = 0x3E0;
}

/// Errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LapicError {
    /// Vector < 32 is reserved for exceptions.
    BadVector,
    /// Unsupported timer divisor.
    BadDivisor,
    /// Previous IPI never completed delivery.
    IpiTimeout,
}

/// Local APIC handle.
pub struct LocalApic<M: Mmio> {
    m: M,
}

/// Encode the timer divide register value for divisors 1,2,4,8,16,32,64,128.
pub fn encode_divisor(d: u32) -> Result<u32, LapicError> {
    Ok(match d {
        1 => 0b1011,
        2 => 0b0000,
        4 => 0b0001,
        8 => 0b0010,
        16 => 0b0011,
        32 => 0b1000,
        64 => 0b1001,
        128 => 0b1010,
        _ => return Err(LapicError::BadDivisor),
    })
}

/// IPI kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipi {
    /// Fixed-vector interrupt.
    Fixed(u8),
    /// INIT (level assert).
    Init,
    /// Startup IPI; payload is the 4 KiB page number the AP starts at (real-mode address page << 12).
    Startup(u8),
}

impl<M: Mmio> LocalApic<M> {
    /// Wrap a mapped register window.
    pub fn new(m: M) -> Self {
        LocalApic { m }
    }
    /// APIC id (bits 24..31 in xAPIC mode).
    pub fn id(&mut self) -> u8 {
        (self.m.read32(reg::ID) >> 24) as u8
    }
    /// Version register: (version, max LVT entry).
    pub fn version(&mut self) -> (u8, u8) {
        let v = self.m.read32(reg::VERSION);
        (v as u8, (v >> 16) as u8)
    }
    /// Software-enable with the given spurious vector.
    pub fn enable(&mut self, spurious: u8) -> Result<(), LapicError> {
        if spurious < 32 {
            return Err(LapicError::BadVector);
        }
        self.m.write32(reg::SVR, 0x100 | u32::from(spurious));
        Ok(())
    }
    /// Signal end of interrupt.
    pub fn eoi(&mut self) {
        self.m.write32(reg::EOI, 0);
    }
    /// Start the timer. `periodic` selects periodic vs one-shot mode.
    pub fn start_timer(
        &mut self,
        vector: u8,
        divisor: u32,
        count: u32,
        periodic: bool,
    ) -> Result<(), LapicError> {
        if vector < 32 {
            return Err(LapicError::BadVector);
        }
        let d = encode_divisor(divisor)?;
        self.m.write32(reg::TIMER_DIV, d);
        self.m.write32(
            reg::LVT_TIMER,
            u32::from(vector) | if periodic { 1 << 17 } else { 0 },
        );
        self.m.write32(reg::TIMER_INIT, count);
        Ok(())
    }
    /// Mask the timer and stop it.
    pub fn stop_timer(&mut self) {
        self.m.write32(reg::LVT_TIMER, 1 << 16);
        self.m.write32(reg::TIMER_INIT, 0);
    }
    /// Send an IPI to `dest` (xAPIC id). Waits (bounded) for any previous IPI to finish delivering.
    pub fn send_ipi(&mut self, dest: u8, ipi: Ipi) -> Result<(), LapicError> {
        let lo = match ipi {
            Ipi::Fixed(v) => {
                if v < 32 {
                    return Err(LapicError::BadVector);
                }
                u32::from(v)
            }
            Ipi::Init => (0b101 << 8) | (1 << 14),
            Ipi::Startup(page) => (0b110 << 8) | (1 << 14) | u32::from(page),
        };
        self.wait_idle()?;
        self.m.write32(reg::ICR_HI, u32::from(dest) << 24);
        self.m.write32(reg::ICR_LO, lo);
        Ok(())
    }
    fn wait_idle(&mut self) -> Result<(), LapicError> {
        for _ in 0..100_000 {
            if self.m.read32(reg::ICR_LO) & (1 << 12) == 0 {
                return Ok(());
            }
        }
        Err(LapicError::IpiTimeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    #[derive(Default)]
    struct Mock {
        regs: BTreeMap<usize, u32>,
        log: Vec<(usize, u32)>,
        stuck_busy: bool,
    }
    impl Mmio for &mut Mock {
        fn read32(&mut self, off: usize) -> u32 {
            let v = *self.regs.get(&off).unwrap_or(&0);
            if off == reg::ICR_LO && self.stuck_busy {
                v | 1 << 12
            } else {
                v
            }
        }
        fn write32(&mut self, off: usize, v: u32) {
            self.log.push((off, v));
            self.regs.insert(off, v);
        }
    }
    #[test]
    fn id_version_enable() {
        let mut m = Mock::default();
        m.regs.insert(reg::ID, 3 << 24);
        m.regs.insert(reg::VERSION, 0x0006_0014);
        let mut a = LocalApic::new(&mut m);
        assert_eq!(a.id(), 3);
        assert_eq!(a.version(), (0x14, 6));
        assert_eq!(a.enable(0x10), Err(LapicError::BadVector));
        a.enable(0xFF).unwrap();
        a.eoi();
        assert_eq!(m.regs[&reg::SVR], 0x1FF);
        assert_eq!(m.log.last(), Some(&(reg::EOI, 0)));
    }
    #[test]
    fn divisor_encoding_matches_sdm_table() {
        let want = [
            (1, 0b1011),
            (2, 0),
            (4, 1),
            (8, 2),
            (16, 3),
            (32, 8),
            (64, 9),
            (128, 10),
        ];
        for (d, e) in want {
            assert_eq!(encode_divisor(d), Ok(e), "divisor {d}");
        }
        assert_eq!(encode_divisor(3), Err(LapicError::BadDivisor));
        assert_eq!(encode_divisor(0), Err(LapicError::BadDivisor));
    }
    #[test]
    fn timer_modes_and_ordering() {
        let mut m = Mock::default();
        let mut a = LocalApic::new(&mut m);
        assert_eq!(a.start_timer(5, 16, 1000, true), Err(LapicError::BadVector));
        a.start_timer(0x40, 16, 1000, true).unwrap();
        assert_eq!(m.regs[&reg::LVT_TIMER], 0x40 | 1 << 17);
        // count must be written LAST: writing it arms the timer, so divisor+LVT must already be set.
        assert_eq!(m.log.last(), Some(&(reg::TIMER_INIT, 1000)));
        let mut a = LocalApic::new(&mut m);
        a.start_timer(0x40, 1, 5, false).unwrap();
        assert_eq!(m.regs[&reg::LVT_TIMER], 0x40);
        let mut a = LocalApic::new(&mut m);
        a.stop_timer();
        assert_eq!(m.regs[&reg::LVT_TIMER] & 1 << 16, 1 << 16);
        assert_eq!(m.regs[&reg::TIMER_INIT], 0);
    }
    #[test]
    fn ipi_encodings_and_destination_written_first() {
        let mut m = Mock::default();
        let mut a = LocalApic::new(&mut m);
        a.send_ipi(2, Ipi::Init).unwrap();
        a.send_ipi(2, Ipi::Startup(0x08)).unwrap();
        a.send_ipi(1, Ipi::Fixed(0xF0)).unwrap();
        assert_eq!(a.send_ipi(1, Ipi::Fixed(3)), Err(LapicError::BadVector));
        let lo: Vec<u32> = m
            .log
            .iter()
            .filter(|x| x.0 == reg::ICR_LO)
            .map(|x| x.1)
            .collect();
        assert_eq!(lo, [0x4500, 0x4608, 0xF0]);
        // ICR_HI precedes each ICR_LO write (writing LO sends the IPI).
        let pos: Vec<usize> = m.log.iter().map(|x| x.0).collect();
        assert_eq!(
            pos,
            [
                reg::ICR_HI,
                reg::ICR_LO,
                reg::ICR_HI,
                reg::ICR_LO,
                reg::ICR_HI,
                reg::ICR_LO
            ]
        );
        assert_eq!(m.log[0].1, 2 << 24);
    }
    #[test]
    fn busy_icr_times_out_instead_of_hanging() {
        let mut m = Mock {
            stuck_busy: true,
            ..Mock::default()
        };
        let mut a = LocalApic::new(&mut m);
        assert_eq!(a.send_ipi(1, Ipi::Init), Err(LapicError::IpiTimeout));
        assert!(m.log.is_empty(), "must not write ICR while busy");
    }
}
