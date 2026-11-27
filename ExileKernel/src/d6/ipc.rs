//! Core IPC subsystem (Mission D6, file 4). SPDX-License-Identifier: Apache-2.0
//!
//! [`Ipc`] owns every D6 table: objects, capabilities, per-process handle tables, channels,
//! endpoints and the wait registry. The rest of D6 extends it with `impl Ipc` blocks:
//! `ipc_endpoint.rs` (endpoints, pairing, listeners, closure), `ipc_transfer.rs` (capability
//! transfer) and `ipc_wait.rs` (blocking). `service.rs` and `driver.rs` sit on top.
//!
//! # Model
//! * A **channel** is a pair of **endpoints** (`A`, `B`). Each endpoint is a kernel object that
//!   processes reach only through capabilities with `SEND` / `RECV` rights.
//! * Each direction has one bounded FIFO queue of [`QUEUE_CAPACITY`] messages. `queues[i]` holds
//!   the messages waiting to be received by end `i`.
//!
//! # Ordering policy (documented, tested)
//! * **Per direction, strict FIFO**: messages are received in exactly the order they were
//!   accepted. Message ids are assigned at acceptance and strictly increase per channel.
//! * No ordering is promised between the two directions of a channel, or between channels.
//! * Messages already queued when the *sender's* end closes stay receivable; after the queue
//!   drains, `recv` reports `PeerClosed`. Messages queued *for* an end that closes are discarded
//!   with their in-flight capabilities (nobody can ever receive them) and counted in
//!   [`IpcStats::discarded_on_close`].
//!
//! # Backpressure
//! A full queue is never silently dropped into: non-blocking `send` returns
//! [`Error::QueueFull`]; blocking `send` sleeps through the scheduler until space appears, the
//! peer closes, the wait is cancelled or times out.
//!
//! # Lock order (single global order; no function may take them backwards)
//! `KernelSlot (scheduler)` -> `SyncSlot (SyncDomain)` -> `IpcSlot (this struct)`.
//! The model API passes `&mut Scheduler` / `&mut SyncDomain` explicitly ([`Env`]) so the order is
//! structural: IPC code never locks anything itself.
//!
//! # Contexts
//! Blocking calls refuse interrupt context and preemption-disabled sections
//! ([`Context`]). Non-blocking `send`/`recv` never block and only *wake*, which the scheduler
//! allows from interrupt context.
#![allow(dead_code)]

use crate::d6::capability::{CapTable, Rights, MAX_CAPS};
use crate::d6::handle::{Handle, HandleRegistry, MAX_HANDLES, MAX_PROCESSES};
use crate::d6::ipc_endpoint::EndpointTable;
use crate::d6::ipc_message::{Message, MsgId};
use crate::d6::ipc_wait::{Env, WaitTable};
use crate::d6::object::{Counter, Error, ObjectId, ObjectKind, ObjectTable, Owner};

pub const MAX_OBJECTS: usize = 160;
pub const MAX_OBJECTS_PER_OWNER: usize = 64;
pub const MAX_CAPS_PER_OWNER: usize = 48;
pub const MAX_ENDPOINTS: usize = 48;
pub const MAX_CHANNELS: usize = 16;
pub const QUEUE_CAPACITY: usize = 8;
/// Thread slots the wait registry can track (must be >= the scheduler's `N`).
pub const MAX_THREADS: usize = 16;
/// Wait queues (`SyncDomain` objects) the IPC layer needs: two per endpoint.
pub const MAX_PROC_REFS: usize = 16;
pub const SYNC_OBJECTS_NEEDED: usize = 2 * MAX_ENDPOINTS;

/// Rights a freshly created channel endpoint capability carries.
pub const PEER_RIGHTS: Rights = Rights::SEND
    .union(Rights::RECV)
    .union(Rights::TRANSFER)
    .union(Rights::DERIVE)
    .union(Rights::CANCEL)
    .union(Rights::DESTROY);

/// Bounded FIFO of messages. Fixed storage, no allocation.
#[derive(Clone, Copy)]
pub struct Queue {
    buf: [Option<Message>; QUEUE_CAPACITY],
    head: usize,
    len: usize,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub const fn new() -> Self {
        Self {
            buf: [None; QUEUE_CAPACITY],
            head: 0,
            len: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub const fn is_full(&self) -> bool {
        self.len == QUEUE_CAPACITY
    }
    pub fn push(&mut self, m: Message) -> Result<(), Error> {
        if self.is_full() {
            return Err(Error::QueueFull);
        }
        let tail = (self.head + self.len) % QUEUE_CAPACITY;
        self.buf[tail] = Some(m);
        self.len += 1;
        Ok(())
    }
    pub fn front(&self) -> Option<&Message> {
        if self.len == 0 {
            None
        } else {
            self.buf[self.head].as_ref()
        }
    }
    pub fn pop(&mut self) -> Option<Message> {
        if self.len == 0 {
            return None;
        }
        let m = self.buf[self.head].take();
        self.head = (self.head + 1) % QUEUE_CAPACITY;
        self.len -= 1;
        m
    }
    pub fn iter(&self) -> impl Iterator<Item = &Message> + '_ {
        (0..self.len).filter_map(move |i| self.buf[(self.head + i) % QUEUE_CAPACITY].as_ref())
    }
}

/// Two endpoints and the two queues between them.
#[derive(Clone, Copy)]
pub struct Channel {
    /// The `Channel` object; its `inflight` counter equals the queued messages in both queues.
    pub object: ObjectId,
    /// Endpoint objects; `None` once that end has closed.
    pub ends: [Option<ObjectId>; 2],
    /// `queues[i]`: messages waiting to be received by end `i`.
    pub queues: [Queue; 2],
    pub next_id: u64,
}

pub struct ChannelTable<const CH: usize> {
    slots: [Option<Channel>; CH],
}

impl<const CH: usize> Default for ChannelTable<CH> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CH: usize> ChannelTable<CH> {
    pub const fn new() -> Self {
        Self { slots: [None; CH] }
    }
    pub fn live(&self) -> usize {
        self.slots.iter().flatten().count()
    }
    pub fn get(&self, idx: usize) -> Result<&Channel, Error> {
        self.slots.get(idx).and_then(|s| s.as_ref()).ok_or(Error::Stale)
    }
    pub fn get_mut(&mut self, idx: usize) -> Result<&mut Channel, Error> {
        self.slots.get_mut(idx).and_then(|s| s.as_mut()).ok_or(Error::Stale)
    }
    pub fn iter(&self) -> impl Iterator<Item = (usize, &Channel)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|c| (i, c)))
    }
    /// Allocates a slot and its `Channel` object (tag = slot index).
    pub fn create<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        owner: Owner,
    ) -> Result<usize, Error> {
        let i = self
            .slots
            .iter()
            .position(|s| s.is_none())
            .ok_or(Error::ChannelsExhausted)?;
        let object = objs.create(ObjectKind::Channel, owner, i as u32)?;
        self.slots[i] = Some(Channel {
            object,
            ends: [None, None],
            queues: [Queue::new(), Queue::new()],
            next_id: 1,
        });
        Ok(i)
    }
    pub fn free(&mut self, idx: usize) {
        if let Some(s) = self.slots.get_mut(idx) {
            *s = None;
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IpcStats {
    pub sent: u64,
    pub received: u64,
    pub queue_full: u64,
    pub discarded_on_close: u64,
    pub transfers_committed: u64,
    pub transfers_rolled_back: u64,
    pub transfers_revoked: u64,
    pub wakeups: u64,
}

/// The whole IPC/capability state of the kernel.
pub struct Ipc {
    pub(crate) objects: ObjectTable<MAX_OBJECTS>,
    pub(crate) caps: CapTable<MAX_CAPS>,
    pub(crate) procs: HandleRegistry<MAX_PROCESSES, MAX_HANDLES>,
    pub(crate) channels: ChannelTable<MAX_CHANNELS>,
    pub(crate) endpoints: EndpointTable<MAX_ENDPOINTS>,
    pub(crate) waits: WaitTable,
    pub(crate) proc_refs: [Option<Owner>; MAX_PROC_REFS],
    pub(crate) stats: IpcStats,
}

impl Default for Ipc {
    fn default() -> Self {
        Self::new()
    }
}

impl Ipc {
    pub const fn new() -> Self {
        Self {
            objects: ObjectTable::with_quota(MAX_OBJECTS_PER_OWNER),
            caps: CapTable::with_quota(MAX_CAPS_PER_OWNER),
            procs: HandleRegistry::new(),
            channels: ChannelTable::new(),
            endpoints: EndpointTable::new(),
            waits: WaitTable::new(),
            proc_refs: [None; MAX_PROC_REFS],
            stats: IpcStats {
                sent: 0,
                received: 0,
                queue_full: 0,
                discarded_on_close: 0,
                transfers_committed: 0,
                transfers_rolled_back: 0,
                transfers_revoked: 0,
                wakeups: 0,
            },
        }
    }

    pub fn stats(&self) -> IpcStats {
        self.stats
    }
    pub fn objects(&self) -> &ObjectTable<MAX_OBJECTS> {
        &self.objects
    }
    pub fn caps(&self) -> &CapTable<MAX_CAPS> {
        &self.caps
    }
    pub fn channels(&self) -> &ChannelTable<MAX_CHANNELS> {
        &self.channels
    }

    // ---- processes ---------------------------------------------------------------------

    /// Gives a process its own, empty handle table. A process starts with *no* authority.
    pub fn create_process(&mut self, owner: Owner) -> Result<(), Error> {
        self.procs.create(owner)
    }

    pub fn has_process(&self, owner: Owner) -> bool {
        self.procs.contains(owner)
    }

    /// Closes one handle of `owner`. If it was the last capability to an endpoint, the endpoint
    /// closes (its peer sees `PeerClosed`).
    pub fn close_handle<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
    ) -> Result<(), Error> {
        let t = self.procs.get_mut(owner)?;
        t.close(&mut self.caps, &mut self.objects, h)?;
        self.reap_endpoints(env)?;
        Ok(())
    }

    /// Duplicates a handle with at most the rights of the original (needs `DERIVE`).
    pub fn duplicate_handle(
        &mut self,
        owner: Owner,
        h: Handle,
        mask: Rights,
    ) -> Result<Handle, Error> {
        let t = self.procs.get_mut(owner)?;
        t.duplicate(&mut self.caps, &mut self.objects, h, mask)
    }

    /// Revokes everything derived from the capability behind `h` (needs `REVOKE`).
    pub fn revoke_derived<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
    ) -> Result<usize, Error> {
        let t = self.procs.get_mut(owner)?;
        let id = t.resolve(h)?;
        let n = self.caps.revoke_descendants(&mut self.objects, id, owner)?;
        self.reap_endpoints(env)?;
        Ok(n)
    }

    /// The rights of the capability behind `h` (no authority needed to inspect your own).
    pub fn rights_of(&mut self, owner: Owner, h: Handle) -> Result<Rights, Error> {
        let t = self.procs.get_mut(owner)?;
        Ok(t.lookup(&self.caps, &self.objects, h, Rights::NONE)?.1.rights)
    }

    // ---- messaging ---------------------------------------------------------------------

    /// Parses `bytes` as a wire message from `owner` and sends it. The sender identity is the
    /// calling process, never anything in `bytes`.
    pub fn send_wire<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
        bytes: &[u8],
    ) -> Result<MsgId, Error> {
        let msg = Message::from_wire(owner, bytes)?;
        self.send(env, owner, h, msg)
    }

    /// Non-blocking send. Atomic: on any error nothing was queued, no capability was staged and
    /// no reference changed.
    pub fn send<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
        mut msg: Message,
    ) -> Result<MsgId, Error> {
        if !msg.kind.is_user_sendable() {
            return Err(Error::BadKind);
        }
        // The sender identity is stamped by the kernel, whatever the caller built.
        msg.sender = owner;
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::SEND)?;
        let id = self.send_via(env, owner, cap.object, msg)?;
        self.reap_endpoints(env)?;
        Ok(id)
    }

    /// Queues `msg` toward the peer of endpoint `ep` on behalf of `sender`. The caller has
    /// already authorised the operation (capability check, or kernel authority for the
    /// reference-monitor paths of `driver.rs`).
    pub(crate) fn send_via<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        sender: Owner,
        ep: ObjectId,
        msg: Message,
    ) -> Result<MsgId, Error> {
        let owner = sender;
        let (chan, side) = self.endpoints.peer_of(&self.objects, ep)?;
        let (peer_end, full) = {
            let c = self.channels.get(chan)?;
            let peer = 1 - side;
            (c.ends[peer], c.queues[peer].is_full())
        };
        let Some(peer_end) = peer_end else {
            return Err(Error::PeerClosed);
        };
        if full {
            self.stats.queue_full += 1;
            return Err(Error::QueueFull);
        }
        self.enqueue(env, owner, chan, side, peer_end, msg)
    }

    /// Stage transfers, queue the message, then (for MOVE) close the sender's handles.
    pub(crate) fn enqueue<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        sender: Owner,
        chan: usize,
        side: usize,
        peer_end: ObjectId,
        mut msg: Message,
    ) -> Result<MsgId, Error> {
        self.stage_transfers(sender, &mut msg)?;
        let peer = 1 - side;
        let chan_obj = self.channels.get(chan)?.object;
        if let Err(e) = self.objects.acquire(chan_obj, Counter::Inflight) {
            self.discard_staged(&msg);
            return Err(e);
        }
        let id = {
            let c = self.channels.get_mut(chan)?;
            let id = MsgId(c.next_id);
            c.next_id += 1;
            msg.id = id;
            if let Err(e) = c.queues[peer].push(msg) {
                // Cannot happen (fullness was checked) but must not leak if it does.
                let _ = self.objects.release(chan_obj, Counter::Inflight);
                self.discard_staged(&msg);
                return Err(e);
            }
            id
        };
        self.stats.sent += 1;
        if msg.flags.contains(crate::d6::ipc_message::MsgFlags::MOVE_CAPS) {
            self.close_moved_handles(sender, &msg)?;
        }
        self.wake_receiver(env, peer_end)?;
        Ok(id)
    }

    /// Non-blocking receive of the oldest queued message.
    ///
    /// Capabilities carried by the message are installed in the receiver's handle table first;
    /// if that cannot happen (`DestinationFull`, `QuotaExceeded`) the message **stays queued**.
    pub fn recv<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
    ) -> Result<Message, Error> {
        let r = self.recv_inner(env, owner, h);
        // Discarded in-flight capabilities may have dropped the last reference to an endpoint.
        self.reap_endpoints(env)?;
        r
    }

    fn recv_inner<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
    ) -> Result<Message, Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::RECV)?;
        let (chan, side) = self.endpoints.peer_of(&self.objects, cap.object)?;
        let (mut msg, peer_end) = {
            let c = self.channels.get(chan)?;
            match c.queues[side].front() {
                Some(m) => (*m, c.ends[1 - side]),
                None if c.ends[1 - side].is_none() => return Err(Error::PeerClosed),
                None => return Err(Error::QueueEmpty),
            }
        };
        match self.deliver_transfers(owner, &mut msg) {
            Ok(()) => {}
            Err(Error::TransferRevoked) => {
                // Revoked authority is never installed. The message is consumed (reported to
                // the receiver) so the queue cannot jam behind it.
                self.stats.transfers_revoked += 1;
                self.consume_front(chan, side)?;
                self.discard_staged(&msg);
                if let Some(pe) = peer_end {
                    self.wake_sender(env, pe)?;
                }
                return Err(Error::TransferRevoked);
            }
            Err(e) => return Err(e),
        }
        self.consume_front(chan, side)?;
        self.stats.received += 1;
        if let Some(pe) = peer_end {
            self.wake_sender(env, pe)?;
        }
        Ok(msg)
    }

    /// Pops the head of `queues[side]` and releases its in-flight pin on the channel object.
    pub(crate) fn consume_front(&mut self, chan: usize, side: usize) -> Result<Message, Error> {
        let (m, obj) = {
            let c = self.channels.get_mut(chan)?;
            (c.queues[side].pop().ok_or(Error::Corrupt("consume of empty queue"))?, c.object)
        };
        self.objects
            .release(obj, Counter::Inflight)
            .map_err(|_| Error::Corrupt("queued message without a channel pin"))?;
        Ok(m)
    }

    /// Free queue slots toward the peer of the end behind `h` (needs `SEND`). 0 when the peer
    /// is closed. Lets a server apply backpressure before it consumes a request.
    pub fn send_space(&mut self, owner: Owner, h: Handle) -> Result<usize, Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::SEND)?;
        let (chan, side) = self.endpoints.peer_of(&self.objects, cap.object)?;
        let c = self.channels.get(chan)?;
        Ok(if c.ends[1 - side].is_none() {
            0
        } else {
            QUEUE_CAPACITY - c.queues[1 - side].len()
        })
    }

    /// Messages waiting for the end behind `h`.
    pub fn pending(&mut self, owner: Owner, h: Handle) -> Result<usize, Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::RECV)?;
        let (chan, side) = self.endpoints.peer_of(&self.objects, cap.object)?;
        Ok(self.channels.get(chan)?.queues[side].len())
    }

    // ---- process references -------------------------------------------------------------

    /// Kernel operation: gives `holder` a capability naming process `subject` with `rights`
    /// (typically `LOOKUP` to learn who it is and `DESTROY` to terminate it). Holding no such
    /// capability, a process has no way to name or terminate another.
    pub fn grant_process_ref(
        &mut self,
        holder: Owner,
        subject: Owner,
        rights: Rights,
    ) -> Result<Handle, Error> {
        if !self.procs.contains(subject) {
            return Err(Error::NoSuchProcess);
        }
        self.gc_proc_refs();
        let slot = self
            .proc_refs
            .iter()
            .position(|s| s.is_none())
            .ok_or(Error::ObjectsExhausted)?;
        let obj = self.objects.create(ObjectKind::Process, Owner::KERNEL, slot as u32)?;
        match self
            .procs
            .get_mut(holder)?
            .grant(&mut self.caps, &mut self.objects, obj, rights)
        {
            Ok(h) => {
                self.proc_refs[slot] = Some(subject);
                Ok(h)
            }
            Err(e) => {
                let _ = self.objects.revoke(obj);
                Err(e)
            }
        }
    }

    fn gc_proc_refs(&mut self) {
        for i in 0..MAX_PROC_REFS {
            if self.proc_refs[i].is_some()
                && !self
                    .objects
                    .iter()
                    .any(|(_, r)| r.kind == ObjectKind::Process && r.tag as usize == i)
            {
                self.proc_refs[i] = None;
            }
        }
    }

    /// The process a process-reference handle names (needs `LOOKUP`).
    pub fn process_subject(&mut self, holder: Owner, h: Handle) -> Result<Owner, Error> {
        let (_, cap) = self
            .procs
            .get_mut(holder)?
            .lookup(&self.caps, &self.objects, h, Rights::LOOKUP)?;
        let rec = self.objects.get_kind(cap.object, ObjectKind::Process)?;
        self.proc_refs
            .get(rec.tag as usize)
            .copied()
            .flatten()
            .ok_or(Error::Stale)
    }

    /// Terminates the referenced process's IPC state (needs `DESTROY`).
    pub fn terminate_via_ref<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        holder: Owner,
        h: Handle,
    ) -> Result<usize, Error> {
        let (_, cap) = self
            .procs
            .get_mut(holder)?
            .lookup(&self.caps, &self.objects, h, Rights::DESTROY)?;
        let rec = self.objects.get_kind(cap.object, ObjectKind::Process)?;
        let subject = self
            .proc_refs
            .get(rec.tag as usize)
            .copied()
            .flatten()
            .ok_or(Error::Stale)?;
        self.destroy_process(env, subject)
    }

    /// Revokes every process reference whose subject is `subject` (it is gone).
    pub(crate) fn revoke_process_refs_to(&mut self, subject: Owner) -> Result<(), Error> {
        let mut hit: [Option<ObjectId>; MAX_PROC_REFS] = [None; MAX_PROC_REFS];
        let mut n = 0;
        for (id, r) in self.objects.iter() {
            if r.kind == ObjectKind::Process
                && r.state == crate::d6::object::ObjectState::Active
                && self.proc_refs.get(r.tag as usize).copied().flatten() == Some(subject)
            {
                *hit.get_mut(n).ok_or(Error::Corrupt("too many process refs"))? = Some(id);
                n += 1;
            }
        }
        for id in hit.iter().flatten() {
            let tag = self.objects.get_any(*id)?.tag as usize;
            self.objects.revoke(*id)?;
            self.caps.sweep_object(&mut self.objects, *id)?;
            self.proc_refs[tag] = None;
        }
        Ok(())
    }

    /// Whole-subsystem consistency check for quiescent points.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        self.objects.check_invariants()?;
        self.caps.check_invariants(&self.objects)?;
        self.procs.check_invariants(&self.caps, &self.objects)?;
        // Channels: queue contents pin the channel object; staged capabilities exist in flight.
        let mut staged_total = 0usize;
        for (i, c) in self.channels.iter() {
            let rec = self
                .objects
                .get_any(c.object)
                .map_err(|_| "channel object missing")?;
            if rec.kind != ObjectKind::Channel || rec.tag as usize != i {
                return Err("channel object does not point back");
            }
            let queued = c.queues[0].len() + c.queues[1].len();
            if rec.inflight as usize != queued {
                return Err("channel pins differ from queued messages");
            }
            if c.ends[0].is_none() && c.ends[1].is_none() {
                return Err("channel with both ends closed was not freed");
            }
            for side in 0..2 {
                if c.ends[side].is_none() && !c.queues[side].is_empty() {
                    return Err("messages queued for a closed end");
                }
                if let Some(e) = c.ends[side] {
                    let er = self.objects.get_any(e).map_err(|_| "end object missing")?;
                    if er.kind != ObjectKind::Endpoint {
                        return Err("channel end is not an endpoint");
                    }
                }
                for m in c.queues[side].iter() {
                    // A staged id that is no longer live was revoked while in flight; that is
                    // legal (delivery reports `TransferRevoked`). Live ones must be in flight.
                    for id in m.staged.iter().flatten() {
                        if self.caps.exists(*id) {
                            self.caps
                                .lookup_inflight(&self.objects, *id)
                                .map_err(|_| "queued message holds a non-inflight capability")?;
                            staged_total += 1;
                        }
                    }
                }
            }
        }
        staged_total += self.endpoints.backlog_total();
        if self.caps.held_by(Owner::INFLIGHT) != staged_total {
            return Err("in-flight capability leaked or double-counted");
        }
        self.endpoints.check_invariants(&self.objects, &self.channels)?;
        self.check_waits()?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Shared test rig: an `Ipc` plus a scheduler and sync domain.
    use super::*;
    use crate::sched::Scheduler;
    use crate::sync::SyncDomain;
    use crate::thread::{Handle as Tid, Priority};
    extern crate std;
    use std::boxed::Box;

    pub const N: usize = 16;
    pub const S: usize = SYNC_OBJECTS_NEEDED;

    pub struct Rig {
        pub ipc: Box<Ipc>,
        pub sched: Scheduler<N>,
        pub sync: Box<SyncDomain<N, S>>,
    }

    /// Field-level borrow of the scheduler and sync domain (so `r.ipc` stays usable).
    macro_rules! renv {
        ($r:expr) => {
            $crate::d6::ipc_wait::Env {
                sched: &mut $r.sched,
                sync: &mut $r.sync,
            }
        };
    }
    pub(crate) use renv;

    pub fn p(n: u64) -> Owner {
        Owner::process(n).unwrap()
    }

    impl Rig {
        pub fn new() -> Rig {
            let mut sched = Scheduler::<N>::new();
            sched.init_idle(0, 0).unwrap();
            Rig {
                ipc: Box::new(Ipc::new()),
                sched,
                sync: Box::new(SyncDomain::new()),
            }
        }

        pub fn env(&mut self) -> Env<'_, N, S> {
            Env {
                sched: &mut self.sched,
                sync: &mut self.sync,
            }
        }

        pub fn proc(&mut self, n: u64) -> Owner {
            let o = p(n);
            self.ipc.create_process(o).unwrap();
            o
        }

        pub fn spawn(&mut self, level: u8) -> Tid {
            self.sched.spawn(Priority::new(level).unwrap(), 0, 0).unwrap()
        }

        /// schedule() + switch_complete(), like the architecture layer.
        pub fn run_once(&mut self) -> Tid {
            let sw = self.sched.schedule().unwrap();
            if sw.is_switch()
                && let Some(prev) = sw.prev
            {
                self.sched.switch_complete(prev).unwrap();
            }
            self.sched.check_invariants().unwrap();
            sw.next
        }

        pub fn run_until(&mut self, t: Tid) {
            for _ in 0..64 {
                if self.sched.current() == Some(t) {
                    return;
                }
                self.run_once();
            }
            panic!("thread never became current");
        }

        /// A connected pair of endpoints, one handle in each process.
        pub fn pair(&mut self, a: Owner, b: Owner) -> (Handle, Handle) {
            let mut env = Env {
                sched: &mut self.sched,
                sync: &mut self.sync,
            };
            self.ipc.create_pair_handles(&mut env, a, b).unwrap()
        }

        pub fn check(&self) {
            self.ipc.check_invariants().unwrap();
            self.sync.check_invariants(&self.sched).unwrap();
        }
    }

    pub fn msg(sender: Owner, tag: u64, payload: &[u8]) -> Message {
        Message::new(
            sender,
            crate::d6::ipc_message::MsgKind::Data,
            crate::d6::ipc_message::MsgFlags::NONE,
            tag,
            payload,
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use crate::d6::ipc_message::MsgKind;

    #[test]
    fn queue_is_a_bounded_fifo_ring() {
        let mut q = Queue::new();
        assert!(q.is_empty() && q.pop().is_none() && q.front().is_none());
        for round in 0..3u64 {
            for i in 0..QUEUE_CAPACITY as u64 {
                q.push(msg(p(1), round * 100 + i, b"")).unwrap();
            }
            assert!(q.is_full());
            assert_eq!(q.push(msg(p(1), 999, b"")), Err(Error::QueueFull));
            assert_eq!(q.len(), QUEUE_CAPACITY);
            assert_eq!(q.iter().count(), QUEUE_CAPACITY);
            for i in 0..QUEUE_CAPACITY as u64 {
                assert_eq!(q.pop().unwrap().tag, round * 100 + i, "FIFO across wrap-around");
            }
            assert!(q.is_empty());
        }
    }

    #[test]
    fn send_receive_fifo_and_message_ids() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        for i in 0..5u64 {
            let m = msg(a, i, &[i as u8; 4]);
            let id = r.ipc.send(&mut renv!(r), a, ha, m).unwrap();
            assert_eq!(id, MsgId(i + 1));
        }
        assert_eq!(r.ipc.pending(b, hb), Ok(5));
        for i in 0..5u64 {
            let m = r.ipc.recv(&mut renv!(r), b, hb).unwrap();
            assert_eq!(m.tag, i, "FIFO");
            assert_eq!(m.id, MsgId(i + 1));
            assert_eq!(m.payload(), &[i as u8; 4]);
            assert_eq!(m.sender, a, "sender identity is the sending process");
        }
        assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).err(), Some(Error::QueueEmpty));
        // The other direction is independent.
        r.ipc.send(&mut renv!(r), b, hb, msg(b, 77, b"back")).unwrap();
        assert_eq!(r.ipc.recv(&mut renv!(r), a, ha).unwrap().tag, 77);
        r.check();
    }

    #[test]
    fn sender_identity_cannot_be_forged_through_the_message_struct() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        // The caller builds a message claiming to be process 2 (or the kernel).
        for forged in [b, Owner::KERNEL] {
            let m = msg(forged, 1, b"x");
            r.ipc.send(&mut renv!(r), a, ha, m).unwrap();
            assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).unwrap().sender, a);
        }
        // Kernel-only message kinds cannot be sent at all.
        let notice = Message::new(a, MsgKind::KernelNotice, crate::d6::ipc_message::MsgFlags::NONE, 0, b"")
            .unwrap();
        assert_eq!(r.ipc.send(&mut renv!(r), a, ha, notice), Err(Error::BadKind));
    }

    #[test]
    fn rights_are_enforced_per_direction() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let send_only = r.ipc.duplicate_handle(a, ha, Rights::SEND).unwrap();
        let recv_only = r.ipc.duplicate_handle(b, hb, Rights::RECV).unwrap();
        assert_eq!(r.ipc.recv(&mut renv!(r), a, send_only).err(), Some(Error::RightsDenied));
        assert_eq!(
            r.ipc.send(&mut renv!(r), b, recv_only, msg(b, 0, b"")),
            Err(Error::RightsDenied)
        );
        r.ipc.send(&mut renv!(r), a, send_only, msg(a, 5, b"")).unwrap();
        assert_eq!(r.ipc.recv(&mut renv!(r), b, recv_only).unwrap().tag, 5);
        // Another process's handle value is useless.
        let c = r.proc(3);
        assert!(r.ipc.send(&mut renv!(r), c, ha, msg(c, 0, b"")).is_err());
        assert!(r.ipc.recv(&mut renv!(r), c, hb).is_err());
        r.check();
    }

    #[test]
    fn full_queue_returns_an_error_and_loses_nothing() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        for i in 0..QUEUE_CAPACITY as u64 {
            r.ipc.send(&mut renv!(r), a, ha, msg(a, i, b"")).unwrap();
        }
        for _ in 0..3 {
            assert_eq!(r.ipc.send(&mut renv!(r), a, ha, msg(a, 99, b"")), Err(Error::QueueFull));
        }
        assert_eq!(r.ipc.stats().queue_full, 3);
        for i in 0..QUEUE_CAPACITY as u64 {
            assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).unwrap().tag, i);
        }
        assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).err(), Some(Error::QueueEmpty));
        // Space is available again.
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 100, b"")).unwrap();
        assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).unwrap().tag, 100);
        r.check();
    }

    #[test]
    fn oversized_and_malformed_wire_messages_are_rejected_before_queueing() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let mut w = std::vec![1u8, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        w.extend_from_slice(&[0u8; 4]);
        assert_eq!(r.ipc.send_wire(&mut renv!(r), a, ha, &w), Err(Error::TrailingBytes));
        w.truncate(16);
        assert!(r.ipc.send_wire(&mut renv!(r), a, ha, &w).is_ok());
        w[4] = 0xFF;
        assert_eq!(r.ipc.send_wire(&mut renv!(r), a, ha, &w), Err(Error::PayloadTooLarge));
        assert_eq!(r.ipc.send_wire(&mut renv!(r), a, ha, &[]), Err(Error::Truncated));
        assert_eq!(r.ipc.pending(b, hb), Ok(1), "only the valid message was queued");
        r.check();
    }

    #[test]
    fn peer_close_is_detected_after_the_queue_drains() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"last words")).unwrap();
        r.ipc.close_endpoint(&mut renv!(r), a, ha).unwrap();
        // Messages sent before the close are still readable...
        assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).unwrap().payload(), b"last words");
        // ...then the closure is reported, and sending to the dead peer fails.
        assert_eq!(r.ipc.recv(&mut renv!(r), b, hb).err(), Some(Error::PeerClosed));
        assert_eq!(r.ipc.send(&mut renv!(r), b, hb, msg(b, 2, b"")), Err(Error::PeerClosed));
        assert_eq!(r.ipc.channels().live(), 1);
        r.check();
        r.ipc.close_endpoint(&mut renv!(r), b, hb).unwrap();
        assert_eq!(r.ipc.channels().live(), 0, "both ends closed frees the channel");
        assert_eq!(r.ipc.objects().live(), 0, "no object outlives its users");
        r.check();
    }

    #[test]
    fn closing_an_end_discards_messages_queued_for_it_and_counts_them() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        for i in 0..3 {
            r.ipc.send(&mut renv!(r), a, ha, msg(a, i, b"")).unwrap();
        }
        r.ipc.close_endpoint(&mut renv!(r), b, hb).unwrap();
        assert_eq!(r.ipc.stats().discarded_on_close, 3);
        assert_eq!(r.ipc.send(&mut renv!(r), a, ha, msg(a, 9, b"")), Err(Error::PeerClosed));
        r.check();
    }

    #[test]
    fn channel_slots_are_bounded_and_reusable() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let mut held = std::vec::Vec::new();
        loop {
            let mut env = Env { sched: &mut r.sched, sync: &mut r.sync };
            match r.ipc.create_pair_handles(&mut env, a, b) {
                Ok(p) => held.push(p),
                Err(e) => {
                    assert!(
                        matches!(
                            e,
                            Error::ChannelsExhausted
                                | Error::HandlesExhausted
                                | Error::QuotaExceeded
                                | Error::EndpointsExhausted
                                | Error::CapsExhausted
                                | Error::ObjectsExhausted
                                | Error::WaitQueuesExhausted
                        ),
                        "{e:?}"
                    );
                    break;
                }
            }
            r.check();
        }
        assert!(!held.is_empty());
        r.check();
        let n = held.len();
        for (ha, hb) in held {
            r.ipc.close_endpoint(&mut renv!(r), a, ha).unwrap();
            r.ipc.close_endpoint(&mut renv!(r), b, hb).unwrap();
        }
        assert_eq!(r.ipc.channels().live(), 0);
        assert_eq!(r.ipc.objects().live(), 0);
        let (ha, hb) = r.pair(a, b);
        r.ipc.send(&mut renv!(r), a, ha, msg(a, 1, b"")).unwrap();
        assert!(r.ipc.recv(&mut renv!(r), b, hb).is_ok());
        assert!(n >= 4);
        r.check();
    }

    // ---- randomised lifecycle + stress ---------------------------------------------------------

    /// Seeds are recorded here and printed on failure so any failing sequence replays exactly.
    const LIFECYCLE_SEEDS: [u64; 6] = [
        0x6C69_6665_6379_636C, // "lifecycl"
        0x1,
        0x2,
        0xDEAD_BEEF,
        0x0123_4567_89AB_CDEF,
        0xFFFF_FFFF_FFFF_FFFF,
    ];

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// A random mix of every public operation (valid and invalid) over several processes.
    /// Whatever happens, no call may panic and the invariants of the whole subsystem
    /// (objects, capabilities, handles, queues, waits, scheduler, sync) must hold after every step.
    #[test]
    fn randomized_lifecycle_preserves_invariants() {
        use crate::d6::ipc_message::TransferRequest;
        for seed in LIFECYCLE_SEEDS {
            let mut rng = Rng(seed);
            let mut r = Rig::new();
            let owners: std::vec::Vec<Owner> = (1..=4u64).map(|n| r.proc(n)).collect();
            let mut handles: std::vec::Vec<std::vec::Vec<Handle>> = owners.iter().map(|_| std::vec::Vec::new()).collect();
            let mut dead = [false; 4];
            let (mut sends, mut recvs, mut xfers) = (0u32, 0u32, 0u32);
            for step in 0..3000u32 {
                let ctx = format!("seed {seed:#x} step {step}");
                let a = rng.below(4);
                if dead[a] {
                    // Revive: a destroyed process id can be created again, fresh.
                    r.ipc.create_process(owners[a]).unwrap_or_else(|e| panic!("{ctx}: revive {e:?}"));
                    dead[a] = false;
                    handles[a].clear();
                }
                let oa = owners[a];
                // Drop tracked handles that no longer resolve (peer process died, revoked, moved).
                for (i, o) in owners.iter().enumerate() {
                    if !dead[i] {
                        handles[i].retain(|h| r.ipc.rights_of(*o, *h).is_ok());
                    }
                }
                let pick = |rng: &mut Rng, hs: &std::vec::Vec<Handle>| -> Handle {
                    // Mostly a live handle, sometimes garbage.
                    if hs.is_empty() || rng.below(8) == 0 {
                        Handle::from_raw(rng.next() as u32)
                    } else {
                        hs[rng.below(hs.len())]
                    }
                };
                match rng.below(12) {
                    0 | 1 => {
                        let b = rng.below(4);
                        if !dead[b] && r.ipc.create_pair_handles(&mut renv!(r), oa, owners[b]).map(|(x, y)| {
                            handles[a].push(x);
                            handles[b].push(y);
                        }).is_err() {}
                    }
                    2 | 3 => {
                        let h = pick(&mut rng, &handles[a]);
                        let mut m = msg(oa, step as u64, b"s");
                        if rng.below(3) == 0 {
                            let t = pick(&mut rng, &handles[a]);
                            if t != h
                                && let Ok(x) = m.clone().with_transfers(&[TransferRequest { handle: t.raw(), rights: if rng.below(2) == 0 { Rights::SEND.union(Rights::RECV) } else { Rights::NONE } }])
                            {
                                m = x;
                                if rng.below(2) == 0 && let Ok(mv) = m.clone().with_move() {
                                    m = mv;
                                }
                            }
                        }
                        if r.ipc.send(&mut renv!(r), oa, h, m).is_ok() {
                            sends += 1;
                        }
                    }
                    4 | 5 => {
                        let h = pick(&mut rng, &handles[a]);
                        if let Ok(m) = r.ipc.recv(&mut renv!(r), oa, h) {
                            recvs += 1;
                            xfers += m.transfers().len() as u32;
                            for t in m.transfers() {
                                handles[a].push(Handle::from_raw(t.handle));
                            }
                        }
                    }
                    6 | 10 => {
                        let h = pick(&mut rng, &handles[a]);
                        if r.ipc.close_handle(&mut renv!(r), oa, h).is_ok() {
                            handles[a].retain(|x| *x != h);
                        }
                    }
                    7 => {
                        let h = pick(&mut rng, &handles[a]);
                        let mask = if rng.below(2) == 0 {
                            Rights::ALL
                        } else {
                            Rights::from_bits(rng.next() as u32 & Rights::ALL.bits()).unwrap()
                        };
                        if let Ok(d) = r.ipc.duplicate_handle(oa, h, mask) {
                            handles[a].push(d);
                        }
                    }
                    8 => {
                        let h = pick(&mut rng, &handles[a]);
                        let _ = r.ipc.revoke_derived(&mut renv!(r), oa, h);
                    }
                    9 => {
                        if let Ok(l) = r.ipc.create_listener(&mut renv!(r), oa) {
                            handles[a].push(l);
                        }
                        let h = pick(&mut rng, &handles[a]);
                        if let Ok((c, _)) = r.ipc.accept(&mut renv!(r), oa, h) {
                            handles[a].push(c);
                        }
                    }
                    _ => {
                        if rng.below(60) == 0 {
                            r.ipc.destroy_process(&mut renv!(r), oa).unwrap_or_else(|e| panic!("{ctx}: destroy {e:?}"));
                            dead[a] = true;
                        }
                    }
                }
                // Handles that died with a peer's process or a revocation are pruned lazily: the
                // tracker may hold stale values, which the operations above must tolerate.
                r.ipc.check_invariants().unwrap_or_else(|e| panic!("{ctx}: {e}"));
                if step % 100 == 0 {
                    r.sync.check_invariants(&r.sched).unwrap_or_else(|e| panic!("{ctx}: sync {e:?}"));
                }
            }
            eprintln!("seed {seed:#x}: sends {sends} recvs {recvs} xfers {xfers}");
            assert!(sends > 50 && recvs > 20, "seed {seed:#x}: the run must exercise IPC (sends {sends}, recvs {recvs})");
            assert!(xfers > 0, "seed {seed:#x}: the run must exercise transfers");
            for (i, o) in owners.iter().enumerate() {
                if !dead[i] {
                    r.ipc.destroy_process(&mut renv!(r), *o).unwrap();
                }
            }
            r.check();
            assert_eq!(r.ipc.objects().live(), 0, "seed {seed:#x}: objects leaked");
            assert_eq!(r.ipc.caps().live(), 0, "seed {seed:#x}: capabilities leaked");
            assert_eq!(r.ipc.channels().live(), 0, "seed {seed:#x}: channels leaked");
        }
    }

    /// Thousands of create / transfer / close / destroy cycles with exact resource accounting:
    /// after every cycle the subsystem is back to zero objects, capabilities and channels.
    #[test]
    fn stress_cycles_return_every_resource() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        for cycle in 0..5000u64 {
            let (ha, hb) = r.pair(a, b);
            let (xa, xb) = r.pair(a, a);
            let t = crate::d6::ipc_message::TransferRequest { handle: xb.raw(), rights: Rights::SEND.union(Rights::RECV) };
            let m = msg(a, cycle, b"x").with_transfers(&[t]).unwrap();
            let m = if cycle % 2 == 0 { m.with_move().unwrap() } else { m };
            r.ipc.send(&mut renv!(r), a, ha, m).unwrap();
            let got = r.ipc.recv(&mut renv!(r), b, hb).unwrap();
            assert_eq!(got.transfers().len(), 1);
            // The transferred end carries data back to xa across the process boundary.
            let tb = Handle::from_raw(got.transfers()[0].handle);
            r.ipc.send(&mut renv!(r), b, tb, msg(b, cycle, b"back")).unwrap();
            assert_eq!(r.ipc.recv(&mut renv!(r), a, xa).unwrap().payload(), b"back");
            match cycle % 3 {
                0 => {
                    r.ipc.close_handle(&mut renv!(r), a, ha).unwrap();
                    r.ipc.close_handle(&mut renv!(r), b, hb).unwrap();
                    r.ipc.close_handle(&mut renv!(r), a, xa).unwrap();
                    r.ipc.close_handle(&mut renv!(r), b, tb).unwrap();
                    if cycle % 2 != 0 {
                        r.ipc.close_handle(&mut renv!(r), a, xb).unwrap();
                    }
                }
                1 => {
                    // Whole-process teardown releases everything, then the process is recreated.
                    r.ipc.destroy_process(&mut renv!(r), a).unwrap();
                    r.ipc.destroy_process(&mut renv!(r), b).unwrap();
                    r.ipc.create_process(a).unwrap();
                    r.ipc.create_process(b).unwrap();
                }
                _ => {
                    r.ipc.destroy_process(&mut renv!(r), b).unwrap();
                    r.ipc.destroy_process(&mut renv!(r), a).unwrap();
                    r.ipc.create_process(b).unwrap();
                    r.ipc.create_process(a).unwrap();
                }
            }
            if cycle % 250 == 0 {
                r.check();
            }
            assert_eq!(r.ipc.objects().live(), 0, "cycle {cycle}");
            assert_eq!(r.ipc.caps().live(), 0, "cycle {cycle}");
            assert_eq!(r.ipc.channels().live(), 0, "cycle {cycle}");
        }
        r.check();
    }
}
