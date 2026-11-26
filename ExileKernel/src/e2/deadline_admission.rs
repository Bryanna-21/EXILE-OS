//! Bounded periodic-work admission accounting; not an EDF scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reservation {
    pub id: u32,
    pub budget_ns: u64,
    pub period_ns: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    InvalidLimit,
    InvalidId,
    InvalidPeriod,
    InvalidBudget,
    Duplicate,
    Full,
    LimitExceeded,
    NotFound,
    Overflow,
}
pub struct Admission<const N: usize> {
    items: [Option<Reservation>; N],
    limit_ppm: u32,
    used_ppm: u64,
}
impl<const N: usize> Admission<N> {
    pub const fn new(limit_ppm: u32) -> Self {
        Self {
            items: [None; N],
            limit_ppm,
            used_ppm: 0,
        }
    }
    pub const fn utilization_ppm(&self) -> u64 {
        self.used_ppm
    }
    fn cost(r: Reservation) -> Result<u64, AdmissionError> {
        if r.id == 0 {
            return Err(AdmissionError::InvalidId);
        }
        if r.period_ns == 0 {
            return Err(AdmissionError::InvalidPeriod);
        }
        if r.budget_ns == 0 {
            return Err(AdmissionError::InvalidBudget);
        }
        if r.budget_ns > r.period_ns {
            return Err(AdmissionError::InvalidBudget);
        }
        let n = (r.budget_ns as u128)
            .checked_mul(1_000_000)
            .ok_or(AdmissionError::Overflow)?;
        let p = r.period_ns as u128;
        let v = n.checked_add(p - 1).ok_or(AdmissionError::Overflow)? / p;
        u64::try_from(v).map_err(|_| AdmissionError::Overflow)
    }
    pub fn reserve(&mut self, r: Reservation) -> Result<(), AdmissionError> {
        if self.limit_ppm > 1_000_000 {
            return Err(AdmissionError::InvalidLimit);
        }
        let cost = Self::cost(r)?;
        if self.items.iter().flatten().any(|x| x.id == r.id) {
            return Err(AdmissionError::Duplicate);
        }
        let slot = self
            .items
            .iter()
            .position(Option::is_none)
            .ok_or(AdmissionError::Full)?;
        let total = self
            .used_ppm
            .checked_add(cost)
            .ok_or(AdmissionError::Overflow)?;
        if total > self.limit_ppm as u64 {
            return Err(AdmissionError::LimitExceeded);
        }
        self.items[slot] = Some(r);
        self.used_ppm = total;
        Ok(())
    }
    pub fn release(&mut self, id: u32) -> Result<Reservation, AdmissionError> {
        let i = self
            .items
            .iter()
            .position(|x| x.is_some_and(|r| r.id == id))
            .ok_or(AdmissionError::NotFound)?;
        let r = self.items[i].take().ok_or(AdmissionError::NotFound)?;
        self.used_ppm = self
            .used_ppm
            .checked_sub(Self::cost(r)?)
            .ok_or(AdmissionError::Overflow)?;
        Ok(r)
    }
    pub fn check_invariants(&self) -> bool {
        self.limit_ppm <= 1_000_000
            && self
                .items
                .iter()
                .flatten()
                .try_fold(0u64, |a, r| a.checked_add(Self::cost(*r).ok()?))
                .is_some_and(|n| n == self.used_ppm && n <= self.limit_ppm as u64)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserve_release_and_reject_overcommit() {
        let mut a = Admission::<2>::new(800_000);
        let r = Reservation {
            id: 1,
            budget_ns: 1,
            period_ns: 2,
        };
        a.reserve(r).unwrap();
        assert_eq!(a.utilization_ppm(), 500_000);
        assert_eq!(
            a.reserve(Reservation {
                id: 2,
                budget_ns: 2,
                period_ns: 5
            }),
            Err(AdmissionError::LimitExceeded)
        );
        a.release(1).unwrap();
        assert!(a.check_invariants());
    }
    #[test]
    fn invalid_and_duplicate() {
        let mut a = Admission::<1>::new(1_000_000);
        assert_eq!(
            a.reserve(Reservation {
                id: 1,
                budget_ns: 0,
                period_ns: 1
            }),
            Err(AdmissionError::InvalidBudget)
        );
        let r = Reservation {
            id: 1,
            budget_ns: 1,
            period_ns: 1,
        };
        a.reserve(r).unwrap();
        assert_eq!(a.reserve(r), Err(AdmissionError::Duplicate));
    }
}
