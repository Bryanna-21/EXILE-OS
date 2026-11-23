//! IRQ registration, vector ownership and dispatch (Mission C3.1). SPDX-License-Identifier: Apache-2.0
//!
//! Vector map (x86-64):
//!
//! | range       | class     | use |
//! |-------------|-----------|-----|
//! | 0x00..=0x1F | Exception | CPU exceptions. Never registrable, never EOI'd. |
//! | 0x20..=0x2F | Legacy    | 8259 PIC lines IRQ0..15 after remapping (`pic.rs`). |
//! | 0x30..=0xEF | Dynamic   | Handed out by [`IrqTable::register_dynamic`] (APIC/IOAPIC, MSI later). |
//! | 0xF0..=0xFE | Reserved  | Kept for IPIs and local-APIC sources; not registrable yet. |
//! | 0xFF        | Spurious  | Local-APIC spurious vector. No handler, no EOI. |
//!
//! Handler contract: a handler runs in interrupt context with interrupts disabled (the IDT uses
//! interrupt gates). It must not allocate, block, sleep, switch context, or take a lock that
//! non-interrupt code can hold with interrupts enabled (`spin::SpinLock` does not mask
//! interrupts; use [`IrqGuard`] around such critical sections, or use atomics). A handler is
//! never re-entered on the same CPU.
//!
//! Concurrency: all table state is atomic, so dispatch is lock-free and allocation-free.
//! `register`/`deregister` must be serialised by the caller (today: boot code, interrupts off).
//! There is no SMP yet; once there is, a deregistered handler may still be finishing on another
//! CPU, so callers must synchronise (e.g. mask the line and wait) before freeing its cookie.
//!
//! Policy:
//! * Spurious: the controller decides (`InterruptController::is_spurious`); the interrupt is
//!   counted and **not** acknowledged unless the controller requires a partial EOI itself.
//! * Unhandled (no handler, or the handler returned `NotHandled`): counted, acknowledged so the
//!   line cannot wedge the controller, and the line is masked once it has produced
//!   [`UNHANDLED_MASK_THRESHOLD`] unhandled interrupts since the last registration, to stop an
//!   interrupt storm.
//! * Exception vectors reaching [`dispatch`] are not IRQs: `Outcome::NotAnIrq`, nothing touched.
#![allow(dead_code)]
#![allow(clippy::manual_is_multiple_of, clippy::field_reassign_with_default)]

use crate::frame::TrapFrame;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub const EXCEPTION_VECTORS: u8 = 32;
pub const LEGACY_BASE: u8 = 0x20;
pub const LEGACY_LAST: u8 = 0x2F;
pub const DYNAMIC_FIRST: u8 = 0x30;
pub const DYNAMIC_LAST: u8 = 0xEF;
pub const RESERVED_FIRST: u8 = 0xF0;
pub const SPURIOUS_VECTOR: u8 = 0xFF;
/// Unhandled interrupts on one vector before the line is masked.
pub const UNHANDLED_MASK_THRESHOLD: u64 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorClass {
    Exception,
    Legacy,
    Dynamic,
    Reserved,
    Spurious,
}

pub const fn classify_vector(vector: u8) -> VectorClass {
    match vector {
        0..=0x1F => VectorClass::Exception,
        0x20..=0x2F => VectorClass::Legacy,
        0x30..=0xEF => VectorClass::Dynamic,
        0xF0..=0xFE => VectorClass::Reserved,
        0xFF => VectorClass::Spurious,
    }
}

/// True if a handler may be registered on this vector.
pub const fn is_registrable(vector: u8) -> bool {
    matches!(
        classify_vector(vector),
        VectorClass::Legacy | VectorClass::Dynamic
    )
}

/// True for vectors that arrive from an interrupt controller rather than from the CPU itself.
pub const fn is_external_vector(vector: u8) -> bool {
    vector >= EXCEPTION_VECTORS
}

/// Identity of a vector's owner. Only the owner can deregister. Zero is not a valid owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner(NonZeroU32);

impl Owner {
    pub const fn new(id: u32) -> Option<Self> {
        match NonZeroU32::new(id) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }
    pub const fn id(self) -> u32 {
        self.0.get()
    }
}

/// Well-known owners.
pub const OWNER_TIMER: Owner = match Owner::new(1) {
    Some(o) => o,
    None => panic!(),
};
pub const OWNER_TEST: Owner = match Owner::new(0xFFFF_0001) {
    Some(o) => o,
    None => panic!(),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerResult {
    Handled,
    /// The device did not raise this interrupt (shared or misrouted line).
    NotHandled,
}

/// `cookie` is the value given at registration.
pub type Handler = fn(vector: u8, cookie: usize) -> HandlerResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqError {
    /// Exception, reserved or spurious vector.
    NotRegistrable,
    /// The vector already has an owner.
    InUse,
    /// The vector is not owned by the caller (or has no owner).
    NotOwner,
    /// No free dynamic vector.
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerError {
    InvalidVector,
    /// This controller cannot do that (e.g. masking a vector at the local APIC).
    Unsupported,
    NotInitialized,
}

/// What dispatch needs from an interrupt controller. Controller-independent: implemented by the
/// 8259 PIC (`pic.rs`) and the APIC pair (`ioapic.rs`).
pub trait InterruptController {
    fn name(&self) -> &'static str;
    /// Returns true if `vector` is a spurious interrupt. Must perform any controller-specific
    /// step a spurious interrupt requires (the PIC's IRQ15 master EOI) and must **not** send
    /// the ordinary EOI.
    fn is_spurious(&mut self, vector: u8) -> bool;
    /// Acknowledges a real interrupt.
    fn eoi(&mut self, vector: u8);
    fn mask(&mut self, vector: u8) -> Result<(), ControllerError>;
    fn unmask(&mut self, vector: u8) -> Result<(), ControllerError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A handler ran and reported `Handled`.
    Handled,
    /// No handler, or it reported `NotHandled`; the interrupt was acknowledged.
    Unhandled,
    /// The controller identified a spurious interrupt; nothing was acknowledged by dispatch.
    Spurious,
    /// Vector below 32: a CPU exception, not an IRQ. Nothing was touched.
    NotAnIrq,
    /// The saved frame carried a vector above 255.
    BadFrame,
}

pub struct IrqTable {
    owner: [AtomicU32; 256],
    handler: [AtomicUsize; 256],
    cookie: [AtomicUsize; 256],
    handled: [AtomicU64; 256],
    unhandled: [AtomicU64; 256],
    /// Unhandled interrupts since registration; drives the storm mask.
    storm: [AtomicU64; 256],
    spurious: AtomicU64,
    bad_frames: AtomicU64,
}

impl Default for IrqTable {
    fn default() -> Self {
        Self::new()
    }
}

/// The kernel's table.
pub static IRQS: IrqTable = IrqTable::new();

impl IrqTable {
    pub const fn new() -> Self {
        Self {
            owner: [const { AtomicU32::new(0) }; 256],
            handler: [const { AtomicUsize::new(0) }; 256],
            cookie: [const { AtomicUsize::new(0) }; 256],
            handled: [const { AtomicU64::new(0) }; 256],
            unhandled: [const { AtomicU64::new(0) }; 256],
            storm: [const { AtomicU64::new(0) }; 256],
            spurious: AtomicU64::new(0),
            bad_frames: AtomicU64::new(0),
        }
    }

    /// Claims `vector` for `owner` and installs `handler`. The handler becomes visible to
    /// dispatch only after the claim and cookie are stored (Release/Acquire pairing).
    pub fn register(
        &self,
        vector: u8,
        owner: Owner,
        handler: Handler,
        cookie: usize,
    ) -> Result<(), IrqError> {
        if !is_registrable(vector) {
            return Err(IrqError::NotRegistrable);
        }
        let v = vector as usize;
        self.owner[v]
            .compare_exchange(0, owner.id(), Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| IrqError::InUse)?;
        self.cookie[v].store(cookie, Ordering::Relaxed);
        self.storm[v].store(0, Ordering::Relaxed);
        self.handler[v].store(handler as usize, Ordering::Release);
        Ok(())
    }

    /// Claims the lowest free dynamic vector.
    pub fn register_dynamic(
        &self,
        owner: Owner,
        handler: Handler,
        cookie: usize,
    ) -> Result<u8, IrqError> {
        for vector in DYNAMIC_FIRST..=DYNAMIC_LAST {
            match self.register(vector, owner, handler, cookie) {
                Ok(()) => return Ok(vector),
                Err(IrqError::InUse) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(IrqError::Exhausted)
    }

    /// Releases a vector. Only its owner may. After this returns no *new* dispatch will call
    /// the handler (see the SMP note in the module docs).
    pub fn deregister(&self, vector: u8, owner: Owner) -> Result<(), IrqError> {
        let v = vector as usize;
        if self.owner[v].load(Ordering::Acquire) != owner.id() {
            return Err(IrqError::NotOwner);
        }
        self.handler[v].store(0, Ordering::Release);
        self.cookie[v].store(0, Ordering::Relaxed);
        self.owner[v].store(0, Ordering::Release);
        Ok(())
    }

    pub fn owner_of(&self, vector: u8) -> Option<Owner> {
        Owner::new(self.owner[vector as usize].load(Ordering::Acquire))
    }

    pub fn handled_count(&self, vector: u8) -> u64 {
        self.handled[vector as usize].load(Ordering::Relaxed)
    }

    pub fn unhandled_count(&self, vector: u8) -> u64 {
        self.unhandled[vector as usize].load(Ordering::Relaxed)
    }

    pub fn spurious_count(&self) -> u64 {
        self.spurious.load(Ordering::Relaxed)
    }

    pub fn bad_frame_count(&self) -> u64 {
        self.bad_frames.load(Ordering::Relaxed)
    }

    fn lookup(&self, vector: u8) -> Option<(Handler, usize)> {
        let v = vector as usize;
        let raw = self.handler[v].load(Ordering::Acquire);
        if raw == 0 {
            return None;
        }
        let cookie = self.cookie[v].load(Ordering::Relaxed);
        // SAFETY: `raw` is non-zero only if `register` stored a valid `Handler` converted with
        // `as usize`; function pointers round-trip through usize on supported targets.
        let handler = unsafe { core::mem::transmute::<usize, Handler>(raw) };
        Some((handler, cookie))
    }
}

/// Dispatches one external interrupt. See the module docs for the policy.
pub fn dispatch<C: InterruptController + ?Sized>(
    table: &IrqTable,
    ctrl: &mut C,
    vector: u8,
) -> Outcome {
    if !is_external_vector(vector) {
        return Outcome::NotAnIrq;
    }
    if ctrl.is_spurious(vector) {
        table.spurious.fetch_add(1, Ordering::Relaxed);
        return Outcome::Spurious;
    }
    let v = vector as usize;
    let handled = match table.lookup(vector) {
        Some((handler, cookie)) => handler(vector, cookie) == HandlerResult::Handled,
        None => false,
    };
    let outcome = if handled {
        table.handled[v].fetch_add(1, Ordering::Relaxed);
        Outcome::Handled
    } else {
        table.unhandled[v].fetch_add(1, Ordering::Relaxed);
        Outcome::Unhandled
    };
    // Acknowledge before any masking so the controller state stays consistent.
    ctrl.eoi(vector);
    if !handled {
        let n = table.storm[v].fetch_add(1, Ordering::Relaxed) + 1;
        if n == UNHANDLED_MASK_THRESHOLD {
            // Best effort: a controller that cannot mask a vector (local APIC) returns Err.
            let _ = ctrl.mask(vector);
        }
    }
    outcome
}

/// Dispatches from the saved interrupt frame.
pub fn dispatch_frame<C: InterruptController + ?Sized>(
    table: &IrqTable,
    ctrl: &mut C,
    frame: &TrapFrame,
) -> Outcome {
    match frame.vector_u8() {
        Some(v) => dispatch(table, ctrl, v),
        None => {
            table.bad_frames.fetch_add(1, Ordering::Relaxed);
            Outcome::BadFrame
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Interrupt flag control
// ---------------------------------------------------------------------------------------------

/// RFLAGS.IF.
pub const RFLAGS_IF: u64 = 1 << 9;

#[cfg(all(target_arch = "x86_64", not(test)))]
mod flags {
    /// # Safety
    /// Ring 0.
    pub unsafe fn save_and_disable() -> u64 {
        let f: u64;
        // SAFETY: caller contract; pushfq/pop reads RFLAGS, cli clears IF. No memory access
        // beyond the stack slot used by pushfq, so `nostack` is NOT asserted.
        unsafe {
            core::arch::asm!("pushfq", "pop {}", "cli", out(reg) f, options(preserves_flags));
        }
        f
    }
    /// # Safety
    /// Ring 0; `saved` came from `save_and_disable`.
    pub unsafe fn restore(saved: u64) {
        if saved & super::RFLAGS_IF != 0 {
            // SAFETY: caller contract. Re-enabling is only done if IF was set before.
            unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
        }
    }
    /// # Safety
    /// Ring 0, and every precondition for taking interrupts (IDT, controller, handlers) holds.
    pub unsafe fn enable() {
        unsafe { core::arch::asm!("sti", options(nomem, nostack)) };
    }
    /// # Safety
    /// Ring 0.
    pub unsafe fn disable() {
        unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
    }
}

/// Host-test stand-in: a per-thread fake IF so the guard logic is testable without privileged
/// instructions.
#[cfg(any(test, not(target_arch = "x86_64")))]
mod flags {
    use std::cell::Cell;
    std::thread_local! { pub static IF: Cell<bool> = const { Cell::new(true) }; }
    pub unsafe fn save_and_disable() -> u64 {
        IF.with(|f| {
            let was = f.replace(false);
            if was { super::RFLAGS_IF } else { 0 }
        })
    }
    pub unsafe fn restore(saved: u64) {
        if saved & super::RFLAGS_IF != 0 {
            IF.with(|f| f.set(true));
        }
    }
    pub unsafe fn enable() {
        IF.with(|f| f.set(true));
    }
    pub unsafe fn disable() {
        IF.with(|f| f.set(false));
    }
}

/// Enables interrupts on this CPU.
///
/// # Safety
/// Ring 0. Only after the IDT is loaded and complete, the controller is initialised, handlers
/// are registered, EOI works, and any lock shared with a handler masks interrupts.
pub unsafe fn enable_interrupts() {
    // SAFETY: forwarded contract.
    unsafe { flags::enable() }
}

/// # Safety
/// Ring 0.
pub unsafe fn disable_interrupts() {
    // SAFETY: forwarded contract.
    unsafe { flags::disable() }
}

/// Disables interrupts for its lifetime and restores the previous state on drop (re-enabling
/// only if they were enabled before, so guards nest). It protects state shared between normal
/// code and handlers on **this** CPU; it is not a cross-CPU lock.
pub struct IrqGuard {
    saved: u64,
}

impl IrqGuard {
    /// # Safety
    /// Ring 0.
    pub unsafe fn new() -> Self {
        // SAFETY: forwarded contract.
        Self {
            saved: unsafe { flags::save_and_disable() },
        }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        // SAFETY: `saved` came from `save_and_disable` in `new`, which required ring 0.
        unsafe { flags::restore(self.saved) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        spurious_vectors: Vec<u8>,
        eois: Vec<u8>,
        masks: Vec<u8>,
        spurious_calls: Vec<u8>,
        fail_mask: bool,
    }
    impl InterruptController for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn is_spurious(&mut self, v: u8) -> bool {
            self.spurious_calls.push(v);
            self.spurious_vectors.contains(&v)
        }
        fn eoi(&mut self, v: u8) {
            self.eois.push(v);
        }
        fn mask(&mut self, v: u8) -> Result<(), ControllerError> {
            self.masks.push(v);
            if self.fail_mask {
                Err(ControllerError::Unsupported)
            } else {
                Ok(())
            }
        }
        fn unmask(&mut self, _v: u8) -> Result<(), ControllerError> {
            Ok(())
        }
    }

    std::thread_local! { static SEEN: RefCell<Vec<(u8, usize)>> = const { RefCell::new(Vec::new()) }; }
    fn handled(v: u8, c: usize) -> HandlerResult {
        SEEN.with(|s| s.borrow_mut().push((v, c)));
        HandlerResult::Handled
    }
    fn refuses(v: u8, c: usize) -> HandlerResult {
        SEEN.with(|s| s.borrow_mut().push((v, c)));
        HandlerResult::NotHandled
    }
    fn seen() -> Vec<(u8, usize)> {
        SEEN.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }
    const A: Owner = OWNER_TIMER;
    const B: Owner = OWNER_TEST;

    #[test]
    fn vector_classes_partition_the_space() {
        let mut counts = [0usize; 5];
        for v in 0..=255u8 {
            counts[classify_vector(v) as usize] += 1;
        }
        assert_eq!(counts, [32, 16, 192, 15, 1]);
        for v in 0..=0x1F {
            assert_eq!(classify_vector(v), VectorClass::Exception);
            assert!(!is_registrable(v));
            assert!(!is_external_vector(v));
        }
        assert_eq!(classify_vector(0x20), VectorClass::Legacy);
        assert_eq!(classify_vector(0x2F), VectorClass::Legacy);
        assert_eq!(classify_vector(0x30), VectorClass::Dynamic);
        assert_eq!(classify_vector(0xEF), VectorClass::Dynamic);
        assert_eq!(classify_vector(0xF0), VectorClass::Reserved);
        assert_eq!(classify_vector(0xFE), VectorClass::Reserved);
        assert_eq!(classify_vector(0xFF), VectorClass::Spurious);
        assert!(is_external_vector(0x20));
        assert!(Owner::new(0).is_none());
    }

    #[test]
    fn exception_reserved_and_spurious_vectors_cannot_be_registered() {
        let t = IrqTable::new();
        for v in (0..=0x1F).chain(0xF0..=0xFF) {
            assert_eq!(
                t.register(v, A, handled, 0),
                Err(IrqError::NotRegistrable),
                "{v:#x}"
            );
            assert!(t.owner_of(v).is_none());
        }
    }

    #[test]
    fn register_dispatch_passes_vector_and_cookie_then_acks() {
        let t = IrqTable::new();
        t.register(0x30, A, handled, 0xC0FFEE).unwrap();
        let mut c = Fake::default();
        assert_eq!(dispatch(&t, &mut c, 0x30), Outcome::Handled);
        assert_eq!(seen(), vec![(0x30, 0xC0FFEE)]);
        assert_eq!(c.eois, vec![0x30]);
        assert_eq!(t.handled_count(0x30), 1);
        assert_eq!(t.unhandled_count(0x30), 0);
    }

    #[test]
    fn ownership_is_exclusive_and_only_the_owner_releases() {
        let t = IrqTable::new();
        t.register(0x21, A, handled, 1).unwrap();
        assert_eq!(t.register(0x21, B, handled, 2), Err(IrqError::InUse));
        assert_eq!(t.register(0x21, A, handled, 2), Err(IrqError::InUse));
        assert_eq!(t.deregister(0x21, B), Err(IrqError::NotOwner));
        assert_eq!(t.deregister(0x22, A), Err(IrqError::NotOwner)); // never owned
        assert_eq!(t.owner_of(0x21), Some(A));
        t.deregister(0x21, A).unwrap();
        assert_eq!(t.owner_of(0x21), None);
        assert_eq!(t.deregister(0x21, A), Err(IrqError::NotOwner)); // double release
        t.register(0x21, B, handled, 3).unwrap(); // reusable after release
    }

    #[test]
    fn deregistered_vector_is_unhandled_and_the_old_cookie_is_gone() {
        let t = IrqTable::new();
        t.register(0x31, A, handled, 7).unwrap();
        t.deregister(0x31, A).unwrap();
        let mut c = Fake::default();
        assert_eq!(dispatch(&t, &mut c, 0x31), Outcome::Unhandled);
        assert!(
            seen().is_empty(),
            "handler must not run after deregistration"
        );
        assert_eq!(c.eois, vec![0x31]);
    }

    #[test]
    fn dynamic_allocation_is_ordered_unique_and_exhausts_cleanly() {
        let t = IrqTable::new();
        let got: Vec<u8> = (0..192)
            .map(|i| t.register_dynamic(A, handled, i).unwrap())
            .collect();
        assert_eq!(got, (DYNAMIC_FIRST..=DYNAMIC_LAST).collect::<Vec<_>>());
        assert_eq!(t.register_dynamic(B, handled, 0), Err(IrqError::Exhausted));
        // Legacy vectors were never handed out dynamically.
        assert!(t.owner_of(0x2F).is_none());
        t.deregister(0x55, A).unwrap();
        assert_eq!(t.register_dynamic(B, handled, 0), Ok(0x55));
    }

    #[test]
    fn exception_vectors_are_not_irqs_and_nothing_is_touched() {
        let t = IrqTable::new();
        let mut c = Fake::default();
        for v in 0..32u8 {
            assert_eq!(dispatch(&t, &mut c, v), Outcome::NotAnIrq);
        }
        assert!(c.eois.is_empty() && c.spurious_calls.is_empty() && c.masks.is_empty());
    }

    #[test]
    fn spurious_is_counted_not_acked_and_does_not_run_handlers() {
        let t = IrqTable::new();
        t.register(0x27, A, handled, 0).unwrap();
        let mut c = Fake {
            spurious_vectors: vec![0x27],
            ..Fake::default()
        };
        assert_eq!(dispatch(&t, &mut c, 0x27), Outcome::Spurious);
        assert!(seen().is_empty());
        assert!(c.eois.is_empty());
        assert_eq!(t.spurious_count(), 1);
        assert_eq!(t.handled_count(0x27), 0);
    }

    #[test]
    fn unhandled_irq_is_acked_and_the_line_is_masked_exactly_at_the_threshold() {
        let t = IrqTable::new();
        let mut c = Fake::default();
        for i in 1..=UNHANDLED_MASK_THRESHOLD + 5 {
            assert_eq!(dispatch(&t, &mut c, 0x44), Outcome::Unhandled);
            let expect = usize::from(i >= UNHANDLED_MASK_THRESHOLD);
            assert_eq!(c.masks.len(), expect, "after {i}");
        }
        assert_eq!(c.masks, vec![0x44]);
        assert_eq!(
            c.eois.len() as u64,
            UNHANDLED_MASK_THRESHOLD + 5,
            "every one is acked"
        );
        assert_eq!(t.unhandled_count(0x44), UNHANDLED_MASK_THRESHOLD + 5);
    }

    #[test]
    fn handler_returning_not_handled_counts_as_unhandled() {
        let t = IrqTable::new();
        t.register(0x32, A, refuses, 9).unwrap();
        let mut c = Fake::default();
        assert_eq!(dispatch(&t, &mut c, 0x32), Outcome::Unhandled);
        assert_eq!(seen(), vec![(0x32, 9)]);
        assert_eq!(c.eois, vec![0x32]);
    }

    #[test]
    fn a_controller_that_cannot_mask_does_not_break_dispatch() {
        let t = IrqTable::new();
        let mut c = Fake {
            fail_mask: true,
            ..Fake::default()
        };
        for _ in 0..40 {
            assert_eq!(dispatch(&t, &mut c, 0x50), Outcome::Unhandled);
        }
        assert_eq!(c.eois.len(), 40);
    }

    #[test]
    fn reregistration_resets_the_storm_counter() {
        let t = IrqTable::new();
        let mut c = Fake::default();
        for _ in 0..UNHANDLED_MASK_THRESHOLD - 1 {
            dispatch(&t, &mut c, 0x60);
        }
        t.register(0x60, A, handled, 0).unwrap();
        t.deregister(0x60, A).unwrap();
        dispatch(&t, &mut c, 0x60); // would be the threshold without the reset
        assert!(c.masks.is_empty());
    }

    #[test]
    fn frame_dispatch_uses_the_saved_vector() {
        let t = IrqTable::new();
        t.register(0x33, A, handled, 5).unwrap();
        let mut c = Fake::default();
        let mut f = TrapFrame::default();
        f.vector = 0x33;
        assert_eq!(dispatch_frame(&t, &mut c, &f), Outcome::Handled);
        f.vector = 3; // #BP is not an IRQ
        assert_eq!(dispatch_frame(&t, &mut c, &f), Outcome::NotAnIrq);
        f.vector = 256;
        assert_eq!(dispatch_frame(&t, &mut c, &f), Outcome::BadFrame);
        f.vector = u64::MAX;
        assert_eq!(dispatch_frame(&t, &mut c, &f), Outcome::BadFrame);
        assert_eq!(t.bad_frame_count(), 2);
    }

    #[test]
    fn irq_guard_nests_and_restores_the_previous_state() {
        unsafe {
            enable_interrupts();
            assert!(flags::IF.with(|f| f.get()));
            {
                let _outer = IrqGuard::new();
                assert!(!flags::IF.with(|f| f.get()));
                {
                    let _inner = IrqGuard::new();
                    assert!(!flags::IF.with(|f| f.get()));
                }
                assert!(
                    !flags::IF.with(|f| f.get()),
                    "inner drop must not re-enable"
                );
            }
            assert!(flags::IF.with(|f| f.get()), "outer drop restores enabled");
            disable_interrupts();
            {
                let _g = IrqGuard::new();
            }
            assert!(
                !flags::IF.with(|f| f.get()),
                "guard must not enable what was off"
            );
        }
    }

    #[test]
    fn concurrent_registration_hands_each_vector_to_one_owner() {
        use std::sync::Arc;
        let t = Arc::new(IrqTable::new());
        let hs: Vec<_> = (1..=4u32)
            .map(|id| {
                let t = Arc::clone(&t);
                std::thread::spawn(move || {
                    let o = Owner::new(id).unwrap();
                    (0..48)
                        .filter_map(|_| t.register_dynamic(o, handled, 0).ok())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut all: Vec<u8> = hs.into_iter().flat_map(|h| h.join().unwrap()).collect();
        all.sort();
        let n = all.len();
        all.dedup();
        assert_eq!(all.len(), n, "a vector was granted twice");
        assert_eq!(n, 192);
    }
}
