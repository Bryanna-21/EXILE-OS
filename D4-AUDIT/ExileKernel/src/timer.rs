//! Timer arithmetic and the monotonic tick clock. SPDX-License-Identifier: Apache-2.0
//!
//! Hardware-independent. The 8253/8254 PIT input clock is 1,193,182 Hz; a
//! 16-bit reload value divides it. All conversions are checked: no wrapping,
//! and sleeps round UP so a thread never wakes before its deadline.
use core::sync::atomic::{AtomicU64, Ordering};

/// PIT input clock in Hz.
pub const PIT_BASE_HZ: u32 = 1_193_182;
/// Default kernel tick rate. With `sched::SLICE_TICKS` = 4 a slice is 40 ms.
pub const DEFAULT_HZ: u32 = 100;
/// Lowest accepted rate: the 16-bit divisor cannot go below 19 Hz.
pub const MIN_HZ: u32 = 19;
/// Highest accepted rate. Above this the handler would dominate the CPU.
pub const MAX_HZ: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Requested rate outside `MIN_HZ..=MAX_HZ`.
    HzOutOfRange,
    /// A conversion overflowed 64 bits.
    Overflow,
}

/// PIT reload value for `hz`, rounded to the nearest divisor.
pub fn pit_divisor(hz: u32) -> Result<u16, Error> {
    if !(MIN_HZ..=MAX_HZ).contains(&hz) {
        return Err(Error::HzOutOfRange);
    }
    let d = (PIT_BASE_HZ + hz / 2) / hz;
    u16::try_from(d).map_err(|_| Error::HzOutOfRange)
}

/// Actual rate in milli-Hz for a divisor (0 means 65536 on the hardware).
pub fn actual_milli_hz(divisor: u16) -> u64 {
    let d = if divisor == 0 {
        65_536u64
    } else {
        u64::from(divisor)
    };
    u64::from(PIT_BASE_HZ) * 1000 / d
}

/// Ticks needed to cover at least `ms` milliseconds (rounds up).
pub fn ms_to_ticks_ceil(ms: u64, hz: u32) -> Result<u64, Error> {
    if !(MIN_HZ..=MAX_HZ).contains(&hz) {
        return Err(Error::HzOutOfRange);
    }
    let n = ms.checked_mul(u64::from(hz)).ok_or(Error::Overflow)?;
    Ok(n / 1000 + u64::from(n % 1000 != 0))
}

/// Whole milliseconds covered by `ticks` (rounds down).
pub fn ticks_to_ms(ticks: u64, hz: u32) -> Result<u64, Error> {
    if !(MIN_HZ..=MAX_HZ).contains(&hz) {
        return Err(Error::HzOutOfRange);
    }
    let n = ticks.checked_mul(1000).ok_or(Error::Overflow)?;
    Ok(n / u64::from(hz))
}

/// Monotonic tick counter, incremented by the timer interrupt handler.
/// It saturates instead of wrapping, so it can never appear to go backwards.
#[derive(Debug)]
pub struct TickClock {
    ticks: AtomicU64,
}

impl TickClock {
    pub const fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
        }
    }

    /// Records one tick and returns the new count.
    pub fn tick(&self) -> u64 {
        match self
            .ticks
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |t| t.checked_add(1))
        {
            Ok(old) => old + 1,
            Err(cur) => cur, // saturated at u64::MAX
        }
    }

    pub fn now(&self) -> u64 {
        self.ticks.load(Ordering::SeqCst)
    }

    /// Address of the counter, for code that must poll it without calling Rust
    /// (the on-target register-integrity test thread). Read-only use only.
    pub fn as_ptr(&self) -> *const u64 {
        self.ticks.as_ptr().cast_const()
    }
}

impl Default for TickClock {
    fn default() -> Self {
        Self::new()
    }
}

/// The kernel's tick clock (single instance; advanced only by the timer handler).
pub static CLOCK: TickClock = TickClock::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divisor_for_default_rate() {
        assert_eq!(pit_divisor(100), Ok(11_932));
        let milli = actual_milli_hz(11_932);
        assert!(milli > 99_900 && milli < 100_100, "{milli}");
    }

    #[test]
    fn divisor_boundaries() {
        assert_eq!(pit_divisor(MIN_HZ), Ok(62_799));
        assert_eq!(pit_divisor(MAX_HZ), Ok(119));
        assert_eq!(pit_divisor(MIN_HZ - 1), Err(Error::HzOutOfRange));
        assert_eq!(pit_divisor(0), Err(Error::HzOutOfRange));
        assert_eq!(pit_divisor(MAX_HZ + 1), Err(Error::HzOutOfRange));
        assert_eq!(pit_divisor(u32::MAX), Err(Error::HzOutOfRange));
    }

    #[test]
    fn every_accepted_rate_has_a_valid_divisor_close_to_the_request() {
        for hz in MIN_HZ..=MAX_HZ {
            let d = pit_divisor(hz).unwrap();
            assert!(d >= 1);
            let got = actual_milli_hz(d) as i64;
            let want = i64::from(hz) * 1000;
            // Rounding the divisor to nearest bounds the relative error by
            // 0.5 / divisor; `actual_milli_hz` floors, which adds at most 1 unit.
            let err = ((got - want).abs() - 1).max(0);
            assert!(err * 2 * i64::from(d) <= want, "hz={hz} got={got}");
        }
    }

    #[test]
    fn divisor_zero_means_65536() {
        assert_eq!(actual_milli_hz(0), 1_193_182_000 / 65_536);
    }

    #[test]
    fn ms_to_ticks_rounds_up_and_never_wakes_early() {
        assert_eq!(ms_to_ticks_ceil(0, 100), Ok(0));
        assert_eq!(ms_to_ticks_ceil(1, 100), Ok(1));
        assert_eq!(ms_to_ticks_ceil(10, 100), Ok(1));
        assert_eq!(ms_to_ticks_ceil(11, 100), Ok(2));
        assert_eq!(ms_to_ticks_ceil(1000, 100), Ok(100));
        for ms in 0..500u64 {
            let t = ms_to_ticks_ceil(ms, 100).unwrap();
            assert!(ticks_to_ms(t, 100).unwrap() >= ms, "ms={ms}");
        }
    }

    #[test]
    fn conversions_detect_overflow_and_bad_rates() {
        assert_eq!(ms_to_ticks_ceil(u64::MAX, 100), Err(Error::Overflow));
        assert_eq!(ticks_to_ms(u64::MAX, 100), Err(Error::Overflow));
        assert_eq!(ms_to_ticks_ceil(5, 0), Err(Error::HzOutOfRange));
        assert_eq!(ticks_to_ms(5, MAX_HZ + 1), Err(Error::HzOutOfRange));
        // Largest value that still fits is accepted.
        assert!(ms_to_ticks_ceil(u64::MAX / 100, 100).is_ok());
    }

    #[test]
    fn clock_is_monotonic() {
        let c = TickClock::new();
        assert_eq!(c.now(), 0);
        let mut last = 0;
        for _ in 0..1000 {
            let t = c.tick();
            assert!(t > last);
            last = t;
        }
        assert_eq!(c.now(), 1000);
    }

    #[test]
    fn clock_pointer_tracks_the_counter() {
        let c = TickClock::new();
        c.tick();
        c.tick();
        // SAFETY: the pointer is to the live atomic inside `c`.
        assert_eq!(unsafe { core::ptr::read_volatile(c.as_ptr()) }, 2);
    }

    #[test]
    fn clock_saturates_instead_of_wrapping() {
        let c = TickClock {
            ticks: AtomicU64::new(u64::MAX - 1),
        };
        assert_eq!(c.tick(), u64::MAX);
        assert_eq!(c.tick(), u64::MAX, "must stay at MAX, not wrap to 0");
        assert_eq!(c.now(), u64::MAX);
    }
}
