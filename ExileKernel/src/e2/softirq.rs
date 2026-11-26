//! Atomic deferred-notification bitmap. Handlers run only when the caller dispatches.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftIrqError {
    BadClass,
    AlreadyRegistered,
    Busy,
}
pub struct SoftIrq<const N: usize> {
    pending: AtomicU64,
    active: AtomicBool,
    handlers: [Option<fn()>; N],
}
impl<const N: usize> SoftIrq<N> {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU64::new(0),
            active: AtomicBool::new(false),
            handlers: [None; N],
        }
    }
    pub fn register(&mut self, class: usize, handler: fn()) -> Result<(), SoftIrqError> {
        if class >= N || class >= 64 {
            return Err(SoftIrqError::BadClass);
        }
        if self.handlers[class].is_some() {
            return Err(SoftIrqError::AlreadyRegistered);
        }
        self.handlers[class] = Some(handler);
        Ok(())
    }
    /// IRQ-safe producer operation: atomically sets a bit and does no callback work.
    pub fn raise(&self, class: usize) -> Result<(), SoftIrqError> {
        if class >= N || class >= 64 {
            return Err(SoftIrqError::BadClass);
        }
        self.pending.fetch_or(1u64 << class, Ordering::Release);
        Ok(())
    }
    /// Dispatch at most `budget` classes. Registration must be complete before concurrent use.
    pub fn dispatch(&self, budget: usize) -> Result<usize, SoftIrqError> {
        if self
            .active
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(SoftIrqError::Busy);
        }
        let bits = self.pending.swap(0, Ordering::AcqRel);
        let mut done = 0;
        let mut deferred = 0u64;
        for i in 0..N.min(64) {
            if bits & (1u64 << i) != 0 {
                if done >= budget {
                    deferred |= 1u64 << i;
                    continue;
                }
                if let Some(h) = self.handlers[i] {
                    h();
                    done += 1
                } else {
                    deferred |= 1u64 << i
                }
            }
        }
        if deferred != 0 {
            self.pending.fetch_or(deferred, Ordering::Release);
        }
        self.active.store(false, Ordering::Release);
        Ok(done)
    }
    pub fn pending(&self) -> u64 {
        self.pending.load(Ordering::Acquire)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn hit() {
        CALLS.fetch_add(1, Ordering::Relaxed);
    }
    #[test]
    fn notification_and_budget() {
        CALLS.store(0, Ordering::Relaxed);
        let mut s = SoftIrq::<3>::new();
        s.register(0, hit).unwrap();
        s.register(2, hit).unwrap();
        s.raise(0).unwrap();
        s.raise(2).unwrap();
        assert_eq!(s.dispatch(1).unwrap(), 1);
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
        assert_ne!(s.pending(), 0);
        assert_eq!(s.dispatch(2).unwrap(), 1);
        assert_eq!(CALLS.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn invalid_class() {
        let s = SoftIrq::<1>::new();
        assert_eq!(s.raise(1), Err(SoftIrqError::BadClass));
    }
}
