//! Interrupt-flag control and the IRQ-saving spinlock. SPDX-License-Identifier: Apache-2.0
//!
//! `SpinLock` alone is unsafe against an interrupt handler on the same CPU: if a
//! handler takes a lock its interrupted code already holds, the CPU spins forever.
//! `IrqSpinLock` closes that hole by saving RFLAGS.IF and clearing it BEFORE the
//! lock is taken, and restoring the saved state AFTER the lock is released. Every
//! lock that an interrupt handler may take (the scheduler lock) must be an
//! `IrqSpinLock`.
//!
//! Hardware path: `pushfq; pop; cli` and `sti`/`cli`. Host test builds use a
//! per-OS-thread model of IF instead, because `cli` is privileged; the model
//! lets the nesting and restore rules be tested, and says nothing about the CPU.
//!
//! Single CPU only. This is not an SMP-safe interrupt discipline.
use crate::spin::{SpinLock, SpinLockGuard};
use core::ops::{Deref, DerefMut};

/// RFLAGS.IF.
pub const IF_BIT: u64 = 1 << 9;

/// Saved interrupt state: the RFLAGS value (only IF is interpreted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "dropping saved interrupt state without restoring it leaves interrupts disabled"]
pub struct IrqFlags(u64);

impl IrqFlags {
    pub const fn from_rflags(rflags: u64) -> Self {
        Self(rflags)
    }
    /// Were interrupts enabled when this state was saved?
    pub const fn were_enabled(self) -> bool {
        self.0 & IF_BIT != 0
    }
}

#[cfg(not(test))]
mod hw {
    use super::{IF_BIT, IrqFlags};
    use core::arch::asm;

    pub fn save_disable() -> IrqFlags {
        let flags: u64;
        // SAFETY: reads RFLAGS and clears IF. Ring 0 only (the kernel). No memory
        // is accessed other than the push/pop pair on the current stack.
        unsafe {
            asm!("pushfq", "pop {f}", "cli", f = out(reg) flags);
        }
        IrqFlags(flags)
    }

    pub fn restore(f: IrqFlags) {
        // SAFETY: sets IF to the saved value; `sti`/`cli` touch no memory.
        unsafe {
            if f.0 & IF_BIT != 0 {
                asm!("sti", options(nomem, nostack));
            } else {
                asm!("cli", options(nomem, nostack));
            }
        }
    }

    pub fn enabled() -> bool {
        let flags: u64;
        // SAFETY: reads RFLAGS through the stack.
        unsafe {
            asm!("pushfq", "pop {f}", f = out(reg) flags, options(preserves_flags));
        }
        flags & IF_BIT != 0
    }
}

#[cfg(test)]
mod hw {
    use super::{IF_BIT, IrqFlags};
    use std::cell::Cell;

    std::thread_local! {
        /// Model of RFLAGS.IF for this OS thread. Starts enabled.
        static MODEL_IF: Cell<bool> = const { Cell::new(true) };
    }

    pub fn save_disable() -> IrqFlags {
        let was = MODEL_IF.with(|c| c.replace(false));
        IrqFlags(if was { IF_BIT } else { 0 })
    }
    pub fn restore(f: IrqFlags) {
        MODEL_IF.with(|c| c.set(f.0 & IF_BIT != 0));
    }
    pub fn enabled() -> bool {
        MODEL_IF.with(|c| c.get())
    }
}

/// Disables interrupts and returns the previous state.
pub fn save_disable() -> IrqFlags {
    hw::save_disable()
}

/// Restores a state returned by `save_disable`.
pub fn restore(f: IrqFlags) {
    hw::restore(f)
}

/// Are interrupts currently enabled?
pub fn enabled() -> bool {
    hw::enabled()
}

/// Unconditionally enables interrupts. Used once per new thread (first run) and
/// once at the end of boot; everywhere else use `save_disable`/`restore`.
pub fn enable() {
    hw::restore(IrqFlags(IF_BIT));
}

/// RAII: interrupts are off from `new()` until the value is dropped, which
/// restores the PREVIOUS state (so nesting is safe).
#[must_use = "interrupts are restored as soon as this value is dropped"]
pub struct IrqOff(IrqFlags);

impl IrqOff {
    pub fn new() -> Self {
        Self(save_disable())
    }
}

impl Default for IrqOff {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IrqOff {
    fn drop(&mut self) {
        restore(self.0);
    }
}

/// A spinlock that is safe to share with interrupt handlers on one CPU.
pub struct IrqSpinLock<T> {
    inner: SpinLock<T>,
}

pub struct IrqSpinLockGuard<'a, T> {
    // Field order is the drop order: the lock is released first, THEN the saved
    // interrupt state is restored.
    guard: SpinLockGuard<'a, T>,
    _restore: IrqOff,
}

impl<T> IrqSpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: SpinLock::new(value),
        }
    }

    /// Disables interrupts, then takes the lock.
    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        let restore = IrqOff::new();
        let guard = self.inner.lock();
        IrqSpinLockGuard {
            guard,
            _restore: restore,
        }
    }

    /// Disables interrupts and tries the lock; on failure the interrupt state is
    /// restored before returning `None`.
    pub fn try_lock(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let restore = IrqOff::new();
        let guard = self.inner.try_lock()?;
        Some(IrqSpinLockGuard {
            guard,
            _restore: restore,
        })
    }
}

impl<T> Deref for IrqSpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for IrqSpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset(on: bool) {
        if on {
            enable()
        } else {
            let _ = save_disable();
        }
    }

    #[test]
    fn save_disable_clears_and_returns_previous() {
        reset(true);
        let f = save_disable();
        assert!(f.were_enabled());
        assert!(!enabled());
        restore(f);
        assert!(enabled());
    }

    #[test]
    fn restore_preserves_an_already_disabled_state() {
        reset(false);
        let f = save_disable();
        assert!(!f.were_enabled());
        restore(f);
        assert!(!enabled(), "must not enable interrupts that were off");
    }

    #[test]
    fn nested_irq_off_restores_each_level() {
        reset(true);
        {
            let _a = IrqOff::new();
            assert!(!enabled());
            {
                let _b = IrqOff::new();
                assert!(!enabled());
            }
            assert!(!enabled(), "inner drop must not re-enable");
        }
        assert!(enabled());
    }

    #[test]
    fn lock_disables_interrupts_while_held_and_restores_after() {
        reset(true);
        let l = IrqSpinLock::new(1u32);
        {
            let mut g = l.lock();
            assert!(!enabled(), "IF must be clear while the lock is held");
            *g += 1;
        }
        assert!(enabled());
        assert_eq!(*l.lock(), 2);
    }

    #[test]
    fn lock_taken_with_interrupts_off_leaves_them_off() {
        reset(false);
        let l = IrqSpinLock::new(0u8);
        drop(l.lock());
        assert!(!enabled());
        reset(true);
    }

    #[test]
    fn failed_try_lock_restores_interrupt_state() {
        reset(true);
        let l = IrqSpinLock::new(0u8);
        let g = l.lock();
        assert!(!enabled());
        // A second acquisition attempt while held fails and must restore the
        // state it found, which is "disabled" (the holder's).
        assert!(l.try_lock().is_none());
        assert!(!enabled());
        drop(g);
        assert!(enabled());
        assert!(l.try_lock().is_some());
        assert!(enabled());
    }

    #[test]
    fn lock_is_released_before_interrupts_return() {
        // If the guard restored IF first, an interrupt could run while the lock
        // was still held. The model cannot observe the instant between the two
        // drops, but it can check the end state of both.
        reset(true);
        let l = IrqSpinLock::new(7u32);
        drop(l.lock());
        assert!(enabled());
        assert!(l.try_lock().is_some(), "lock must be free after drop");
    }

    #[test]
    fn flags_from_rflags_interprets_only_if() {
        assert!(IrqFlags::from_rflags(0x202).were_enabled());
        assert!(!IrqFlags::from_rflags(0x2).were_enabled());
        assert!(!IrqFlags::from_rflags(!IF_BIT).were_enabled());
    }
}
