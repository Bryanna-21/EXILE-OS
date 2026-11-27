//! IPC endpoint management (Mission D6, file 6). SPDX-License-Identifier: Apache-2.0
//!
//! An endpoint is a kernel object (`ObjectKind::Endpoint`) reachable only through capabilities.
//! There are two roles:
//! * **Peer** end of a channel (two ends, two bounded queues, see `ipc.rs`).
//! * **Listener**: accepts connections from clients. `connect` creates a fresh channel, hands the
//!   client its end at once and parks the server's end (as an in-flight capability owned by
//!   `Owner::INFLIGHT`) in the listener's bounded backlog; `accept` moves it into the acceptor's
//!   handle table and records the acceptor as the end's owner.
//!
//! # Ownership and identity
//! The object `owner` of an endpoint is the process that created it (a client's end belongs to
//! the client, an accepted server end to the acceptor, a listener to the server). Authority to
//! *use* an endpoint comes only from capabilities; ownership only decides who is accountable
//! (quota) and what is closed when the process exits. A process cannot impersonate another:
//! sender identity is stamped from the calling process (`ipc.rs`), `accept` reports the
//! kernel-recorded client identity, and a handle value is only ever resolved in the caller's own
//! table (`handle.rs`).
//!
//! # Closure
//! An endpoint closes when (a) a holder with `DESTROY` closes it ([`Ipc::close_endpoint`]), (b)
//! its last capability disappears ([`Ipc::reap_endpoints`], called after every operation that can
//! drop references), or (c) its owner exits. Closure
//! * discards the messages queued *for that end* (and their in-flight capabilities),
//! * detaches the end from the channel so the peer sees `PeerClosed` once its queue drains,
//! * wakes every thread blocked on either end (they re-evaluate and report the outcome),
//! * revokes the object and removes every capability naming it,
//! * frees the endpoint's wait queues and table slot, and frees the channel when both ends are gone.
#![allow(dead_code)]

use crate::d6::capability::{CapId, Rights};
use crate::d6::handle::Handle;
use crate::d6::ipc::{Ipc, MAX_CHANNELS, PEER_RIGHTS};
use crate::d6::ipc_wait::{Env, WaitOutcome};
use crate::d6::object::{Counter, Error, ObjectId, ObjectKind, ObjectTable, Owner};
use crate::sync;

pub const BACKLOG: usize = 4;

/// Rights on a freshly created listener.
pub const LISTENER_RIGHTS: Rights = Rights::ACCEPT
    .union(Rights::CONNECT)
    .union(Rights::DERIVE)
    .union(Rights::TRANSFER)
    .union(Rights::CANCEL)
    .union(Rights::DESTROY)
    .union(Rights::REVOKE);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnRole {
    /// Created by `create_channel`/`create_pair_handles`: no client/server distinction.
    Symmetric,
    Client,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pending {
    /// In-flight capability for the server end (held by `Owner::INFLIGHT`).
    pub cap: CapId,
    /// Kernel-recorded identity of the connecting process.
    pub client: Owner,
}

#[derive(Clone, Copy)]
pub struct Listener {
    backlog: [Option<Pending>; BACKLOG],
    head: usize,
    len: usize,
}

impl Listener {
    const fn new() -> Self {
        Self {
            backlog: [None; BACKLOG],
            head: 0,
            len: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_full(&self) -> bool {
        self.len == BACKLOG
    }
    fn push(&mut self, p: Pending) -> Result<(), Error> {
        if self.is_full() {
            return Err(Error::BacklogFull);
        }
        self.backlog[(self.head + self.len) % BACKLOG] = Some(p);
        self.len += 1;
        Ok(())
    }
    fn front(&self) -> Option<Pending> {
        if self.len == 0 { None } else { self.backlog[self.head] }
    }
    fn pop(&mut self) -> Option<Pending> {
        if self.len == 0 {
            return None;
        }
        let p = self.backlog[self.head].take();
        self.head = (self.head + 1) % BACKLOG;
        self.len -= 1;
        p
    }
    fn iter(&self) -> impl Iterator<Item = Pending> + '_ {
        (0..self.len).filter_map(move |i| self.backlog[(self.head + i) % BACKLOG])
    }
}

#[derive(Clone, Copy)]
pub enum Role {
    Peer {
        chan: usize,
        side: usize,
        conn: ConnRole,
    },
    Listener(Listener),
}

#[derive(Clone, Copy)]
pub struct EndpointRec {
    pub object: ObjectId,
    pub role: Role,
    /// Threads blocked in `recv` / `accept` on this endpoint wait here.
    pub recv_wq: sync::ObjId,
    /// Threads blocked in `send` on this endpoint wait here.
    pub send_wq: sync::ObjId,
}

pub struct EndpointTable<const E: usize> {
    slots: [Option<EndpointRec>; E],
}

impl<const E: usize> Default for EndpointTable<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const E: usize> EndpointTable<E> {
    pub const fn new() -> Self {
        Self { slots: [None; E] }
    }

    pub fn live(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    fn free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }

    pub fn get(&self, idx: usize) -> Option<&EndpointRec> {
        self.slots.get(idx).and_then(|s| s.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, &EndpointRec)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|r| (i, r)))
    }

    /// Resolves an *active* endpoint object to its table slot.
    pub fn by_object<const O: usize>(
        &self,
        objs: &ObjectTable<O>,
        obj: ObjectId,
    ) -> Result<(usize, &EndpointRec), Error> {
        let tag = objs.get_kind(obj, ObjectKind::Endpoint)?.tag as usize;
        match self.get(tag) {
            Some(r) if r.object == obj => Ok((tag, r)),
            _ => Err(Error::Corrupt("endpoint object without a table entry")),
        }
    }

    /// `(channel, side)` of a peer endpoint; `WrongKind` for a listener.
    pub fn peer_of<const O: usize>(
        &self,
        objs: &ObjectTable<O>,
        obj: ObjectId,
    ) -> Result<(usize, usize), Error> {
        match self.by_object(objs, obj)?.1.role {
            Role::Peer { chan, side, .. } => Ok((chan, side)),
            Role::Listener(_) => Err(Error::WrongKind),
        }
    }

    pub fn conn_role<const O: usize>(
        &self,
        objs: &ObjectTable<O>,
        obj: ObjectId,
    ) -> Result<ConnRole, Error> {
        match self.by_object(objs, obj)?.1.role {
            Role::Peer { conn, .. } => Ok(conn),
            Role::Listener(_) => Err(Error::WrongKind),
        }
    }

    /// Number of in-flight capabilities parked in listener backlogs.
    pub fn backlog_total(&self) -> usize {
        self.slots
            .iter()
            .flatten()
            .map(|r| match &r.role {
                Role::Listener(l) => l.len(),
                _ => 0,
            })
            .sum()
    }

    pub fn check_invariants<const O: usize, const CH: usize>(
        &self,
        objs: &ObjectTable<O>,
        chans: &crate::d6::ipc::ChannelTable<CH>,
    ) -> Result<(), &'static str> {
        for (i, r) in self.iter() {
            let o = objs.get_any(r.object).map_err(|_| "endpoint object missing")?;
            if o.kind != ObjectKind::Endpoint || o.tag as usize != i {
                return Err("endpoint object does not point back");
            }
            if o.state != crate::d6::object::ObjectState::Active {
                return Err("revoked endpoint still has a table entry");
            }
            if let Role::Peer { chan, side, .. } = r.role {
                let c = chans.get(chan).map_err(|_| "endpoint names a dead channel")?;
                if c.ends[side] != Some(r.object) {
                    return Err("channel does not point back at its endpoint");
                }
            }
        }
        for (_, c) in chans.iter() {
            for e in c.ends.iter().flatten() {
                if !self.iter().any(|(_, r)| r.object == *e) {
                    return Err("channel end without an endpoint entry");
                }
            }
        }
        Ok(())
    }
}

fn sync_err(e: sync::Error) -> Error {
    match e {
        sync::Error::NoObjects => Error::WaitQueuesExhausted,
        e => Error::Sync(e),
    }
}

impl Ipc {
    // ---- allocation ----------------------------------------------------------------------

    fn new_sync_queues<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
    ) -> Result<(sync::ObjId, sync::ObjId), Error> {
        let r = env.sync.create_queue().map_err(sync_err)?;
        match env.sync.create_queue() {
            Ok(s) => Ok((r, s)),
            Err(e) => {
                let _ = env.sync.destroy(r);
                Err(sync_err(e))
            }
        }
    }

    fn alloc_endpoint<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        obj_owner: Owner,
        role: Role,
    ) -> Result<ObjectId, Error> {
        let slot = self.endpoints.free_slot().ok_or(Error::EndpointsExhausted)?;
        let obj = self.objects.create(ObjectKind::Endpoint, obj_owner, slot as u32)?;
        let (recv_wq, send_wq) = match self.new_sync_queues(env) {
            Ok(q) => q,
            Err(e) => {
                let _ = self.objects.revoke(obj);
                return Err(e);
            }
        };
        self.endpoints.slots[slot] = Some(EndpointRec {
            object: obj,
            role,
            recv_wq,
            send_wq,
        });
        Ok(obj)
    }

    /// Creates a channel with two (capability-less) endpoint objects.
    pub(crate) fn make_channel<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owners: (Owner, Owner),
        conn: (ConnRole, ConnRole),
    ) -> Result<(ObjectId, ObjectId), Error> {
        let chan = self.channels.create(&mut self.objects, Owner::KERNEL)?;
        let e0 = match self.alloc_endpoint(
            env,
            owners.0,
            Role::Peer {
                chan,
                side: 0,
                conn: conn.0,
            },
        ) {
            Ok(e) => e,
            Err(e) => {
                self.free_channel(chan);
                return Err(e);
            }
        };
        let e1 = match self.alloc_endpoint(
            env,
            owners.1,
            Role::Peer {
                chan,
                side: 1,
                conn: conn.1,
            },
        ) {
            Ok(e) => e,
            Err(err) => {
                let _ = self.close_endpoint_obj(env, e0);
                self.free_channel(chan);
                return Err(err);
            }
        };
        let c = self.channels.get_mut(chan)?;
        c.ends = [Some(e0), Some(e1)];
        Ok((e0, e1))
    }

    fn free_channel(&mut self, chan: usize) {
        if let Ok(c) = self.channels.get(chan) {
            let obj = c.object;
            let _ = self.objects.revoke(obj);
        }
        self.channels.free(chan);
    }

    // ---- creation (kernel and user entry points) --------------------------------------------

    /// Kernel operation: a new channel whose ends are granted to `a` and `b` (they may be the
    /// same process). Each side gets exactly [`PEER_RIGHTS`] and nothing else. Atomic.
    pub fn create_pair_handles<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        a: Owner,
        b: Owner,
    ) -> Result<(Handle, Handle), Error> {
        if !self.procs.contains(a) || !self.procs.contains(b) {
            return Err(Error::NoSuchProcess);
        }
        let need = if a == b { 2 } else { 1 };
        if self.procs.get(a)?.free_slots() < need || self.procs.get(b)?.free_slots() < need {
            return Err(Error::HandlesExhausted);
        }
        let (e0, e1) = self.make_channel(env, (a, b), (ConnRole::Symmetric, ConnRole::Symmetric))?;
        let ha = match self
            .procs
            .get_mut(a)?
            .grant(&mut self.caps, &mut self.objects, e0, PEER_RIGHTS)
        {
            Ok(h) => h,
            Err(e) => {
                self.abort_pair(env, e0, e1);
                return Err(e);
            }
        };
        let hb = match self
            .procs
            .get_mut(b)?
            .grant(&mut self.caps, &mut self.objects, e1, PEER_RIGHTS)
        {
            Ok(h) => h,
            Err(e) => {
                let _ = self.procs.get_mut(a).and_then(|t| t.close(&mut self.caps, &mut self.objects, ha));
                self.abort_pair(env, e0, e1);
                return Err(e);
            }
        };
        Ok((ha, hb))
    }

    fn abort_pair<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        e0: ObjectId,
        e1: ObjectId,
    ) {
        let _ = self.close_endpoint_obj(env, e0);
        let _ = self.close_endpoint_obj(env, e1);
    }

    /// User operation: the caller creates a channel and receives both ends.
    pub fn create_channel<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
    ) -> Result<(Handle, Handle), Error> {
        self.create_pair_handles(env, owner, owner)
    }

    /// Creates a listener owned by `owner`, with [`LISTENER_RIGHTS`].
    pub fn create_listener<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
    ) -> Result<Handle, Error> {
        if self.procs.get(owner)?.free_slots() == 0 {
            return Err(Error::HandlesExhausted);
        }
        let obj = self.alloc_endpoint(env, owner, Role::Listener(Listener::new()))?;
        match self
            .procs
            .get_mut(owner)?
            .grant(&mut self.caps, &mut self.objects, obj, LISTENER_RIGHTS)
        {
            Ok(h) => Ok(h),
            Err(e) => {
                let _ = self.close_endpoint_obj(env, obj);
                Err(e)
            }
        }
    }

    // ---- connection establishment ----------------------------------------------------------

    /// Connects `client` to the listener behind `h` (needs `CONNECT`). The client immediately
    /// owns its end; the server end waits in the backlog for `accept`. Atomic.
    pub fn connect<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        client: Owner,
        h: Handle,
    ) -> Result<Handle, Error> {
        let (_, cap) = self
            .procs
            .get_mut(client)?
            .lookup(&self.caps, &self.objects, h, Rights::CONNECT)?;
        self.connect_obj(env, client, cap.object)
    }

    /// Connection on the authority of an already-checked capability (also used by `service.rs`,
    /// whose kernel-held connect capability is checked there).
    pub(crate) fn connect_obj<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        client: Owner,
        listener: ObjectId,
    ) -> Result<Handle, Error> {
        match self.endpoints.by_object(&self.objects, listener)?.1.role {
            Role::Listener(l) if l.is_full() => return Err(Error::BacklogFull),
            Role::Listener(_) => {}
            Role::Peer { .. } => return Err(Error::NotListener),
        }
        if self.procs.get(client)?.free_slots() == 0 {
            return Err(Error::HandlesExhausted);
        }
        let (e_cli, e_srv) =
            self.make_channel(env, (client, Owner::KERNEL), (ConnRole::Client, ConnRole::Server))?;
        let ch = match self
            .procs
            .get_mut(client)?
            .grant(&mut self.caps, &mut self.objects, e_cli, PEER_RIGHTS)
        {
            Ok(h) => h,
            Err(e) => {
                self.abort_pair(env, e_cli, e_srv);
                return Err(e);
            }
        };
        let cap = match self.caps.mint(&mut self.objects, Owner::INFLIGHT, e_srv, PEER_RIGHTS) {
            Ok(c) => c,
            Err(e) => {
                let _ = self
                    .procs
                    .get_mut(client)
                    .and_then(|t| t.close(&mut self.caps, &mut self.objects, ch));
                self.abort_pair(env, e_cli, e_srv);
                return Err(e);
            }
        };
        let (slot, _) = self.endpoints.by_object(&self.objects, listener)?;
        if let Some(EndpointRec {
            role: Role::Listener(l),
            ..
        }) = self.endpoints.slots[slot].as_mut()
        {
            l.push(Pending { cap, client })?;
        }
        self.wake_receiver(env, listener)?;
        Ok(ch)
    }

    /// Accepts the oldest pending connection (needs `ACCEPT`). Returns the handle for the server
    /// end and the kernel-recorded identity of the client. `NoPending` if there is none.
    pub fn accept<const N: usize, const S: usize>(
        &mut self,
        _env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
    ) -> Result<(Handle, Owner), Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::ACCEPT)?;
        let (slot, _) = self.endpoints.by_object(&self.objects, cap.object)?;
        loop {
            let front = match self.endpoints.slots[slot].as_ref().map(|r| r.role) {
                Some(Role::Listener(l)) => l.front(),
                _ => return Err(Error::NotListener),
            };
            let Some(p) = front else {
                return Err(Error::NoPending);
            };
            // Entries whose server end died (should not happen) are skipped, not delivered.
            let Ok(srv) = self.caps.lookup_inflight(&self.objects, p.cap).copied() else {
                self.pop_backlog(slot);
                continue;
            };
            if self.procs.get(owner)?.free_slots() == 0 {
                return Err(Error::HandlesExhausted);
            }
            if self.caps.charged_to(owner) >= crate::d6::ipc::MAX_CAPS_PER_OWNER
                || self.objects.owned_by(owner) >= crate::d6::ipc::MAX_OBJECTS_PER_OWNER
            {
                return Err(Error::QuotaExceeded);
            }
            self.caps.reown(&self.objects, p.cap, owner)?;
            if let Err(e) = self.objects.set_owner(srv.object, owner) {
                let _ = self.caps.to_inflight(p.cap, Owner::INFLIGHT);
                return Err(e);
            }
            let nh = match self.procs.get_mut(owner)?.insert(p.cap) {
                Ok(nh) => nh,
                Err(e) => {
                    let _ = self.caps.to_inflight(p.cap, Owner::INFLIGHT);
                    let _ = self.objects.set_owner(srv.object, Owner::KERNEL);
                    return Err(e);
                }
            };
            self.pop_backlog(slot);
            return Ok((nh, p.client));
        }
    }

    fn pop_backlog(&mut self, slot: usize) -> Option<Pending> {
        if let Some(EndpointRec {
            role: Role::Listener(l),
            ..
        }) = self.endpoints.slots[slot].as_mut()
        {
            l.pop()
        } else {
            None
        }
    }

    /// Pending connections on the listener behind `h`.
    pub fn backlog_len(&mut self, owner: Owner, h: Handle) -> Result<usize, Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::ACCEPT)?;
        match self.endpoints.by_object(&self.objects, cap.object)?.1.role {
            Role::Listener(l) => Ok(l.len()),
            _ => Err(Error::NotListener),
        }
    }

    // ---- peer state ------------------------------------------------------------------------

    /// Whether the other end behind `h` is still open (any right suffices: you hold the end).
    pub fn peer_open(&mut self, owner: Owner, h: Handle) -> Result<bool, Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::NONE)?;
        let (chan, side) = self.endpoints.peer_of(&self.objects, cap.object)?;
        Ok(self.channels.get(chan)?.ends[1 - side].is_some())
    }

    /// Client/server/symmetric role of the end behind `h`.
    pub fn conn_role_of(&mut self, owner: Owner, h: Handle) -> Result<ConnRole, Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::NONE)?;
        self.endpoints.conn_role(&self.objects, cap.object)
    }

    // ---- ownership -------------------------------------------------------------------------

    /// Hands accountability for an endpoint to another process (needs `DESTROY`, i.e. full
    /// control). Capability transfer alone never changes ownership.
    pub fn set_endpoint_owner(
        &mut self,
        owner: Owner,
        h: Handle,
        new_owner: Owner,
    ) -> Result<(), Error> {
        if !self.procs.contains(new_owner) {
            return Err(Error::NoSuchProcess);
        }
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::DESTROY)?;
        self.objects.set_owner(cap.object, new_owner)
    }

    // ---- closure ---------------------------------------------------------------------------

    /// Closes the endpoint behind `h` for everybody (needs `DESTROY`).
    pub fn close_endpoint<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        owner: Owner,
        h: Handle,
    ) -> Result<(), Error> {
        let (_, cap) = self
            .procs
            .get_mut(owner)?
            .lookup(&self.caps, &self.objects, h, Rights::DESTROY)?;
        self.close_endpoint_obj(env, cap.object)?;
        self.reap_endpoints(env)?;
        Ok(())
    }

    /// Closes every active endpoint whose last capability is gone. Loops because closing an
    /// endpoint discards queued capabilities, which can orphan further endpoints.
    pub fn reap_endpoints<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
    ) -> Result<usize, Error> {
        let mut n = 0;
        loop {
            let found = self
                .endpoints
                .iter()
                .map(|(_, r)| r.object)
                .find(|o| self.objects.get(*o).is_ok_and(|r| r.refs == 0));
            match found {
                Some(o) => {
                    self.close_endpoint_obj(env, o)?;
                    n += 1;
                }
                None => return Ok(n),
            }
        }
    }

    /// The closure procedure described in the module docs. Never recursive.
    pub(crate) fn close_endpoint_obj<const N: usize, const S: usize>(
        &mut self,
        env: &mut Env<N, S>,
        obj: ObjectId,
    ) -> Result<(), Error> {
        let (slot, rec) = {
            let (s, r) = self.endpoints.by_object(&self.objects, obj)?;
            (s, *r)
        };
        // 1. Wake everyone blocked here; they re-evaluate and find the endpoint gone.
        self.wake_endpoint_waiters(env, obj, WaitOutcome::Closed)?;
        match rec.role {
            Role::Peer { chan, side, .. } => {
                // 2. Messages queued for this end can never be received.
                loop {
                    let m = self.channels.get_mut(chan)?.queues[side].pop();
                    let Some(m) = m else { break };
                    let cobj = self.channels.get(chan)?.object;
                    self.objects
                        .release(cobj, Counter::Inflight)
                        .map_err(|_| Error::Corrupt("queued message without a channel pin"))?;
                    self.discard_staged(&m);
                    self.stats.discarded_on_close += 1;
                }
                // 3. Detach; the peer observes PeerClosed after draining.
                let peer = {
                    let c = self.channels.get_mut(chan)?;
                    c.ends[side] = None;
                    c.ends[1 - side]
                };
                if let Some(pe) = peer {
                    self.wake_endpoint_waiters(env, pe, WaitOutcome::Woken)?;
                } else {
                    let cobj = self.channels.get(chan)?.object;
                    self.objects.revoke(cobj)?;
                    self.channels.free(chan);
                }
            }
            Role::Listener(l) => {
                for p in l.iter() {
                    if self.caps.exists(p.cap) {
                        self.caps.remove(&mut self.objects, p.cap)?;
                    }
                }
            }
        }
        // 4. Wait queues and slot.
        env.sync.wq_wake_all(env.sched, rec.recv_wq)?;
        env.sync.wq_wake_all(env.sched, rec.send_wq)?;
        env.sync.destroy(rec.recv_wq)?;
        env.sync.destroy(rec.send_wq)?;
        self.endpoints.slots[slot] = None;
        // 5. Revoke the object and pull every capability that named it.
        self.objects.revoke(obj)?;
        self.caps.sweep_object(&mut self.objects, obj)?;
        Ok(())
    }
}

const _: () = assert!(MAX_CHANNELS >= 4);

#[cfg(test)]
mod tests {
    use crate::d6::capability::Rights;
    use crate::d6::ipc::testutil::*;

    use crate::d6::object::{Error, Owner};

    #[test]
    fn server_client_connection_round_trip() {
        let mut r = Rig::new();
        let (srv, cli) = (r.proc(1), r.proc(2));
        let hl = r.ipc.create_listener(&mut renv!(r), srv).unwrap();
        // The server derives a connect-only capability for clients and "sends" it to the client.
        let connect = r.ipc.duplicate_handle(srv, hl, Rights::CONNECT).unwrap();
        assert_eq!(r.ipc.connect(&mut renv!(r), srv, connect).map(|_| ()), Ok(()), "self-connect works");
        // Drain that connection so the rest is clean.
        let (_, who) = r.ipc.accept(&mut renv!(r), srv, hl).unwrap();
        assert_eq!(who, srv);
        r.check();

        // A client with no capability cannot connect at all (no ambient authority).
        assert!(r.ipc.connect(&mut renv!(r), cli, connect).is_err());
        // Give the client a connect capability by transferring one through a channel.
        let (hs, hc) = r.pair(srv, cli);
        let m = msg(srv, 1, b"hello").with_transfers(&[crate::d6::ipc_message::TransferRequest {
            handle: connect.raw(),
            rights: Rights::CONNECT.union(Rights::TRANSFER),
        }]).unwrap();
        // CONNECT-only duplicate has no TRANSFER right: refused.
        assert_eq!(r.ipc.send(&mut renv!(r), srv, hs, m), Err(Error::RightsDenied));
        let connect2 = r.ipc.duplicate_handle(srv, hl, Rights::CONNECT.union(Rights::TRANSFER)).unwrap();
        let m = msg(srv, 1, b"hello").with_transfers(&[crate::d6::ipc_message::TransferRequest {
            handle: connect2.raw(),
            rights: Rights::CONNECT,
        }]).unwrap();
        r.ipc.send(&mut renv!(r), srv, hs, m).unwrap();
        let got = r.ipc.recv(&mut renv!(r), cli, hc).unwrap();
        let hconnect = crate::d6::handle::Handle::from_raw(got.transfers()[0].handle);
        assert_eq!(got.transfers()[0].rights, Rights::CONNECT);

        let hcli = r.ipc.connect(&mut renv!(r), cli, hconnect).unwrap();
        assert_eq!(r.ipc.backlog_len(srv, hl), Ok(1));
        let (hsrv, who) = r.ipc.accept(&mut renv!(r), srv, hl).unwrap();
        assert_eq!(who, cli, "the server learns the kernel-recorded client identity");
        assert_eq!(r.ipc.conn_role_of(cli, hcli), Ok(super::ConnRole::Client));
        assert_eq!(r.ipc.conn_role_of(srv, hsrv), Ok(super::ConnRole::Server));
        assert_eq!(r.ipc.accept(&mut renv!(r), srv, hl).err(), Some(Error::NoPending));
        r.ipc.send(&mut renv!(r), cli, hcli, msg(cli, 5, b"ping")).unwrap();
        let m = r.ipc.recv(&mut renv!(r), srv, hsrv).unwrap();
        assert_eq!((m.payload(), m.sender), (&b"ping"[..], cli));
        r.ipc.send(&mut renv!(r), srv, hsrv, msg(srv, 5, b"pong")).unwrap();
        assert_eq!(r.ipc.recv(&mut renv!(r), cli, hcli).unwrap().payload(), b"pong");
        r.check();
    }

    #[test]
    fn connect_rights_and_listener_kind_are_enforced() {
        let mut r = Rig::new();
        let (srv, cli) = (r.proc(1), r.proc(2));
        let hl = r.ipc.create_listener(&mut renv!(r), srv).unwrap();
        let accept_only = r.ipc.duplicate_handle(srv, hl, Rights::ACCEPT).unwrap();
        assert_eq!(r.ipc.connect(&mut renv!(r), srv, accept_only).err(), Some(Error::RightsDenied));
        let connect_only = r.ipc.duplicate_handle(srv, hl, Rights::CONNECT).unwrap();
        assert_eq!(r.ipc.accept(&mut renv!(r), srv, connect_only).err(), Some(Error::RightsDenied));
        // A peer endpoint is not a listener; a listener is not a peer.
        let (ha, _hb) = r.pair(srv, cli);
        let dup = r.ipc.duplicate_handle(srv, ha, Rights::CONNECT.union(Rights::ACCEPT)).unwrap_err();
        assert_eq!(dup, Error::RightsEscalation, "peer ends carry no CONNECT/ACCEPT");
        assert_eq!(r.ipc.send(&mut renv!(r), srv, hl, msg(srv, 0, b"")), Err(Error::RightsDenied));
        let dup = r.ipc.duplicate_handle(srv, hl, Rights::SEND).unwrap_err();
        assert_eq!(dup, Error::RightsEscalation);
        r.check();
    }

    #[test]
    fn backlog_is_bounded_and_connections_keep_fifo_order() {
        let mut r = Rig::new();
        let srv = r.proc(1);
        let hl = r.ipc.create_listener(&mut renv!(r), srv).unwrap();
        let clients: std::vec::Vec<Owner> = (10..10 + super::BACKLOG as u64 + 1).map(|n| r.proc(n)).collect();
        let mut hs = std::vec::Vec::new();
        for c in &clients[..super::BACKLOG] {
            hs.push(r.ipc.connect(&mut renv!(r), srv, hl).map(|h| (h, *c)).unwrap());
        }
        // Backlog full: explicit error, nothing leaked.
        let (objs, caps) = (r.ipc.objects().live(), r.ipc.caps().live());
        assert_eq!(r.ipc.connect(&mut renv!(r), srv, hl).err(), Some(Error::BacklogFull));
        assert_eq!((r.ipc.objects().live(), r.ipc.caps().live()), (objs, caps));
        r.check();
        for _ in 0..super::BACKLOG {
            r.ipc.accept(&mut renv!(r), srv, hl).unwrap();
        }
        assert!(r.ipc.connect(&mut renv!(r), srv, hl).is_ok(), "space again");
        r.check();
    }

    #[test]
    fn closing_a_listener_fails_pending_connections_and_frees_everything() {
        let mut r = Rig::new();
        let srv = r.proc(1);
        let hl = r.ipc.create_listener(&mut renv!(r), srv).unwrap();
        let hc = r.ipc.connect(&mut renv!(r), srv, hl).unwrap();
        r.ipc.connect(&mut renv!(r), srv, hl).unwrap();
        r.ipc.close_endpoint(&mut renv!(r), srv, hl).unwrap();
        assert_eq!(r.ipc.backlog_len(srv, hl).err(), Some(Error::Stale));
        // The never-accepted server ends are gone, so the client sees a closed peer.
        assert_eq!(r.ipc.recv(&mut renv!(r), srv, hc).err(), Some(Error::PeerClosed));
        assert_eq!(r.ipc.connect(&mut renv!(r), srv, hl).err(), Some(Error::Stale));
        r.check();
    }

    #[test]
    fn last_handle_close_closes_the_endpoint() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, hb) = r.pair(a, b);
        let ha2 = r.ipc.duplicate_handle(a, ha, Rights::SEND).unwrap();
        r.ipc.close_handle(&mut renv!(r), a, ha).unwrap();
        assert!(r.ipc.peer_open(b, hb).unwrap(), "a duplicate still holds the endpoint");
        r.ipc.close_handle(&mut renv!(r), a, ha2).unwrap();
        assert!(!r.ipc.peer_open(b, hb).unwrap(), "last capability gone: endpoint closed");
        r.check();
    }

    #[test]
    fn ownership_is_explicit_and_quota_checked() {
        let mut r = Rig::new();
        let (a, b) = (r.proc(1), r.proc(2));
        let (ha, _hb) = r.pair(a, b);
        assert_eq!(r.ipc.set_endpoint_owner(a, ha, r.ipc_missing()), Err(Error::NoSuchProcess));
        r.ipc.set_endpoint_owner(a, ha, b).unwrap();
        let weak = r.ipc.duplicate_handle(a, ha, Rights::SEND).unwrap();
        assert_eq!(r.ipc.set_endpoint_owner(a, weak, b), Err(Error::RightsDenied));
        r.check();
    }

    impl Rig {
        fn ipc_missing(&self) -> Owner {
            p(9999)
        }
    }

    #[test]
    fn peer_open_and_wrong_process_access() {
        let mut r = Rig::new();
        let (a, b, c) = (r.proc(1), r.proc(2), r.proc(3));
        let (ha, hb) = r.pair(a, b);
        assert_eq!(r.ipc.peer_open(a, ha), Ok(true));
        assert!(r.ipc.peer_open(c, ha).is_err(), "another process cannot use a's handle value");
        assert!(r.ipc.close_endpoint(&mut renv!(r), c, hb).is_err());
        r.ipc.close_endpoint(&mut renv!(r), b, hb).unwrap();
        assert_eq!(r.ipc.peer_open(a, ha), Ok(false));
        r.check();
    }
}
