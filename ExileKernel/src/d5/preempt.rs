//! Timer-driven preemption (Mission D, requirement 7). SPDX-License-Identifier: Apache-2.0
//!
//! # The three steps are separate
//! 1. **Delivery** (interrupt context, IF clear, lock-free): the C3 vector handler
//!    [`tick_handler`] advances the monotonic clock and bumps an atomic pending-tick counter.
//!    It takes no lock and never touches the scheduler, which keeps the C3 handler rules
//!    ("atomics only") true even though the interrupted code may hold the scheduler lock.
//! 2. **Request** (interrupt EXIT, IF still clear, `HARDIRQ` back to zero): [`irq_exit`] drains
//!    the pending ticks into `Scheduler::tick`, which only does accounting and may set
//!    `need_resched`. The scheduler lock is safe to take here because `KernelSlot::with`
//!    disables interrupts, so the interrupted code cannot be holding it mid-update.
//! 3. **Switch**: [`irq_exit_action`] decides `None`, `Deferred` or `Reschedule`; a reschedule
//!    runs `dispatch::reschedule` on the interrupted thread's own kernel stack. Its full state
//!    sits in the trap frame on that stack and is restored by the stub's `iretq` when the
//!    thread is next switched to.
//!
//! # Critical sections
//! `PREEMPT` is a nestable preemption-disable depth. While it is non-zero a requested
//! reschedule stays pending (`Deferred`) and runs when [`preempt_enable`] drops the depth to
//! zero. Voluntary blocking calls are refused (`PreemptDisabled`) while it is non-zero and
//! from interrupt context (`InterruptContext`). `HARDIRQ` counts handler nesting (>1 never
//! happens with interrupt gates but is handled: no switch inside a nested handler).
//!
//! # Limits (documented, not hidden)
//! * Single CPU. `PREEMPT`/`HARDIRQ` are global, not per-CPU; the pending-tick counter is
//!   global; there is no IPI or cross-CPU resched request.
//! * FPU/SSE/x87 state is NOT saved across preemption. The kernel image is built without SSE,
//!   so kernel threads are unaffected; a user thread that executes x87/SSE would have its
//!   state corrupted by a switch. Lazy-FPU (CR0.TS) or FXSAVE in the trap stubs is not
//!   implemented.
//! * Time keeping: ticks coalesce. If more than [`MAX_TICKS_PER_EXIT`] accumulate before the
//!   exit path runs, the excess is dropped from scheduler accounting (counted in
//!   [`dropped_ticks`]); `timer::CLOCK` still counts every tick.
#![allow(dead_code)]

use crate::irq::{HandlerResult, IrqError, IrqTable, OWNER_TIMER};
use crate::timer;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub const MAX_DEPTH: u32 = 1 << 20;
pub const MAX_TICKS_PER_EXIT: u64 = 64;

/// Nestable depth counter. Underflow and overflow are errors, never wrapped.
pub struct Depth(AtomicU32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthError {
    Underflow,
    Overflow,
}

impl Depth {
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }
    pub fn depth(&self) -> u32 {
        self.0.load(Ordering::Acquire)
    }
    pub fn inc(&self) -> Result<u32, DepthError> {
        let mut cur = self.0.load(Ordering::Acquire);
        loop {
            if cur >= MAX_DEPTH {
                return Err(DepthError::Overflow);
            }
            match self
                .0
                .compare_exchange(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(cur + 1),
                Err(v) => cur = v,
            }
        }
    }
    pub fn dec(&self) -> Result<u32, DepthError> {
        let mut cur = self.0.load(Ordering::Acquire);
        loop {
            if cur == 0 {
                return Err(DepthError::Underflow);
            }
            match self
                .0
                .compare_exchange(cur, cur - 1, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(cur - 1),
                Err(v) => cur = v,
            }
        }
    }
    pub fn reset(&self) {
        self.0.store(0, Ordering::Release);
    }
}

impl Default for Depth {
    fn default() -> Self {
        Self::new()
    }
}

/// Preemption-disable depth (single CPU).
pub static PREEMPT: Depth = Depth::new();
/// Hardware-interrupt handler depth (single CPU).
pub static HARDIRQ: Depth = Depth::new();
static PENDING_TICKS: AtomicU64 = AtomicU64::new(0);
static DROPPED_TICKS: AtomicU64 = AtomicU64::new(0);
static EXIT_HOOK: AtomicUsize = AtomicUsize::new(0);

pub fn in_interrupt() -> bool {
    HARDIRQ.depth() != 0
}
pub fn preemption_enabled() -> bool {
    PREEMPT.depth() == 0
}
pub fn pending_ticks() -> u64 {
    PENDING_TICKS.load(Ordering::Acquire)
}
pub fn dropped_ticks() -> u64 {
    DROPPED_TICKS.load(Ordering::Acquire)
}

/// Why a voluntary blocking operation is refused right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    InterruptContext,
    PreemptDisabled,
}

/// Voluntary yield/block/sleep/join/lock-wait are only legal in thread context with preemption
/// enabled.
pub fn check_may_block() -> Result<(), BlockError> {
    if in_interrupt() {
        return Err(BlockError::InterruptContext);
    }
    if !preemption_enabled() {
        return Err(BlockError::PreemptDisabled);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqExitAction {
    /// No reschedule was requested.
    None,
    /// A reschedule is requested but must wait (preemption disabled, nested handler, or a
    /// previous switch has not completed). The request stays pending in the scheduler.
    Deferred,
    /// Switch now.
    Reschedule,
}

/// Pure decision for the interrupt-exit step.
pub const fn irq_exit_action(
    need_resched: bool,
    preempt_depth: u32,
    hardirq_depth: u32,
    switch_pending: bool,
) -> IrqExitAction {
    if !need_resched {
        IrqExitAction::None
    } else if preempt_depth != 0 || hardirq_depth != 0 || switch_pending {
        IrqExitAction::Deferred
    } else {
        IrqExitAction::Reschedule
    }
}

/// C3 vector handler: counts the tick on the monotonic clock and records one pending scheduler
/// tick. Lock-free and allocation-free, as the C3 handler rules require.
pub fn tick_handler(_vector: u8, _cookie: usize) -> HandlerResult {
    timer::CLOCK.tick();
    PENDING_TICKS.fetch_add(1, Ordering::AcqRel);
    HandlerResult::Handled
}

/// Registers [`tick_handler`] on `vector` in place of the plain C3 `timer::tick_handler`.
pub fn install(table: &IrqTable, vector: u8) -> Result<(), IrqError> {
    table.register(vector, OWNER_TIMER, tick_handler, 0)
}

/// Releases the timer vector (mask the line and stop the source first).
pub fn uninstall(table: &IrqTable, vector: u8) -> Result<(), IrqError> {
    table.deregister(vector, OWNER_TIMER)
}

/// Atomically takes the pending ticks, bounded by [`MAX_TICKS_PER_EXIT`]; the excess is
/// dropped from scheduler accounting and counted.
pub fn take_pending_ticks() -> u64 {
    let n = PENDING_TICKS.swap(0, Ordering::AcqRel);
    if n > MAX_TICKS_PER_EXIT {
        DROPPED_TICKS.fetch_add(n - MAX_TICKS_PER_EXIT, Ordering::AcqRel);
        MAX_TICKS_PER_EXIT
    } else {
        n
    }
}

/// Marks entry into / exit from a hardware interrupt handler (called by the trap path).
pub fn irq_enter() -> Result<(), DepthError> {
    HARDIRQ.inc().map(|_| ())
}
pub fn irq_leave() -> Result<(), DepthError> {
    HARDIRQ.dec().map(|_| ())
}

/// Installs the function the trap path calls at interrupt exit (it owns the `KernelSlot`).
pub fn install_exit_hook(hook: fn()) {
    EXIT_HOOK.store(hook as usize, Ordering::Release);
}
pub fn clear_exit_hook() {
    EXIT_HOOK.store(0, Ordering::Release);
}
/// Runs the installed exit hook, if any. Returns whether one ran.
pub fn run_exit_hook() -> bool {
    let p = EXIT_HOOK.load(Ordering::Acquire);
    if p == 0 {
        return false;
    }
    // SAFETY: only `install_exit_hook` stores a non-zero value, and it stores a valid `fn()`.
    let f: fn() = unsafe { core::mem::transmute::<usize, fn()>(p) };
    f();
    true
}

/// Disables preemption (nestable). Pair with [`preempt_enable`].
pub fn preempt_disable() -> Result<(), DepthError> {
    PREEMPT.inc().map(|_| ())
}

/// RAII preemption-disable for bookkeeping-only critical sections (does NOT reschedule on drop;
/// use [`preempt_enable`] where a deferred switch should run).
pub struct NoPreempt(());
impl NoPreempt {
    pub fn new() -> Result<Self, DepthError> {
        preempt_disable()?;
        Ok(Self(()))
    }
}
impl Drop for NoPreempt {
    fn drop(&mut self) {
        let _ = PREEMPT.dec();
    }
}

#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use kernel_side::*;

#[cfg(target_arch = "x86_64")]
mod kernel_side {
    use super::*;
    use crate::dispatch::{self, KernelSlot};
    use crate::kthread::{Error, StackMemory};

    /// Re-enables preemption. At depth zero a deferred reschedule runs now. Returns whether a
    /// reschedule ran.
    ///
    /// # Safety
    /// See `dispatch::reschedule`.
    pub unsafe fn preempt_enable<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
    ) -> Result<bool, Error> {
        let depth = PREEMPT.dec().map_err(|_| Error::PreemptCount)?;
        if depth != 0 || in_interrupt() {
            return Ok(false);
        }
        // SAFETY: forwarded.
        unsafe { dispatch::preempt_point(slot) }
    }

    /// The interrupt-EXIT step: call from the trap path after the handler returned and `HARDIRQ`
    /// is zero, with interrupts still disabled and on the interrupted thread's kernel stack.
    /// Feeds pending ticks to the scheduler, then reschedules if allowed.
    ///
    /// # Safety
    /// See `dispatch::reschedule`; additionally this runs inside the trap path.
    pub unsafe fn irq_exit<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
    ) -> Result<IrqExitAction, Error> {
        for _ in 0..take_pending_ticks() {
            dispatch::tick(slot);
        }
        let (need, pending) = slot.with(|k| (k.sched().need_resched(), k.switch_pending()));
        let action = irq_exit_action(need, PREEMPT.depth(), HARDIRQ.depth(), pending);
        if action == IrqExitAction::Reschedule {
            // SAFETY: forwarded.
            unsafe { dispatch::reschedule(slot) }?;
        }
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The counters are process-global and the real-switch tests in dispatch.rs use them too:
    /// serialise on the same lock those tests take.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        let g = crate::switch::HOST_SWITCH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        PREEMPT.reset();
        HARDIRQ.reset();
        PENDING_TICKS.store(0, Ordering::SeqCst);
        DROPPED_TICKS.store(0, Ordering::SeqCst);
        clear_exit_hook();
        g
    }

    #[test]
    fn exit_action_truth_table() {
        use IrqExitAction::*;
        for need in [false, true] {
            for p in [0u32, 1, 5] {
                for h in [0u32, 1] {
                    for sw in [false, true] {
                        let want = if !need {
                            None
                        } else if p != 0 || h != 0 || sw {
                            Deferred
                        } else {
                            Reschedule
                        };
                        assert_eq!(irq_exit_action(need, p, h, sw), want);
                    }
                }
            }
        }
    }

    #[test]
    fn depth_nests_and_rejects_underflow_and_overflow() {
        let _g = lock();
        assert_eq!(PREEMPT.dec(), Err(DepthError::Underflow));
        assert_eq!(PREEMPT.inc(), Ok(1));
        assert_eq!(PREEMPT.inc(), Ok(2));
        assert!(!preemption_enabled());
        assert_eq!(PREEMPT.dec(), Ok(1));
        assert_eq!(PREEMPT.dec(), Ok(0));
        assert!(preemption_enabled());
        let d = Depth::new();
        d.0.store(MAX_DEPTH, Ordering::SeqCst);
        assert_eq!(d.inc(), Err(DepthError::Overflow));
        assert_eq!(d.depth(), MAX_DEPTH);
    }

    #[test]
    fn blocking_is_refused_in_critical_sections_and_interrupts() {
        let _g = lock();
        assert_eq!(check_may_block(), Ok(()));
        {
            let _np = NoPreempt::new().unwrap();
            assert_eq!(check_may_block(), Err(BlockError::PreemptDisabled));
            {
                let _inner = NoPreempt::new().unwrap();
                assert_eq!(PREEMPT.depth(), 2);
            }
            assert_eq!(PREEMPT.depth(), 1);
        }
        assert_eq!(check_may_block(), Ok(()));
        irq_enter().unwrap();
        assert!(in_interrupt());
        assert_eq!(check_may_block(), Err(BlockError::InterruptContext));
        irq_leave().unwrap();
        assert_eq!(irq_leave(), Err(DepthError::Underflow));
        assert_eq!(check_may_block(), Ok(()));
    }

    #[test]
    fn tick_handler_counts_clock_and_pending_without_locks() {
        let _g = lock();
        let before = timer::CLOCK.ticks();
        for _ in 0..5 {
            assert_eq!(tick_handler(0x20, 0), HandlerResult::Handled);
        }
        assert_eq!(timer::CLOCK.ticks(), before + 5);
        assert_eq!(pending_ticks(), 5);
        assert_eq!(take_pending_ticks(), 5);
        assert_eq!(pending_ticks(), 0);
    }

    #[test]
    fn pending_ticks_are_bounded_and_the_excess_is_counted() {
        let _g = lock();
        PENDING_TICKS.store(MAX_TICKS_PER_EXIT + 10, Ordering::SeqCst);
        assert_eq!(take_pending_ticks(), MAX_TICKS_PER_EXIT);
        assert_eq!(dropped_ticks(), 10);
        assert_eq!(take_pending_ticks(), 0);
    }

    #[test]
    fn install_registers_with_the_c3_table_and_rejects_exception_vectors() {
        let _g = lock();
        let table = IrqTable::new();
        install(&table, 0x20).unwrap();
        assert_eq!(table.owner_of(0x20), Some(OWNER_TIMER));
        assert!(install(&table, 0x20).is_err(), "double registration");
        assert!(install(&table, 0x0E).is_err(), "exception vector");
        uninstall(&table, 0x20).unwrap();
        assert_eq!(table.owner_of(0x20), None);
    }

    #[test]
    fn exit_hook_runs_only_when_installed() {
        let _g = lock();
        static RAN: AtomicU32 = AtomicU32::new(0);
        fn hook() {
            RAN.fetch_add(1, Ordering::SeqCst);
        }
        RAN.store(0, Ordering::SeqCst);
        assert!(!run_exit_hook());
        install_exit_hook(hook);
        assert!(run_exit_hook());
        assert_eq!(RAN.load(Ordering::SeqCst), 1);
        clear_exit_hook();
        assert!(!run_exit_hook());
    }
}
