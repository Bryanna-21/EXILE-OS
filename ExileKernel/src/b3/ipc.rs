//! Capability-protected IPC domain (Mission B3). SPDX-License-Identifier: Apache-2.0
//!
//! [`IpcDomain<N, O, C, Q>`] ties the B3 pieces to the existing scheduler: an object table of up
//! to `O` endpoints/notifications, a capability table of `C` slots, per-object bounded queues
//! of `Q` messages, and one wait record per thread slot (`N`) so waiter storage cannot
//! overflow. Operations take `&mut Scheduler<N>` explicitly and use only
//! `block_current` / `wake`, exactly like `d5::sync`; the caller runs the scheduler after a
//! `Blocked` outcome and then calls [`IpcDomain::resume`].
//!
//! # Semantics
//! * **Authority**: every operation names its object through a handle that must belong to the
//!   calling `owner` (a process id) and carry the needed right; there is no ambient access.
//! * **Endpoints**: bounded FIFO. `send` on a full queue blocks (or `WouldBlock` when
//!   non-blocking) -- messages are never dropped. If a receiver is already waiting the message
//!   is handed to it directly, otherwise it is queued. When `recv` frees a slot, the best
//!   blocked sender's message is moved into the queue in the same step (sender hand-off), so
//!   queue order equals send order.
//! * **Notifications**: `signal` ORs bits into a word and never blocks (IRQ-safe, see
//!   [`IpcDomain::kernel_signal`]); `wait` takes the whole word or blocks for the next signal.
//! * **Capability transfer**: a message may carry one capability the sender holds with `GRANT`.
//!   It is derived into limbo at send time and rebound to the receiver on delivery. If it was
//!   revoked while in flight, or the receiver's table quota is full, the message is still
//!   delivered with `cap_lost` set -- the data is never silently dropped and nothing is
//!   granted beyond the receiver's quota.
//! * **Liveness**: an object lives while some *real owner* holds a capability to it (limbo
//!   copies do not count, so a capability sent through its own endpoint cannot keep it alive
//!   forever). When the last one goes, the object is destroyed, queued messages are discarded
//!   with their limbo capabilities, and every thread blocked on it wakes with `Closed`.
//!   Revoking or closing the capability a blocked thread used wakes it with `Revoked`.
//! * **Lost capabilities** are counted in `Stats::caps_lost`, whatever the reason.
//! * **Waiter choice**: highest effective priority first, FIFO within a level.
//! * **Teardown**: [`IpcDomain::thread_exit`] and [`IpcDomain::owner_exit`] leave no wait
//!   record, limbo capability or object behind.
//!
//! # Not implemented
//! No timeouts (the scheduler has no timed event wait; use the non-blocking forms), no
//! cross-CPU operation, no priority inheritance through IPC, no reply capabilities.
#![allow(dead_code)]

use super::cap_rights::{Kind, Rights};
use super::cap_space::{CapError, CapRef, CapTable, LIMBO};
use super::ipc_endpoint::Endpoint;
use super::ipc_message::{Delivery, Message, MsgError, UserMessage};
use super::ipc_notify::Notify;
use super::kobject::{ObjError, ObjId, ObjTable};
use crate::sched::{BlockOutcome, Scheduler};
use crate::thread::{self, BlockReason, Handle, State};

/// Event ids used for `BlockReason::Event`; distinct from `d5::sync` ids (which are bare slot
/// numbers) so a thread blocked in one domain is never mistaken for a waiter in the other.
pub const IPC_EVENT_BASE: u32 = 0x4950_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// Malformed, stale, or owned by someone else (all indistinguishable to the caller).
    BadHandle,
    /// The capability lacks a required right.
    Perm,
    NoRights,
    /// Capability table full.
    NoSlots,
    /// The owner is at its capability quota.
    Quota,
    BadOwner,
    /// Object table full.
    NoObjects,
    /// The operation would block and blocking was not requested.
    WouldBlock,
    BadArg,
    /// The object was destroyed while the thread waited.
    Closed,
    /// The capability the thread waited with was revoked or closed while it waited.
    Revoked,
    /// The thread was woken without an IPC event.
    Interrupted,
    WrongKind,
    NoCurrent,
    IsIdle,
    Sched(thread::Error),
    Corrupt(&'static str),
}

impl From<CapError> for IpcError {
    fn from(e: CapError) -> Self {
        match e {
            CapError::BadHandle => IpcError::BadHandle,
            CapError::Perm => IpcError::Perm,
            CapError::NoRights => IpcError::NoRights,
            CapError::NoSlots => IpcError::NoSlots,
            CapError::Quota => IpcError::Quota,
            CapError::BadOwner => IpcError::BadOwner,
        }
    }
}

impl From<ObjError> for IpcError {
    fn from(e: ObjError) -> Self {
        match e {
            ObjError::Stale => IpcError::BadHandle,
            ObjError::NoObjects => IpcError::NoObjects,
        }
    }
}

impl From<thread::Error> for IpcError {
    fn from(e: thread::Error) -> Self {
        IpcError::Sched(e)
    }
}

impl From<MsgError> for IpcError {
    fn from(_: MsgError) -> Self {
        IpcError::BadArg
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    Sent,
    /// The caller is Blocked on a full queue; run the scheduler, then `resume`.
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvOutcome {
    Received(Delivery),
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Signalled(u64),
    Blocked,
}

/// What a blocked thread's operation ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    Sent,
    Received(Delivery),
    Signalled(u64),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub sent: u64,
    pub received: u64,
    pub handoffs: u64,
    pub signals: u64,
    pub caps_lost: u64,
    pub waits_failed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitKind {
    Send,
    Recv,
    Notify,
}

#[derive(Clone, Copy)]
struct Wait {
    kind: WaitKind,
    handle: Handle,
    cap: CapRef,
    obj: ObjId,
    owner: u64,
    seq: u64,
    /// The message of a blocked sender (its capability, if any, is in limbo).
    msg: Option<Message>,
}

#[derive(Clone, Copy)]
enum Done {
    Sent,
    Received(Delivery),
    Signalled(u64),
    Failed(IpcError),
}

pub struct IpcDomain<const N: usize, const O: usize, const C: usize, const Q: usize> {
    objs: ObjTable<O>,
    caps: CapTable<C>,
    eps: [Endpoint<Q>; O],
    nfs: [Notify; O],
    waits: [Option<Wait>; N],
    done: [Option<(Handle, Done)>; N],
    seq: u64,
    stats: Stats,
}

impl<const N: usize, const O: usize, const C: usize, const Q: usize> Default
    for IpcDomain<N, O, C, Q>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, const O: usize, const C: usize, const Q: usize> IpcDomain<N, O, C, Q> {
    pub fn new() -> Self {
        Self {
            objs: ObjTable::new(),
            caps: CapTable::new(),
            eps: core::array::from_fn(|_| Endpoint::new()),
            nfs: [Notify::new(); O],
            waits: [None; N],
            done: [None; N],
            seq: 0,
            stats: Stats::default(),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn caps(&self) -> &CapTable<C> {
        &self.caps
    }

    pub fn set_owner_cap_limit(&mut self, limit: usize) -> Result<(), IpcError> {
        Ok(self.caps.set_owner_limit(limit)?)
    }

    pub fn live_objects(&self) -> usize {
        self.objs.live_count()
    }

    pub fn waiting_threads(&self) -> usize {
        self.waits.iter().flatten().count()
    }

    /// Messages queued on the endpoint `owner`'s handle names.
    pub fn queued(&self, owner: u64, handle: u64) -> Result<usize, IpcError> {
        let r = self.caps.lookup(owner, handle, Rights::NONE)?;
        if r.kind != Kind::Endpoint {
            return Err(IpcError::WrongKind);
        }
        Ok(self.eps[r.obj.index()].len())
    }

    // ---- creation and capability management ------------------------------------------------

    fn create(&mut self, owner: u64, kind: Kind) -> Result<u64, IpcError> {
        if owner == LIMBO {
            return Err(IpcError::BadOwner);
        }
        let obj = self.objs.create(kind)?;
        match self.caps.mint_root(owner, obj, kind) {
            Ok(c) => {
                self.eps[obj.index()] = Endpoint::new();
                self.nfs[obj.index()] = Notify::new();
                Ok(c.handle())
            }
            Err(e) => {
                // Roll back: no object without a capability.
                let _ = self.objs.destroy(obj);
                Err(e.into())
            }
        }
    }

    pub fn create_endpoint(&mut self, owner: u64) -> Result<u64, IpcError> {
        self.create(owner, Kind::Endpoint)
    }

    pub fn create_notification(&mut self, owner: u64) -> Result<u64, IpcError> {
        self.create(owner, Kind::Notification)
    }

    /// Derives an attenuated copy in the caller's own space (no `GRANT` needed).
    pub fn derive(&mut self, owner: u64, handle: u64, mask: Rights) -> Result<u64, IpcError> {
        let src = self.caps.lookup(owner, handle, Rights::NONE)?;
        Ok(self.caps.derive(src.cap, owner, mask)?.handle())
    }

    /// Kernel-side capability grant to another owner (process creation, name service); the
    /// source needs `GRANT`. Returns the handle in `to`'s space.
    pub fn grant(
        &mut self,
        owner: u64,
        handle: u64,
        to: u64,
        mask: Rights,
    ) -> Result<u64, IpcError> {
        if to == LIMBO {
            return Err(IpcError::BadOwner);
        }
        let src = self.caps.lookup(owner, handle, Rights::GRANT)?;
        Ok(self.caps.derive(src.cap, to, mask)?.handle())
    }

    pub fn close(&mut self, s: &mut Scheduler<N>, owner: u64, handle: u64) -> Result<(), IpcError> {
        self.caps.close(owner, handle)?;
        self.sweep(s)
    }

    /// Deletes everything derived from the capability. Returns how many were deleted.
    pub fn revoke(
        &mut self,
        s: &mut Scheduler<N>,
        owner: u64,
        handle: u64,
    ) -> Result<usize, IpcError> {
        let n = self.caps.revoke(owner, handle)?;
        self.sweep(s)?;
        Ok(n)
    }

    /// Process teardown: closes every capability `owner` holds. Returns the count.
    pub fn owner_exit(&mut self, s: &mut Scheduler<N>, owner: u64) -> Result<usize, IpcError> {
        if owner == LIMBO {
            return Err(IpcError::BadOwner);
        }
        let n = self.caps.remove_owner(owner);
        self.sweep(s)?;
        Ok(n)
    }

    // ---- internals -------------------------------------------------------------------------

    fn event_of(obj: ObjId) -> u32 {
        IPC_EVENT_BASE + obj.index() as u32
    }

    fn current(s: &Scheduler<N>) -> Result<Handle, IpcError> {
        let cur = s.current().ok_or(IpcError::NoCurrent)?;
        if s.thread(cur)?.is_idle() {
            return Err(IpcError::IsIdle);
        }
        Ok(cur)
    }

    fn is_valid_waiter(s: &Scheduler<N>, w: &Wait) -> bool {
        s.thread(w.handle).is_ok_and(|t| {
            t.state() == State::Blocked
                && t.block_reason() == BlockReason::Event(Self::event_of(w.obj))
        })
    }

    fn discard_msg_cap(&mut self, m: &Message) {
        if let Some(c) = m.cap {
            self.caps.discard(c);
        }
    }

    /// Drops state a previous occupant of this thread slot may have left behind.
    fn begin(&mut self, cur: Handle) {
        let i = cur.index();
        if let Some(w) = self.waits[i].take()
            && let Some(m) = w.msg
        {
            self.discard_msg_cap(&m);
        }
        if let Some((_, Done::Received(d))) = self.done[i].take()
            && let Some(c) = d.cap.and_then(CapRef::from_handle)
        {
            self.caps.discard(c);
        }
    }

    /// Best waiter of `kind` on `obj`: highest effective level, FIFO within a level. Records of
    /// threads that are no longer blocked on the object are dropped on the way.
    fn pick_waiter(&mut self, s: &Scheduler<N>, obj: ObjId, kind: WaitKind) -> Option<usize> {
        let mut best: Option<(usize, usize, u64)> = None;
        for i in 0..N {
            let Some(w) = self.waits[i] else { continue };
            if w.obj != obj || w.kind != kind {
                continue;
            }
            if !Self::is_valid_waiter(s, &w) {
                self.waits[i] = None;
                if let Some(m) = w.msg {
                    self.discard_msg_cap(&m);
                }
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
        best.map(|(i, _, _)| i)
    }

    fn block_on(&mut self, s: &mut Scheduler<N>, obj: ObjId) -> Result<(), IpcError> {
        for _ in 0..2 {
            if s.block_current(BlockReason::Event(Self::event_of(obj)))? == BlockOutcome::Blocked {
                return Ok(());
            }
        }
        Err(IpcError::Corrupt("thread would not block"))
    }

    fn enqueue(
        &mut self,
        kind: WaitKind,
        cur: Handle,
        owner: u64,
        r: &super::cap_space::Resolved,
        msg: Option<Message>,
    ) {
        self.seq += 1;
        self.waits[cur.index()] = Some(Wait {
            kind,
            handle: cur,
            cap: r.cap,
            obj: r.obj,
            owner,
            seq: self.seq,
            msg,
        });
        self.done[cur.index()] = None;
    }

    /// Turns a queued/sent message into a delivery for `owner`, moving its capability out of
    /// limbo when that is still possible.
    fn make_delivery(&mut self, m: Message, owner: u64) -> Delivery {
        let (cap, cap_lost) = match m.cap {
            None => (None, false),
            Some(c) => {
                if self.caps.is_live(c) && self.caps.rebind(c, owner).is_ok() {
                    (Some(c.handle()), false)
                } else {
                    self.caps.discard(c);
                    self.stats.caps_lost += 1;
                    (None, true)
                }
            }
        };
        Delivery {
            label: m.label,
            len: m.len,
            words: m.words,
            cap,
            cap_lost,
        }
    }

    fn destroy_object(&mut self, obj: ObjId) {
        let idx = obj.index();
        let mut drained = [None; Q];
        self.eps[idx].drain_into(&mut drained);
        for m in drained.iter().flatten() {
            self.discard_msg_cap(m);
        }
        self.caps.remove_object(obj);
        self.nfs[idx].reset();
        let _ = self.objs.destroy(obj);
    }

    /// Destroys objects without owners and fails waiters whose capability is gone, until
    /// nothing changes (discarding a limbo capability can in turn orphan another object).
    fn sweep(&mut self, s: &mut Scheduler<N>) -> Result<(), IpcError> {
        loop {
            let mut changed = false;
            for i in 0..O {
                if let Some(id) = self.objs.id_at(i)
                    && self.caps.user_refs(id) == 0
                {
                    self.destroy_object(id);
                    changed = true;
                }
            }
            for i in 0..N {
                let Some(w) = self.waits[i] else { continue };
                if self.caps.is_live(w.cap) {
                    continue;
                }
                self.waits[i] = None;
                if let Some(m) = w.msg {
                    self.discard_msg_cap(&m);
                }
                if Self::is_valid_waiter(s, &w) {
                    let e = if self.objs.is_live(w.obj) {
                        IpcError::Revoked
                    } else {
                        IpcError::Closed
                    };
                    self.done[i] = Some((w.handle, Done::Failed(e)));
                    self.stats.waits_failed += 1;
                    s.wake(w.handle)?;
                }
                changed = true;
            }
            if !changed {
                return Ok(());
            }
        }
    }

    // ---- endpoints -------------------------------------------------------------------------

    /// Sends `msg` on the endpoint `handle` names. See the module docs for the semantics.
    pub fn send(
        &mut self,
        s: &mut Scheduler<N>,
        owner: u64,
        handle: u64,
        msg: &UserMessage,
        block: bool,
    ) -> Result<SendOutcome, IpcError> {
        let cur = Self::current(s)?;
        msg.validate()?;
        let r = self.caps.lookup(owner, handle, Rights::SEND)?;
        if r.kind != Kind::Endpoint {
            return Err(IpcError::WrongKind);
        }
        self.begin(cur);
        let limbo = match msg.cap {
            None => None,
            Some(h) => {
                let src = self.caps.lookup(owner, h, Rights::GRANT)?;
                Some(self.caps.derive(src.cap, LIMBO, Rights::DEFINED)?)
            }
        };
        let m = Message {
            label: msg.label,
            len: msg.len,
            words: msg.words,
            cap: limbo,
        };
        let out = self.send_inner(s, cur, owner, &r, m, block);
        if out.is_err() {
            self.discard_msg_cap(&m);
        }
        out
    }

    fn send_inner(
        &mut self,
        s: &mut Scheduler<N>,
        cur: Handle,
        owner: u64,
        r: &super::cap_space::Resolved,
        m: Message,
        block: bool,
    ) -> Result<SendOutcome, IpcError> {
        let idx = r.obj.index();
        if let Some(wi) = self.pick_waiter(s, r.obj, WaitKind::Recv) {
            let w = self.waits[wi]
                .take()
                .ok_or(IpcError::Corrupt("picked waiter vanished"))?;
            let d = self.make_delivery(m, w.owner);
            self.done[wi] = Some((w.handle, Done::Received(d)));
            self.stats.handoffs += 1;
            self.stats.sent += 1;
            self.stats.received += 1;
            s.wake(w.handle)?;
            return Ok(SendOutcome::Sent);
        }
        if !self.eps[idx].is_full() {
            self.eps[idx]
                .push(m)
                .map_err(|_| IpcError::Corrupt("push failed on a non-full queue"))?;
            self.stats.sent += 1;
            return Ok(SendOutcome::Sent);
        }
        if !block {
            return Err(IpcError::WouldBlock);
        }
        self.block_on(s, r.obj)?;
        self.enqueue(WaitKind::Send, cur, owner, r, Some(m));
        Ok(SendOutcome::Blocked)
    }

    /// Receives from the endpoint `handle` names.
    pub fn recv(
        &mut self,
        s: &mut Scheduler<N>,
        owner: u64,
        handle: u64,
        block: bool,
    ) -> Result<RecvOutcome, IpcError> {
        let cur = Self::current(s)?;
        let r = self.caps.lookup(owner, handle, Rights::RECV)?;
        if r.kind != Kind::Endpoint {
            return Err(IpcError::WrongKind);
        }
        self.begin(cur);
        let idx = r.obj.index();
        if let Ok(m) = self.eps[idx].pop() {
            if let Some(wi) = self.pick_waiter(s, r.obj, WaitKind::Send) {
                let w = self.waits[wi]
                    .take()
                    .ok_or(IpcError::Corrupt("picked sender vanished"))?;
                let moved = w
                    .msg
                    .ok_or(IpcError::Corrupt("blocked sender without a message"))?;
                self.eps[idx]
                    .push(moved)
                    .map_err(|_| IpcError::Corrupt("no room right after a pop"))?;
                self.stats.sent += 1;
                self.done[wi] = Some((w.handle, Done::Sent));
                s.wake(w.handle)?;
            }
            let d = self.make_delivery(m, owner);
            self.stats.received += 1;
            return Ok(RecvOutcome::Received(d));
        }
        if !block {
            return Err(IpcError::WouldBlock);
        }
        self.block_on(s, r.obj)?;
        self.enqueue(WaitKind::Recv, cur, owner, &r, None);
        Ok(RecvOutcome::Blocked)
    }

    // ---- notifications ---------------------------------------------------------------------

    /// Signals the notification `handle` names. Never blocks.
    pub fn signal(
        &mut self,
        s: &mut Scheduler<N>,
        owner: u64,
        handle: u64,
        bits: u64,
    ) -> Result<(), IpcError> {
        let r = self.caps.lookup(owner, handle, Rights::SIGNAL)?;
        self.signal_object(s, r.obj, bits)
    }

    /// Resolves a handle to its object, for kernel code (e.g. an interrupt binding) that will
    /// later signal without a capability lookup.
    pub fn object_of(&self, owner: u64, handle: u64, need: Rights) -> Result<ObjId, IpcError> {
        Ok(self.caps.lookup(owner, handle, need)?.obj)
    }

    /// Signals by object id: the entry point for interrupt handlers. Non-blocking; only the
    /// scheduler's `wake` is used, which is legal with interrupts masked.
    pub fn kernel_signal(
        &mut self,
        s: &mut Scheduler<N>,
        obj: ObjId,
        bits: u64,
    ) -> Result<(), IpcError> {
        self.signal_object(s, obj, bits)
    }

    fn signal_object(
        &mut self,
        s: &mut Scheduler<N>,
        obj: ObjId,
        bits: u64,
    ) -> Result<(), IpcError> {
        if bits == 0 {
            return Err(IpcError::BadArg);
        }
        if self.objs.kind(obj)? != Kind::Notification {
            return Err(IpcError::WrongKind);
        }
        self.stats.signals += 1;
        if let Some(wi) = self.pick_waiter(s, obj, WaitKind::Notify) {
            let w = self.waits[wi]
                .take()
                .ok_or(IpcError::Corrupt("picked waiter vanished"))?;
            self.done[wi] = Some((w.handle, Done::Signalled(bits)));
            s.wake(w.handle)?;
            return Ok(());
        }
        self.nfs[obj.index()]
            .signal(bits)
            .map_err(|_| IpcError::BadArg)
    }

    /// Takes the pending word of the notification `handle` names, or blocks for the next one.
    pub fn wait(
        &mut self,
        s: &mut Scheduler<N>,
        owner: u64,
        handle: u64,
        block: bool,
    ) -> Result<WaitOutcome, IpcError> {
        let cur = Self::current(s)?;
        let r = self.caps.lookup(owner, handle, Rights::WAIT)?;
        if r.kind != Kind::Notification {
            return Err(IpcError::WrongKind);
        }
        self.begin(cur);
        if let Some(w) = self.nfs[r.obj.index()].take() {
            return Ok(WaitOutcome::Signalled(w));
        }
        if !block {
            return Err(IpcError::WouldBlock);
        }
        self.block_on(s, r.obj)?;
        self.enqueue(WaitKind::Notify, cur, owner, &r, None);
        Ok(WaitOutcome::Blocked)
    }

    // ---- completion and teardown -----------------------------------------------------------

    /// Collects the result for a thread that was `Blocked` by this domain and has been
    /// scheduled again. A wake-up that did not come from IPC removes the wait record and
    /// reports `Interrupted`.
    pub fn resume(&mut self, h: Handle) -> Result<Resume, IpcError> {
        let i = h.index();
        if let Some((dh, d)) = self.done[i].take() {
            if dh == h {
                return match d {
                    Done::Sent => Ok(Resume::Sent),
                    Done::Received(d) => Ok(Resume::Received(d)),
                    Done::Signalled(b) => Ok(Resume::Signalled(b)),
                    Done::Failed(e) => Err(e),
                };
            }
            if let Done::Received(d) = d
                && let Some(c) = d.cap.and_then(CapRef::from_handle)
            {
                self.caps.discard(c);
            }
        }
        if let Some(w) = self.waits[i]
            && w.handle == h
        {
            self.waits[i] = None;
            if let Some(m) = w.msg {
                self.discard_msg_cap(&m);
            }
        }
        Err(IpcError::Interrupted)
    }

    /// A thread is gone (exited or killed): removes its wait record, discards the capability
    /// of a message it was sending, and releases any capability delivered to it that it never
    /// collected.
    pub fn thread_exit(&mut self, s: &mut Scheduler<N>, h: Handle) -> Result<(), IpcError> {
        let i = h.index();
        if let Some(w) = self.waits[i]
            && w.handle == h
        {
            self.waits[i] = None;
            if let Some(m) = w.msg {
                self.discard_msg_cap(&m);
            }
        }
        if let Some((dh, Done::Received(d))) = self.done[i]
            && dh == h
        {
            if let Some(c) = d.cap.and_then(CapRef::from_handle) {
                self.caps.discard(c);
            }
            self.done[i] = None;
        } else if self.done[i].is_some_and(|(dh, _)| dh == h) {
            self.done[i] = None;
        }
        self.sweep(s)
    }

    /// Subsystem shutdown: deletes every capability, destroys every object and wakes every
    /// blocked thread with `Closed`. Returns the number of capabilities deleted. The domain is
    /// empty afterwards and may be used again.
    pub fn close_all(&mut self, s: &mut Scheduler<N>) -> Result<usize, IpcError> {
        let n = self.caps.remove_all();
        self.sweep(s)?;
        Ok(n)
    }

    /// Whether the thread has a wait record in this domain.
    pub fn is_waiting(&self, h: Handle) -> bool {
        self.waits[h.index()].is_some_and(|w| w.handle == h)
    }

    // ---- diagnostics -----------------------------------------------------------------------

    /// Structural self-check for quiescent points (after the scheduler has switched).
    pub fn check_invariants(&self, s: &Scheduler<N>) -> Result<(), &'static str> {
        self.caps.check_invariants()?;
        for (i, w) in self.waits.iter().enumerate() {
            let Some(w) = w else { continue };
            if w.handle.index() != i {
                return Err("wait record in the wrong slot");
            }
            if !Self::is_valid_waiter(s, w) {
                return Err("waiter is not blocked on its object");
            }
            if !self.caps.is_live(w.cap) {
                return Err("waiter holds a dead capability");
            }
            let kind = self
                .objs
                .kind(w.obj)
                .map_err(|_| "waiting on a dead object")?;
            let idx = w.obj.index();
            match w.kind {
                WaitKind::Recv => {
                    if kind != Kind::Endpoint || !self.eps[idx].is_empty() || w.msg.is_some() {
                        return Err("receiver waiting although a message is queued");
                    }
                }
                WaitKind::Send => {
                    if kind != Kind::Endpoint || !self.eps[idx].is_full() || w.msg.is_none() {
                        return Err("sender waiting although the queue has room");
                    }
                }
                WaitKind::Notify => {
                    if kind != Kind::Notification || self.nfs[idx].pending() != 0 || w.msg.is_some()
                    {
                        return Err("notification waiter although a signal is pending");
                    }
                }
            }
        }
        let mut limbo_expected = 0;
        for i in 0..O {
            match self.objs.id_at(i) {
                Some(id) => {
                    if self.caps.user_refs(id) == 0 {
                        return Err("live object without an owner");
                    }
                    self.eps[i].check_invariants()?;
                    for m in self.eps[i].iter() {
                        if let Some(c) = m.cap
                            && self.caps.is_live(c)
                        {
                            if self.caps.owner_of(c) != Some(LIMBO) {
                                return Err("queued message capability is not in limbo");
                            }
                            limbo_expected += 1;
                        }
                    }
                }
                None => {
                    if !self.eps[i].is_empty() || self.nfs[i].pending() != 0 {
                        return Err("dead object keeps state");
                    }
                }
            }
        }
        for w in self.waits.iter().flatten() {
            if let Some(m) = w.msg
                && let Some(c) = m.cap
                && self.caps.is_live(c)
            {
                if self.caps.owner_of(c) != Some(LIMBO) {
                    return Err("blocked sender capability is not in limbo");
                }
                limbo_expected += 1;
            }
        }
        if self.caps.limbo_count() != limbo_expected {
            return Err("limbo capability not referenced by any message");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::Priority;
    extern crate std;
    use std::vec::Vec;

    type S = Scheduler<16>;
    type D = IpcDomain<16, 8, 32, 4>;

    const A: u64 = 1;
    const B: u64 = 2;

    fn p(l: u8) -> Priority {
        Priority::new(l).unwrap()
    }

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

    fn run_until(s: &mut S, h: Handle) {
        for _ in 0..40 {
            if s.current() == Some(h) {
                return;
            }
            run(s);
        }
        panic!("thread never became current");
    }

    fn msg(label: u64) -> UserMessage {
        UserMessage::new(label, &[label + 1], None).unwrap()
    }

    fn recv_now(d: &mut D, s: &mut S, o: u64, h: u64) -> Delivery {
        match d.recv(s, o, h, false).unwrap() {
            RecvOutcome::Received(x) => x,
            RecvOutcome::Blocked => panic!("blocked in non-blocking mode"),
        }
    }

    #[test]
    fn handles_are_bound_to_their_owner() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        assert_eq!(
            d.send(&mut s, B, ep, &msg(1), false),
            Err(IpcError::BadHandle)
        );
        assert_eq!(d.recv(&mut s, B, ep, false), Err(IpcError::BadHandle));
        assert_eq!(
            d.send(&mut s, A, 0, &msg(1), false),
            Err(IpcError::BadHandle)
        );
        assert_eq!(
            d.send(&mut s, A, ep + (1 << 32), &msg(1), false),
            Err(IpcError::BadHandle)
        );
        assert_eq!(d.create_endpoint(LIMBO), Err(IpcError::BadOwner));
        assert_eq!(d.send(&mut s, A, ep, &msg(1), false), Ok(SendOutcome::Sent));
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn queue_is_fifo_bounded_and_nonblocking_reports_would_block() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        assert_eq!(d.recv(&mut s, A, ep, false), Err(IpcError::WouldBlock));
        for i in 0..4 {
            assert_eq!(d.send(&mut s, A, ep, &msg(i), false), Ok(SendOutcome::Sent));
        }
        assert_eq!(
            d.send(&mut s, A, ep, &msg(9), false),
            Err(IpcError::WouldBlock)
        );
        assert_eq!(d.queued(A, ep), Ok(4));
        for i in 0..4 {
            let x = recv_now(&mut d, &mut s, A, ep);
            assert_eq!((x.label, x.data()), (i, &[i + 1][..]));
        }
        assert_eq!(d.recv(&mut s, A, ep, false), Err(IpcError::WouldBlock));
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn rights_gate_every_operation() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let send_only = d.derive(A, ep, Rights::SEND).unwrap();
        let recv_only = d.derive(A, ep, Rights::RECV).unwrap();
        assert_eq!(d.recv(&mut s, A, send_only, false), Err(IpcError::Perm));
        assert_eq!(
            d.send(&mut s, A, recv_only, &msg(1), false),
            Err(IpcError::Perm)
        );
        d.send(&mut s, A, send_only, &msg(1), false).unwrap();
        assert_eq!(recv_now(&mut d, &mut s, A, recv_only).label, 1);
        // an endpoint capability cannot be used as a notification and vice versa
        let nf = d.create_notification(A).unwrap();
        assert_eq!(d.send(&mut s, A, nf, &msg(1), false), Err(IpcError::Perm));
        assert_eq!(d.signal(&mut s, A, ep, 1), Err(IpcError::Perm));
        assert_eq!(d.wait(&mut s, A, ep, false), Err(IpcError::Perm));
        assert_eq!(
            d.derive(A, send_only, Rights::RECV),
            Err(IpcError::NoRights)
        );
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn invalid_messages_are_rejected_before_any_state_changes() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let dirty = UserMessage {
            len: 0,
            words: [0, 0, 7, 0],
            ..UserMessage::default()
        };
        let long = UserMessage {
            len: 5,
            ..UserMessage::default()
        };
        assert_eq!(d.send(&mut s, A, ep, &dirty, false), Err(IpcError::BadArg));
        assert_eq!(d.send(&mut s, A, ep, &long, false), Err(IpcError::BadArg));
        assert_eq!(d.queued(A, ep), Ok(0));
    }

    #[test]
    fn blocking_recv_is_woken_by_a_send_and_gets_the_message_directly() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        assert_eq!(d.recv(&mut s, A, ep, true), Ok(RecvOutcome::Blocked));
        run_until(&mut s, hs[1]);
        d.check_invariants(&s).unwrap();
        assert_eq!(d.waiting_threads(), 1);
        d.send(&mut s, A, ep, &msg(5), false).unwrap();
        assert_eq!(d.queued(A, ep), Ok(0), "handed over, not queued");
        run_until(&mut s, hs[0]);
        match d.resume(hs[0]).unwrap() {
            Resume::Received(x) => assert_eq!((x.label, x.data()), (5, &[6][..])),
            r => panic!("{r:?}"),
        }
        assert_eq!(d.waiting_threads(), 0);
        assert_eq!(d.stats().handoffs, 1);
        assert_eq!(d.stats().sent, 1);
        assert_eq!(d.stats().received, 1);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn blocking_send_on_a_full_queue_resumes_in_order_after_a_receive() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        for i in 0..4 {
            d.send(&mut s, A, ep, &msg(i), false).unwrap();
        }
        assert_eq!(
            d.send(&mut s, A, ep, &msg(4), true),
            Ok(SendOutcome::Blocked)
        );
        run_until(&mut s, hs[1]);
        d.check_invariants(&s).unwrap();
        // one receive frees a slot and moves the blocked sender's message to the tail
        assert_eq!(recv_now(&mut d, &mut s, A, ep).label, 0);
        assert_eq!(d.queued(A, ep), Ok(4));
        run_until(&mut s, hs[0]);
        assert_eq!(d.resume(hs[0]), Ok(Resume::Sent));
        let got: Vec<u64> = (0..4)
            .map(|_| recv_now(&mut d, &mut s, A, ep).label)
            .collect();
        assert_eq!(got, [1, 2, 3, 4]);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn waiters_are_served_by_priority_then_fifo() {
        let (mut s, mut d, hs) = setup(&[4, 4, 4, 4]);
        let ep = d.create_endpoint(A).unwrap();
        // hs[0], hs[1], hs[2] block in that order; hs[2] is then raised to level 6; hs[3] sends.
        for &h in &hs[..3] {
            run_until(&mut s, h);
            assert_eq!(d.recv(&mut s, A, ep, true), Ok(RecvOutcome::Blocked));
        }
        s.set_priority(hs[2], p(6)).unwrap();
        run_until(&mut s, hs[3]);
        let mut order = Vec::new();
        for i in 0..3 {
            d.send(&mut s, A, ep, &msg(i), false).unwrap();
            // the woken thread is the one whose record disappeared
            for &h in &hs[..3] {
                if !order.contains(&h.index())
                    && matches!(d.done[h.index()], Some((_, Done::Received(_))))
                {
                    order.push(h.index());
                }
            }
        }
        assert_eq!(order, [hs[2].index(), hs[0].index(), hs[1].index()]);
    }

    #[test]
    fn capability_transfer_moves_a_derived_cap_atomically() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        let m = UserMessage::new(1, &[], Some(payload)).unwrap();
        // B must hold a receive capability to the carrier endpoint
        let ep_b = d.grant(A, ep, B, Rights::RECV).unwrap();
        d.send(&mut s, A, ep, &m, false).unwrap();
        assert_eq!(d.caps().limbo_count(), 1);
        d.check_invariants(&s).unwrap();
        let x = recv_now(&mut d, &mut s, B, ep_b);
        assert!(!x.cap_lost);
        let got = x.cap.unwrap();
        assert_eq!(d.caps().limbo_count(), 0);
        // sender keeps its own capability; receiver's is a separate one with the same rights
        let r = d
            .caps()
            .lookup(B, got, Rights::SEND | Rights::RECV)
            .unwrap();
        assert_eq!(r.rights, Rights::root_for(Kind::Endpoint));
        assert!(d.caps().lookup(A, payload, Rights::SEND).is_ok());
        assert_eq!(
            d.caps().lookup(A, got, Rights::NONE),
            Err(CapError::BadHandle)
        );
        // B can use it
        d.send(&mut s, B, got, &msg(2), false).unwrap();
        assert_eq!(recv_now(&mut d, &mut s, A, payload).label, 2);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn sending_a_capability_needs_grant_and_a_valid_handle_and_leaks_nothing() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        let weak = d.derive(A, payload, Rights::SEND | Rights::RECV).unwrap();
        let m = UserMessage::new(1, &[], Some(weak)).unwrap();
        assert_eq!(d.send(&mut s, A, ep, &m, false), Err(IpcError::Perm));
        let bogus = UserMessage::new(1, &[], Some(0xDEAD)).unwrap();
        assert_eq!(
            d.send(&mut s, A, ep, &bogus, false),
            Err(IpcError::BadHandle)
        );
        let foreign = UserMessage::new(1, &[], Some(ep ^ (1 << 40))).unwrap();
        assert_eq!(
            d.send(&mut s, A, ep, &foreign, false),
            Err(IpcError::BadHandle)
        );
        assert_eq!(d.caps().limbo_count(), 0);
        assert_eq!(d.queued(A, ep), Ok(0));
        // a failed non-blocking send (full queue) also releases its limbo capability
        for i in 0..4 {
            d.send(&mut s, A, ep, &msg(i), false).unwrap();
        }
        let ok = UserMessage::new(1, &[], Some(payload)).unwrap();
        assert_eq!(d.send(&mut s, A, ep, &ok, false), Err(IpcError::WouldBlock));
        assert_eq!(d.caps().limbo_count(), 0);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn capability_revoked_in_flight_is_reported_as_lost() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        let m = UserMessage::new(1, &[], Some(payload)).unwrap();
        d.send(&mut s, A, ep, &m, false).unwrap();
        assert_eq!(
            d.revoke(&mut s, A, payload).unwrap(),
            1,
            "the limbo copy is a descendant"
        );
        let x = recv_now(&mut d, &mut s, A, ep);
        assert!(x.cap_lost && x.cap.is_none());
        assert_eq!(x.label, 1, "the data still arrives");
        assert_eq!(d.stats().caps_lost, 1);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn receiver_quota_turns_a_capability_into_cap_lost_but_keeps_the_message() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        let ep_b = d.grant(A, ep, B, Rights::RECV).unwrap();
        d.set_owner_cap_limit(1).unwrap();
        let m = UserMessage::new(3, &[], Some(payload)).unwrap();
        // limit applies to B too: it already holds ep_b, so it is full
        d.send(&mut s, A, ep, &m, false).unwrap(); // limbo copies are exempt from the quota
        let x = recv_now(&mut d, &mut s, B, ep_b);
        assert!(x.cap_lost);
        assert_eq!(x.label, 3);
        assert_eq!(d.stats().caps_lost, 1);
        assert_eq!(d.caps().limbo_count(), 0);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn revoking_the_capability_a_thread_waits_with_wakes_it_with_revoked() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let ep_b = d.grant(A, ep, B, Rights::RECV).unwrap();
        assert_eq!(d.recv(&mut s, B, ep_b, true), Ok(RecvOutcome::Blocked));
        run_until(&mut s, hs[1]);
        assert_eq!(d.revoke(&mut s, A, ep), Ok(1));
        assert_eq!(d.waiting_threads(), 0);
        run_until(&mut s, hs[0]);
        assert_eq!(d.resume(hs[0]), Err(IpcError::Revoked));
        assert_eq!(d.recv(&mut s, B, ep_b, false), Err(IpcError::BadHandle));
        assert_eq!(d.live_objects(), 1, "A still owns the endpoint");
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn closing_the_last_capability_destroys_the_object_and_wakes_waiters_with_closed() {
        let (mut s, mut d, hs) = setup(&[4, 4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let ep_b = d.grant(A, ep, B, Rights::RECV | Rights::SEND).unwrap();
        assert_eq!(d.recv(&mut s, B, ep_b, true), Ok(RecvOutcome::Blocked));
        run_until(&mut s, hs[1]);
        d.close(&mut s, A, ep).unwrap();
        assert_eq!(d.live_objects(), 1, "B's copy keeps it alive");
        assert_eq!(d.waiting_threads(), 1);
        run_until(&mut s, hs[2]);
        d.close(&mut s, B, ep_b).unwrap();
        assert_eq!(d.live_objects(), 0);
        assert_eq!(d.waiting_threads(), 0);
        run_until(&mut s, hs[0]);
        assert_eq!(d.resume(hs[0]), Err(IpcError::Closed));
        assert_eq!(d.close(&mut s, B, ep_b), Err(IpcError::BadHandle));
        // the slot can be reused without resurrecting old handles
        let again = d.create_endpoint(A).unwrap();
        assert_ne!(again, ep);
        assert_eq!(
            d.send(&mut s, A, ep, &msg(1), false),
            Err(IpcError::BadHandle)
        );
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn destroying_an_endpoint_discards_queued_messages_and_their_limbo_caps() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        for _ in 0..3 {
            d.send(
                &mut s,
                A,
                ep,
                &UserMessage::new(1, &[], Some(payload)).unwrap(),
                false,
            )
            .unwrap();
        }
        assert_eq!(d.caps().limbo_count(), 3);
        d.close(&mut s, A, ep).unwrap();
        assert_eq!(d.caps().limbo_count(), 0);
        assert_eq!(d.live_objects(), 1);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn a_capability_sent_through_its_own_endpoint_cannot_keep_it_alive() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        d.send(
            &mut s,
            A,
            ep,
            &UserMessage::new(1, &[], Some(ep)).unwrap(),
            false,
        )
        .unwrap();
        assert_eq!(d.caps().limbo_count(), 1);
        d.close(&mut s, A, ep).unwrap();
        assert_eq!(d.live_objects(), 0);
        assert_eq!(d.caps().live_count(), 0);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn notification_signal_then_wait_and_wait_then_signal() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let nf = d.create_notification(A).unwrap();
        assert_eq!(d.wait(&mut s, A, nf, false), Err(IpcError::WouldBlock));
        d.signal(&mut s, A, nf, 0b001).unwrap();
        d.signal(&mut s, A, nf, 0b100).unwrap();
        assert_eq!(
            d.wait(&mut s, A, nf, false),
            Ok(WaitOutcome::Signalled(0b101))
        );
        assert_eq!(d.signal(&mut s, A, nf, 0), Err(IpcError::BadArg));
        assert_eq!(d.wait(&mut s, A, nf, true), Ok(WaitOutcome::Blocked));
        run_until(&mut s, hs[1]);
        d.check_invariants(&s).unwrap();
        d.signal(&mut s, A, nf, 0b10).unwrap();
        run_until(&mut s, hs[0]);
        assert_eq!(d.resume(hs[0]), Ok(Resume::Signalled(0b10)));
        assert_eq!(
            d.wait(&mut s, A, nf, false),
            Err(IpcError::WouldBlock),
            "nothing left over"
        );
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn kernel_signal_models_an_interrupt_waking_a_waiter() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let nf = d.create_notification(A).unwrap();
        let obj = d.object_of(A, nf, Rights::SIGNAL).unwrap();
        assert_eq!(d.wait(&mut s, A, nf, true), Ok(WaitOutcome::Blocked));
        run_until(&mut s, hs[1]);
        d.kernel_signal(&mut s, obj, 1 << 40).unwrap();
        run_until(&mut s, hs[0]);
        assert_eq!(d.resume(hs[0]), Ok(Resume::Signalled(1 << 40)));
        // a signal to a destroyed object is refused
        d.close(&mut s, A, nf).unwrap();
        assert_eq!(d.kernel_signal(&mut s, obj, 1), Err(IpcError::BadHandle));
        let ep = d.create_endpoint(A).unwrap();
        let ep_obj = d.object_of(A, ep, Rights::NONE).unwrap();
        assert_eq!(d.kernel_signal(&mut s, ep_obj, 1), Err(IpcError::WrongKind));
    }

    #[test]
    fn external_wake_up_is_reported_as_interrupted_and_cleans_the_record() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        assert_eq!(d.recv(&mut s, A, ep, true), Ok(RecvOutcome::Blocked));
        run_until(&mut s, hs[1]);
        s.wake(hs[0]).unwrap();
        run_until(&mut s, hs[0]);
        assert_eq!(d.resume(hs[0]), Err(IpcError::Interrupted));
        assert_eq!(d.waiting_threads(), 0);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn killed_blocked_sender_leaves_no_wait_record_and_no_limbo_capability() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        for i in 0..4 {
            d.send(&mut s, A, ep, &msg(i), false).unwrap();
        }
        let m = UserMessage::new(9, &[], Some(payload)).unwrap();
        assert_eq!(d.send(&mut s, A, ep, &m, true), Ok(SendOutcome::Blocked));
        assert_eq!(d.caps().limbo_count(), 1);
        run_until(&mut s, hs[1]);
        s.kill(hs[0], -1).unwrap();
        d.thread_exit(&mut s, hs[0]).unwrap();
        assert_eq!(d.waiting_threads(), 0);
        assert_eq!(d.caps().limbo_count(), 0);
        assert_eq!(
            d.queued(A, ep),
            Ok(4),
            "the dead sender's message was never queued"
        );
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn thread_exit_releases_a_delivered_but_uncollected_capability() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        let ep_b = d.grant(A, ep, B, Rights::RECV).unwrap();
        assert_eq!(d.recv(&mut s, B, ep_b, true), Ok(RecvOutcome::Blocked));
        run_until(&mut s, hs[1]);
        d.send(
            &mut s,
            A,
            ep,
            &UserMessage::new(1, &[], Some(payload)).unwrap(),
            false,
        )
        .unwrap();
        assert_eq!(
            d.caps().owner_count(B),
            2,
            "delivery rebound the cap to B already"
        );
        s.kill(hs[0], -1).unwrap();
        d.thread_exit(&mut s, hs[0]).unwrap();
        assert_eq!(d.caps().owner_count(B), 1);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn owner_exit_closes_everything_and_destroys_orphaned_objects() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let nf = d.create_notification(A).unwrap();
        let ep_b = d.grant(A, ep, B, Rights::RECV).unwrap();
        assert_eq!(d.recv(&mut s, B, ep_b, true), Ok(RecvOutcome::Blocked));
        run_until(&mut s, hs[1]);
        assert_eq!(d.owner_exit(&mut s, A), Ok(2));
        assert_eq!(d.live_objects(), 1, "B still holds the endpoint");
        assert_eq!(d.waiting_threads(), 1, "B's waiter is unaffected");
        assert_eq!(d.signal(&mut s, A, nf, 1), Err(IpcError::BadHandle));
        assert_eq!(d.owner_exit(&mut s, LIMBO), Err(IpcError::BadOwner));
        assert_eq!(d.owner_exit(&mut s, B), Ok(1));
        assert_eq!(d.live_objects(), 0);
        run_until(&mut s, hs[0]);
        assert_eq!(d.resume(hs[0]), Err(IpcError::Closed));
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn object_and_capability_exhaustion_roll_back_cleanly() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let mut made = Vec::new();
        for _ in 0..8 {
            made.push(d.create_endpoint(A).unwrap());
        }
        assert_eq!(d.create_endpoint(A), Err(IpcError::NoObjects));
        assert_eq!(d.create_notification(B), Err(IpcError::NoObjects));
        d.close(&mut s, A, made.pop().unwrap()).unwrap();
        assert!(d.create_notification(B).is_ok());

        let mut d2 = D::new();
        d2.set_owner_cap_limit(2).unwrap();
        d2.create_endpoint(A).unwrap();
        d2.create_endpoint(A).unwrap();
        assert_eq!(d2.create_endpoint(A), Err(IpcError::Quota));
        assert_eq!(
            d2.live_objects(),
            2,
            "the failed create left no object behind"
        );
        d2.check_invariants(&s).unwrap();
    }

    #[test]
    fn stale_state_from_a_previous_thread_in_the_slot_is_not_delivered() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        let ep_b = d.grant(A, ep, B, Rights::RECV).unwrap();
        assert_eq!(d.recv(&mut s, B, ep_b, true), Ok(RecvOutcome::Blocked));
        run_until(&mut s, hs[1]);
        d.send(
            &mut s,
            A,
            ep,
            &UserMessage::new(1, &[], Some(payload)).unwrap(),
            false,
        )
        .unwrap();
        // the receiver dies WITHOUT calling thread_exit; its slot is reused
        s.kill(hs[0], 0).unwrap();
        s.reap(hs[0]).unwrap();
        let newcomer = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(newcomer.index(), hs[0].index());
        run_until(&mut s, newcomer);
        assert_eq!(d.resume(newcomer), Err(IpcError::Interrupted));
        // and its next operation discards the leftover capability
        let nf = d.create_notification(A).unwrap();
        assert_eq!(d.wait(&mut s, A, nf, false), Err(IpcError::WouldBlock));
        assert_eq!(d.caps().owner_count(B), 1);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn stale_wait_record_of_a_dead_sender_is_cleaned_by_the_slots_next_user() {
        let (mut s, mut d, hs) = setup(&[4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        for i in 0..4 {
            d.send(&mut s, A, ep, &msg(i), false).unwrap();
        }
        let m = UserMessage::new(9, &[], Some(payload)).unwrap();
        assert_eq!(d.send(&mut s, A, ep, &m, true), Ok(SendOutcome::Blocked));
        run_until(&mut s, hs[1]);
        // killed WITHOUT thread_exit; the slot is reused by a new thread
        s.kill(hs[0], 0).unwrap();
        s.reap(hs[0]).unwrap();
        let newcomer = s.spawn(p(4), 0, 0).unwrap();
        assert_eq!(newcomer.index(), hs[0].index());
        run_until(&mut s, newcomer);
        assert_eq!(
            d.caps().limbo_count(),
            1,
            "still leaked until the slot is used again"
        );
        let nf = d.create_notification(A).unwrap();
        assert_eq!(d.wait(&mut s, A, nf, false), Err(IpcError::WouldBlock));
        assert_eq!(d.caps().limbo_count(), 0);
        assert_eq!(d.waiting_threads(), 0);
        d.check_invariants(&s).unwrap();
    }

    #[test]
    fn close_all_destroys_everything_and_wakes_every_waiter_with_closed() {
        let (mut s, mut d, hs) = setup(&[4, 4, 4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        let nf = d.create_notification(B).unwrap();
        let payload = d.create_endpoint(A).unwrap();
        d.send(
            &mut s,
            A,
            ep,
            &UserMessage::new(1, &[], Some(payload)).unwrap(),
            false,
        )
        .unwrap();
        assert_eq!(d.recv(&mut s, A, payload, true), Ok(RecvOutcome::Blocked));
        run_until(&mut s, hs[1]);
        assert_eq!(d.wait(&mut s, B, nf, true), Ok(WaitOutcome::Blocked));
        run_until(&mut s, hs[2]);
        assert!(d.is_waiting(hs[0]) && d.is_waiting(hs[1]) && !d.is_waiting(hs[2]));
        assert_eq!(d.close_all(&mut s), Ok(4));
        assert_eq!(
            (d.live_objects(), d.waiting_threads(), d.caps().live_count()),
            (0, 0, 0)
        );
        run_until(&mut s, hs[0]);
        assert_eq!(d.resume(hs[0]), Err(IpcError::Closed));
        run_until(&mut s, hs[1]);
        assert_eq!(d.resume(hs[1]), Err(IpcError::Closed));
        d.check_invariants(&s).unwrap();
        assert_eq!(d.close_all(&mut s), Ok(0), "idempotent");
        assert!(d.create_endpoint(A).is_ok(), "usable again");
    }

    #[test]
    fn no_current_thread_and_idle_thread_are_refused() {
        let mut s = S::new();
        let mut d = D::new();
        let ep = d.create_endpoint(A).unwrap();
        assert_eq!(
            d.send(&mut s, A, ep, &msg(1), false),
            Err(IpcError::NoCurrent)
        );
        s.init_idle(0, 0).unwrap();
        run(&mut s);
        assert_eq!(d.recv(&mut s, A, ep, false), Err(IpcError::IsIdle));
    }

    #[test]
    fn invariant_checker_detects_a_leaked_limbo_capability() {
        let (mut s, mut d, hs) = setup(&[4]);
        run_until(&mut s, hs[0]);
        let ep = d.create_endpoint(A).unwrap();
        d.check_invariants(&s).unwrap();
        let src = d.caps.lookup(A, ep, Rights::GRANT).unwrap();
        d.caps.derive(src.cap, LIMBO, Rights::SEND).unwrap();
        assert!(d.check_invariants(&s).is_err());
    }
}
