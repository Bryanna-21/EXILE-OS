//! Single-CPU scheduler policy model: priority round-robin with aging.
//!
//! This is POLICY and STATE only. `schedule()` decides which thread runs next
//! and returns a `Switch`; performing the register/stack switch is the job of
//! the architecture layer (Mission C), which does not exist yet. Nothing here
//! is wired into the kernel image, takes locks, or is SMP-safe: every method
//! assumes the caller holds the (future) scheduler lock with IRQs disabled.
//!
//! Policy (see docs/SCHEDULER.md):
//! - 8 levels; 0 = idle only, 1..=7 ordinary, higher is more urgent.
//! - Strict priority between levels, FIFO round-robin within a level.
//! - Fixed time slice of `SLICE_TICKS`; a thread preempted or yielding goes to
//!   the back of its base-level queue.
//! - Aging: a Ready thread that waits `AGING_TICKS` is boosted one level
//!   (repeatedly, up to the top level) and loses the boost when dispatched.
//!   This bounds starvation of low-priority threads.
//! - Timer tick, reschedule request and actual switch are separate steps:
//!   `tick()` only sets `need_resched`; `schedule()` selects; the arch layer
//!   switches and then calls `switch_complete()`.
use crate::runqueue::RunQueues;
use crate::thread::{
    BlockReason, Error, Handle, PRIORITY_LEVELS, Priority, State, Thread, ThreadTable, WakeReason,
};

pub const SLICE_TICKS: u32 = 4;
pub const AGING_TICKS: u64 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch {
    pub prev: Option<Handle>,
    pub next: Handle,
}

impl Switch {
    /// False when the chosen thread is the one already running.
    pub fn is_switch(&self) -> bool {
        self.prev != Some(self.next)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    /// Thread is now Blocked; the caller must invoke `schedule()`.
    Blocked,
    /// A wake-up was already pending or the condition already held; keep running.
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepOutcome {
    /// Thread is now Sleeping; the caller must invoke `schedule()`.
    Sleeping,
    /// Zero-tick sleep: behaves as a yield point; thread stays Running.
    Immediate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeOutcome {
    Woken,
    /// Target was Running; its next `block_current` will not sleep.
    Pending,
    AlreadyReady,
}

pub struct Scheduler<const N: usize> {
    table: ThreadTable<N>,
    queues: RunQueues<N>,
    current: Option<Handle>,
    idle: Option<Handle>,
    now: u64,
    need_resched: bool,
}

impl<const N: usize> Default for Scheduler<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Scheduler<N> {
    pub fn new() -> Self {
        Self {
            table: ThreadTable::new(),
            queues: RunQueues::new(),
            current: None,
            idle: None,
            now: 0,
            need_resched: false,
        }
    }

    pub fn current(&self) -> Option<Handle> {
        self.current
    }
    pub fn idle(&self) -> Option<Handle> {
        self.idle
    }
    pub fn now(&self) -> u64 {
        self.now
    }
    pub fn need_resched(&self) -> bool {
        self.need_resched
    }
    pub fn thread(&self, h: Handle) -> Result<&Thread, Error> {
        self.table.get(h)
    }
    pub fn live_threads(&self) -> usize {
        self.table.live()
    }

    /// Reserves a slot and identity for the idle thread (priority 0). The idle
    /// thread is exactly once; it stays `New` and invisible to the scheduler
    /// until `publish`. Pair with `discard_reserved` on failure.
    pub fn reserve_idle(&mut self, entry: usize, arg: usize) -> Result<Handle, Error> {
        if self.idle.is_some() {
            return Err(Error::IdleExists);
        }
        let h = self.table.create(Priority::IDLE, entry, arg)?;
        self.idle = Some(h);
        Ok(h)
    }

    /// Reserves a thread slot and identity for an ordinary thread. The thread
    /// is `New`: not queued, never selected, not counted as competition. The
    /// caller attaches whatever else it needs (a kernel stack) and then calls
    /// `publish`, or `discard_reserved` to roll back.
    pub fn reserve(&mut self, prio: Priority, entry: usize, arg: usize) -> Result<Handle, Error> {
        if prio == Priority::IDLE {
            return Err(Error::BadPriority);
        }
        self.table.create(prio, entry, arg)
    }

    /// `New -> Ready`: makes a reserved thread runnable. On failure the thread
    /// is still reserved (and must be discarded or retried).
    pub fn publish(&mut self, h: Handle) -> Result<(), Error> {
        let state = self.table.get(h)?.state();
        if state != State::New {
            // `make_ready` would also accept Running/Blocked/Sleeping; publication
            // is strictly the one-time New -> Ready step.
            return Err(Error::InvalidTransition {
                from: state,
                to: State::Ready,
            });
        }
        self.make_ready(h)?;
        self.check_preempt(h);
        Ok(())
    }

    /// Frees a reserved thread that was never published (creation rollback).
    pub fn discard_reserved(&mut self, h: Handle) -> Result<(), Error> {
        self.table.discard_new(h)?;
        if self.idle == Some(h) {
            self.idle = None;
        }
        Ok(())
    }

    /// Creates the always-runnable idle thread (priority 0). Exactly once.
    pub fn init_idle(&mut self, entry: usize, arg: usize) -> Result<Handle, Error> {
        let h = self.reserve_idle(entry, arg)?;
        if let Err(e) = self.publish(h) {
            let _ = self.discard_reserved(h);
            return Err(e);
        }
        Ok(h)
    }

    /// Creates a thread and makes it Ready. On failure nothing is left behind.
    pub fn spawn(&mut self, prio: Priority, entry: usize, arg: usize) -> Result<Handle, Error> {
        let h = self.reserve(prio, entry, arg)?;
        if let Err(e) = self.publish(h) {
            let _ = self.discard_reserved(h);
            return Err(e);
        }
        Ok(h)
    }

    /// Moves a thread to Ready and enqueues it. Queue insertion happens before
    /// the state change so a failure leaves the thread untouched.
    fn make_ready(&mut self, h: Handle) -> Result<(), Error> {
        let now = self.now;
        let t = self.table.get(h)?;
        if !t.state().can_become(State::Ready) {
            return Err(Error::InvalidTransition {
                from: t.state(),
                to: State::Ready,
            });
        }
        let level = t.effective_level();
        self.queues.push_back(h.index(), level)?;
        let t = self.table.get_mut(h)?;
        t.set_state(State::Ready)?;
        t.ready_since = now;
        Ok(())
    }

    fn check_preempt(&mut self, woken: Handle) {
        let (Some(cur), Ok(w)) = (self.current, self.table.get(woken)) else {
            return;
        };
        if let Ok(c) = self.table.get(cur)
            && w.effective_level() > c.effective_level()
        {
            self.need_resched = true;
        }
    }

    /// True when a non-idle thread is queued. The idle thread sits in the
    /// level-0 queue while a worker runs and must not count as competition.
    fn others_ready(&self) -> bool {
        let idle_queued = self.idle.is_some_and(|i| self.queues.contains(i.index()));
        self.queues.total() > usize::from(idle_queued)
    }

    /// Timer interrupt entry: advances time, expires sleepers, accounts CPU
    /// time and returns whether a reschedule is requested. It never switches.
    pub fn tick(&mut self) -> bool {
        self.now = self.now.saturating_add(1);
        self.expire_sleepers();
        if let Some(cur) = self.current
            && let Ok(t) = self.table.get_mut(cur)
        {
            t.cpu_ticks += 1;
            t.slice_left = t.slice_left.saturating_sub(1);
            let expired = t.slice_left == 0;
            let idle = t.is_idle();
            if (expired || idle) && self.others_ready() {
                self.need_resched = true;
            }
        }
        self.need_resched
    }

    fn expire_sleepers(&mut self) {
        for i in 0..N {
            let Some(h) = self.table.handle_at(i) else {
                continue;
            };
            let due = self
                .table
                .get(h)
                .is_ok_and(|t| t.state() == State::Sleeping && t.deadline <= self.now);
            if due && self.make_ready(h).is_ok() {
                if let Ok(t) = self.table.get_mut(h) {
                    t.wake_reason = WakeReason::Timeout;
                    t.block_reason = BlockReason::None;
                }
                self.check_preempt(h);
            }
        }
    }

    fn age(&mut self) -> Result<(), Error> {
        for i in 0..N {
            let Some(h) = self.table.handle_at(i) else {
                continue;
            };
            let t = self.table.get(h)?;
            if t.state() != State::Ready
                || t.is_idle()
                || self.now.saturating_sub(t.ready_since) < AGING_TICKS
                || t.effective_level() >= PRIORITY_LEVELS - 1
            {
                continue;
            }
            self.queues.remove(i)?;
            let now = self.now;
            let t = self.table.get_mut(h)?;
            t.boost += 1;
            t.ready_since = now;
            let level = t.effective_level();
            self.queues.push_back(i, level)?;
        }
        Ok(())
    }

    /// Chooses the next thread. A still-Running current thread is re-queued
    /// (yield/preempt); Blocked/Sleeping/Terminated ones are not. The caller
    /// performs the architecture switch and then calls `switch_complete`.
    pub fn schedule(&mut self) -> Result<Switch, Error> {
        self.need_resched = false;
        self.age()?;
        let prev = self.current;
        if let Some(p) = prev
            && self.table.get(p)?.state() == State::Running
        {
            self.make_ready(p)?;
        }
        let (index, _) = self.queues.pop_front_highest().ok_or(Error::NoRunnable)?;
        let next = self.table.handle_at(index).ok_or(Error::Corrupt)?;
        let switching = prev != Some(next);
        let t = self.table.get_mut(next)?;
        if switching && t.on_cpu {
            return Err(Error::Corrupt);
        }
        t.set_state(State::Running)?;
        t.boost = 0;
        t.slice_left = SLICE_TICKS;
        t.dispatches += 1;
        t.on_cpu = true;
        self.current = Some(next);
        Ok(Switch { prev, next })
    }

    /// Called on the new thread's context once the switch away from `prev`
    /// finished. Only now may a terminated `prev` be reaped.
    pub fn switch_complete(&mut self, prev: Handle) -> Result<(), Error> {
        if self.current == Some(prev) {
            return Err(Error::NotCurrent);
        }
        self.table.get_mut(prev)?.on_cpu = false;
        Ok(())
    }

    fn running_current(&self) -> Result<Handle, Error> {
        let cur = self.current.ok_or(Error::NoCurrent)?;
        let t = self.table.get(cur)?;
        if t.state() != State::Running {
            return Err(Error::NotCurrent);
        }
        if t.is_idle() {
            return Err(Error::IsIdle);
        }
        Ok(cur)
    }

    pub fn yield_now(&mut self) -> Result<Switch, Error> {
        self.schedule()
    }

    /// Blocks the current thread. If a wake-up arrived earlier (lost-wakeup
    /// race), it is consumed and the thread keeps running instead.
    pub fn block_current(&mut self, reason: BlockReason) -> Result<BlockOutcome, Error> {
        let cur = self.running_current()?;
        let t = self.table.get_mut(cur)?;
        if t.wake_pending {
            t.wake_pending = false;
            return Ok(BlockOutcome::Continue);
        }
        t.set_state(State::Blocked)?;
        t.block_reason = reason;
        t.wake_reason = WakeReason::None;
        Ok(BlockOutcome::Blocked)
    }

    /// Blocks until `target` terminates. Returns `Continue` if it already has.
    pub fn join_wait(&mut self, target: Handle) -> Result<BlockOutcome, Error> {
        let cur = self.running_current()?;
        if target == cur {
            return Err(Error::NotCurrent);
        }
        if self.table.get(target)?.state() == State::Terminated {
            return Ok(BlockOutcome::Continue);
        }
        self.block_current(BlockReason::Join(target))
    }

    /// Sleeps for `ticks` (>= deadline, never early). Zero is an immediate
    /// yield point. A pending wake-up targeted at a different wait is dropped.
    pub fn sleep_current(&mut self, ticks: u64) -> Result<SleepOutcome, Error> {
        let cur = self.running_current()?;
        let deadline = self.now.checked_add(ticks).ok_or(Error::DeadlineOverflow)?;
        let t = self.table.get_mut(cur)?;
        t.wake_pending = false;
        if ticks == 0 {
            return Ok(SleepOutcome::Immediate);
        }
        t.set_state(State::Sleeping)?;
        t.deadline = deadline;
        t.block_reason = BlockReason::Sleep;
        t.wake_reason = WakeReason::None;
        Ok(SleepOutcome::Sleeping)
    }

    /// Wakes a Blocked or Sleeping thread (cancelling a sleep). A Running
    /// target records a pending wake-up so the wake cannot be lost.
    pub fn wake(&mut self, h: Handle) -> Result<WakeOutcome, Error> {
        let state = self.table.get(h)?.state();
        match state {
            State::Blocked | State::Sleeping => {
                self.make_ready(h)?;
                let t = self.table.get_mut(h)?;
                t.wake_reason = if state == State::Sleeping {
                    WakeReason::Cancelled
                } else {
                    WakeReason::Event
                };
                t.block_reason = BlockReason::None;
                self.check_preempt(h);
                Ok(WakeOutcome::Woken)
            }
            State::Running => {
                self.table.get_mut(h)?.wake_pending = true;
                Ok(WakeOutcome::Pending)
            }
            State::Ready => Ok(WakeOutcome::AlreadyReady),
            State::New | State::Terminated => Err(Error::InvalidTransition {
                from: state,
                to: State::Ready,
            }),
        }
    }

    /// The current thread exits. The caller must then invoke `schedule()`.
    pub fn terminate_current(&mut self, status: i32) -> Result<(), Error> {
        let cur = self.running_current()?;
        let t = self.table.get_mut(cur)?;
        t.set_state(State::Terminated)?;
        t.exit_status = Some(status);
        self.wake_joiners(cur);
        Ok(())
    }

    /// Terminates a non-running thread. Running threads must exit themselves
    /// (cross-CPU kill needs the event mechanism, requirement 18).
    pub fn kill(&mut self, h: Handle, status: i32) -> Result<(), Error> {
        let t = self.table.get(h)?;
        if t.is_idle() {
            return Err(Error::IsIdle);
        }
        match t.state() {
            State::Running => return Err(Error::IsRunning),
            State::Terminated => {
                return Err(Error::InvalidTransition {
                    from: State::Terminated,
                    to: State::Terminated,
                });
            }
            State::Ready => self.queues.remove(h.index())?,
            _ => {}
        }
        let t = self.table.get_mut(h)?;
        t.set_state(State::Terminated)?;
        t.exit_status = Some(status);
        t.block_reason = BlockReason::None;
        self.wake_joiners(h);
        Ok(())
    }

    fn wake_joiners(&mut self, target: Handle) {
        for i in 0..N {
            let Some(h) = self.table.handle_at(i) else {
                continue;
            };
            let waiting = self.table.get(h).is_ok_and(|t| {
                t.state() == State::Blocked && t.block_reason == BlockReason::Join(target)
            });
            if waiting && self.make_ready(h).is_ok() {
                if let Ok(t) = self.table.get_mut(h) {
                    t.wake_reason = WakeReason::Event;
                    t.block_reason = BlockReason::None;
                }
                self.check_preempt(h);
            }
        }
    }

    /// Changes base priority. Takes effect immediately for a Ready thread and
    /// at the next dispatch/requeue for a Running one.
    pub fn set_priority(&mut self, h: Handle, prio: Priority) -> Result<(), Error> {
        if prio == Priority::IDLE || self.table.get(h)?.is_idle() {
            return Err(Error::IsIdle);
        }
        let state = self.table.get(h)?.state();
        if state == State::Ready {
            self.queues.remove(h.index())?;
        }
        let now = self.now;
        let t = self.table.get_mut(h)?;
        t.priority = prio;
        t.boost = 0;
        if state == State::Ready {
            t.ready_since = now;
            let level = t.effective_level();
            self.queues.push_back(h.index(), level)?;
            self.check_preempt(h);
        } else if state == State::Running
            && self
                .queues
                .highest_level()
                .is_some_and(|l| l > prio.level())
        {
            self.need_resched = true;
        }
        Ok(())
    }

    /// Succeeds only if `reap(h)` would: the thread is `Terminated`, is not the
    /// current thread and has finished switching away (`on_cpu` is false).
    /// Changes nothing, so callers can do fallible work (such as scrubbing the
    /// stack) between this check and `reap` without risking a half-reaped thread.
    pub fn reap_ready(&self, h: Handle) -> Result<(), Error> {
        let t = self.table.get(h)?;
        if self.current == Some(h) {
            return Err(Error::StillOnCpu);
        }
        if t.state() != State::Terminated {
            return Err(Error::NotTerminated);
        }
        if t.on_cpu() {
            return Err(Error::StillOnCpu);
        }
        Ok(())
    }

    /// Reclaims a terminated thread; refused while it is still on the CPU.
    pub fn reap(&mut self, h: Handle) -> Result<i32, Error> {
        if self.current == Some(h) {
            return Err(Error::StillOnCpu);
        }
        self.table.reap(h)
    }

    /// Handle of the live thread stored in table slot `index`, if any.
    pub fn handle_at(&self, index: usize) -> Option<Handle> {
        self.table.handle_at(index)
    }

    /// Number of table slots (capacity).
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Structural self-check, valid at quiescent points (after `schedule()`
    /// and `switch_complete()`, never between a state change and `schedule()`).
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        let mut ready = 0usize;
        let mut running = 0usize;
        for i in 0..N {
            let Some(h) = self.table.handle_at(i) else {
                if self.queues.contains(i) {
                    return Err("queued slot has no thread");
                }
                continue;
            };
            let t = self
                .table
                .get(h)
                .map_err(|_| "handle_at gave stale handle")?;
            let queued = self.queues.contains(i);
            match t.state() {
                State::Ready => {
                    ready += 1;
                    if !queued || self.queues.level_of(i) != Some(t.effective_level()) {
                        return Err("ready thread not queued at its level");
                    }
                }
                State::Running => {
                    running += 1;
                    if queued || !t.on_cpu() || self.current != Some(h) {
                        return Err("running thread inconsistent");
                    }
                }
                _ if queued => return Err("non-ready thread queued"),
                _ => {}
            }
        }
        if ready != self.queues.total() {
            return Err("queue count differs from ready threads");
        }
        if running > 1 || (running == 1) != self.current.is_some() {
            return Err("current/running mismatch");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{State::*, WakeReason};

    type S = Scheduler<16>;

    fn p(l: u8) -> Priority {
        Priority::new(l).unwrap()
    }

    /// Performs schedule() plus the arch layer's switch_complete().
    fn run<const N: usize>(s: &mut Scheduler<N>) -> Switch {
        let sw = s.schedule().unwrap();
        if sw.is_switch()
            && let Some(prev) = sw.prev
        {
            s.switch_complete(prev).unwrap();
        }
        s.check_invariants().unwrap();
        sw
    }

    fn setup() -> S {
        let mut s = S::new();
        s.init_idle(0, 0).unwrap();
        s
    }

    #[test]
    fn idle_runs_when_nothing_else_is_runnable() {
        let mut s = setup();
        assert_eq!(run(&mut s).next, s.idle().unwrap());
        assert_eq!(run(&mut s).next, s.idle().unwrap());
        assert_eq!(s.init_idle(0, 0), Err(Error::IdleExists));
    }

    #[test]
    fn schedule_without_idle_or_threads_is_an_error() {
        let mut s = S::new();
        assert_eq!(s.schedule().err(), Some(Error::NoRunnable));
    }

    #[test]
    fn idle_cannot_block_sleep_or_die() {
        let mut s = setup();
        run(&mut s);
        assert_eq!(s.block_current(BlockReason::Event(1)), Err(Error::IsIdle));
        assert_eq!(s.sleep_current(5), Err(Error::IsIdle));
        assert_eq!(s.terminate_current(0), Err(Error::IsIdle));
        assert_eq!(s.kill(s.idle().unwrap(), 0), Err(Error::IsIdle));
        assert_eq!(s.spawn(Priority::IDLE, 0, 0), Err(Error::BadPriority));
    }

    #[test]
    fn equal_priority_is_round_robin() {
        let mut s = setup();
        let t: Vec<_> = (0..3).map(|_| s.spawn(p(4), 0, 0).unwrap()).collect();
        let order: Vec<_> = (0..7).map(|_| run(&mut s).next).collect();
        assert_eq!(order, [t[0], t[1], t[2], t[0], t[1], t[2], t[0]]);
    }

    #[test]
    fn higher_priority_runs_first_and_idle_is_last() {
        let mut s = setup();
        let lo = s.spawn(p(1), 0, 0).unwrap();
        let hi = s.spawn(p(6), 0, 0).unwrap();
        assert_eq!(run(&mut s).next, hi);
        // hi yields but is the only top-level thread: it keeps running.
        assert!(!run(&mut s).is_switch());
        s.block_current(BlockReason::Event(1)).unwrap();
        assert_eq!(run(&mut s).next, lo);
        s.block_current(BlockReason::Event(2)).unwrap();
        assert_eq!(run(&mut s).next, s.idle().unwrap());
    }

    #[test]
    fn blocked_thread_is_never_selected_until_woken() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        let b = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(run(&mut s).next, a);
        assert_eq!(
            s.block_current(BlockReason::Event(9)),
            Ok(BlockOutcome::Blocked)
        );
        for _ in 0..5 {
            assert_eq!(run(&mut s).next, b);
        }
        assert_eq!(s.wake(a), Ok(WakeOutcome::Woken));
        assert_eq!(s.thread(a).unwrap().wake_reason(), WakeReason::Event);
        assert_eq!(run(&mut s).next, a);
    }

    #[test]
    fn wake_before_block_is_not_lost() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        run(&mut s);
        assert_eq!(s.wake(a), Ok(WakeOutcome::Pending));
        assert_eq!(
            s.block_current(BlockReason::Event(1)),
            Ok(BlockOutcome::Continue)
        );
        assert_eq!(s.thread(a).unwrap().state(), Running);
        // Pending flag is consumed exactly once.
        assert_eq!(
            s.block_current(BlockReason::Event(1)),
            Ok(BlockOutcome::Blocked)
        );
    }

    #[test]
    fn wake_of_ready_is_idempotent_and_dead_threads_are_rejected() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(s.wake(a), Ok(WakeOutcome::AlreadyReady));
        s.kill(a, 1).unwrap();
        assert!(matches!(s.wake(a), Err(Error::InvalidTransition { .. })));
    }

    #[test]
    fn sleep_wakes_at_or_after_deadline_and_not_before() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        run(&mut s);
        assert_eq!(s.sleep_current(5), Ok(SleepOutcome::Sleeping));
        run(&mut s);
        for _ in 0..4 {
            s.tick();
            assert_eq!(s.thread(a).unwrap().state(), Sleeping);
        }
        assert!(
            s.tick(),
            "waking a higher-level thread than idle requests resched"
        );
        assert_eq!(s.thread(a).unwrap().state(), Ready);
        assert_eq!(s.thread(a).unwrap().wake_reason(), WakeReason::Timeout);
        assert_eq!(run(&mut s).next, a);
    }

    #[test]
    fn zero_sleep_is_immediate_and_overflow_is_rejected() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        run(&mut s);
        assert_eq!(s.sleep_current(0), Ok(SleepOutcome::Immediate));
        assert_eq!(s.thread(a).unwrap().state(), Running);
        s.tick(); // now = 1, so now + u64::MAX overflows
        assert_eq!(s.sleep_current(u64::MAX), Err(Error::DeadlineOverflow));
        assert_eq!(s.thread(a).unwrap().state(), Running);
    }

    #[test]
    fn sleep_can_be_cancelled_and_is_not_queued_while_asleep() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        run(&mut s);
        s.sleep_current(1000).unwrap();
        run(&mut s);
        assert_eq!(s.thread(a).unwrap().state(), Sleeping);
        assert_eq!(s.wake(a), Ok(WakeOutcome::Woken));
        assert_eq!(s.thread(a).unwrap().wake_reason(), WakeReason::Cancelled);
    }

    #[test]
    fn tick_requests_but_never_performs_a_switch() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        let b = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(run(&mut s).next, a);
        for _ in 0..SLICE_TICKS - 1 {
            assert!(!s.tick());
        }
        assert!(s.tick(), "slice expired with another thread ready");
        assert_eq!(s.current(), Some(a), "tick alone must not change current");
        assert_eq!(run(&mut s).next, b);
        assert!(!s.need_resched());
    }

    #[test]
    fn expired_slice_alone_does_not_request_resched_without_competition() {
        let mut s = setup();
        s.spawn(p(4), 0, 0).unwrap();
        run(&mut s);
        // Only the idle thread is queued; it is not competition for a worker.
        for _ in 0..SLICE_TICKS * 3 {
            assert!(!s.tick());
        }
        assert!(
            !run(&mut s).is_switch(),
            "an explicit yield re-picks the worker"
        );
    }

    #[test]
    fn waking_higher_priority_thread_requests_preemption() {
        let mut s = setup();
        let lo = s.spawn(p(2), 0, 0).unwrap();
        let hi = s.spawn(p(6), 0, 0).unwrap();
        assert_eq!(run(&mut s).next, hi);
        s.block_current(BlockReason::Event(1)).unwrap();
        assert_eq!(run(&mut s).next, lo);
        assert!(!s.need_resched());
        s.wake(hi).unwrap();
        assert!(s.need_resched());
        assert_eq!(run(&mut s).next, hi);
    }

    #[test]
    fn spawning_higher_priority_requests_preemption() {
        let mut s = setup();
        s.spawn(p(2), 0, 0).unwrap();
        run(&mut s);
        assert!(!s.need_resched());
        s.spawn(p(5), 0, 0).unwrap();
        assert!(s.need_resched());
    }

    #[test]
    fn low_priority_thread_is_not_starved_by_a_busy_high_priority_thread() {
        let mut s = setup();
        let hi = s.spawn(p(7), 0, 0).unwrap();
        let lo = s.spawn(p(1), 0, 0).unwrap();
        run(&mut s);
        let mut waited = 0u64;
        let bound =
            PRIORITY_LEVELS as u64 * (AGING_TICKS + SLICE_TICKS as u64) + SLICE_TICKS as u64;
        loop {
            if s.tick() {
                run(&mut s);
            }
            waited += 1;
            if s.current() == Some(lo) {
                break;
            }
            assert!(waited <= bound, "lo starved for {waited} ticks");
        }
        assert!(s.thread(hi).unwrap().cpu_ticks() > 0);
    }

    #[test]
    fn low_priority_thread_keeps_getting_turns_over_a_long_run() {
        // Boost is dropped on dispatch, so the pattern repeats rather than latching.
        let mut s = setup();
        s.spawn(p(7), 0, 0).unwrap();
        let lo = s.spawn(p(1), 0, 0).unwrap();
        run(&mut s);
        for _ in 0..400 {
            if s.tick() {
                run(&mut s);
            }
        }
        let t = s.thread(lo).unwrap();
        assert!(t.dispatches() >= 2, "lo must keep getting turns");
        assert!(t.cpu_ticks() > 0);
    }

    #[test]
    fn terminate_cannot_be_reaped_until_switch_completes() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        let b = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(run(&mut s).next, a);
        s.terminate_current(42).unwrap();
        assert_eq!(s.reap(a), Err(Error::StillOnCpu));
        let sw = s.schedule().unwrap();
        assert_eq!((sw.prev, sw.next), (Some(a), b));
        assert_eq!(
            s.reap(a),
            Err(Error::StillOnCpu),
            "switch not completed yet"
        );
        s.switch_complete(a).unwrap();
        s.check_invariants().unwrap();
        assert_eq!(s.reap(a), Ok(42));
        assert_eq!(s.thread(a).err(), Some(Error::Stale));
    }

    #[test]
    fn join_wakes_when_target_exits() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        let b = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(run(&mut s).next, a);
        assert_eq!(s.join_wait(b), Ok(BlockOutcome::Blocked));
        assert_eq!(run(&mut s).next, b);
        s.terminate_current(3).unwrap();
        assert_eq!(s.thread(a).unwrap().state(), Ready);
        assert_eq!(run(&mut s).next, a);
        assert_eq!(s.join_wait(b), Ok(BlockOutcome::Continue));
        assert_eq!(s.reap(b), Ok(3));
        assert_eq!(s.join_wait(a), Err(Error::NotCurrent), "no self-join");
    }

    #[test]
    fn kill_removes_ready_thread_from_queue_and_refuses_running() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        let b = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(run(&mut s).next, a);
        assert_eq!(s.kill(a, 0), Err(Error::IsRunning));
        s.kill(b, 9).unwrap();
        s.check_invariants().unwrap();
        assert_eq!(s.reap(b), Ok(9));
        assert!(matches!(s.kill(b, 0), Err(Error::Stale)));
    }

    #[test]
    fn set_priority_requeues_ready_thread() {
        let mut s = setup();
        let a = s.spawn(p(2), 0, 0).unwrap();
        let b = s.spawn(p(2), 0, 0).unwrap();
        s.set_priority(b, p(6)).unwrap();
        s.check_invariants().unwrap();
        assert_eq!(run(&mut s).next, b);
        s.set_priority(b, p(1)).unwrap();
        assert!(s.need_resched(), "running thread lowered below a ready one");
        assert_eq!(run(&mut s).next, a);
        assert_eq!(s.set_priority(a, Priority::IDLE), Err(Error::IsIdle));
    }

    #[test]
    fn spawn_failure_leaves_no_residue() {
        let mut s = Scheduler::<3>::new();
        s.init_idle(0, 0).unwrap();
        s.spawn(p(4), 0, 0).unwrap();
        s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(s.spawn(p(4), 0, 0), Err(Error::NoSlots));
        assert_eq!(s.live_threads(), 3);
        s.check_invariants().unwrap();
    }

    #[test]
    fn stale_handles_are_rejected_everywhere() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        s.kill(a, 0).unwrap();
        s.reap(a).unwrap();
        assert_eq!(s.wake(a), Err(Error::Stale));
        assert_eq!(s.kill(a, 0), Err(Error::Stale));
        assert_eq!(s.set_priority(a, p(3)), Err(Error::Stale));
        assert_eq!(s.reap(a), Err(Error::Stale));
        assert_eq!(s.switch_complete(a), Err(Error::Stale));
    }

    /// Deterministic randomized model test: arbitrary operation sequences,
    /// invariants checked after every step. Host-side model test only.
    fn stress(seed: u64, steps: usize) {
        let mut s = Scheduler::<12>::new();
        s.init_idle(0, 0).unwrap();
        let mut handles: Vec<Handle> = Vec::new();
        let mut x = seed;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        run(&mut s);
        for _ in 0..steps {
            let r = rnd();
            let pick = |h: &Vec<Handle>, r: u64| {
                if h.is_empty() {
                    None
                } else {
                    Some(h[(r >> 16) as usize % h.len()])
                }
            };
            let res: Result<(), Error> = match r % 11 {
                0 | 1 => match s.spawn(p(1 + (r >> 8) as u8 % 7), 0, 0) {
                    Ok(h) => {
                        handles.push(h);
                        Ok(())
                    }
                    Err(e) => Err(e),
                },
                2 => s.block_current(BlockReason::Event(1)).map(|_| ()),
                3 => s.sleep_current((r >> 8) % 30).map(|_| ()),
                4 | 5 => pick(&handles, r).map_or(Ok(()), |h| s.wake(h).map(|_| ())),
                6 => pick(&handles, r).map_or(Ok(()), |h| s.kill(h, 1)),
                7 => s.terminate_current(2),
                8 => pick(&handles, r)
                    .map_or(Ok(()), |h| s.set_priority(h, p(1 + (r >> 8) as u8 % 7))),
                9 => pick(&handles, r).map_or(Ok(()), |h| s.reap(h).map(|_| ())),
                _ => {
                    s.tick();
                    Ok(())
                }
            };
            if let Err(e) = res {
                assert!(
                    !matches!(e, Error::Corrupt | Error::Queue(_) | Error::NoRunnable),
                    "internal error {e:?}"
                );
            }
            // Return to a quiescent point the way the kernel would.
            let cur_state = s.current().map(|c| s.thread(c).unwrap().state());
            if cur_state != Some(Running) || s.need_resched() || r % 5 == 0 {
                run(&mut s);
            }
            s.check_invariants().unwrap();
        }
    }

    #[test]
    fn stress_random_operations_preserve_invariants() {
        for seed in [1u64, 7, 0xdead_beef, 0x1234_5678_9abc_def1] {
            stress(seed, 60_000);
        }
    }

    #[test]
    fn many_runnable_threads_share_the_cpu_fairly() {
        let mut s = Scheduler::<40>::new();
        s.init_idle(0, 0).unwrap();
        let hs: Vec<_> = (0..32).map(|_| s.spawn(p(4), 0, 0).unwrap()).collect();
        run(&mut s);
        for _ in 0..32 * SLICE_TICKS as usize * 50 {
            if s.tick() {
                run(&mut s);
            }
        }
        let ticks: Vec<_> = hs
            .iter()
            .map(|&h| s.thread(h).unwrap().cpu_ticks())
            .collect();
        let (min, max) = (ticks.iter().min().unwrap(), ticks.iter().max().unwrap());
        assert!(max - min <= SLICE_TICKS as u64, "unfair: {min}..{max}");
    }

    #[test]
    fn many_blocked_threads_cost_nothing_and_all_wake() {
        let mut s = Scheduler::<40>::new();
        s.init_idle(0, 0).unwrap();
        let hs: Vec<_> = (0..32).map(|_| s.spawn(p(4), 0, 0).unwrap()).collect();
        for _ in 0..32 {
            run(&mut s);
            s.block_current(BlockReason::Event(7)).unwrap();
        }
        assert_eq!(run(&mut s).next, s.idle().unwrap());
        for &h in &hs {
            assert_eq!(s.wake(h), Ok(WakeOutcome::Woken));
        }
        s.check_invariants().unwrap();
        for &h in &hs {
            assert_eq!(run(&mut s).next, h);
        }
    }

    #[test]
    fn repeated_create_destroy_reuses_capacity_without_leaks() {
        let mut s = Scheduler::<4>::new();
        s.init_idle(0, 0).unwrap();
        run(&mut s);
        for round in 0..10_000 {
            let h = s.spawn(p(3), 0, 0).unwrap();
            let sw = run(&mut s);
            assert_eq!(sw.next, h);
            s.terminate_current(round).unwrap();
            run(&mut s);
            assert_eq!(s.reap(h), Ok(round));
        }
        assert_eq!(s.live_threads(), 1);
    }

    #[test]
    fn reserved_thread_is_invisible_until_published() {
        let mut s = setup();
        let a = s.reserve(p(6), 0, 0).unwrap();
        assert_eq!(s.thread(a).unwrap().state(), New);
        assert_eq!(s.live_threads(), 2);
        s.check_invariants().unwrap();
        // Not selectable, not competition, cannot be woken or killed into life.
        assert_eq!(run(&mut s).next, s.idle().unwrap());
        assert!(!s.tick() && !s.need_resched());
        assert!(matches!(s.wake(a), Err(Error::InvalidTransition { .. })));
        s.publish(a).unwrap();
        assert_eq!(s.thread(a).unwrap().state(), Ready);
        assert!(
            s.need_resched(),
            "published higher-priority thread preempts idle"
        );
        assert_eq!(run(&mut s).next, a);
        assert!(matches!(s.publish(a), Err(Error::InvalidTransition { .. })));
    }

    #[test]
    fn discard_reserved_frees_slot_and_invalidates_handle() {
        let mut s = Scheduler::<2>::new();
        s.init_idle(0, 0).unwrap();
        let a = s.reserve(p(3), 0, 0).unwrap();
        assert_eq!(s.reserve(p(3), 0, 0), Err(Error::NoSlots));
        s.discard_reserved(a).unwrap();
        assert_eq!(s.thread(a).err(), Some(Error::Stale));
        assert_eq!(
            s.discard_reserved(a),
            Err(Error::Stale),
            "no double discard"
        );
        let b = s.reserve(p(3), 0, 0).unwrap();
        assert_eq!(a.index(), b.index());
        s.publish(b).unwrap();
        // A published thread can no longer be "discarded".
        assert!(matches!(
            s.discard_reserved(b),
            Err(Error::InvalidTransition { .. })
        ));
        s.check_invariants().unwrap();
    }

    #[test]
    fn reserve_idle_rules_and_rollback() {
        let mut s = S::new();
        assert_eq!(s.reserve(Priority::IDLE, 0, 0), Err(Error::BadPriority));
        let i = s.reserve_idle(0, 0).unwrap();
        assert_eq!(s.idle(), Some(i));
        assert_eq!(s.reserve_idle(0, 0), Err(Error::IdleExists));
        s.discard_reserved(i).unwrap();
        assert_eq!(s.idle(), None, "discarding the reserved idle clears it");
        let i2 = s.reserve_idle(0, 0).unwrap();
        s.publish(i2).unwrap();
        assert_eq!(run(&mut s).next, i2);
    }

    #[test]
    fn reap_ready_agrees_with_reap_without_changing_anything() {
        let mut s = setup();
        let a = s.spawn(p(4), 0, 0).unwrap();
        let b = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(s.reap_ready(b), Err(Error::NotTerminated));
        assert_eq!(run(&mut s).next, a);
        assert_eq!(s.reap_ready(a), Err(Error::StillOnCpu), "current thread");
        s.terminate_current(9).unwrap();
        assert_eq!(s.reap_ready(a), Err(Error::StillOnCpu));
        let sw = s.schedule().unwrap();
        assert_eq!(
            s.reap_ready(a),
            Err(Error::StillOnCpu),
            "switch not complete"
        );
        s.switch_complete(sw.prev.unwrap()).unwrap();
        assert_eq!(s.reap_ready(a), Ok(()));
        assert_eq!(s.reap_ready(a), Ok(()), "check is idempotent");
        assert_eq!(s.reap(a), Ok(9));
        assert_eq!(s.reap_ready(a), Err(Error::Stale));
        s.check_invariants().unwrap();
    }
}
