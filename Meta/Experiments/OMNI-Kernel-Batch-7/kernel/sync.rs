//! A spin lock. On a single CPU with interrupts masked it is a plain critical section; the
//! interrupt masking is `irq_save`/`irq_restore`, real only for bare-metal builds.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "none")]
fn irq_save() -> u64 {
    let flags: u64;
    // SAFETY: reads RFLAGS and clears IF; both are always valid in ring 0.
    unsafe {
        core::arch::asm!("pushfq", "pop {0}", "cli", out(reg) flags, options(nomem));
    }
    flags
}

#[cfg(target_os = "none")]
fn irq_restore(flags: u64) {
    if flags & 0x200 != 0 {
        // SAFETY: re-enables interrupts only if they were enabled when saved.
        unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
    }
}

// Hosted builds have no interrupts to mask.
#[cfg(not(target_os = "none"))]
fn irq_save() -> u64 {
    0
}

#[cfg(not(target_os = "none"))]
fn irq_restore(_flags: u64) {}

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is serialised by `locked`.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
    irq: u64,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinGuard<'_, T> {
        let irq = irq_save();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinGuard { lock: self, irq }
    }
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves the lock is held.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard proves the lock is held exclusively.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        irq_restore(self.irq);
    }
}
