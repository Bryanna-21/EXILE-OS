//! Minimal test-and-set spinlock. SPDX-License-Identifier: Apache-2.0
//!
//! Provides mutual exclusion between CPUs/threads. It does NOT mask interrupts:
//! until an IRQ-saving variant exists (needs `cli`/`sti` and the IDT work), a lock
//! must never be taken from an interrupt handler that can preempt a holder of
//! the same lock on that CPU, or it will deadlock. Locking is not recursive; a
//! second `lock()` on the owning context spins forever. There is no fairness.
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: `value` is only reachable through a `SpinLockGuard`, and a guard exists
// only while `locked` is true, which `lock`/`try_lock` set with an Acquire
// compare-exchange and `Drop` clears with a Release store. So at most one
// context accesses `value` at a time and writes made under the lock are visible
// to the next holder. Moving the lock between contexts is fine when `T: Send`.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard { lock: self })
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        loop {
            if let Some(g) = self.try_lock() {
                return g;
            }
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves the lock is held (see the `Sync` impl above).
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as above, and `&mut self` makes this the only live reference
        // obtained through this guard.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn try_lock_fails_while_held_and_succeeds_after_release() {
        let l = SpinLock::new(5u32);
        let g = l.try_lock().unwrap();
        assert!(l.try_lock().is_none());
        drop(g);
        assert!(l.try_lock().is_some());
    }

    #[test]
    fn guard_gives_mutable_access() {
        let l = SpinLock::new(1u32);
        *l.lock() += 41;
        assert_eq!(*l.lock(), 42);
    }

    #[test]
    fn mutual_exclusion_under_contention() {
        let l = Arc::new(SpinLock::new(0u64));
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let l = Arc::clone(&l);
                std::thread::spawn(move || {
                    for _ in 0..50_000 {
                        let mut g = l.lock();
                        let v = *g;
                        *g = v + 1; // non-atomic read-modify-write
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(*l.lock(), 200_000);
    }
}
