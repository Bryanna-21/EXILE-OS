//! Kernel capability system (Mission D6, file 1). SPDX-License-Identifier: Apache-2.0
//!
//! A capability is an unforgeable kernel record `(object, rights, owner)`. It is the *only*
//! way a process names a kernel object. User code never holds a [`CapId`] or an object id: it
//! holds process-local handles (`handle.rs`) which the kernel resolves to a `CapId`.
//!
//! # Rules enforced (all by construction, not by convention)
//! * **Rights are a closed set.** [`Rights::from_bits`] rejects unknown bits
//!   ([`Error::UnknownRights`]); there is no way to build a `Rights` with an unknown bit.
//! * **No escalation.** A derived capability's rights must be a subset of its parent's
//!   ([`Error::RightsEscalation`] otherwise), and deriving needs the parent to hold
//!   [`Rights::DERIVE`]. The invariant `child.rights ⊆ parent.rights` is re-checked by
//!   [`CapTable::check_invariants`].
//! * **Lookup enforces the operation's rights.** [`CapTable::lookup`] takes the rights the
//!   operation needs and the identity of the caller; wrong owner, stale id, revoked object or
//!   missing right each fail with a distinct error.
//! * **Stale ids never alias.** Slots carry a generation; freeing bumps it; a slot whose
//!   generation would wrap is retired.
//! * **Revocation is immediate and total.** [`CapTable::revoke_descendants`] removes a
//!   capability's whole derivation subtree; [`CapTable::sweep_object`] removes every capability
//!   to a revoked object. Both release the object references the capabilities held.
//!
//! # Derivation tree
//! Each capability records its parent (generation-checked). Closing a single capability
//! ([`CapTable::release`]) re-parents its children to its parent, so an ancestor's revocation
//! still reaches them. Depth is bounded by [`MAX_DEPTH`].
//!
//! # Accounting
//! Each live capability holds exactly one `refs` unit on its object (checked against the object
//! table by `check_invariants`). Capabilities are charged to an owner (`charged`) whose count is
//! bounded by the table's quota; a capability in flight inside a message is held by
//! `Owner::INFLIGHT` but stays charged to the sender until delivery.
#![allow(dead_code)]

use crate::d6::object::{Counter, Error, ObjectId, ObjectKind, ObjectTable, Owner};

/// Maximum derivation depth (root = 0).
pub const MAX_DEPTH: u8 = 8;
pub const MAX_CAPS: usize = 256;

/// Bit set of operations a capability allows. Only the bits defined here are valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights(u32);

impl Rights {
    pub const NONE: Rights = Rights(0);
    /// Send messages through an endpoint.
    pub const SEND: Rights = Rights(1 << 0);
    /// Receive messages from an endpoint.
    pub const RECV: Rights = Rights(1 << 1);
    /// Derive / duplicate this capability (with equal or fewer rights).
    pub const DERIVE: Rights = Rights(1 << 2);
    /// Send this capability to another process inside a message.
    pub const TRANSFER: Rights = Rights(1 << 3);
    /// Connect to a listener.
    pub const CONNECT: Rights = Rights(1 << 4);
    /// Accept connections on a listener.
    pub const ACCEPT: Rights = Rights(1 << 5);
    /// Cancel waiters / pending operations on an endpoint.
    pub const CANCEL: Rights = Rights(1 << 6);
    /// Revoke capabilities derived from this one.
    pub const REVOKE: Rights = Rights(1 << 7);
    /// Register services in a directory.
    pub const REGISTER: Rights = Rights(1 << 8);
    /// Look services up in a directory.
    pub const LOOKUP: Rights = Rights(1 << 9);
    /// Destroy the underlying object (close an endpoint for everybody).
    pub const DESTROY: Rights = Rights(1 << 10);
    pub const ALL: Rights = Rights((1 << 11) - 1);

    /// Validates raw bits from an untrusted source.
    pub const fn from_bits(bits: u32) -> Result<Rights, Error> {
        if bits & !Rights::ALL.0 != 0 {
            Err(Error::UnknownRights)
        } else {
            Ok(Rights(bits))
        }
    }

    pub const fn bits(self) -> u32 {
        self.0
    }
    pub const fn contains(self, other: Rights) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn is_subset_of(self, other: Rights) -> bool {
        self.0 & !other.0 == 0
    }
    pub const fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }
    pub const fn intersect(self, other: Rights) -> Rights {
        Rights(self.0 & other.0)
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Generation-checked capability identity. Kernel-internal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapId {
    index: u16,
    generation: u32,
}

impl CapId {
    pub const fn index(self) -> usize {
        self.index as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub object: ObjectId,
    pub kind: ObjectKind,
    pub rights: Rights,
    /// Current holder (a process, the kernel, or `Owner::INFLIGHT`).
    pub owner: Owner,
    /// Whose quota this capability counts against.
    pub charged: Owner,
    pub parent: Option<CapId>,
    pub depth: u8,
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u32,
    retired: bool,
    cap: Option<Capability>,
}

pub struct CapTable<const C: usize> {
    slots: [Slot; C],
    quota: usize,
    live: usize,
}

impl<const C: usize> Default for CapTable<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const C: usize> CapTable<C> {
    pub const fn new() -> Self {
        Self::with_quota(C)
    }

    /// `quota`: maximum capabilities charged to one *process* owner.
    pub const fn with_quota(quota: usize) -> Self {
        Self {
            slots: [Slot {
                generation: 0,
                retired: false,
                cap: None,
            }; C],
            quota,
            live: 0,
        }
    }

    pub const fn capacity(&self) -> usize {
        C
    }
    pub const fn live(&self) -> usize {
        self.live
    }

    pub fn charged_to(&self, owner: Owner) -> usize {
        self.slots
            .iter()
            .filter(|s| s.cap.is_some_and(|c| c.charged == owner))
            .count()
    }

    pub fn held_by(&self, owner: Owner) -> usize {
        self.slots
            .iter()
            .filter(|s| s.cap.is_some_and(|c| c.owner == owner))
            .count()
    }

    fn check_quota(&self, charged: Owner) -> Result<(), Error> {
        if charged.is_process() && self.charged_to(charged) >= self.quota {
            return Err(Error::QuotaExceeded);
        }
        Ok(())
    }

    fn cap(&self, id: CapId) -> Result<&Capability, Error> {
        let s = self.slots.get(id.index()).ok_or(Error::Stale)?;
        match &s.cap {
            Some(c) if s.generation == id.generation => Ok(c),
            _ => Err(Error::Stale),
        }
    }

    fn id_at(&self, i: usize) -> CapId {
        CapId {
            index: i as u16,
            generation: self.slots[i].generation,
        }
    }

    /// Reads a capability record without any authority check (kernel use).
    pub fn peek(&self, id: CapId) -> Result<Capability, Error> {
        self.cap(id).copied()
    }

    pub fn exists(&self, id: CapId) -> bool {
        self.cap(id).is_ok()
    }

    fn install<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        cap: Capability,
    ) -> Result<CapId, Error> {
        let i = self
            .slots
            .iter()
            .position(|s| s.cap.is_none() && !s.retired)
            .ok_or(Error::CapsExhausted)?;
        objs.acquire(cap.object, Counter::Refs)?;
        self.slots[i].cap = Some(cap);
        self.live += 1;
        Ok(self.id_at(i))
    }

    /// Creates a root capability (kernel operation: this is how authority enters the system).
    pub fn mint<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        owner: Owner,
        object: ObjectId,
        rights: Rights,
    ) -> Result<CapId, Error> {
        let kind = objs.get(object)?.kind;
        self.check_quota(owner)?;
        self.install(
            objs,
            Capability {
                object,
                kind,
                rights,
                owner,
                charged: owner,
                parent: None,
                depth: 0,
            },
        )
    }

    /// Authorised derivation: `caller` must own `parent` and `parent` must hold `DERIVE`.
    pub fn derive<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        parent: CapId,
        caller: Owner,
        rights: Rights,
        new_owner: Owner,
    ) -> Result<CapId, Error> {
        self.lookup(objs, parent, caller, Rights::DERIVE)?;
        self.derive_kernel(objs, parent, rights, new_owner, new_owner)
    }

    /// Derivation on the kernel's authority (caller checks done elsewhere). Rights may still
    /// only shrink. `holder` holds the new capability, `charged` pays for it.
    pub fn derive_kernel<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        parent: CapId,
        rights: Rights,
        holder: Owner,
        charged: Owner,
    ) -> Result<CapId, Error> {
        let p = *self.cap(parent)?;
        objs.get(p.object)?;
        if !rights.is_subset_of(p.rights) {
            return Err(Error::RightsEscalation);
        }
        if p.depth >= MAX_DEPTH {
            return Err(Error::DepthExceeded);
        }
        self.check_quota(charged)?;
        self.install(
            objs,
            Capability {
                object: p.object,
                kind: p.kind,
                rights,
                owner: holder,
                charged,
                parent: Some(parent),
                depth: p.depth + 1,
            },
        )
    }

    /// The authority check every privileged operation goes through.
    pub fn lookup<const O: usize>(
        &self,
        objs: &ObjectTable<O>,
        id: CapId,
        caller: Owner,
        required: Rights,
    ) -> Result<&Capability, Error> {
        let c = self.cap(id)?;
        if c.owner != caller {
            return Err(Error::NotOwner);
        }
        objs.get(c.object)?;
        if !c.rights.contains(required) {
            return Err(Error::RightsDenied);
        }
        Ok(c)
    }

    /// Lookup of a capability in flight inside a message (kernel use).
    pub fn lookup_inflight<const O: usize>(
        &self,
        objs: &ObjectTable<O>,
        id: CapId,
    ) -> Result<&Capability, Error> {
        let c = self.cap(id)?;
        if c.owner != Owner::INFLIGHT {
            return Err(Error::NotOwner);
        }
        objs.get(c.object)?;
        Ok(c)
    }

    /// Gives an in-flight capability to its final holder, charging the new holder's quota.
    pub fn reown<const O: usize>(
        &mut self,
        objs: &ObjectTable<O>,
        id: CapId,
        new_owner: Owner,
    ) -> Result<(), Error> {
        self.lookup_inflight(objs, id)?;
        self.check_quota(new_owner)?;
        let c = self.slots[id.index()].cap.as_mut().ok_or(Error::Stale)?;
        c.owner = new_owner;
        c.charged = new_owner;
        Ok(())
    }

    /// Undo of [`reown`](Self::reown): returns a delivered capability to the in-flight state and
    /// restores the previous quota charge (rollback of a failed delivery).
    pub fn to_inflight(&mut self, id: CapId, charged: Owner) -> Result<(), Error> {
        self.cap(id)?;
        let c = self.slots[id.index()].cap.as_mut().ok_or(Error::Stale)?;
        c.owner = Owner::INFLIGHT;
        c.charged = charged;
        Ok(())
    }

    /// Removes one capability (a handle close). Children are re-parented to its parent.
    pub fn release<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        id: CapId,
        caller: Owner,
    ) -> Result<(), Error> {
        if self.cap(id)?.owner != caller {
            return Err(Error::NotOwner);
        }
        self.remove(objs, id)
    }

    /// Removes a capability on the kernel's authority.
    pub fn remove<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        id: CapId,
    ) -> Result<(), Error> {
        let c = *self.cap(id)?;
        for i in 0..C {
            if let Some(ch) = self.slots[i].cap.as_mut() {
                if ch.parent == Some(id) {
                    ch.parent = c.parent;
                }
            }
        }
        let s = &mut self.slots[id.index()];
        s.cap = None;
        match s.generation.checked_add(1) {
            Some(g) => s.generation = g,
            None => s.retired = true,
        }
        self.live -= 1;
        objs.release(c.object, Counter::Refs)
            .map_err(|_| Error::Corrupt("capability held no object reference"))?;
        Ok(())
    }

    /// Marks `root`'s descendants (and `root` if `include_root`) and removes them.
    fn revoke_tree<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        root: CapId,
        include_root: bool,
    ) -> Result<usize, Error> {
        self.cap(root)?;
        let mut mark = [false; C];
        mark[root.index()] = true;
        loop {
            let mut changed = false;
            for i in 0..C {
                if mark[i] {
                    continue;
                }
                let Some(c) = self.slots[i].cap else { continue };
                let Some(p) = c.parent else { continue };
                // A parent id whose generation no longer matches is not a live parent.
                if self.cap(p).is_ok() && mark[p.index()] {
                    mark[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        if !include_root {
            mark[root.index()] = false;
        }
        let mut n = 0;
        for (i, m) in mark.iter().enumerate() {
            if *m {
                let id = self.id_at(i);
                if self.cap(id).is_ok() {
                    self.remove(objs, id)?;
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    /// Authorised revocation: removes every capability derived (transitively) from `id`.
    /// `caller` must own `id` and `id` must hold `REVOKE`. `id` itself stays valid.
    pub fn revoke_descendants<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        id: CapId,
        caller: Owner,
    ) -> Result<usize, Error> {
        self.lookup(objs, id, caller, Rights::REVOKE)?;
        self.revoke_tree(objs, id, false)
    }

    /// Kernel revocation of `id` and everything derived from it.
    pub fn revoke_subtree<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        id: CapId,
    ) -> Result<usize, Error> {
        self.revoke_tree(objs, id, true)
    }

    /// Removes every capability naming `object` (used when the object is revoked).
    pub fn sweep_object<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        object: ObjectId,
    ) -> Result<usize, Error> {
        let mut n = 0;
        for i in 0..C {
            if self.slots[i].cap.is_some_and(|c| c.object == object) {
                let id = self.id_at(i);
                self.remove(objs, id)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Removes every capability *held* by `owner` (process exit). In-flight capabilities the
    /// owner sent stay valid until delivered or discarded.
    pub fn sweep_owner<const O: usize>(
        &mut self,
        objs: &mut ObjectTable<O>,
        owner: Owner,
    ) -> Result<usize, Error> {
        let mut n = 0;
        for i in 0..C {
            if self.slots[i].cap.is_some_and(|c| c.owner == owner) {
                let id = self.id_at(i);
                self.remove(objs, id)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Cross-table consistency check for quiescent points.
    pub fn check_invariants<const O: usize>(&self, objs: &ObjectTable<O>) -> Result<(), &'static str> {
        let mut live = 0usize;
        let mut per_object = [0u32; O];
        for (i, s) in self.slots.iter().enumerate() {
            let Some(c) = &s.cap else { continue };
            live += 1;
            if s.retired {
                return Err("retired slot holds a capability");
            }
            let o = objs
                .get_any(c.object)
                .map_err(|_| "capability names a dead object")?;
            if o.kind != c.kind {
                return Err("capability kind differs from its object");
            }
            per_object[c.object.index()] += 1;
            if c.depth > MAX_DEPTH {
                return Err("derivation depth exceeded");
            }
            if let Some(p) = c.parent {
                let pc = self.cap(p).map_err(|_| "capability has a dead parent")?;
                if !c.rights.is_subset_of(pc.rights) {
                    return Err("child holds rights its parent lacks");
                }
                if c.depth <= pc.depth {
                    return Err("child is not deeper than its parent");
                }
                if p.index() == i {
                    return Err("capability is its own parent");
                }
            } else if c.depth != 0 && c.depth > MAX_DEPTH {
                return Err("bad root depth");
            }
            if !c.rights.is_subset_of(Rights::ALL) {
                return Err("unknown rights bits");
            }
        }
        if live != self.live {
            return Err("live count differs from the slots");
        }
        for (id, rec) in objs.iter() {
            if rec.refs != per_object[id.index()] {
                return Err("object reference count differs from its capabilities");
            }
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

    type O = ObjectTable<16>;
    type T = CapTable<16>;

    fn p(n: u64) -> Owner {
        Owner::process(n).unwrap()
    }

    fn setup() -> (O, T, ObjectId) {
        let mut o = O::new();
        let obj = o.create(ObjectKind::Endpoint, p(1), 0).unwrap();
        (o, T::new(), obj)
    }

    #[test]
    fn unknown_rights_bits_are_rejected() {
        assert_eq!(Rights::from_bits(1 << 11), Err(Error::UnknownRights));
        assert_eq!(Rights::from_bits(u32::MAX), Err(Error::UnknownRights));
        assert_eq!(Rights::from_bits(0x8000_0000), Err(Error::UnknownRights));
        assert_eq!(Rights::from_bits(0), Ok(Rights::NONE));
        assert_eq!(Rights::from_bits(Rights::ALL.bits()), Ok(Rights::ALL));
        for b in 0..11 {
            assert!(Rights::from_bits(1 << b).is_ok());
        }
    }

    #[test]
    fn rights_algebra() {
        let rw = Rights::SEND.union(Rights::RECV);
        assert!(rw.contains(Rights::SEND));
        assert!(!Rights::SEND.contains(rw));
        assert!(Rights::SEND.is_subset_of(rw));
        assert!(!rw.is_subset_of(Rights::SEND));
        assert_eq!(rw.intersect(Rights::RECV), Rights::RECV);
        assert!(Rights::NONE.is_subset_of(Rights::NONE));
        assert!(Rights::NONE.is_empty());
        assert!(Rights::ALL.contains(rw));
    }

    #[test]
    fn mint_lookup_and_release() {
        let (mut o, mut t, obj) = setup();
        let c = t.mint(&mut o, p(1), obj, Rights::SEND.union(Rights::RECV)).unwrap();
        assert_eq!(o.get(obj).unwrap().refs, 1);
        let cap = t.lookup(&o, c, p(1), Rights::SEND).unwrap();
        assert_eq!(cap.object, obj);
        assert_eq!(cap.kind, ObjectKind::Endpoint);
        t.check_invariants(&o).unwrap();
        t.release(&mut o, c, p(1)).unwrap();
        assert_eq!(o.get(obj).unwrap().refs, 0);
        assert_eq!(t.lookup(&o, c, p(1), Rights::NONE).err(), Some(Error::Stale));
        assert_eq!(t.release(&mut o, c, p(1)), Err(Error::Stale), "double release");
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn lookup_enforces_rights_and_owner() {
        let (mut o, mut t, obj) = setup();
        let c = t.mint(&mut o, p(1), obj, Rights::SEND).unwrap();
        assert!(t.lookup(&o, c, p(1), Rights::SEND).is_ok());
        assert!(t.lookup(&o, c, p(1), Rights::NONE).is_ok());
        assert_eq!(t.lookup(&o, c, p(1), Rights::RECV).err(), Some(Error::RightsDenied));
        assert_eq!(
            t.lookup(&o, c, p(1), Rights::SEND.union(Rights::RECV)).err(),
            Some(Error::RightsDenied),
            "every required bit must be present"
        );
        assert_eq!(t.lookup(&o, c, p(2), Rights::SEND).err(), Some(Error::NotOwner));
        assert_eq!(t.release(&mut o, c, p(2)), Err(Error::NotOwner));
        assert!(t.exists(c), "a refused release changes nothing");
    }

    #[test]
    fn derivation_attenuates_and_never_escalates() {
        let (mut o, mut t, obj) = setup();
        let all = Rights::SEND.union(Rights::RECV).union(Rights::DERIVE);
        let c = t.mint(&mut o, p(1), obj, all).unwrap();
        let d = t.derive(&mut o, c, p(1), Rights::SEND, p(2)).unwrap();
        let dc = t.lookup(&o, d, p(2), Rights::SEND).unwrap();
        assert_eq!(dc.rights, Rights::SEND);
        assert_eq!(dc.owner, p(2));
        assert_eq!(dc.parent, Some(c));
        assert_eq!(dc.depth, 1);
        // The child cannot derive (no DERIVE) and cannot receive.
        assert_eq!(t.derive(&mut o, d, p(2), Rights::SEND, p(3)), Err(Error::RightsDenied));
        assert_eq!(t.lookup(&o, d, p(2), Rights::RECV).err(), Some(Error::RightsDenied));
        // Escalation attempts.
        assert_eq!(
            t.derive(&mut o, c, p(1), all.union(Rights::TRANSFER), p(2)),
            Err(Error::RightsEscalation)
        );
        let d2 = t.derive(&mut o, c, p(1), Rights::SEND.union(Rights::DERIVE), p(2)).unwrap();
        assert_eq!(
            t.derive(&mut o, d2, p(2), Rights::SEND.union(Rights::RECV), p(3)),
            Err(Error::RightsEscalation),
            "a grandchild cannot regain what the middle capability lacks"
        );
        // Derivation by a non-owner is refused.
        assert_eq!(t.derive(&mut o, c, p(2), Rights::SEND, p(2)), Err(Error::NotOwner));
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn revoke_descendants_removes_the_whole_subtree_only() {
        let (mut o, mut t, obj) = setup();
        let all = Rights::SEND.union(Rights::DERIVE).union(Rights::REVOKE);
        let root = t.mint(&mut o, p(1), obj, all).unwrap();
        let sib = t.mint(&mut o, p(1), obj, all).unwrap();
        let a = t.derive(&mut o, root, p(1), all, p(2)).unwrap();
        let b = t.derive(&mut o, a, p(2), all, p(3)).unwrap();
        let c = t.derive(&mut o, b, p(3), Rights::SEND, p(4)).unwrap();
        let s = t.derive(&mut o, sib, p(1), Rights::SEND, p(5)).unwrap();
        assert_eq!(o.get(obj).unwrap().refs, 6);
        // Without REVOKE on the capability the caller cannot revoke.
        assert_eq!(t.revoke_descendants(&mut o, c, p(4)), Err(Error::RightsDenied));
        assert_eq!(t.revoke_descendants(&mut o, root, p(9)), Err(Error::NotOwner));
        assert_eq!(t.revoke_descendants(&mut o, root, p(1)), Ok(3));
        for dead in [a, b, c] {
            assert_eq!(t.lookup(&o, dead, p(2), Rights::NONE).err(), Some(Error::Stale));
            assert!(!t.exists(dead));
        }
        assert!(t.exists(root) && t.exists(sib) && t.exists(s), "unrelated authority survives");
        assert_eq!(o.get(obj).unwrap().refs, 3, "revocation released the references");
        assert_eq!(t.revoke_descendants(&mut o, root, p(1)), Ok(0));
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn closing_a_middle_capability_keeps_the_chain_revocable() {
        let (mut o, mut t, obj) = setup();
        let all = Rights::SEND.union(Rights::DERIVE).union(Rights::REVOKE);
        let root = t.mint(&mut o, p(1), obj, all).unwrap();
        let mid = t.derive(&mut o, root, p(1), all, p(2)).unwrap();
        let leaf = t.derive(&mut o, mid, p(2), Rights::SEND, p(3)).unwrap();
        t.release(&mut o, mid, p(2)).unwrap();
        assert_eq!(t.peek(leaf).unwrap().parent, Some(root), "re-parented");
        t.check_invariants(&o).unwrap();
        assert_eq!(t.revoke_descendants(&mut o, root, p(1)), Ok(1));
        assert!(!t.exists(leaf));
    }

    #[test]
    fn stale_ids_never_alias_new_capabilities() {
        let (mut o, mut t, obj) = setup();
        let a = t.mint(&mut o, p(1), obj, Rights::SEND).unwrap();
        t.release(&mut o, a, p(1)).unwrap();
        let b = t.mint(&mut o, p(1), obj, Rights::RECV).unwrap();
        assert_eq!(a.index(), b.index());
        assert_eq!(t.lookup(&o, a, p(1), Rights::NONE).err(), Some(Error::Stale));
        assert_eq!(t.release(&mut o, a, p(1)), Err(Error::Stale));
        assert_eq!(t.peek(b).unwrap().rights, Rights::RECV);
    }

    #[test]
    fn generation_wrap_retires_the_slot() {
        let mut o = ObjectTable::<4>::new();
        let mut t = CapTable::<1>::new();
        let obj = o.create(ObjectKind::Endpoint, p(1), 0).unwrap();
        t.set_generation_for_test(0, u32::MAX);
        let c = t.mint(&mut o, p(1), obj, Rights::SEND).unwrap();
        t.release(&mut o, c, p(1)).unwrap();
        assert_eq!(t.mint(&mut o, p(1), obj, Rights::SEND), Err(Error::CapsExhausted));
        assert!(!t.exists(c));
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn table_exhaustion_and_quota_are_errors_that_change_nothing() {
        let mut o = ObjectTable::<4>::new();
        let mut t = CapTable::<3>::with_quota(2);
        let obj = o.create(ObjectKind::Endpoint, p(1), 0).unwrap();
        t.mint(&mut o, p(1), obj, Rights::SEND).unwrap();
        t.mint(&mut o, p(1), obj, Rights::SEND).unwrap();
        assert_eq!(t.mint(&mut o, p(1), obj, Rights::SEND), Err(Error::QuotaExceeded));
        t.mint(&mut o, p(2), obj, Rights::SEND).unwrap();
        assert_eq!(t.mint(&mut o, p(3), obj, Rights::SEND), Err(Error::CapsExhausted));
        assert_eq!(o.get(obj).unwrap().refs, 3, "failed mints took no reference");
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn depth_is_bounded() {
        let (mut o, mut t, obj) = setup();
        let r = Rights::SEND.union(Rights::DERIVE);
        let mut cur = t.mint(&mut o, p(1), obj, r).unwrap();
        for _ in 0..MAX_DEPTH {
            cur = t.derive(&mut o, cur, p(1), r, p(1)).unwrap();
        }
        assert_eq!(t.derive(&mut o, cur, p(1), r, p(1)), Err(Error::DepthExceeded));
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn revoked_objects_make_capabilities_unusable_and_sweep_frees_them() {
        let (mut o, mut t, obj) = setup();
        let c = t.mint(&mut o, p(1), obj, Rights::SEND.union(Rights::DERIVE)).unwrap();
        let d = t.derive(&mut o, c, p(1), Rights::SEND, p(2)).unwrap();
        assert_eq!(o.revoke(obj), Ok(false), "capabilities still reference it");
        assert_eq!(t.lookup(&o, c, p(1), Rights::SEND).err(), Some(Error::Revoked));
        assert_eq!(t.lookup(&o, d, p(2), Rights::SEND).err(), Some(Error::Revoked));
        assert_eq!(t.derive(&mut o, c, p(1), Rights::SEND, p(3)), Err(Error::Revoked));
        assert_eq!(t.mint(&mut o, p(1), obj, Rights::SEND), Err(Error::Revoked));
        assert_eq!(t.sweep_object(&mut o, obj), Ok(2));
        assert!(!o.exists(obj), "the last reference freed the revoked object");
        assert_eq!(t.live(), 0);
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn sweep_owner_removes_only_that_owners_holdings() {
        let (mut o, mut t, obj) = setup();
        for _ in 0..3 {
            t.mint(&mut o, p(1), obj, Rights::SEND).unwrap();
        }
        let keep = t.mint(&mut o, p(2), obj, Rights::SEND).unwrap();
        assert_eq!(t.sweep_owner(&mut o, p(1)), Ok(3));
        assert!(t.exists(keep));
        assert_eq!(o.get(obj).unwrap().refs, 1);
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn inflight_capabilities_reown_with_quota() {
        let mut o = ObjectTable::<4>::new();
        let mut t = CapTable::<8>::with_quota(2);
        let obj = o.create(ObjectKind::Endpoint, p(1), 0).unwrap();
        let src = t.mint(&mut o, p(1), obj, Rights::SEND.union(Rights::DERIVE)).unwrap();
        let f = t.derive_kernel(&mut o, src, Rights::SEND, Owner::INFLIGHT, p(1)).unwrap();
        assert_eq!(t.charged_to(p(1)), 2, "the in-flight capability stays charged to the sender");
        assert_eq!(t.lookup(&o, f, p(2), Rights::SEND).err(), Some(Error::NotOwner));
        assert!(t.lookup_inflight(&o, f).is_ok());
        assert!(t.lookup_inflight(&o, src).is_err(), "only in-flight capabilities qualify");
        // Receiver already at quota: reown refuses and leaves the capability in flight.
        t.mint(&mut o, p(2), obj, Rights::SEND).unwrap();
        t.mint(&mut o, p(2), obj, Rights::SEND).unwrap();
        assert_eq!(t.reown(&o, f, p(2)), Err(Error::QuotaExceeded));
        assert!(t.lookup_inflight(&o, f).is_ok());
        t.sweep_owner(&mut o, p(2)).unwrap();
        t.reown(&o, f, p(2)).unwrap();
        assert_eq!(t.lookup(&o, f, p(2), Rights::SEND).unwrap().charged, p(2));
        t.check_invariants(&o).unwrap();
    }

    #[test]
    fn kernel_derivation_still_cannot_escalate() {
        let (mut o, mut t, obj) = setup();
        let c = t.mint(&mut o, p(1), obj, Rights::SEND).unwrap();
        assert_eq!(
            t.derive_kernel(&mut o, c, Rights::SEND.union(Rights::RECV), Owner::KERNEL, Owner::KERNEL),
            Err(Error::RightsEscalation)
        );
    }

    #[test]
    fn invariant_checker_detects_reference_drift() {
        let (mut o, mut t, obj) = setup();
        t.mint(&mut o, p(1), obj, Rights::SEND).unwrap();
        t.check_invariants(&o).unwrap();
        // Corrupt the object's counter behind the table's back.
        o.acquire(obj, Counter::Refs).unwrap();
        assert!(t.check_invariants(&o).is_err());
    }
}
