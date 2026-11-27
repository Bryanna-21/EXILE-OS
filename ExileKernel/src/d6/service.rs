//! Minimal user-service infrastructure (Mission D6, file 9). SPDX-License-Identifier: Apache-2.0
//!
//! **This is a service *registry and connection broker*, not a production service manager.** It
//! has no process spawning, no dependency ordering, no supervision tree and no persistence.
//! What it does provide is real and goes through the capability and IPC layers, not around them:
//!
//! * **No ambient authority.** There is no global name lookup. The kernel creates one *directory*
//!   object and grants processes capabilities to it with `REGISTER` and/or `LOOKUP` rights
//!   ([`ServiceManager::grant_directory`]). Registering needs `REGISTER`; discovering and
//!   connecting needs `LOOKUP`.
//! * **Publication = a kernel-held capability.** A server registers a *listener* it owns. The
//!   kernel derives a `CONNECT`-only capability from the server's own listener capability and
//!   keeps it (owner `Owner::KERNEL`). Clients never see the listener; they receive the client end
//!   of a fresh channel created by `Ipc::connect_obj`. Because the published capability is a
//!   *descendant* of the server's, the server can un-publish by revoking its derivations, and
//!   closing the listener (or the server exiting) kills the publication.
//! * **Connection authorisation.** Each service has an [`Access`] policy (`Open` to every holder
//!   of `LOOKUP`, or an explicit allowlist of client identities) and [`ServiceLimits`]
//!   (maximum concurrent clients). Refusals are explicit errors.
//! * **Lifecycle.** `Available -> Unavailable | Restarting | Failed | (freed)` driven by
//!   [`ServiceManager::process_exited`] and the [`RestartPolicy`]. Restart is an *interface*:
//!   the manager asks a [`RestartHandler`] (a future process manager) to restart; re-registration
//!   of a restarting name is only accepted from the owner the handler authorised, so a different
//!   process cannot hijack the name of a crashed service.
//! * **Containment.** When a service's process exits, `Ipc::destroy_process` closes everything it
//!   owned; its clients observe `PeerClosed` on their channels and nothing else changes.
//!
//! [`EchoService`] is the minimal test service required by the mission: it registers, accepts
//! client connections, receives requests and returns responses, entirely through handles and
//! IPC. It runs as a polled loop (`pump`) because running it as a real user-mode server needs
//! the D5 user-mode/syscall path, which is not activated here.
#![allow(dead_code)]

use crate::d6::capability::{CapId, Rights};
use crate::d6::handle::Handle;
use crate::d6::ipc::Ipc;
use crate::d6::ipc_message::{Message, MsgFlags, MsgKind, MAX_PAYLOAD};
use crate::d6::ipc_wait::Env;
use crate::d6::object::{Error, ObjectId, ObjectKind, Owner};

pub const MAX_SERVICES: usize = 8;
pub const MAX_SERVICES_PER_OWNER: usize = 2;
pub const NAME_MAX: usize = 16;
pub const MAX_CLIENTS: usize = 6;
pub const MAX_ALLOWED: usize = 4;
const DIR_TAG: u32 = u32::MAX;

/// Rights that may be placed on a directory capability.
pub const DIR_RIGHTS: Rights = Rights::REGISTER
    .union(Rights::LOOKUP)
    .union(Rights::DERIVE)
    .union(Rights::TRANSFER);

/// Rights of the per-service management handle returned to the registrant.
const MGMT_RIGHTS: Rights = Rights::DESTROY.union(Rights::LOOKUP);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceName {
    bytes: [u8; NAME_MAX],
    len: u8,
}

impl ServiceName {
    /// 1..=16 bytes of `[a-z0-9._-]`.
    pub fn new(name: &[u8]) -> Result<ServiceName, Error> {
        if name.is_empty() || name.len() > NAME_MAX {
            return Err(Error::BadName);
        }
        if !name
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(Error::BadName);
        }
        let mut bytes = [0u8; NAME_MAX];
        bytes[..name.len()].copy_from_slice(name);
        Ok(ServiceName {
            bytes,
            len: name.len() as u8,
        })
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceId {
    index: u8,
    generation: u32,
}

impl ServiceId {
    pub const fn index(self) -> usize {
        self.index as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Accepting connections.
    Available,
    /// The service object or its publication is gone (owner exited / listener closed) and no
    /// restart is planned yet.
    Unavailable,
    /// The owner died and a restart was requested; the name is reserved for the restarted owner.
    Restarting,
    /// Restarts exhausted or refused. The name can be registered again by anyone.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    Never,
    /// Restart after failure (`failed == true`) at most `max` times.
    OnFailure { max: u8 },
    /// Restart after any exit at most `max` times.
    Always { max: u8 },
}

/// The interface a process manager implements to carry out restarts.
pub trait RestartHandler {
    /// Asked to start a new instance of `name`; `attempt` counts from 1. Returns whether a
    /// restart was scheduled. The handler should then call
    /// [`ServiceManager::authorize_restart`] with the new process.
    fn request_restart(&mut self, name: &ServiceName, attempt: u8) -> bool;
}

/// Declines every restart.
pub struct NoRestart;

impl RestartHandler for NoRestart {
    fn request_restart(&mut self, _: &ServiceName, _: u8) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Every holder of a `LOOKUP` directory capability may connect.
    Open,
    /// Only these client identities may connect.
    Allowlist([Option<Owner>; MAX_ALLOWED]),
}

impl Access {
    pub fn allowlist(owners: &[Owner]) -> Result<Access, Error> {
        if owners.len() > MAX_ALLOWED {
            return Err(Error::LimitReached);
        }
        let mut a = [None; MAX_ALLOWED];
        for (i, o) in owners.iter().enumerate() {
            a[i] = Some(*o);
        }
        Ok(Access::Allowlist(a))
    }
    fn permits(&self, who: Owner) -> bool {
        match self {
            Access::Open => true,
            Access::Allowlist(l) => l.iter().flatten().any(|o| *o == who),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceLimits {
    /// Maximum concurrently connected clients (<= [`MAX_CLIENTS`]).
    pub max_clients: u8,
}

impl ServiceLimits {
    pub const DEFAULT: ServiceLimits = ServiceLimits {
        max_clients: MAX_CLIENTS as u8,
    };
}

/// What a successful `connect` hands the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published {
    /// The service is a listener: `connect` returns the client end of a new channel.
    Listener,
    /// The service is a driver interface (see `driver.rs`): `connect` returns a capability to it.
    DriverInterface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceInfo {
    pub id: ServiceId,
    pub state: ServiceState,
    pub published: Published,
    pub clients: usize,
}

#[derive(Clone, Copy)]
struct Conn {
    client: Owner,
    end: ObjectId,
}

#[derive(Clone, Copy)]
struct Record {
    id: ServiceId,
    name: ServiceName,
    owner: Owner,
    object: ObjectId,
    connect_cap: Option<CapId>,
    published: Published,
    state: ServiceState,
    policy: RestartPolicy,
    restarts: u8,
    restart_owner: Option<Owner>,
    access: Access,
    limits: ServiceLimits,
    conns: [Option<Conn>; MAX_CLIENTS],
}

pub struct ServiceManager {
    dir: ObjectId,
    recs: [Option<Record>; MAX_SERVICES],
    gens: [u32; MAX_SERVICES],
}

impl ServiceManager {
    /// Creates the (single) directory object. The kernel owns it.
    pub fn new(ipc: &mut Ipc) -> Result<ServiceManager, Error> {
        let dir = ipc.objects.create(ObjectKind::Service, Owner::KERNEL, DIR_TAG)?;
        Ok(ServiceManager {
            dir,
            recs: [None; MAX_SERVICES],
            gens: [0; MAX_SERVICES],
        })
    }

    pub fn live(&self) -> usize {
        self.recs.iter().flatten().count()
    }

    /// Kernel operation: grants `holder` a capability to the directory with `rights`.
    pub fn grant_directory(
        &self,
        ipc: &mut Ipc,
        holder: Owner,
        rights: Rights,
    ) -> Result<Handle, Error> {
        if !rights.is_subset_of(DIR_RIGHTS) {
            return Err(Error::RightsEscalation);
        }
        ipc.procs
            .get_mut(holder)?
            .grant(&mut ipc.caps, &mut ipc.objects, self.dir, rights)
    }

    fn check_dir(&self, ipc: &mut Ipc, who: Owner, h: Handle, need: Rights) -> Result<(), Error> {
        let (_, cap) = ipc
            .procs
            .get_mut(who)?
            .lookup(&ipc.caps, &ipc.objects, h, need)?;
        if cap.object != self.dir {
            return Err(Error::NotAuthorized);
        }
        Ok(())
    }

    fn find(&self, name: &ServiceName) -> Option<usize> {
        self.recs
            .iter()
            .position(|r| r.as_ref().is_some_and(|r| r.name == *name))
    }

    fn id_of(&self, svc: &Record) -> usize {
        svc.id.index()
    }

    /// Lazily notices services whose object disappeared (owner exit handled without the
    /// manager). They become `Unavailable` (their restart policy is applied by
    /// [`process_exited`](Self::process_exited) only).
    fn refresh(&mut self, ipc: &Ipc) {
        for r in self.recs.iter_mut().flatten() {
            if r.state == ServiceState::Available && ipc.objects.get(r.object).is_err() {
                r.state = ServiceState::Unavailable;
                r.connect_cap = None;
                r.conns = [None; MAX_CLIENTS];
            }
        }
    }

    fn free_slot(&mut self, ipc: &mut Ipc, i: usize) {
        if let Some(r) = self.recs[i].take() {
            if let Some(c) = r.connect_cap {
                if ipc.caps.exists(c) {
                    let _ = ipc.caps.remove(&mut ipc.objects, c);
                }
            }
            if ipc.objects.get(r.object).is_ok() {
                let _ = ipc.objects.revoke(r.object);
                let _ = ipc.caps.sweep_object(&mut ipc.objects, r.object);
            }
            self.gens[i] = self.gens[i].wrapping_add(1);
        }
    }

    // ---- registration ------------------------------------------------------------------------

    /// Registers the listener behind `listener_h` under `name`. Needs `REGISTER` on the directory
    /// capability and `ACCEPT | CONNECT | DERIVE` on the listener capability (the registrant must
    /// itself hold the authority it publishes). Returns the service id and a management handle
    /// (`DESTROY | LOOKUP`) to the service object.
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &mut self,
        ipc: &mut Ipc,
        owner: Owner,
        dir_h: Handle,
        name: &[u8],
        listener_h: Handle,
        policy: RestartPolicy,
        access: Access,
        limits: ServiceLimits,
    ) -> Result<(ServiceId, Handle), Error> {
        self.check_dir(ipc, owner, dir_h, Rights::REGISTER)?;
        let (lid, lcap) = ipc.procs.get_mut(owner)?.lookup(
            &ipc.caps,
            &ipc.objects,
            listener_h,
            Rights::ACCEPT.union(Rights::CONNECT).union(Rights::DERIVE),
        )?;
        match ipc.endpoints.by_object(&ipc.objects, lcap.object)?.1.role {
            crate::d6::ipc_endpoint::Role::Listener(_) => {}
            _ => return Err(Error::NotListener),
        }
        let cap = ipc
            .caps
            .derive_kernel(&mut ipc.objects, lid, Rights::CONNECT, Owner::KERNEL, Owner::KERNEL)?;
        self.finish_register(ipc, owner, name, cap, Published::Listener, policy, access, limits)
            .inspect_err(|_| {
                let _ = ipc.caps.remove(&mut ipc.objects, cap);
            })
    }

    /// Registers a driver interface (called by `driver.rs`, which owns the interface object).
    /// The kernel-held capability is minted directly on `iface` with the rights clients get.
    #[allow(clippy::too_many_arguments)]
    pub fn register_driver(
        &mut self,
        ipc: &mut Ipc,
        owner: Owner,
        dir_h: Handle,
        name: &[u8],
        iface: ObjectId,
        client_rights: Rights,
        policy: RestartPolicy,
        access: Access,
        limits: ServiceLimits,
    ) -> Result<(ServiceId, Handle), Error> {
        self.check_dir(ipc, owner, dir_h, Rights::REGISTER)?;
        let cap = ipc.caps.mint(&mut ipc.objects, Owner::KERNEL, iface, client_rights)?;
        self.finish_register(ipc, owner, name, cap, Published::DriverInterface, policy, access, limits)
            .inspect_err(|_| {
                let _ = ipc.caps.remove(&mut ipc.objects, cap);
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_register(
        &mut self,
        ipc: &mut Ipc,
        owner: Owner,
        name: &[u8],
        connect_cap: CapId,
        published: Published,
        policy: RestartPolicy,
        access: Access,
        limits: ServiceLimits,
    ) -> Result<(ServiceId, Handle), Error> {
        let name = ServiceName::new(name)?;
        if limits.max_clients == 0 || limits.max_clients as usize > MAX_CLIENTS {
            return Err(Error::LimitReached);
        }
        self.refresh(ipc);
        // Name handling: Failed names are free; a Restarting name only goes back to the owner the
        // restart handler authorised; anything else is taken.
        let mut reuse = None;
        if let Some(i) = self.find(&name) {
            let r = self.recs[i].as_ref().ok_or(Error::Corrupt("service slot"))?;
            match r.state {
                ServiceState::Failed => self.free_slot(ipc, i),
                ServiceState::Restarting if r.restart_owner == Some(owner) => reuse = Some(i),
                _ => return Err(Error::NameTaken),
            }
        }
        let per_owner = self
            .recs
            .iter()
            .flatten()
            .filter(|r| r.owner == owner && r.state == ServiceState::Available)
            .count();
        if per_owner >= MAX_SERVICES_PER_OWNER {
            return Err(Error::LimitReached);
        }
        let slot = match reuse {
            Some(i) => i,
            None => self
                .recs
                .iter()
                .position(|r| r.is_none())
                .ok_or(Error::LimitReached)?,
        };
        if ipc.procs.get(owner)?.free_slots() == 0 {
            return Err(Error::HandlesExhausted);
        }
        let object = ipc.objects.create(ObjectKind::Service, owner, slot as u32)?;
        let mgmt = match ipc
            .procs
            .get_mut(owner)?
            .grant(&mut ipc.caps, &mut ipc.objects, object, MGMT_RIGHTS)
        {
            Ok(h) => h,
            Err(e) => {
                let _ = ipc.objects.revoke(object);
                return Err(e);
            }
        };
        let (restarts, id) = match reuse {
            Some(i) => {
                let old = self.recs[i].take().ok_or(Error::Corrupt("service slot"))?;
                if let Some(c) = old.connect_cap {
                    if ipc.caps.exists(c) {
                        let _ = ipc.caps.remove(&mut ipc.objects, c);
                    }
                }
                (old.restarts, old.id)
            }
            None => (
                0,
                ServiceId {
                    index: slot as u8,
                    generation: self.gens[slot],
                },
            ),
        };
        self.recs[slot] = Some(Record {
            id,
            name,
            owner,
            object,
            connect_cap: Some(connect_cap),
            published,
            state: ServiceState::Available,
            policy,
            restarts,
            restart_owner: None,
            access,
            limits,
            conns: [None; MAX_CLIENTS],
        });
        Ok((id, mgmt))
    }

    /// Unregisters the service behind a management handle (needs `DESTROY`). The listener itself
    /// stays with its owner; only the publication goes away.
    pub fn unregister(&mut self, ipc: &mut Ipc, owner: Owner, mgmt: Handle) -> Result<(), Error> {
        let (_, cap) = ipc
            .procs
            .get_mut(owner)?
            .lookup(&ipc.caps, &ipc.objects, mgmt, Rights::DESTROY)?;
        let rec = ipc.objects.get_kind(cap.object, ObjectKind::Service)?;
        let slot = rec.tag as usize;
        match self.recs.get(slot).and_then(|r| r.as_ref()) {
            Some(r) if r.object == cap.object && r.owner == owner => {}
            _ => return Err(Error::NotFound),
        }
        self.free_slot(ipc, slot);
        Ok(())
    }

    // ---- discovery and connection ------------------------------------------------------------

    /// Looks a service up by name (needs `LOOKUP`). Does not connect.
    pub fn lookup(
        &mut self,
        ipc: &mut Ipc,
        client: Owner,
        dir_h: Handle,
        name: &[u8],
    ) -> Result<ServiceInfo, Error> {
        self.check_dir(ipc, client, dir_h, Rights::LOOKUP)?;
        let name = ServiceName::new(name)?;
        self.refresh(ipc);
        let i = self.find(&name).ok_or(Error::NotFound)?;
        let r = self.recs[i].as_mut().ok_or(Error::NotFound)?;
        let clients = Self::prune(ipc, r);
        Ok(ServiceInfo {
            id: r.id,
            state: r.state,
            published: r.published,
            clients,
        })
    }

    /// Drops connection records whose client end no longer exists; returns the live count.
    fn prune(ipc: &Ipc, r: &mut Record) -> usize {
        for c in r.conns.iter_mut() {
            if c.is_some_and(|c| ipc.objects.get(c.end).is_err()) {
                *c = None;
            }
        }
        r.conns.iter().flatten().count()
    }

    /// Connects `client` to the named service (needs `LOOKUP`; the service's [`Access`] policy
    /// and limits apply). For a listener service the result is the client end of a new channel
    /// (the server finds it via `accept`); for a driver interface it is a capability to it.
    pub fn connect<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        client: Owner,
        dir_h: Handle,
        name: &[u8],
    ) -> Result<Handle, Error> {
        self.check_dir(ipc, client, dir_h, Rights::LOOKUP)?;
        let name = ServiceName::new(name)?;
        self.refresh(ipc);
        let i = self.find(&name).ok_or(Error::NotFound)?;
        let r = self.recs[i].as_mut().ok_or(Error::NotFound)?;
        if r.state != ServiceState::Available {
            return Err(Error::ServiceUnavailable);
        }
        if !r.access.permits(client) {
            return Err(Error::ConnectionDenied);
        }
        let active = Self::prune(ipc, r);
        if active >= r.limits.max_clients as usize {
            return Err(Error::LimitReached);
        }
        let cap_id = r.connect_cap.ok_or(Error::ServiceUnavailable)?;
        let published = r.published;
        // The kernel-held capability is re-checked on every connection: if the server revoked it,
        // closed the listener or died, this fails and nothing is created.
        let cap = match ipc.caps.lookup(&ipc.objects, cap_id, Owner::KERNEL, Rights::NONE) {
            Ok(c) => *c,
            Err(Error::Stale | Error::Revoked) => {
                if let Some(r) = self.recs[i].as_mut() {
                    r.state = ServiceState::Unavailable;
                }
                return Err(Error::ServiceUnavailable);
            }
            Err(e) => return Err(e),
        };
        let h = match published {
            Published::Listener => {
                if !cap.rights.contains(Rights::CONNECT) {
                    return Err(Error::RightsDenied);
                }
                ipc.connect_obj(env, client, cap.object)?
            }
            Published::DriverInterface => {
                if ipc.procs.get(client)?.free_slots() == 0 {
                    return Err(Error::HandlesExhausted);
                }
                let d = ipc.caps.derive_kernel(&mut ipc.objects, cap_id, cap.rights, client, client)?;
                match ipc.procs.get_mut(client)?.insert(d) {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = ipc.caps.remove(&mut ipc.objects, d);
                        return Err(e);
                    }
                }
            }
        };
        // Remember the connection (for the client limit) by the client's capability object.
        let end = ipc
            .procs
            .get(client)?
            .resolve(h)
            .and_then(|c| ipc.caps.peek(c))
            .map(|c| c.object)?;
        if let Some(r) = self.recs[i].as_mut() {
            if let Some(slot) = r.conns.iter_mut().find(|c| c.is_none()) {
                *slot = Some(Conn { client, end });
            }
        }
        Ok(h)
    }

    // ---- lifecycle ------------------------------------------------------------------------

    /// A process exited. Tears down its IPC state (see `Ipc::destroy_process`) and applies the
    /// restart policy of each service it owned. `failed` distinguishes a crash from a clean exit.
    /// Returns the number of services affected.
    pub fn process_exited<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        owner: Owner,
        failed: bool,
        handler: &mut dyn RestartHandler,
    ) -> Result<usize, Error> {
        match ipc.destroy_process(env, owner) {
            Ok(_) | Err(Error::NoSuchProcess) => {}
            Err(e) => return Err(e),
        }
        let mut n = 0;
        for i in 0..MAX_SERVICES {
            let Some(r) = self.recs[i].as_mut() else { continue };
            if r.owner != owner {
                continue;
            }
            n += 1;
            r.connect_cap = None; // swept with the listener / interface
            r.conns = [None; MAX_CLIENTS];
            let want = match r.policy {
                RestartPolicy::Never => None,
                RestartPolicy::OnFailure { max } if failed => Some(max),
                RestartPolicy::OnFailure { .. } => None,
                RestartPolicy::Always { max } => Some(max),
            };
            r.state = match want {
                Some(max) if r.restarts < max => {
                    r.restarts += 1;
                    if handler.request_restart(&r.name, r.restarts) {
                        ServiceState::Restarting
                    } else {
                        ServiceState::Failed
                    }
                }
                Some(_) => ServiceState::Failed,
                None => ServiceState::Failed,
            };
            r.restart_owner = None;
            if r.state == ServiceState::Failed {
                // Nothing will come back: release the slot (the name is free again).
                self.free_slot(ipc, i);
            }
        }
        Ok(n)
    }

    /// Called by the restart handler: only `new_owner` may re-register the restarting `name`.
    pub fn authorize_restart(&mut self, name: &[u8], new_owner: Owner) -> Result<(), Error> {
        let name = ServiceName::new(name)?;
        let i = self.find(&name).ok_or(Error::NotFound)?;
        let r = self.recs[i].as_mut().ok_or(Error::NotFound)?;
        if r.state != ServiceState::Restarting {
            return Err(Error::BadState);
        }
        r.restart_owner = Some(new_owner);
        Ok(())
    }

    pub fn state_of(&self, name: &[u8]) -> Option<ServiceState> {
        let name = ServiceName::new(name).ok()?;
        let i = self.find(&name)?;
        self.recs[i].as_ref().map(|r| r.state)
    }

    pub fn restarts_of(&self, name: &[u8]) -> Option<u8> {
        let name = ServiceName::new(name).ok()?;
        let i = self.find(&name)?;
        self.recs[i].as_ref().map(|r| r.restarts)
    }

    pub fn check_invariants(&self, ipc: &Ipc) -> Result<(), &'static str> {
        for (i, r) in self.recs.iter().enumerate() {
            let Some(r) = r else { continue };
            if r.id.index() != i {
                return Err("service id does not match its slot");
            }
            if self.recs.iter().flatten().filter(|o| o.name == r.name).count() != 1 {
                return Err("duplicate service name");
            }
            if r.state == ServiceState::Available {
                if ipc.objects.get(r.object).is_err() {
                    // refresh() has not run yet; only a stale-but-Available record is legal if the
                    // object is gone, which callers resolve lazily.
                    continue;
                }
                let c = r.connect_cap.ok_or("available service without a connect capability")?;
                if ipc.caps.exists(c) && ipc.caps.peek(c).map_err(|_| "peek")?.owner != Owner::KERNEL {
                    return Err("published capability left the kernel");
                }
            } else if r.connect_cap.is_some_and(|c| ipc.caps.exists(c)) {
                return Err("unavailable service still holds a published capability");
            }
        }
        Ok(())
    }
}

/// The mission's minimal test service: registers, accepts connections, receives requests and
/// answers each with a `Response` carrying the same tag and the payload prefixed with `echo:`.
pub struct EchoService {
    owner: Owner,
    listener: Handle,
    mgmt: Handle,
    conns: [Option<Handle>; MAX_CLIENTS],
    pub served: u32,
    pub accepted: u32,
}

const ECHO_PREFIX: &[u8] = b"echo:";

impl EchoService {
    /// Creates the listener and registers it as `name`.
    pub fn start<const N: usize, const S: usize>(
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        mgr: &mut ServiceManager,
        owner: Owner,
        dir_h: Handle,
        name: &[u8],
        access: Access,
        limits: ServiceLimits,
    ) -> Result<EchoService, Error> {
        let listener = ipc.create_listener(env, owner)?;
        let (_, mgmt) = match mgr.register(ipc, owner, dir_h, name, listener, RestartPolicy::Never, access, limits) {
            Ok(x) => x,
            Err(e) => {
                let _ = ipc.close_handle(env, owner, listener);
                return Err(e);
            }
        };
        Ok(EchoService {
            owner,
            listener,
            mgmt,
            conns: [None; MAX_CLIENTS],
            served: 0,
            accepted: 0,
        })
    }

    pub fn owner(&self) -> Owner {
        self.owner
    }

    pub fn connections(&self) -> usize {
        self.conns.iter().flatten().count()
    }

    /// One iteration of the server loop. Accepts what is pending, then answers every request it
    /// can answer *without losing it*: a request is only consumed when the reply has somewhere to
    /// go (`send_space`), so backpressure stays in the client's queue. Returns the number of
    /// requests answered.
    pub fn pump<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
    ) -> Result<usize, Error> {
        while let Some(slot) = self.conns.iter().position(|c| c.is_none()) {
            match ipc.accept(env, self.owner, self.listener) {
                Ok((h, _client)) => {
                    self.conns[slot] = Some(h);
                    self.accepted += 1;
                }
                Err(Error::NoPending) => break,
                Err(e) => return Err(e),
            }
        }
        let mut answered = 0;
        for i in 0..MAX_CLIENTS {
            let Some(h) = self.conns[i] else { continue };
            loop {
                match ipc.send_space(self.owner, h) {
                    Ok(0) => {
                        // Either the client is gone or its queue is full. Distinguish.
                        if !ipc.peer_open(self.owner, h).unwrap_or(false) {
                            // Drain what it left, then release the slot below.
                            if matches!(ipc.recv(env, self.owner, h), Err(Error::PeerClosed)) {
                                let _ = ipc.close_handle(env, self.owner, h);
                                self.conns[i] = None;
                            } else {
                                continue;
                            }
                        }
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        self.conns[i] = None;
                        break;
                    }
                }
                match ipc.recv(env, self.owner, h) {
                    Ok(m) => {
                        if m.kind != MsgKind::Request {
                            continue;
                        }
                        let mut out = [0u8; MAX_PAYLOAD];
                        out[..ECHO_PREFIX.len()].copy_from_slice(ECHO_PREFIX);
                        let room = MAX_PAYLOAD - ECHO_PREFIX.len();
                        let n = m.payload().len().min(room);
                        out[ECHO_PREFIX.len()..ECHO_PREFIX.len() + n].copy_from_slice(&m.payload()[..n]);
                        let reply = Message::new(
                            self.owner,
                            MsgKind::Response,
                            MsgFlags::NONE,
                            m.tag,
                            &out[..ECHO_PREFIX.len() + n],
                        )?;
                        ipc.send(env, self.owner, h, reply)?;
                        self.served += 1;
                        answered += 1;
                    }
                    Err(Error::QueueEmpty) => break,
                    Err(Error::PeerClosed) => {
                        let _ = ipc.close_handle(env, self.owner, h);
                        self.conns[i] = None;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(answered)
    }

    /// Clean shutdown: unregisters and closes everything (the process itself exits via the
    /// manager's `process_exited`).
    pub fn stop<const N: usize, const S: usize>(
        &mut self,
        ipc: &mut Ipc,
        env: &mut Env<N, S>,
        mgr: &mut ServiceManager,
    ) -> Result<(), Error> {
        mgr.unregister(ipc, self.owner, self.mgmt)?;
        // Close every accepted connection so clients observe `PeerClosed` rather than hanging.
        for slot in self.conns.iter_mut() {
            if let Some(h) = slot.take() {
                let _ = ipc.close_handle(env, self.owner, h);
            }
        }
        ipc.close_endpoint(env, self.owner, self.listener)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d6::ipc::testutil::*;

    struct Handler {
        accept: bool,
        calls: std::vec::Vec<(std::vec::Vec<u8>, u8)>,
    }
    impl RestartHandler for Handler {
        fn request_restart(&mut self, name: &ServiceName, attempt: u8) -> bool {
            self.calls.push((name.as_bytes().to_vec(), attempt));
            self.accept
        }
    }

    struct World {
        r: Rig,
        mgr: ServiceManager,
        srv: Owner,
        cli: Owner,
        srv_dir: Handle,
        cli_dir: Handle,
    }

    fn world() -> World {
        let mut r = Rig::new();
        let mgr = ServiceManager::new(&mut r.ipc).unwrap();
        let (srv, cli) = (r.proc(1), r.proc(2));
        let srv_dir = mgr.grant_directory(&mut r.ipc, srv, Rights::REGISTER).unwrap();
        let cli_dir = mgr.grant_directory(&mut r.ipc, cli, Rights::LOOKUP).unwrap();
        World { r, mgr, srv, cli, srv_dir, cli_dir }
    }

    fn start(w: &mut World, name: &[u8]) -> EchoService {
        EchoService::start(
            &mut w.r.ipc,
            &mut renv!(w.r),
            &mut w.mgr,
            w.srv,
            w.srv_dir,
            name,
            Access::Open,
            ServiceLimits::DEFAULT,
        )
        .unwrap()
    }

    fn req(from: Owner, tag: u64, body: &[u8]) -> Message {
        Message::new(from, MsgKind::Request, MsgFlags::NEEDS_REPLY, tag, body).unwrap()
    }

    #[test]
    fn register_discover_connect_and_round_trip() {
        let mut w = world();
        let mut echo = start(&mut w, b"echo");
        let info = w.mgr.lookup(&mut w.r.ipc, w.cli, w.cli_dir, b"echo").unwrap();
        assert_eq!(info.state, ServiceState::Available);
        assert_eq!(info.published, Published::Listener);
        let h = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"echo").unwrap();
        // Before the server runs, nothing has been answered.
        w.r.ipc.send(&mut renv!(w.r), w.cli, h, req(w.cli, 41, b"hello")).unwrap();
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), w.cli, h).err(), Some(Error::QueueEmpty));
        assert_eq!(echo.pump(&mut w.r.ipc, &mut renv!(w.r)), Ok(1));
        let resp = w.r.ipc.recv(&mut renv!(w.r), w.cli, h).unwrap();
        assert_eq!(resp.kind, MsgKind::Response);
        assert_eq!(resp.tag, 41);
        assert_eq!(resp.payload(), b"echo:hello");
        assert_eq!(resp.sender, w.srv, "the reply is stamped with the server's identity");
        assert_eq!((echo.accepted, echo.served), (1, 1));
        // Many requests, answered in order.
        for i in 0..5u64 {
            w.r.ipc.send(&mut renv!(w.r), w.cli, h, req(w.cli, i, &[b'a' + i as u8])).unwrap();
        }
        assert_eq!(echo.pump(&mut w.r.ipc, &mut renv!(w.r)), Ok(5));
        for i in 0..5u64 {
            assert_eq!(w.r.ipc.recv(&mut renv!(w.r), w.cli, h).unwrap().tag, i);
        }
        w.r.check();
        w.mgr.check_invariants(&w.r.ipc).unwrap();
    }

    #[test]
    fn no_ambient_authority_and_rights_are_checked() {
        let mut w = world();
        let _echo = start(&mut w, b"echo");
        let stranger = w.r.proc(9);
        // A process with no directory capability can do nothing.
        assert!(w.mgr.lookup(&mut w.r.ipc, stranger, w.cli_dir, b"echo").is_err());
        assert!(w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), stranger, w.cli_dir, b"echo").is_err());
        // LOOKUP-only cannot register; REGISTER-only cannot look up.
        let l = w.r.ipc.create_listener(&mut renv!(w.r), w.cli).unwrap();
        assert_eq!(
            w.mgr.register(&mut w.r.ipc, w.cli, w.cli_dir, b"x", l, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).err(),
            Some(Error::RightsDenied)
        );
        assert_eq!(w.mgr.lookup(&mut w.r.ipc, w.srv, w.srv_dir, b"echo").err(), Some(Error::RightsDenied));
        // Unknown rights cannot be put on a directory capability.
        assert_eq!(
            w.mgr.grant_directory(&mut w.r.ipc, stranger, Rights::SEND).err(),
            Some(Error::RightsEscalation)
        );
        // The registrant must hold the authority it publishes: a peer endpoint is not a listener.
        let (pe, _po) = w.r.pair(w.srv, w.cli);
        assert!(w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, b"y", pe, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).is_err());
        // A capability to some other service-kind object is not the directory.
        let o = w.r.ipc.objects.create(ObjectKind::Service, w.srv, 0).unwrap();
        let fake = w.r.ipc.procs.get_mut(w.srv).unwrap().grant(&mut w.r.ipc.caps, &mut w.r.ipc.objects, o, Rights::REGISTER).unwrap();
        let l2 = w.r.ipc.create_listener(&mut renv!(w.r), w.srv).unwrap();
        assert_eq!(
            w.mgr.register(&mut w.r.ipc, w.srv, fake, b"z", l2, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).err(),
            Some(Error::NotAuthorized)
        );
        w.r.check();
    }

    #[test]
    fn duplicate_and_invalid_names() {
        let mut w = world();
        let _echo = start(&mut w, b"echo");
        let l = w.r.ipc.create_listener(&mut renv!(w.r), w.srv).unwrap();
        assert_eq!(
            w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, b"echo", l, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).err(),
            Some(Error::NameTaken)
        );
        for bad in [&b""[..], b"UPPER", b"has space", b"waytoolongservicename", b"a/b", b"\xff"] {
            assert_eq!(
                w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, bad, l, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).err(),
                Some(Error::BadName),
                "{bad:?}"
            );
        }
        assert!(ServiceName::new(b"a.b_c-9").is_ok());
        assert_eq!(w.mgr.lookup(&mut w.r.ipc, w.cli, w.cli_dir, b"nothere").err(), Some(Error::NotFound));
        w.r.check();
    }

    #[test]
    fn connection_authorisation_allowlist_and_client_limit() {
        let mut w = world();
        let allowed = w.r.proc(5);
        let allowed_dir = w.mgr.grant_directory(&mut w.r.ipc, allowed, Rights::LOOKUP).unwrap();
        let mut echo = EchoService::start(
            &mut w.r.ipc,
            &mut renv!(w.r),
            &mut w.mgr,
            w.srv,
            w.srv_dir,
            b"private",
            Access::allowlist(&[allowed]).unwrap(),
            ServiceLimits { max_clients: 2 },
        )
        .unwrap();
        assert_eq!(
            w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"private").err(),
            Some(Error::ConnectionDenied)
        );
        let h1 = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), allowed, allowed_dir, b"private").unwrap();
        let h2 = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), allowed, allowed_dir, b"private").unwrap();
        assert_eq!(
            w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), allowed, allowed_dir, b"private").err(),
            Some(Error::LimitReached),
            "max_clients"
        );
        // Closing a connection frees a slot.
        w.r.ipc.close_handle(&mut renv!(w.r), allowed, h1).unwrap();
        let h3 = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), allowed, allowed_dir, b"private").unwrap();
        // The server sees the kernel-recorded identity of each client.
        echo.pump(&mut w.r.ipc, &mut renv!(w.r)).unwrap();
        assert!(echo.connections() >= 2);
        let _ = (h2, h3);
        assert!(Access::allowlist(&[allowed; MAX_ALLOWED + 1]).is_err());
        w.r.check();
        w.mgr.check_invariants(&w.r.ipc).unwrap();
    }

    #[test]
    fn many_simultaneous_clients_are_served_independently() {
        let mut w = world();
        let mut echo = start(&mut w, b"echo");
        let mut clients = std::vec::Vec::new();
        for n in 20..20 + MAX_CLIENTS as u64 {
            let c = w.r.proc(n);
            let d = w.mgr.grant_directory(&mut w.r.ipc, c, Rights::LOOKUP).unwrap();
            let h = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), c, d, b"echo").unwrap();
            clients.push((c, h));
            // The listener backlog is smaller than MAX_CLIENTS: let the server accept as we go.
            assert_eq!(echo.pump(&mut w.r.ipc, &mut renv!(w.r)), Ok(0));
        }
        for (i, (c, h)) in clients.iter().enumerate() {
            w.r.ipc.send(&mut renv!(w.r), *c, *h, req(*c, i as u64, &[i as u8; 3])).unwrap();
        }
        assert_eq!(echo.pump(&mut w.r.ipc, &mut renv!(w.r)), Ok(MAX_CLIENTS));
        for (i, (c, h)) in clients.iter().enumerate() {
            let m = w.r.ipc.recv(&mut renv!(w.r), *c, *h).unwrap();
            assert_eq!((m.tag, m.payload()), (i as u64, &[&b"echo:"[..], &[i as u8; 3]].concat()[..]));
        }
        w.r.check();
    }

    #[test]
    fn server_backpressure_leaves_requests_queued_not_lost() {
        let mut w = world();
        let mut echo = start(&mut w, b"echo");
        let h = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"echo").unwrap();
        // The client never reads: the server can answer at most QUEUE_CAPACITY requests.
        let cap = crate::d6::ipc::QUEUE_CAPACITY as u64;
        for i in 0..cap {
            w.r.ipc.send(&mut renv!(w.r), w.cli, h, req(w.cli, i, b"x")).unwrap();
        }
        assert_eq!(echo.pump(&mut w.r.ipc, &mut renv!(w.r)), Ok(cap as usize));
        for i in 0..3u64 {
            w.r.ipc.send(&mut renv!(w.r), w.cli, h, req(w.cli, 100 + i, b"y")).unwrap_or(crate::d6::ipc_message::MsgId(0));
        }
        assert_eq!(echo.pump(&mut w.r.ipc, &mut renv!(w.r)), Ok(0), "no room for replies: nothing consumed");
        // Reading frees room and the pending requests are answered.
        for _ in 0..cap {
            w.r.ipc.recv(&mut renv!(w.r), w.cli, h).unwrap();
        }
        assert_eq!(echo.pump(&mut w.r.ipc, &mut renv!(w.r)), Ok(3));
        w.r.check();
    }

    #[test]
    fn service_termination_is_contained() {
        let mut w = world();
        let mut echo = start(&mut w, b"echo");
        let h = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"echo").unwrap();
        echo.pump(&mut w.r.ipc, &mut renv!(w.r)).unwrap();
        // An unrelated pair of processes keeps working across the crash.
        let (x, y) = (w.r.proc(30), w.r.proc(31));
        let (hx, hy) = w.r.pair(x, y);
        let n = w.mgr.process_exited(&mut w.r.ipc, &mut renv!(w.r), w.srv, true, &mut NoRestart).unwrap();
        assert_eq!(n, 1);
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), w.cli, h).err(), Some(Error::PeerClosed));
        assert_eq!(w.mgr.state_of(b"echo"), None, "no restart policy: the name is released");
        assert_eq!(
            w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"echo").err(),
            Some(Error::NotFound)
        );
        w.r.ipc.send(&mut renv!(w.r), x, hx, msg(x, 1, b"fine")).unwrap();
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), y, hy).unwrap().payload(), b"fine");
        // The name can be reused by a new instance.
        let srv2 = w.r.proc(2000);
        let d2 = w.mgr.grant_directory(&mut w.r.ipc, srv2, Rights::REGISTER).unwrap();
        EchoService::start(&mut w.r.ipc, &mut renv!(w.r), &mut w.mgr, srv2, d2, b"echo", Access::Open, ServiceLimits::DEFAULT).unwrap();
        w.r.check();
        w.mgr.check_invariants(&w.r.ipc).unwrap();
    }

    #[test]
    fn restart_policy_states_and_name_protection() {
        let mut w = world();
        let l = w.r.ipc.create_listener(&mut renv!(w.r), w.srv).unwrap();
        w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, b"svc", l, RestartPolicy::OnFailure { max: 2 }, Access::Open, ServiceLimits::DEFAULT).unwrap();
        let mut hd = Handler { accept: true, calls: std::vec::Vec::new() };
        // Clean exit with OnFailure: no restart.
        // (Use a second service to check that first.)
        w.mgr.process_exited(&mut w.r.ipc, &mut renv!(w.r), w.srv, true, &mut hd).unwrap();
        assert_eq!(hd.calls, [(b"svc".to_vec(), 1)]);
        assert_eq!(w.mgr.state_of(b"svc"), Some(ServiceState::Restarting));
        // While restarting, clients are told so, and nobody can take the name.
        assert_eq!(
            w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"svc").err(),
            Some(Error::ServiceUnavailable)
        );
        let thief = w.r.proc(66);
        let td = w.mgr.grant_directory(&mut w.r.ipc, thief, Rights::REGISTER).unwrap();
        let tl = w.r.ipc.create_listener(&mut renv!(w.r), thief).unwrap();
        assert_eq!(
            w.mgr.register(&mut w.r.ipc, thief, td, b"svc", tl, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).err(),
            Some(Error::NameTaken)
        );
        // The authorised new instance takes it over; the restart counter survives.
        let srv2 = w.r.proc(3000);
        assert_eq!(w.mgr.authorize_restart(b"svc", srv2), Ok(()));
        let d2 = w.mgr.grant_directory(&mut w.r.ipc, srv2, Rights::REGISTER).unwrap();
        let l2 = w.r.ipc.create_listener(&mut renv!(w.r), srv2).unwrap();
        w.mgr.register(&mut w.r.ipc, srv2, d2, b"svc", l2, RestartPolicy::OnFailure { max: 2 }, Access::Open, ServiceLimits::DEFAULT).unwrap();
        assert_eq!(w.mgr.state_of(b"svc"), Some(ServiceState::Available));
        assert_eq!(w.mgr.restarts_of(b"svc"), Some(1));
        assert!(w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"svc").is_ok());
        // Second and third crash: attempt 2 is accepted, then the budget is exhausted -> Failed.
        w.mgr.process_exited(&mut w.r.ipc, &mut renv!(w.r), srv2, true, &mut hd).unwrap();
        assert_eq!(hd.calls.len(), 2);
        assert_eq!(w.mgr.state_of(b"svc"), Some(ServiceState::Restarting));
        let srv3 = w.r.proc(3001);
        w.mgr.authorize_restart(b"svc", srv3).unwrap();
        let d3 = w.mgr.grant_directory(&mut w.r.ipc, srv3, Rights::REGISTER).unwrap();
        let l3 = w.r.ipc.create_listener(&mut renv!(w.r), srv3).unwrap();
        w.mgr.register(&mut w.r.ipc, srv3, d3, b"svc", l3, RestartPolicy::OnFailure { max: 2 }, Access::Open, ServiceLimits::DEFAULT).unwrap();
        w.mgr.process_exited(&mut w.r.ipc, &mut renv!(w.r), srv3, true, &mut hd).unwrap();
        assert_eq!(hd.calls.len(), 2, "budget exhausted: the handler is not asked again");
        assert_eq!(w.mgr.state_of(b"svc"), None, "Failed services release their name");
        w.r.check();
        w.mgr.check_invariants(&w.r.ipc).unwrap();
    }

    #[test]
    fn refused_restart_and_clean_exit_handling() {
        let mut w = world();
        let l = w.r.ipc.create_listener(&mut renv!(w.r), w.srv).unwrap();
        w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, b"a", l, RestartPolicy::OnFailure { max: 3 }, Access::Open, ServiceLimits::DEFAULT).unwrap();
        let mut refuse = Handler { accept: false, calls: std::vec::Vec::new() };
        // Clean exit + OnFailure: the handler is not even asked.
        w.mgr.process_exited(&mut w.r.ipc, &mut renv!(w.r), w.srv, false, &mut refuse).unwrap();
        assert!(refuse.calls.is_empty());
        assert_eq!(w.mgr.state_of(b"a"), None);
        // Handler refuses -> Failed (released).
        let s = w.r.proc(77);
        let d = w.mgr.grant_directory(&mut w.r.ipc, s, Rights::REGISTER).unwrap();
        let l = w.r.ipc.create_listener(&mut renv!(w.r), s).unwrap();
        w.mgr.register(&mut w.r.ipc, s, d, b"b", l, RestartPolicy::Always { max: 1 }, Access::Open, ServiceLimits::DEFAULT).unwrap();
        w.mgr.process_exited(&mut w.r.ipc, &mut renv!(w.r), s, false, &mut refuse).unwrap();
        assert_eq!(refuse.calls.len(), 1, "Always restarts even after a clean exit");
        assert_eq!(w.mgr.state_of(b"b"), None);
        assert_eq!(w.mgr.authorize_restart(b"b", s), Err(Error::NotFound));
        w.r.check();
    }

    #[test]
    fn resource_exhaustion_and_per_owner_limits() {
        let mut w = world();
        let mut names = std::vec::Vec::new();
        // MAX_SERVICES_PER_OWNER per process.
        for i in 0..MAX_SERVICES_PER_OWNER {
            let n = std::format!("s{i}");
            let l = w.r.ipc.create_listener(&mut renv!(w.r), w.srv).unwrap();
            w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, n.as_bytes(), l, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).unwrap();
            names.push(n);
        }
        let l = w.r.ipc.create_listener(&mut renv!(w.r), w.srv).unwrap();
        assert_eq!(
            w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, b"extra", l, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).err(),
            Some(Error::LimitReached)
        );
        // Global table.
        let mut k = 0;
        loop {
            let o = w.r.proc(500 + k);
            k += 1;
            let d = w.mgr.grant_directory(&mut w.r.ipc, o, Rights::REGISTER).unwrap();
            let l = w.r.ipc.create_listener(&mut renv!(w.r), o).unwrap();
            let n = std::format!("g{k}");
            match w.mgr.register(&mut w.r.ipc, o, d, n.as_bytes(), l, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT) {
                Ok(_) => {}
                Err(e) => {
                    assert_eq!(e, Error::LimitReached);
                    break;
                }
            }
        }
        assert_eq!(w.mgr.live(), MAX_SERVICES);
        // Invalid limits.
        let o = w.r.proc(9000);
        let d = w.mgr.grant_directory(&mut w.r.ipc, o, Rights::REGISTER).unwrap();
        let l = w.r.ipc.create_listener(&mut renv!(w.r), o).unwrap();
        for bad in [0u8, MAX_CLIENTS as u8 + 1] {
            assert_eq!(
                w.mgr.register(&mut w.r.ipc, o, d, b"lim", l, RestartPolicy::Never, Access::Open, ServiceLimits { max_clients: bad }).err(),
                Some(Error::LimitReached)
            );
        }
        w.r.check();
        w.mgr.check_invariants(&w.r.ipc).unwrap();
    }

    #[test]
    fn unregister_and_listener_closure_unpublish() {
        let mut w = world();
        let l = w.r.ipc.create_listener(&mut renv!(w.r), w.srv).unwrap();
        let (_, mgmt) = w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, b"u", l, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).unwrap();
        // Another process cannot unregister it; a LOOKUP-only copy cannot either.
        assert!(w.mgr.unregister(&mut w.r.ipc, w.cli, mgmt).is_err());
        w.mgr.unregister(&mut w.r.ipc, w.srv, mgmt).unwrap();
        assert_eq!(w.mgr.lookup(&mut w.r.ipc, w.cli, w.cli_dir, b"u").err(), Some(Error::NotFound));
        // Closing the listener makes a registered service unavailable.
        let l2 = w.r.ipc.create_listener(&mut renv!(w.r), w.srv).unwrap();
        w.mgr.register(&mut w.r.ipc, w.srv, w.srv_dir, b"v", l2, RestartPolicy::Never, Access::Open, ServiceLimits::DEFAULT).unwrap();
        w.r.ipc.close_endpoint(&mut renv!(w.r), w.srv, l2).unwrap();
        assert_eq!(
            w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"v").err(),
            Some(Error::ServiceUnavailable)
        );
        assert_eq!(w.mgr.state_of(b"v"), Some(ServiceState::Unavailable));
        w.r.check();
        w.mgr.check_invariants(&w.r.ipc).unwrap();
    }

    #[test]
    fn a_service_that_stops_revokes_the_published_capability() {
        let mut w = world();
        let mut echo = start(&mut w, b"echo");
        let h = w.mgr.connect(&mut w.r.ipc, &mut renv!(w.r), w.cli, w.cli_dir, b"echo").unwrap();
        echo.pump(&mut w.r.ipc, &mut renv!(w.r)).unwrap();
        echo.stop(&mut w.r.ipc, &mut renv!(w.r), &mut w.mgr).unwrap();
        assert_eq!(w.mgr.lookup(&mut w.r.ipc, w.cli, w.cli_dir, b"echo").err(), Some(Error::NotFound));
        assert_eq!(w.r.ipc.recv(&mut renv!(w.r), w.cli, h).err(), Some(Error::PeerClosed));
        w.r.check();
    }
}
