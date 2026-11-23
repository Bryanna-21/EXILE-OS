//! Timer-source abstraction, monotonic tick accounting and overflow-safe time conversion
//! (Mission C3.5). SPDX-License-Identifier: Apache-2.0
//!
//! A [`TimerSource`] produces periodic interrupts; the interrupt handler ([`tick_handler`],
//! registered through `irq.rs`) counts them in a lock-free monotonic counter. Time is derived,
//! never accumulated: `ns = ticks * counts_per_tick * 1e9 / counter_hz`, computed in 128 bits
//! and rejected (`None`) rather than wrapped if it does not fit, so rounding error does not
//! build up and a PIT tick of 11932 counts at 1193182 Hz is exact, not "10 ms".
//!
//! This module does **not** enable scheduler preemption or call into the scheduler. The tick
//! handler only increments counters.
#![allow(dead_code)]

use crate::irq::{HandlerResult, IrqError, IrqTable, OWNER_TIMER, Owner};
use core::sync::atomic::{AtomicU64, Ordering};

pub const NS_PER_SEC: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerError {
    /// Requested rate is zero or outside what the source can generate.
    InvalidFrequency,
    /// The source lacks a required capability (periodic mode, legacy routing, ...).
    Unsupported,
    /// The hardware did not behave (counter not advancing, bad capabilities).
    Hardware,
    /// Operation requires the counter to be stopped (or running).
    BadState,
}

/// A source of periodic interrupts.
pub trait TimerSource {
    fn name(&self) -> &'static str;
    /// Starts periodic interrupts at about `hz`. Returns the tick definition actually
    /// programmed so the clock can be configured exactly.
    fn start_periodic(&mut self, hz: u32) -> Result<TickRate, TimerError>;
    /// Stops interrupt generation. After this returns no new interrupt is generated, although
    /// one already pending in the controller may still be delivered (mask the line first).
    fn stop(&mut self);
}

/// One tick is `counts_per_tick` counts of a counter running at `counter_hz`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickRate {
    pub counts_per_tick: u64,
    pub counter_hz: u64,
}

impl TickRate {
    /// `ticks` converted to nanoseconds; `None` on overflow or a zero-frequency rate.
    pub fn ticks_to_ns(&self, ticks: u64) -> Option<u64> {
        counts_to_ns(ticks.checked_mul(self.counts_per_tick)?, self.counter_hz)
    }
    /// Interrupts per second, rounded to nearest.
    pub fn hz_rounded(&self) -> u64 {
        if self.counts_per_tick == 0 {
            return 0;
        }
        (self.counter_hz + self.counts_per_tick / 2) / self.counts_per_tick
    }
}

/// `counts` of a `freq_hz` counter as nanoseconds, rounding down. `None` if `freq_hz == 0` or
/// the result exceeds `u64`.
pub fn counts_to_ns(counts: u64, freq_hz: u64) -> Option<u64> {
    if freq_hz == 0 {
        return None;
    }
    let ns = (counts as u128) * (NS_PER_SEC as u128) / (freq_hz as u128);
    u64::try_from(ns).ok()
}

/// Nanoseconds as counts of a `freq_hz` counter, rounding down. `None` on overflow.
pub fn ns_to_counts(ns: u64, freq_hz: u64) -> Option<u64> {
    if freq_hz == 0 {
        return None;
    }
    let c = (ns as u128) * (freq_hz as u128) / (NS_PER_SEC as u128);
    u64::try_from(c).ok()
}

/// Elapsed counts between two reads of a free-running `bits`-wide counter (1..=64), correct
/// across one wrap. More than one wrap between reads is undetectable; callers must sample often
/// enough.
pub fn counter_delta(prev: u64, now: u64, bits: u32) -> u64 {
    debug_assert!((1..=64).contains(&bits));
    let mask = if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    now.wrapping_sub(prev) & mask
}

/// Which hardware to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Hpet,
    Pit,
}

/// HPET only if its table was found and its registers are reachable; otherwise the PIT, which
/// is always available through port I/O.
pub fn choose_source(hpet_table_found: bool, hpet_mmio_reachable: bool) -> SourceKind {
    if hpet_table_found && hpet_mmio_reachable {
        SourceKind::Hpet
    } else {
        SourceKind::Pit
    }
}

/// Lock-free monotonic tick counter plus the rate that turns ticks into time.
pub struct MonotonicClock {
    ticks: AtomicU64,
    counts_per_tick: AtomicU64,
    counter_hz: AtomicU64,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

pub static CLOCK: MonotonicClock = MonotonicClock::new();

impl MonotonicClock {
    pub const fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
            counts_per_tick: AtomicU64::new(0),
            counter_hz: AtomicU64::new(0),
        }
    }

    /// Sets the tick definition. Call before the source is started (interrupts off).
    pub fn configure(&self, rate: TickRate) -> Result<(), TimerError> {
        if rate.counts_per_tick == 0 || rate.counter_hz == 0 {
            return Err(TimerError::InvalidFrequency);
        }
        self.counts_per_tick
            .store(rate.counts_per_tick, Ordering::Relaxed);
        self.counter_hz.store(rate.counter_hz, Ordering::Release);
        Ok(())
    }

    pub fn rate(&self) -> Option<TickRate> {
        let hz = self.counter_hz.load(Ordering::Acquire);
        let c = self.counts_per_tick.load(Ordering::Relaxed);
        (hz != 0 && c != 0).then_some(TickRate {
            counts_per_tick: c,
            counter_hz: hz,
        })
    }

    /// Counts one timer interrupt. Never decreases; saturates instead of wrapping (a u64 tick
    /// count cannot realistically overflow, but a monotonic clock must never go backwards).
    pub fn tick(&self) -> u64 {
        let mut cur = self.ticks.load(Ordering::Relaxed);
        loop {
            let next = cur.saturating_add(1);
            match self
                .ticks
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed)
            {
                Ok(_) => return next,
                Err(actual) => cur = actual,
            }
        }
    }

    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Acquire)
    }

    /// Monotonic time since the first tick was counted; `None` until configured, or on overflow.
    pub fn now_ns(&self) -> Option<u64> {
        self.rate()?.ticks_to_ns(self.ticks())
    }

    pub fn reset(&self) {
        self.ticks.store(0, Ordering::Release);
    }
}

/// The interrupt handler for the timer vector: counts a tick. Allocation-free, lock-free.
pub fn tick_handler(_vector: u8, _cookie: usize) -> HandlerResult {
    CLOCK.tick();
    HandlerResult::Handled
}

/// Registers [`tick_handler`] on `vector` (a legacy or dynamic vector).
pub fn install(table: &IrqTable, vector: u8) -> Result<(), IrqError> {
    table.register(vector, OWNER_TIMER, tick_handler, 0)
}

/// Releases the timer vector. Mask the line and stop the source first.
pub fn uninstall(table: &IrqTable, vector: u8) -> Result<(), IrqError> {
    table.deregister(vector, OWNER_TIMER)
}

/// Same as [`install`] for tests that need their own owner/handler pair.
pub fn install_with(table: &IrqTable, vector: u8, owner: Owner) -> Result<(), IrqError> {
    table.register(vector, owner, tick_handler, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_are_exact_and_overflow_safe() {
        assert_eq!(counts_to_ns(1_000, 1_000), Some(1_000_000_000));
        assert_eq!(counts_to_ns(0, 1), Some(0));
        assert_eq!(counts_to_ns(1, 0), None);
        // 14.318 MHz HPET: 14_318_180 counts is exactly one second.
        assert_eq!(counts_to_ns(14_318_180, 14_318_180), Some(NS_PER_SEC));
        // A u64 of counts at 1 Hz is far beyond u64 nanoseconds: None, not a wrapped value.
        assert_eq!(counts_to_ns(u64::MAX, 1), None);
        // Largest intermediate (u64::MAX * 1e9) fits in u128; result fits when freq is large.
        assert_eq!(counts_to_ns(u64::MAX, u64::MAX), Some(NS_PER_SEC));
        assert_eq!(counts_to_ns(u64::MAX, 1_000_000_000), Some(u64::MAX));
        assert_eq!(ns_to_counts(NS_PER_SEC, 1_193_182), Some(1_193_182));
        assert_eq!(
            ns_to_counts(u64::MAX, u64::MAX),
            None,
            "result exceeds u64: reported, not wrapped"
        );
        assert_eq!(ns_to_counts(u64::MAX, 1_000_000_000), Some(u64::MAX));
        assert_eq!(ns_to_counts(1, 0), None);
        // Round trip within one count of resolution.
        for hz in [32_768u64, 1_193_182, 14_318_180, 100_000_000] {
            for ns in [1u64, 999, 1_000_000, 123_456_789_012] {
                let c = ns_to_counts(ns, hz).unwrap();
                let back = counts_to_ns(c, hz).unwrap();
                assert!(
                    back <= ns && ns - back <= NS_PER_SEC / hz + 1,
                    "hz {hz} ns {ns}"
                );
            }
        }
    }

    #[test]
    fn rounding_is_toward_zero_so_time_never_runs_ahead() {
        assert_eq!(counts_to_ns(1, 3), Some(333_333_333));
        assert_eq!(counts_to_ns(2, 3), Some(666_666_666));
        assert_eq!(counts_to_ns(3, 3), Some(NS_PER_SEC));
    }

    #[test]
    fn counter_delta_handles_wrap_at_every_width() {
        assert_eq!(counter_delta(10, 25, 32), 15);
        assert_eq!(counter_delta(0xFFFF_FFF0, 0x10, 32), 0x20, "32-bit wrap");
        assert_eq!(counter_delta(u64::MAX - 1, 3, 64), 5, "64-bit wrap");
        assert_eq!(counter_delta(0xFFFE, 1, 16), 3);
        assert_eq!(counter_delta(7, 7, 32), 0);
        assert_eq!(counter_delta(5, 3, 8), 254);
        // Upper garbage bits of a narrower counter are ignored.
        assert_eq!(counter_delta(0xAA_FFFF_FFF0, 0xBB_0000_0010, 32), 0x20);
    }

    #[test]
    fn tick_rate_conversion_is_exact_for_the_pit() {
        // 100 Hz request on the PIT: reload 11932 at 1193182 Hz.
        let r = TickRate {
            counts_per_tick: 11932,
            counter_hz: 1_193_182,
        };
        assert_eq!(r.hz_rounded(), 100);
        assert_eq!(r.ticks_to_ns(0), Some(0));
        // 100 ticks = 11932 * 100 counts = 0.99998...s, NOT a flat second.
        assert_eq!(
            r.ticks_to_ns(100),
            Some((100u128 * 11932 * 1_000_000_000 / 1_193_182) as u64)
        );
        assert!(
            r.ticks_to_ns(100).unwrap() > 1_000_000_000,
            "reload 11932 is 99.9985 Hz: a hair over a second, not a flat one"
        );
        assert_eq!(
            r.ticks_to_ns(u64::MAX),
            None,
            "ticks * counts overflowing u64 is reported"
        );
        assert_eq!(
            TickRate {
                counts_per_tick: 0,
                counter_hz: 1
            }
            .hz_rounded(),
            0
        );
        assert_eq!(
            TickRate {
                counts_per_tick: 1,
                counter_hz: 0
            }
            .ticks_to_ns(5),
            None
        );
    }

    #[test]
    fn clock_needs_configuration_and_validates_it() {
        let c = MonotonicClock::new();
        assert_eq!(c.now_ns(), None);
        assert_eq!(c.rate(), None);
        assert_eq!(
            c.configure(TickRate {
                counts_per_tick: 0,
                counter_hz: 1
            }),
            Err(TimerError::InvalidFrequency)
        );
        assert_eq!(
            c.configure(TickRate {
                counts_per_tick: 1,
                counter_hz: 0
            }),
            Err(TimerError::InvalidFrequency)
        );
        assert_eq!(c.rate(), None, "a rejected configuration changes nothing");
        c.configure(TickRate {
            counts_per_tick: 1,
            counter_hz: 1000,
        })
        .unwrap();
        assert_eq!(c.now_ns(), Some(0));
        for _ in 0..1500 {
            c.tick();
        }
        assert_eq!(c.ticks(), 1500);
        assert_eq!(c.now_ns(), Some(1_500_000_000));
    }

    #[test]
    fn clock_is_monotonic_and_saturates() {
        let c = MonotonicClock::new();
        c.configure(TickRate {
            counts_per_tick: 1,
            counter_hz: 1_000_000_000,
        })
        .unwrap();
        let mut last = 0;
        for _ in 0..1000 {
            c.tick();
            let n = c.now_ns().unwrap();
            assert!(n > last);
            last = n;
        }
        c.ticks.store(u64::MAX, Ordering::Relaxed);
        assert_eq!(c.tick(), u64::MAX, "saturates, never wraps to zero");
        assert_eq!(c.ticks(), u64::MAX);
    }

    #[test]
    fn concurrent_ticks_are_not_lost() {
        use std::sync::Arc;
        let c = Arc::new(MonotonicClock::new());
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let c = Arc::clone(&c);
                std::thread::spawn(move || {
                    for _ in 0..25_000 {
                        c.tick();
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(c.ticks(), 100_000);
    }

    #[test]
    fn source_selection_prefers_hpet_only_when_usable() {
        assert_eq!(choose_source(true, true), SourceKind::Hpet);
        assert_eq!(
            choose_source(true, false),
            SourceKind::Pit,
            "table found but MMIO unmapped"
        );
        assert_eq!(choose_source(false, true), SourceKind::Pit);
        assert_eq!(choose_source(false, false), SourceKind::Pit);
    }

    #[test]
    fn install_registers_the_tick_handler_and_refuses_bad_vectors() {
        let t = IrqTable::new();
        assert_eq!(install(&t, 0x20), Ok(()));
        assert_eq!(t.owner_of(0x20), Some(OWNER_TIMER));
        assert_eq!(install(&t, 0x20), Err(IrqError::InUse));
        assert_eq!(
            install(&t, 0x08),
            Err(IrqError::NotRegistrable),
            "an exception vector"
        );
        assert_eq!(install(&t, 0xFF), Err(IrqError::NotRegistrable));
        assert_eq!(uninstall(&t, 0x20), Ok(()));
        assert_eq!(uninstall(&t, 0x20), Err(IrqError::NotOwner));
    }
}
