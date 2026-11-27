//! Driver interface reference monitor (Mission D6, file 10). SPDX-License-Identifier: Apache-2.0
//!
//! # What this is, and what it is not
//!
//! This module gives a driver **a mediated, capability-checked IPC interface**. It does **not**
//! make a driver memory-isolated. A driver in this kernel is still code that the kernel has
//! chosen to run; if that code executes in Ring 0 it can do anything Ring 0 can do, and nothing
//! here changes that. Accordingly:
//!
//! * [`Isolation::ProtocolMediated`] is the *only* isolation level this module can claim, and
//!   [`Isolation::hardware_isolated`] is `false` for it. Callers must not describe a driver
//!   registered here as "isolated" unless the driver's code actually runs in an address space
//!   and privilege level that the D5 user-mode path enforces (not activated by D6).
//! * What the monitor *does* guarantee is about the **message path**, and every guarantee below
//!   is host-tested:
//!   1. A client never holds a `SEND` capability to the driver. It holds a `SEND`-only
//!      capability on the *interface object*, which can only be used with [`DriverMonitor::submit`].
//!   2. The driver's channel end and each client's reply end are `RECV`-only. Neither side can
//!      send, duplicate or transfer them; every message that crosses is built by the monitor.
//!   3. Requests are validated before they reach the driver: kind, size, no capability
//!      transfers, operation number inside the driver's declared operation mask.
//!   4. The driver sees kernel-assigned request tags, never the client's tag, and can only reply
//!      to a tag that is pending for *it*. Responses are validated (size) and delivered only to the
//!      client that made the request.
//!   5. Clients cannot see each other, and a slow or dead client cannot block the driver or other
//!      clients (bounded pending table, per-client limit).
//!   6. When a driver fails, every pending request is answered with an `Error` message, the
//!      client ends are closed, the interface capability is revoked and the service name is
//!      handled by the service manager's restart policy.
//!
//! The monitor holds the only `SEND` ends. It runs as a distinguished *monitor* process identity
//! supplied by the caller (so its handles are accounted like any other process's).

use crate::d6::capability::Rights;
use crate::d6::handle::Handle;
use crate::d6::ipc::Ipc;
use crate::d6::ipc_message::{Message, MsgFlags, MsgKind, MAX_PAYLOAD};
use crate::d6::ipc_wait::Env;
use crate::d6::object::{Error, ObjectId, ObjectKind, Owner};
use crate::d6::service::{Access, RestartHandler, RestartPolicy, ServiceId, ServiceLimits, ServiceManager};

/// Drivers the monitor can host. Bounded by the channel table (each driver and each client
/// connection costs one channel).
pub const MAX_DRIVERS: usize = 2;
/// Clients per driver.
pub const MAX_DRIVER_CLIENTS: usize = 4;
/// Outstanding (unanswered) requests per driver.
pub const MAX_PENDING: usize = 8;
/// Outstanding requests one client may have.
pub const MAX_PENDING_PER_CLIENT: usize = MAX_PENDING / MAX_DRIVER_CLIENTS;
/// Operation numbers are `0..MAX_OPS`.
pub const MAX_OPS: u8 = 32;

/// How strongly a driver is isolated. Deliberately has a single, weak variant today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// The driver is reachable only through the monitor's validated message protocol. Its own
    /// code is not memory- or privilege-isolated by this module.
    ProtocolMediated,
}

impl Isolation {
    /// Whether the driver's code is confined by hardware (address space + privilege level).
    /// Always `false`: this module cannot provide it.
    pub const fn hardware_isolated(self) -> bool {
        match self {
            Isolation::ProtocolMediated => false,
        }
    }
}

/// What a driver declares it implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverSpec {
    /// Bit `n` set = operation `n` is allowed.
    pub ops: u32,
    /// Largest request/response payload (including the operation byte of a request).
    pub max_payload: usize,
}

impl DriverSpec {
    pub fn new(ops: u32, max_payload: usize) -> Result<DriverSpec, Error> {
        if ops == 0 || max_payload < 1 || max_payload > MAX_PAYLOAD {
            return Err(Error::BadRequest);
        }
        Ok(DriverSpec { ops, max_payload })
    }
}

/// Identifies a hosted driver. Stale after the driver is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverId {
    slot: u8,
    epoch: u32,
}

#[derive(Debug, Clone, Copy)]
struct Client {
    owner: Owner,
    /// The monitor's (full-rights) end of this client's reply channel.
    mon_h: Handle,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    kernel_tag: u64,
    client_slot: u8,
    client_tag: u64,
}

struct DriverRec {
    iface: ObjectId,
    owner: Owner,
    spec: DriverSpec,
    /// The monitor's end of the request channel (the driver holds the `RECV`-only peer).
    to_driver: Handle,
    clients: [Option<Client>; MAX_DRIVER_CLIENTS],
    pending: [Option<Pending>; MAX_PENDING],
    next_tag: u64,
}

/// Counters, for tests and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DriverStats {
    pub forwarded: u64,
    pub answered: u64,
    pub rejected: u64,
    pub failed_requests: u64,
    pub drivers_failed: u64,
}

pub struct DriverMonitor {
    owner: Owner,
    drivers: [Option<DriverRec>; MAX_DRIVERS],
    gens: [u32; MAX_DRIVERS],
    stats: DriverStats,
}

impl DriverMonitor {
    /// Creates the monitor with `owner` as its identity (registered as a process in `ipc`).
    pub fn new(ipc: &mut Ipc, owner: Owner) -> Result<DriverMonitor, Error> {
        ipc.create_process(owner)?;
        Ok(DriverMonitor {
            owner,
            drivers: [const { None }; MAX_DRIVERS],
            gens: [0; MAX_DRIVERS],
            stats: DriverStats::default(),
        })
    }

    pub fn owner(&self) -> Owner {
        self.owner
    }

    pub fn stats(&self) -> DriverStats {
        self.stats
    }

    pub fn isolation_of(&self, id: DriverId) -> Result<Isolation, Error> {
        self.rec(id).map(|_| Isolation::ProtocolMediated)
    }

    pub fn is_alive(&self, id: DriverId) -> bool {
        self.rec(id).is_ok()
    }

    pub fn pending_of(&self, id: DriverId) -> Result<usize, Error> {
        Ok(self.rec(id)?.pending.iter().flatten().count())
    }

    pub fn clients_of(&self, id: DriverId) -> Result<usize, Error> {
        Ok(self.rec(id)?.clients.iter().flatten().count())
    }

    fn rec(&self, id: DriverId) -> Result<&DriverRec, Error> {
        match self.drivers.get(id.slot as usize) {
            Some(Some(r)) if self.gens[id.slot as usize] == id.epoch => Ok(r),
            _ => Err(Error::DriverFailed),
        }
    }

    fn id_of(&self, slot: usize) -> DriverId {
        DriverId {
            slot: slot as u8,
            epoch: self.gens[slot],
        }
    }

    // ---- registration ------------------------------------------------------------------------

    /// Registers a driver owned by `driver` under `name`. Needs `REGISTER` on `dir_h`. Returns the
    /// driver id, the driver's request handle (`RECV`-only) and the management handle.
    #[allow(clippy::too_many_arguments)]
    pub fn register_driver<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        mgr: &mut ServiceManager,
        driver: Owner,
        dir_h: Handle,
        name: &[u8],
        spec: DriverSpec,
        policy: RestartPolicy,
        access: Access,
        limits: ServiceLimits,
    ) -> Result<(DriverId, Handle, Handle), Error> {
        if driver == self.owner || !driver.is_process() {
            return Err(Error::ReservedOwner);
        }
        if limits.max_clients as usize > MAX_DRIVER_CLIENTS {
            return Err(Error::LimitReached);
        }
        let slot = self.drivers.iter().position(|d| d.is_none()).ok_or(Error::LimitReached)?;
        let (mon_h, drv_h) = Self::recv_only_pair(ipc, env, self.owner, driver)?;
        let iface = match ipc.objects.create(ObjectKind::DriverInterface, driver, slot as u32) {
            Ok(o) => o,
            Err(e) => {
                let _ = ipc.close_handle(env, self.owner, mon_h);
                let _ = ipc.close_handle(env, driver, drv_h);
                return Err(e);
            }
        };
        let registered = mgr.register_driver(
            ipc,
            driver,
            dir_h,
            name,
            iface,
            Rights::SEND,
            policy,
            access,
            limits,
        );
        let (_sid, mgmt): (ServiceId, Handle) = match registered {
            Ok(x) => x,
            Err(e) => {
                let _ = ipc.objects.revoke(iface);
                let _ = ipc.caps.sweep_object(&mut ipc.objects, iface);
                let _ = ipc.close_handle(env, self.owner, mon_h);
                let _ = ipc.close_handle(env, driver, drv_h);
                return Err(e);
            }
        };
        self.drivers[slot] = Some(DriverRec {
            iface,
            owner: driver,
            spec,
            to_driver: mon_h,
            clients: [None; MAX_DRIVER_CLIENTS],
            pending: [None; MAX_PENDING],
            next_tag: 1,
        });
        Ok((self.id_of(slot), drv_h, mgmt))
    }

    /// A channel between the monitor (full rights) and `peer` (`RECV` only).
    fn recv_only_pair<const N: usize, const S: usize>(
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        monitor: Owner,
        peer: Owner,
    ) -> Result<(Handle, Handle), Error> {
        let (hm, hp) = ipc.create_pair_handles(env, monitor, peer)?;
        let weak = match ipc.duplicate_handle(peer, hp, Rights::RECV) {
            Ok(h) => h,
            Err(e) => {
                let _ = ipc.close_handle(env, monitor, hm);
                let _ = ipc.close_handle(env, peer, hp);
                return Err(e);
            }
        };
        // The full-rights original goes away; the attenuated child keeps the endpoint open.
        if let Err(e) = ipc.close_handle(env, peer, hp) {
            let _ = ipc.close_handle(env, peer, weak);
            let _ = ipc.close_handle(env, monitor, hm);
            return Err(e);
        }
        Ok((hm, weak))
    }

    // ---- clients -----------------------------------------------------------------------------

    /// Resolves the client's interface handle to the driver slot it names. The handle must carry
    /// `SEND` on a `DriverInterface` object that a live driver owns.
    fn resolve(&self, ipc: &mut Ipc, client: Owner, iface_h: Handle) -> Result<usize, Error> {
        let (_, cap) = ipc
            .procs
            .get_mut(client)?
            .lookup(&ipc.caps, &ipc.objects, iface_h, Rights::SEND)?;
        ipc.objects.get_kind(cap.object, ObjectKind::DriverInterface)?;
        self.drivers
            .iter()
            .position(|d| d.as_ref().is_some_and(|d| d.iface == cap.object))
            .ok_or(Error::DriverFailed)
    }

    /// Drops client connections whose reply end has been closed (the client closed it or exited)
    /// together with their pending requests.
    fn prune<const N: usize, const S: usize>(&mut self, ipc: &mut Ipc, env: &mut Env<N, S>, slot: usize) {
        let mon = self.owner;
        let Some(rec) = self.drivers[slot].as_mut() else { return };
        for ci in 0..MAX_DRIVER_CLIENTS {
            let Some(c) = rec.clients[ci] else { continue };
            if ipc.peer_open(mon, c.mon_h).unwrap_or(false) {
                continue;
            }
            let _ = ipc.close_handle(env, mon, c.mon_h);
            rec.clients[ci] = None;
            for p in rec.pending.iter_mut() {
                if p.is_some_and(|p| p.client_slot as usize == ci) {
                    *p = None;
                }
            }
        }
    }

    /// Opens the client's reply channel for the driver behind `iface_h`. Returns a `RECV`-only
    /// handle on which the client receives responses. One binding per client per driver.
    pub fn bind<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        client: Owner,
        iface_h: Handle,
    ) -> Result<Handle, Error> {
        let slot = self.resolve(ipc, client, iface_h)?;
        self.prune(ipc, env, slot);
        let rec = self.drivers[slot].as_ref().ok_or(Error::DriverFailed)?;
        if rec.clients.iter().flatten().any(|c| c.owner == client) {
            return Err(Error::Busy);
        }
        let ci = rec.clients.iter().position(|c| c.is_none()).ok_or(Error::LimitReached)?;
        let (mon_h, reply_h) = Self::recv_only_pair(ipc, env, self.owner, client)?;
        if let Some(rec) = self.drivers[slot].as_mut() {
            rec.clients[ci] = Some(Client { owner: client, mon_h });
        }
        Ok(reply_h)
    }

    /// Submits one request. `payload[0]` is the operation number; the rest is operation data.
    /// Returns the client tag echoed in the eventual response.
    pub fn submit<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        client: Owner,
        iface_h: Handle,
        msg: &Message,
    ) -> Result<(), Error> {
        let slot = self.resolve(ipc, client, iface_h)?;
        self.prune(ipc, env, slot);
        let r = self.validate_and_forward(ipc, env, slot, client, msg);
        match r {
            Ok(()) => self.stats.forwarded += 1,
            Err(Error::DriverFailed) => {}
            Err(_) => self.stats.rejected += 1,
        }
        r
    }

    fn validate_and_forward<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        slot: usize,
        client: Owner,
        msg: &Message,
    ) -> Result<(), Error> {
        let mon = self.owner;
        let rec = self.drivers[slot].as_mut().ok_or(Error::DriverFailed)?;
        if !ipc.has_process(rec.owner) {
            self.fail_driver(ipc, env, slot);
            return Err(Error::DriverFailed);
        }
        if msg.kind != MsgKind::Request || !msg.transfers().is_empty() {
            return Err(Error::BadRequest);
        }
        let payload = msg.payload();
        if payload.is_empty() || payload.len() > rec.spec.max_payload {
            return Err(Error::BadRequest);
        }
        let op = payload[0];
        if op >= MAX_OPS || rec.spec.ops & (1u32 << op) == 0 {
            return Err(Error::BadRequest);
        }
        let ci = rec
            .clients
            .iter()
            .position(|c| c.is_some_and(|c| c.owner == client))
            .ok_or(Error::NotAuthorized)?;
        let mine = rec.pending.iter().flatten().filter(|p| p.client_slot as usize == ci).count();
        if mine >= MAX_PENDING_PER_CLIENT {
            return Err(Error::QuotaExceeded);
        }
        let pi = rec.pending.iter().position(|p| p.is_none()).ok_or(Error::Busy)?;
        let kernel_tag = rec.next_tag;
        rec.next_tag = kernel_tag.checked_add(1).ok_or(Error::LimitReached)?;
        let fwd = Message::new(mon, MsgKind::Request, MsgFlags::NEEDS_REPLY, kernel_tag, payload)?;
        let to_driver = rec.to_driver;
        match ipc.send(env, mon, to_driver, fwd) {
            Ok(_) => {}
            Err(Error::PeerClosed | Error::EndpointClosed) => {
                self.fail_driver(ipc, env, slot);
                return Err(Error::DriverFailed);
            }
            Err(e) => return Err(e),
        }
        if let Some(rec) = self.drivers[slot].as_mut() {
            rec.pending[pi] = Some(Pending {
                kernel_tag,
                client_slot: ci as u8,
                client_tag: msg.tag,
            });
        }
        Ok(())
    }

    // ---- driver side -------------------------------------------------------------------------

    /// The driver answers the request it received under `kernel_tag`.
    pub fn reply<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        driver: Owner,
        kernel_tag: u64,
        payload: &[u8],
    ) -> Result<(), Error> {
        self.complete(ipc, env, driver, kernel_tag, Some(payload))
    }

    /// The driver reports that it cannot complete the request; the client receives an `Error`.
    pub fn reply_error<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        driver: Owner,
        kernel_tag: u64,
    ) -> Result<(), Error> {
        self.complete(ipc, env, driver, kernel_tag, None)
    }

    fn complete<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        driver: Owner,
        kernel_tag: u64,
        payload: Option<&[u8]>,
    ) -> Result<(), Error> {
        let mon = self.owner;
        // Only the owner of a driver may answer for it, and only for requests pending *for it*.
        let slot = self
            .drivers
            .iter()
            .position(|d| d.as_ref().is_some_and(|d| d.owner == driver))
            .ok_or(Error::NotAuthorized)?;
        let rec = self.drivers[slot].as_mut().ok_or(Error::NotAuthorized)?;
        if let Some(p) = payload {
            if p.len() > rec.spec.max_payload {
                self.stats.rejected += 1;
                return Err(Error::BadResponse);
            }
        }
        let pi = rec
            .pending
            .iter()
            .position(|p| p.is_some_and(|p| p.kernel_tag == kernel_tag))
            .ok_or(Error::BadResponse)?;
        let pend = rec.pending[pi].ok_or(Error::BadResponse)?;
        let Some(client) = rec.clients[pend.client_slot as usize] else {
            rec.pending[pi] = None;
            return Err(Error::PeerClosed);
        };
        let out = match payload {
            Some(p) => Message::new(mon, MsgKind::Response, MsgFlags::NONE, pend.client_tag, p)?,
            None => Message::new(mon, MsgKind::Error, MsgFlags::NONE, pend.client_tag, &[])?,
        };
        match ipc.send(env, mon, client.mon_h, out) {
            Ok(_) => {
                rec.pending[pi] = None;
                if payload.is_some() {
                    self.stats.answered += 1;
                } else {
                    self.stats.failed_requests += 1;
                }
                Ok(())
            }
            // Client queue full: the request stays pending so the driver can retry later.
            Err(Error::QueueFull) => Err(Error::QueueFull),
            Err(Error::PeerClosed | Error::EndpointClosed) => {
                rec.pending[pi] = None;
                let _ = ipc.close_handle(env, mon, client.mon_h);
                rec.clients[pend.client_slot as usize] = None;
                Err(Error::PeerClosed)
            }
            Err(e) => Err(e),
        }
    }

    // ---- failure -----------------------------------------------------------------------------

    /// Removes a driver: pending requests get an `Error`, client and driver ends are closed and
    /// the interface capability (and everything derived from it) is revoked.
    fn fail_driver<const N: usize, const S: usize>(&mut self, ipc: &mut Ipc, env: &mut Env<N, S>, slot: usize) {
        let mon = self.owner;
        let Some(rec) = self.drivers[slot].take() else { return };
        for p in rec.pending.iter().flatten() {
            if let Some(c) = rec.clients[p.client_slot as usize] {
                if let Ok(m) = Message::new(mon, MsgKind::Error, MsgFlags::NONE, p.client_tag, &[]) {
                    let _ = ipc.send(env, mon, c.mon_h, m);
                }
                self.stats.failed_requests += 1;
            }
        }
        for c in rec.clients.iter().flatten() {
            let _ = ipc.close_handle(env, mon, c.mon_h);
        }
        let _ = ipc.close_handle(env, mon, rec.to_driver);
        if ipc.objects.get(rec.iface).is_ok() {
            let _ = ipc.objects.revoke(rec.iface);
            let _ = ipc.caps.sweep_object(&mut ipc.objects, rec.iface);
        }
        self.gens[slot] = self.gens[slot].wrapping_add(1);
        self.stats.drivers_failed += 1;
    }

    /// The driver process exited (`failed` = crashed). Clients are told, the interface dies, and
    /// the service manager applies the restart policy. Returns the services affected.
    pub fn driver_exited<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        mgr: &mut ServiceManager,
        driver: Owner,
        failed: bool,
        handler: &mut dyn RestartHandler,
    ) -> Result<usize, Error> {
        for slot in 0..MAX_DRIVERS {
            if self.drivers[slot].as_ref().is_some_and(|d| d.owner == driver) {
                self.fail_driver(ipc, env, slot);
            }
        }
        mgr.process_exited(ipc, env, driver, failed, handler)
    }

    /// Invariants across the monitor and the IPC core.
    pub fn check_invariants(&self, ipc: &Ipc) -> Result<(), &'static str> {
        let mut seen = [None::<ObjectId>; MAX_DRIVERS];
        for (i, d) in self.drivers.iter().enumerate() {
            let Some(d) = d else { continue };
            if seen.iter().flatten().any(|o| *o == d.iface) {
                return Err("driver: two drivers share an interface");
            }
            seen[i] = Some(d.iface);
            if ipc.objects.get_kind(d.iface, ObjectKind::DriverInterface).is_err() {
                return Err("driver: interface object missing");
            }
            if d.spec.ops == 0 || d.spec.max_payload == 0 || d.spec.max_payload > MAX_PAYLOAD {
                return Err("driver: bad spec");
            }
            let mut tags = [0u64; MAX_PENDING];
            let mut n = 0;
            for p in d.pending.iter().flatten() {
                if d.clients[p.client_slot as usize].is_none() {
                    return Err("driver: pending request without a client");
                }
                if p.kernel_tag == 0 || p.kernel_tag >= d.next_tag || tags[..n].contains(&p.kernel_tag) {
                    return Err("driver: bad or duplicate kernel tag");
                }
                tags[n] = p.kernel_tag;
                n += 1;
            }
            for ci in 0..MAX_DRIVER_CLIENTS {
                let mine = d.pending.iter().flatten().filter(|p| p.client_slot as usize == ci).count();
                if mine > MAX_PENDING_PER_CLIENT {
                    return Err("driver: client over its pending limit");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d6::ipc::testutil::*;

    struct NoRestart;
    impl RestartHandler for NoRestart {
        fn request_restart(&mut self, _n: &crate::d6::service::ServiceName, _a: u8) -> bool {
            false
        }
    }

    struct World {
        r: Rig,
        mgr: ServiceManager,
        mon: DriverMonitor,
        drv: Owner,
        cli: Owner,
        drv_dir: Handle,
        cli_dir: Handle,
    }

    const LIM: ServiceLimits = ServiceLimits { max_clients: MAX_DRIVER_CLIENTS as u8 };
    const OPS: u32 = 0b1011; // ops 0, 1, 3

    fn world() -> World {
        let mut r = Rig::new();
        let mut mgr = ServiceManager::new(&mut r.ipc).unwrap();
        let mon = DriverMonitor::new(&mut r.ipc, p(500)).unwrap();
        let (drv, cli) = (r.proc(1), r.proc(2));
        let drv_dir = mgr.grant_directory(&mut r.ipc, drv, Rights::REGISTER).unwrap();
        let cli_dir = mgr.grant_directory(&mut r.ipc, cli, Rights::LOOKUP).unwrap();
        let _ = &mut mgr;
        World { r, mgr, mon, drv, cli, drv_dir, cli_dir }
    }

    fn reg(w: &mut World, name: &[u8]) -> (DriverId, Handle, Handle) {
        w.mon
            .register_driver(
                &mut w.r.ipc,
                &mut renv!(w.r),
                &mut w.mgr,
                w.drv,
                w.drv_dir,
                name,
                DriverSpec::new(OPS, 32).unwrap(),
                RestartPolicy::Never,
                Access::Open,
                LIM,
            )
            .unwrap()
    }

    fn connect(w: &mut World, c: Owner, d: Handle, name: &[u8]) -> (Handle, Handle) {
        let ih = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), c, d, name).unwrap();
        let rh = w.mon.bind(&mut w.r.ipc, &mut renv!(w.r), c, ih).unwrap();
        (ih, rh)
    }

    fn connect_cli(w: &mut World, name: &[u8]) -> (Handle, Handle) {
        let (c, d) = (w.cli, w.cli_dir);
        connect(w, c, d, name)
    }

    fn rq(from: Owner, tag: u64, body: &[u8]) -> Message {
        Message::new(from, MsgKind::Request, MsgFlags::NEEDS_REPLY, tag, body).unwrap()
    }

    fn check(w: &World) {
        w.r.check();
        w.mgr.check_invariants(&w.r.ipc).unwrap();
        w.mon.check_invariants(&w.r.ipc).unwrap();
    }

    #[test]
    fn isolation_claim_is_honest() {
        assert!(!Isolation::ProtocolMediated.hardware_isolated());
        let mut w = world();
        let (id, _, _) = reg(&mut w, b"disk");
        assert_eq!(w.mon.isolation_of(id), Ok(Isolation::ProtocolMediated));
    }

    #[test]
    fn full_request_response_round_trip() {
        let mut w = world();
        let (id, dh, _) = reg(&mut w, b"disk");
        let (ih, rh) = connect_cli(&mut w, b"disk");
        w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, 77, &[1, 9, 9])).unwrap();
        assert_eq!(w.mon.pending_of(id), Ok(1));
        let m = w.r.ipc.recv(&mut renv!(w.r), w.drv, dh).unwrap();
        assert_eq!(m.kind, MsgKind::Request);
        assert_ne!(m.tag, 77, "driver never sees the client's tag");
        assert_eq!(m.sender, w.mon.owner());
        assert_eq!(m.payload(), &[1, 9, 9]);
        w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.drv, m.tag, b"done").unwrap();
        let resp = w.r.ipc.recv(&mut renv!(w.r), w.cli, rh).unwrap();
        assert_eq!((resp.kind, resp.tag, resp.payload()), (MsgKind::Response, 77, &b"done"[..]));
        assert_eq!(w.mon.pending_of(id), Ok(0));
        // The same reply cannot be replayed.
        assert_eq!(w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.drv, m.tag, b"again").err(), Some(Error::BadResponse));
        check(&w);
    }

    #[test]
    fn neither_side_can_send_or_pass_on_its_ends() {
        let mut w = world();
        let (_, dh, _) = reg(&mut w, b"disk");
        let (ih, rh) = connect_cli(&mut w, b"disk");
        // Driver end and client reply end are RECV-only.
        assert_eq!(w.r.ipc.rights_of(w.drv, dh), Ok(Rights::RECV));
        assert_eq!(w.r.ipc.rights_of(w.cli, rh), Ok(Rights::RECV));
        assert_eq!(w.r.ipc.send(&mut renv!(w.r), w.drv, dh, msg(w.drv, 1, b"x")).err(), Some(Error::RightsDenied));
        assert_eq!(w.r.ipc.send(&mut renv!(w.r), w.cli, rh, msg(w.cli, 1, b"x")).err(), Some(Error::RightsDenied));
        assert!(w.r.ipc.duplicate_handle(w.drv, dh, Rights::RECV).is_err());
        assert!(w.r.ipc.duplicate_handle(w.cli, rh, Rights::RECV).is_err());
        // The client's interface capability is SEND-only: it cannot be duplicated or sent as data.
        assert_eq!(w.r.ipc.rights_of(w.cli, ih), Ok(Rights::SEND));
        assert!(w.r.ipc.duplicate_handle(w.cli, ih, Rights::SEND).is_err());
        // A raw send on the interface handle is not an endpoint operation.
        assert!(w.r.ipc.send(&mut renv!(w.r), w.cli, ih, msg(w.cli, 1, b"x")).is_err());
        check(&w);
    }

    #[test]
    fn invalid_requests_never_reach_the_driver() {
        let mut w = world();
        let (id, dh, _) = reg(&mut w, b"disk");
        let (ih, _rh) = connect_cli(&mut w, b"disk");
        let bad: [(Message, Error); 6] = [
            (rq(w.cli, 1, &[]), Error::BadRequest),                       // empty
            (rq(w.cli, 1, &[2]), Error::BadRequest),                      // op 2 not allowed
            (rq(w.cli, 1, &[40]), Error::BadRequest),                     // op out of range
            (rq(w.cli, 1, &[0; 33]), Error::BadRequest),                  // over the driver's limit
            (msg(w.cli, 1, &[0]), Error::BadRequest),                     // not a Request
            (
                Message::new(w.cli, MsgKind::Response, MsgFlags::NONE, 1, &[0]).unwrap(),
                Error::BadRequest,
            ),
        ];
        for (m, e) in bad {
            assert_eq!(w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &m).err(), Some(e));
        }
        // A request carrying a capability transfer is refused.
        let with_xfer = rq(w.cli, 1, &[0])
            .with_transfers(&[crate::d6::ipc_message::TransferRequest { handle: ih.raw(), rights: Rights::NONE }])
            .unwrap();
        assert_eq!(w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &with_xfer).err(), Some(Error::BadRequest));
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), w.drv, dh).err(), Some(Error::QueueEmpty));
        assert_eq!(w.mon.pending_of(id), Ok(0));
        assert_eq!(w.mon.stats().rejected, 7);
        check(&w);
    }

    #[test]
    fn unbound_clients_and_foreign_handles_are_refused() {
        let mut w = world();
        let (_, _dh, _) = reg(&mut w, b"disk");
        let ih = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"disk").unwrap();
        // Connected but not bound: no reply channel exists, so the request is refused.
        assert_eq!(w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, 1, &[0])).err(), Some(Error::NotAuthorized));
        // A handle value that exists only in someone else's table does not resolve.
        let other = w.r.proc(3);
        assert!(w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), other, ih, &rq(other, 1, &[0])).is_err());
        // A directory handle is not an interface handle.
        assert!(w.mon.bind(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir).is_err());
        check(&w);
    }

    #[test]
    fn driver_can_only_answer_its_own_pending_requests() {
        let mut w = world();
        let (_, dh, _) = reg(&mut w, b"disk");
        let (ih, rh) = connect_cli(&mut w, b"disk");
        w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, 5, &[0])).unwrap();
        let m = w.r.ipc.recv(&mut renv!(w.r), w.drv, dh).unwrap();
        // Someone else (the client, or any process) cannot answer for the driver.
        assert_eq!(w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.cli, m.tag, b"x").err(), Some(Error::NotAuthorized));
        // A forged / guessed tag does not match.
        assert_eq!(w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.drv, m.tag + 1000, b"x").err(), Some(Error::BadResponse));
        // An oversized response is refused and the request stays pending.
        assert_eq!(w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.drv, m.tag, &[0u8; 33]).err(), Some(Error::BadResponse));
        w.mon.reply_error(&mut w.r.ipc, &mut renv!(w.r), w.drv, m.tag).unwrap();
        let e = w.r.ipc.recv(&mut renv!(w.r), w.cli, rh).unwrap();
        assert_eq!((e.kind, e.tag), (MsgKind::Error, 5));
        check(&w);
    }

    #[test]
    fn clients_are_isolated_from_each_other() {
        let mut w = world();
        let (_, dh, _) = reg(&mut w, b"disk");
        let c2 = w.r.proc(3);
        let d2 = w.mgr.grant_directory(&mut w.r.ipc, c2, Rights::LOOKUP).unwrap();
        let (ih1, rh1) = connect_cli(&mut w, b"disk");
        let (ih2, rh2) = connect(&mut w, c2, d2, b"disk");
        w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih1, &rq(w.cli, 1, &[0, 1])).unwrap();
        w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), c2, ih2, &rq(c2, 1, &[0, 2])).unwrap();
        let a = w.r.ipc.recv(&mut renv!(w.r), w.drv, dh).unwrap();
        let b = w.r.ipc.recv(&mut renv!(w.r), w.drv, dh).unwrap();
        w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.drv, b.tag, b"for-c2").unwrap();
        w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.drv, a.tag, b"for-c1").unwrap();
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), w.cli, rh1).unwrap().payload(), b"for-c1");
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), c2, rh2).unwrap().payload(), b"for-c2");
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), w.cli, rh1).err(), Some(Error::QueueEmpty));
        // A client cannot bind twice.
        assert_eq!(w.mon.bind(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih1).err(), Some(Error::Busy));
        check(&w);
    }

    #[test]
    fn one_client_cannot_starve_the_pending_table() {
        let mut w = world();
        let (id, _dh, _) = reg(&mut w, b"disk");
        let (ih, _) = connect_cli(&mut w, b"disk");
        for t in 0..MAX_PENDING_PER_CLIENT as u64 {
            w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, t, &[0])).unwrap();
        }
        assert_eq!(
            w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, 99, &[0])).err(),
            Some(Error::QuotaExceeded)
        );
        assert_eq!(w.mon.pending_of(id), Ok(MAX_PENDING_PER_CLIENT));
        // Another client is unaffected.
        let c2 = w.r.proc(3);
        let d2 = w.mgr.grant_directory(&mut w.r.ipc, c2, Rights::LOOKUP).unwrap();
        let (ih2, _) = connect(&mut w, c2, d2, b"disk");
        w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), c2, ih2, &rq(c2, 1, &[0])).unwrap();
        check(&w);
    }

    #[test]
    fn a_client_that_leaves_does_not_block_the_driver() {
        let mut w = world();
        let (id, dh, _) = reg(&mut w, b"disk");
        let (ih, rh) = connect_cli(&mut w, b"disk");
        w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, 1, &[0])).unwrap();
        let m = w.r.ipc.recv(&mut renv!(w.r), w.drv, dh).unwrap();
        w.r.ipc.close_handle(&mut renv!(w.r), w.cli, rh).unwrap();
        // The driver's reply finds the client gone: the request is dropped, not left pending.
        assert_eq!(w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.drv, m.tag, b"x").err(), Some(Error::PeerClosed));
        assert_eq!(w.mon.pending_of(id), Ok(0));
        assert_eq!(w.mon.clients_of(id), Ok(0));
        // The slot is reusable.
        let c2 = w.r.proc(3);
        let d2 = w.mgr.grant_directory(&mut w.r.ipc, c2, Rights::LOOKUP).unwrap();
        connect(&mut w, c2, d2, b"disk");
        check(&w);
    }

    #[test]
    fn driver_crash_is_contained() {
        let mut w = world();
        let (id, _dh, _) = reg(&mut w, b"disk");
        let (ih, rh) = connect_cli(&mut w, b"disk");
        w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, 8, &[0])).unwrap();
        let n = w
            .mon
            .driver_exited(&mut w.r.ipc, &mut renv!(w.r), &mut w.mgr, w.drv, true, &mut NoRestart)
            .unwrap();
        assert_eq!(n, 1);
        assert!(!w.mon.is_alive(id));
        // The client is told about the lost request, then sees the closed channel.
        let e = w.r.ipc.recv(&mut renv!(w.r), w.cli, rh).unwrap();
        assert_eq!((e.kind, e.tag), (MsgKind::Error, 8));
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), w.cli, rh).err(), Some(Error::PeerClosed));
        // The interface capability is dead and the name is gone.
        assert!(w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, 9, &[0])).is_err());
        assert!(w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"disk").is_err());
        // An unrelated process is untouched and the system is consistent.
        assert!(w.r.ipc.has_process(w.cli));
        check(&w);
    }

    #[test]
    fn driver_closing_its_end_fails_the_driver_on_next_use() {
        let mut w = world();
        let (id, dh, _) = reg(&mut w, b"disk");
        let (ih, _rh) = connect_cli(&mut w, b"disk");
        w.r.ipc.close_handle(&mut renv!(w.r), w.drv, dh).unwrap();
        assert_eq!(
            w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), w.cli, ih, &rq(w.cli, 1, &[0])).err(),
            Some(Error::DriverFailed)
        );
        assert!(!w.mon.is_alive(id));
        check(&w);
    }

    #[test]
    fn registration_rules() {
        let mut w = world();
        // The monitor identity cannot be a driver; a driver needs REGISTER.
        let mon_owner = w.mon.owner();
        let spec = DriverSpec::new(OPS, 8).unwrap();
        assert_eq!(
            w.mon
                .register_driver(&mut w.r.ipc, &mut renv!(w.r), &mut w.mgr, mon_owner, w.drv_dir, b"x", spec, RestartPolicy::Never, Access::Open, LIM)
                .err(),
            Some(Error::ReservedOwner)
        );
        assert!(
            w.mon
                .register_driver(&mut w.r.ipc, &mut renv!(w.r), &mut w.mgr, w.cli, w.cli_dir, b"x", spec, RestartPolicy::Never, Access::Open, LIM)
                .is_err(),
            "LOOKUP-only directory cannot register"
        );
        // Failed registration leaks nothing.
        w.r.check();
        w.mon.check_invariants(&w.r.ipc).unwrap();
        let before = w.r.ipc.objects().live();
        reg(&mut w, b"a");
        assert!(
            w.mon
                .register_driver(&mut w.r.ipc, &mut renv!(w.r), &mut w.mgr, w.drv, w.drv_dir, b"a", spec, RestartPolicy::Never, Access::Open, LIM)
                .is_err(),
            "duplicate name"
        );
        let after = w.r.ipc.objects().live();
        assert!(after > before);
        assert!(DriverSpec::new(0, 8).is_err());
        assert!(DriverSpec::new(1, 0).is_err());
        assert!(DriverSpec::new(1, MAX_PAYLOAD + 1).is_err());
        check(&w);
    }

    /// Randomised sequence of client/driver actions; invariants must hold after every step.
    #[test]
    fn randomized_protocol_keeps_invariants() {
        for seed in [0x44_52_56_5f_53_45_45_44u64, 1, 2, 0xdead_beef, 0x6D69_7373_696F_6E36] {
            let mut rng = Lcg(seed);
            let mut w = world();
            let (id, dh, _) = reg(&mut w, b"disk");
            let mut clients = std::vec::Vec::new();
            for n in 10..13u64 {
                let c = w.r.proc(n);
                let d = w.mgr.grant_directory(&mut w.r.ipc, c, Rights::LOOKUP).unwrap();
                let (ih, rh) = connect(&mut w, c, d, b"disk");
                clients.push((c, ih, rh));
            }
            let mut tags = std::vec::Vec::new();
            let mut sent = 0u64;
            let mut got = 0u64;
            for step in 0..400 {
                match rng.next() % 4 {
                    0 | 1 => {
                        let (c, ih, _) = clients[(rng.next() % 3) as usize];
                        let op = [0u8, 1, 3, 2, 9][(rng.next() % 5) as usize];
                        let ok = w.mon.submit(&mut w.r.ipc, &mut renv!(w.r), c, ih, &rq(c, step, &[op])).is_ok();
                        assert_eq!(ok, matches!(op, 0 | 1 | 3) && w.mon.pending_of(id).is_ok() && ok);
                        if ok {
                            sent += 1;
                        }
                    }
                    2 => {
                        if let Ok(m) = w.r.ipc.recv(&mut renv!(w.r), w.drv, dh) {
                            tags.push(m.tag);
                        }
                    }
                    _ => {
                        if let Some(t) = tags.pop() {
                            let r = if rng.next() % 5 == 0 {
                                w.mon.reply_error(&mut w.r.ipc, &mut renv!(w.r), w.drv, t)
                            } else {
                                w.mon.reply(&mut w.r.ipc, &mut renv!(w.r), w.drv, t, b"r")
                            };
                            match r {
                                Ok(()) => got += 1,
                                Err(Error::QueueFull) => tags.push(t),
                                Err(e) => panic!("seed {seed:#x} step {step}: {e:?}"),
                            }
                        }
                        let (c, _, rh) = clients[(rng.next() % 3) as usize];
                        let _ = w.r.ipc.recv(&mut renv!(w.r), c, rh);
                    }
                }
                w.mon.check_invariants(&w.r.ipc).unwrap_or_else(|e| panic!("seed {seed:#x} step {step}: {e}"));
                w.r.ipc.check_invariants().unwrap_or_else(|e| panic!("seed {seed:#x} step {step}: {e}"));
            }
            assert!(got <= sent, "seed {seed:#x}: more replies than requests");
            check(&w);
        }
    }

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    #[test]
    fn restart_policy_applies_to_failed_drivers() {
        struct Yes(u32);
        impl RestartHandler for Yes {
            fn request_restart(&mut self, _n: &crate::d6::service::ServiceName, _a: u8) -> bool {
                self.0 += 1;
                true
            }
        }
        let mut w = world();
        let spec = DriverSpec::new(OPS, 32).unwrap();
        let (_, _, _) = w
            .mon
            .register_driver(&mut w.r.ipc, &mut renv!(w.r), &mut w.mgr, w.drv, w.drv_dir, b"disk", spec, RestartPolicy::OnFailure { max: 2 }, Access::Open, LIM)
            .unwrap();
        let mut h = Yes(0);
        w.mon.driver_exited(&mut w.r.ipc, &mut renv!(w.r), &mut w.mgr, w.drv, true, &mut h).unwrap();
        assert_eq!(h.0, 1);
        assert_eq!(w.mgr.state_of(b"disk"), Some(crate::d6::service::ServiceState::Restarting));
        w.mon.check_invariants(&w.r.ipc).unwrap();
        w.r.check();
    }
}
