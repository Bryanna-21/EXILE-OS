//! Uniprocessor cooperative scheduler on top of the real context switch.
//!
//! Design points (see docs/SCHEDULER-INTEGRATION.md):
//!  * Threads never switch to each other; they switch to the scheduler context, which
//!    picks the next one. A thread's stack is therefore only ever resumed by `run`.
//!  * A thread that leaves the CPU stays `Running` until the scheduler context has
//!    regained control and *commits* its action. A wake that lands in between only bumps
//!    the thread's sequence number; the commit sees it and keeps the thread runnable.
//!    That closes the lost-wakeup window without holding a lock across a switch.
//!  * Block protocol: `prepare_block` snapshots a sequence number, the caller then
//!    re-checks its condition, and `block(ticket)` returns immediately (or at commit)
//!    if a wake happened after the snapshot.
//!  * The lock masks interrupts on bare metal (`sync.rs`), so `tick`, `wake` and `kill`
//!    may be called from interrupt context.

use crate::context::{self, STACK_SIZE, Stack};
use crate::sync::SpinLock;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_THREADS: usize = 16;
pub type SpaceId = u16;
pub const KERNEL_SPACE: SpaceId = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ThreadId {
    index: u16,
    generation: u16,
}

impl ThreadId {
    pub const NONE: ThreadId = ThreadId {
        index: u16::MAX,
        generation: 0,
    };
    pub fn raw(self) -> u32 {
        ((self.generation as u32) << 16) | self.index as u32
    }
    pub fn index(self) -> usize {
        self.index as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SchedError {
    /// Thread table full.
    Full,
    /// The id names a thread that no longer exists (generation mismatch or free slot).
    Stale,
    /// The call is not valid in this context (e.g. blocking outside a thread).
    InvalidState,
}

/// Why a blocked thread resumed. Ordered: a stronger reason replaces a weaker one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Reason {
    None,
    Timeout,
    Woken,
    Cancelled,
    Killed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadState {
    Runnable,
    Running,
    Blocked,
    /// Blocked with a deadline.
    Sleeping,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunExit {
    /// No live threads remain.
    AllDone,
    /// The idle callback asked to stop with threads still alive (blocked or sleeping).
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ticket {
    id: ThreadId,
    seq: u64,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub spawned: u64,
    pub exited: u64,
    pub switches: u64,
    pub refused_switches: u64,
    pub stack_corruptions: u64,
    pub bad_switches: u64,
    pub killed_before_run: u64,
    pub space_loads: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    Free,
    Runnable,
    Running,
    Blocked,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    None,
    Yield,
    Block,
    Exit(i64),
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u16,
    state: St,
    space: SpaceId,
    queued: bool,
    started: bool,
    killed: bool,
    seq: u64,
    ticket_seq: u64,
    pending: Reason,
    deadline: Option<u64>,
    action: Action,
}

impl Slot {
    const FREE: Slot = Slot {
        generation: 0,
        state: St::Free,
        space: 0,
        queued: false,
        started: false,
        killed: false,
        seq: 0,
        ticket_seq: 0,
        pending: Reason::None,
        deadline: None,
        action: Action::None,
    };
}

struct Inner {
    slots: [Slot; MAX_THREADS],
    queue: [u16; MAX_THREADS],
    qhead: usize,
    qlen: usize,
    current: Option<ThreadId>,
    ticks: u64,
    live: usize,
    loaded_space: SpaceId,
    space_hook: Option<fn(SpaceId)>,
    in_run: bool,
    stats: Stats,
    exit_log: [(u32, i64); MAX_THREADS],
    exit_count: usize,
    #[cfg(test)]
    pre_commit: Option<fn(&Sched, ThreadId)>,
}

impl Inner {
    const fn new() -> Self {
        Inner {
            slots: [Slot::FREE; MAX_THREADS],
            queue: [0; MAX_THREADS],
            qhead: 0,
            qlen: 0,
            current: None,
            ticks: 0,
            live: 0,
            loaded_space: KERNEL_SPACE,
            space_hook: None,
            in_run: false,
            stats: Stats {
                spawned: 0,
                exited: 0,
                switches: 0,
                refused_switches: 0,
                stack_corruptions: 0,
                bad_switches: 0,
                killed_before_run: 0,
                space_loads: 0,
            },
            exit_log: [(0, 0); MAX_THREADS],
            exit_count: 0,
            #[cfg(test)]
            pre_commit: None,
        }
    }

    fn resolve(&self, id: ThreadId) -> Option<usize> {
        let i = id.index as usize;
        if i < MAX_THREADS
            && self.slots[i].state != St::Free
            && self.slots[i].generation == id.generation
        {
            Some(i)
        } else {
            None
        }
    }

    fn enqueue(&mut self, idx: usize) {
        if self.slots[idx].queued {
            return;
        }
        debug_assert!(self.qlen < MAX_THREADS);
        let tail = (self.qhead + self.qlen) % MAX_THREADS;
        self.queue[tail] = idx as u16;
        self.qlen += 1;
        self.slots[idx].queued = true;
    }

    fn dequeue(&mut self) -> Option<usize> {
        if self.qlen == 0 {
            return None;
        }
        let i = self.queue[self.qhead] as usize;
        self.qhead = (self.qhead + 1) % MAX_THREADS;
        self.qlen -= 1;
        self.slots[i].queued = false;
        Some(i)
    }

    /// Record a wake. Always bumps the sequence so a wake that lands before the thread
    /// has committed its block is not lost. Returns true if a blocked thread became
    /// runnable.
    fn wake_slot(&mut self, idx: usize, reason: Reason) -> bool {
        let s = &mut self.slots[idx];
        s.seq += 1;
        if reason > s.pending {
            s.pending = reason;
        }
        s.deadline = None;
        if s.state == St::Blocked {
            s.state = St::Runnable;
            self.enqueue(idx);
            true
        } else {
            false
        }
    }

    fn free(&mut self, idx: usize) {
        debug_assert!(!self.slots[idx].queued);
        let generation = self.slots[idx].generation.wrapping_add(1);
        self.slots[idx] = Slot::FREE;
        self.slots[idx].generation = generation;
        self.live -= 1;
    }
}

struct Pick {
    id: ThreadId,
    rsp: usize,
    hook: Option<fn(SpaceId)>,
}

pub struct Sched {
    inner: SpinLock<Inner>,
    /// Saved stack pointers live outside the lock: the assembly writes them directly.
    saved: [AtomicUsize; MAX_THREADS],
    sched_rsp: AtomicUsize,
    stacks: UnsafeCell<[Stack; MAX_THREADS]>,
}

// SAFETY: `inner` is lock-protected; `saved`/`sched_rsp` are atomics; a stack is touched
// only by its owning thread or, while the slot is free, by `spawn`.
unsafe impl Sync for Sched {}
unsafe impl Send for Sched {}

extern "C" fn exit_hook(sched: usize) -> ! {
    // SAFETY: `sched` is the address of the pinned `Sched` that spawned the thread.
    let s = unsafe { &*(sched as *const Sched) };
    s.exit_current(0)
}

impl Default for Sched {
    fn default() -> Self {
        Self::new()
    }
}

impl Sched {
    /// The scheduler must not move once a thread has been spawned: stacks hold its address.
    pub const fn new() -> Self {
        Sched {
            inner: SpinLock::new(Inner::new()),
            saved: [const { AtomicUsize::new(0) }; MAX_THREADS],
            sched_rsp: AtomicUsize::new(0),
            stacks: UnsafeCell::new([Stack::ZERO; MAX_THREADS]),
        }
    }

    fn stack_base(&self, idx: usize) -> *mut u8 {
        debug_assert!(idx < MAX_THREADS);
        // SAFETY: in-bounds index into the stack array.
        unsafe { (self.stacks.get() as *mut Stack).add(idx) as *mut u8 }
    }

    pub fn spawn(
        &self,
        space: SpaceId,
        entry: extern "C" fn(usize),
        arg: usize,
    ) -> Result<ThreadId, SchedError> {
        let mut g = self.inner.lock();
        let idx = (0..MAX_THREADS)
            .find(|&i| g.slots[i].state == St::Free)
            .ok_or(SchedError::Full)?;
        // SAFETY: the slot is free, so nothing else uses this stack.
        let rsp = unsafe {
            context::init_stack(
                self.stack_base(idx),
                STACK_SIZE,
                entry,
                arg,
                exit_hook,
                self as *const Sched as usize,
            )
        };
        self.saved[idx].store(rsp, Ordering::Release);
        let generation = g.slots[idx].generation;
        g.slots[idx] = Slot {
            generation,
            state: St::Runnable,
            space,
            ..Slot::FREE
        };
        g.live += 1;
        g.stats.spawned += 1;
        g.enqueue(idx);
        Ok(ThreadId {
            index: idx as u16,
            generation,
        })
    }

    // ---- queries -------------------------------------------------------------------

    pub fn current(&self) -> Option<ThreadId> {
        self.inner.lock().current
    }
    pub fn live(&self) -> usize {
        self.inner.lock().live
    }
    pub fn ticks(&self) -> u64 {
        self.inner.lock().ticks
    }
    pub fn stats(&self) -> Stats {
        self.inner.lock().stats
    }
    pub fn loaded_space(&self) -> SpaceId {
        self.inner.lock().loaded_space
    }
    pub fn space_of(&self, id: ThreadId) -> Option<SpaceId> {
        let g = self.inner.lock();
        g.resolve(id).map(|i| g.slots[i].space)
    }
    pub fn state_of(&self, id: ThreadId) -> Option<ThreadState> {
        let g = self.inner.lock();
        let i = g.resolve(id)?;
        let s = &g.slots[i];
        Some(match s.state {
            St::Runnable => ThreadState::Runnable,
            St::Running => ThreadState::Running,
            St::Blocked if s.deadline.is_some() => ThreadState::Sleeping,
            _ => ThreadState::Blocked,
        })
    }
    pub fn is_killed(&self, id: ThreadId) -> bool {
        let g = self.inner.lock();
        g.resolve(id).map(|i| g.slots[i].killed).unwrap_or(false)
    }
    pub fn exit_code_of(&self, id: ThreadId) -> Option<i64> {
        let g = self.inner.lock();
        let n = g.exit_count.min(MAX_THREADS);
        (0..n)
            .map(|k| g.exit_log[(g.exit_count - 1 - k) % MAX_THREADS])
            .find(|e| e.0 == id.raw())
            .map(|e| e.1)
    }
    pub fn set_space_hook(&self, hook: Option<fn(SpaceId)>) {
        self.inner.lock().space_hook = hook;
    }

    // ---- thread-side operations ------------------------------------------------------

    fn leave(&self, idx: usize) {
        let save = self.saved[idx].as_ptr();
        let to = self.sched_rsp.load(Ordering::Acquire);
        // SAFETY: `to` was stored by `run` just before it switched into this thread, and
        // `save` is this thread's own slot.
        unsafe { context::switch(save, to) }
    }

    fn take_pending(s: &mut Slot) -> Reason {
        let r = if s.killed { Reason::Killed } else { s.pending };
        s.pending = Reason::None;
        r
    }

    pub fn yield_now(&self) -> Result<(), SchedError> {
        let idx = {
            let mut g = self.inner.lock();
            let id = g.current.ok_or(SchedError::InvalidState)?;
            let i = g.resolve(id).ok_or(SchedError::Stale)?;
            g.slots[i].action = Action::Yield;
            i
        };
        self.leave(idx);
        Ok(())
    }

    pub fn prepare_block(&self) -> Result<Ticket, SchedError> {
        let mut g = self.inner.lock();
        let id = g.current.ok_or(SchedError::InvalidState)?;
        let i = g.resolve(id).ok_or(SchedError::Stale)?;
        let s = &mut g.slots[i];
        // A pending reason from before this snapshot is stale; a kill is sticky.
        s.pending = if s.killed {
            Reason::Killed
        } else {
            Reason::None
        };
        Ok(Ticket { id, seq: s.seq })
    }

    /// Block until woken, cancelled, killed or `deadline` (absolute tick) passes.
    pub fn block(&self, t: Ticket, deadline: Option<u64>) -> Result<Reason, SchedError> {
        let idx = {
            let mut g = self.inner.lock();
            let cur = g.current.ok_or(SchedError::InvalidState)?;
            if cur != t.id {
                return Err(SchedError::InvalidState);
            }
            let now = g.ticks;
            let i = g.resolve(cur).ok_or(SchedError::Stale)?;
            let s = &mut g.slots[i];
            if s.killed || s.seq != t.seq {
                return Ok(Self::take_pending(s));
            }
            if let Some(d) = deadline
                && d <= now
            {
                s.pending = Reason::None;
                return Ok(Reason::Timeout);
            }
            s.deadline = deadline;
            s.ticket_seq = t.seq;
            s.action = Action::Block;
            i
        };
        self.leave(idx);
        let mut g = self.inner.lock();
        let i = g.resolve(t.id).ok_or(SchedError::Stale)?;
        g.slots[i].deadline = None;
        Ok(Self::take_pending(&mut g.slots[i]))
    }

    pub fn sleep_ticks(&self, n: u64) -> Result<Reason, SchedError> {
        let t = self.prepare_block()?;
        let deadline = self.ticks().saturating_add(n);
        self.block(t, Some(deadline))
    }

    pub fn exit_current(&self, code: i64) -> ! {
        let idx = {
            let mut g = self.inner.lock();
            let id = match g.current {
                Some(id) => id,
                None => panic!("exit_current outside a thread"),
            };
            let i = g.resolve(id).expect("current thread vanished");
            g.slots[i].action = Action::Exit(code);
            i
        };
        self.leave(idx);
        panic!("exited thread was resumed");
    }

    // ---- wake side (thread, interrupt or other-CPU context) --------------------------

    pub fn wake(&self, id: ThreadId) -> Result<bool, SchedError> {
        self.wake_with(id, Reason::Woken)
    }
    pub fn cancel(&self, id: ThreadId) -> Result<bool, SchedError> {
        self.wake_with(id, Reason::Cancelled)
    }
    fn wake_with(&self, id: ThreadId, r: Reason) -> Result<bool, SchedError> {
        let mut g = self.inner.lock();
        let i = g.resolve(id).ok_or(SchedError::Stale)?;
        Ok(g.wake_slot(i, r))
    }

    /// Mark a thread killed. Sticky; takes effect at its next block, syscall boundary or
    /// first run. There is no asynchronous preemption in this batch.
    pub fn kill(&self, id: ThreadId) -> Result<(), SchedError> {
        let mut g = self.inner.lock();
        let i = g.resolve(id).ok_or(SchedError::Stale)?;
        g.slots[i].killed = true;
        g.wake_slot(i, Reason::Killed);
        Ok(())
    }

    /// Kill every thread of a user address space. Returns how many were marked.
    pub fn terminate_space(&self, space: SpaceId) -> Result<usize, SchedError> {
        if space == KERNEL_SPACE {
            return Err(SchedError::InvalidState);
        }
        let mut g = self.inner.lock();
        let mut n = 0;
        for i in 0..MAX_THREADS {
            if g.slots[i].state != St::Free && g.slots[i].space == space && !g.slots[i].killed {
                g.slots[i].killed = true;
                g.wake_slot(i, Reason::Killed);
                n += 1;
            }
        }
        Ok(n)
    }

    /// Timer tick: advance time and wake expired sleepers.
    pub fn tick(&self) -> u64 {
        let mut g = self.inner.lock();
        g.ticks += 1;
        let now = g.ticks;
        for i in 0..MAX_THREADS {
            if g.slots[i].state != St::Free
                && let Some(d) = g.slots[i].deadline
                && d <= now
            {
                g.wake_slot(i, Reason::Timeout);
            }
        }
        now
    }

    // ---- the scheduler context ---------------------------------------------------------

    fn pick(&self, g: &mut Inner) -> Option<Pick> {
        while let Some(i) = g.dequeue() {
            if g.slots[i].state != St::Runnable {
                continue;
            }
            let generation = g.slots[i].generation;
            if g.slots[i].killed && !g.slots[i].started {
                // Never ran: nothing to unwind, reap without switching in.
                g.stats.killed_before_run += 1;
                g.free(i);
                continue;
            }
            let rsp = self.saved[i].load(Ordering::Acquire);
            let base = self.stack_base(i);
            // SAFETY: `base` is this slot's stack, initialised by `spawn`.
            let intact = unsafe { context::canary_ok(base) };
            if !intact || !context::validate_saved_rsp(base as usize, STACK_SIZE, rsp) {
                g.stats.refused_switches += 1;
                g.free(i);
                continue;
            }
            let space = g.slots[i].space;
            let hook = if g.loaded_space != space {
                g.loaded_space = space;
                g.stats.space_loads += 1;
                g.space_hook
            } else {
                None
            };
            g.slots[i].state = St::Running;
            g.slots[i].started = true;
            g.stats.switches += 1;
            let id = ThreadId {
                index: i as u16,
                generation,
            };
            g.current = Some(id);
            return Some(Pick { id, rsp, hook });
        }
        None
    }

    fn commit(&self, g: &mut Inner, id: ThreadId) {
        let idx = id.index as usize;
        g.current = None;
        // SAFETY: slot stacks are always initialised.
        let intact = unsafe { context::canary_ok(self.stack_base(idx)) };
        let action = g.slots[idx].action;
        g.slots[idx].action = Action::None;
        if !intact {
            g.stats.stack_corruptions += 1;
            g.free(idx);
            return;
        }
        match action {
            Action::Yield => {
                g.slots[idx].state = St::Runnable;
                g.enqueue(idx);
            }
            Action::Block => {
                let s = &mut g.slots[idx];
                if s.killed || s.seq != s.ticket_seq {
                    // A wake arrived after the snapshot but before we got here.
                    s.state = St::Runnable;
                    g.enqueue(idx);
                } else {
                    s.state = St::Blocked;
                }
            }
            Action::Exit(code) => {
                let n = g.exit_count;
                g.exit_log[n % MAX_THREADS] = (id.raw(), code);
                g.exit_count += 1;
                g.stats.exited += 1;
                g.free(idx);
            }
            Action::None => {
                g.stats.bad_switches += 1;
                g.free(idx);
            }
        }
    }

    /// Run threads until none are left or `idle` (called when nothing is runnable) says stop.
    /// `idle` is where a bare-metal kernel would `sti; hlt`, and where a test ticks time.
    pub fn run<F: FnMut(&Sched) -> bool>(&self, mut idle: F) -> RunExit {
        {
            let mut g = self.inner.lock();
            assert!(!g.in_run, "nested Sched::run");
            g.in_run = true;
        }
        let exit = loop {
            let picked = {
                let mut g = self.inner.lock();
                self.pick(&mut g)
            };
            match picked {
                Some(p) => {
                    if let Some(h) = p.hook {
                        h(self.inner.lock().loaded_space);
                    }
                    // SAFETY: `p.rsp` passed `validate_saved_rsp` and the canary check, and
                    // the thread is not running (it was Runnable).
                    unsafe { context::switch(self.sched_rsp.as_ptr(), p.rsp) };
                    #[cfg(test)]
                    {
                        let h = self.inner.lock().pre_commit;
                        if let Some(h) = h {
                            h(self, p.id);
                        }
                    }
                    let mut g = self.inner.lock();
                    self.commit(&mut g, p.id);
                }
                None => {
                    if self.inner.lock().live == 0 {
                        break RunExit::AllDone;
                    }
                    if !idle(self) {
                        break RunExit::Stopped;
                    }
                }
            }
        };
        self.inner.lock().in_run = false;
        exit
    }

    // ---- test-only fault injection ---------------------------------------------------

    #[cfg(test)]
    pub(crate) fn test_set_pre_commit(&self, h: Option<fn(&Sched, ThreadId)>) {
        self.inner.lock().pre_commit = h;
    }
    #[cfg(test)]
    pub(crate) fn test_set_saved_rsp(&self, id: ThreadId, v: usize) {
        self.saved[id.index()].store(v, Ordering::Release);
    }
    #[cfg(test)]
    pub(crate) fn test_stack_base(&self, id: ThreadId) -> *mut u8 {
        self.stack_base(id.index())
    }
}

#[cfg(test)]
impl Sched {
    pub(crate) unsafe fn test_peek_saved(&self, id: ThreadId) -> usize {
        self.saved[id.index()].load(Ordering::Acquire)
    }
}
