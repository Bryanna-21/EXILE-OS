//! Interrupt-safe spin lock (SEC-05).
//!
//! Batch 7's `SpinLock` masks interrupts only when built with `target_os = "none"`; the
//! Batch 8 fallback image is built for the host target, so there the lock does not mask
//! anything. This lock takes an explicit [`IrqControl`], so the masking behaviour is part of
//! the type and cannot silently disappear with the build target.
//!
//! Protocol: lock = save the interrupt state and disable interrupts, then spin; unlock =
//! release, then restore the saved state. Guards must be dropped in reverse acquisition
//! order (Rust's default scope order). Restoring to the SAVED state, not to "enabled",
//! keeps nested locks and callers that already had interrupts off correct.
//!
//! Evidence level: the logic is tested hosted with a simulated interrupt controller
//! (`SimIrq` in the tests). The real x86-64 control (`X86Irq`, feature `x86-ring0`) compiles
//! but has never been executed; `cli`/`sti` need ring 0.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Opaque saved interrupt state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IrqState(pub u64);

pub trait IrqControl {
    /// Return the current state, then disable interrupts on this CPU.
    fn save_and_disable(&self) -> IrqState;
    /// Put the state saved by the matching `save_and_disable` back.
    fn restore(&self, state: IrqState);
}

pub struct IrqSpinLock<T, C: IrqControl> {
    locked: AtomicBool,
    ctl: C,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is serialised by `locked`; `ctl` is shared by reference only.
unsafe impl<T: Send, C: IrqControl + Sync> Sync for IrqSpinLock<T, C> {}
unsafe impl<T: Send, C: IrqControl + Send> Send for IrqSpinLock<T, C> {}

pub struct IrqGuard<'a, T, C: IrqControl> {
    lock: &'a IrqSpinLock<T, C>,
    saved: IrqState,
}

impl<T, C: IrqControl> IrqSpinLock<T, C> {
    pub const fn new(ctl: C, value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            ctl,
            data: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> IrqGuard<'_, T, C> {
        let saved = self.ctl.save_and_disable();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        IrqGuard { lock: self, saved }
    }

    /// Never spins. On failure the interrupt state is restored before returning.
    pub fn try_lock(&self) -> Option<IrqGuard<'_, T, C>> {
        let saved = self.ctl.save_and_disable();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(IrqGuard { lock: self, saved })
        } else {
            self.ctl.restore(saved);
            None
        }
    }
}

impl<T, C: IrqControl> Deref for IrqGuard<'_, T, C> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves the lock is held.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T, C: IrqControl> DerefMut for IrqGuard<'_, T, C> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard proves the lock is held exclusively.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T, C: IrqControl> Drop for IrqGuard<'_, T, C> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        self.lock.ctl.restore(self.saved);
    }
}

/// Real x86-64 control: saves RFLAGS and clears IF; restores IF only if it was set.
/// Compiled only with the `x86-ring0` feature on x86-64. NEVER EXECUTED in any test.
#[cfg(all(feature = "x86-ring0", target_arch = "x86_64"))]
pub struct X86Irq;

#[cfg(all(feature = "x86-ring0", target_arch = "x86_64"))]
impl IrqControl for X86Irq {
    fn save_and_disable(&self) -> IrqState {
        let flags: u64;
        // SAFETY: valid only in ring 0, which is the only place this type may be used.
        unsafe { core::arch::asm!("pushfq", "pop {0}", "cli", out(reg) flags, options(nomem)) };
        IrqState(flags)
    }
    fn restore(&self, state: IrqState) {
        if state.0 & 0x200 != 0 {
            // SAFETY: re-enables interrupts only if they were enabled when saved.
            unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
        }
    }
}
