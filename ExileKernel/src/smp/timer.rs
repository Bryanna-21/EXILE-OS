//! Per-CPU local APIC timer calibration and periodic-mode programming.

use super::ipi::APIC_TIMER_VECTOR;
use core::sync::atomic::{AtomicU64, Ordering};

pub const APIC_TIMER_DIVIDE_BY_16: u32 = 0b0011;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerError {
    ZeroSample,
    Overflow,
    ZeroTicks,
    InvalidVector,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerCalibration {
    pub ticks_per_second: u64,
}
impl TimerCalibration {
    pub fn from_sample(initial_count: u32, elapsed_ns: u64) -> Result<Self, TimerError> {
        if initial_count == 0 || elapsed_ns == 0 {
            return Err(TimerError::ZeroSample);
        }
        let ticks = (initial_count as u128)
            .checked_mul(1_000_000_000)
            .ok_or(TimerError::Overflow)?
            / elapsed_ns as u128;
        if ticks == 0 || ticks > u64::MAX as u128 {
            return Err(TimerError::Overflow);
        }
        Ok(Self {
            ticks_per_second: ticks as u64,
        })
    }
    pub fn ticks_for_period_ns(self, period_ns: u64) -> Result<u32, TimerError> {
        let product = (self.ticks_per_second as u128)
            .checked_mul(period_ns as u128)
            .ok_or(TimerError::Overflow)?;
        let ticks = product
            .checked_add(999_999_999)
            .ok_or(TimerError::Overflow)?
            / 1_000_000_000;
        if ticks == 0 {
            return Err(TimerError::ZeroTicks);
        }
        if ticks > u32::MAX as u128 {
            return Err(TimerError::Overflow);
        }
        Ok(ticks as u32)
    }
}

pub trait LocalTimerIo {
    fn write_divide(&mut self, value: u32);
    fn write_lvt(&mut self, value: u32);
    fn write_initial_count(&mut self, value: u32);
}

pub fn start_periodic(
    io: &mut impl LocalTimerIo,
    calibration: TimerCalibration,
    period_ns: u64,
    vector: u8,
) -> Result<u32, TimerError> {
    if vector != APIC_TIMER_VECTOR {
        return Err(TimerError::InvalidVector);
    }
    let ticks = calibration.ticks_for_period_ns(period_ns)?;
    io.write_divide(APIC_TIMER_DIVIDE_BY_16);
    io.write_lvt((1 << 17) | vector as u32);
    io.write_initial_count(ticks);
    Ok(ticks)
}

pub struct PerCpuTicks {
    ticks: AtomicU64,
}
impl PerCpuTicks {
    pub const fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
        }
    }
    pub fn on_tick(&self) -> u64 {
        self.ticks.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub fn read(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }
    pub fn reset(&self) {
        self.ticks.store(0, Ordering::Relaxed);
    }
}
impl Default for PerCpuTicks {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        divide: u32,
        lvt: u32,
        initial: u32,
    }
    impl LocalTimerIo for Fake {
        fn write_divide(&mut self, value: u32) {
            self.divide = value;
        }
        fn write_lvt(&mut self, value: u32) {
            self.lvt = value;
        }
        fn write_initial_count(&mut self, value: u32) {
            self.initial = value;
        }
    }
    #[test]
    fn calibrates_and_starts_periodic_timer() {
        let calibration = TimerCalibration::from_sample(10_000, 1_000_000).unwrap();
        let mut fake = Fake::default();
        let ticks = start_periodic(&mut fake, calibration, 1_000_000, APIC_TIMER_VECTOR).unwrap();
        assert_eq!(ticks, 10_000);
        assert_eq!(fake.divide, APIC_TIMER_DIVIDE_BY_16);
        assert_eq!(fake.lvt & 0xFF, APIC_TIMER_VECTOR as u32);
        assert_ne!(fake.lvt & (1 << 17), 0);
    }
    #[test]
    fn counts_ticks_independently() {
        let ticks = PerCpuTicks::new();
        assert_eq!(ticks.on_tick(), 1);
        assert_eq!(ticks.on_tick(), 2);
    }
}
