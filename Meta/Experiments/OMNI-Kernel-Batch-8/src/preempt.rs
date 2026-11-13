//! Tick-driven preemption requests (deferred preemption).
//!
//! The timer interrupt handler must not take the scheduler lock: the interrupted thread may already
//! hold it, and the handler would then spin forever on a single CPU. So the handler only does
//! lock-free accounting here (`on_tick`): it counts down the running time slice and, when the slice is
//! used up, raises a request flag. The running thread honours the request at a *safe point* by calling
//! `take_request` and, if that returns true, `Sched::yield_now`. A region that must not be switched away
//! from brackets itself with `disable`/`enable`; a request raised meanwhile is kept, not lost.
//!
//! This is preemption at safe points, not asynchronous context switching from inside the interrupt.
//! A thread that never reaches a safe point is never preempted. Single CPU only.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreemptError {
    /// A time slice of zero ticks would request a switch on every tick.
    ZeroQuantum,
    /// `enable` without a matching `disable`.
    Unbalanced,
}

pub struct Preempt {
    quantum: u32,
    remaining: AtomicU32,
    requested: AtomicBool,
    disabled: AtomicU32,
    requests: AtomicU64,
    honoured: AtomicU64,
}

impl Preempt {
    pub const fn new(quantum: u32) -> Result<Preempt, PreemptError> {
        if quantum == 0 {
            return Err(PreemptError::ZeroQuantum);
        }
        Ok(Preempt {
            quantum,
            remaining: AtomicU32::new(quantum),
            requested: AtomicBool::new(false),
            disabled: AtomicU32::new(0),
            requests: AtomicU64::new(0),
            honoured: AtomicU64::new(0),
        })
    }

    pub fn quantum(&self) -> u32 {
        self.quantum
    }

    /// Called from the timer interrupt, once per tick. Lock-free; atomics only. Returns true when this
    /// tick used up the slice (a request was raised and the slice was reloaded).
    pub fn on_tick(&self) -> bool {
        // Only the timer interrupt writes `remaining`, and it does not nest, so load/store is enough.
        let left = self.remaining.load(Ordering::Relaxed);
        if left > 1 {
            self.remaining.store(left - 1, Ordering::Relaxed);
            return false;
        }
        self.remaining.store(self.quantum, Ordering::Relaxed);
        self.requested.store(true, Ordering::Release);
        self.requests.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Forbid honouring requests (nestable). Requests raised meanwhile stay pending.
    pub fn disable(&self) {
        self.disabled.fetch_add(1, Ordering::SeqCst);
    }

    pub fn enable(&self) -> Result<(), PreemptError> {
        self.disabled
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |d| d.checked_sub(1))
            .map(|_| ())
            .map_err(|_| PreemptError::Unbalanced)
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::SeqCst) != 0
    }

    /// Safe-point check. True means: the caller should yield now. The request is consumed.
    pub fn take_request(&self) -> bool {
        if self.is_disabled() {
            return false;
        }
        if self.requested.swap(false, Ordering::AcqRel) {
            self.honoured.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Number of slices that ran out.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    /// Number of requests that a safe point consumed.
    pub fn honoured(&self) -> u64 {
        self.honoured.load(Ordering::Relaxed)
    }
}
