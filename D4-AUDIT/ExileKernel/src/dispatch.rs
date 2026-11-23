//! Scheduler dispatch: connects `Scheduler` (policy, thread lifecycle) and
//! `KThreadStacks` (stacks, saved stack pointers) to the architecture's
//! context switch. SPDX-License-Identifier: Apache-2.0
//!
//! One authority: the `Scheduler` owns the only `ThreadTable`, all thread
//! states, queues and `on_cpu`. `KThreadStacks` owns only stack metadata keyed by
//! `Handle`. `Kernel` just owns both and sequences them.
//!
//! # The switch protocol (three separate steps)
//! 1. `plan_switch` (bookkeeping, under the lock): `Scheduler::schedule` picks
//!    the next thread; the outgoing thread stays `on_cpu`; the plan holds where to
//!    save the outgoing stack pointer and which stack pointer to load. The
//!    outgoing thread is recorded as `pending_prev`.
//! 2. `switch::switch_context` (assembly, lock RELEASED, interrupts disabled).
//! 3. `complete_switch` (bookkeeping, under the lock, executed on the INCOMING
//!    thread's stack): `Scheduler::switch_complete(prev)` clears `prev.on_cpu`.
//!    A resumed thread runs this right after `switch_context` returns; a new
//!    thread runs it from the trampoline's first-run hook.
//!
//! Only after step 3 can a terminated thread be reaped and its stack scrubbed and
//! freed, so a stack is never reclaimed while code is still running on it.
//!
//! # Interrupts and locking (single CPU only)
//! The scheduler lock is an `IrqSpinLock`: taking it clears IF and releasing it
//! restores the previous state, so an interrupt handler can never spin on a lock
//! its own interrupted code holds. The lock is NEVER held across the switch.
//! Instead interrupts stay disabled from `plan_switch` until the incoming thread's
//! `complete_switch`, which is what makes the unlocked window safe on one CPU.
//! `reschedule`, `block_current`, `sleep_current` and `join_wait` therefore run
//! their whole sequence (state change + switch) with interrupts off and restore
//! the caller's previous IF state when they resume.
//!
//! Interrupt-driven preemption: the timer handler only counts ticks and asks the
//! scheduler for a reschedule (`tick` sets `need_resched`). After the handler has
//! left interrupt context, `irq_exit_reschedule` (the interrupt-EXIT step)
//! decides with `preempt::irq_exit_action` and, if allowed, calls `reschedule` on
//! the interrupted thread's own kernel stack. The interrupted thread's full
//! register and SSE state sits in its trap frame on that stack and is restored by
//! `iretq` when it is next switched to. A request that arrives while preemption
//! is disabled (or while a switch is pending) stays pending in the scheduler.
//!
//! A new thread starts with IF clear (it inherits the switcher's state); its
//! first-run hook must enable interrupts once the switch bookkeeping is done.
//! There is NO SMP support: the unlocked window and the single `pending_prev`
//! slot are not CPU-safe.
use crate::irq::IrqSpinLock;
use crate::kstack::StackPool;
use crate::kthread::{Config, Error, KThreadStacks, StackMemory, ThreadParams};
use crate::sched::{BlockOutcome, Scheduler, SleepOutcome, WakeOutcome};
use crate::thread::{BlockReason, Handle, State, ThreadId};

/// What the architecture layer must do for one switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchPlan {
    /// Where the switch routine stores the outgoing stack pointer.
    pub prev_sp_slot: *mut u64,
    /// Stack pointer of the thread to resume.
    pub next_sp: u64,
}

pub struct Kernel<const N: usize, M: StackMemory> {
    sched: Scheduler<N>,
    stacks: KThreadStacks<N>,
    mem: M,
    pending_prev: Option<Handle>,
    /// Receives the stack pointer of the bootstrap context (never a thread).
    boot_sp: u64,
    switches: u64,
}

impl<const N: usize, M: StackMemory> Kernel<N, M> {
    pub fn new(cfg: Config, pool: StackPool, mem: M) -> Result<Self, Error> {
        Ok(Self {
            sched: Scheduler::new(),
            stacks: KThreadStacks::new(cfg, pool)?,
            mem,
            pending_prev: None,
            boot_sp: 0,
            switches: 0,
        })
    }

    pub fn sched(&self) -> &Scheduler<N> {
        &self.sched
    }
    pub fn sched_mut(&mut self) -> &mut Scheduler<N> {
        &mut self.sched
    }
    pub fn stacks(&self) -> &KThreadStacks<N> {
        &self.stacks
    }
    pub fn switch_count(&self) -> u64 {
        self.switches
    }
    pub fn boot_sp(&self) -> u64 {
        self.boot_sp
    }
    pub fn switch_pending(&self) -> bool {
        self.pending_prev.is_some()
    }

    pub fn create(&mut self, p: ThreadParams) -> Result<Handle, Error> {
        self.stacks.create(&mut self.sched, &mut self.mem, p)
    }

    pub fn create_idle(&mut self, entry: u64, arg: u64) -> Result<Handle, Error> {
        self.stacks
            .create_idle(&mut self.sched, &mut self.mem, entry, arg)
    }

    /// Scrubs and reclaims a terminated, off-CPU thread. See `KThreadStacks::destroy`.
    pub fn destroy(&mut self, h: Handle) -> Result<i32, Error> {
        self.stacks.destroy(&mut self.sched, &mut self.mem, h)
    }

    /// Reclaims every terminated thread that has finished switching away and
    /// reports `(id, exit status)` for each. Threads still on their stack are
    /// skipped (they are picked up on a later call).
    pub fn reap_terminated(&mut self, mut report: impl FnMut(ThreadId, i32)) -> usize {
        let mut n = 0;
        for i in 0..self.sched.capacity() {
            let Some(h) = self.sched.handle_at(i) else {
                continue;
            };
            let Ok(t) = self.sched.thread(h) else {
                continue;
            };
            if t.state() != State::Terminated || self.sched.reap_ready(h).is_err() {
                continue;
            }
            let id = t.id();
            if let Ok(status) = self.destroy(h) {
                report(id, status);
                n += 1;
            }
        }
        n
    }

    /// Step 1: choose the next thread. `Ok(None)` means the same thread keeps
    /// running (no switch). Refuses to plan while a previous switch is still
    /// pending, and then changes nothing.
    pub fn plan_switch(&mut self) -> Result<Option<SwitchPlan>, Error> {
        if self.pending_prev.is_some() {
            return Err(Error::SwitchInProgress);
        }
        let sw = self.sched.schedule()?;
        if !sw.is_switch() {
            return Ok(None);
        }
        let next_sp = self.stacks.saved_sp(sw.next)?;
        let prev_sp_slot = match sw.prev {
            Some(p) => self.stacks.saved_sp_slot(p)?,
            None => &mut self.boot_sp as *mut u64,
        };
        self.pending_prev = sw.prev;
        self.switches += 1;
        Ok(Some(SwitchPlan {
            prev_sp_slot,
            next_sp,
        }))
    }

    /// Step 3: the switch away from the previous thread has finished. A no-op
    /// when nothing is pending (first dispatch from the bootstrap context).
    pub fn complete_switch(&mut self) -> Result<(), Error> {
        if let Some(prev) = self.pending_prev.take() {
            self.sched.switch_complete(prev)?;
        }
        Ok(())
    }

    /// Scheduler and stack accounting must agree. Call at quiescent points.
    pub fn check_accounting(&self) -> Result<(), &'static str> {
        self.stacks.check_accounting(&self.sched)
    }

    /// Slot for the current thread's saved stack pointer (host tests use it to
    /// return to the bootstrap context).
    #[cfg(test)]
    pub(crate) fn current_sp_slot(&mut self) -> Result<*mut u64, Error> {
        let h = self.sched.current().ok_or(Error::Corrupt)?;
        self.stacks.saved_sp_slot(h)
    }
}

/// A `Kernel` behind the scheduler lock, usable as a `static`.
pub struct KernelSlot<const N: usize, M: StackMemory> {
    inner: IrqSpinLock<Option<Kernel<N, M>>>,
}

impl<const N: usize, M: StackMemory> Default for KernelSlot<N, M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, M: StackMemory> KernelSlot<N, M> {
    pub const fn new() -> Self {
        Self {
            inner: IrqSpinLock::new(None),
        }
    }

    /// Installs (or replaces) the kernel. Call before any thread runs; replacing
    /// a kernel with live switched-out threads would strand their stacks.
    pub fn install(&self, k: Kernel<N, M>) {
        *self.inner.lock() = Some(k);
    }

    pub fn is_installed(&self) -> bool {
        self.inner.lock().is_some()
    }

    /// Runs `f` with the scheduler lock held and interrupts disabled (the
    /// previous interrupt state is restored afterwards). `f` must only do bookkeeping: it
    /// must never switch contexts or block. Panics if nothing is installed.
    pub fn with<R>(&self, f: impl FnOnce(&mut Kernel<N, M>) -> R) -> R {
        let mut g = self.inner.lock();
        match g.as_mut() {
            Some(k) => f(k),
            None => panic!("EUK: kernel not installed"),
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub use real::*;

#[cfg(target_arch = "x86_64")]
mod real {
    use super::*;
    use crate::irq::IrqOff;
    use crate::preempt::{self, HARDIRQ, IrqExitAction, PREEMPT};
    use crate::switch;

    /// A thread may only give up the CPU voluntarily outside interrupt context
    /// and with preemption enabled.
    fn check_may_block() -> Result<(), Error> {
        if preempt::in_interrupt() {
            return Err(Error::InterruptContext);
        }
        if !preempt::preemption_enabled() {
            return Err(Error::PreemptDisabled);
        }
        Ok(())
    }

    /// Disables preemption (nestable). Pair with `preempt_enable`.
    pub fn preempt_disable() -> Result<(), Error> {
        PREEMPT.inc().map(|_| ()).map_err(|_| Error::PreemptCount)
    }

    /// Re-enables preemption. When the depth reaches zero and a reschedule was
    /// requested meanwhile, it runs now. Returns whether a reschedule ran.
    ///
    /// # Safety
    /// See `reschedule`.
    pub unsafe fn preempt_enable<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
    ) -> Result<bool, Error> {
        let depth = PREEMPT.dec().map_err(|_| Error::PreemptCount)?;
        if depth != 0 {
            return Ok(false);
        }
        // SAFETY: forwarded.
        unsafe { preempt_point(slot) }
    }

    /// The interrupt-EXIT step. Call from the IRQ exit hook, after the handler
    /// has finished and `HARDIRQ` is back to zero, with interrupts still off.
    /// Reschedules on the interrupted thread's own kernel stack when the
    /// scheduler requested it and nothing forbids it. Returns the decision.
    ///
    /// # Safety
    /// See `reschedule`; additionally this runs on the interrupted thread's
    /// kernel stack inside the trap path (its full state is in the trap frame).
    pub unsafe fn irq_exit_reschedule<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
    ) -> Result<IrqExitAction, Error> {
        let (need, pending) = slot.with(|k| (k.sched().need_resched(), k.switch_pending()));
        let action = preempt::irq_exit_action(need, PREEMPT.depth(), HARDIRQ.depth(), pending);
        if action == IrqExitAction::Reschedule {
            // SAFETY: forwarded.
            unsafe { reschedule(slot) }?;
        }
        Ok(action)
    }

    /// Switches to the thread the scheduler selects, if different from the
    /// current one. Returns when this context is resumed (immediately if no
    /// switch was needed).
    ///
    /// # Safety
    /// - Interrupts are disabled by this function until it returns (the previous
    ///   state is restored); it may be called with them on or off.
    /// - `slot` is not moved or dropped while any thread is switched out (use a
    ///   `static`), and every thread was created through this same `slot`.
    /// - The trampoline hooks are installed and complete the switch via `slot`.
    /// - Single CPU only. The caller holds no spinlock.
    pub unsafe fn reschedule<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
    ) -> Result<(), Error> {
        // Interrupts stay off from planning until this context is resumed; the
        // caller's previous state returns when `_irq` drops after the resume.
        let _irq = IrqOff::new();
        let plan = slot.with(|k| k.plan_switch())?;
        if let Some(p) = plan {
            // SAFETY: the plan's slot points into the pinned `Kernel` and the
            // next stack pointer comes from `KThreadStacks` for a live, off-CPU
            // thread; the remaining conditions are this function's contract.
            unsafe { switch::switch_context(p.prev_sp_slot, p.next_sp) };
            // Running on the resumed thread's stack: finish the bookkeeping.
            slot.with(|k| k.complete_switch())?;
        }
        Ok(())
    }

    /// Voluntary yield: the caller stays Ready and runs again later.
    ///
    /// # Safety
    /// See `reschedule`.
    pub unsafe fn yield_now<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
    ) -> Result<(), Error> {
        check_may_block()?;
        // SAFETY: forwarded.
        unsafe { reschedule(slot) }
    }

    /// Reschedules if a timer tick or wake-up requested it. Returns whether a
    /// reschedule was performed. This is the point where a future interrupt exit
    /// path would act on `need_resched`.
    ///
    /// # Safety
    /// See `reschedule`.
    pub unsafe fn preempt_point<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
    ) -> Result<bool, Error> {
        if preempt::in_interrupt() {
            return Err(Error::InterruptContext);
        }
        if !preempt::preemption_enabled() {
            return Ok(false); // deferred until the depth returns to zero
        }
        if slot.with(|k| k.sched().need_resched()) {
            // SAFETY: forwarded.
            unsafe { reschedule(slot) }?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Blocks the current thread until `wake`. If a wake-up already arrived the
    /// call returns without blocking.
    ///
    /// # Safety
    /// See `reschedule`.
    pub unsafe fn block_current<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
        reason: BlockReason,
    ) -> Result<BlockOutcome, Error> {
        check_may_block()?;
        // State change and switch form one interrupt-atomic sequence.
        let _irq = IrqOff::new();
        let out = slot.with(|k| k.sched_mut().block_current(reason))?;
        if out == BlockOutcome::Blocked {
            // SAFETY: forwarded.
            unsafe { reschedule(slot) }?;
        }
        Ok(out)
    }

    /// Sleeps for `ticks` (never wakes early). Zero ticks yields instead.
    ///
    /// # Safety
    /// See `reschedule`.
    pub unsafe fn sleep_current<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
        ticks: u64,
    ) -> Result<SleepOutcome, Error> {
        check_may_block()?;
        let _irq = IrqOff::new();
        let out = slot.with(|k| k.sched_mut().sleep_current(ticks))?;
        // SAFETY: forwarded.
        unsafe { reschedule(slot) }?;
        Ok(out)
    }

    /// Blocks until `target` terminates (returns at once if it already has).
    ///
    /// # Safety
    /// See `reschedule`.
    pub unsafe fn join_wait<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
        target: Handle,
    ) -> Result<BlockOutcome, Error> {
        check_may_block()?;
        let _irq = IrqOff::new();
        let out = slot.with(|k| k.sched_mut().join_wait(target))?;
        if out == BlockOutcome::Blocked {
            // SAFETY: forwarded.
            unsafe { reschedule(slot) }?;
        }
        Ok(out)
    }

    /// Wakes a blocked or sleeping thread. Bookkeeping only; the caller decides
    /// whether to `preempt_point` afterwards.
    pub fn wake<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
        h: Handle,
    ) -> Result<WakeOutcome, Error> {
        Ok(slot.with(|k| k.sched_mut().wake(h))?)
    }

    /// Timer tick bookkeeping (time, sleepers, accounting). Returns whether a
    /// reschedule is requested; it never switches.
    pub fn tick<const N: usize, M: StackMemory>(slot: &KernelSlot<N, M>) -> bool {
        slot.with(|k| k.sched_mut().tick())
    }

    /// The current thread exits with `status` and never runs again.
    /// `after_terminate` runs under the lock once the thread is `Terminated` but
    /// still executing on its own stack; it is where a caller can observe that
    /// reclamation is refused at this point.
    ///
    /// Idle threads cannot exit: `terminate_current` rejects them and this
    /// function then panics instead of entering the ordinary exit path.
    ///
    /// # Safety
    /// See `reschedule`.
    pub unsafe fn exit_current_with<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
        status: i32,
        after_terminate: impl FnOnce(&mut Kernel<N, M>),
    ) -> ! {
        let ok = slot.with(|k| {
            let r = k.sched_mut().terminate_current(status);
            if r.is_ok() {
                after_terminate(k);
            }
            r.is_ok()
        });
        if !ok {
            panic!("EUK: exit of a thread that cannot exit (idle or not running)");
        }
        // SAFETY: forwarded. This context is Terminated and never resumed.
        let _ = unsafe { reschedule(slot) };
        panic!("EUK: terminated thread was resumed or nothing is runnable");
    }

    /// `exit_current_with` with nothing to observe.
    ///
    /// # Safety
    /// See `reschedule`.
    pub unsafe fn exit_current<const N: usize, M: StackMemory>(
        slot: &KernelSlot<N, M>,
        status: i32,
    ) -> ! {
        // SAFETY: forwarded.
        unsafe { exit_current_with(slot, status, |_| {}) }
    }

    /// Host-test helper: suspend the current thread and resume the bootstrap
    /// context that made the first `reschedule`.
    ///
    /// # Safety
    /// See `reschedule`; additionally the bootstrap context must be suspended
    /// inside its first `reschedule`.
    #[cfg(test)]
    pub(crate) unsafe fn leave_to_boot<const N: usize, M: StackMemory>(slot: &KernelSlot<N, M>) {
        let (slot_ptr, boot) = slot.with(|k| (k.current_sp_slot().unwrap(), k.boot_sp()));
        assert_ne!(boot, 0, "no bootstrap context to return to");
        // SAFETY: slot_ptr is this thread's slot; boot is the suspended bootstrap sp.
        unsafe { switch::switch_context(slot_ptr, boot) };
    }
}

#[cfg(test)]
mod tests {
    //! Model tests: the plan/complete protocol is driven by a simulated
    //! architecture layer (no assembly runs here). The `real_tests` module below
    //! executes the actual context switch on the x86-64 host.
    use super::*;
    use crate::kstack::GUARD_BYTES;
    use crate::kthread::MemError;
    use crate::thread::{self, Priority};

    const BASE: u64 = 0x20_0000;
    const STACK: u64 = 16 * 1024;
    const SLOT: u64 = STACK + GUARD_BYTES;
    const TEXT: (u64, u64) = (0x10_0000, 0x11_0000);
    const TRAMP: u64 = 0x10_0100;
    const ENTRY: u64 = 0x10_2000;

    struct FakeMem {
        data: Vec<u8>,
    }
    impl StackMemory for FakeMem {
        fn write(&mut self, addr: u64, d: &[u8]) -> Result<(), MemError> {
            let s = addr.checked_sub(BASE).ok_or(MemError)? as usize;
            let e = s.checked_add(d.len()).ok_or(MemError)?;
            self.data.get_mut(s..e).ok_or(MemError)?.copy_from_slice(d);
            Ok(())
        }
        fn zero(&mut self, addr: u64, len: u64) -> Result<(), MemError> {
            let s = addr.checked_sub(BASE).ok_or(MemError)? as usize;
            let e = s.checked_add(len as usize).ok_or(MemError)?;
            self.data.get_mut(s..e).ok_or(MemError)?.fill(0);
            Ok(())
        }
    }

    type Kn = Kernel<8, FakeMem>;

    fn kernel() -> Kn {
        let slots = 8u64;
        let pool = StackPool::new(BASE, slots * SLOT, STACK, 1 << 30).unwrap();
        let cfg = Config {
            text_start: TEXT.0,
            text_end: TEXT.1,
            trampoline: TRAMP,
        };
        let mem = FakeMem {
            data: vec![0xAA; (slots * SLOT) as usize],
        };
        Kernel::new(cfg, pool, mem).unwrap()
    }

    fn params(prio: u8) -> ThreadParams {
        ThreadParams {
            entry: ENTRY,
            arg: 0,
            priority: Priority::new(prio).unwrap(),
        }
    }

    /// The simulated architecture layer: plan, "switch", complete.
    fn sim_switch(k: &mut Kn) -> bool {
        match k.plan_switch().unwrap() {
            Some(_) => {
                k.complete_switch().unwrap();
                true
            }
            None => false,
        }
    }

    fn two_threads() -> (Kn, Handle, Handle, Handle) {
        let mut k = kernel();
        let idle = k.create_idle(ENTRY, 0).unwrap();
        let a = k.create(params(4)).unwrap();
        let b = k.create(params(4)).unwrap();
        (k, idle, a, b)
    }

    #[test]
    fn first_dispatch_saves_into_the_bootstrap_slot_and_loads_the_new_frame() {
        let (mut k, _idle, a, _b) = two_threads();
        let plan = k.plan_switch().unwrap().unwrap();
        assert_eq!(plan.prev_sp_slot, core::ptr::addr_of_mut!(k.boot_sp));
        assert_eq!(plan.next_sp, k.stacks().saved_sp(a).unwrap());
        assert!(!k.switch_pending(), "no outgoing thread to complete");
        assert_eq!(k.sched().current(), Some(a));
        assert_eq!(k.sched().thread(a).unwrap().state(), State::Running);
        k.complete_switch().unwrap();
        k.check_accounting().unwrap();
        k.sched().check_invariants().unwrap();
    }

    #[test]
    fn plan_points_at_the_outgoing_slot_and_incoming_saved_sp() {
        let (mut k, _idle, a, b) = two_threads();
        sim_switch(&mut k); // boot -> a
        let plan = k.plan_switch().unwrap().unwrap(); // a yields, b next
        assert_eq!(plan.prev_sp_slot, k.stacks.saved_sp_slot(a).unwrap());
        assert_eq!(plan.next_sp, k.stacks().saved_sp(b).unwrap());
        assert!(k.switch_pending());
        assert!(
            k.sched().thread(a).unwrap().on_cpu(),
            "outgoing thread stays on_cpu until the switch is complete"
        );
        assert_eq!(k.sched().thread(a).unwrap().state(), State::Ready);
        k.complete_switch().unwrap();
        assert!(!k.sched().thread(a).unwrap().on_cpu());
        assert!(!k.switch_pending());
    }

    #[test]
    fn planning_again_before_completion_is_refused_and_changes_nothing() {
        let (mut k, _idle, a, b) = two_threads();
        sim_switch(&mut k);
        k.plan_switch().unwrap().unwrap();
        let (cur, n) = (k.sched().current(), k.switch_count());
        assert_eq!(k.plan_switch(), Err(Error::SwitchInProgress));
        assert_eq!((k.sched().current(), k.switch_count()), (cur, n));
        assert_eq!(cur, Some(b));
        k.complete_switch().unwrap();
        assert_eq!(k.sched().thread(a).unwrap().state(), State::Ready);
        k.sched().check_invariants().unwrap();
    }

    #[test]
    fn yielding_the_only_runnable_thread_needs_no_switch() {
        let mut k = kernel();
        k.create_idle(ENTRY, 0).unwrap();
        let a = k.create(params(4)).unwrap();
        assert!(sim_switch(&mut k));
        let n = k.switch_count();
        for _ in 0..10 {
            assert!(
                !sim_switch(&mut k),
                "idle (level 0) never beats a ready worker"
            );
        }
        assert_eq!((k.switch_count(), k.sched().current()), (n, Some(a)));
        assert!(!k.switch_pending());
        k.sched().check_invariants().unwrap();
    }

    #[test]
    fn idle_runs_when_nothing_else_is_runnable_and_yields_back() {
        let (mut k, idle, a, b) = two_threads();
        sim_switch(&mut k); // a
        k.sched_mut().block_current(BlockReason::Event(1)).unwrap();
        sim_switch(&mut k); // b
        k.sched_mut().block_current(BlockReason::Event(2)).unwrap();
        sim_switch(&mut k);
        assert_eq!(k.sched().current(), Some(idle));
        k.sched_mut().wake(a).unwrap();
        assert!(k.sched_mut().tick(), "worker ready: idle must reschedule");
        assert!(sim_switch(&mut k));
        assert_eq!(k.sched().current(), Some(a));
        assert_eq!(k.sched().thread(b).unwrap().state(), State::Blocked);
        k.check_accounting().unwrap();
    }

    #[test]
    fn terminated_stack_is_reclaimed_only_after_the_switch_completes() {
        let (mut k, _idle, a, b) = two_threads();
        sim_switch(&mut k); // a runs
        k.sched_mut().terminate_current(5).unwrap();
        let on_cpu = Err(Error::Thread(thread::Error::StillOnCpu));
        assert_eq!(k.destroy(a), on_cpu, "terminated, still executing");
        let plan = k.plan_switch().unwrap().unwrap();
        assert_eq!(plan.next_sp, k.stacks().saved_sp(b).unwrap());
        assert_eq!(k.destroy(a), on_cpu, "planned, not yet switched");
        assert_eq!(
            k.reap_terminated(|_, _| panic!("must skip a thread on its stack")),
            0
        );
        assert_eq!(k.stacks().stacks_in_use(), 3);
        k.complete_switch().unwrap();
        let mut seen = Vec::new();
        assert_eq!(k.reap_terminated(|id, st| seen.push((id, st))), 1);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1, 5);
        assert_eq!(k.stacks().stacks_in_use(), 2);
        assert_eq!(k.destroy(a), Err(Error::Thread(thread::Error::Stale)));
        k.check_accounting().unwrap();
        k.sched().check_invariants().unwrap();
    }

    #[test]
    fn reap_terminated_reports_every_dead_thread_once() {
        let (mut k, _idle, a, b) = two_threads();
        k.sched_mut().kill(a, 1).unwrap();
        k.sched_mut().kill(b, 2).unwrap();
        let mut seen = Vec::new();
        assert_eq!(k.reap_terminated(|id, st| seen.push((id.0, st))), 2);
        seen.sort();
        assert_eq!(seen, vec![(2, 1), (3, 2)]);
        assert_eq!(k.reap_terminated(|_, _| panic!("nothing left")), 0);
        assert_eq!(k.stacks().stacks_in_use(), 1, "only the idle stack remains");
    }

    #[test]
    fn creation_failures_leave_scheduler_and_pool_untouched() {
        let mut k = kernel();
        k.create_idle(ENTRY, 0).unwrap();
        let before = (k.sched().live_threads(), k.stacks().stacks_in_use());
        let bad = ThreadParams {
            entry: 0,
            ..params(3)
        };
        assert_eq!(k.create(bad), Err(Error::BadEntry));
        assert_eq!(
            k.create(ThreadParams {
                priority: Priority::IDLE,
                ..params(3)
            }),
            Err(Error::Thread(thread::Error::BadPriority))
        );
        assert_eq!(
            k.create_idle(ENTRY, 0),
            Err(Error::Thread(thread::Error::IdleExists))
        );
        assert_eq!(
            (k.sched().live_threads(), k.stacks().stacks_in_use()),
            before
        );
        k.check_accounting().unwrap();
    }

    #[test]
    fn kernel_slot_requires_installation() {
        let slot: KernelSlot<8, FakeMem> = KernelSlot::new();
        assert!(!slot.is_installed());
        slot.install(kernel());
        assert!(slot.is_installed());
        let live = slot.with(|k| {
            k.create_idle(ENTRY, 0).unwrap();
            k.sched().live_threads()
        });
        assert_eq!(live, 1);
    }

    #[test]
    #[should_panic(expected = "kernel not installed")]
    fn kernel_slot_panics_if_used_before_install() {
        let slot: KernelSlot<8, FakeMem> = KernelSlot::new();
        slot.with(|_| ());
    }

    /// Random operation sequences through the full plan/complete protocol with
    /// scheduler invariants and stack accounting checked after every step.
    fn random_run(seed: u64, steps: usize) {
        let mut k = kernel();
        k.create_idle(ENTRY, 0).unwrap();
        sim_switch(&mut k);
        let mut hs: Vec<Handle> = Vec::new();
        let mut x = seed;
        for _ in 0..steps {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let pick = if hs.is_empty() {
                None
            } else {
                Some(hs[(x >> 16) as usize % hs.len()])
            };
            let r: Result<(), Error> = match x % 12 {
                0 | 1 => k.create(params(1 + (x >> 8) as u8 % 7)).map(|h| hs.push(h)),
                2 => k
                    .sched_mut()
                    .block_current(BlockReason::Event(1))
                    .map(|_| ())
                    .map_err(Error::from),
                3 => k
                    .sched_mut()
                    .sleep_current((x >> 8) % 20)
                    .map(|_| ())
                    .map_err(Error::from),
                4 => pick.map_or(Ok(()), |h| {
                    k.sched_mut().wake(h).map(|_| ()).map_err(Error::from)
                }),
                5 => pick.map_or(Ok(()), |h| k.sched_mut().kill(h, 1).map_err(Error::from)),
                6 => k.sched_mut().terminate_current(2).map_err(Error::from),
                7 => pick.map_or(Ok(()), |h| k.destroy(h).map(|_| ())),
                8 => {
                    k.reap_terminated(|_, _| {});
                    Ok(())
                }
                9 => {
                    k.sched_mut().tick();
                    Ok(())
                }
                _ => {
                    // Switch point: plan then complete, the quiescent boundary.
                    sim_switch(&mut k);
                    k.sched().check_invariants().unwrap();
                    Ok(())
                }
            };
            if let Err(e) = r {
                assert!(
                    !matches!(
                        e,
                        Error::Corrupt
                            | Error::SwitchInProgress
                            | Error::Thread(thread::Error::Corrupt)
                    ),
                    "internal error {e:?}"
                );
            }
            k.check_accounting().unwrap();
        }
    }

    #[test]
    fn random_dispatch_sequences_keep_scheduler_and_stacks_consistent() {
        for seed in [3u64, 99, 0xfeed_beef, 0x0123_4567_89ab_cdef] {
            random_run(seed, 40_000);
        }
    }
}

#[cfg(all(test, target_arch = "x86_64"))]
mod real_tests {
    //! Scenarios that EXECUTE the real context-switch assembly on the x86-64
    //! host: every thread runs on its own host-allocated stack and the
    //! scheduler really switches between them (cooperatively; the "timer" is a
    //! software tick from the idle thread). This is NOT a bare-metal or emulator
    //! test and does not cover privilege levels, interrupts or the kernel image.
    use super::*;
    use crate::kstack::GUARD_BYTES;
    use crate::kthread::MemError;
    use crate::preempt;
    use crate::switch::{self, HOST_SWITCH_LOCK};
    use crate::thread::{self, Priority, WakeReason};
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Mutex, MutexGuard};

    struct HostMem {
        lo: usize,
        hi: usize,
    }
    impl HostMem {
        fn span(&self, addr: u64, len: usize) -> Result<usize, MemError> {
            let a = addr as usize;
            let e = a.checked_add(len).ok_or(MemError)?;
            if a < self.lo || e > self.hi {
                return Err(MemError);
            }
            Ok(a)
        }
    }
    impl StackMemory for HostMem {
        fn write(&mut self, addr: u64, d: &[u8]) -> Result<(), MemError> {
            let a = self.span(addr, d.len())?;
            // SAFETY: [a, a+len) is inside the leaked host buffer [lo, hi).
            unsafe { core::ptr::copy_nonoverlapping(d.as_ptr(), a as *mut u8, d.len()) };
            Ok(())
        }
        fn zero(&mut self, addr: u64, len: u64) -> Result<(), MemError> {
            let a = self.span(addr, len as usize)?;
            // SAFETY: as above.
            unsafe { core::ptr::write_bytes(a as *mut u8, 0, len as usize) };
            Ok(())
        }
    }

    static K: KernelSlot<8, HostMem> = KernelSlot::new();
    static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static STATUSES: Mutex<Vec<(u64, i32)>> = Mutex::new(Vec::new());
    static SELF_DESTROY: Mutex<Vec<Result<i32, Error>>> = Mutex::new(Vec::new());
    static HANDLES: Mutex<Vec<Handle>> = Mutex::new(Vec::new());
    static OFF_STACK: AtomicBool = AtomicBool::new(false);
    static TIMED_OUT: AtomicBool = AtomicBool::new(false);
    static IDLE_ITERS: AtomicU64 = AtomicU64::new(0);
    static MISC: AtomicU64 = AtomicU64::new(0);
    static MISC2: AtomicU64 = AtomicU64::new(0);

    const SLOTS: u64 = 6;
    const STACK: u64 = 64 * 1024;

    extern "C" fn first_run() {
        K.with(|k| k.complete_switch().expect("complete_switch"));
    }

    extern "C" fn exit_hook(status: i32) -> ! {
        // SAFETY: single-threaded cooperative host scenario; hooks installed;
        // K is a static.
        unsafe {
            exit_current_with(&K, status, |k| {
                let me = k.sched().current().unwrap();
                SELF_DESTROY.lock().unwrap().push(k.destroy(me));
            })
        }
    }

    fn setup() -> MutexGuard<'static, ()> {
        let g = HOST_SWITCH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let total = (SLOTS * (STACK + GUARD_BYTES)) as usize;
        let buf: &'static mut [u8] = Box::leak(vec![0u8; total + 4096].into_boxed_slice());
        let base = (buf.as_ptr() as usize + 4095) & !4095;
        let pool = StackPool::new(base as u64, total as u64, STACK, 1 << 47).unwrap();
        let cfg = Config {
            text_start: 0x1000,
            text_end: 0x7fff_ffff_f000,
            trampoline: switch::trampoline_address(),
        };
        let mem = HostMem {
            lo: base,
            hi: base + total,
        };
        K.install(Kernel::new(cfg, pool, mem).unwrap());
        LOG.lock().unwrap().clear();
        STATUSES.lock().unwrap().clear();
        SELF_DESTROY.lock().unwrap().clear();
        HANDLES.lock().unwrap().clear();
        for a in [&IDLE_ITERS, &MISC, &MISC2] {
            a.store(0, Ordering::SeqCst);
        }
        OFF_STACK.store(false, Ordering::SeqCst);
        TIMED_OUT.store(false, Ordering::SeqCst);
        switch::install_hooks(first_run, exit_hook);
        g
    }

    fn log(s: impl Into<String>) {
        LOG.lock().unwrap().push(s.into());
    }
    fn logged() -> Vec<String> {
        LOG.lock().unwrap().clone()
    }
    fn fp(f: extern "C" fn(u64) -> i32) -> u64 {
        f as *const () as usize as u64
    }
    fn spawn(f: extern "C" fn(u64) -> i32, arg: u64, prio: u8) -> Handle {
        K.with(|k| {
            k.create(ThreadParams {
                entry: fp(f),
                arg,
                priority: Priority::new(prio).unwrap(),
            })
            .unwrap()
        })
    }
    fn idle_with(f: extern "C" fn(u64) -> i32) {
        K.with(|k| k.create_idle(fp(f), 0).unwrap());
    }
    /// Verifies the running thread's locals live inside its own owned stack.
    fn check_on_own_stack() {
        let marker = 0u8;
        let addr = &marker as *const u8 as u64;
        let ok = K.with(|k| {
            let me = k.sched().current().unwrap();
            let st = k.stacks().stack(me).unwrap();
            st.base() <= addr && addr < st.top()
        });
        if !ok {
            OFF_STACK.store(true, Ordering::SeqCst);
        }
    }
    fn reap(k: &mut Kernel<8, HostMem>) {
        k.reap_terminated(|id, st| STATUSES.lock().unwrap().push((id.0, st)));
    }
    fn finish_guard() {
        if IDLE_ITERS.fetch_add(1, Ordering::SeqCst) > 200_000 {
            TIMED_OUT.store(true, Ordering::SeqCst);
            // SAFETY: idle context, bootstrap suspended in `start`.
            unsafe { leave_to_boot(&K) };
        }
    }
    /// Default idle: reap, stop when only the idle thread is left, else tick+yield.
    extern "C" fn idle(_: u64) -> i32 {
        loop {
            let live = K.with(|k| {
                reap(k);
                k.sched().live_threads()
            });
            if live == 1 {
                // SAFETY: see finish_guard.
                unsafe { leave_to_boot(&K) };
            }
            finish_guard();
            tick(&K);
            // SAFETY: cooperative single-CPU scenario.
            unsafe { yield_now(&K) }.unwrap();
        }
    }
    /// Boots the first thread from the bootstrap context; returns when the idle
    /// thread hands control back.
    fn start() {
        // SAFETY: K is a static, hooks are installed, nothing else runs.
        unsafe { reschedule(&K) }.unwrap();
        assert!(!TIMED_OUT.load(Ordering::SeqCst), "scenario did not finish");
        assert!(
            !OFF_STACK.load(Ordering::SeqCst),
            "a thread ran off its own stack"
        );
    }
    fn assert_clean(created: u64) {
        K.with(|k| {
            assert_eq!(k.sched().live_threads(), 1, "only idle remains");
            assert_eq!(k.stacks().stacks_in_use(), 1);
            assert_eq!(k.stacks().created_total(), created);
            assert_eq!(k.stacks().destroyed_total(), created - 1);
            k.check_accounting().unwrap();
            k.sched().check_invariants().unwrap();
            assert!(!k.switch_pending());
        });
    }

    extern "C" fn ping_pong(arg: u64) -> i32 {
        let name = if arg == 0 { 'A' } else { 'B' };
        check_on_own_stack();
        for i in 0..3 {
            log(format!("{name}{i}"));
            // SAFETY: cooperative single-CPU scenario.
            unsafe { yield_now(&K) }.unwrap();
        }
        check_on_own_stack();
        10 + arg as i32
    }

    #[test]
    fn real_cooperative_switching_yield_resume_exit_and_reclaim() {
        let _g = setup();
        idle_with(idle);
        spawn(ping_pong, 0, 4);
        spawn(ping_pong, 1, 4);
        assert!(
            logged().is_empty(),
            "nothing runs before the first dispatch"
        );
        start();
        assert_eq!(logged(), ["A0", "B0", "A1", "B1", "A2", "B2"]);
        let mut st = STATUSES.lock().unwrap().clone();
        st.sort();
        assert_eq!(
            st,
            vec![(2, 10), (3, 11)],
            "return value became exit status"
        );
        // While terminated-but-still-on-its-stack, destroy was refused.
        let sd = SELF_DESTROY.lock().unwrap().clone();
        assert_eq!(sd, vec![Err(Error::Thread(thread::Error::StillOnCpu)); 2]);
        assert_clean(3);
        assert!(K.with(|k| k.switch_count()) >= 7);
    }

    extern "C" fn waiter(_: u64) -> i32 {
        log("W0");
        // SAFETY: cooperative single-CPU scenario.
        let out = unsafe { block_current(&K, BlockReason::Event(1)) }.unwrap();
        let me = K.with(|k| k.sched().current().unwrap());
        let why = K.with(|k| k.sched().thread(me).unwrap().wake_reason());
        log(format!("W1 {out:?} {why:?}"));
        0
    }
    extern "C" fn waker(_: u64) -> i32 {
        log("S0");
        let w = HANDLES.lock().unwrap()[0];
        let o = wake(&K, w).unwrap();
        log(format!("S1 {o:?}"));
        // SAFETY: cooperative single-CPU scenario.
        unsafe { yield_now(&K) }.unwrap();
        log("S2");
        0
    }

    #[test]
    fn real_block_and_wake() {
        let _g = setup();
        idle_with(idle);
        let w = spawn(waiter, 0, 4);
        HANDLES.lock().unwrap().push(w);
        spawn(waker, 0, 4);
        start();
        assert_eq!(
            logged(),
            ["W0", "S0", "S1 Woken", "W1 Blocked Event", "S2"],
            "a blocked thread is not run until woken, then resumes after its block call"
        );
        assert_clean(3);
    }

    extern "C" fn sleeper(arg: u64) -> i32 {
        let start = K.with(|k| k.sched().now());
        // SAFETY: cooperative single-CPU scenario.
        unsafe { sleep_current(&K, arg) }.unwrap();
        let end = K.with(|k| k.sched().now());
        log(format!("S{arg} slept {}", end - start));
        0
    }

    #[test]
    fn real_sleep_wakes_at_the_deadline_in_deadline_order() {
        let _g = setup();
        idle_with(idle);
        spawn(sleeper, 5, 4);
        spawn(sleeper, 2, 4);
        start();
        assert_eq!(logged(), ["S2 slept 2", "S5 slept 5"]);
        assert_clean(3);
    }

    extern "C" fn low(_: u64) -> i32 {
        log("L0");
        spawn(high, 0, 6); // publishing a higher-priority thread requests a reschedule
        let need = K.with(|k| k.sched().need_resched());
        // SAFETY: cooperative single-CPU scenario.
        let preempted = unsafe { preempt_point(&K) }.unwrap();
        log(format!("L1 need={need} preempted={preempted}"));
        0
    }
    extern "C" fn high(_: u64) -> i32 {
        log("H");
        0
    }

    #[test]
    fn real_higher_priority_thread_preempts_at_the_preemption_point() {
        let _g = setup();
        idle_with(idle);
        spawn(low, 0, 2);
        start();
        assert_eq!(logged(), ["L0", "H", "L1 need=true preempted=true"]);
        assert_clean(3);
    }

    extern "C" fn tag(arg: u64) -> i32 {
        log(format!("t{arg}"));
        0
    }

    #[test]
    fn real_dispatch_order_is_priority_then_fifo() {
        let _g = setup();
        idle_with(idle);
        for (id, prio) in [(1, 3), (2, 5), (3, 4), (4, 4), (5, 5)] {
            spawn(tag, id, prio);
        }
        start();
        assert_eq!(logged(), ["t2", "t5", "t3", "t4", "t1"]);
        assert_clean(6);
    }

    extern "C" fn joiner(_: u64) -> i32 {
        log("J0");
        let target = HANDLES.lock().unwrap()[0];
        // SAFETY: cooperative single-CPU scenario.
        let out = unsafe { join_wait(&K, target) }.unwrap();
        let status = K.with(|k| k.sched().thread(target).unwrap().exit_status());
        log(format!("J1 {out:?} {status:?}"));
        0
    }
    extern "C" fn target_thread(_: u64) -> i32 {
        log("T0");
        // SAFETY: cooperative single-CPU scenario.
        unsafe { yield_now(&K) }.unwrap();
        log("T1");
        3
    }

    #[test]
    fn real_join_waits_for_termination_and_sees_the_exit_status() {
        let _g = setup();
        idle_with(idle);
        // The target is created first so J can name it, but J runs first by priority.
        let t = spawn(target_thread, 0, 3);
        HANDLES.lock().unwrap().push(t);
        spawn(joiner, 0, 4);
        start();
        assert_eq!(logged(), ["J0", "T0", "T1", "J1 Blocked Some(3)"]);
        assert_clean(3);
    }

    extern "C" fn blocker(arg: u64) -> i32 {
        // SAFETY: cooperative single-CPU scenario.
        unsafe { block_current(&K, BlockReason::Event(arg as u32)) }.unwrap();
        MISC.fetch_add(1, Ordering::SeqCst);
        0
    }
    extern "C" fn idle_wake_all(_: u64) -> i32 {
        loop {
            let (live, blocked) = K.with(|k| {
                reap(k);
                let mut blocked = Vec::new();
                for i in 0..k.sched().capacity() {
                    if let Some(h) = k.sched().handle_at(i)
                        && k.sched().thread(h).unwrap().state() == State::Blocked
                    {
                        blocked.push(h);
                    }
                }
                (k.sched().live_threads(), blocked)
            });
            if live == 1 {
                // SAFETY: see finish_guard.
                unsafe { leave_to_boot(&K) };
            }
            if blocked.len() == 5 {
                MISC2.store(blocked.len() as u64, Ordering::SeqCst);
                for h in blocked {
                    wake(&K, h).unwrap();
                }
            }
            finish_guard();
            tick(&K);
            // SAFETY: cooperative single-CPU scenario.
            unsafe { yield_now(&K) }.unwrap();
        }
    }

    #[test]
    fn real_many_blocked_threads_all_resume_when_woken() {
        let _g = setup();
        idle_with(idle_wake_all);
        for i in 0..5 {
            spawn(blocker, i, 4);
        }
        start();
        assert_eq!(
            MISC2.load(Ordering::SeqCst),
            5,
            "all five were blocked at once"
        );
        assert_eq!(
            MISC.load(Ordering::SeqCst),
            5,
            "all five resumed after their block call"
        );
        assert_clean(6);
    }

    extern "C" fn churn(arg: u64) -> i32 {
        for _ in 0..(arg % 3) {
            // SAFETY: cooperative single-CPU scenario.
            unsafe { yield_now(&K) }.unwrap();
        }
        MISC.fetch_add(1, Ordering::SeqCst);
        arg as i32
    }
    const TOTAL: u64 = 400;
    extern "C" fn idle_churn(_: u64) -> i32 {
        let mut next = 0u64;
        loop {
            let live = K.with(|k| {
                reap(k);
                while next < TOTAL && k.sched().live_threads() < 5 {
                    let h = k.create(ThreadParams {
                        entry: fp(churn),
                        arg: next,
                        priority: Priority::new(1 + (next % 7) as u8).unwrap(),
                    });
                    if h.is_err() {
                        break;
                    }
                    next += 1;
                }
                k.sched().live_threads()
            });
            if live == 1 && next == TOTAL {
                // SAFETY: see finish_guard.
                unsafe { leave_to_boot(&K) };
            }
            finish_guard();
            tick(&K);
            // SAFETY: cooperative single-CPU scenario.
            unsafe { yield_now(&K) }.unwrap();
        }
    }

    #[test]
    fn real_repeated_create_run_exit_reclaim_reuses_stacks_without_leaks() {
        let _g = setup();
        idle_with(idle_churn);
        start();
        assert_eq!(
            MISC.load(Ordering::SeqCst),
            TOTAL,
            "every thread ran to completion"
        );
        let st = STATUSES.lock().unwrap();
        assert_eq!(st.len() as u64, TOTAL);
        let sum: i64 = st.iter().map(|&(_, s)| s as i64).sum();
        assert_eq!(
            sum,
            (0..TOTAL as i64).sum::<i64>(),
            "each exit status delivered once"
        );
        let mut ids: Vec<_> = st.iter().map(|&(id, _)| id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len() as u64, TOTAL, "thread ids unique");
        drop(st);
        assert_clean(TOTAL + 1);
        // Far more threads than stacks: slots were reused after real reclamation.
        assert!(TOTAL > SLOTS * 10);
    }

    #[test]
    fn real_sleep_state_does_not_leak_a_wakeup_to_other_threads() {
        // A sleeper that is explicitly woken early reports Cancelled and the
        // woken-early thread resumes without waiting for the deadline.
        extern "C" fn long_sleeper(_: u64) -> i32 {
            // SAFETY: cooperative single-CPU scenario.
            unsafe { sleep_current(&K, 1_000_000) }.unwrap();
            let me = K.with(|k| k.sched().current().unwrap());
            let why = K.with(|k| k.sched().thread(me).unwrap().wake_reason());
            log(format!("woke {why:?}"));
            0
        }
        extern "C" fn canceller(_: u64) -> i32 {
            let s = HANDLES.lock().unwrap()[0];
            log(format!("{:?}", wake(&K, s).unwrap()));
            0
        }
        let _g = setup();
        idle_with(idle);
        let s = spawn(long_sleeper, 0, 5);
        HANDLES.lock().unwrap().push(s);
        spawn(canceller, 0, 4);
        start();
        assert_eq!(
            logged(),
            [
                "Woken",
                format!("woke {:?}", WakeReason::Cancelled).as_str()
            ]
        );
        assert!(
            K.with(|k| k.sched().now()) < 100,
            "did not wait out the deadline"
        );
        assert_clean(3);
    }

    // ---- Mission D4: interrupt/preemption rules, executed on real stacks. ----
    // The interrupt flag is modelled (see `irq`), and "interrupt context" is
    // simulated by raising `HARDIRQ`. These tests prove the scheduling RULES and
    // the switch protocol with them; real interrupt delivery is only covered by
    // the QEMU test.

    extern "C" fn refuses_to_block_when_preempt_disabled(_: u64) -> i32 {
        preempt_disable().unwrap();
        preempt_disable().unwrap(); // nested
        let t = K.with(|k| k.sched().current().unwrap());
        let r = [
            unsafe { yield_now(&K) }.err(),
            unsafe { block_current(&K, BlockReason::Event(7)) }.err(),
            unsafe { sleep_current(&K, 2) }.err(),
            unsafe { join_wait(&K, t) }.err(),
        ];
        log(format!("{:?}", r));
        // Nothing changed state: still Running, no switch pending.
        let st = K.with(|k| (k.sched().thread(t).unwrap().state(), k.switch_pending()));
        log(format!("{:?}", st));
        log(format!("{:?}", unsafe { preempt_enable(&K) }.unwrap()));
        log(format!(
            "depth-after-first-enable={}",
            preempt::PREEMPT.depth()
        ));
        log(format!("{:?}", unsafe { preempt_enable(&K) }.unwrap()));
        // Unbalanced release is an error and changes nothing.
        log(format!("{:?}", unsafe { preempt_enable(&K) }.err()));
        log(format!("depth-end={}", preempt::PREEMPT.depth()));
        0
    }

    #[test]
    fn d4_blocking_operations_are_refused_while_preemption_is_disabled() {
        let _g = setup();
        idle_with(idle);
        spawn(refuses_to_block_when_preempt_disabled, 0, 4);
        start();
        let want_err = "[Some(PreemptDisabled), Some(PreemptDisabled), Some(PreemptDisabled), Some(PreemptDisabled)]";
        assert_eq!(
            logged(),
            [
                want_err,
                "(Running, false)",
                "false",
                "depth-after-first-enable=1",
                "false",
                "Some(PreemptCount)",
                "depth-end=0",
            ]
        );
        assert_clean(2);
    }

    extern "C" fn refuses_in_interrupt_context(_: u64) -> i32 {
        preempt::HARDIRQ.inc().unwrap();
        let t = K.with(|k| k.sched().current().unwrap());
        let r = [
            unsafe { yield_now(&K) }.err(),
            unsafe { block_current(&K, BlockReason::Event(7)) }.err(),
            unsafe { sleep_current(&K, 2) }.err(),
            unsafe { join_wait(&K, t) }.err(),
        ];
        let pp = unsafe { preempt_point(&K) }.err();
        preempt::HARDIRQ.dec().unwrap();
        log(format!("{:?} {:?}", r, pp));
        // Allowed again once the "handler" has finished.
        log(format!("{:?}", unsafe { yield_now(&K) }.is_ok()));
        0
    }

    #[test]
    fn d4_blocking_operations_are_refused_in_interrupt_context() {
        let _g = setup();
        idle_with(idle);
        spawn(refuses_in_interrupt_context, 0, 4);
        start();
        assert_eq!(
            logged(),
            [
                "[Some(InterruptContext), Some(InterruptContext), Some(InterruptContext), Some(InterruptContext)] Some(InterruptContext)",
                "true",
            ]
        );
        assert_eq!(preempt::HARDIRQ.depth(), 0);
        assert_clean(2);
    }

    extern "C" fn high_prio(_: u64) -> i32 {
        log("HIGH runs");
        0
    }

    extern "C" fn defers_then_preempts(_: u64) -> i32 {
        preempt_disable().unwrap();
        // A higher-priority thread becomes Ready: the scheduler requests a reschedule.
        K.with(|k| {
            k.create(ThreadParams {
                entry: fp(high_prio),
                arg: 0,
                priority: Priority::new(6).unwrap(),
            })
            .unwrap()
        });
        log(format!(
            "requested={}",
            K.with(|k| k.sched().need_resched())
        ));
        // Preemption is disabled: the request must be kept, not acted on.
        log(format!("point={:?}", unsafe { preempt_point(&K) }.unwrap()));
        log("LOW still running");
        log(format!(
            "still-requested={}",
            K.with(|k| k.sched().need_resched())
        ));
        // Re-enabling runs the deferred reschedule: HIGH runs before this returns.
        log(format!(
            "enable-ran={:?}",
            unsafe { preempt_enable(&K) }.unwrap()
        ));
        log("LOW resumed");
        0
    }

    #[test]
    fn d4_reschedule_requested_while_disabled_runs_when_preemption_returns() {
        let _g = setup();
        idle_with(idle);
        spawn(defers_then_preempts, 0, 4);
        start();
        assert_eq!(
            logged(),
            [
                "requested=true",
                "point=false",
                "LOW still running",
                "still-requested=true",
                "HIGH runs",
                "enable-ran=true",
                "LOW resumed",
            ]
        );
        assert_clean(3);
    }

    extern "C" fn exit_decisions(_: u64) -> i32 {
        let act = |a: Result<preempt::IrqExitAction, Error>| format!("{:?}", a.unwrap());
        // No request pending.
        log(act(unsafe { irq_exit_reschedule(&K) }));
        K.with(|k| {
            k.create(ThreadParams {
                entry: fp(high_prio),
                arg: 0,
                priority: Priority::new(6).unwrap(),
            })
            .unwrap()
        });
        // Requested, but preemption is disabled.
        preempt::PREEMPT.inc().unwrap();
        log(act(unsafe { irq_exit_reschedule(&K) }));
        preempt::PREEMPT.dec().unwrap();
        // Requested, but we are "still in a nested interrupt".
        preempt::HARDIRQ.inc().unwrap();
        log(act(unsafe { irq_exit_reschedule(&K) }));
        preempt::HARDIRQ.dec().unwrap();
        // Requested and allowed: HIGH runs inside this call, then we resume.
        log(act(unsafe { irq_exit_reschedule(&K) }));
        log("LOW after");
        log(act(unsafe { irq_exit_reschedule(&K) }));
        0
    }

    #[test]
    fn d4_interrupt_exit_step_defers_or_reschedules_as_the_rules_say() {
        let _g = setup();
        idle_with(idle);
        spawn(exit_decisions, 0, 4);
        start();
        assert_eq!(
            logged(),
            [
                "None",
                "Deferred",
                "Deferred",
                "HIGH runs",
                "Reschedule",
                "LOW after",
                "None"
            ]
        );
        assert_clean(3);
    }

    extern "C" fn if_observer(_: u64) -> i32 {
        // A brand-new thread starts with interrupts off (it inherits the
        // switcher's state, which is off). Enabling here must not leak into the
        // thread that is resumed later.
        log(format!("B starts if={}", crate::irq::enabled()));
        crate::irq::enable();
        0
    }

    extern "C" fn if_keeper(_: u64) -> i32 {
        crate::irq::enable();
        spawn_low(if_observer);
        unsafe { yield_now(&K) }.unwrap();
        log(format!("A resumed if={}", crate::irq::enabled()));
        // Now yield with interrupts OFF: they must still be off after the resume.
        let saved = crate::irq::save_disable();
        spawn_low(if_observer);
        unsafe { yield_now(&K) }.unwrap();
        log(format!("A resumed2 if={}", crate::irq::enabled()));
        crate::irq::restore(saved);
        log(format!("A restored if={}", crate::irq::enabled()));
        0
    }

    fn spawn_low(f: extern "C" fn(u64) -> i32) {
        K.with(|k| {
            k.create(ThreadParams {
                entry: fp(f),
                arg: 0,
                priority: Priority::new(4).unwrap(),
            })
            .unwrap()
        });
    }

    #[test]
    fn d4_each_context_gets_its_own_interrupt_state_back_after_a_switch() {
        let _g = setup();
        crate::irq::enable();
        idle_with(idle);
        spawn(if_keeper, 0, 4);
        start();
        assert_eq!(
            logged(),
            [
                "B starts if=false",
                "A resumed if=true",
                "B starts if=false",
                "A resumed2 if=false",
                "A restored if=true",
            ]
        );
        assert_clean(4);
    }
}
