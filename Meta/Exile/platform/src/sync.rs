//! Synchronisation primitives.
//!
//! * `TicketLock<T>`: FIFO spinlock. Its `unsafe` blocks are listed in Documentation/Architecture/Platform/SUBSYSTEM_STATUS.md (section "Unsafe boundaries") (the only other `unsafe` is the CPUID intrinsic in hal/cpu.rs).
//! * `RankStack`: lock-order validator (debug aid; detects inversion, does not prevent it).
//!
//! Status: TicketLock is exercised with real host threads (contention test), which validates the
//! atomics logic on the host's memory model. It has NOT run in-kernel, with interrupts, or on
//! multiple physical cores under the kernel's scheduler. Interrupt-safety (disable IRQs while held)
//! is the caller's job and is not provided here.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};

/// FIFO ticket spinlock.
pub struct TicketLock<T> {
    next: AtomicU32,
    serving: AtomicU32,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is only handed out through a `Guard`, and at most one Guard exists at a
// time (guaranteed by the ticket protocol: a guard exists only for the holder of ticket ==
// `serving`, and `serving` advances only when that guard is dropped). Hence `T: Send` suffices.
unsafe impl<T: Send> Sync for TicketLock<T> {}
// SAFETY: moving the lock moves the owned `T`; requires `T: Send`.
unsafe impl<T: Send> Send for TicketLock<T> {}

/// RAII guard; releases the lock on drop.
pub struct Guard<'a, T> {
    lock: &'a TicketLock<T>,
}

impl<T> TicketLock<T> {
    /// New unlocked lock.
    pub const fn new(v: T) -> Self {
        TicketLock {
            next: AtomicU32::new(0),
            serving: AtomicU32::new(0),
            data: UnsafeCell::new(v),
        }
    }
    /// Acquire, spinning until it is our turn.
    pub fn lock(&self) -> Guard<'_, T> {
        let t = self.next.fetch_add(1, Ordering::Relaxed);
        while self.serving.load(Ordering::Acquire) != t {
            core::hint::spin_loop();
        }
        Guard { lock: self }
    }
    /// Try to acquire without waiting.
    pub fn try_lock(&self) -> Option<Guard<'_, T>> {
        let s = self.serving.load(Ordering::Acquire);
        self.next
            .compare_exchange(s, s.wrapping_add(1), Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Guard { lock: self })
    }
}

impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: this Guard is the unique holder of the lock (see Sync impl above).
        unsafe { &*self.lock.data.get() }
    }
}
impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: unique holder; `&mut self` prevents aliasing through this guard.
        unsafe { &mut *self.lock.data.get() }
    }
}
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        self.lock.serving.fetch_add(1, Ordering::Release);
    }
}

/// Lock-order violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankError {
    /// Acquired a lock whose rank is not strictly greater than the innermost held.
    Inversion {
        /// Rank already held (innermost).
        held: u8,
        /// Rank being acquired.
        wanted: u8,
    },
    /// Stack depth exceeded.
    TooDeep,
    /// Release does not match the innermost lock.
    BadRelease,
}

/// Tracks held lock ranks for one CPU/thread; ranks must strictly increase.
#[derive(Debug, Default)]
pub struct RankStack {
    held: [u8; 8],
    depth: usize,
}

impl RankStack {
    /// Empty.
    pub const fn new() -> Self {
        RankStack {
            held: [0; 8],
            depth: 0,
        }
    }
    /// Record acquisition.
    pub fn acquire(&mut self, rank: u8) -> Result<(), RankError> {
        if self.depth > 0 && rank <= self.held[self.depth - 1] {
            return Err(RankError::Inversion {
                held: self.held[self.depth - 1],
                wanted: rank,
            });
        }
        if self.depth == self.held.len() {
            return Err(RankError::TooDeep);
        }
        self.held[self.depth] = rank;
        self.depth += 1;
        Ok(())
    }
    /// Record release (LIFO).
    pub fn release(&mut self, rank: u8) -> Result<(), RankError> {
        if self.depth == 0 || self.held[self.depth - 1] != rank {
            return Err(RankError::BadRelease);
        }
        self.depth -= 1;
        Ok(())
    }
    /// Locks currently held.
    pub fn depth(&self) -> usize {
        self.depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn basic_lock_unlock_and_try() {
        let l = TicketLock::new(5u32);
        {
            let mut g = l.lock();
            *g += 1;
            assert!(l.try_lock().is_none(), "held lock must refuse try_lock");
        }
        assert_eq!(*l.try_lock().unwrap(), 6);
    }
    // NOTE: kept small and at 2 threads: a FIFO ticket lock convoys badly when spinning threads
    // outnumber vCPUs (a preempted waiter stalls all later tickets). Observed >100s at 4 threads
    // on a small container. A real kernel must disable preemption while spinning.
    #[test]
    fn contended_counter_is_exact() {
        let l = Arc::new(TicketLock::new(0u64));
        let hs: std::vec::Vec<_> = (0..2)
            .map(|_| {
                let l = l.clone();
                std::thread::spawn(move || {
                    for _ in 0..5_000 {
                        *l.lock() += 1;
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(*l.lock(), 10_000);
    }
    #[test]
    fn non_atomic_compound_update_stays_consistent() {
        // Two fields that must always be equal; a broken lock would let a reader see them differ.
        let l = Arc::new(TicketLock::new((0u64, 0u64)));
        let w = {
            let l = l.clone();
            std::thread::spawn(move || {
                for _ in 0..5_000 {
                    let mut g = l.lock();
                    g.0 += 1;
                    std::hint::black_box(&g);
                    g.1 += 1;
                }
            })
        };
        let r = {
            let l = l.clone();
            std::thread::spawn(move || {
                for _ in 0..5_000 {
                    let g = l.lock();
                    assert_eq!(g.0, g.1, "torn read: mutual exclusion violated");
                }
            })
        };
        w.join().unwrap();
        r.join().unwrap();
    }
    #[test]
    fn rank_order_enforced() {
        let mut r = RankStack::new();
        r.acquire(1).unwrap();
        r.acquire(5).unwrap();
        assert_eq!(
            r.acquire(3),
            Err(RankError::Inversion { held: 5, wanted: 3 })
        );
        assert_eq!(
            r.acquire(5),
            Err(RankError::Inversion { held: 5, wanted: 5 }),
            "re-entry is an inversion"
        );
        assert_eq!(r.release(1), Err(RankError::BadRelease), "non-LIFO release");
        r.release(5).unwrap();
        r.release(1).unwrap();
        assert_eq!(r.release(1), Err(RankError::BadRelease));
        assert_eq!(r.depth(), 0);
    }
    #[test]
    fn rank_depth_limit() {
        let mut r = RankStack::new();
        for i in 1..=8 {
            r.acquire(i).unwrap();
        }
        assert_eq!(r.acquire(9), Err(RankError::TooDeep));
    }
}
