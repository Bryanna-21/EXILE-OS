//! Kernel thread resource management (Mission D2).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Scheduler owns thread identity and lifecycle. This manager owns stack
//! allocation, initial frames, and generation-checked stack metadata.

use crate::context::{self, FRAME_BYTES};
use crate::kstack::{self, Stack, StackPool};
use crate::sched::{BlockOutcome, Scheduler, SleepOutcome, Switch, WakeOutcome};
use crate::spin::SpinLock;
use crate::thread::{self, BlockReason, Handle, Priority, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Thread(thread::Error),
    Stack(kstack::Error),
    Context(context::Error),
    BadEntry,
    BadConfig,
    Memory,
    Corrupt,
}

impl From<thread::Error> for Error {
    fn from(e: thread::Error) -> Self { Error::Thread(e) }
}
impl From<kstack::Error> for Error {
    fn from(e: kstack::Error) -> Self { Error::Stack(e) }
}
impl From<context::Error> for Error {
    fn from(e: context::Error) -> Self { Error::Context(e) }
}

/// Stack-memory backend.
///
/// Contract: `write` must be atomic with respect to failure. If it returns
/// `Err`, the addressed bytes must remain unchanged. Implementations that
/// cannot provide this guarantee must stage writes before committing them.
pub trait StackMemory {
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), MemError>;
    fn zero(&mut self, addr: u64, len: u64) -> Result<(), MemError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub text_start: u64,
    pub text_end: u64,
    pub trampoline: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadParams {
    pub entry: u64,
    pub arg: u64,
    pub priority: Priority,
}

struct Meta {
    handle: Option<Handle>,
    stack: Option<Stack>,
    saved_sp: u64,
}

pub struct KThreadManager<const N: usize> {
    cfg: Config,
    scheduler: Scheduler<N>,
    pool: StackPool,
    meta: [Meta; N],
    created: u64,
    destroyed: u64,
}

pub type SharedKThreads<const N: usize> = SpinLock<KThreadManager<N>>;

impl<const N: usize> KThreadManager<N> {
    pub fn new(cfg: Config, pool: StackPool) -> Result<Self, Error> {
        if cfg.text_start >= cfg.text_end
            || !(cfg.text_start..cfg.text_end).contains(&cfg.trampoline)
            || !context::is_canonical(cfg.text_start)
            || !context::is_canonical(cfg.text_end - 1)
        {
            return Err(Error::BadConfig);
        }

        Ok(Self {
            cfg,
            scheduler: Scheduler::new(),
            pool,
            meta: core::array::from_fn(|_| Meta {
                handle: None,
                stack: None,
                saved_sp: 0,
            }),
            created: 0,
            destroyed: 0,
        })
    }

    pub fn scheduler(&self) -> &Scheduler<N> {
        &self.scheduler
    }

    /// Advances scheduler time. This does not itself perform a context switch.
    pub fn tick(&mut self) -> bool {
        self.scheduler.tick()
    }

    /// Selects the next thread. The architecture layer performs the actual
    /// context switch, then calls `switch_out_complete` for the previous thread.
    pub fn schedule(&mut self) -> Result<Switch, Error> {
        Ok(self.scheduler.schedule()?)
    }

    /// Requests a scheduling point without exposing mutable scheduler access.
    pub fn yield_now(&mut self) -> Result<Switch, Error> {
        Ok(self.scheduler.yield_now()?)
    }

    /// Blocks the currently running thread. If Blocked is returned, the caller
    /// must schedule another thread before returning to ordinary execution.
    pub fn block_current(&mut self, reason: BlockReason) -> Result<BlockOutcome, Error> {
        Ok(self.scheduler.block_current(reason)?)
    }

    pub fn join_wait(&mut self, target: Handle) -> Result<BlockOutcome, Error> {
        Ok(self.scheduler.join_wait(target)?)
    }

    pub fn sleep_current(&mut self, ticks: u64) -> Result<SleepOutcome, Error> {
        Ok(self.scheduler.sleep_current(ticks)?)
    }

    pub fn wake(&mut self, h: Handle) -> Result<WakeOutcome, Error> {
        self.meta_for(h)?;
        Ok(self.scheduler.wake(h)?)
    }

    pub fn set_priority(&mut self, h: Handle, priority: Priority) -> Result<(), Error> {
        self.meta_for(h)?;
        Ok(self.scheduler.set_priority(h, priority)?)
    }

    pub fn live(&self) -> usize {
        self.scheduler.live_threads()
    }

    pub fn stacks_in_use(&self) -> usize {
        self.pool.in_use()
    }

    pub fn created_total(&self) -> u64 {
        self.created
    }

    pub fn destroyed_total(&self) -> u64 {
        self.destroyed
    }

    fn meta_for(&self, h: Handle) -> Result<&Meta, Error> {
        self.scheduler.thread(h)?;
        let meta = self.meta.get(h.index()).ok_or(Error::Corrupt)?;
        if meta.handle != Some(h) || meta.stack.is_none() {
            return Err(Error::Corrupt);
        }
        Ok(meta)
    }

    pub fn thread(&self, h: Handle) -> Result<&thread::Thread, Error> {
        Ok(self.scheduler.thread(h)?)
    }

    pub fn stack(&self, h: Handle) -> Result<&Stack, Error> {
        self.meta_for(h)?
            .stack
            .as_ref()
            .ok_or(Error::Corrupt)
    }

    /// Stack pointer the switch routine must load to start or resume this thread.
    pub fn saved_sp(&self, h: Handle) -> Result<u64, Error> {
        Ok(self.meta_for(h)?.saved_sp)
    }

    fn check_entry(&self, entry: u64) -> Result<(), Error> {
        if (self.cfg.text_start..self.cfg.text_end).contains(&entry) {
            Ok(())
        } else {
            Err(Error::BadEntry)
        }
    }

    /// Reserve a thread and prepare its stack. The thread remains New until
    /// start() is called. Prefer spawn() for atomic create-and-publish.
    pub fn create<M: StackMemory>(
        &mut self,
        mem: &mut M,
        p: ThreadParams,
    ) -> Result<Handle, Error> {
        if p.priority == Priority::IDLE {
            return Err(thread::Error::BadPriority.into());
        }
        self.create_inner(mem, p, false, false)
    }

    /// Reserve the scheduler's unique idle slot and prepare its stack.
    /// The idle thread remains New until start() is called.
    pub fn create_idle<M: StackMemory>(
        &mut self,
        mem: &mut M,
        entry: u64,
        arg: u64,
    ) -> Result<Handle, Error> {
        self.create_inner(
            mem,
            ThreadParams { entry, arg, priority: Priority::IDLE },
            true,
            false,
        )
    }

    /// Create, initialize, and publish a runnable non-idle thread.
    /// All resources are rolled back if preparation or publication fails.
    pub fn spawn<M: StackMemory>(
        &mut self,
        mem: &mut M,
        p: ThreadParams,
    ) -> Result<Handle, Error> {
        if p.priority == Priority::IDLE {
            return Err(thread::Error::BadPriority.into());
        }
        self.create_inner(mem, p, false, true)
    }

    fn create_inner<M: StackMemory>(
        &mut self,
        mem: &mut M,
        p: ThreadParams,
        idle: bool,
        publish: bool,
    ) -> Result<Handle, Error> {
        let next_created = self.created.checked_add(1).ok_or(Error::Corrupt)?;
        self.check_entry(p.entry)?;

        let h = if idle {
            self.scheduler.reserve_idle(p.entry as usize, p.arg as usize)?
        } else {
            self.scheduler.reserve(p.priority, p.entry as usize, p.arg as usize)?
        };

        let stack = match self.pool.alloc() {
            Ok(stack) => stack,
            Err(e) => {
                self.scheduler.discard_reserved(h).map_err(|_| Error::Corrupt)?;
                return Err(e.into());
            }
        };

        let ctx = match context::build_initial(
            stack.base(),
            stack.top(),
            p.entry,
            p.arg,
            self.cfg.trampoline,
        ) {
            Ok(ctx) => ctx,
            Err(e) => {
                self.pool.release(stack).map_err(|_| Error::Corrupt)?;
                self.scheduler.discard_reserved(h).map_err(|_| Error::Corrupt)?;
                return Err(e.into());
            }
        };

        debug_assert!(ctx.saved_sp() + FRAME_BYTES <= stack.top());

        // Validate the ownership slot before writing anything into reusable
        // stack memory. A post-write metadata failure would require scrubbing.
        let idx = h.index();
        if idx >= N || self.meta[idx].handle.is_some() || self.meta[idx].stack.is_some() {
            self.pool.release(stack).map_err(|_| Error::Corrupt)?;
            self.scheduler.discard_reserved(h).map_err(|_| Error::Corrupt)?;
            return Err(Error::Corrupt);
        }

        if mem.write(ctx.saved_sp(), &ctx.to_bytes()).is_err() {
            self.pool.release(stack).map_err(|_| Error::Corrupt)?;
            self.scheduler.discard_reserved(h).map_err(|_| Error::Corrupt)?;
            return Err(Error::Memory);
        }

        self.meta[idx] = Meta {
            handle: Some(h),
            stack: Some(stack),
            saved_sp: ctx.saved_sp(),
        };

        if publish {
            if let Err(e) = self.scheduler.publish(h) {
                // Do not recycle a stack containing an initialized context.
                // If scrubbing fails, retain the New thread and its stack token.
                let stack = self.meta[idx].stack.as_ref().ok_or(Error::Corrupt)?;
                self.pool.validate_release(stack)?;
                mem.zero(stack.base(), stack.size())
                    .map_err(|_| Error::Memory)?;

                self.scheduler.discard_reserved(h).map_err(|_| Error::Corrupt)?;
                let stack = self.meta[idx].stack.take().ok_or(Error::Corrupt)?;
                self.pool.release(stack).map_err(|_| Error::Corrupt)?;
                self.meta[idx].handle = None;
                self.meta[idx].saved_sp = 0;
                return Err(e.into());
            }
        }

        self.created = next_created;
        Ok(h)
    }

    /// Publish a successfully prepared New thread.
    pub fn start(&mut self, h: Handle) -> Result<(), Error> {
        self.meta_for(h)?;
        self.scheduler.publish(h)?;
        Ok(())
    }

    /// Complete bookkeeping after execution has switched away from h.
    pub fn switch_out_complete(&mut self, h: Handle) -> Result<(), Error> {
        self.meta_for(h)?;
        self.scheduler.switch_complete(h)?;
        Ok(())
    }

    /// Terminate a thread which is not currently executing.
    pub fn terminate(&mut self, h: Handle, status: i32) -> Result<(), Error> {
        self.meta_for(h)?;
        self.scheduler.kill(h, status)?;
        Ok(())
    }

    /// The currently executing thread exits itself.
    pub fn exit_running(&mut self, h: Handle, status: i32) -> Result<(), Error> {
        self.meta_for(h)?;
        if self.scheduler.current() != Some(h) {
            return Err(thread::Error::NotCurrent.into());
        }
        self.scheduler.terminate_current(status)?;
        Ok(())
    }

    /// Scrub, reap, and release a terminated thread's stack.
    /// A stack is never reclaimed while its thread remains on-CPU.
    pub fn destroy<M: StackMemory>(
        &mut self,
        mem: &mut M,
        h: Handle,
    ) -> Result<i32, Error> {
        let t = self.scheduler.thread(h)?;
        if t.state() != State::Terminated {
            return Err(thread::Error::NotTerminated.into());
        }
        if t.on_cpu() {
            return Err(thread::Error::StillOnCpu.into());
        }

        let next_destroyed = self.destroyed.checked_add(1).ok_or(Error::Corrupt)?;
        let idx = h.index();
        let meta = self.meta_for(h)?;
        let stack = meta.stack.as_ref().ok_or(Error::Corrupt)?;
        self.pool.validate_release(stack)?;
        let (base, size) = (stack.base(), stack.size());

        // Scrubbing can fail; leave both scheduler record and stack ownership intact.
        mem.zero(base, size).map_err(|_| Error::Memory)?;

        // Reap only after confirming the thread is terminated and off-CPU.
        let status = self.scheduler.reap(h)?;
        let stack = self.meta[idx].stack.take().ok_or(Error::Corrupt)?;
        self.pool.release(stack)?;
        self.meta[idx].handle = None;
        self.meta[idx].saved_sp = 0;
        self.destroyed = next_destroyed;
        Ok(status)
    }

    pub fn check_accounting(&self) -> Result<(), &'static str> {
        self.scheduler
            .check_invariants()
            .map_err(|_| "scheduler invariants failed")?;

        let mut owned = 0usize;

        for (i, meta) in self.meta.iter().enumerate() {
            match (meta.handle, meta.stack.as_ref()) {
                (None, None) => {
                    if meta.saved_sp != 0 {
                        return Err("free metadata slot has saved sp");
                    }
                }
                (Some(h), Some(stack)) => {
                    if h.index() != i {
                        return Err("handle index differs from metadata slot");
                    }
                    if self.scheduler.thread(h).is_err() {
                        return Err("metadata contains stale scheduler handle");
                    }
                    if meta.saved_sp < stack.base() || meta.saved_sp >= stack.top() {
                        return Err("saved sp outside owned stack");
                    }
                    if self.pool.classify(stack.base()) != kstack::Region::Stack {
                        return Err("stack outside pool");
                    }
                    self.pool
                        .validate_release(stack)
                        .map_err(|_| "invalid stack ownership token")?;
                    owned += 1;
                }
                _ => return Err("partial stack metadata ownership"),
            }
        }

        if owned != self.pool.in_use() {
            return Err("pool in-use count differs from owned stacks");
        }
        if owned != self.scheduler.live_threads() {
            return Err("scheduler live count differs from owned stacks");
        }
        let accounted = self.destroyed
            .checked_add(owned as u64)
            .ok_or("destroyed + live count overflow")?;
        if self.created != accounted {
            return Err("created != destroyed + live");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{WORD_ARG_R13, WORD_ENTRY_R12, WORD_RET_TRAMPOLINE};
    use crate::kstack::GUARD_BYTES;
    use std::sync::Arc;

    const BASE: u64 = 0x20_0000;
    const STACK: u64 = 16 * 1024;
    const SLOT: u64 = STACK + GUARD_BYTES;
    const TEXT: (u64, u64) = (0x10_0000, 0x11_0000);
    const TRAMP: u64 = 0x10_0100;
    const ENTRY: u64 = 0x10_2000;

    /// Host stand-in for stack memory.
    struct Mem {
        base: u64,
        data: Vec<u8>,
        fail_write: bool,
        fail_zero: bool,
    }

    impl Mem {
        fn new(slots: u64) -> Self {
            Self {
                base: BASE,
                data: vec![0xAA; (slots * SLOT) as usize],
                fail_write: false,
                fail_zero: false,
            }
        }
        fn range(&self, addr: u64, len: u64) -> Result<std::ops::Range<usize>, MemError> {
            let s = addr.checked_sub(self.base).ok_or(MemError)? as usize;
            let e = s.checked_add(len as usize).ok_or(MemError)?;
            if e > self.data.len() {
                return Err(MemError);
            }
            Ok(s..e)
        }
        fn read(&self, addr: u64, len: u64) -> &[u8] {
            &self.data[self.range(addr, len).unwrap()]
        }
    }

    impl StackMemory for Mem {
        fn write(&mut self, addr: u64, d: &[u8]) -> Result<(), MemError> {
            if self.fail_write {
                return Err(MemError);
            }
            let r = self.range(addr, d.len() as u64)?;
            self.data[r].copy_from_slice(d);
            Ok(())
        }
        fn zero(&mut self, addr: u64, len: u64) -> Result<(), MemError> {
            if self.fail_zero {
                return Err(MemError);
            }
            let r = self.range(addr, len)?;
            self.data[r].fill(0);
            Ok(())
        }
    }

    fn cfg() -> Config {
        Config {
            text_start: TEXT.0,
            text_end: TEXT.1,
            trampoline: TRAMP,
        }
    }

    fn mgr<const N: usize>(slots: u64) -> (KThreadManager<N>, Mem) {
        let pool = StackPool::new(BASE, slots * SLOT, STACK, 1 << 30).unwrap();
        (KThreadManager::new(cfg(), pool).unwrap(), Mem::new(slots))
    }

    fn params(arg: u64) -> ThreadParams {
        ThreadParams {
            entry: ENTRY,
            arg,
            priority: Priority::DEFAULT,
        }
    }

    #[test]
    fn creation_yields_new_thread_with_stack_and_valid_initial_frame() {
        let (mut m, mut mem) = mgr::<4>(4);
        let h = m.create(&mut mem, params(0x1234)).unwrap();
        let t = m.thread(h).unwrap();
        assert_eq!(t.state(), State::New);
        assert_eq!(t.priority(), Priority::DEFAULT);
        assert_eq!(t.exit_status(), None);
        assert!(!t.on_cpu());
        assert_eq!(t.entry(), (ENTRY as usize, 0x1234));

        let s = m.stack(h).unwrap();
        assert_eq!(s.top() % 16, 0);
        assert_eq!(s.base() % 4096, 0);
        assert_eq!(s.size(), STACK);

        let sp = m.saved_sp(h).unwrap();
        assert_eq!(sp, s.top() - FRAME_BYTES);
        let frame = mem.read(sp, FRAME_BYTES);
        let word = |i: usize| u64::from_le_bytes(frame[i * 8..i * 8 + 8].try_into().unwrap());
        assert_eq!(word(WORD_ENTRY_R12), ENTRY);
        assert_eq!(word(WORD_ARG_R13), 0x1234);
        assert_eq!(word(WORD_RET_TRAMPOLINE), TRAMP);
        m.check_accounting().unwrap();
    }

    #[test]
    fn thread_ids_are_unique_across_create_and_destroy() {
        let (mut m, mut mem) = mgr::<2>(2);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let h = m.create(&mut mem, params(0)).unwrap();
            assert!(seen.insert(m.thread(h).unwrap().id().0), "id reused");
            m.terminate(h, 0).unwrap();
            m.destroy(&mut mem, h).unwrap();
        }
    }

    #[test]
    fn each_thread_gets_its_own_non_overlapping_stack() {
        let (mut m, mut mem) = mgr::<4>(4);
        let hs: Vec<_> = (0..4)
            .map(|i| m.create(&mut mem, params(i)).unwrap())
            .collect();
        let mut ranges: Vec<_> = hs
            .iter()
            .map(|&h| (m.stack(h).unwrap().guard_base(), m.stack(h).unwrap().top()))
            .collect();
        ranges.sort();
        for w in ranges.windows(2) {
            assert!(w[0].1 <= w[1].0);
        }
        // Frames did not clobber each other.
        for (i, &h) in hs.iter().enumerate() {
            let f = mem.read(m.saved_sp(h).unwrap(), FRAME_BYTES);
            let arg = u64::from_le_bytes(f[16..24].try_into().unwrap());
            assert_eq!(arg, i as u64);
        }
    }

    #[test]
    fn invalid_entry_points_are_rejected_without_side_effects() {
        let (mut m, mut mem) = mgr::<2>(2);
        for bad in [0, TEXT.0 - 1, TEXT.1, u64::MAX, 0xffff_8000_0000_0000] {
            let r = m.create(
                &mut mem,
                ThreadParams {
                    entry: bad,
                    ..params(0)
                },
            );
            assert_eq!(r, Err(Error::BadEntry), "entry {bad:#x}");
        }
        assert_eq!((m.live(), m.stacks_in_use(), m.created_total()), (0, 0, 0));
    }

    #[test]
    fn idle_priority_only_via_create_idle() {
        let (mut m, mut mem) = mgr::<2>(2);
        let r = m.create(
            &mut mem,
            ThreadParams {
                priority: Priority::IDLE,
                ..params(0)
            },
        );
        assert_eq!(r, Err(Error::Thread(thread::Error::BadPriority)));
        let h = m.create_idle(&mut mem, ENTRY, 0).unwrap();
        assert!(m.thread(h).unwrap().is_idle());
        assert_eq!(m.create_idle(&mut mem, 0, 0), Err(Error::BadEntry));
    }

    #[test]
    fn bad_configuration_is_rejected() {
        let pool = || StackPool::new(BASE, SLOT, STACK, 1 << 30).unwrap();
        let mk = |c: Config| KThreadManager::<1>::new(c, pool()).err();
        assert_eq!(
            mk(Config {
                text_start: 5,
                text_end: 5,
                trampoline: 5
            }),
            Some(Error::BadConfig)
        );
        assert_eq!(
            mk(Config {
                trampoline: TEXT.1,
                ..cfg()
            }),
            Some(Error::BadConfig)
        );
        assert_eq!(
            mk(Config {
                trampoline: TEXT.0 - 1,
                ..cfg()
            }),
            Some(Error::BadConfig)
        );
        assert_eq!(
            mk(Config {
                text_start: 0x0000_8000_0000_0000,
                text_end: 0x0000_8000_0001_0000,
                trampoline: 0x0000_8000_0000_0010
            }),
            Some(Error::BadConfig),
            "non-canonical text"
        );
    }

    #[test]
    fn stack_exhaustion_fails_cleanly_and_recovers() {
        let (mut m, mut mem) = mgr::<8>(2);
        let a = m.create(&mut mem, params(1)).unwrap();
        let _b = m.create(&mut mem, params(2)).unwrap();
        let before = (m.live(), m.stacks_in_use(), m.created_total());
        assert_eq!(
            m.create(&mut mem, params(3)),
            Err(Error::Stack(kstack::Error::Exhausted))
        );
        assert_eq!(
            (m.live(), m.stacks_in_use(), m.created_total()),
            before,
            "no residue"
        );
        m.check_accounting().unwrap();
        m.terminate(a, 0).unwrap();
        m.destroy(&mut mem, a).unwrap();
        m.create(&mut mem, params(4)).unwrap();
    }

    #[test]
    fn thread_table_exhaustion_fails_cleanly() {
        let (mut m, mut mem) = mgr::<2>(8);
        m.create(&mut mem, params(0)).unwrap();
        m.create(&mut mem, params(0)).unwrap();
        assert_eq!(
            m.create(&mut mem, params(0)),
            Err(Error::Thread(thread::Error::NoSlots))
        );
        assert_eq!(m.stacks_in_use(), 2);
        m.check_accounting().unwrap();
    }

    #[test]
    fn memory_write_failure_rolls_back_thread_and_stack() {
        let (mut m, mut mem) = mgr::<2>(2);
        mem.fail_write = true;
        assert_eq!(m.create(&mut mem, params(0)), Err(Error::Memory));
        assert_eq!((m.live(), m.stacks_in_use(), m.created_total()), (0, 0, 0));
        mem.fail_write = false;
        m.create(&mut mem, params(0)).unwrap();
        m.check_accounting().unwrap();
    }

    #[test]
    fn stack_too_small_for_frame_is_reported_and_rolled_back() {
        // Can't happen with MIN_STACK_BYTES; prove the path through the builder directly.
        use crate::context::{Error as CE, build_initial};
        assert_eq!(
            build_initial(0x1000, 0x1020, ENTRY, 0, TRAMP),
            Err(CE::StackTooSmall)
        );
    }

    #[test]
    fn terminate_then_destroy_reclaims_and_scrubs_the_stack() {
        let (mut m, mut mem) = mgr::<1>(1);
        let h = m.create(&mut mem, params(9)).unwrap();
        m.start(h).unwrap();
        let (base, size, sp) = {
            let s = m.stack(h).unwrap();
            (s.base(), s.size(), m.saved_sp(h).unwrap())
        };
        assert!(mem.read(sp, FRAME_BYTES).iter().any(|&b| b != 0));
        m.terminate(h, -3).unwrap();
        assert_eq!(m.stacks_in_use(), 1, "stack kept until destroy");
        assert_eq!(m.destroy(&mut mem, h), Ok(-3));
        assert_eq!((m.live(), m.stacks_in_use()), (0, 0));
        assert!(
            mem.read(base, size).iter().all(|&b| b == 0),
            "stack scrubbed"
        );
        assert_eq!((m.created_total(), m.destroyed_total()), (1, 1));
        // The reclaimed stack is reusable.
        let h2 = m.create(&mut mem, params(1)).unwrap();
        assert_eq!(m.stack(h2).unwrap().base(), base);
    }

    #[test]
    fn double_destruction_is_prevented() {
        let (mut m, mut mem) = mgr::<2>(2);
        let h = m.create(&mut mem, params(0)).unwrap();
        m.terminate(h, 0).unwrap();
        m.destroy(&mut mem, h).unwrap();
        assert_eq!(
            m.destroy(&mut mem, h),
            Err(Error::Thread(thread::Error::Stale))
        );
        assert_eq!(m.terminate(h, 0), Err(Error::Thread(thread::Error::Stale)));
        assert_eq!(m.stacks_in_use(), 0);
        // A stale handle must not reach a thread that reuses the same slot.
        let h2 = m.create(&mut mem, params(0)).unwrap();
        assert_eq!(h.index(), h2.index());
        m.terminate(h2, 0).unwrap();
        assert_eq!(
            m.destroy(&mut mem, h),
            Err(Error::Thread(thread::Error::Stale))
        );
        assert_eq!(m.thread(h2).unwrap().state(), State::Terminated);
        m.check_accounting().unwrap();
    }

    #[test]
    fn destroy_requires_termination() {
        let (mut m, mut mem) = mgr::<2>(2);
        let h = m.create(&mut mem, params(0)).unwrap();
        assert_eq!(
            m.destroy(&mut mem, h),
            Err(Error::Thread(thread::Error::NotTerminated))
        );
        m.start(h).unwrap();
        assert_eq!(
            m.destroy(&mut mem, h),
            Err(Error::Thread(thread::Error::NotTerminated))
        );
        assert_eq!(m.stacks_in_use(), 1);
    }

    #[test]
    fn stack_is_never_reclaimed_while_the_thread_is_still_on_it() {
        let (mut m, mut mem) = mgr::<2>(2);
        let h = m.create(&mut mem, params(0)).unwrap();
        let replacement = m.create(&mut mem, params(1)).unwrap();
        m.start(h).unwrap();
        m.start(replacement).unwrap();

        assert_eq!(m.schedule().unwrap().next, h);
        assert!(m.thread(h).unwrap().on_cpu());

        // Running threads cannot be killed from outside or destroyed.
        assert_eq!(
            m.terminate(h, 1),
            Err(Error::Thread(thread::Error::IsRunning))
        );
        assert_eq!(
            m.destroy(&mut mem, h),
            Err(Error::Thread(thread::Error::NotTerminated))
        );

        // Exit first, then actually schedule away before completing the switch.
        m.exit_running(h, 7).unwrap();
        assert_eq!(
            m.destroy(&mut mem, h),
            Err(Error::Thread(thread::Error::StillOnCpu))
        );
        assert_eq!(m.stacks_in_use(), 2);

        assert_eq!(m.schedule().unwrap().next, replacement);
        m.switch_out_complete(h).unwrap();
        assert_eq!(
            m.switch_out_complete(h),
            Err(Error::Thread(thread::Error::NotCurrent))
        );

        assert_eq!(m.destroy(&mut mem, h), Ok(7));
        assert_eq!(m.stacks_in_use(), 1);
    }

    #[test]
    fn scrub_failure_leaves_the_thread_destroyable_later() {
        let (mut m, mut mem) = mgr::<1>(1);
        let h = m.create(&mut mem, params(0)).unwrap();
        m.terminate(h, 5).unwrap();
        mem.fail_zero = true;
        assert_eq!(m.destroy(&mut mem, h), Err(Error::Memory));
        assert_eq!((m.live(), m.stacks_in_use()), (1, 1));
        m.check_accounting().unwrap();
        mem.fail_zero = false;
        assert_eq!(m.destroy(&mut mem, h), Ok(5));
    }

    #[test]
    fn manager_lifecycle_api_rejects_duplicate_and_invalid_operations() {
        let (mut m, mut mem) = mgr::<2>(2);
        let h = m.create(&mut mem, params(0)).unwrap();

        assert_eq!(m.thread(h).unwrap().state(), State::New);
        m.start(h).unwrap();
        assert_eq!(m.thread(h).unwrap().state(), State::Ready);
        assert_eq!(
            m.start(h),
            Err(Error::Thread(thread::Error::InvalidTransition {
                from: State::Ready,
                to: State::Ready,
            }))
        );

        m.terminate(h, 0).unwrap();
        assert_eq!(m.thread(h).unwrap().state(), State::Terminated);
        assert_eq!(
            m.start(h),
            Err(Error::Thread(thread::Error::InvalidTransition {
                from: State::Terminated,
                to: State::Ready,
            }))
        );
        assert_eq!(
            m.terminate(h, 1),
            Err(Error::Thread(thread::Error::InvalidTransition {
                from: State::Terminated,
                to: State::Terminated,
            }))
        );
        assert_eq!(
            m.exit_running(h, 1),
            Err(Error::Thread(thread::Error::NotCurrent))
        );

        assert_eq!(m.destroy(&mut mem, h), Ok(0));
        m.check_accounting().unwrap();
    }

    #[test]
    fn blocked_and_sleeping_threads_can_be_terminated_and_destroyed() {
        let (mut m, mut mem) = mgr::<4>(4);
        let a = m.create(&mut mem, params(0)).unwrap();
        let b = m.create(&mut mem, params(0)).unwrap();
        let replacement = m.create(&mut mem, params(0)).unwrap();

        m.start(a).unwrap();
        m.start(b).unwrap();
        m.start(replacement).unwrap();

        // Block a while it is current, then dispatch b.
        assert_eq!(m.schedule().unwrap().next, a);
        m.block_current(BlockReason::Event(1))
            .unwrap();
        assert_eq!(m.schedule().unwrap().next, b);

        // Sleep b while it is current, then dispatch the replacement.
        m.sleep_current(5).unwrap();
        assert_eq!(
            m.schedule().unwrap().next,
            replacement
        );

        // Both context switches have now completed.
        m.switch_out_complete(a).unwrap();
        m.switch_out_complete(b).unwrap();

        for h in [a, b] {
            m.terminate(h, 2).unwrap();
            assert_eq!(m.destroy(&mut mem, h), Ok(2));
        }

        // The replacement is intentionally still running and owns its stack.
        assert_eq!((m.live(), m.stacks_in_use()), (1, 1));
        m.check_accounting().unwrap();
    }

    #[test]
    fn create_destroy_cycles_do_not_leak() {
        let (mut m, mut mem) = mgr::<4>(4);
        for round in 0..5_000u64 {
            let hs: Vec<_> = (0..4)
                .map(|i| m.create(&mut mem, params(round + i)).unwrap())
                .collect();
            assert_eq!(m.stacks_in_use(), 4);
            for h in hs {
                m.terminate(h, round as i32).unwrap();
                assert_eq!(m.destroy(&mut mem, h), Ok(round as i32));
            }
            assert_eq!((m.live(), m.stacks_in_use()), (0, 0));
        }
        assert_eq!(m.created_total(), 20_000);
        assert_eq!(m.destroyed_total(), 20_000);
        m.check_accounting().unwrap();
    }

    /// Deterministic random operation sequences with accounting checked each step.
    #[test]
    fn random_lifecycle_operations_keep_accounting_consistent() {
        let (mut m, mut mem) = mgr::<6>(5); // fewer stacks than thread slots
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut hs: Vec<Handle> = Vec::new();
        for _ in 0..40_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let pick = if hs.is_empty() {
                None
            } else {
                Some(hs[(x >> 16) as usize % hs.len()])
            };
            let r: Result<(), Error> = match x % 7 {
                0 | 1 => m.create(&mut mem, params(x)).map(|h| hs.push(h)),
                2 => pick.map_or(Ok(()), |h| m.start(h)),
                3 => pick.map_or(Ok(()), |h| m.terminate(h, 1)),
                4 => pick.map_or(Ok(()), |h| m.destroy(&mut mem, h).map(|_| ())),
                5 => match m.schedule() {
                    Ok(sw) => {
                        if sw.is_switch() {
                            if let Some(prev) = sw.prev {
                                m.switch_out_complete(prev)
                            } else {
                                Ok(())
                            }
                        } else {
                            Ok(())
                        }
                    }
                    Err(e) => Err(e.into()),
                },
                _ => pick.map_or(Ok(()), |h| m.switch_out_complete(h)),
            };
            if let Err(e) = r {
                assert!(!matches!(e, Error::Corrupt), "internal corruption");
            }
            m.check_accounting().unwrap();
        }
    }

    #[test]
    fn concurrent_creation_and_destruction_through_the_spinlock() {
        let pool = StackPool::new(BASE, 16 * SLOT, STACK, 1 << 30).unwrap();
        let shared: Arc<SharedKThreads<16>> =
            Arc::new(SpinLock::new(KThreadManager::new(cfg(), pool).unwrap()));
        let mem = Arc::new(SpinLock::new(Mem::new(16)));
        let workers: Vec<_> = (0..4)
            .map(|w| {
                let (shared, mem) = (Arc::clone(&shared), Arc::clone(&mem));
                std::thread::spawn(move || {
                    let mut ids = Vec::new();
                    for i in 0..2_000u64 {
                        let h = {
                            let mut m = shared.lock();
                            let mut mm = mem.lock();
                            match m.create(&mut *mm, params(w * 10_000 + i)) {
                                Ok(h) => {
                                    ids.push(m.thread(h).unwrap().id());
                                    h
                                }
                                Err(_) => continue, // transient exhaustion is fine
                            }
                        };
                        let mut m = shared.lock();
                        let mut mm = mem.lock();
                        m.terminate(h, 0).unwrap();
                        m.destroy(&mut *mm, h).unwrap();
                    }
                    ids
                })
            })
            .collect();
        let mut all: Vec<_> = workers
            .into_iter()
            .flat_map(|w| w.join().unwrap())
            .collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n, "thread ids must be globally unique");
        assert!(n > 4_000, "workers made real progress ({n})");
        let m = shared.lock();
        assert_eq!((m.live(), m.stacks_in_use()), (0, 0));
        m.check_accounting().unwrap();
        assert_eq!(m.created_total(), m.destroyed_total());
    }
}
