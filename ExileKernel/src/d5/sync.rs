//! Kernel synchronisation (Mission D, requirements 10 and 11). SPDX-License-Identifier: Apache-2.0
//!
//! Sleeping mutexes, counting semaphores, wait queues and condition variables, built on the
//! existing scheduler (`Scheduler::block_current` / `wake`) and thread lifecycle. No allocation.
//!
//! # Model
//! A [`SyncDomain<N, S>`] owns up to `S` objects (one table, generation-checked [`ObjId`]s) and
//! one wait record per thread slot (`N`): a thread waits on at most one object, so waiter
//! storage cannot overflow. Operations take the `&mut Scheduler<N>` explicitly; the blocking
//! wrappers at the bottom (x86-64 only) add the kernel-slot locking, interrupt masking and the
//! real context switch.
//!
//! # Rules enforced
//! * **Ownership**: a mutex has exactly one owning thread; only the owner may unlock; relocking
//!   by the owner is `Recursive` (never a silent self-deadlock).
//! * **Lock order**: every mutex has a `rank` (0 = exempt). A blocking `lock` of rank `r` fails
//!   with `LockOrder` if the thread already holds a ranked mutex of rank `>= r`. Ranks must
//!   therefore be acquired strictly increasing; same-rank pairs are a violation too.
//! * **Deadlock**: before blocking on mutex `m` owned by `o`, the wait-for chain
//!   `o -> (mutex o waits on) -> its owner -> ...` is followed (bounded by `N`); if it reaches the
//!   caller the lock fails with `Deadlock` and nothing changes. Semaphores and wait queues have
//!   no owner and are not part of the graph (a deadlock through them is not detected).
//! * **Priority inheritance**: a mutex owner runs at least at the highest effective level of any
//!   thread waiting on any mutex it owns, transitively along wait-for chains. The level is
//!   recomputed from scratch on every change (acquire, release, waiter removal, exit), so it can
//!   never stick after the lock is dropped. `check_invariants` verifies it for every thread.
//! * **Hand-off**: unlock/post gives the object directly to the best waiter (highest effective
//!   level, FIFO within a level), so a woken waiter cannot be overtaken (no lock stealing).
//! * **Exit**: `thread_exit` removes the thread from any wait and releases every mutex it owns
//!   as *abandoned* (the next acquirer is told), so a dying thread cannot wedge others.
//! * **Contexts**: the model has no notion of interrupt context; the wrappers refuse blocking
//!   and lock/unlock of mutexes from interrupt context and with preemption disabled.
//!   `sem_post`, `cond_signal` and `wq_wake_*` do not block and are legal from interrupt context
//!   (they only need the scheduler lock, which masks interrupts).
//!
//! # Not implemented
//! No timeouts (the scheduler has no timed event wait; use `try_*`), no cross-CPU support, no
//! read/write locks, and deadlock detection is limited to mutex cycles.
#![allow(dead_code)]

use crate::sched::{BlockOutcome, Scheduler};
use crate::thread::{self, BlockReason, Handle, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Object id refers to a destroyed object.
    Stale,
    WrongKind,
    NoObjects,
    /// Destroy refused: owned, waited on, or semaphore/queue not idle.
    Busy,
    NotOwner,
    /// The owner tried to lock its own mutex.
    Recursive,
    LockOrder {
        held: u8,
        requested: u8,
    },
    Deadlock,
    /// A `try_*` operation could not proceed.
    WouldBlock,
    Overflow,
    BadArg,
    IsIdle,
    NoCurrent,
    /// The thread was woken without being granted the object.
    Interrupted,
    Sched(thread::Error),
    Corrupt(&'static str),
}

impl From<thread::Error> for Error {
    fn from(e: thread::Error) -> Self {
        Error::Sched(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjId {
    index: u16,
    gen_: u32,
}

impl ObjId {
    pub const fn index(self) -> usize {
        self.index as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockOutcome {
    /// The caller owns the mutex now. `abandoned` = a previous owner died holding it.
    Acquired { abandoned: bool },
    /// The caller is Blocked on the mutex; run the scheduler, then call `resume`.
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Acquired,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    Granted {
        abandoned: bool,
    },
    /// Woken without a grant (external wake-up): the wait record was removed.
    Interrupted,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub lock_order_violations: u64,
    pub deadlocks_prevented: u64,
    pub inheritance_raises: u64,
    pub handoffs: u64,
    pub abandoned: u64,
}

#[derive(Clone, Copy)]
enum Obj {
    Mutex {
        owner: Option<Handle>,
        rank: u8,
        abandoned: bool,
    },
    Sem {
        count: u32,
        max: u32,
    },
    Queue,
}

#[derive(Clone, Copy)]
struct Slot {
    gen_: u32,
    retired: bool,
    obj: Option<Obj>,
}

#[derive(Clone, Copy)]
struct Wait {
    obj: ObjId,
    seq: u64,
    handle: Handle,
}

pub struct SyncDomain<const N: usize, const S: usize> {
    slots: [Slot; S],
    waits: [Option<Wait>; N],
    /// Set when a mutex was handed to this thread while Blocked; consumed by `resume`.
    grant_abandoned: [bool; N],
    seq: u64,
    stats: Stats,
}

impl<const N: usize, const S: usize> Default for SyncDomain<N, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, const S: usize> SyncDomain<N, S> {
    pub const fn new() -> Self {
        Self {
            slots: [Slot {
                gen_: 0,
                retired: false,
                obj: None,
            }; S],
            waits: [None; N],
            grant_abandoned: [false; N],
            seq: 0,
            stats: Stats {
                lock_order_violations: 0,
                deadlocks_prevented: 0,
                inheritance_raises: 0,
                handoffs: 0,
                abandoned: 0,
            },
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn live_objects(&self) -> usize {
        self.slots.iter().filter(|s| s.obj.is_some()).count()
    }

    pub fn waiting_threads(&self) -> usize {
        self.waits.iter().flatten().count()
    }

    // ---- object table -------------------------------------------------------------------

    fn create(&mut self, obj: Obj) -> Result<ObjId, Error> {
        let i = self
            .slots
            .iter()
            .position(|s| s.obj.is_none() && !s.retired)
            .ok_or(Error::NoObjects)?;
        let s = &mut self.slots[i];
        s.obj = Some(obj);
        Ok(ObjId {
            index: i as u16,
            gen_: s.gen_,
        })
    }

    fn obj(&self, id: ObjId) -> Result<Obj, Error> {
        let s = self.slots.get(id.index()).ok_or(Error::Stale)?;
        match s.obj {
            Some(o) if s.gen_ == id.gen_ => Ok(o),
            _ => Err(Error::Stale),
        }
    }

    fn obj_mut(&mut self, id: ObjId) -> Result<&mut Obj, Error> {
        let s = self.slots.get_mut(id.index()).ok_or(Error::Stale)?;
        if s.gen_ != id.gen_ {
            return Err(Error::Stale);
        }
        s.obj.as_mut().ok_or(Error::Stale)
    }

    pub fn create_mutex(&mut self, rank: u8) -> Result<ObjId, Error> {
        self.create(Obj::Mutex {
            owner: None,
            rank,
            abandoned: false,
        })
    }

    pub fn create_semaphore(&mut self, initial: u32, max: u32) -> Result<ObjId, Error> {
        if max == 0 || initial > max {
            return Err(Error::BadArg);
        }
        self.create(Obj::Sem {
            count: initial,
            max,
        })
    }

    /// A wait queue, usable as a condition variable (`cond_*`).
    pub fn create_queue(&mut self) -> Result<ObjId, Error> {
        self.create(Obj::Queue)
    }

    fn has_waiters(&self, id: ObjId) -> bool {
        self.waits.iter().flatten().any(|w| w.obj == id)
    }

    /// Destroys an idle object. Refused while owned, waited on, or (semaphore) below its
    /// initial state is not tracked: only waiters and ownership block destruction.
    pub fn destroy(&mut self, id: ObjId) -> Result<(), Error> {
        let o = self.obj(id)?;
        if self.has_waiters(id) {
            return Err(Error::Busy);
        }
        if let Obj::Mutex { owner: Some(_), .. } = o {
            return Err(Error::Busy);
        }
        let s = &mut self.slots[id.index()];
        s.obj = None;
        match s.gen_.checked_add(1) {
            Some(g) => s.gen_ = g,
            None => s.retired = true,
        }
        Ok(())
    }

    // ---- helpers ------------------------------------------------------------------------

    fn current(s: &Scheduler<N>) -> Result<Handle, Error> {
        let cur = s.current().ok_or(Error::NoCurrent)?;
        if s.thread(cur)?.is_idle() {
            return Err(Error::IsIdle);
        }
        Ok(cur)
    }

    fn owned_by(&self, h: Handle) -> impl Iterator<Item = (usize, u8)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(move |(i, s)| match s.obj {
                Some(Obj::Mutex {
                    owner: Some(o),
                    rank,
                    ..
                }) if o == h => Some((i, rank)),
                _ => None,
            })
    }

    fn held_max_rank(&self, h: Handle) -> u8 {
        self.owned_by(h).map(|(_, r)| r).max().unwrap_or(0)
    }

    fn is_valid_waiter(&self, s: &Scheduler<N>, w: &Wait) -> bool {
        s.thread(w.handle).is_ok_and(|t| {
            t.state() == State::Blocked
                && t.block_reason() == BlockReason::Event(w.obj.index() as u32)
        })
    }

    /// Best waiter on `id`: highest effective level, FIFO within a level. Stale records (dead or
    /// no longer blocked on this object) are dropped on the way.
    fn pick_waiter(&mut self, s: &Scheduler<N>, id: ObjId) -> Option<Handle> {
        let mut best: Option<(usize, usize, u64)> = None; // (slot, level, seq)
        for i in 0..N {
            let Some(w) = self.waits[i] else { continue };
            if w.obj != id {
                continue;
            }
            if !self.is_valid_waiter(s, &w) {
                self.waits[i] = None;
                continue;
            }
            let lvl = s.thread(w.handle).map(|t| t.effective_level()).unwrap_or(0);
            let better = match best {
                None => true,
                Some((_, bl, bs)) => lvl > bl || (lvl == bl && w.seq < bs),
            };
            if better {
                best = Some((i, lvl, w.seq));
            }
        }
        best.and_then(|(i, _, _)| self.waits[i].map(|w| w.handle))
    }

    /// Level `h` must inherit: max effective level over all waiters on mutexes `h` owns.
    fn expected_inherited(&self, s: &Scheduler<N>, h: Handle) -> u8 {
        let mut level = 0u8;
        for (i, _) in self.owned_by(h) {
            for w in self.waits.iter().flatten() {
                if w.obj.index() == i && self.is_valid_waiter(s, w) {
                    if let Ok(t) = s.thread(w.handle) {
                        level = level.max(
                            u8::try_from(t.effective_level())
                                .expect("effective priority level must fit in u8"),
                        );
                    }
                }
            }
        }
        level
    }

    fn recompute(&mut self, s: &mut Scheduler<N>, h: Handle) -> Result<(), Error> {
        let want = self.expected_inherited(s, h);
        let t = s.thread(h)?;
        if t.is_idle() {
            return Ok(());
        }
        // Raising counts as an inheritance event; only record it once.
        if want > t.inherited_level() {
            self.stats.inheritance_raises += 1;
        }
        s.set_inherited(h, want)?;
        Ok(())
    }

    /// Recomputes `start`, then follows `start -> mutex it waits on -> owner` upward.
    fn propagate(&mut self, s: &mut Scheduler<N>, start: Handle) -> Result<(), Error> {
        let mut h = start;
        for _ in 0..=N {
            self.recompute(s, h)?;
            let Some(w) = self.waits.get(h.index()).copied().flatten() else {
                return Ok(());
            };
            match self.obj(w.obj) {
                Ok(Obj::Mutex { owner: Some(o), .. }) => h = o,
                _ => return Ok(()),
            }
        }
        Ok(())
    }

    fn enqueue(&mut self, cur: Handle, id: ObjId) {
        self.seq += 1;
        self.waits[cur.index()] = Some(Wait {
            obj: id,
            seq: self.seq,
            handle: cur,
        });
        self.grant_abandoned[cur.index()] = false;
    }

    /// Blocks the current thread on `id` in the scheduler. A stale pending wake-up is consumed
    /// first so the thread really blocks.
    fn block_on(&mut self, s: &mut Scheduler<N>, id: ObjId) -> Result<(), Error> {
        for _ in 0..2 {
            if s.block_current(BlockReason::Event(id.index() as u32))? == BlockOutcome::Blocked {
                return Ok(());
            }
        }
        Err(Error::Corrupt("thread would not block"))
    }

    // ---- mutex ----------------------------------------------------------------------------

    fn check_order(&mut self, cur: Handle, rank: u8) -> Result<(), Error> {
        let held = self.held_max_rank(cur);
        if rank != 0 && held != 0 && held >= rank {
            self.stats.lock_order_violations += 1;
            return Err(Error::LockOrder {
                held,
                requested: rank,
            });
        }
        Ok(())
    }

    fn take_free(&mut self, id: ObjId, cur: Handle) -> Result<bool, Error> {
        match self.obj_mut(id)? {
            Obj::Mutex {
                owner, abandoned, ..
            } => {
                debug_assert!(owner.is_none());
                *owner = Some(cur);
                Ok(core::mem::take(abandoned))
            }
            _ => Err(Error::WrongKind),
        }
    }

    pub fn mutex_try_lock(
        &mut self,
        s: &mut Scheduler<N>,
        id: ObjId,
    ) -> Result<LockOutcome, Error> {
        let cur = Self::current(s)?;
        let Obj::Mutex { owner, rank, .. } = self.obj(id)? else {
            return Err(Error::WrongKind);
        };
        match owner {
            Some(o) if o == cur => Err(Error::Recursive),
            Some(_) => Err(Error::WouldBlock),
            None => {
                self.check_order(cur, rank)?;
                let abandoned = self.take_free(id, cur)?;
                Ok(LockOutcome::Acquired { abandoned })
            }
        }
    }

    /// Locks `id` for the current thread, or blocks it. See the module rules.
    pub fn mutex_lock(&mut self, s: &mut Scheduler<N>, id: ObjId) -> Result<LockOutcome, Error> {
        let cur = Self::current(s)?;
        let Obj::Mutex { owner, rank, .. } = self.obj(id)? else {
            return Err(Error::WrongKind);
        };
        if owner == Some(cur) {
            return Err(Error::Recursive);
        }
        self.check_order(cur, rank)?;
        let Some(mut o) = owner else {
            let abandoned = self.take_free(id, cur)?;
            return Ok(LockOutcome::Acquired { abandoned });
        };
        // Deadlock: does the owner chain lead back to the caller?
        for _ in 0..=N {
            if o == cur {
                self.stats.deadlocks_prevented += 1;
                return Err(Error::Deadlock);
            }
            let next =
                self.waits
                    .get(o.index())
                    .copied()
                    .flatten()
                    .and_then(|w| match self.obj(w.obj) {
                        Ok(Obj::Mutex { owner: Some(n), .. }) => Some(n),
                        _ => None,
                    });
            match next {
                Some(n) => o = n,
                None => break,
            }
        }
        self.block_on(s, id)?;
        self.enqueue(cur, id);
        self.propagate(s, cur)?;
        Ok(LockOutcome::Blocked)
    }

    /// Releases mutex `id` held by `from` and hands it to the best waiter, if any.
    fn release(&mut self, s: &mut Scheduler<N>, id: ObjId, from: Handle) -> Result<(), Error> {
        let (rank_abandoned, _) = match self.obj(id)? {
            Obj::Mutex {
                abandoned, rank, ..
            } => (abandoned, rank),
            _ => return Err(Error::WrongKind),
        };
        let next = self.pick_waiter(s, id);
        if let Obj::Mutex { owner, .. } = self.obj_mut(id)? {
            *owner = next;
        }
        // De-boost the old owner BEFORE waking the new one so the wake-up's preemption check
        // compares against the old owner's true level.
        self.recompute(s, from)?;
        if let Some(n) = next {
            self.waits[n.index()] = None;
            self.grant_abandoned[n.index()] = rank_abandoned;
            if rank_abandoned {
                if let Obj::Mutex { abandoned, .. } = self.obj_mut(id)? {
                    *abandoned = false;
                }
            }
            self.stats.handoffs += 1;
            self.recompute(s, n)?;
            s.wake(n)?;
        }
        Ok(())
    }

    pub fn mutex_unlock(&mut self, s: &mut Scheduler<N>, id: ObjId) -> Result<(), Error> {
        let cur = s.current().ok_or(Error::NoCurrent)?;
        match self.obj(id)? {
            Obj::Mutex { owner, .. } if owner == Some(cur) => {}
            Obj::Mutex { .. } => return Err(Error::NotOwner),
            _ => return Err(Error::WrongKind),
        }
        self.release(s, id, cur)
    }

    pub fn mutex_owner(&self, id: ObjId) -> Result<Option<Handle>, Error> {
        match self.obj(id)? {
            Obj::Mutex { owner, .. } => Ok(owner),
            _ => Err(Error::WrongKind),
        }
    }

    /// Completes a blocking wait after the thread resumed: reports whether the object was
    /// granted, or the thread was woken by something else (the wait is then withdrawn).
    pub fn resume(&mut self, s: &mut Scheduler<N>, h: Handle) -> Result<Resume, Error> {
        match self.waits[h.index()] {
            None => {
                let abandoned = core::mem::take(&mut self.grant_abandoned[h.index()]);
                Ok(Resume::Granted { abandoned })
            }
            Some(w) => {
                self.waits[h.index()] = None;
                if let Ok(Obj::Mutex { owner: Some(o), .. }) = self.obj(w.obj) {
                    self.propagate(s, o)?;
                }
                Ok(Resume::Interrupted)
            }
        }
    }

    // ---- semaphore ------------------------------------------------------------------------

    pub fn sem_try_wait(&mut self, id: ObjId) -> Result<(), Error> {
        match self.obj_mut(id)? {
            Obj::Sem { count, .. } if *count > 0 => {
                *count -= 1;
                Ok(())
            }
            Obj::Sem { .. } => Err(Error::WouldBlock),
            _ => Err(Error::WrongKind),
        }
    }

    pub fn sem_wait(&mut self, s: &mut Scheduler<N>, id: ObjId) -> Result<WaitOutcome, Error> {
        let cur = Self::current(s)?;
        if self.sem_try_wait(id).is_ok() {
            return Ok(WaitOutcome::Acquired);
        }
        self.block_on(s, id)?;
        self.enqueue(cur, id);
        Ok(WaitOutcome::Blocked)
    }

    /// Releases one unit: handed to the best waiter, else added to the count. Non-blocking, so
    /// legal from interrupt context.
    pub fn sem_post(&mut self, s: &mut Scheduler<N>, id: ObjId) -> Result<(), Error> {
        let Obj::Sem { count, max } = self.obj(id)? else {
            return Err(Error::WrongKind);
        };
        if let Some(n) = self.pick_waiter(s, id) {
            self.waits[n.index()] = None;
            self.stats.handoffs += 1;
            s.wake(n)?;
            return Ok(());
        }
        if count >= max {
            return Err(Error::Overflow);
        }
        if let Obj::Sem { count, .. } = self.obj_mut(id)? {
            *count += 1;
        }
        Ok(())
    }

    pub fn sem_count(&self, id: ObjId) -> Result<u32, Error> {
        match self.obj(id)? {
            Obj::Sem { count, .. } => Ok(count),
            _ => Err(Error::WrongKind),
        }
    }

    // ---- wait queue / condition variable ------------------------------------------------

    /// Blocks the current thread on queue `id`. Always blocks (there is no condition); pair with
    /// `wq_wake_one/all`. Callers re-check their predicate after waking.
    pub fn wq_wait(&mut self, s: &mut Scheduler<N>, id: ObjId) -> Result<(), Error> {
        let cur = Self::current(s)?;
        match self.obj(id)? {
            Obj::Queue => {}
            _ => return Err(Error::WrongKind),
        }
        self.block_on(s, id)?;
        self.enqueue(cur, id);
        Ok(())
    }

    pub fn wq_wake_one(&mut self, s: &mut Scheduler<N>, id: ObjId) -> Result<bool, Error> {
        match self.obj(id)? {
            Obj::Queue => {}
            _ => return Err(Error::WrongKind),
        }
        let Some(n) = self.pick_waiter(s, id) else {
            return Ok(false);
        };
        self.waits[n.index()] = None;
        s.wake(n)?;
        Ok(true)
    }

    pub fn wq_wake_all(&mut self, s: &mut Scheduler<N>, id: ObjId) -> Result<usize, Error> {
        let mut n = 0;
        while self.wq_wake_one(s, id)? {
            n += 1;
        }
        Ok(n)
    }

    /// Condition-variable wait: atomically releases `mutex` (owned by the caller) and blocks on
    /// `cond`. Because both happen in one call (the scheduler lock is held and interrupts are
    /// off in the wrapper), a `cond_signal` issued after the unlock cannot be lost. After
    /// resuming, the caller must re-lock `mutex` and re-check its predicate.
    pub fn cond_wait(
        &mut self,
        s: &mut Scheduler<N>,
        cond: ObjId,
        mutex: ObjId,
    ) -> Result<(), Error> {
        let cur = Self::current(s)?;
        if !matches!(self.obj(cond)?, Obj::Queue) {
            return Err(Error::WrongKind);
        }
        match self.obj(mutex)? {
            Obj::Mutex { owner, .. } if owner == Some(cur) => {}
            Obj::Mutex { .. } => return Err(Error::NotOwner),
            _ => return Err(Error::WrongKind),
        }
        self.block_on(s, cond)?;
        self.enqueue(cur, cond);
        self.release(s, mutex, cur)?;
        Ok(())
    }

    pub fn cond_signal(&mut self, s: &mut Scheduler<N>, cond: ObjId) -> Result<bool, Error> {
        self.wq_wake_one(s, cond)
    }

    pub fn cond_broadcast(&mut self, s: &mut Scheduler<N>, cond: ObjId) -> Result<usize, Error> {
        self.wq_wake_all(s, cond)
    }

    // ---- thread exit ----------------------------------------------------------------------

    /// Call BEFORE the thread terminates (or right after `kill`, before reaping): withdraws its
    /// wait and releases every mutex it owns as abandoned. Returns how many it released.
    pub fn thread_exit(&mut self, s: &mut Scheduler<N>, h: Handle) -> Result<usize, Error> {
        if let Some(w) = self.waits[h.index()].take() {
            if let Ok(Obj::Mutex { owner: Some(o), .. }) = self.obj(w.obj) {
                if o != h {
                    self.propagate(s, o)?;
                }
            }
        }
        self.grant_abandoned[h.index()] = false;
        let mut released = 0;
        for i in 0..S {
            let Slot {
                gen_,
                obj: Some(Obj::Mutex { owner: Some(o), .. }),
                ..
            } = self.slots[i]
            else {
                continue;
            };
            if o != h {
                continue;
            }
            let id = ObjId {
                index: i as u16,
                gen_,
            };
            if let Obj::Mutex { abandoned, .. } = self.obj_mut(id)? {
                *abandoned = true;
            }
            self.stats.abandoned += 1;
            self.release(s, id, h)?;
            released += 1;
        }
        Ok(released)
    }

    // ---- diagnostics ------------------------------------------------------------------------

    /// Structural self-check for quiescent points (after the scheduler has switched).
    pub fn check_invariants(&self, s: &Scheduler<N>) -> Result<(), &'static str> {
        for (i, w) in self.waits.iter().enumerate() {
            let Some(w) = w else { continue };
            if w.handle.index() != i {
                return Err("wait record in the wrong slot");
            }
            if !self.is_valid_waiter(s, w) {
                return Err("waiter is not blocked on its object");
            }
            match self.obj(w.obj).map_err(|_| "waiting on a dead object")? {
                Obj::Mutex { owner: Some(o), .. } => {
                    if o == w.handle {
                        return Err("thread waits on a mutex it owns");
                    }
                }
                Obj::Mutex { owner: None, .. } => return Err("waiter on an unowned mutex"),
                Obj::Sem { count, .. } => {
                    if count != 0 {
                        return Err("waiter on a semaphore with units available");
                    }
                }
                Obj::Queue => {}
            }
        }
        for slot in &self.slots {
            if let Some(Obj::Mutex { owner: Some(o), .. }) = slot.obj {
                let t = s.thread(o).map_err(|_| "mutex owned by a stale thread")?;
                if t.state() == State::Terminated {
                    return Err("mutex owned by a terminated thread");
                }
            }
        }
        for i in 0..N {
            let Some(h) = s.handle_at(i) else { continue };
            let t = s.thread(h).map_err(|_| "stale handle")?;
            if t.is_idle() {
                continue;
            }
            if t.inherited_level() != self.expected_inherited(s, h) {
                return Err("inherited priority differs from the lock graph");
            }
        }
        Ok(())
    }
}

// ---- blocking wrappers (real context switches) -----------------------------------------

#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use blocking::*;

#[cfg(target_arch = "x86_64")]
mod blocking {
    use super::*;
    use crate::dispatch::{self, KernelSlot};
    use crate::irq::IrqGuard;
    use crate::kthread::{self, StackMemory};
    use crate::preempt;
    use crate::spin::SpinLock;

    /// Errors of the blocking layer.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SyncError {
        Sync(Error),
        Kernel(kthread::Error),
    }
    impl From<Error> for SyncError {
        fn from(e: Error) -> Self {
            SyncError::Sync(e)
        }
    }
    impl From<kthread::Error> for SyncError {
        fn from(e: kthread::Error) -> Self {
            SyncError::Kernel(e)
        }
    }

    /// A `SyncDomain` behind its own lock. Lock order: `KernelSlot` lock, then this one.
    pub struct SyncSlot<const N: usize, const S: usize> {
        inner: SpinLock<SyncDomain<N, S>>,
    }

    impl<const N: usize, const S: usize> Default for SyncSlot<N, S> {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<const N: usize, const S: usize> SyncSlot<N, S> {
        pub const fn new() -> Self {
            Self {
                inner: SpinLock::new(SyncDomain::new()),
            }
        }
        /// Runs `f` with interrupts masked and the domain locked. Never call it while holding
        /// the kernel lock in the opposite order.
        pub fn with<R>(&self, f: impl FnOnce(&mut SyncDomain<N, S>) -> R) -> R {
            // SAFETY: ring 0; the guard restores the previous interrupt state.
            let _irq = unsafe { IrqGuard::new() };
            let mut g = self.inner.lock();
            f(&mut g)
        }
    }

    fn may_block() -> Result<(), SyncError> {
        match preempt::check_may_block() {
            Ok(()) => Ok(()),
            Err(preempt::BlockError::InterruptContext) => {
                Err(kthread::Error::InterruptContext.into())
            }
            Err(preempt::BlockError::PreemptDisabled) => {
                Err(kthread::Error::PreemptDisabled.into())
            }
        }
    }

    fn me<const N: usize, M: StackMemory>(k: &KernelSlot<N, M>) -> Result<Handle, SyncError> {
        k.with(|kern| kern.sched().current())
            .ok_or(Error::NoCurrent.into())
    }

    /// Locks `id`, blocking (and donating priority) if needed. Returns whether the previous
    /// owner died holding it.
    ///
    /// # Safety
    /// See `dispatch::reschedule`.
    pub unsafe fn mutex_lock<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        id: ObjId,
    ) -> Result<bool, SyncError> {
        may_block()?;
        // SAFETY: ring 0. IF stays clear from the state change until the resume check.
        let _irq = unsafe { IrqGuard::new() };
        let h = me(k)?;
        let out = k.with(|kern| sy.with(|d| d.mutex_lock(kern.sched_mut(), id)))?;
        match out {
            LockOutcome::Acquired { abandoned } => Ok(abandoned),
            LockOutcome::Blocked => {
                // SAFETY: forwarded.
                unsafe { dispatch::reschedule(k) }?;
                match k.with(|kern| sy.with(|d| d.resume(kern.sched_mut(), h)))? {
                    Resume::Granted { abandoned } => Ok(abandoned),
                    Resume::Interrupted => Err(Error::Interrupted.into()),
                }
            }
        }
    }

    pub fn mutex_unlock<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        id: ObjId,
    ) -> Result<(), SyncError> {
        if preempt::in_interrupt() {
            return Err(kthread::Error::InterruptContext.into());
        }
        k.with(|kern| sy.with(|d| d.mutex_unlock(kern.sched_mut(), id)))?;
        Ok(())
    }

    /// # Safety
    /// See `dispatch::reschedule`.
    pub unsafe fn sem_wait<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        id: ObjId,
    ) -> Result<(), SyncError> {
        may_block()?;
        // SAFETY: ring 0.
        let _irq = unsafe { IrqGuard::new() };
        let h = me(k)?;
        match k.with(|kern| sy.with(|d| d.sem_wait(kern.sched_mut(), id)))? {
            WaitOutcome::Acquired => Ok(()),
            WaitOutcome::Blocked => {
                // SAFETY: forwarded.
                unsafe { dispatch::reschedule(k) }?;
                match k.with(|kern| sy.with(|d| d.resume(kern.sched_mut(), h)))? {
                    Resume::Granted { .. } => Ok(()),
                    Resume::Interrupted => Err(Error::Interrupted.into()),
                }
            }
        }
    }

    /// Legal from interrupt context.
    pub fn sem_post<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        id: ObjId,
    ) -> Result<(), SyncError> {
        k.with(|kern| sy.with(|d| d.sem_post(kern.sched_mut(), id)))?;
        Ok(())
    }

    /// Condition wait: releases `mutex`, blocks on `cond`, re-locks `mutex` on resume.
    ///
    /// # Safety
    /// See `dispatch::reschedule`.
    pub unsafe fn cond_wait<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        cond: ObjId,
        mutex: ObjId,
    ) -> Result<(), SyncError> {
        may_block()?;
        let h = me(k)?;
        {
            // SAFETY: ring 0. Release + enqueue + block + switch are one interrupt-atomic step.
            let _irq = unsafe { IrqGuard::new() };
            k.with(|kern| sy.with(|d| d.cond_wait(kern.sched_mut(), cond, mutex)))?;
            // SAFETY: forwarded.
            unsafe { dispatch::reschedule(k) }?;
            let r = k.with(|kern| sy.with(|d| d.resume(kern.sched_mut(), h)))?;
            if r == Resume::Interrupted {
                // Spurious wake-up: still a legal condition-variable outcome; fall through
                // and re-lock like a normal wake-up.
            }
        }
        // SAFETY: forwarded.
        unsafe { mutex_lock(k, sy, mutex) }.map(|_| ())
    }

    /// Legal from interrupt context.
    pub fn cond_signal<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        cond: ObjId,
    ) -> Result<bool, SyncError> {
        Ok(k.with(|kern| sy.with(|d| d.cond_signal(kern.sched_mut(), cond)))?)
    }

    /// Legal from interrupt context.
    pub fn cond_broadcast<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        cond: ObjId,
    ) -> Result<usize, SyncError> {
        Ok(k.with(|kern| sy.with(|d| d.cond_broadcast(kern.sched_mut(), cond)))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{Priority, WakeReason};
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    type S = Scheduler<16>;
    type D = SyncDomain<16, 8>;

    fn p(l: u8) -> Priority {
        Priority::new(l).unwrap()
    }

    /// schedule() + the architecture layer's switch_complete().
    fn run(s: &mut S) -> Handle {
        let sw = s.schedule().unwrap();
        if sw.is_switch()
            && let Some(prev) = sw.prev
        {
            s.switch_complete(prev).unwrap();
        }
        s.check_invariants().unwrap();
        sw.next
    }

    fn setup(prios: &[u8]) -> (S, D, Vec<Handle>) {
        let mut s = S::new();
        s.init_idle(0, 0).unwrap();
        let hs: Vec<Handle> = prios
            .iter()
            .map(|&l| s.spawn(p(l), 0, 0).unwrap())
            .collect();
        (s, D::new(), hs)
    }

    /// Runs until `h` is the current thread.
    fn run_until(s: &mut S, h: Handle) {
        for _ in 0..40 {
            if s.current() == Some(h) {
                return;
            }
            run(s);
        }
        panic!("thread never became current");
    }

    fn level(s: &S, h: Handle) -> usize {
        s.thread(h).unwrap().effective_level()
    }

    #[test]
    fn uncontended_mutex_ownership_rules() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        let m = d.create_mutex(0).unwrap();
        run_until(&mut s, hs[0]);
        assert_eq!(
            d.mutex_lock(&mut s, m),
            Ok(LockOutcome::Acquired { abandoned: false })
        );
        assert_eq!(d.mutex_owner(m), Ok(Some(hs[0])));
        assert_eq!(d.mutex_lock(&mut s, m), Err(Error::Recursive));
        assert_eq!(d.mutex_try_lock(&mut s, m), Err(Error::Recursive));
        // A different thread may neither unlock nor destroy it.
        run_until(&mut s, hs[1]);
        assert_eq!(d.mutex_unlock(&mut s, m), Err(Error::NotOwner));
        assert_eq!(d.mutex_try_lock(&mut s, m), Err(Error::WouldBlock));
        assert_eq!(d.destroy(m), Err(Error::Busy));
        run_until(&mut s, hs[0]);
        d.mutex_unlock(&mut s, m).unwrap();
        assert_eq!(
            d.mutex_unlock(&mut s, m),
            Err(Error::NotOwner),
            "double unlock"
        );
        d.destroy(m).unwrap();
        assert_eq!(
            d.mutex_lock(&mut s, m),
            Err(Error::Stale),
            "destroyed object"
        );
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn idle_and_wrong_kind_are_rejected() {
        let (mut s, mut d, hs) = setup(&[4]);
        let m = d.create_mutex(0).unwrap();
        let sem = d.create_semaphore(1, 2).unwrap();
        // Nothing has been scheduled yet.
        assert_eq!(s.current(), None);
        assert_eq!(d.mutex_lock(&mut s, m), Err(Error::NoCurrent));
        run_until(&mut s, hs[0]);
        assert_eq!(d.mutex_lock(&mut s, sem), Err(Error::WrongKind));
        assert_eq!(d.sem_post(&mut s, m), Err(Error::WrongKind));
        assert_eq!(d.wq_wait(&mut s, m), Err(Error::WrongKind));
        assert_eq!(d.create_semaphore(0, 0), Err(Error::BadArg));
        assert_eq!(d.create_semaphore(3, 2), Err(Error::BadArg));
    }

    fn spawn(s: &mut S, l: u8) -> Handle {
        s.spawn(p(l), 0, 0).unwrap()
    }

    #[test]
    fn blocking_lock_hands_off_in_priority_then_fifo_order() {
        let (mut s, mut d, hs) = setup(&[2]);
        let owner = hs[0];
        let m = d.create_mutex(0).unwrap();
        run_until(&mut s, owner);
        d.mutex_lock(&mut s, m).unwrap();
        // a(3), b(3), hi(5) arrive in that order and each blocks on the mutex.
        let mut ws = vec![];
        for l in [3u8, 3, 5] {
            let w = spawn(&mut s, l);
            ws.push(w);
            assert_eq!(
                run(&mut s),
                w,
                "the newcomer runs (owner is boosted to at most its level)"
            );
            assert_eq!(d.mutex_lock(&mut s, m), Ok(LockOutcome::Blocked));
            run(&mut s);
            d.check_invariants(&s).unwrap();
        }
        let [a, b, hi] = [ws[0], ws[1], ws[2]];
        assert_eq!(d.waiting_threads(), 3);
        assert_eq!(level(&s, owner), 5);
        run_until(&mut s, owner);
        let mut order = vec![];
        let mut cur = owner;
        for i in 0..3 {
            d.mutex_unlock(&mut s, m).unwrap();
            if i > 0 {
                // The previous grantee is done; let the next one run.
                s.terminate_current(0).unwrap();
            }
            let new_owner = d.mutex_owner(m).unwrap().unwrap();
            order.push(new_owner);
            run_until(&mut s, new_owner);
            assert_eq!(
                d.resume(&mut s, new_owner),
                Ok(Resume::Granted { abandoned: false })
            );
            d.check_invariants(&s).unwrap();
            cur = new_owner;
        }
        let _ = cur;
        assert_eq!(
            order,
            vec![hi, a, b],
            "highest level first, FIFO within a level"
        );
        d.mutex_unlock(&mut s, m).unwrap();
        assert_eq!(d.mutex_owner(m), Ok(None));
        assert_eq!(d.stats().handoffs, 3);
    }

    #[test]
    fn priority_inheritance_is_transitive_and_fully_undone() {
        // L(2) owns A; M(3) owns B and waits for A; H(6) waits for B.
        let (mut s, mut d, hs) = setup(&[2]);
        let l = hs[0];
        let (a, b) = (d.create_mutex(0).unwrap(), d.create_mutex(0).unwrap());
        run_until(&mut s, l);
        d.mutex_lock(&mut s, a).unwrap();
        let m = spawn(&mut s, 3);
        assert_eq!(run(&mut s), m);
        d.mutex_lock(&mut s, b).unwrap();
        assert_eq!(d.mutex_lock(&mut s, a), Ok(LockOutcome::Blocked));
        assert_eq!(run(&mut s), l);
        assert_eq!(level(&s, l), 3, "L inherits M");
        let h = spawn(&mut s, 6);
        assert_eq!(run(&mut s), h);
        assert_eq!(d.mutex_lock(&mut s, b), Ok(LockOutcome::Blocked));
        assert_eq!(run(&mut s), l);
        assert_eq!(level(&s, m), 6, "M inherits H");
        assert_eq!(level(&s, l), 6, "L inherits H through M");
        d.check_invariants(&s).unwrap();
        // L unlocks A: ownership moves to M, L drops back to its base level.
        d.mutex_unlock(&mut s, a).unwrap();
        assert_eq!(level(&s, l), 2, "inheritance released with the lock");
        assert_eq!(level(&s, m), 6, "M still blocks H");
        run_until(&mut s, m);
        d.resume(&mut s, m).unwrap();
        d.mutex_unlock(&mut s, b).unwrap();
        assert_eq!(level(&s, m), 3);
        d.check_invariants(&s).unwrap();
        assert!(d.stats().inheritance_raises >= 3);
    }

    #[test]
    fn inheritance_prevents_priority_inversion_in_scheduling_order() {
        // Without inheritance the middle-priority thread would starve the owner.
        let (mut s, mut d, hs) = setup(&[2]);
        let l = hs[0];
        let m = d.create_mutex(0).unwrap();
        run_until(&mut s, l);
        d.mutex_lock(&mut s, m).unwrap();
        let h = spawn(&mut s, 6);
        assert_eq!(run(&mut s), h);
        assert_eq!(d.mutex_lock(&mut s, m), Ok(LockOutcome::Blocked));
        let mid = spawn(&mut s, 4);
        // `mid` (4) is Ready and `l` is boosted to 6: the owner must run before `mid`.
        assert_eq!(run(&mut s), l, "owner, not the middle thread");
        d.mutex_unlock(&mut s, m).unwrap();
        assert_eq!(
            run(&mut s),
            h,
            "the waiter, not the middle thread, runs next"
        );
        assert_eq!(s.thread(mid).unwrap().state(), State::Ready);
    }

    #[test]
    fn lock_order_violations_are_reported_and_leave_no_trace() {
        let (mut s, mut d, hs) = setup(&[4]);
        let r1 = d.create_mutex(1).unwrap();
        let r2 = d.create_mutex(2).unwrap();
        let r2b = d.create_mutex(2).unwrap();
        let free = d.create_mutex(0).unwrap();
        run_until(&mut s, hs[0]);
        d.mutex_lock(&mut s, r1).unwrap();
        d.mutex_lock(&mut s, r2).unwrap();
        d.mutex_lock(&mut s, free).unwrap(); // rank 0 is exempt
        d.mutex_unlock(&mut s, r1).unwrap();
        // Holding rank 2: rank 1 and a second rank 2 are violations.
        assert_eq!(
            d.mutex_lock(&mut s, r1),
            Err(Error::LockOrder {
                held: 2,
                requested: 1
            })
        );
        assert_eq!(
            d.mutex_lock(&mut s, r2b),
            Err(Error::LockOrder {
                held: 2,
                requested: 2
            })
        );
        assert_eq!(
            d.mutex_try_lock(&mut s, r1),
            Err(Error::LockOrder {
                held: 2,
                requested: 1
            })
        );
        assert_eq!(d.mutex_owner(r1), Ok(None));
        assert_eq!(d.stats().lock_order_violations, 3);
        d.mutex_unlock(&mut s, r2).unwrap();
        d.mutex_lock(&mut s, r1).unwrap();
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn deadlock_cycles_are_refused_before_blocking() {
        let (mut s, mut d, hs) = setup(&[4, 4, 4]);
        let [t1, t2, t3] = [hs[0], hs[1], hs[2]];
        let ms = [
            d.create_mutex(0).unwrap(),
            d.create_mutex(0).unwrap(),
            d.create_mutex(0).unwrap(),
        ];
        for (t, m) in [t1, t2, t3].into_iter().zip(ms) {
            run_until(&mut s, t);
            d.mutex_lock(&mut s, m).unwrap();
        }
        // t1 -> m2 (owned by t2), t2 -> m3 (owned by t3): fine, a chain.
        run_until(&mut s, t1);
        assert_eq!(d.mutex_lock(&mut s, ms[1]), Ok(LockOutcome::Blocked));
        run(&mut s);
        run_until(&mut s, t2);
        assert_eq!(d.mutex_lock(&mut s, ms[2]), Ok(LockOutcome::Blocked));
        run(&mut s);
        // t3 -> m1 (owned by t1) would close the cycle t3->t1->t2->t3.
        run_until(&mut s, t3);
        assert_eq!(d.mutex_lock(&mut s, ms[0]), Err(Error::Deadlock));
        assert_eq!(
            s.thread(t3).unwrap().state(),
            State::Running,
            "refused, never blocked"
        );
        assert_eq!(d.stats().deadlocks_prevented, 1);
        d.check_invariants(&s).unwrap();
        // The chain is still sound: t3 releasing lets t2 proceed.
        d.mutex_unlock(&mut s, ms[2]).unwrap();
        assert_eq!(d.mutex_owner(ms[2]), Ok(Some(t2)));
    }

    #[test]
    fn semaphore_counts_blocks_and_hands_off() {
        let (mut s, mut d, hs) = setup(&[4, 4, 4]);
        let sem = d.create_semaphore(1, 2).unwrap();
        run_until(&mut s, hs[0]);
        assert_eq!(d.sem_wait(&mut s, sem), Ok(WaitOutcome::Acquired));
        assert_eq!(d.sem_try_wait(sem), Err(Error::WouldBlock));
        assert_eq!(d.sem_wait(&mut s, sem), Ok(WaitOutcome::Blocked));
        run(&mut s);
        run_until(&mut s, hs[1]);
        assert_eq!(d.sem_wait(&mut s, sem), Ok(WaitOutcome::Blocked));
        run(&mut s);
        d.check_invariants(&s).unwrap();
        run_until(&mut s, hs[2]);
        // Two posts wake the two waiters FIFO; the count stays 0 (units are handed over).
        d.sem_post(&mut s, sem).unwrap();
        d.sem_post(&mut s, sem).unwrap();
        assert_eq!(d.sem_count(sem), Ok(0));
        assert_eq!(s.thread(hs[0]).unwrap().state(), State::Ready);
        assert_eq!(s.thread(hs[1]).unwrap().state(), State::Ready);
        // With no waiters left, posts accumulate up to the maximum, then overflow.
        d.sem_post(&mut s, sem).unwrap();
        d.sem_post(&mut s, sem).unwrap();
        assert_eq!(d.sem_post(&mut s, sem), Err(Error::Overflow));
        assert_eq!(d.sem_count(sem), Ok(2));
        run_until(&mut s, hs[0]);
        assert_eq!(
            d.resume(&mut s, hs[0]),
            Ok(Resume::Granted { abandoned: false })
        );
    }

    #[test]
    fn condition_variable_releases_the_mutex_atomically_and_never_loses_a_signal() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        let [waiter, signaller] = [hs[0], hs[1]];
        let (m, cv) = (d.create_mutex(0).unwrap(), d.create_queue().unwrap());
        run_until(&mut s, waiter);
        d.mutex_lock(&mut s, m).unwrap();
        d.cond_wait(&mut s, cv, m).unwrap();
        assert_eq!(d.mutex_owner(m), Ok(None), "mutex released by the wait");
        run(&mut s);
        run_until(&mut s, signaller);
        // The signaller takes the mutex the waiter gave up, then signals.
        d.mutex_lock(&mut s, m).unwrap();
        assert_eq!(d.cond_signal(&mut s, cv), Ok(true));
        assert_eq!(d.cond_signal(&mut s, cv), Ok(false), "no more waiters");
        d.mutex_unlock(&mut s, m).unwrap();
        run_until(&mut s, waiter);
        assert_eq!(
            d.resume(&mut s, waiter),
            Ok(Resume::Granted { abandoned: false })
        );
        // cond_wait demands ownership.
        assert_eq!(d.cond_wait(&mut s, cv, m), Err(Error::NotOwner));
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn broadcast_wakes_every_waiter() {
        let (mut s, mut d, hs) = setup(&[4, 4, 4, 4]);
        let q = d.create_queue().unwrap();
        for &w in &hs[..3] {
            run_until(&mut s, w);
            d.wq_wait(&mut s, q).unwrap();
            run(&mut s);
        }
        run_until(&mut s, hs[3]);
        assert_eq!(d.destroy(q), Err(Error::Busy));
        assert_eq!(d.wq_wake_all(&mut s, q), Ok(3));
        assert_eq!(d.wq_wake_all(&mut s, q), Ok(0));
        for &w in &hs[..3] {
            assert_eq!(s.thread(w).unwrap().state(), State::Ready);
            assert_eq!(s.thread(w).unwrap().wake_reason(), WakeReason::Event);
        }
        d.destroy(q).unwrap();
    }

    #[test]
    fn external_wakeup_withdraws_the_wait_and_removes_inheritance() {
        let (mut s, mut d, hs) = setup(&[2]);
        let owner = hs[0];
        let m = d.create_mutex(0).unwrap();
        run_until(&mut s, owner);
        d.mutex_lock(&mut s, m).unwrap();
        let waiter = spawn(&mut s, 6);
        assert_eq!(run(&mut s), waiter);
        assert_eq!(d.mutex_lock(&mut s, m), Ok(LockOutcome::Blocked));
        assert_eq!(run(&mut s), owner);
        assert_eq!(level(&s, owner), 6);
        // Something else wakes the waiter (not a grant).
        s.wake(waiter).unwrap();
        run_until(&mut s, waiter);
        assert_eq!(d.resume(&mut s, waiter), Ok(Resume::Interrupted));
        assert_eq!(level(&s, owner), 2, "donation withdrawn with the wait");
        assert_eq!(d.mutex_owner(m), Ok(Some(owner)));
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn a_pending_wakeup_does_not_make_a_lock_return_early() {
        let (mut s, mut d, hs) = setup(&[2]);
        let m = d.create_mutex(0).unwrap();
        run_until(&mut s, hs[0]);
        d.mutex_lock(&mut s, m).unwrap();
        let t = spawn(&mut s, 4);
        assert_eq!(run(&mut s), t);
        // A stale wake-up targeted at the running thread is recorded...
        assert_eq!(s.wake(t), Ok(crate::sched::WakeOutcome::Pending));
        // ...and must be consumed without turning the lock into a bogus acquisition.
        assert_eq!(d.mutex_lock(&mut s, m), Ok(LockOutcome::Blocked));
        assert_eq!(s.thread(t).unwrap().state(), State::Blocked);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn owner_exit_abandons_the_mutex_and_hands_it_on() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        let [owner, waiter] = [hs[0], hs[1]];
        let m = d.create_mutex(0).unwrap();
        run_until(&mut s, owner);
        d.mutex_lock(&mut s, m).unwrap();
        run_until(&mut s, waiter);
        assert_eq!(d.mutex_lock(&mut s, m), Ok(LockOutcome::Blocked));
        run(&mut s);
        run_until(&mut s, owner);
        assert_eq!(d.thread_exit(&mut s, owner), Ok(1));
        s.terminate_current(0).unwrap();
        run(&mut s);
        assert_eq!(d.mutex_owner(m), Ok(Some(waiter)));
        run_until(&mut s, waiter);
        assert_eq!(
            d.resume(&mut s, waiter),
            Ok(Resume::Granted { abandoned: true })
        );
        d.check_invariants(&s).unwrap();
        assert_eq!(d.stats().abandoned, 1);
        // The flag is cleared once reported.
        d.mutex_unlock(&mut s, m).unwrap();
        assert_eq!(
            d.mutex_try_lock(&mut s, m),
            Ok(LockOutcome::Acquired { abandoned: false })
        );
    }

    #[test]
    fn killing_a_blocked_waiter_cleans_its_wait_and_donation() {
        let (mut s, mut d, hs) = setup(&[2]);
        let owner = hs[0];
        let m = d.create_mutex(0).unwrap();
        run_until(&mut s, owner);
        d.mutex_lock(&mut s, m).unwrap();
        let waiter = spawn(&mut s, 6);
        assert_eq!(run(&mut s), waiter);
        assert_eq!(d.mutex_lock(&mut s, m), Ok(LockOutcome::Blocked));
        assert_eq!(run(&mut s), owner);
        d.thread_exit(&mut s, waiter).unwrap();
        s.kill(waiter, -9).unwrap();
        assert_eq!(level(&s, owner), 2);
        assert_eq!(d.waiting_threads(), 0);
        d.check_invariants(&s).unwrap();
        d.mutex_unlock(&mut s, m).unwrap();
        assert_eq!(d.mutex_owner(m), Ok(None));
    }

    #[test]
    fn object_table_exhaustion_and_generation_safety() {
        let mut d = SyncDomain::<4, 3>::new();
        let ids: Vec<ObjId> = (0..3).map(|_| d.create_mutex(0).unwrap()).collect();
        assert_eq!(d.create_mutex(0), Err(Error::NoObjects));
        d.destroy(ids[1]).unwrap();
        let again = d.create_semaphore(0, 1).unwrap();
        assert_eq!(again.index(), ids[1].index());
        assert_ne!(again, ids[1], "generation differs");
        assert_eq!(d.destroy(ids[1]), Err(Error::Stale));
        assert_eq!(d.live_objects(), 3);
    }

    /// Randomised model test: arbitrary lock/unlock/sem/queue/exit operations by arbitrary
    /// threads; every structural invariant (incl. exact inherited priorities) is checked after
    /// every step.
    #[test]
    fn randomised_operations_preserve_all_invariants() {
        for seed in 1..=6u64 {
            let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let mut next = move || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng
            };
            let prios = [1u8, 2, 3, 4, 5, 6, 7, 3];
            let (mut s, mut d, hs) = setup(&prios);
            let mx: Vec<ObjId> = (0..3)
                .map(|i| d.create_mutex(if i == 2 { 5 } else { 0 }).unwrap())
                .collect();
            let sem = d.create_semaphore(1, 3).unwrap();
            let q = d.create_queue().unwrap();
            let mut dead = [false; 8];
            for _ in 0..4000 {
                let cur = run(&mut s);
                let Some(i) = hs.iter().position(|&h| h == cur) else {
                    // idle: wake something blocked externally so the system keeps moving
                    for &h in &hs {
                        if s.thread(h).is_ok_and(|t| t.state() == State::Blocked) {
                            let _ = d.wq_wake_all(&mut s, q);
                            let _ = d.sem_post(&mut s, sem);
                            break;
                        }
                    }
                    continue;
                };
                // A thread that was woken must first complete its wait.
                if d.waits[cur.index()].is_none()
                    && s.thread(cur)
                        .is_ok_and(|t| t.wake_reason() == WakeReason::Event)
                {
                    let _ = d.resume(&mut s, cur);
                }
                let r = next() % 12;
                let m = mx[(next() % 3) as usize];
                let res = match r {
                    0..=2 => d.mutex_lock(&mut s, m).map(|_| ()),
                    3 | 4 => d.mutex_unlock(&mut s, m),
                    5 => d.sem_wait(&mut s, sem).map(|_| ()),
                    6 => d.sem_post(&mut s, sem),
                    7 => d.wq_wait(&mut s, q),
                    8 => d.wq_wake_one(&mut s, q).map(|_| ()),
                    9 => d.mutex_try_lock(&mut s, m).map(|_| ()),
                    10 if next() % 40 == 0 && !dead[i] => {
                        dead[i] = true;
                        let r = d.thread_exit(&mut s, cur).map(|_| ());
                        s.terminate_current(0).unwrap();
                        r
                    }
                    _ => Ok(()),
                };
                if let Err(e) = res {
                    assert!(
                        matches!(
                            e,
                            Error::Recursive
                                | Error::WouldBlock
                                | Error::NotOwner
                                | Error::LockOrder { .. }
                                | Error::Deadlock
                                | Error::Overflow
                        ),
                        "seed {seed}: unexpected {e:?}"
                    );
                }
                // Reap dead threads' bookkeeping so handles stay quiescent.
                if let Some(pending) = (0..16).filter_map(|j| s.handle_at(j)).find(|&h| {
                    s.thread(h)
                        .is_ok_and(|t| t.state() == State::Terminated && !t.on_cpu())
                        && s.current() != Some(h)
                }) {
                    let _ = s.reap(pending);
                }
                if let Err(e) = d.check_invariants(&s) {
                    panic!("seed {seed}: invariant: {e}");
                }
            }
        }
    }
}
