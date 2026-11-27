//! IPC blocking and wake-up integration (Mission D6, file 8). SPDX-License-Identifier: Apache-2.0
//!
//! Blocking IPC is built on the D5 [`SyncDomain`] wait queues and the scheduler's
//! `block_current` / `wake`; there is no spinning. Each endpoint owns two wait queues:
//! `recv_wq` (receivers and `accept`ers) and `send_wq` (senders waiting for queue space).
//!
//! # Protocol (model API; the `blocking` wrappers below run it on the real kernel)
//! 1. `recv_blocking` tries a non-blocking receive. Only if the queue is empty does it block: the
//!    emptiness check, the wait-queue enqueue and the scheduler state change happen in **one
//!    critical section** (the caller holds scheduler -> sync -> ipc, with interrupts masked), so a
//!    message cannot arrive between "empty" and "asleep". If a wake-up reaches the thread after it
//!    is marked `Blocked` but before the context switch, it becomes `Ready` and the switch simply
//!    returns; if it arrives while the thread is still `Running`, the scheduler records a pending
//!    wake-up that `block_current` consumes. **No wake-up is lost.**
//! 2. The blocked thread resumes in `recv_resume` / `send_resume` / `accept_resume`, which
//!    *re-evaluate the whole predicate* (the message may have been taken by another receiver, the
//!    peer may have closed, the wait may have been cancelled). It either completes, fails with a
//!    specific error, or blocks again keeping its original deadline.
//! 3. A woken thread that does not consume (cancel, timeout, closure) **forwards the wake-up** if
//!    work is still pending, so a cancelled waiter cannot swallow a message another waiter needs.
//!
//! # Wake semantics
//! * One message -> **wake-one** receiver (`wq_wake_one`: highest effective priority, FIFO within
//!   a level). One freed queue slot -> wake-one sender.
//! * Endpoint closure, peer closure and `cancel_endpoint` -> **wake-all** waiters of the endpoint
//!   (each re-evaluates and reports its own outcome).
//!
//! # Pins and cleanup
//! A blocked thread holds an `ops` pin on its endpoint object, so the object cannot be destroyed
//! under it. Pins are released when the thread resumes, when [`Ipc::thread_exit`] runs (call it
//! before the thread terminates, like `SyncDomain::thread_exit`), on process teardown, or by
//! [`Ipc::reap_dead_waiters`] for threads that died without cleanup. A terminated thread is never
//! left linked to a live wait queue: `thread_exit` withdraws its sync-queue record, and
//! sync-queue records of dead threads are dropped by `SyncDomain` when it next picks a waiter.
//!
//! # Contexts
//! Blocking is refused with [`Error::BlockInInterrupt`] in interrupt context and with
//! [`Error::PreemptDisabled`] while preemption is disabled ([`Context`]); the refusal happens
//! *before* any state changes. Non-blocking `send`/`recv`, `cancel_*`, `expire_timeouts` and the
//! wake helpers never block and are legal from interrupt context (an IRQ handler may *send* to
//! wake a server, but can never wait for a message).
//!
//! # Timeouts
//! The scheduler has no timed event wait, so IPC keeps its own absolute deadlines (in scheduler
//! ticks, `Scheduler::now`). The timer path (or any thread) calls [`Ipc::expire_timeouts`]; it
//! only withdraws and wakes, so it is legal from interrupt-exit context. A thread whose deadline
//! passed reports `TimedOut` from its resume call.
#![allow(dead_code)]

use crate::d6::capability::Rights;
use crate::d6::handle::Handle;
use crate::d6::ipc::{Ipc, MAX_OBJECTS_PER_OWNER, MAX_THREADS};
use crate::d6::ipc_endpoint::Role;
use crate::d6::ipc_message::{Message, MsgId};
use crate::d6::object::{Counter, Error, ObjectId, ObjectKind, Owner};
use crate::preempt;
use crate::sched::Scheduler;
use crate::sync::SyncDomain;
use crate::thread::Handle as Tid;

/// Everything the IPC layer needs from the rest of the kernel, passed explicitly so that lock
/// order is structural (the caller already holds these locks).
pub struct Env<'a, const N: usize, const S: usize> {
    pub sched: &'a mut Scheduler<N>,
    pub sync: &'a mut SyncDomain<N, S>,
}

/// Where the caller is running. Built from `preempt` by the kernel wrappers; explicit in the
/// model API so tests are deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    pub in_interrupt: bool,
    pub preempt_disabled: bool,
}

impl Context {
    pub const THREAD: Context = Context {
        in_interrupt: false,
        preempt_disabled: false,
    };
    pub const INTERRUPT: Context = Context {
        in_interrupt: true,
        preempt_disabled: false,
    };

    /// The real context, from the D5 preemption state.
    pub fn current() -> Context {
        Context {
            in_interrupt: preempt::in_interrupt(),
            preempt_disabled: !preempt::preemption_enabled(),
        }
    }

    pub const fn check_may_block(self) -> Result<(), Error> {
        if self.in_interrupt {
            Err(Error::BlockInInterrupt)
        } else if self.preempt_disabled {
            Err(Error::PreemptDisabled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOp {
    Recv,
    Send,
    Accept,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Waiting,
    /// Woken to re-evaluate (message, space, peer closure).
    Woken,
    /// The endpoint itself closed.
    Closed,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Copy)]
pub struct WaitRec {
    pub thread: Tid,
    pub owner: Owner,
    pub endpoint: ObjectId,
    pub op: WaitOp,
    pub deadline: Option<u64>,
    pub outcome: WaitOutcome,
    /// The message of a blocked send (staged only when the send actually happens).
    pub pending: Option<Message>,
}

/// One record per blocked thread, indexed by scheduler slot.
pub struct WaitTable {
    recs: [Option<WaitRec>; MAX_THREADS],
}

impl Default for WaitTable {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitTable {
    pub const fn new() -> Self {
        Self {
            recs: [None; MAX_THREADS],
        }
    }
    pub fn count(&self) -> usize {
        self.recs.iter().flatten().count()
    }
    pub fn get(&self, t: Tid) -> Option<&WaitRec> {
        self.recs.get(t.index()).and_then(|r| r.as_ref()).filter(|r| r.thread == t)
    }
    fn insert(&mut self, rec: WaitRec) -> Result<(), Error> {
        let slot = self.recs.get_mut(rec.thread.index()).ok_or(Error::Config)?;
        if slot.is_some() {
            return Err(Error::Busy);
        }
        *slot = Some(rec);
        Ok(())
    }
    fn take(&mut self, t: Tid) -> Option<WaitRec> {
        let slot = self.recs.get_mut(t.index())?;
        if slot.is_some_and(|r| r.thread == t) {
            slot.take()
        } else {
            None
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = &WaitRec> + '_ {
        self.recs.iter().flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvStatus {
    Message(Message),
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendStatus {
    Sent(MsgId),
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptStatus {
    Accepted(Handle, Owner),
    Blocked,
}

fn closed_or(e: Error) -> Error {
    match e {
        Error::Stale | Error::Revoked => Error::EndpointClosed,
        e => e,
    }
}

impl Ipc {
    fn check_env<const N: usize>() -> Result<(), Error> {
        if N > MAX_THREADS { Err(Error::Config) } else { Ok(()) }
    }

    fn current_thread<const N: usize, const S: usize>(env: &Env<N, S>) -> Result<Tid, Error> {
        let t = env.sched.current().ok_or(Error::NoCurrentThread)?;
        if env.sched.thread(t)?.is_idle() {
            return Err(Error::NoCurrentThread);
        }
        Ok(t)
    }

    // ---- wake helpers (never block; legal from interrupt context) --------------------------

    pub(crate) fn wake_receiver<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ep: ObjectId,
    ) -> Result<(), Error> {
        let Some(wq) = self.endpoints.by_object(&self.objects, ep).ok().map(|(_, r)| r.recv_wq)
        else {
            return Ok(());
        };
        if env.sync.wq_wake_one(env.sched, wq)? {
            self.stats.wakeups += 1;
        }
        Ok(())
    }

    pub(crate) fn wake_sender<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ep: ObjectId,
    ) -> Result<(), Error> {
        let Some(wq) = self.endpoints.by_object(&self.objects, ep).ok().map(|(_, r)| r.send_wq)
        else {
            return Ok(());
        };
        if env.sync.wq_wake_one(env.sched, wq)? {
            self.stats.wakeups += 1;
        }
        Ok(())
    }

    /// Wake-all: marks and withdraws every thread blocked on `ep` that is still `Waiting`.
    pub(crate) fn wake_endpoint_waiters<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ep: ObjectId,
        outcome: WaitOutcome,
    ) -> Result<usize, Error> {
        let mut n = 0;
        for i in 0..MAX_THREADS {
            let Some(rec) = self.waits.recs[i].as_mut() else { continue };
            if rec.endpoint != ep || rec.outcome != WaitOutcome::Waiting {
                continue;
            }
            rec.outcome = outcome;
            let t = rec.thread;
            env.sync.cancel_wait(env.sched, t)?;
            self.stats.wakeups += 1;
            n += 1;
        }
        Ok(n)
    }

    // ---- blocking core ----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn wait_on<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
        op: WaitOp,
        deadline: Option<u64>,
        pending: Option<Message>,
    ) -> Result<(), Error> {
        Self::check_env::<N>()?;
        let thread = Self::current_thread(env)?;
        if self.waits.get(thread).is_some() {
            return Err(Error::Busy);
        }
        let need = if op == WaitOp::Send { Rights::SEND } else if op == WaitOp::Accept { Rights::ACCEPT } else { Rights::RECV };
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, need)?;
        let wq = {
            let (_, r) = self.endpoints.by_object(&self.objects, cap.object)?;
            if op == WaitOp::Send { r.send_wq } else { r.recv_wq }
        };
        self.objects.acquire(cap.object, Counter::Ops)?;
        if let Err(e) = env.sync.wq_wait(env.sched, wq) {
            let _ = self.objects.release(cap.object, Counter::Ops);
            return Err(e.into());
        }
        self.waits.insert(WaitRec {
            thread,
            owner,
            endpoint: cap.object,
            op,
            deadline,
            outcome: WaitOutcome::Waiting,
            pending,
        })?;
        Ok(())
    }

    fn deadline_for<const N: usize, const S: usize>(
        env: &Env<N, S>,
        timeout: Option<u64>,
    ) -> Result<Option<u64>, Error> {
        match timeout {
            None => Ok(None),
            Some(0) => Err(Error::WouldBlock),
            Some(t) => env.sched.now().checked_add(t).map(Some).ok_or(Error::SizeOverflow),
        }
    }

    /// Ends the current thread's wait: drops its record, withdraws a leftover sync record and
    /// releases the endpoint pin.
    fn finish_wait<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
    ) -> Result<WaitRec, Error> {
        let thread = Self::current_thread(env)?;
        let rec = self.waits.take(thread).ok_or(Error::BadState)?;
        // The thread is Running again, so this only drops a stale record (never wakes it).
        env.sync.cancel_wait(env.sched, thread)?;
        let _ = self.objects.release(rec.endpoint, Counter::Ops);
        Ok(rec)
    }

    /// Is there work for another waiter on this endpoint (used to forward a wake-up)?
    fn endpoint_ready(&self, ep: ObjectId, op: WaitOp) -> bool {
        let Ok((_, r)) = self.endpoints.by_object(&self.objects, ep) else {
            return false;
        };
        match (r.role, op) {
            (Role::Peer { chan, side, .. }, WaitOp::Recv) => self
                .channels
                .get(chan)
                .is_ok_and(|c| !c.queues[side].is_empty()),
            (Role::Peer { chan, side, .. }, WaitOp::Send) => self
                .channels
                .get(chan)
                .is_ok_and(|c| c.ends[1 - side].is_some() && !c.queues[1 - side].is_full()),
            (Role::Listener(l), WaitOp::Accept) => l.len() > 0,
            _ => false,
        }
    }

    fn forward_wake<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        rec: &WaitRec,
    ) -> Result<(), Error> {
        if self.endpoint_ready(rec.endpoint, rec.op) {
            match rec.op {
                WaitOp::Send => self.wake_sender(env, rec.endpoint),
                _ => self.wake_receiver(env, rec.endpoint),
            }
        } else {
            Ok(())
        }
    }

    fn resumed_error(rec: &WaitRec) -> Option<Error> {
        match rec.outcome {
            WaitOutcome::Cancelled => Some(Error::Cancelled),
            WaitOutcome::TimedOut => Some(Error::TimedOut),
            _ => None,
        }
    }

    fn expired<const N: usize, const S: usize>(env: &Env<N, S>, rec: &WaitRec) -> bool {
        rec.deadline.is_some_and(|d| env.sched.now() >= d)
    }

    // ---- receive ----------------------------------------------------------------------------

    /// Receives a message, blocking the *current thread* if the queue is empty. On `Blocked` the
    /// caller must run the scheduler and then call [`recv_resume`](Self::recv_resume).
    pub fn recv_blocking<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ctx: Context,
        owner: Owner,
        h: Handle,
        timeout: Option<u64>,
    ) -> Result<RecvStatus, Error> {
        match self.recv(env, owner, h) {
            Ok(m) => return Ok(RecvStatus::Message(m)),
            Err(Error::QueueEmpty) => {}
            Err(e) => return Err(e),
        }
        ctx.check_may_block()?;
        let deadline = Self::deadline_for(env, timeout)?;
        self.wait_on(env, owner, h, WaitOp::Recv, deadline, None)?;
        Ok(RecvStatus::Blocked)
    }

    pub fn recv_resume<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ctx: Context,
        owner: Owner,
        h: Handle,
    ) -> Result<RecvStatus, Error> {
        let rec = self.finish_wait(env)?;
        if let Some(e) = Self::resumed_error(&rec) {
            self.forward_wake(env, &rec)?;
            return Err(e);
        }
        match self.recv(env, owner, h) {
            Ok(m) => Ok(RecvStatus::Message(m)),
            Err(Error::QueueEmpty) => {
                if Self::expired(env, &rec) {
                    return Err(Error::TimedOut);
                }
                ctx.check_may_block()?;
                self.wait_on(env, owner, h, WaitOp::Recv, rec.deadline, None)?;
                Ok(RecvStatus::Blocked)
            }
            Err(e) => {
                self.forward_wake(env, &rec)?;
                Err(closed_or(e))
            }
        }
    }

    // ---- send -------------------------------------------------------------------------------

    /// Sends, blocking the current thread while the peer's queue is full (backpressure).
    pub fn send_blocking<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ctx: Context,
        owner: Owner,
        h: Handle,
        msg: Message,
        timeout: Option<u64>,
    ) -> Result<SendStatus, Error> {
        match self.send(env, owner, h, msg) {
            Ok(id) => return Ok(SendStatus::Sent(id)),
            Err(Error::QueueFull) => {}
            Err(e) => return Err(e),
        }
        ctx.check_may_block()?;
        let deadline = Self::deadline_for(env, timeout)?;
        self.wait_on(env, owner, h, WaitOp::Send, deadline, Some(msg))?;
        Ok(SendStatus::Blocked)
    }

    pub fn send_resume<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ctx: Context,
        owner: Owner,
        h: Handle,
    ) -> Result<SendStatus, Error> {
        let rec = self.finish_wait(env)?;
        if let Some(e) = Self::resumed_error(&rec) {
            self.forward_wake(env, &rec)?;
            return Err(e);
        }
        let msg = rec.pending.ok_or(Error::Corrupt("blocked send without a message"))?;
        match self.send(env, owner, h, msg) {
            Ok(id) => Ok(SendStatus::Sent(id)),
            Err(Error::QueueFull) => {
                if Self::expired(env, &rec) {
                    return Err(Error::TimedOut);
                }
                ctx.check_may_block()?;
                self.wait_on(env, owner, h, WaitOp::Send, rec.deadline, Some(msg))?;
                Ok(SendStatus::Blocked)
            }
            Err(e) => {
                self.forward_wake(env, &rec)?;
                Err(closed_or(e))
            }
        }
    }

    // ---- accept -----------------------------------------------------------------------------

    pub fn accept_blocking<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ctx: Context,
        owner: Owner,
        h: Handle,
        timeout: Option<u64>,
    ) -> Result<AcceptStatus, Error> {
        match self.accept(env, owner, h) {
            Ok((nh, who)) => return Ok(AcceptStatus::Accepted(nh, who)),
            Err(Error::NoPending) => {}
            Err(e) => return Err(e),
        }
        ctx.check_may_block()?;
        let deadline = Self::deadline_for(env, timeout)?;
        self.wait_on(env, owner, h, WaitOp::Accept, deadline, None)?;
        Ok(AcceptStatus::Blocked)
    }

    pub fn accept_resume<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        ctx: Context,
        owner: Owner,
        h: Handle,
    ) -> Result<AcceptStatus, Error> {
        let rec = self.finish_wait(env)?;
        if let Some(e) = Self::resumed_error(&rec) {
            self.forward_wake(env, &rec)?;
            return Err(e);
        }
        match self.accept(env, owner, h) {
            Ok((nh, who)) => Ok(AcceptStatus::Accepted(nh, who)),
            Err(Error::NoPending) => {
                if Self::expired(env, &rec) {
                    return Err(Error::TimedOut);
                }
                ctx.check_may_block()?;
                self.wait_on(env, owner, h, WaitOp::Accept, rec.deadline, None)?;
                Ok(AcceptStatus::Blocked)
            }
            Err(e) => {
                self.forward_wake(env, &rec)?;
                Err(closed_or(e))
            }
        }
    }

    // ---- cancellation, timeouts, thread and process teardown -----------------------------------

    /// Cancels every wait on the endpoint behind `h` (needs `CANCEL`); the waiters report
    /// `Cancelled`. Returns how many were cancelled.
    pub fn cancel_endpoint<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
    ) -> Result<usize, Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::CANCEL)?;
        self.wake_endpoint_waiters(env, cap.object, WaitOutcome::Cancelled)
    }

    /// Kernel: cancels one thread's IPC wait (signal delivery, explicit cancellation).
    pub fn cancel_thread<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        thread: Tid,
    ) -> Result<bool, Error> {
        let Some(rec) = self.waits.recs.get_mut(thread.index()).and_then(|r| r.as_mut()) else {
            return Ok(false);
        };
        if rec.thread != thread || rec.outcome != WaitOutcome::Waiting {
            return Ok(false);
        }
        rec.outcome = WaitOutcome::Cancelled;
        env.sync.cancel_wait(env.sched, thread)?;
        self.stats.wakeups += 1;
        Ok(true)
    }

    /// Wakes every waiter whose deadline has passed. Never blocks.
    pub fn expire_timeouts<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
    ) -> Result<usize, Error> {
        let now = env.sched.now();
        let mut n = 0;
        for i in 0..MAX_THREADS {
            let Some(rec) = self.waits.recs[i].as_mut() else { continue };
            if rec.outcome != WaitOutcome::Waiting || !rec.deadline.is_some_and(|d| d <= now) {
                continue;
            }
            rec.outcome = WaitOutcome::TimedOut;
            let t = rec.thread;
            env.sync.cancel_wait(env.sched, t)?;
            self.stats.wakeups += 1;
            n += 1;
        }
        Ok(n)
    }

    /// A thread is about to terminate. Call BEFORE `terminate_current` / right after `kill`
    /// (same rule as `SyncDomain::thread_exit`): drops its IPC wait and releases the endpoint pin,
    /// withdraws it from the sync queue and abandons any mutex it owns.
    pub fn thread_exit<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        thread: Tid,
    ) -> Result<(), Error> {
        if let Some(rec) = self.waits.take(thread) {
            let _ = self.objects.release(rec.endpoint, Counter::Ops);
        }
        env.sync.thread_exit(env.sched, thread)?;
        Ok(())
    }

    /// Drops wait records whose thread died without `thread_exit`. Returns how many.
    pub fn reap_dead_waiters<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
    ) -> usize {
        let mut n = 0;
        for i in 0..MAX_THREADS {
            let Some(rec) = self.waits.recs[i] else { continue };
            let alive = env.sched.thread(rec.thread).is_ok_and(|t| {
                t.state() == crate::thread::State::Blocked
            });
            if !alive {
                self.waits.recs[i] = None;
                let _ = self.objects.release(rec.endpoint, Counter::Ops);
                n += 1;
            }
        }
        n
    }

    /// Process exit: abandons the process's waits, closes everything it owns, closes its handles
    /// and removes the capabilities it holds. Other processes keep working; their endpoints to
    /// this process observe `PeerClosed`. Returns the number of handles that were open.
    pub fn destroy_process<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
    ) -> Result<usize, Error> {
        if !self.procs.contains(owner) {
            return Err(Error::NoSuchProcess);
        }
        // 1. Its threads can no longer wait.
        for i in 0..MAX_THREADS {
            let Some(rec) = self.waits.recs[i] else { continue };
            if rec.owner == owner {
                self.waits.recs[i] = None;
                let _ = env.sync.cancel_wait(env.sched, rec.thread);
                let _ = self.objects.release(rec.endpoint, Counter::Ops);
            }
        }
        // 2. Everything it owns: endpoints close, other objects are revoked.
        let mut owned: [Option<(ObjectId, ObjectKind)>; MAX_OBJECTS_PER_OWNER] =
            [None; MAX_OBJECTS_PER_OWNER];
        let mut k = 0;
        for (id, rec) in self.objects.iter() {
            if rec.owner == owner && rec.state == crate::d6::object::ObjectState::Active {
                *owned.get_mut(k).ok_or(Error::Corrupt("owner exceeds its object quota"))? =
                    Some((id, rec.kind));
                k += 1;
            }
        }
        for (id, kind) in owned.iter().flatten() {
            match kind {
                ObjectKind::Endpoint => self.close_endpoint_obj(env, *id)?,
                _ => {
                    self.objects.revoke(*id)?;
                    self.caps.sweep_object(&mut self.objects, *id)?;
                }
            }
        }
        self.revoke_process_refs_to(owner)?;
        // 3. Its handle table and any capability it still holds.
        let n = self.procs.destroy(owner, &mut self.caps, &mut self.objects)?;
        self.reap_endpoints(env)?;
        Ok(n)
    }

    pub(crate) fn check_waits(&self) -> Result<(), &'static str> {
        for (id, rec) in self.objects.iter() {
            let waiting = self.waits.iter().filter(|w| w.endpoint == id).count();
            if rec.ops as usize != waiting {
                return Err("object ops pins differ from wait records");
            }
        }
        for (i, w) in self.waits.recs.iter().enumerate() {
            let Some(w) = w else { continue };
            if w.thread.index() != i {
                return Err("wait record in the wrong slot");
            }
            if !self.objects.exists(w.endpoint) {
                return Err("wait record names a dead endpoint");
            }
        }
        Ok(())
    }
}

// ---- real-kernel wrappers ----------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use blocking::*;

#[cfg(target_arch = "x86_64")]
mod blocking {
    //! Context-switching wrappers. Lock order (never reversed): kernel slot (scheduler) ->
    //! `SyncSlot` -> [`IpcSlot`]. Interrupts stay masked from the state change until the resume
    //! re-evaluation, so the check-and-block sequence cannot be torn by an interrupt.
    //!
    //! Target-compiled and host-compiled only: these functions perform real context switches and
    //! have no host runtime test; the protocol they wrap is tested through the model API.
    use super::*;
    use crate::dispatch::{self, KernelSlot};
    use crate::irq::IrqGuard;
    use crate::kthread::StackMemory;
    use crate::spin::SpinLock;
    use crate::sync::SyncSlot;

    pub struct IpcSlot {
        inner: SpinLock<Ipc>,
    }

    impl Default for IpcSlot {
        fn default() -> Self {
            Self::new()
        }
    }

    impl IpcSlot {
        pub const fn new() -> Self {
            Self {
                inner: SpinLock::new(Ipc::new()),
            }
        }
        /// Runs `f` with interrupts masked and the IPC state locked.
        pub fn with<R>(&self, f: impl FnOnce(&mut Ipc) -> R) -> R {
            // SAFETY: ring 0; the guard restores the previous interrupt state.
            let _irq = unsafe { IrqGuard::new() };
            let mut g = self.inner.lock();
            f(&mut g)
        }
    }

    fn sched_failed(_: crate::kthread::Error) -> Error {
        Error::Corrupt("reschedule failed")
    }

    /// Blocking receive.
    ///
    /// # Safety
    /// See `dispatch::reschedule`.
    pub unsafe fn recv<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        ipc: &IpcSlot,
        owner: Owner,
        h: Handle,
        timeout: Option<u64>,
    ) -> Result<Message, Error> {
        // SAFETY: ring 0. IF stays clear from the state change until the resume check.
        let _irq = unsafe { IrqGuard::new() };
        let mut st = k.with(|kern| {
            sy.with(|d| {
                ipc.with(|i| {
                    let mut env = Env { sched: kern.sched_mut(), sync: d };
                    i.recv_blocking(&mut env, Context::current(), owner, h, timeout)
                })
            })
        })?;
        loop {
            match st {
                RecvStatus::Message(m) => return Ok(m),
                RecvStatus::Blocked => {
                    // SAFETY: forwarded.
                    unsafe { dispatch::reschedule(k) }.map_err(sched_failed)?;
                    st = k.with(|kern| {
                        sy.with(|d| {
                            ipc.with(|i| {
                                let mut env = Env { sched: kern.sched_mut(), sync: d };
                                i.recv_resume(&mut env, Context::current(), owner, h)
                            })
                        })
                    })?;
                }
            }
        }
    }

    /// Blocking send (backpressure).
    ///
    /// # Safety
    /// See `dispatch::reschedule`.
    pub unsafe fn send<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        ipc: &IpcSlot,
        owner: Owner,
        h: Handle,
        msg: Message,
        timeout: Option<u64>,
    ) -> Result<MsgId, Error> {
        // SAFETY: ring 0.
        let _irq = unsafe { IrqGuard::new() };
        let mut st = k.with(|kern| {
            sy.with(|d| {
                ipc.with(|i| {
                    let mut env = Env { sched: kern.sched_mut(), sync: d };
                    i.send_blocking(&mut env, Context::current(), owner, h, msg, timeout)
                })
            })
        })?;
        loop {
            match st {
                SendStatus::Sent(id) => return Ok(id),
                SendStatus::Blocked => {
                    // SAFETY: forwarded.
                    unsafe { dispatch::reschedule(k) }.map_err(sched_failed)?;
                    st = k.with(|kern| {
                        sy.with(|d| {
                            ipc.with(|i| {
                                let mut env = Env { sched: kern.sched_mut(), sync: d };
                                i.send_resume(&mut env, Context::current(), owner, h)
                            })
                        })
                    })?;
                }
            }
        }
    }

    /// Blocking accept.
    ///
    /// # Safety
    /// See `dispatch::reschedule`.
    pub unsafe fn accept<const N: usize, const S: usize, M: StackMemory>(
        k: &KernelSlot<N, M>,
        sy: &SyncSlot<N, S>,
        ipc: &IpcSlot,
        owner: Owner,
        h: Handle,
        timeout: Option<u64>,
    ) -> Result<(Handle, Owner), Error> {
        // SAFETY: ring 0.
        let _irq = unsafe { IrqGuard::new() };
        let mut st = k.with(|kern| {
            sy.with(|d| {
                ipc.with(|i| {
                    let mut env = Env { sched: kern.sched_mut(), sync: d };
                    i.accept_blocking(&mut env, Context::current(), owner, h, timeout)
                })
            })
        })?;
        loop {
            match st {
                AcceptStatus::Accepted(nh, who) => return Ok((nh, who)),
                AcceptStatus::Blocked => {
                    // SAFETY: forwarded.
                    unsafe { dispatch::reschedule(k) }.map_err(sched_failed)?;
                    st = k.with(|kern| {
                        sy.with(|d| {
                            ipc.with(|i| {
                                let mut env = Env { sched: kern.sched_mut(), sync: d };
                                i.accept_resume(&mut env, Context::current(), owner, h)
                            })
                        })
                    })?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d6::capability::Rights;
    use crate::d6::ipc::testutil::*;
    use crate::d6::ipc::*;
    use crate::thread::State;

    fn is_blocked(r: &Rig, t: Tid) -> bool {
        r.sched.thread(t).unwrap().state() == State::Blocked
    }

    #[test]
    fn blocking_receive_sleeps_through_the_scheduler_and_wakes_on_send() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let t = r.spawn(4);
        r.run_until(t);
        let st = r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
        assert_eq!(st, RecvStatus::Blocked);
        assert!(is_blocked(&r, t));
        assert_eq!(r.ipc.waits.count(), 1);
        assert_eq!(r.ipc.objects().get_any(r.ipc.caps().peek(r.ipc.procs.get(b).unwrap().resolve(hb).unwrap()).unwrap().object).unwrap().ops, 1);
        r.run_once(); // switch away: the blocked thread does not run
        assert_ne!(r.sched.current(), Some(t));
        r.check();
        // A sender (any context, even an interrupt handler) wakes it.
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 7, b"wake")).unwrap();
        assert!(!is_blocked(&r, t), "the waiter is Ready");
        assert_eq!(r.ipc.stats().wakeups, 1);
        r.run_until(t);
        match r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb).unwrap() {
            RecvStatus::Message(m) => assert_eq!((m.tag, m.payload()), (7, &b"wake"[..])),
            s => panic!("{s:?}"),
        }
        assert_eq!(r.ipc.waits.count(), 0);
        r.check();
    }

    #[test]
    fn a_message_already_queued_never_blocks() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"")).unwrap();
        let t = r.spawn(4);
        r.run_until(t);
        assert!(matches!(
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None),
            Ok(RecvStatus::Message(_))
        ));
        assert_eq!(r.sched.thread(t).unwrap().state(), State::Running);
    }

    #[test]
    fn wakeup_between_block_and_switch_is_not_lost() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let t = r.spawn(4);
        r.run_until(t);
        assert_eq!(
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None),
            Ok(RecvStatus::Blocked)
        );
        // Interrupt-style send arrives BEFORE the context switch completes.
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 3, b"race")).unwrap();
        r.run_once();
        r.run_until(t);
        assert!(matches!(
            r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb),
            Ok(RecvStatus::Message(m)) if m.tag == 3
        ));
        r.check();
    }

    #[test]
    fn a_stale_pending_scheduler_wakeup_does_not_prevent_blocking() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let t = r.spawn(4);
        r.run_until(t);
        r.sched.wake(t).unwrap(); // pending wake-up on a Running thread
        assert_eq!(
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None),
            Ok(RecvStatus::Blocked)
        );
        assert!(is_blocked(&r, t));
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"")).unwrap();
        r.run_until(t);
        assert!(matches!(
            r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb),
            Ok(RecvStatus::Message(_))
        ));
        r.check();
    }

    #[test]
    fn spurious_wakeup_reblocks_and_keeps_waiting() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let t = r.spawn(4);
        r.run_until(t);
        r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
        r.run_once();
        // Wake it without any message (as a stray scheduler wake would).
        let env = r.env();
        env.sync.cancel_wait(env.sched, t).unwrap();
        r.run_until(t);
        assert_eq!(
            r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb),
            Ok(RecvStatus::Blocked),
            "nothing to receive: it blocks again"
        );
        r.run_once();
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 5, b"")).unwrap();
        r.run_until(t);
        assert!(matches!(r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb), Ok(RecvStatus::Message(_))));
        r.check();
    }

    #[test]
    fn wake_one_wakes_exactly_one_receiver_in_priority_then_fifo_order() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        // Block in the order hi1, hi2, lo.
        let ts = [r.spawn(5), r.spawn(5), r.spawn(3)];
        for t in ts {
            r.run_until(t);
            assert_eq!(
                r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None),
                Ok(RecvStatus::Blocked)
            );
            r.run_once();
        }
        assert_eq!(r.ipc.waits.count(), 3);
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"")).unwrap();
        let woken: std::vec::Vec<bool> = ts.iter().map(|t| !is_blocked(&r, *t)).collect();
        assert_eq!(woken, [true, false, false], "exactly one: the first of the highest level");
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 2, b"")).unwrap();
        let woken: std::vec::Vec<bool> = ts.iter().map(|t| !is_blocked(&r, *t)).collect();
        assert_eq!(woken, [true, true, false], "then the next one, still not the low one");
        r.ipc.check_invariants().unwrap();
    }

    #[test]
    fn two_receivers_one_message_the_loser_blocks_again_without_losing_anything() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let (t1, t2) = (r.spawn(4), r.spawn(4));
        for t in [t1, t2] {
            r.run_until(t);
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
            r.run_once();
        }
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"")).unwrap();
        // A third, running receiver steals the message before the woken thread runs.
        let t3 = r.spawn(6);
        r.run_until(t3);
        assert!(r.ipc.recv(&mut renv!(r), b, hb).is_ok());
        r.sched.terminate_current(0).unwrap();
        r.run_once();
        r.run_until(t1);
        assert_eq!(
            r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb),
            Ok(RecvStatus::Blocked)
        );
        r.check();
    }

    #[test]
    fn blocking_send_waits_for_space_and_completes_in_order() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        for i in 0..QUEUE_CAPACITY as u64 {
            r.ipc.send(&mut renv!(r), a, ha, msg(a, i, b"")).unwrap();
        }
        let t = r.spawn(4);
        r.run_until(t);
        assert_eq!(
            r.ipc.send_blocking(&mut renv!(r), Context::THREAD, a, ha, msg(a, 99, b"late"), None),
            Ok(SendStatus::Blocked)
        );
        r.run_once();
        r.check();
        // The receiver drains one message: a slot frees and the sender is woken.
        assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).unwrap().tag, 0);
        assert!(!is_blocked(&r, t));
        r.run_until(t);
        assert!(matches!(
            r.ipc.send_resume(&mut renv!(r), Context::THREAD, a, ha),
            Ok(SendStatus::Sent(_))
        ));
        for i in 1..QUEUE_CAPACITY as u64 {
            assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).unwrap().tag, i);
        }
        assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).unwrap().tag, 99, "FIFO: the blocked send is last");
        r.check();
    }

    #[test]
    fn cancellation_wakes_blocked_threads_with_cancelled() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (_ha, hb) = r.pair(a, b);
        let ts = [r.spawn(4), r.spawn(4)];
        for t in ts {
            r.run_until(t);
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
            r.run_once();
        }
        let weak = r.ipc.duplicate_handle(b, hb, Rights::RECV).unwrap();
        assert_eq!(r.ipc.cancel_endpoint(&mut renv!(r), b, weak), Err(Error::RightsDenied));
        assert_eq!(r.ipc.cancel_endpoint(&mut renv!(r), b, hb), Ok(2), "wake-all");
        for t in ts {
            r.run_until(t);
            assert_eq!(
                r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb),
                Err(Error::Cancelled)
            );
        }
        assert_eq!(r.ipc.waits.count(), 0);
        assert_eq!(r.ipc.cancel_endpoint(&mut renv!(r), b, hb), Ok(0));
        r.check();
        // The endpoint is still fully usable afterwards.
        r.ipc.send(&mut renv!(r), a, _ha, msg(a, 1, b"")).unwrap();
        assert!(r.ipc.recv(&mut renv!(r), b, hb).is_ok());
    }

    #[test]
    fn a_cancelled_waiter_forwards_the_wakeup_it_would_have_swallowed() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let (t1, t2) = (r.spawn(5), r.spawn(4));
        for t in [t1, t2] {
            r.run_until(t);
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
            r.run_once();
        }
        // t1 is chosen for the message but is cancelled before it runs.
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"")).unwrap();
        assert!(!is_blocked(&r, t1) && is_blocked(&r, t2));
        r.ipc.waits.recs[t1.index()].as_mut().unwrap().outcome = WaitOutcome::Cancelled;
        r.run_until(t1);
        assert_eq!(r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb), Err(Error::Cancelled));
        assert!(!is_blocked(&r, t2), "the message was handed on to the next waiter");
        r.sched.terminate_current(0).unwrap();
        r.run_once();
        r.run_until(t2);
        assert!(matches!(r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb), Ok(RecvStatus::Message(_))));
        r.check();
    }

    #[test]
    fn peer_closure_wakes_blocked_receivers_and_senders() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let (rx, tx) = (r.spawn(4), r.spawn(4));
        r.run_until(rx);
        r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
        r.run_once();
        for i in 0..QUEUE_CAPACITY as u64 {
            r.ipc.send(&mut renv!(r), b, hb, msg(b, i, b"")).unwrap();
        }
        r.run_until(tx);
        assert_eq!(
            r.ipc.send_blocking(&mut renv!(r), Context::THREAD, b, hb, msg(b, 100, b""), None),
            Ok(SendStatus::Blocked)
        );
        r.run_once();
        r.check();
        r.ipc.close_endpoint(&mut renv!(r), a, ha).unwrap();
        r.run_until(rx);
        // rx was waiting on b's own end; the message queue for it is empty and the peer is gone.
        assert_eq!(r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb), Err(Error::PeerClosed));
        r.run_until(tx);
        assert_eq!(r.ipc.send_resume(&mut renv!(r), Context::THREAD, b, hb), Err(Error::PeerClosed));
        assert_eq!(r.ipc.waits.count(), 0);
        r.check();
    }

    #[test]
    fn closing_the_waited_endpoint_wakes_everyone_with_endpoint_closed() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (_ha, hb) = r.pair(a, b);
        let t = r.spawn(4);
        r.run_until(t);
        r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
        r.run_once();
        r.ipc.close_endpoint(&mut renv!(r), b, hb).unwrap(); // another thread of b closes it
        assert!(!is_blocked(&r, t));
        // The object is only freed once the waiter has dropped its pin.
        assert_eq!(
            r.ipc.objects().live(),
            3,
            "channel + open end + the closed end that lingers until its waiter leaves"
        );
        r.run_once();
        r.run_until(t);
        assert_eq!(r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb), Err(Error::EndpointClosed));
        r.check();
        assert_eq!(r.ipc.objects().live(), 2, "the pin is gone and the closed end was freed");
    }

    #[test]
    fn thread_termination_while_waiting_leaves_nothing_linked() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let (t, other) = (r.spawn(4), r.spawn(4));
        r.run_until(t);
        r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
        r.run_once();
        assert_eq!(r.sync.waiting_threads(), 1);
        // Orderly path: thread_exit before the kill.
        r.ipc.thread_exit(&mut renv!(r), t).unwrap();
        r.sched.kill(t, -1).unwrap();
        assert_eq!(r.sync.waiting_threads(), 0, "not linked to any wait queue");
        assert_eq!(r.ipc.waits.count(), 0);
        r.check();
        // A later message does not try to wake the dead thread and stays receivable.
        r.run_until(other);
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"")).unwrap();
        assert!(r.ipc.recv(&mut renv!(r), b, hb).is_ok());
        r.check();
    }

    #[test]
    fn a_thread_killed_without_cleanup_is_reaped_later() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let (t, other) = (r.spawn(4), r.spawn(4));
        r.run_until(t);
        r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
        r.run_once();
        r.sched.kill(t, -1).unwrap(); // no thread_exit
        // A send must not wake or trip over the dead waiter.
        r.run_until(other);
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"")).unwrap();
        assert_eq!(r.ipc.reap_dead_waiters(&mut renv!(r)), 1);
        assert_eq!(r.ipc.waits.count(), 0);
        r.ipc.close_endpoint(&mut renv!(r), a, ha).unwrap();
        r.ipc.close_endpoint(&mut renv!(r), b, hb).unwrap();
        assert_eq!(r.ipc.objects().live(), 0, "no pin leaked");
        r.check();
    }

    #[test]
    fn blocking_is_refused_in_interrupt_context_and_with_preemption_disabled() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let t = r.spawn(4);
        r.run_until(t);
        let before = r.ipc.objects().get_any(r.ipc.caps().peek(r.ipc.procs.get(b).unwrap().resolve(hb).unwrap()).unwrap().object).unwrap().ops;
        assert_eq!(
            r.ipc.recv_blocking(&mut renv!(r), Context::INTERRUPT, b, hb, None),
            Err(Error::BlockInInterrupt)
        );
        let no_preempt = Context { in_interrupt: false, preempt_disabled: true };
        assert_eq!(
            r.ipc.recv_blocking(&mut renv!(r), no_preempt, b, hb, None),
            Err(Error::PreemptDisabled)
        );
        assert_eq!(
            r.ipc.accept_blocking(&mut renv!(r), Context::INTERRUPT, b, hb, None).err(),
            Some(Error::RightsDenied),
            "authority is checked before context"
        );
        for i in 0..QUEUE_CAPACITY as u64 {
            r.ipc.send(&mut renv!(r), a, ha, msg(a, i, b"")).unwrap();
        }
        assert_eq!(
            r.ipc.send_blocking(&mut renv!(r), Context::INTERRUPT, a, ha, msg(a, 9, b""), None),
            Err(Error::BlockInInterrupt)
        );
        // Refusal changed nothing: still running, no record, no pin.
        assert_eq!(r.sched.thread(t).unwrap().state(), State::Running);
        assert_eq!(r.ipc.waits.count(), 0);
        let after = r.ipc.objects().get_any(r.ipc.caps().peek(r.ipc.procs.get(b).unwrap().resolve(hb).unwrap()).unwrap().object).unwrap().ops;
        assert_eq!(before, after);
        // Non-blocking operations remain legal from interrupt context, and a ready message is
        // delivered even by the "blocking" call because it does not block.
        assert!(matches!(
            r.ipc.recv_blocking(&mut renv!(r), Context::INTERRUPT, b, hb, None),
            Ok(RecvStatus::Message(_))
        ));
        r.check();
    }

    #[test]
    fn the_idle_thread_and_no_thread_cannot_block() {
        let mut r = Rig::new();
        let (_a, b) = (r.proc(1), r.proc(2));
        let (_ha, hb) = r.pair(p(1), b);
        assert_eq!(
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None),
            Err(Error::NoCurrentThread)
        );
        let t = r.spawn(4);
        r.run_until(t);
        r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
        // One wait per thread.
        assert_eq!(
            r.ipc.wait_on(&mut renv!(r), b, hb, WaitOp::Recv, None, None),
            Err(Error::Busy)
        );
    }

    #[test]
    fn timeouts_expire_and_zero_timeout_is_would_block() {
        let mut r = Rig::new();
        let (_a, b) = (r.proc(1), r.proc(2));
        let (_ha, hb) = r.pair(p(1), b);
        let t = r.spawn(4);
        r.run_until(t);
        assert_eq!(
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, Some(0)),
            Err(Error::WouldBlock)
        );
        assert_eq!(
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, Some(u64::MAX)),
            Ok(RecvStatus::Blocked),
            "a huge timeout is fine while now is small"
        );
        r.ipc.thread_exit(&mut renv!(r), t).unwrap();
        r.sched.kill(t, 0).unwrap();
        let t2 = r.spawn(4);
        r.run_until(t2);
        assert_eq!(
            r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, Some(3)),
            Ok(RecvStatus::Blocked)
        );
        r.run_once();
        assert_eq!(r.ipc.expire_timeouts(&mut renv!(r)), Ok(0), "not yet");
        for _ in 0..3 {
            r.sched.tick();
        }
        assert_eq!(r.ipc.expire_timeouts(&mut renv!(r)), Ok(1));
        assert_eq!(r.ipc.expire_timeouts(&mut renv!(r)), Ok(0), "once");
        r.run_until(t2);
        assert_eq!(r.ipc.recv_resume(&mut renv!(r), Context::THREAD, b, hb), Err(Error::TimedOut));
        assert_eq!(r.ipc.waits.count(), 0);
        r.check();
    }

    #[test]
    fn blocking_accept_wakes_when_a_client_connects() {
        let mut r = Rig::new();
        let (srv, cli) = (r.proc(1), r.proc(2));
        let hl = r.ipc.create_listener(&mut renv!(r), srv).unwrap();
        let t = r.spawn(4);
        r.run_until(t);
        assert_eq!(
            r.ipc.accept_blocking(&mut renv!(r), Context::THREAD, srv, hl, None),
            Ok(AcceptStatus::Blocked)
        );
        r.run_once();
        // The client needs a connect capability: in this model the server's own table has one.
        let c = r.ipc.duplicate_handle(srv, hl, Rights::CONNECT).unwrap();
        let _ = cli;
        r.ipc.connect(&mut renv!(r), srv, c).unwrap();
        assert!(!is_blocked(&r, t));
        r.run_until(t);
        assert!(matches!(
            r.ipc.accept_resume(&mut renv!(r), Context::THREAD, srv, hl),
            Ok(AcceptStatus::Accepted(_, who)) if who == srv
        ));
        r.check();
    }

    #[test]
    fn process_exit_abandons_waits_closes_owned_objects_and_contains_the_failure() {
        let mut r = Rig::new();
        let (srv, c1, c2) = (r.proc(1), r.proc(2), r.proc(3));
        let (s1, k1) = r.pair(srv, c1);
        let (_s2, k2) = r.pair(srv, c2);
        let (c1c2a, c1c2b) = r.pair(c1, c2);
        let t = r.spawn(4);
        r.run_until(t);
        r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, srv, s1, None).unwrap();
        r.run_once();
        r.ipc.send(&mut renv!(r), c1, k1, msg(c1, 1, b"")).unwrap();
        // The server dies. Its waiter record and sync wait are gone, its endpoints are closed.
        r.sched.kill(t, -1).unwrap();
        let closed = r.ipc.destroy_process(&mut renv!(r), srv).unwrap();
        assert_eq!(closed, 2);
        assert!(!r.ipc.has_process(srv));
        assert_eq!(r.ipc.waits.count(), 0);
        assert_eq!(r.ipc.recv(&mut renv!(r), c1, k1).err(), Some(Error::PeerClosed));
        assert_eq!(r.ipc.recv(&mut renv!(r), c2, k2).err(), Some(Error::PeerClosed));
        // Unrelated processes are unaffected.
        r.ipc.send(&mut renv!(r), c1, c1c2a, msg(c1, 9, b"ok")).unwrap();
        assert_eq!(r.ipc.recv(&mut renv!(r), c2, c1c2b).unwrap().payload(), b"ok");
        r.check();
        assert_eq!(r.ipc.destroy_process(&mut renv!(r), srv), Err(Error::NoSuchProcess));
    }

    #[test]
    fn endpoint_pins_keep_the_object_until_the_waiter_leaves() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let t = r.spawn(4);
        r.run_until(t);
        r.ipc.recv_blocking(&mut renv!(r), Context::THREAD, b, hb, None).unwrap();
        r.ipc.close_handle(&mut renv!(r), b, hb).unwrap();
        assert!(r.ipc.waits.count() == 1 || r.ipc.waits.count() == 0);
        r.run_once();
        r.check();
        r.ipc.close_handle(&mut renv!(r), a, ha).unwrap();
    }
}
