//! Kernel thread stacks and initial contexts. SPDX-License-Identifier: Apache-2.0
//!
//! `KThreadStacks` owns ONLY: the stack pool, one stack per live thread, the
//! initial stack frame and the saved stack pointer. It owns no thread table, no
//! thread state and no `on_cpu` flag: the `Scheduler` is the single authority for
//! lifecycle (see `sched`). Metadata is keyed by the full generation-checked
//! `Handle`, so a stale handle can never reach a reused slot's stack.
//!
//! Transactional creation (reserve -> attach stack -> publish):
//! 1. `Scheduler::reserve` / `reserve_idle` reserves a `New` thread (invisible
//!    to dispatch);
//! 2. a stack is allocated and the initial frame is built and written;
//! 3. `Scheduler::publish` makes the thread `Ready`.
//! Any failure releases the stack and `discard_reserved`s the thread: no slot,
//! stack or scheduler entry leaks. (IDs are never reused, so a failed creation
//! burns one ID value.)
//!
//! Destruction order (`destroy`): `Scheduler::reap_ready` (Terminated, not
//! current, switch away completed) -> scrub the stack (may fail, nothing changed)
//! -> `Scheduler::reap` -> release the stack to the pool. A stack is therefore
//! never freed while a thread can still be executing on it.
//!
//! Synchronization: no interior mutability and no globals here. Callers hold the
//! scheduler lock (see `dispatch`) around every call.
use crate::context::{self, FRAME_BYTES};
use crate::kstack::{self, Stack, StackPool};
use crate::sched::Scheduler;
use crate::thread::{self, Handle, Priority};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Thread(thread::Error),
    Stack(kstack::Error),
    Context(context::Error),
    /// Entry point is null or outside the kernel text range.
    BadEntry,
    /// Configuration rejected (bad text range or trampoline).
    BadConfig,
    /// The memory backend failed to write or scrub.
    Memory,
    /// A switch was planned but its bookkeeping was never completed.
    SwitchInProgress,
    /// Internal inconsistency; treat as fatal.
    Corrupt,
    /// Per-thread preemption nesting counter overflowed or underflowed.
    PreemptCount,
    /// An operation requiring thread context was attempted in interrupt context.
    InterruptContext,
    /// Blocking was attempted while preemption was disabled.
    PreemptDisabled,
}

impl From<thread::Error> for Error {
    fn from(e: thread::Error) -> Self {
        Error::Thread(e)
    }
}
impl From<kstack::Error> for Error {
    fn from(e: kstack::Error) -> Self {
        Error::Stack(e)
    }
}
impl From<context::Error> for Error {
    fn from(e: context::Error) -> Self {
        Error::Context(e)
    }
}

/// Access to stack memory. The kernel supplies an implementation over the
/// identity-mapped window (it needs `unsafe`); host tests supply a fake or a
/// real host buffer.
///
/// Implementations MUST guarantee that `write` is atomic with respect to
/// failure: if it returns Err, the addressed bytes must remain unchanged.
/// Implementations unable to provide this guarantee must stage writes and
/// commit them only after all failure conditions have passed.
pub trait StackMemory {
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), MemError>;
    fn zero(&mut self, addr: u64, len: u64) -> Result<(), MemError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Kernel code range `[text_start, text_end)`; entry points must lie in it.
    pub text_start: u64,
    pub text_end: u64,
    /// Address of the assembly trampoline that starts every thread (must be in text).
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

impl Meta {
    const fn empty() -> Self {
        Meta {
            handle: None,
            stack: None,
            saved_sp: 0,
        }
    }
}

pub struct KThreadStacks<const N: usize> {
    cfg: Config,
    pool: StackPool,
    meta: [Meta; N],
    created: u64,
    destroyed: u64,
}

impl<const N: usize> KThreadStacks<N> {
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
            pool,
            meta: core::array::from_fn(|_| Meta::empty()),
            created: 0,
            destroyed: 0,
        })
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

    fn owned(&self, h: Handle) -> Result<&Meta, Error> {
        match self.meta.get(h.index()) {
            Some(m) if m.handle == Some(h) => Ok(m),
            _ => Err(thread::Error::Stale.into()),
        }
    }

    pub fn stack(&self, h: Handle) -> Result<&Stack, Error> {
        self.owned(h)?.stack.as_ref().ok_or(Error::Corrupt)
    }

    /// Stack pointer the switch routine must load to start (or resume) `h`.
    pub fn saved_sp(&self, h: Handle) -> Result<u64, Error> {
        Ok(self.owned(h)?.saved_sp)
    }

    /// Address where the switch routine must store `h`'s stack pointer when it
    /// is switched away from.
    ///
    /// The pointer stays valid as long as this `KThreadStacks` is not moved or
    /// dropped and `h` is live. Using it is the caller's responsibility (see
    /// `dispatch`): it must be written by exactly one switch, with the manager
    /// not borrowed at that moment.
    pub fn saved_sp_slot(&mut self, h: Handle) -> Result<*mut u64, Error> {
        self.owned(h)?;
        Ok(&mut self.meta[h.index()].saved_sp as *mut u64)
    }

    fn check_entry(&self, entry: u64) -> Result<(), Error> {
        if (self.cfg.text_start..self.cfg.text_end).contains(&entry) {
            Ok(())
        } else {
            Err(Error::BadEntry)
        }
    }

    /// Creates a Ready thread (priority 1..=7) with a fresh stack and frame.
    pub fn create<M: StackMemory>(
        &mut self,
        sched: &mut Scheduler<N>,
        mem: &mut M,
        p: ThreadParams,
    ) -> Result<Handle, Error> {
        self.check_entry(p.entry)?;
        self.create_with(sched, mem, p.entry, p.arg, |s| {
            s.reserve(p.priority, p.entry as usize, p.arg as usize)
        })
    }

    /// Creates the idle thread (priority 0). Only one may exist.
    pub fn create_idle<M: StackMemory>(
        &mut self,
        sched: &mut Scheduler<N>,
        mem: &mut M,
        entry: u64,
        arg: u64,
    ) -> Result<Handle, Error> {
        self.check_entry(entry)?;
        self.create_with(sched, mem, entry, arg, |s| {
            s.reserve_idle(entry as usize, arg as usize)
        })
    }

    fn create_with<M: StackMemory>(
        &mut self,
        sched: &mut Scheduler<N>,
        mem: &mut M,
        entry: u64,
        arg: u64,
        reserve: impl FnOnce(&mut Scheduler<N>) -> Result<Handle, thread::Error>,
    ) -> Result<Handle, Error> {
        let next_created = self.created.checked_add(1).ok_or(Error::Corrupt)?;
        let h = reserve(sched)?;
        let idx = h.index();
        if self.meta[idx].handle.is_some() || self.meta[idx].stack.is_some() {
            // A freed thread slot still has stack metadata: accounting is broken.
            if sched.discard_reserved(h).is_err() {
                return Err(Error::Corrupt);
            }
            return Err(Error::Corrupt);
        }
        let stack = match self.pool.alloc() {
            Ok(s) => s,
            Err(e) => return self.abort(sched, h, None, 0, e.into()),
        };
        let ctx = match context::build_initial(
            stack.base(),
            stack.top(),
            entry,
            arg,
            self.cfg.trampoline,
        ) {
            Ok(c) => c,
            Err(e) => {
                let saved_sp = stack.top().saturating_sub(FRAME_BYTES);
                return self.abort(sched, h, Some(stack), saved_sp, e.into());
            }
        };
        debug_assert!(ctx.saved_sp() + FRAME_BYTES <= stack.top());
        if mem.write(ctx.saved_sp(), &ctx.to_bytes()).is_err() {
            return self.abort(sched, h, Some(stack), ctx.saved_sp(), Error::Memory);
        }
        self.meta[idx] = Meta {
            handle: Some(h),
            stack: Some(stack),
            saved_sp: ctx.saved_sp(),
        };
        if let Err(e) = sched.publish(h) {
            let stack = self.meta[idx].stack.take();
            let saved_sp = self.meta[idx].saved_sp;
            self.meta[idx] = Meta::empty();
            return self.abort(sched, h, stack, saved_sp, e.into());
        }
        self.created = next_created;
        Ok(h)
    }

    fn abort(
        &mut self,
        sched: &mut Scheduler<N>,
        h: Handle,
        stack: Option<Stack>,
        saved_sp: u64,
        cause: Error,
    ) -> Result<Handle, Error> {
        let Some(stack) = stack else {
            if sched.discard_reserved(h).is_err() {
                return Err(Error::Corrupt);
            }
            return Err(cause);
        };

        let idx = h.index();
        if self.meta[idx].handle.is_some() || self.meta[idx].stack.is_some() {
            return Err(Error::Corrupt);
        }

        // Retain ownership metadata until both rollback operations succeed.
        self.meta[idx] = Meta {
            handle: Some(h),
            stack: Some(stack),
            saved_sp,
        };

        if sched.discard_reserved(h).is_err() {
            return Err(Error::Corrupt);
        }

        let release_result = {
            let stack = self.meta[idx].stack.as_ref().ok_or(Error::Corrupt)?;
            self.pool.release_ref(stack)
        };

        if release_result.is_err() {
            return Err(Error::Corrupt);
        }

        self.meta[idx] = Meta::empty();
        Err(cause)
    }

    /// Reclaims a terminated thread: scrubs its stack, frees the scheduler slot
    /// and returns the stack to the pool; returns the exit status.
    ///
    /// Refused (nothing changed) unless the thread is `Terminated`, is not the
    /// current thread and has finished switching away. If scrubbing fails the
    /// thread stays terminated and destroyable.
    pub fn destroy<M: StackMemory>(
        &mut self,
        sched: &mut Scheduler<N>,
        mem: &mut M,
        h: Handle,
    ) -> Result<i32, Error> {
        let next_destroyed = self.destroyed.checked_add(1).ok_or(Error::Corrupt)?;
        sched.reap_ready(h)?;

        let (base, size) = {
            let s = self.stack(h)?;
            self.pool.validate_release(s).map_err(|_| Error::Corrupt)?;
            (s.base(), s.size())
        };

        mem.zero(base, size).map_err(|_| Error::Memory)?;
        let status = sched.reap(h)?;
        let idx = h.index();

        {
            let stack = self.meta[idx].stack.as_ref().ok_or(Error::Corrupt)?;
            self.pool.release_ref(stack).map_err(|_| Error::Corrupt)?;
        }

        self.meta[idx] = Meta::empty();
        self.destroyed = next_destroyed;
        Ok(status)
    }

    /// Accounting self-check: every live scheduler thread owns exactly one
    /// stack (matched by full handle), no stack belongs to a dead thread, and the
    /// pool's in-use count agrees.
    pub fn check_accounting(&self, sched: &Scheduler<N>) -> Result<(), &'static str> {
        let mut owned = 0usize;
        for i in 0..N {
            let live = sched.handle_at(i);
            let m = &self.meta[i];
            match (live, m.handle) {
                (None, None) => {
                    if m.stack.is_some() {
                        return Err("stack attached to a free thread slot");
                    }
                }
                (None, Some(_)) => return Err("stack metadata for a dead thread"),
                (Some(_), None) => return Err("live thread without stack metadata"),
                (Some(a), Some(b)) => {
                    if a != b {
                        return Err("stack metadata belongs to a previous generation");
                    }
                    let Some(s) = m.stack.as_ref() else {
                        return Err("live thread without a stack");
                    };
                    owned = owned.checked_add(1).ok_or("owned count overflow")?;
                    if m.saved_sp < s.base() || m.saved_sp >= s.top() {
                        return Err("saved sp outside owned stack");
                    }
                    if self.pool.classify(s.base()) != kstack::Region::Stack {
                        return Err("stack outside pool");
                    }
                }
            }
        }
        if owned != self.pool.in_use() {
            return Err("pool in-use count differs from owned stacks");
        }
        if owned != sched.live_threads() {
            return Err("scheduler thread count differs from owned stacks");
        }
        let owned_u64 = u64::try_from(owned).map_err(|_| "owned count overflow")?;
        if self.destroyed.checked_add(owned_u64) != Some(self.created) {
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
    use crate::spin::SpinLock;
    use crate::thread::State;
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

    struct Sys<const N: usize> {
        sched: Scheduler<N>,
        stacks: KThreadStacks<N>,
        mem: Mem,
    }

    impl<const N: usize> Sys<N> {
        fn new(slots: u64) -> Self {
            let pool = StackPool::new(BASE, slots * SLOT, STACK, 1 << 30).unwrap();
            Self {
                sched: Scheduler::new(),
                stacks: KThreadStacks::new(cfg(), pool).unwrap(),
                mem: Mem::new(slots),
            }
        }
        fn create(&mut self, p: ThreadParams) -> Result<Handle, Error> {
            self.stacks.create(&mut self.sched, &mut self.mem, p)
        }
        fn destroy(&mut self, h: Handle) -> Result<i32, Error> {
            self.stacks.destroy(&mut self.sched, &mut self.mem, h)
        }
        fn residue(&self) -> (usize, usize, u64) {
            (
                self.sched.live_threads(),
                self.stacks.stacks_in_use(),
                self.stacks.created_total(),
            )
        }
        fn ok(&self) {
            self.stacks.check_accounting(&self.sched).unwrap();
            self.sched.check_invariants().unwrap();
        }
    }

    fn params(arg: u64) -> ThreadParams {
        ThreadParams {
            entry: ENTRY,
            arg,
            priority: Priority::DEFAULT,
        }
    }

    #[test]
    fn creation_yields_ready_thread_with_stack_and_valid_initial_frame() {
        let mut s = Sys::<4>::new(4);
        let h = s.create(params(0x1234)).unwrap();
        let t = s.sched.thread(h).unwrap();
        assert_eq!(t.state(), State::Ready, "published at the end of creation");
        assert_eq!(t.priority(), Priority::DEFAULT);
        assert_eq!(t.exit_status(), None);
        assert!(!t.on_cpu());
        assert_eq!(t.entry(), (ENTRY as usize, 0x1234));

        let st = s.stacks.stack(h).unwrap();
        assert_eq!(st.top() % 16, 0);
        assert_eq!(st.base() % 4096, 0);
        assert_eq!(st.size(), STACK);

        let sp = s.stacks.saved_sp(h).unwrap();
        assert_eq!(sp, st.top() - FRAME_BYTES);
        let frame = s.mem.read(sp, FRAME_BYTES);
        let word = |i: usize| u64::from_le_bytes(frame[i * 8..i * 8 + 8].try_into().unwrap());
        assert_eq!(word(WORD_ENTRY_R12), ENTRY);
        assert_eq!(word(WORD_ARG_R13), 0x1234);
        assert_eq!(word(WORD_RET_TRAMPOLINE), TRAMP);
        s.ok();
    }

    #[test]
    fn thread_ids_are_unique_across_create_and_destroy() {
        let mut s = Sys::<2>::new(2);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let h = s.create(params(0)).unwrap();
            assert!(seen.insert(s.sched.thread(h).unwrap().id()), "id reused");
            s.sched.kill(h, 0).unwrap();
            s.destroy(h).unwrap();
        }
    }

    #[test]
    fn each_thread_gets_its_own_non_overlapping_stack() {
        let mut s = Sys::<4>::new(4);
        let hs: Vec<_> = (0..4).map(|i| s.create(params(i)).unwrap()).collect();
        let mut ranges: Vec<_> = hs
            .iter()
            .map(|&h| {
                let st = s.stacks.stack(h).unwrap();
                (st.guard_base(), st.top())
            })
            .collect();
        ranges.sort();
        for w in ranges.windows(2) {
            assert!(w[0].1 <= w[1].0);
        }
        for (i, &h) in hs.iter().enumerate() {
            let f = s.mem.read(s.stacks.saved_sp(h).unwrap(), FRAME_BYTES);
            let arg = u64::from_le_bytes(f[16..24].try_into().unwrap());
            assert_eq!(arg, i as u64);
        }
    }

    #[test]
    fn invalid_entry_points_are_rejected_without_side_effects() {
        let mut s = Sys::<2>::new(2);
        for bad in [0, TEXT.0 - 1, TEXT.1, u64::MAX, 0xffff_8000_0000_0000] {
            let r = s.create(ThreadParams {
                entry: bad,
                ..params(0)
            });
            assert_eq!(r, Err(Error::BadEntry), "entry {bad:#x}");
        }
        assert_eq!(s.residue(), (0, 0, 0));
    }

    #[test]
    fn idle_priority_only_via_create_idle_and_only_once() {
        let mut s = Sys::<3>::new(3);
        let r = s.create(ThreadParams {
            priority: Priority::IDLE,
            ..params(0)
        });
        assert_eq!(r, Err(Error::Thread(thread::Error::BadPriority)));
        assert_eq!(s.residue(), (0, 0, 0));
        let h = s
            .stacks
            .create_idle(&mut s.sched, &mut s.mem, ENTRY, 0)
            .unwrap();
        assert!(s.sched.thread(h).unwrap().is_idle());
        assert_eq!(s.sched.idle(), Some(h));
        let again = s.stacks.create_idle(&mut s.sched, &mut s.mem, ENTRY, 0);
        assert_eq!(again, Err(Error::Thread(thread::Error::IdleExists)));
        assert_eq!(s.residue(), (1, 1, 1), "second idle left no residue");
        let bad = s.stacks.create_idle(&mut s.sched, &mut s.mem, 0, 0);
        assert_eq!(bad, Err(Error::BadEntry));
        s.ok();
    }

    #[test]
    fn failed_idle_creation_clears_the_idle_reservation() {
        let mut s = Sys::<2>::new(2);
        s.mem.fail_write = true;
        let r = s.stacks.create_idle(&mut s.sched, &mut s.mem, ENTRY, 0);
        assert_eq!(r, Err(Error::Memory));
        assert_eq!(s.sched.idle(), None);
        s.mem.fail_write = false;
        s.stacks
            .create_idle(&mut s.sched, &mut s.mem, ENTRY, 0)
            .unwrap();
        s.ok();
    }

    #[test]
    fn bad_configuration_is_rejected() {
        let pool = || StackPool::new(BASE, SLOT, STACK, 1 << 30).unwrap();
        let mk = |c: Config| KThreadStacks::<1>::new(c, pool()).err();
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
        let mut s = Sys::<8>::new(2);
        let a = s.create(params(1)).unwrap();
        let _b = s.create(params(2)).unwrap();
        let before = s.residue();
        assert_eq!(
            s.create(params(3)),
            Err(Error::Stack(kstack::Error::Exhausted))
        );
        assert_eq!(s.residue(), before, "no residue in scheduler or pool");
        s.ok();
        s.sched.kill(a, 0).unwrap();
        s.destroy(a).unwrap();
        s.create(params(4)).unwrap();
        s.ok();
    }

    #[test]
    fn thread_table_exhaustion_fails_before_taking_a_stack() {
        let mut s = Sys::<2>::new(8);
        s.create(params(0)).unwrap();
        s.create(params(0)).unwrap();
        assert_eq!(
            s.create(params(0)),
            Err(Error::Thread(thread::Error::NoSlots))
        );
        assert_eq!(s.stacks.stacks_in_use(), 2);
        s.ok();
    }

    #[test]
    fn memory_write_failure_rolls_back_thread_and_stack() {
        let mut s = Sys::<2>::new(2);
        s.mem.fail_write = true;
        assert_eq!(s.create(params(0)), Err(Error::Memory));
        assert_eq!(s.residue(), (0, 0, 0));
        s.mem.fail_write = false;
        s.create(params(0)).unwrap();
        s.ok();
    }

    #[test]
    fn destroy_reclaims_scrubs_and_allows_reuse() {
        let mut s = Sys::<1>::new(1);
        let h = s.create(params(9)).unwrap();
        let (base, size, sp) = {
            let st = s.stacks.stack(h).unwrap();
            (st.base(), st.size(), s.stacks.saved_sp(h).unwrap())
        };
        assert!(s.mem.read(sp, FRAME_BYTES).iter().any(|&b| b != 0));
        s.sched.kill(h, -3).unwrap();
        assert_eq!(s.stacks.stacks_in_use(), 1, "stack kept until destroy");
        assert_eq!(s.destroy(h), Ok(-3));
        assert_eq!((s.sched.live_threads(), s.stacks.stacks_in_use()), (0, 0));
        assert!(s.mem.read(base, size).iter().all(|&b| b == 0), "scrubbed");
        assert_eq!(
            (s.stacks.created_total(), s.stacks.destroyed_total()),
            (1, 1)
        );
        let h2 = s.create(params(1)).unwrap();
        assert_eq!(s.stacks.stack(h2).unwrap().base(), base);
        s.ok();
    }

    #[test]
    fn double_destruction_and_stale_handles_are_rejected() {
        let mut s = Sys::<2>::new(2);
        let h = s.create(params(0)).unwrap();
        s.sched.kill(h, 0).unwrap();
        s.destroy(h).unwrap();
        let stale = Err(Error::Thread(thread::Error::Stale));
        assert_eq!(s.destroy(h), stale);
        assert_eq!(
            s.stacks.stack(h).err(),
            Some(Error::Thread(thread::Error::Stale))
        );
        assert_eq!(
            s.stacks.saved_sp(h),
            Err(Error::Thread(thread::Error::Stale))
        );
        // Same slot reused by a new thread: the old handle must not see its stack.
        let h2 = s.create(params(0)).unwrap();
        assert_eq!(h.index(), h2.index());
        assert_eq!(s.destroy(h), stale);
        assert_eq!(
            s.stacks.stack(h).err(),
            Some(Error::Thread(thread::Error::Stale))
        );
        assert!(s.stacks.stack(h2).is_ok());
        s.sched.kill(h2, 0).unwrap();
        assert_eq!(
            s.destroy(h),
            stale,
            "stale handle cannot reclaim h2's stack"
        );
        assert_eq!(s.stacks.stacks_in_use(), 1);
        s.ok();
    }

    #[test]
    fn destroy_requires_termination() {
        let mut s = Sys::<2>::new(2);
        let h = s.create(params(0)).unwrap();
        assert_eq!(
            s.destroy(h),
            Err(Error::Thread(thread::Error::NotTerminated))
        );
        assert_eq!(s.stacks.stacks_in_use(), 1);
        s.ok();
    }

    #[test]
    fn stack_is_never_reclaimed_while_the_thread_is_still_on_it() {
        let mut s = Sys::<4>::new(4);
        let a = s.create(params(0)).unwrap();
        let b = s.create(params(0)).unwrap();
        let sw = s.sched.schedule().unwrap();
        assert_eq!(sw.next, a);
        // Running: not reclaimable.
        assert_eq!(s.destroy(a), Err(Error::Thread(thread::Error::StillOnCpu)));
        s.sched.terminate_current(7).unwrap();
        // Terminated but still executing on its own stack.
        let on_cpu = Err(Error::Thread(thread::Error::StillOnCpu));
        assert_eq!(s.destroy(a), on_cpu);
        let sw = s.sched.schedule().unwrap();
        assert_eq!((sw.prev, sw.next), (Some(a), b));
        // Scheduler picked the next thread, but the switch has not completed.
        assert_eq!(s.destroy(a), on_cpu);
        assert_eq!(s.stacks.stacks_in_use(), 2);
        s.sched.switch_complete(a).unwrap();
        assert_eq!(s.destroy(a), Ok(7));
        assert_eq!(s.stacks.stacks_in_use(), 1);
        s.ok();
    }

    #[test]
    fn scrub_failure_leaves_the_thread_destroyable_later() {
        let mut s = Sys::<1>::new(1);
        let h = s.create(params(0)).unwrap();
        s.sched.kill(h, 5).unwrap();
        s.mem.fail_zero = true;
        assert_eq!(s.destroy(h), Err(Error::Memory));
        assert_eq!((s.sched.live_threads(), s.stacks.stacks_in_use()), (1, 1));
        s.ok();
        s.mem.fail_zero = false;
        assert_eq!(s.destroy(h), Ok(5));
        s.ok();
    }

    #[test]
    fn blocked_and_sleeping_threads_can_be_killed_and_destroyed() {
        let mut s = Sys::<4>::new(4);
        s.stacks
            .create_idle(&mut s.sched, &mut s.mem, ENTRY, 0)
            .unwrap();
        let a = s.create(params(0)).unwrap();
        let b = s.create(params(0)).unwrap();
        for st in [0, 1] {
            let sw = s.sched.schedule().unwrap();
            if let Some(p) = sw.prev {
                s.sched.switch_complete(p).ok();
            }
            if st == 0 {
                s.sched
                    .block_current(crate::thread::BlockReason::Event(1))
                    .unwrap();
            } else {
                s.sched.sleep_current(50).unwrap();
            }
        }
        let sw = s.sched.schedule().unwrap();
        assert!(sw.is_switch());
        s.sched.switch_complete(sw.prev.unwrap()).unwrap();
        for h in [a, b] {
            let st = s.sched.thread(h).unwrap().state();
            assert!(matches!(st, State::Blocked | State::Sleeping), "{st:?}");
            s.sched.kill(h, 2).unwrap();
            assert_eq!(s.destroy(h), Ok(2));
        }
        s.ok();
    }

    #[test]
    fn create_destroy_cycles_do_not_leak() {
        let mut s = Sys::<4>::new(4);
        for round in 0..5_000u64 {
            let hs: Vec<_> = (0..4)
                .map(|i| s.create(params(round + i)).unwrap())
                .collect();
            assert_eq!(s.stacks.stacks_in_use(), 4);
            for h in hs {
                s.sched.kill(h, round as i32).unwrap();
                assert_eq!(s.destroy(h), Ok(round as i32));
            }
            assert_eq!((s.sched.live_threads(), s.stacks.stacks_in_use()), (0, 0));
        }
        assert_eq!(s.stacks.created_total(), 20_000);
        assert_eq!(s.stacks.destroyed_total(), 20_000);
        s.ok();
    }

    /// Deterministic random operation sequences; scheduler and stack accounting
    /// are cross-checked after every step.
    #[test]
    fn random_lifecycle_operations_keep_accounting_consistent() {
        let mut s = Sys::<6>::new(5); // fewer stacks than thread slots
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
            let r: Result<(), Error> = match x % 8 {
                0 | 1 => s.create(params(x)).map(|h| hs.push(h)),
                2 => pick.map_or(Ok(()), |h| s.sched.kill(h, 1).map_err(Error::from)),
                3 => pick.map_or(Ok(()), |h| s.destroy(h).map(|_| ())),
                4 | 5 => match s.sched.schedule() {
                    Ok(sw) => {
                        if sw.is_switch()
                            && let Some(p) = sw.prev
                        {
                            s.sched.switch_complete(p).unwrap();
                        }
                        Ok(())
                    }
                    // No idle thread here, so running dry is a legal outcome.
                    Err(e) => Err(e.into()),
                },
                6 => s.sched.terminate_current(3).map_err(Error::from),
                _ => {
                    s.sched.tick();
                    Ok(())
                }
            };
            if let Err(e) = r {
                assert!(!matches!(e, Error::Corrupt), "internal corruption");
            }
            // The scheduler needs an idle thread to never run dry; allow NoRunnable.
            s.stacks.check_accounting(&s.sched).unwrap();
        }
    }

    #[test]
    fn concurrent_creation_and_destruction_through_the_spinlock() {
        let shared = Arc::new(SpinLock::new(Sys::<16>::new(16)));
        let workers: Vec<_> = (0..4)
            .map(|w| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || {
                    let mut ids = Vec::new();
                    for i in 0..2_000u64 {
                        let mut g = shared.lock();
                        let Ok(h) = g.create(params(w * 10_000 + i)) else {
                            continue; // transient exhaustion is fine
                        };
                        ids.push(g.sched.thread(h).unwrap().id());
                        g.sched.kill(h, 0).unwrap();
                        g.destroy(h).unwrap();
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
        let g = shared.lock();
        assert_eq!(g.residue(), (0, 0, g.stacks.created_total()));
        assert_eq!(g.stacks.created_total(), g.stacks.destroyed_total());
        g.ok();
    }
}
