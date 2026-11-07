//! Logic shared by the boot-time self-tests and the host tests: what a probed trap must look
//! like, and the scheduler ordering scenario that runs on the Batch 7 scheduler both here on
//! the host and in the booted kernel.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use omni_kernel7::sched::{KERNEL_SPACE, RunExit, Sched, SchedError};

/// What the trap dispatcher recorded for one probed instruction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Observation {
    pub vector: u64,
    pub error: u64,
    pub cr2: u64,
    pub from_user: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Expectation {
    pub vector: u64,
    /// `None` means "any error code" (for example a #BP has none).
    pub error: Option<u64>,
    pub cr2: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mismatch {
    NoTrap,
    Vector,
    ErrorCode,
    FaultAddress,
    WrongRing,
}

pub fn check(obs: Option<Observation>, exp: Expectation) -> Result<(), Mismatch> {
    let o = obs.ok_or(Mismatch::NoTrap)?;
    if o.from_user {
        return Err(Mismatch::WrongRing);
    }
    if o.vector != exp.vector {
        return Err(Mismatch::Vector);
    }
    if let Some(e) = exp.error
        && o.error != e
    {
        return Err(Mismatch::ErrorCode);
    }
    if let Some(a) = exp.cr2
        && o.cr2 != a
    {
        return Err(Mismatch::FaultAddress);
    }
    Ok(())
}

/// Three threads each record their number and yield three times.
pub const THREADS: usize = 3;
pub const ROUNDS: usize = 3;
pub const EXPECTED_ORDER: [u8; THREADS * ROUNDS] = [0, 1, 2, 0, 1, 2, 0, 1, 2];
/// Context switches into threads: (ROUNDS + 1) slices per thread.
pub const EXPECTED_SWITCHES: u64 = (THREADS * (ROUNDS + 1)) as u64;

pub struct OrderLog {
    len: AtomicUsize,
    slots: [AtomicU8; THREADS * ROUNDS],
}

impl Default for OrderLog {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderLog {
    pub const fn new() -> Self {
        OrderLog {
            len: AtomicUsize::new(0),
            slots: [const { AtomicU8::new(0xFF) }; THREADS * ROUNDS],
        }
    }
    fn push(&self, v: u8) {
        let i = self.len.fetch_add(1, Ordering::SeqCst);
        if let Some(s) = self.slots.get(i) {
            s.store(v, Ordering::SeqCst);
        }
    }
    pub fn len(&self) -> usize {
        self.len.load(Ordering::SeqCst)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn snapshot(&self) -> [u8; THREADS * ROUNDS] {
        let mut out = [0xFF; THREADS * ROUNDS];
        for (o, s) in out.iter_mut().zip(self.slots.iter()) {
            *o = s.load(Ordering::SeqCst);
        }
        out
    }
}

pub struct ThreadCtx<'a> {
    pub sched: &'a Sched,
    pub log: &'a OrderLog,
    pub id: u8,
}

/// Thread entry for the scenario; `arg` is a `*const ThreadCtx` that outlives the run.
pub extern "C" fn order_thread(arg: usize) {
    // SAFETY: `run_order_scenario` passes a pointer to a live ThreadCtx.
    let ctx = unsafe { &*(arg as *const ThreadCtx) };
    for _ in 0..ROUNDS {
        ctx.log.push(ctx.id);
        let _ = ctx.sched.yield_now();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScenarioError {
    Spawn(SchedError),
    Run(RunExit),
    Order,
    Switches(u64),
    Leak(usize),
}

/// Spawn the threads, run to completion and check order, switch count and that every
/// thread slot was reclaimed. The scheduler must be idle and pinned in memory.
pub fn run_order_scenario(sched: &Sched, log: &OrderLog) -> Result<(), ScenarioError> {
    let before = sched.stats().switches;
    let ctxs = [
        ThreadCtx { sched, log, id: 0 },
        ThreadCtx { sched, log, id: 1 },
        ThreadCtx { sched, log, id: 2 },
    ];
    for c in &ctxs {
        sched
            .spawn(KERNEL_SPACE, order_thread, c as *const ThreadCtx as usize)
            .map_err(ScenarioError::Spawn)?;
    }
    let exit = sched.run(|_| false);
    if exit != RunExit::AllDone {
        return Err(ScenarioError::Run(exit));
    }
    if log.len() != THREADS * ROUNDS || log.snapshot() != EXPECTED_ORDER {
        return Err(ScenarioError::Order);
    }
    let switches = sched.stats().switches - before;
    if switches != EXPECTED_SWITCHES {
        return Err(ScenarioError::Switches(switches));
    }
    if sched.live() != 0 {
        return Err(ScenarioError::Leak(sched.live()));
    }
    Ok(())
}
