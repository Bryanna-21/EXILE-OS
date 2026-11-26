//! Checked monotonic-time contracts. Clock reads are supplied by an adapter.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(u64);
impl Instant {
    pub const ZERO: Self = Self(0);
    pub const fn from_nanos(value: u64) -> Self {
        Self(value)
    }
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    pub fn checked_add(self, duration: Duration) -> Result<Self, DeadlineError> {
        self.0
            .checked_add(duration.0)
            .map(Self)
            .ok_or(DeadlineError::Overflow)
    }
    pub fn checked_duration_since(self, earlier: Self) -> Result<Duration, DeadlineError> {
        self.0
            .checked_sub(earlier.0)
            .map(Duration)
            .ok_or(DeadlineError::BeforeOrigin)
    }
    pub const fn reached(self, deadline: Self) -> bool {
        self.0 >= deadline.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration(u64);
impl Duration {
    pub const ZERO: Self = Self(0);
    pub const fn from_nanos(value: u64) -> Self {
        Self(value)
    }
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    pub fn nonzero(value: u64) -> Result<Self, DeadlineError> {
        if value == 0 {
            Err(DeadlineError::ZeroDuration)
        } else {
            Ok(Self(value))
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineError {
    ZeroDuration,
    Overflow,
    BeforeOrigin,
    InvalidBudget,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    at: Instant,
}
impl Deadline {
    pub const fn at(at: Instant) -> Self {
        Self { at }
    }
    pub fn after(now: Instant, duration: Duration) -> Result<Self, DeadlineError> {
        if duration == Duration::ZERO {
            return Err(DeadlineError::ZeroDuration);
        }
        Ok(Self {
            at: now.checked_add(duration)?,
        })
    }
    pub const fn instant(self) -> Instant {
        self.at
    }
    pub const fn is_expired(self, now: Instant) -> bool {
        now.reached(self.at)
    }
}
/// Narrow adapter to the repository's existing monotonic clock.
pub trait MonotonicNow {
    fn now(&self) -> Instant;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_deadlines_and_boundary() {
        let d = Deadline::after(Instant::from_nanos(10), Duration::from_nanos(5)).unwrap();
        assert!(!d.is_expired(Instant::from_nanos(14)));
        assert!(d.is_expired(Instant::from_nanos(15)));
        assert_eq!(
            Deadline::after(Instant::from_nanos(u64::MAX), Duration::from_nanos(1)),
            Err(DeadlineError::Overflow)
        );
        assert_eq!(
            Deadline::after(Instant::ZERO, Duration::ZERO),
            Err(DeadlineError::ZeroDuration)
        );
    }
    #[test]
    fn duration_since_rejects_reverse_order() {
        assert_eq!(
            Instant::ZERO.checked_duration_since(Instant::from_nanos(1)),
            Err(DeadlineError::BeforeOrigin)
        );
    }
}
