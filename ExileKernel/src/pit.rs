//! 8253/8254 PIT channel 0 as the fallback timer (Mission C3.5). SPDX-License-Identifier: Apache-2.0
//!
//! Channel 0 runs in mode 2 (rate generator), binary count, lobyte/hibyte access, and raises
//! IRQ0 every `reload` counts of the 1.193182 MHz input clock. Port access goes through
//! `pic::PortIo`, so the programming sequence is host-tested against a model of the chip.
//!
//! Shutdown: writing a mode-0 control word *without loading a count* leaves the output low and
//! the counter idle, so no further interrupts are generated. (Loading a count in mode 0 would
//! produce one more interrupt at terminal count.) The caller should still mask IRQ0 first.
#![allow(dead_code)]
#![allow(
    clippy::manual_is_multiple_of,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::identity_op,
    clippy::unusual_byte_groupings
)]

use crate::pic::PortIo;
use crate::timer::{TickRate, TimerError, TimerSource};

pub const PIT_CHANNEL0: u16 = 0x40;
pub const PIT_COMMAND: u16 = 0x43;
/// Input clock, Hz (14.31818 MHz / 12).
pub const PIT_HZ: u64 = 1_193_182;
/// Lowest rate: reload 65536 (written as 0).
pub const MIN_HZ: u32 = 19;
pub const MAX_HZ: u32 = PIT_HZ as u32;

/// Channel 0, lobyte/hibyte, mode 2, binary.
const CMD_CH0_MODE2: u8 = 0b00_11_010_0;
/// Channel 0, lobyte/hibyte, mode 0, binary (used to park the channel).
const CMD_CH0_MODE0: u8 = 0b00_11_000_0;
/// Channel 0 counter-latch command.
const CMD_LATCH_CH0: u8 = 0;

/// Reload value (1..=65536) closest to `hz`, or an error if `hz` cannot be generated.
pub fn reload_for_hz(hz: u32) -> Result<u32, TimerError> {
    if !(MIN_HZ..=MAX_HZ).contains(&hz) {
        return Err(TimerError::InvalidFrequency);
    }
    let reload = (PIT_HZ + hz as u64 / 2) / hz as u64;
    Ok(reload.clamp(1, 65536) as u32)
}

pub struct Pit<P: PortIo> {
    io: P,
    reload: Option<u32>,
}

impl<P: PortIo> Pit<P> {
    pub fn new(io: P) -> Self {
        Self { io, reload: None }
    }

    pub fn reload(&self) -> Option<u32> {
        self.reload
    }

    pub fn is_running(&self) -> bool {
        self.reload.is_some()
    }

    /// Reads the current channel 0 count using the latch command (atomic 16-bit snapshot).
    pub fn read_count(&mut self) -> u16 {
        self.io.outb(PIT_COMMAND, CMD_LATCH_CH0);
        let lo = self.io.inb(PIT_CHANNEL0) as u16;
        let hi = self.io.inb(PIT_CHANNEL0) as u16;
        hi << 8 | lo
    }

    pub fn into_io(self) -> P {
        self.io
    }
}

impl<P: PortIo> TimerSource for Pit<P> {
    fn name(&self) -> &'static str {
        "PIT channel 0"
    }

    fn start_periodic(&mut self, hz: u32) -> Result<TickRate, TimerError> {
        let reload = reload_for_hz(hz)?;
        self.io.outb(PIT_COMMAND, CMD_CH0_MODE2);
        // 65536 is encoded as 0.
        let v = reload as u16;
        self.io.outb(PIT_CHANNEL0, v as u8);
        self.io.outb(PIT_CHANNEL0, (v >> 8) as u8);
        self.reload = Some(reload);
        Ok(TickRate {
            counts_per_tick: reload as u64,
            counter_hz: PIT_HZ,
        })
    }

    fn stop(&mut self) {
        // Control word only: no count loaded, so the channel never reaches terminal count.
        self.io.outb(PIT_COMMAND, CMD_CH0_MODE0);
        self.reload = None;
    }
}

#[cfg(test)]
pub(crate) mod model {
    use super::*;

    /// Channel 0 model: control word, lobyte/hibyte loading, mode and reload tracking.
    #[derive(Default)]
    pub struct PitModel {
        pub mode: u8,
        pub count_loaded: bool,
        pub reload: u32,
        pub log: Vec<(u16, u8)>,
        lo: Option<u8>,
        pub current: u16,
        latched: Option<u16>,
        read_hi_next: bool,
    }

    impl PortIo for PitModel {
        fn outb(&mut self, port: u16, v: u8) {
            if port == crate::pic::IO_WAIT_PORT {
                return;
            }
            self.log.push((port, v));
            match port {
                PIT_COMMAND => {
                    let channel = v >> 6;
                    assert_eq!(channel, 0, "only channel 0 may be programmed");
                    let access = (v >> 4) & 3;
                    if access == 0 {
                        self.latched = Some(self.current);
                    } else {
                        assert_eq!(access, 3, "lobyte/hibyte access expected");
                        assert_eq!(v & 1, 0, "binary counting expected");
                        self.mode = (v >> 1) & 7;
                        self.count_loaded = false;
                        self.lo = None;
                    }
                }
                PIT_CHANNEL0 => match self.lo.take() {
                    None => self.lo = Some(v),
                    Some(lo) => {
                        let raw = (v as u32) << 8 | lo as u32;
                        self.reload = if raw == 0 { 65536 } else { raw };
                        self.count_loaded = true;
                    }
                },
                _ => panic!("unexpected PIT port {port:#x}"),
            }
        }
        fn inb(&mut self, port: u16) -> u8 {
            assert_eq!(port, PIT_CHANNEL0);
            let v = self.latched.unwrap_or(self.current);
            let b = if self.read_hi_next {
                (v >> 8) as u8
            } else {
                v as u8
            };
            if self.read_hi_next {
                self.latched = None;
            }
            self.read_hi_next = !self.read_hi_next;
            b
        }
    }
}

#[cfg(test)]
mod tests {
    use super::model::PitModel;
    use super::*;
    use crate::irq::{self, IrqTable, Outcome};
    use crate::pic::{Pic, model::Model};
    use crate::timer::{CLOCK, MonotonicClock, install};

    #[test]
    fn reload_values_for_common_rates() {
        assert_eq!(reload_for_hz(100), Ok(11932));
        assert_eq!(reload_for_hz(1000), Ok(1193));
        assert_eq!(reload_for_hz(250), Ok(4773));
        assert_eq!(reload_for_hz(MIN_HZ), Ok(62799));
        assert_eq!(reload_for_hz(MAX_HZ), Ok(1));
    }

    #[test]
    fn out_of_range_rates_are_rejected() {
        for bad in [0u32, 1, 18, MAX_HZ + 1, u32::MAX] {
            assert_eq!(
                reload_for_hz(bad),
                Err(TimerError::InvalidFrequency),
                "{bad}"
            );
        }
        // Every accepted rate gives a reload the 16-bit counter can hold.
        for hz in (MIN_HZ..=MAX_HZ).step_by(997).chain([MAX_HZ]) {
            let r = reload_for_hz(hz).unwrap();
            assert!((1..=65536).contains(&r), "hz {hz} reload {r}");
        }
    }

    #[test]
    fn actual_rate_is_within_rounding_of_the_request() {
        for hz in [19u32, 50, 100, 250, 1000, 5000, 100_000] {
            let r = reload_for_hz(hz).unwrap() as u64;
            let actual = PIT_HZ as f64 / r as f64;
            let err = ((actual - hz as f64) / hz as f64).abs();
            // Rounding the divisor by at most half a count bounds the error by 0.5/reload.
            assert!(err <= 0.5 / r as f64 + 1e-9, "hz {hz}: {err}");
        }
    }

    #[test]
    fn start_programs_mode_2_with_lobyte_then_hibyte() {
        let mut p = Pit::new(PitModel::default());
        let rate = p.start_periodic(100).unwrap();
        assert_eq!(
            rate,
            TickRate {
                counts_per_tick: 11932,
                counter_hz: PIT_HZ
            }
        );
        let io = p.into_io();
        assert_eq!(io.mode, 2);
        assert!(io.count_loaded);
        assert_eq!(io.reload, 11932);
        assert_eq!(
            io.log,
            vec![
                (PIT_COMMAND, 0x34),
                (PIT_CHANNEL0, 11932u16 as u8),
                (PIT_CHANNEL0, (11932u16 >> 8) as u8)
            ]
        );
    }

    #[test]
    fn reload_65536_is_encoded_as_zero() {
        let mut p = Pit::new(PitModel::default());
        // 18 Hz is below the minimum; 19 Hz has reload 62799. Exercise 65536 directly.
        p.io.outb(PIT_COMMAND, 0x34);
        p.io.outb(PIT_CHANNEL0, 0);
        p.io.outb(PIT_CHANNEL0, 0);
        assert_eq!(p.io.reload, 65536);
    }

    #[test]
    fn invalid_start_changes_nothing() {
        let mut p = Pit::new(PitModel::default());
        assert_eq!(p.start_periodic(0), Err(TimerError::InvalidFrequency));
        assert_eq!(
            p.start_periodic(MAX_HZ + 1),
            Err(TimerError::InvalidFrequency)
        );
        assert!(p.io.log.is_empty(), "no port write for a rejected rate");
        assert!(!p.is_running());
    }

    #[test]
    fn stop_parks_channel_0_without_loading_a_count() {
        let mut p = Pit::new(PitModel::default());
        p.start_periodic(100).unwrap();
        assert!(p.is_running());
        p.stop();
        assert!(!p.is_running());
        let io = p.into_io();
        assert_eq!(io.mode, 0);
        assert!(
            !io.count_loaded,
            "mode 0 with no count never reaches terminal count"
        );
        assert_eq!(*io.log.last().unwrap(), (PIT_COMMAND, 0x30));
        // Restart works after a stop.
        let mut p = Pit::new(io);
        p.start_periodic(1000).unwrap();
        assert_eq!(p.into_io().reload, 1193);
    }

    #[test]
    fn counter_latch_read_assembles_16_bits() {
        let mut p = Pit::new(PitModel::default());
        p.io.current = 0xA55A;
        assert_eq!(p.read_count(), 0xA55A);
        assert_eq!(p.io.log.last().copied(), Some((PIT_COMMAND, 0)));
        p.io.current = 0x0102;
        assert_eq!(p.read_count(), 0x0102);
    }

    #[test]
    fn programming_ch1_or_ch2_would_be_caught_by_the_model() {
        // The model panics on a non-zero channel: guard against the driver ever using one.
        let mut m = PitModel::default();
        let r =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.outb(PIT_COMMAND, 0x74)));
        assert!(r.is_err());
    }

    /// PIC + PIT + IRQ table + monotonic clock together, as the kernel wires them.
    #[test]
    fn pit_irq0_through_pic_dispatch_advances_the_clock() {
        CLOCK.reset();
        let local = MonotonicClock::new();
        let mut pic = Pic::with_default_base(Model::default());
        pic.init().unwrap();
        let table = IrqTable::new();
        install(&table, 0x20).unwrap();

        let mut pit = Pit::new(PitModel::default());
        let rate = pit.start_periodic(100).unwrap();
        local.configure(rate).unwrap();
        pic.unmask_irq(0).unwrap();
        assert_eq!(pic.masks() & 1, 0);

        let before = CLOCK.ticks();
        for _ in 0..250 {
            pic.io_for_test().chips[0].isr = 1; // IRQ0 in service
            assert_eq!(irq::dispatch(&table, &mut pic, 0x20), Outcome::Handled);
            assert_eq!(pic.io_for_test().chips[0].isr, 0, "EOI reached the PIC");
            local.tick();
        }
        assert_eq!(
            CLOCK.ticks() - before,
            250,
            "the registered handler counted every tick"
        );
        // 250 ticks at 11932 counts of 1193182 Hz.
        assert_eq!(
            local.now_ns(),
            Some(250u64 * 11932 * 1_000_000_000 / 1_193_182)
        );
        // Shutdown: mask first, then stop; a stray IRQ0 afterwards is acked, not looped on.
        pic.mask_irq(0).unwrap();
        pit.stop();
        assert_eq!(pic.masks() & 1, 1);
    }
}
