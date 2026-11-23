//! Preemption-disable counter, interrupt-context depth and the interrupt-exit
//! reschedule decision. SPDX-License-Identifier: Apache-2.0
//!
//! Three separate concepts (Mission D 5.7 requires they stay distinct):
//! 1. timer interrupt DELIVERY: the handler runs (`HARDIRQ` is non-zero),
//! 2. reschedule REQUEST: `Scheduler::tick` sets `need_resched`,
//! 3. the actual context SWITCH: performed only at interrupt EXIT, after the
//!    handler has finished and left interrupt context, and only when
//!    `irq_exit_action` says so.
//!
//! Rules enforced by the callers in `dispatch`:
//! * a thread must not block, sleep, join or yield while preemption is disabled
//!   or from interrupt context (`Error::PreemptDisabled` / `InterruptContext`),
//! * a handler never switches; the interrupt-exit step does,
//! * a request made while preemption is disabled is kept and acted on when the
//!   count returns to zero (`dispatch::preempt_enable`).
//!
//! Single CPU: the counters are global. They are atomics so they are sound to
//! touch from a handler, not because they make the scheduler SMP-safe.
use core::sync::atomic::{AtomicU32, Ordering};

/// Nesting limit for either counter. A larger depth is a bug (unbalanced calls).
pub const MAX_DEPTH: u32 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Depth would exceed `MAX_DEPTH`.
    Overflow,
    /// `enable`/`exit` without a matching `disable`/`enter`.
    Underflow,
}

#[derive(Debug)]
pub struct DepthCounter {
    depth: AtomicU32,
}

impl DepthCounter {
    pub const fn new() -> Self {
        Self {
            depth: AtomicU32::new(0),
        }
    }

    pub fn depth(&self) -> u32 {
        self.depth.load(Ordering::SeqCst)
    }

    /// Increments. Fails (changing nothing) at `MAX_DEPTH`.
    pub fn inc(&self) -> Result<u32, Error> {
        self.depth
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |d| {
                if d >= MAX_DEPTH { None } else { Some(d + 1) }
            })
            .map(|old| old + 1)
            .map_err(|_| Error::Overflow)
    }

    /// Decrements and returns the new depth. Fails (changing nothing) at zero.
    pub fn dec(&self) -> Result<u32, Error> {
        self.depth
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |d| d.checked_sub(1))
            .map(|old| old - 1)
            .map_err(|_| Error::Underflow)
    }
}

impl Default for DepthCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Preemption-disable depth (non-zero = the running thread must not be switched
/// away by an interrupt).
pub static PREEMPT: DepthCounter = DepthCounter::new();
/// Hard-interrupt handler depth (non-zero = handler code is running).
pub static HARDIRQ: DepthCounter = DepthCounter::new();

pub fn in_interrupt() -> bool {
    HARDIRQ.depth() != 0
}

pub fn preemption_enabled() -> bool {
    PREEMPT.depth() == 0
}

/// What the interrupt-exit step does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqExitAction {
    /// Nothing was requested.
    None,
    /// A reschedule is requested but must wait (preemption disabled, nested
    /// interrupt, or a previous switch has not completed). The request stays
    /// pending in the scheduler.
    Deferred,
    /// Run the scheduler now, on the interrupted thread's own kernel stack.
    Reschedule,
}

/// Pure decision function; see the module docs.
pub fn irq_exit_action(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_balances_and_reports_new_depth() {
        let c = DepthCounter::new();
        assert_eq!(c.inc(), Ok(1));
        assert_eq!(c.inc(), Ok(2));
        assert_eq!(c.dec(), Ok(1));
        assert_eq!(c.dec(), Ok(0));
        assert_eq!(c.depth(), 0);
    }

    #[test]
    fn underflow_is_an_error_and_changes_nothing() {
        let c = DepthCounter::new();
        assert_eq!(c.dec(), Err(Error::Underflow));
        assert_eq!(c.depth(), 0);
        assert_eq!(c.inc(), Ok(1));
    }

    #[test]
    fn overflow_is_an_error_and_changes_nothing() {
        let c = DepthCounter::new();
        for _ in 0..MAX_DEPTH {
            c.inc().unwrap();
        }
        assert_eq!(c.inc(), Err(Error::Overflow));
        assert_eq!(c.depth(), MAX_DEPTH);
        for _ in 0..MAX_DEPTH {
            c.dec().unwrap();
        }
        assert_eq!(c.depth(), 0);
    }

    #[test]
    fn exit_action_table() {
        use IrqExitAction::*;
        // (need, preempt, hardirq, pending) -> action
        assert_eq!(irq_exit_action(false, 0, 0, false), None);
        assert_eq!(
            irq_exit_action(false, 3, 2, true),
            None,
            "no request, nothing to do"
        );
        assert_eq!(irq_exit_action(true, 0, 0, false), Reschedule);
        assert_eq!(
            irq_exit_action(true, 1, 0, false),
            Deferred,
            "preemption disabled"
        );
        assert_eq!(
            irq_exit_action(true, 0, 1, false),
            Deferred,
            "nested interrupt"
        );
        assert_eq!(
            irq_exit_action(true, 0, 0, true),
            Deferred,
            "switch still pending"
        );
        assert_eq!(irq_exit_action(true, 5, 5, true), Deferred);
    }

    #[test]
    fn deferred_request_becomes_reschedule_when_disable_ends() {
        // Model of the sequence: request arrives while disabled, then enable.
        let c = DepthCounter::new();
        c.inc().unwrap();
        assert_eq!(
            irq_exit_action(true, c.depth(), 0, false),
            IrqExitAction::Deferred
        );
        assert_eq!(c.dec(), Ok(0));
        assert_eq!(
            irq_exit_action(true, c.depth(), 0, false),
            IrqExitAction::Reschedule
        );
    }

    #[test]
    fn counters_are_independent() {
        let p = DepthCounter::new();
        let h = DepthCounter::new();
        p.inc().unwrap();
        assert_eq!(h.depth(), 0);
        h.inc().unwrap();
        p.dec().unwrap();
        assert_eq!(h.depth(), 1);
    }
}
