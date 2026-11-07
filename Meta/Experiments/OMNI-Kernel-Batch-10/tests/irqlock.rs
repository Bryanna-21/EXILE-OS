//! SEC-05: interrupt-safe lock. HOSTED with a simulated interrupt controller. This shows the
//! lock's protocol is right; it is NOT evidence that real interrupt masking works.

use omni10::irqlock::{IrqControl, IrqSpinLock, IrqState};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};

type Handler<'a> = Box<dyn FnOnce() + 'a>;

/// Interrupts run at once when enabled, queue while disabled, and are delivered when a
/// restore re-enables them.
struct SimIrq<'a> {
    enabled: Cell<bool>,
    pending: RefCell<Vec<Handler<'a>>>,
}
impl<'a> SimIrq<'a> {
    fn new(enabled: bool) -> Self {
        SimIrq {
            enabled: Cell::new(enabled),
            pending: RefCell::new(Vec::new()),
        }
    }
    fn raise(&self, h: Handler<'a>) {
        if self.enabled.get() {
            h();
        } else {
            self.pending.borrow_mut().push(h);
        }
    }
    fn flush(&self) {
        while self.enabled.get() {
            let next = {
                let mut p = self.pending.borrow_mut();
                if p.is_empty() {
                    return;
                }
                p.remove(0)
            };
            next();
        }
    }
}
impl IrqControl for &SimIrq<'_> {
    fn save_and_disable(&self) -> IrqState {
        let s = IrqState(self.enabled.get() as u64);
        self.enabled.set(false);
        s
    }
    fn restore(&self, s: IrqState) {
        self.enabled.set(s.0 == 1);
        self.flush();
    }
}

/// The Batch 8 fallback behaviour: nothing is masked and an interrupt always runs at once.
struct NoMask;
impl IrqControl for NoMask {
    fn save_and_disable(&self) -> IrqState {
        IrqState(1)
    }
    fn restore(&self, _: IrqState) {}
}

#[test]
fn irq_01_lock_disables_and_unlock_restores_the_saved_state() {
    for start in [true, false] {
        let irq = SimIrq::new(start);
        let l = IrqSpinLock::new(&irq, 0u32);
        {
            let mut g = l.lock();
            *g += 1;
            assert!(
                !irq.enabled.get(),
                "interrupts must be off while the lock is held"
            );
        }
        assert_eq!(
            irq.enabled.get(),
            start,
            "state not restored to what it was"
        );
        assert_eq!(*l.lock(), 1);
    }
}

#[test]
fn irq_02_nested_locks_restore_in_reverse_order() {
    let irq = SimIrq::new(true);
    let a = IrqSpinLock::new(&irq, 0u32);
    let b = IrqSpinLock::new(&irq, 0u32);
    let ga = a.lock();
    assert!(!irq.enabled.get());
    let gb = b.lock();
    drop(gb);
    assert!(
        !irq.enabled.get(),
        "inner unlock must not re-enable while the outer lock is held"
    );
    drop(ga);
    assert!(
        irq.enabled.get(),
        "outer unlock restores the original enabled state"
    );
}

#[test]
fn irq_03_interrupt_raised_while_held_is_deferred_and_then_sees_a_free_lock() {
    // Leaked so the handler may refer to the lock that refers to the controller.
    let irq: &'static SimIrq<'static> = Box::leak(Box::new(SimIrq::new(true)));
    let lock: &'static IrqSpinLock<u32, &'static SimIrq<'static>> =
        Box::leak(Box::new(IrqSpinLock::new(irq, 0u32)));
    let seen: &'static Cell<Option<bool>> = Box::leak(Box::new(Cell::new(None)));
    {
        let _g = lock.lock();
        irq.raise(Box::new(|| {
            // A handler that wants the same lock. It may only run once the lock is free.
            seen.set(Some(lock.try_lock().is_some()));
        }));
        assert_eq!(seen.get(), None, "handler ran while the lock was held");
    }
    assert_eq!(
        seen.get(),
        Some(true),
        "deferred handler did not run after unlock"
    );
}

#[test]
fn irq_04_without_masking_the_handler_observes_the_held_lock() {
    // This is the defect of the unmasked fallback: the handler runs inside the critical
    // section, finds the lock taken, and a handler using lock() would spin forever.
    let lock = IrqSpinLock::new(NoMask, 0u32);
    let g = lock.lock();
    let saw_held = lock.try_lock().is_none(); // what an interrupt handler would experience
    assert!(saw_held);
    drop(g);
}

#[test]
fn irq_05_failed_try_lock_restores_interrupt_state() {
    let irq = SimIrq::new(true);
    let lock = IrqSpinLock::new(&irq, 0u32);
    let g = lock.lock();
    assert!(lock.try_lock().is_none());
    assert!(
        !irq.enabled.get(),
        "failed try_lock re-enabled interrupts inside the critical section"
    );
    drop(g);
    assert!(irq.enabled.get());
    // and a failed try_lock taken from an interrupts-off context keeps them off
    let irq2 = SimIrq::new(false);
    let l2 = IrqSpinLock::new(&irq2, 0u32);
    let g2 = l2.lock();
    assert!(l2.try_lock().is_none());
    drop(g2);
    assert!(!irq2.enabled.get());
}

#[test]
fn irq_06_mutual_exclusion_across_os_threads() {
    static LOCK: IrqSpinLock<AtomicU64, NoMaskSync> =
        IrqSpinLock::new(NoMaskSync, AtomicU64::new(0));
    struct NoMaskSync;
    impl IrqControl for NoMaskSync {
        fn save_and_disable(&self) -> IrqState {
            IrqState(0)
        }
        fn restore(&self, _: IrqState) {}
    }
    // non-atomic read-modify-write protected only by the lock
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..20_000 {
                    let g = LOCK.lock();
                    let v = g.load(Ordering::Relaxed);
                    g.store(v + 1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(LOCK.lock().load(Ordering::Relaxed), 80_000);
}

#[test]
fn irq_07_guard_gives_exclusive_access_and_releases_on_panic_unwind() {
    let irq = SimIrq::new(true);
    let lock = IrqSpinLock::new(&irq, vec![1, 2, 3]);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut g = lock.lock();
        g.push(4);
        panic!("boom");
    }));
    assert!(r.is_err());
    assert!(irq.enabled.get(), "unwinding left interrupts disabled");
    assert_eq!(
        *lock.lock(),
        vec![1, 2, 3, 4],
        "lock was not released by unwinding"
    );
}
