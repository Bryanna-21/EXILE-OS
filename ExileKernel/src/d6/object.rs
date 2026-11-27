//! Kernel-object foundation (Mission D6, file 2). SPDX-License-Identifier: Apache-2.0
//!
//! Every kernel object that user processes can reach through a capability (IPC endpoints,
//! channels, process references, service registrations, driver interfaces) is one slot of an
//! [`ObjectTable`]. User code never sees an [`ObjectId`]; it only ever holds process-local
//! handles (`handle.rs`) that resolve to capabilities (`capability.rs`) that name an object.
//!
//! # Identity
//! An [`ObjectId`] is `(slot index, generation)`. Destroying an object bumps the slot's
//! generation, so an id that outlives its object can never resolve to a newer object that
//! reuses the slot. A slot whose generation counter would wrap is *retired* (never reused).
//!
//! # Lifecycle
//! `Active` -> `Revoked` -> destroyed (slot freed, generation bumped).
//! * `Active`: lookups succeed, new references may be taken.
//! * `Revoked`: lookups fail with [`Error::Revoked`]; no new references or operations may start;
//!   existing counters may still drain.
//! * The object is destroyed automatically as soon as it is `Revoked` and **all three counters
//!   are zero**, and never earlier. [`ObjectTable::destroy`] is the strict variant: it fails
//!   with [`Error::Busy`] unless the counters are already zero.
//!
//! # Reference accounting (three independent counters)
//! * `refs`: capabilities naming the object (one per live capability).
//! * `inflight`: queued messages that reference the object.
//! * `ops`: operations in progress (e.g. a blocked sender).
//!
//! Counter underflow and overflow are errors ([`Error::RefUnderflow`], [`Error::RefOverflow`]),
//! never wraps and never panics, so a double release is detected instead of corrupting state.
//!
//! # Ownership
//! Every object has an [`Owner`] (a process, or the kernel). Per-owner object counts are limited
//! (`quota`), and the whole table is bounded by `N`.
//!
//! # Locking
//! The table is plain data. A kernel instance wraps the whole IPC state in one lock; the global
//! D6 lock order is documented in `ipc.rs`.
#![allow(dead_code)]

use crate::process::Pid;
use crate::sync;
use crate::thread;

/// The single error type of the D6 subsystem. One enum keeps conversions trivial and lets
/// tests assert exact outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    // ---- identity / lifetime ----
    Stale,
    Revoked,
    WrongKind,
    // ---- resources ----
    ObjectsExhausted,
    CapsExhausted,
    HandlesExhausted,
    EndpointsExhausted,
    ChannelsExhausted,
    ProcessesExhausted,
    WaitQueuesExhausted,
    QuotaExceeded,
    Busy,
    RefUnderflow,
    RefOverflow,
    // ---- rights ----
    UnknownRights,
    RightsDenied,
    RightsEscalation,
    DepthExceeded,
    NotOwner,
    BadHandle,
    ReservedOwner,
    NoSuchProcess,
    ProcessExists,
    // ---- IPC ----
    QueueFull,
    QueueEmpty,
    PeerClosed,
    EndpointClosed,
    Cancelled,
    TimedOut,
    WouldBlock,
    BlockInInterrupt,
    PreemptDisabled,
    NoCurrentThread,
    NotListener,
    BacklogFull,
    NoPending,
    ConnectionDenied,
    // ---- messages ----
    BadVersion,
    BadKind,
    BadFlags,
    BadHeader,
    ReservedNonZero,
    PayloadTooLarge,
    TooManyTransfers,
    SizeOverflow,
    Truncated,
    TrailingBytes,
    BufferTooSmall,
    // ---- transfer ----
    DuplicateTransfer,
    TransferRevoked,
    DestinationFull,
    // ---- services / drivers ----
    NameTaken,
    BadName,
    NotFound,
    ServiceUnavailable,
    LimitReached,
    BadState,
    NotAuthorized,
    DriverFailed,
    BadRequest,
    BadResponse,
    // ---- plumbing ----
    Config,
    Corrupt(&'static str),
    Sched(thread::Error),
    Sync(sync::Error),
}

impl From<thread::Error> for Error {
    fn from(e: thread::Error) -> Self {
        Error::Sched(e)
    }
}

impl From<sync::Error> for Error {
    fn from(e: sync::Error) -> Self {
        Error::Sync(e)
    }
}

/// Who owns an object or capability. Processes map to `Owner` through their D5 [`Pid`]; the two
/// highest values are reserved for the kernel itself and for capabilities in flight inside a
/// message queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Owner(u64);

impl Owner {
    pub const KERNEL: Owner = Owner(u64::MAX);
    pub const INFLIGHT: Owner = Owner(u64::MAX - 1);

    /// A process owner from a raw id. The reserved values are refused.
    pub const fn process(raw: u64) -> Result<Owner, Error> {
        if raw >= u64::MAX - 1 {
            Err(Error::ReservedOwner)
        } else {
            Ok(Owner(raw))
        }
    }

    /// The owner for a D5 process id (`ProcessTable::pid`).
    pub fn from_pid(pid: Pid) -> Result<Owner, Error> {
        Owner::process(pid.0)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn is_process(self) -> bool {
        self.0 < u64::MAX - 1
    }
}

/// What an object is. Classification is fixed at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Endpoint,
    Channel,
    Process,
    Service,
    DriverInterface,
}

/// Generation-checked object identity. Never exposed to user code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectId {
    index: u16,
    generation: u32,
}

impl ObjectId {
    pub const fn index(self) -> usize {
        self.index as usize
    }
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectState {
    Active,
    Revoked,
}

/// A live object. `tag` is an index into the subsystem table that owns the object's payload
/// (endpoint table, channel table, ...); it is kernel-internal and never user-controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectRec {
    pub kind: ObjectKind,
    pub owner: Owner,
    pub state: ObjectState,
    pub tag: u32,
    pub refs: u32,
    pub inflight: u32,
    pub ops: u32,
}

impl ObjectRec {
    pub const fn idle(&self) -> bool {
        self.refs == 0 && self.inflight == 0 && self.ops == 0
    }
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u32,
    retired: bool,
    rec: Option<ObjectRec>,
}

/// Which counter an operation touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    Refs,
    Inflight,
    Ops,
}

pub struct ObjectTable<const N: usize> {
    slots: [Slot; N],
    quota: usize,
    live: usize,
}

impl<const N: usize> Default for ObjectTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ObjectTable<N> {
    pub const fn new() -> Self {
        Self::with_quota(N)
    }

    /// `quota` is the maximum number of live objects per owner.
    pub const fn with_quota(quota: usize) -> Self {
        Self {
            slots: [Slot {
                generation: 0,
                retired: false,
                rec: None,
            }; N],
            quota,
            live: 0,
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn live(&self) -> usize {
        self.live
    }

    pub fn owned_by(&self, owner: Owner) -> usize {
        self.slots
            .iter()
            .filter(|s| s.rec.is_some_and(|r| r.owner == owner))
            .count()
    }

    /// Creates an `Active` object with all counters zero.
    pub fn create(&mut self, kind: ObjectKind, owner: Owner, tag: u32) -> Result<ObjectId, Error> {
        if owner.is_process() && self.owned_by(owner) >= self.quota {
            return Err(Error::QuotaExceeded);
        }
        let i = self
            .slots
            .iter()
            .position(|s| s.rec.is_none() && !s.retired)
            .ok_or(Error::ObjectsExhausted)?;
        let s = &mut self.slots[i];
        s.rec = Some(ObjectRec {
            kind,
            owner,
            state: ObjectState::Active,
            tag,
            refs: 0,
            inflight: 0,
            ops: 0,
        });
        self.live += 1;
        Ok(ObjectId {
            index: i as u16,
            generation: s.generation,
        })
    }

    fn slot(&self, id: ObjectId) -> Result<&ObjectRec, Error> {
        let s = self.slots.get(id.index()).ok_or(Error::Stale)?;
        match &s.rec {
            Some(r) if s.generation == id.generation => Ok(r),
            _ => Err(Error::Stale),
        }
    }

    fn slot_mut(&mut self, id: ObjectId) -> Result<&mut ObjectRec, Error> {
        let s = self.slots.get_mut(id.index()).ok_or(Error::Stale)?;
        if s.generation != id.generation {
            return Err(Error::Stale);
        }
        s.rec.as_mut().ok_or(Error::Stale)
    }

    /// Safe lookup: succeeds only for a current, `Active` object.
    pub fn get(&self, id: ObjectId) -> Result<&ObjectRec, Error> {
        let r = self.slot(id)?;
        match r.state {
            ObjectState::Active => Ok(r),
            ObjectState::Revoked => Err(Error::Revoked),
        }
    }

    /// Like [`get`](Self::get) but also checks the kind.
    pub fn get_kind(&self, id: ObjectId, kind: ObjectKind) -> Result<&ObjectRec, Error> {
        let r = self.get(id)?;
        if r.kind == kind {
            Ok(r)
        } else {
            Err(Error::WrongKind)
        }
    }

    /// Lookup that also sees `Revoked` objects (teardown paths only).
    pub fn get_any(&self, id: ObjectId) -> Result<&ObjectRec, Error> {
        self.slot(id)
    }

    pub fn exists(&self, id: ObjectId) -> bool {
        self.slot(id).is_ok()
    }

    /// Takes one unit of `counter`. Requires an `Active` object.
    pub fn acquire(&mut self, id: ObjectId, counter: Counter) -> Result<(), Error> {
        self.get(id)?;
        let r = self.slot_mut(id)?;
        let c = match counter {
            Counter::Refs => &mut r.refs,
            Counter::Inflight => &mut r.inflight,
            Counter::Ops => &mut r.ops,
        };
        *c = c.checked_add(1).ok_or(Error::RefOverflow)?;
        Ok(())
    }

    /// Releases one unit of `counter` (allowed on `Revoked` objects so they can drain). Destroys
    /// the object if it is `Revoked` and now idle. Returns whether it was destroyed.
    pub fn release(&mut self, id: ObjectId, counter: Counter) -> Result<bool, Error> {
        let r = self.slot_mut(id)?;
        let c = match counter {
            Counter::Refs => &mut r.refs,
            Counter::Inflight => &mut r.inflight,
            Counter::Ops => &mut r.ops,
        };
        *c = c.checked_sub(1).ok_or(Error::RefUnderflow)?;
        self.reap(id)
    }

    /// Revokes the object: lookups now fail. Destroys it immediately if it is already idle.
    /// Idempotent. Returns whether it was destroyed.
    pub fn revoke(&mut self, id: ObjectId) -> Result<bool, Error> {
        self.slot_mut(id)?.state = ObjectState::Revoked;
        self.reap(id)
    }

    /// Strict destruction: only an idle object (any state) may be destroyed this way.
    pub fn destroy(&mut self, id: ObjectId) -> Result<(), Error> {
        if !self.slot(id)?.idle() {
            return Err(Error::Busy);
        }
        self.free(id);
        Ok(())
    }

    fn reap(&mut self, id: ObjectId) -> Result<bool, Error> {
        let r = self.slot(id)?;
        if r.state == ObjectState::Revoked && r.idle() {
            self.free(id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn free(&mut self, id: ObjectId) {
        let s = &mut self.slots[id.index()];
        s.rec = None;
        match s.generation.checked_add(1) {
            Some(g) => s.generation = g,
            None => s.retired = true,
        }
        self.live -= 1;
    }

    /// Moves an object to a new owner, enforcing the new owner's quota.
    pub fn set_owner(&mut self, id: ObjectId, owner: Owner) -> Result<(), Error> {
        let cur = self.get(id)?.owner;
        if cur == owner {
            return Ok(());
        }
        if owner.is_process() && self.owned_by(owner) >= self.quota {
            return Err(Error::QuotaExceeded);
        }
        self.slot_mut(id)?.owner = owner;
        Ok(())
    }

    pub fn set_tag(&mut self, id: ObjectId, tag: u32) -> Result<(), Error> {
        self.slot_mut(id)?.tag = tag;
        Ok(())
    }

    /// Ids of every live object (revoked included) owned by `owner`, written to `out`; returns
    /// the number written (excess is not truncated silently: see the `Err`).
    pub fn collect_owned(&self, owner: Owner, out: &mut [ObjectId]) -> Result<usize, Error> {
        let mut n = 0;
        for (i, s) in self.slots.iter().enumerate() {
            if s.rec.is_some_and(|r| r.owner == owner) {
                *out.get_mut(n).ok_or(Error::BufferTooSmall)? = ObjectId {
                    index: i as u16,
                    generation: s.generation,
                };
                n += 1;
            }
        }
        Ok(n)
    }

    /// Every live object (revoked ones included), for diagnostics and cross-table checks.
    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &ObjectRec)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.rec.as_ref().map(|r| {
                (
                    ObjectId {
                        index: i as u16,
                        generation: s.generation,
                    },
                    r,
                )
            })
        })
    }

    /// Structural self-check.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        let mut live = 0;
        for s in &self.slots {
            if let Some(r) = &s.rec {
                live += 1;
                if s.retired {
                    return Err("retired slot holds an object");
                }
                if r.state == ObjectState::Revoked && r.idle() {
                    return Err("idle revoked object was not destroyed");
                }
            }
        }
        if live != self.live {
            return Err("live count differs from the slots");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_generation_for_test(&mut self, index: usize, generation: u32) {
        self.slots[index].generation = generation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type T = ObjectTable<8>;

    fn o(n: u64) -> Owner {
        Owner::process(n).unwrap()
    }

    #[test]
    fn owner_reserved_values_are_refused() {
        assert_eq!(Owner::process(u64::MAX), Err(Error::ReservedOwner));
        assert_eq!(Owner::process(u64::MAX - 1), Err(Error::ReservedOwner));
        assert!(Owner::process(1).unwrap().is_process());
        assert!(!Owner::KERNEL.is_process());
        assert_eq!(Owner::from_pid(Pid(7)), Ok(o(7)));
        assert_eq!(Owner::from_pid(Pid(u64::MAX)), Err(Error::ReservedOwner));
    }

    #[test]
    fn create_lookup_and_kind_checks() {
        let mut t = T::new();
        let a = t.create(ObjectKind::Endpoint, o(1), 5).unwrap();
        assert_eq!(t.get(a).unwrap().tag, 5);
        assert_eq!(t.get(a).unwrap().owner, o(1));
        assert!(t.get_kind(a, ObjectKind::Endpoint).is_ok());
        assert_eq!(t.get_kind(a, ObjectKind::Channel), Err(Error::WrongKind));
        assert_eq!(t.live(), 1);
        t.check_invariants().unwrap();
    }

    #[test]
    fn stale_ids_never_resolve_to_new_objects() {
        let mut t = T::new();
        let a = t.create(ObjectKind::Channel, o(1), 0).unwrap();
        t.destroy(a).unwrap();
        assert_eq!(t.get(a).err(), Some(Error::Stale));
        let b = t.create(ObjectKind::Service, o(2), 0).unwrap();
        assert_eq!(a.index(), b.index(), "slot is reused");
        assert_ne!(a.generation(), b.generation());
        assert_eq!(t.get(a).err(), Some(Error::Stale), "old id must stay dead");
        assert_eq!(t.get(b).unwrap().kind, ObjectKind::Service);
        assert_eq!(t.release(a, Counter::Refs), Err(Error::Stale));
    }

    #[test]
    fn generation_wrap_retires_the_slot() {
        let mut t = ObjectTable::<1>::new();
        t.set_generation_for_test(0, u32::MAX);
        let a = t.create(ObjectKind::Endpoint, o(1), 0).unwrap();
        t.destroy(a).unwrap();
        assert_eq!(
            t.create(ObjectKind::Endpoint, o(1), 0),
            Err(Error::ObjectsExhausted),
            "a wrapped generation must never be reused"
        );
        assert_eq!(t.get(a).err(), Some(Error::Stale));
        t.check_invariants().unwrap();
    }

    #[test]
    fn counters_gate_destruction() {
        let mut t = T::new();
        let a = t.create(ObjectKind::Channel, o(1), 0).unwrap();
        t.acquire(a, Counter::Refs).unwrap();
        t.acquire(a, Counter::Inflight).unwrap();
        t.acquire(a, Counter::Ops).unwrap();
        assert_eq!(t.destroy(a), Err(Error::Busy));
        // Revocation hides the object but does not free it while counters are held.
        assert_eq!(t.revoke(a), Ok(false));
        assert_eq!(t.get(a).err(), Some(Error::Revoked));
        assert!(t.get_any(a).is_ok());
        assert_eq!(t.acquire(a, Counter::Refs), Err(Error::Revoked));
        assert_eq!(t.release(a, Counter::Refs), Ok(false));
        assert_eq!(t.release(a, Counter::Inflight), Ok(false));
        assert!(t.exists(a));
        assert_eq!(t.release(a, Counter::Ops), Ok(true), "last release frees");
        assert!(!t.exists(a));
        assert_eq!(t.live(), 0);
        t.check_invariants().unwrap();
    }

    #[test]
    fn double_release_is_an_error_not_a_panic() {
        let mut t = T::new();
        let a = t.create(ObjectKind::Channel, o(1), 0).unwrap();
        assert_eq!(t.release(a, Counter::Refs), Err(Error::RefUnderflow));
        t.acquire(a, Counter::Refs).unwrap();
        t.release(a, Counter::Refs).unwrap();
        assert_eq!(t.release(a, Counter::Refs), Err(Error::RefUnderflow));
        assert_eq!(t.release(a, Counter::Inflight), Err(Error::RefUnderflow));
        assert_eq!(t.release(a, Counter::Ops), Err(Error::RefUnderflow));
    }

    #[test]
    fn revoking_an_idle_object_frees_it_and_is_idempotent_until_then() {
        let mut t = T::new();
        let a = t.create(ObjectKind::Endpoint, o(1), 0).unwrap();
        assert_eq!(t.revoke(a), Ok(true));
        assert_eq!(t.revoke(a), Err(Error::Stale));
        let b = t.create(ObjectKind::Endpoint, o(1), 0).unwrap();
        t.acquire(b, Counter::Refs).unwrap();
    }

    #[test]
    fn exhaustion_and_per_owner_quota() {
        let mut t = ObjectTable::<4>::with_quota(2);
        let a = t.create(ObjectKind::Endpoint, o(1), 0).unwrap();
        t.create(ObjectKind::Endpoint, o(1), 0).unwrap();
        assert_eq!(
            t.create(ObjectKind::Endpoint, o(1), 0),
            Err(Error::QuotaExceeded)
        );
        t.create(ObjectKind::Endpoint, o(2), 0).unwrap();
        t.create(ObjectKind::Endpoint, o(2), 0).unwrap();
        assert_eq!(
            t.create(ObjectKind::Endpoint, o(3), 0),
            Err(Error::ObjectsExhausted)
        );
        assert_eq!(t.owned_by(o(1)), 2);
        t.destroy(a).unwrap();
        assert!(t.create(ObjectKind::Endpoint, o(3), 0).is_ok());
        t.check_invariants().unwrap();
    }

    #[test]
    fn ownership_transfer_respects_quota() {
        let mut t = ObjectTable::<4>::with_quota(1);
        let a = t.create(ObjectKind::Endpoint, o(1), 0).unwrap();
        let b = t.create(ObjectKind::Endpoint, o(2), 0).unwrap();
        assert_eq!(t.set_owner(a, o(2)), Err(Error::QuotaExceeded));
        assert_eq!(t.get(a).unwrap().owner, o(1));
        t.destroy(b).unwrap();
        t.set_owner(a, o(2)).unwrap();
        assert_eq!(t.get(a).unwrap().owner, o(2));
        let mut buf = [a; 1];
        assert_eq!(t.collect_owned(o(2), &mut buf), Ok(1));
        let mut none: [ObjectId; 0] = [];
        assert_eq!(t.collect_owned(o(2), &mut none), Err(Error::BufferTooSmall));
    }

    #[test]
    fn all_object_kinds_are_supported() {
        let mut t = T::new();
        for k in [
            ObjectKind::Endpoint,
            ObjectKind::Channel,
            ObjectKind::Process,
            ObjectKind::Service,
            ObjectKind::DriverInterface,
        ] {
            let id = t.create(k, Owner::KERNEL, 0).unwrap();
            assert_eq!(t.get(id).unwrap().kind, k);
        }
        assert_eq!(t.live(), 5);
    }
}
