//! Fair ticket spin lock for short SMP critical sections.

use core::cell::UnsafeCell;
use core::hint::spin_loop;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct TicketLock<T> {
    next: AtomicUsize,
    serving: AtomicUsize,
    value: UnsafeCell<T>,
}
unsafe impl<T: Send> Send for TicketLock<T> {}
unsafe impl<T: Send> Sync for TicketLock<T> {}

impl<T> TicketLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            next: AtomicUsize::new(0),
            serving: AtomicUsize::new(0),
            value: UnsafeCell::new(value),
        }
    }
    pub fn lock(&self) -> TicketGuard<'_, T> {
        let ticket = self.next.fetch_add(1, Ordering::Relaxed);
        while self.serving.load(Ordering::Acquire) != ticket {
            spin_loop();
        }
        TicketGuard { lock: self }
    }
    pub fn try_lock(&self) -> Option<TicketGuard<'_, T>> {
        let serving = self.serving.load(Ordering::Acquire);
        if self
            .next
            .compare_exchange(
                serving,
                serving.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            Some(TicketGuard { lock: self })
        } else {
            None
        }
    }
    pub fn is_locked(&self) -> bool {
        self.next.load(Ordering::Relaxed) != self.serving.load(Ordering::Acquire)
    }
}

pub struct TicketGuard<'a, T> {
    lock: &'a TicketLock<T>,
}
impl<T> Deref for TicketGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the ticket is exclusively served until Drop advances `serving`.
        unsafe { &*self.lock.value.get() }
    }
}
impl<T> DerefMut for TicketGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: this guard uniquely owns the currently served ticket.
        unsafe { &mut *self.lock.value.get() }
    }
}
impl<T> Drop for TicketGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.serving.fetch_add(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protects_mutable_value() {
        let lock = TicketLock::new(1u32);
        *lock.lock() += 2;
        assert_eq!(*lock.lock(), 3);
    }
    #[test]
    fn try_lock_fails_while_held() {
        let lock = TicketLock::new(0u8);
        let guard = lock.lock();
        assert!(lock.try_lock().is_none());
        drop(guard);
        assert!(lock.try_lock().is_some());
    }
}
