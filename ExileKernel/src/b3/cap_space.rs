//! Capability table with derivation tree and revocation (Mission B3). SPDX-License-Identifier: Apache-2.0
//!
//! A capability is an unforgeable, *owner-bound* reference to a kernel object plus a set of
//! [`Rights`]. User space only ever sees a 64-bit handle: `(generation << 32) | (slot + 1)`.
//! Handle 0 is never valid. A handle is honoured only for the owner it was minted for, and a
//! lookup with a wrong owner, wrong generation, out-of-range slot or malformed value all fail
//! with the same [`CapError::BadHandle`], so a process cannot probe other processes' tables.
//!
//! # Derivation tree
//! Every capability except a *root* (minted by the kernel for a fresh object) has a parent:
//! the capability it was derived from. Rights only shrink along the tree (checked at derive
//! time, re-checked by [`CapTable::check_invariants`]).
//! * [`CapTable::revoke`] (needs `REVOKE`) deletes every descendant of a capability, in every
//!   owner's space, and keeps the capability itself.
//! * [`CapTable::close`] deletes one capability only; its children are re-parented to its
//!   parent (they keep working, but are now revoked by the grandparent instead).
//! * [`CapTable::remove_owner`] is process teardown: close everything an owner holds.
//!
//! # Transfer
//! Giving a capability to another owner is a derive with a different owner and needs `GRANT`.
//! An in-flight message holds its capability in the reserved [`LIMBO`] owner, which no handle
//! can ever be looked up for; [`CapTable::rebind`] moves it to the receiver atomically, so a
//! capability is never usable by the sender and the receiver at once through the same slot, and
//! never lost if the message is dropped (the owner of the message discards it).
//!
//! # Resources
//! Fixed `C` slots, a per-owner quota (`set_owner_limit`) so one process cannot exhaust the
//! table, generation-checked slots that are retired instead of wrapping. No allocation.
#![allow(dead_code)]

use super::cap_rights::{Kind, Rights};
use super::kobject::ObjId;

/// Reserved owner of capabilities carried by in-flight messages. No process may be this owner.
pub const LIMBO: u64 = u64::MAX;

/// Highest slot generation. Handles stay below 2^63 so a handle returned in RAX can never be
/// mistaken for a negative error code; a slot that would pass this generation is retired.
pub const MAX_GEN: u32 = 0x7FFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// Malformed, stale, out of range or owned by someone else.
    BadHandle,
    /// The capability exists but lacks a required right.
    Perm,
    /// The derived capability would have no rights.
    NoRights,
    /// No free slot in the table.
    NoSlots,
    /// The owner already holds its maximum number of capabilities.
    Quota,
    /// `LIMBO` used where a real owner is required, or similar misuse.
    BadOwner,
}

/// Internal reference to a capability slot (generation checked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapRef {
    index: u16,
    gen_: u32,
}

impl CapRef {
    pub const fn index(self) -> usize {
        self.index as usize
    }

    /// The user-visible handle value.
    pub const fn handle(self) -> u64 {
        ((self.gen_ as u64) << 32) | (self.index as u64 + 1)
    }

    /// Decodes a handle; `None` for 0 and for values whose low half is not a slot number.
    pub const fn from_handle(h: u64) -> Option<CapRef> {
        let low = (h & 0xFFFF_FFFF) as u32;
        if low == 0 || low > u16::MAX as u32 + 1 || (h >> 32) > MAX_GEN as u64 {
            return None;
        }
        Some(CapRef {
            index: (low - 1) as u16,
            gen_: (h >> 32) as u32,
        })
    }
}

#[derive(Clone, Copy)]
struct Cap {
    owner: u64,
    obj: ObjId,
    kind: Kind,
    rights: Rights,
    parent: Option<u16>,
}

#[derive(Clone, Copy)]
struct Slot {
    gen_: u32,
    retired: bool,
    cap: Option<Cap>,
}

/// What a successful [`CapTable::lookup`] tells the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub cap: CapRef,
    pub obj: ObjId,
    pub kind: Kind,
    pub rights: Rights,
}

pub struct CapTable<const C: usize> {
    slots: [Slot; C],
    owner_limit: usize,
}

impl<const C: usize> Default for CapTable<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const C: usize> CapTable<C> {
    pub fn new() -> Self {
        assert!(
            C > 0 && C <= u16::MAX as usize,
            "slot index must fit in u16"
        );
        Self {
            slots: [Slot {
                gen_: 0,
                retired: false,
                cap: None,
            }; C],
            owner_limit: C,
        }
    }

    pub const fn capacity(&self) -> usize {
        C
    }

    /// Caps the number of capabilities one real owner may hold. `0` is rejected.
    pub fn set_owner_limit(&mut self, limit: usize) -> Result<(), CapError> {
        if limit == 0 {
            return Err(CapError::Quota);
        }
        self.owner_limit = limit;
        Ok(())
    }

    pub fn owner_limit(&self) -> usize {
        self.owner_limit
    }

    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.cap.is_some()).count()
    }

    pub fn retired_count(&self) -> usize {
        self.slots.iter().filter(|s| s.retired).count()
    }

    pub fn owner_count(&self, owner: u64) -> usize {
        self.slots
            .iter()
            .filter(|s| s.cap.is_some_and(|c| c.owner == owner))
            .count()
    }

    /// Capabilities to `obj` held by real owners (limbo copies do not keep an object alive).
    pub fn user_refs(&self, obj: ObjId) -> usize {
        self.slots
            .iter()
            .filter(|s| s.cap.is_some_and(|c| c.obj == obj && c.owner != LIMBO))
            .count()
    }

    /// Whether `owner` could receive one more capability.
    pub fn can_hold_more(&self, owner: u64) -> bool {
        owner == LIMBO || self.owner_count(owner) < self.owner_limit
    }

    fn cap(&self, r: CapRef) -> Option<&Cap> {
        let s = self.slots.get(r.index())?;
        if s.gen_ != r.gen_ {
            return None;
        }
        s.cap.as_ref()
    }

    pub fn is_live(&self, r: CapRef) -> bool {
        self.cap(r).is_some()
    }

    /// Owner of a live capability.
    pub fn owner_of(&self, r: CapRef) -> Option<u64> {
        self.cap(r).map(|c| c.owner)
    }

    pub fn resolve(&self, r: CapRef) -> Option<Resolved> {
        self.cap(r).map(|c| Resolved {
            cap: r,
            obj: c.obj,
            kind: c.kind,
            rights: c.rights,
        })
    }

    fn alloc(&mut self, cap: Cap) -> Result<CapRef, CapError> {
        if cap.owner != LIMBO && self.owner_count(cap.owner) >= self.owner_limit {
            return Err(CapError::Quota);
        }
        let i = self
            .slots
            .iter()
            .position(|s| s.cap.is_none() && !s.retired)
            .ok_or(CapError::NoSlots)?;
        let s = &mut self.slots[i];
        s.cap = Some(cap);
        Ok(CapRef {
            index: i as u16,
            gen_: s.gen_,
        })
    }

    fn free(&mut self, i: usize) {
        let s = &mut self.slots[i];
        s.cap = None;
        match s.gen_.checked_add(1) {
            Some(g) if g <= MAX_GEN => s.gen_ = g,
            _ => s.retired = true,
        }
    }

    /// Mints the root capability of a new object for `owner`, with all rights of its kind.
    pub fn mint_root(&mut self, owner: u64, obj: ObjId, kind: Kind) -> Result<CapRef, CapError> {
        if owner == LIMBO {
            return Err(CapError::BadOwner);
        }
        self.alloc(Cap {
            owner,
            obj,
            kind,
            rights: Rights::root_for(kind),
            parent: None,
        })
    }

    /// Resolves an untrusted handle for `owner`, requiring every right in `need`.
    pub fn lookup(&self, owner: u64, handle: u64, need: Rights) -> Result<Resolved, CapError> {
        if owner == LIMBO {
            return Err(CapError::BadHandle);
        }
        let r = CapRef::from_handle(handle).ok_or(CapError::BadHandle)?;
        let c = self.cap(r).ok_or(CapError::BadHandle)?;
        if c.owner != owner {
            return Err(CapError::BadHandle);
        }
        if !c.rights.contains(need) {
            return Err(CapError::Perm);
        }
        Ok(Resolved {
            cap: r,
            obj: c.obj,
            kind: c.kind,
            rights: c.rights,
        })
    }

    /// Derives a child of `src` for `to` with `src.rights & mask`.
    /// Handing a capability to a different owner (including [`LIMBO`]) needs `GRANT` on `src`.
    pub fn derive(&mut self, src: CapRef, to: u64, mask: Rights) -> Result<CapRef, CapError> {
        let c = *self.cap(src).ok_or(CapError::BadHandle)?;
        if to != c.owner && !c.rights.contains(Rights::GRANT) {
            return Err(CapError::Perm);
        }
        let rights = c.rights.attenuate(mask);
        if rights.is_empty() {
            return Err(CapError::NoRights);
        }
        self.alloc(Cap {
            owner: to,
            obj: c.obj,
            kind: c.kind,
            rights,
            parent: Some(src.index),
        })
    }

    /// Moves a limbo capability to a real owner (the receiver of the message carrying it).
    pub fn rebind(&mut self, r: CapRef, to: u64) -> Result<(), CapError> {
        if to == LIMBO {
            return Err(CapError::BadOwner);
        }
        let held = self.owner_count(to);
        let limit = self.owner_limit;
        let s = self.slots.get_mut(r.index()).ok_or(CapError::BadHandle)?;
        if s.gen_ != r.gen_ {
            return Err(CapError::BadHandle);
        }
        let c = s.cap.as_mut().ok_or(CapError::BadHandle)?;
        if c.owner != LIMBO {
            return Err(CapError::BadOwner);
        }
        if held >= limit {
            return Err(CapError::Quota);
        }
        c.owner = to;
        Ok(())
    }

    /// Deletes every descendant of the capability (any owner); the capability stays.
    /// Returns how many were deleted.
    pub fn revoke(&mut self, owner: u64, handle: u64) -> Result<usize, CapError> {
        let res = self.lookup(owner, handle, Rights::REVOKE)?;
        let mut doomed = [false; C];
        doomed[res.cap.index()] = true;
        // Fixed point over the parent links: at most C passes, each marks at least one new slot.
        loop {
            let mut changed = false;
            for i in 0..C {
                if doomed[i] {
                    continue;
                }
                if let Some(c) = self.slots[i].cap
                    && let Some(p) = c.parent
                    && doomed[p as usize]
                {
                    doomed[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        doomed[res.cap.index()] = false;
        let mut n = 0;
        for (i, d) in doomed.iter().enumerate() {
            if *d {
                self.free(i);
                n += 1;
            }
        }
        Ok(n)
    }

    /// Deletes one slot, re-parenting its children to its parent.
    fn remove_slot(&mut self, i: usize) {
        let Some(c) = self.slots[i].cap else { return };
        for j in 0..C {
            if let Some(ch) = self.slots[j].cap.as_mut()
                && ch.parent == Some(i as u16)
            {
                ch.parent = c.parent;
            }
        }
        self.free(i);
    }

    /// Closes one capability held by `owner`.
    pub fn close(&mut self, owner: u64, handle: u64) -> Result<(), CapError> {
        let res = self.lookup(owner, handle, Rights::NONE)?;
        self.remove_slot(res.cap.index());
        Ok(())
    }

    /// Deletes one capability by internal reference (message teardown). `false` if it was
    /// already gone.
    pub fn discard(&mut self, r: CapRef) -> bool {
        if self.cap(r).is_none() {
            return false;
        }
        self.remove_slot(r.index());
        true
    }

    /// Deletes every capability naming `obj` (object destruction). Returns the count.
    pub fn remove_object(&mut self, obj: ObjId) -> usize {
        let mut n = 0;
        for i in 0..C {
            if self.slots[i].cap.is_some_and(|c| c.obj == obj) {
                self.remove_slot(i);
                n += 1;
            }
        }
        n
    }

    /// Subsystem shutdown: deletes every capability of every owner (including limbo).
    /// Returns the count.
    pub fn remove_all(&mut self) -> usize {
        let mut n = 0;
        for i in 0..C {
            if self.slots[i].cap.is_some() {
                self.remove_slot(i);
                n += 1;
            }
        }
        n
    }

    /// Process teardown: closes everything `owner` holds. Returns the count.
    pub fn remove_owner(&mut self, owner: u64) -> usize {
        let mut n = 0;
        for i in 0..C {
            if self.slots[i].cap.is_some_and(|c| c.owner == owner) {
                self.remove_slot(i);
                n += 1;
            }
        }
        n
    }

    /// Number of live capabilities currently in limbo.
    pub fn limbo_count(&self) -> usize {
        self.owner_count(LIMBO)
    }

    pub fn check_invariants(&self) -> Result<(), &'static str> {
        for (i, s) in self.slots.iter().enumerate() {
            let Some(c) = s.cap else { continue };
            if s.retired {
                return Err("retired slot holds a capability");
            }
            if c.rights.is_empty() {
                return Err("capability without rights");
            }
            if !c.rights.is_subset_of(Rights::valid_for(c.kind)) {
                return Err("capability carries rights foreign to its kind");
            }
            if let Some(p) = c.parent {
                let ps = self.slots.get(p as usize).ok_or("parent out of range")?;
                let pc = ps.cap.ok_or("parent slot is empty")?;
                if p as usize == i {
                    return Err("capability is its own parent");
                }
                if pc.obj != c.obj || pc.kind != c.kind {
                    return Err("child names a different object than its parent");
                }
                if !c.rights.is_subset_of(pc.rights) {
                    return Err("child has rights its parent lacks");
                }
            }
        }
        // Acyclic: following parents from any slot must terminate within C steps.
        for i in 0..C {
            let mut cur = self.slots[i].cap.and_then(|c| c.parent);
            let mut steps = 0;
            while let Some(p) = cur {
                steps += 1;
                if steps > C {
                    return Err("cycle in the derivation tree");
                }
                cur = self.slots[p as usize].cap.and_then(|c| c.parent);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::kobject::ObjTable;
    use super::*;

    const A: u64 = 100;
    const B: u64 = 200;

    struct World {
        objs: ObjTable<8>,
        caps: CapTable<16>,
    }

    impl World {
        fn new() -> Self {
            Self {
                objs: ObjTable::new(),
                caps: CapTable::new(),
            }
        }
        fn endpoint(&mut self, owner: u64) -> (ObjId, CapRef) {
            let o = self.objs.create(Kind::Endpoint).unwrap();
            (o, self.caps.mint_root(owner, o, Kind::Endpoint).unwrap())
        }
    }

    #[test]
    fn handle_encoding_round_trips_and_rejects_garbage() {
        for (index, gen_) in [(0u16, 0u32), (1, 7), (65_535, MAX_GEN), (300, 1)] {
            let r = CapRef { index, gen_ };
            assert_eq!(CapRef::from_handle(r.handle()), Some(r));
            assert_ne!(r.handle(), 0);
        }
        assert_eq!(CapRef::from_handle(0), None);
        assert_eq!(
            CapRef::from_handle(1u64 << 32),
            None,
            "slot 0 with a generation"
        );
        assert_eq!(
            CapRef::from_handle((1u64 << 32) | 1),
            Some(CapRef { index: 0, gen_: 1 })
        );
        assert_eq!(
            CapRef::from_handle(0x2_0001),
            None,
            "low half past the last slot"
        );
        assert_eq!(
            CapRef::from_handle(((MAX_GEN as u64 + 1) << 32) | 1),
            None,
            "generation beyond MAX_GEN"
        );
        assert!(
            CapRef {
                index: 9,
                gen_: MAX_GEN
            }
            .handle()
                <= i64::MAX as u64
        );
        // low half beyond the largest slot number
        assert_eq!(CapRef::from_handle(65_537), None);
        assert_eq!(CapRef::from_handle(u64::MAX >> 32 << 32), None);
    }

    #[test]
    fn root_has_all_kind_rights_and_lookup_enforces_owner_and_rights() {
        let mut w = World::new();
        let (o, c) = w.endpoint(A);
        let h = c.handle();
        let r = w.caps.lookup(A, h, Rights::SEND | Rights::RECV).unwrap();
        assert_eq!((r.obj, r.kind), (o, Kind::Endpoint));
        assert_eq!(r.rights, Rights::root_for(Kind::Endpoint));
        assert_eq!(
            w.caps.lookup(B, h, Rights::NONE),
            Err(CapError::BadHandle),
            "wrong owner"
        );
        assert_eq!(
            w.caps.lookup(LIMBO, h, Rights::NONE),
            Err(CapError::BadHandle)
        );
        assert_eq!(w.caps.lookup(A, 0, Rights::NONE), Err(CapError::BadHandle));
        assert_eq!(
            w.caps.lookup(A, h + (1 << 32), Rights::NONE),
            Err(CapError::BadHandle)
        );
        assert_eq!(
            w.caps.lookup(A, h + 1, Rights::NONE),
            Err(CapError::BadHandle),
            "empty slot"
        );
        assert_eq!(w.caps.lookup(A, h, Rights::SIGNAL), Err(CapError::Perm));
        assert_eq!(
            w.caps.mint_root(LIMBO, o, Kind::Endpoint),
            Err(CapError::BadOwner)
        );
    }

    #[test]
    fn derive_attenuates_and_cannot_amplify() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let send_only = w.caps.derive(root, A, Rights::SEND).unwrap();
        assert_eq!(w.caps.resolve(send_only).unwrap().rights, Rights::SEND);
        // deriving from the attenuated cap can never bring RECV back
        let again = w
            .caps
            .derive(send_only, A, Rights::SEND | Rights::RECV)
            .unwrap();
        assert_eq!(w.caps.resolve(again).unwrap().rights, Rights::SEND);
        assert_eq!(
            w.caps.derive(send_only, A, Rights::RECV),
            Err(CapError::NoRights)
        );
        assert_eq!(
            w.caps.derive(send_only, A, Rights::NONE),
            Err(CapError::NoRights)
        );
        w.caps.check_invariants().unwrap();
    }

    #[test]
    fn cross_owner_derive_needs_grant() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let no_grant = w.caps.derive(root, A, Rights::SEND | Rights::RECV).unwrap();
        assert_eq!(
            w.caps.derive(no_grant, B, Rights::SEND),
            Err(CapError::Perm)
        );
        assert_eq!(
            w.caps.derive(no_grant, LIMBO, Rights::SEND),
            Err(CapError::Perm)
        );
        let with_grant = w
            .caps
            .derive(root, A, Rights::SEND | Rights::GRANT)
            .unwrap();
        let for_b = w
            .caps
            .derive(with_grant, B, Rights::SEND | Rights::GRANT)
            .unwrap();
        assert_eq!(w.caps.owner_of(for_b), Some(B));
        // B may pass it on, but only what it has
        let for_a_again = w
            .caps
            .derive(for_b, A, Rights::SEND | Rights::RECV)
            .unwrap();
        assert_eq!(w.caps.resolve(for_a_again).unwrap().rights, Rights::SEND);
        assert!(w.caps.lookup(B, for_b.handle(), Rights::SEND).is_ok());
        assert_eq!(
            w.caps.lookup(A, for_b.handle(), Rights::SEND),
            Err(CapError::BadHandle)
        );
    }

    #[test]
    fn revoke_deletes_all_descendants_in_every_space_but_not_the_cap_or_siblings() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let mid = w
            .caps
            .derive(root, A, Rights::SEND | Rights::GRANT | Rights::REVOKE)
            .unwrap();
        let sibling = w.caps.derive(root, A, Rights::SEND).unwrap();
        let c1 = w.caps.derive(mid, B, Rights::SEND | Rights::GRANT).unwrap();
        let c2 = w.caps.derive(c1, B, Rights::SEND).unwrap();
        let c3 = w.caps.derive(c1, A, Rights::SEND).unwrap();
        assert_eq!(w.caps.revoke(A, mid.handle()), Ok(3));
        assert!(w.caps.is_live(root) && w.caps.is_live(mid) && w.caps.is_live(sibling));
        assert!(!w.caps.is_live(c1) && !w.caps.is_live(c2) && !w.caps.is_live(c3));
        assert_eq!(w.caps.revoke(A, mid.handle()), Ok(0), "idempotent");
        // stale handles stay stale even after the slots are reused
        let reuse = w.caps.derive(root, A, Rights::SEND).unwrap();
        assert!(w.caps.lookup(B, c1.handle(), Rights::NONE).is_err());
        assert_ne!(reuse.handle(), c1.handle());
        w.caps.check_invariants().unwrap();
    }

    #[test]
    fn revoke_needs_the_revoke_right_and_the_right_owner() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let weak = w.caps.derive(root, A, Rights::SEND).unwrap();
        let _child = w.caps.derive(weak, A, Rights::SEND).unwrap();
        assert_eq!(w.caps.revoke(A, weak.handle()), Err(CapError::Perm));
        assert_eq!(w.caps.revoke(B, root.handle()), Err(CapError::BadHandle));
        assert_eq!(
            w.caps.live_count(),
            3,
            "nothing deleted by the refused calls"
        );
    }

    #[test]
    fn close_reparents_children_so_they_survive_and_are_revoked_by_the_grandparent() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let mid = w
            .caps
            .derive(root, A, Rights::SEND | Rights::GRANT)
            .unwrap();
        let leaf = w.caps.derive(mid, B, Rights::SEND).unwrap();
        w.caps.close(A, mid.handle()).unwrap();
        assert!(!w.caps.is_live(mid));
        assert!(
            w.caps.is_live(leaf),
            "closing the middle does not kill the leaf"
        );
        w.caps.check_invariants().unwrap();
        assert_eq!(
            w.caps.revoke(A, root.handle()),
            Ok(1),
            "root now owns the leaf"
        );
        assert!(!w.caps.is_live(leaf));
        assert_eq!(
            w.caps.close(A, mid.handle()),
            Err(CapError::BadHandle),
            "double close"
        );
    }

    #[test]
    fn closing_a_root_orphans_children_into_new_roots() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let child = w
            .caps
            .derive(root, A, Rights::SEND | Rights::REVOKE)
            .unwrap();
        assert_eq!(
            w.caps.derive(child, B, Rights::SEND),
            Err(CapError::Perm),
            "no GRANT, cannot cross owners"
        );
        let gc = w.caps.derive(child, A, Rights::SEND).unwrap();
        w.caps.close(A, root.handle()).unwrap();
        w.caps.check_invariants().unwrap();
        assert_eq!(w.caps.revoke(A, child.handle()), Ok(1));
        assert!(!w.caps.is_live(gc));
    }

    #[test]
    fn limbo_transfer_rebinds_atomically_and_respects_quota() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let g = w
            .caps
            .derive(root, A, Rights::SEND | Rights::GRANT)
            .unwrap();
        let limbo = w.caps.derive(g, LIMBO, Rights::SEND).unwrap();
        assert_eq!(w.caps.limbo_count(), 1);
        assert_eq!(
            w.caps.lookup(A, limbo.handle(), Rights::NONE),
            Err(CapError::BadHandle)
        );
        assert_eq!(
            w.caps.lookup(B, limbo.handle(), Rights::NONE),
            Err(CapError::BadHandle)
        );
        assert_eq!(
            w.caps.user_refs(w.caps.resolve(limbo).unwrap().obj),
            2,
            "limbo not counted"
        );
        w.caps.rebind(limbo, B).unwrap();
        assert_eq!(w.caps.limbo_count(), 0);
        assert!(w.caps.lookup(B, limbo.handle(), Rights::SEND).is_ok());
        assert_eq!(
            w.caps.rebind(limbo, A),
            Err(CapError::BadOwner),
            "no longer in limbo"
        );
        assert_eq!(w.caps.rebind(limbo, LIMBO), Err(CapError::BadOwner));

        // quota refuses the rebind and leaves the capability in limbo
        w.caps.set_owner_limit(1).unwrap();
        let l2 = w.caps.derive(g, LIMBO, Rights::SEND).unwrap();
        assert_eq!(w.caps.rebind(l2, B), Err(CapError::Quota));
        assert_eq!(w.caps.owner_of(l2), Some(LIMBO));
        assert!(!w.caps.can_hold_more(B));
        assert!(w.caps.can_hold_more(LIMBO));
    }

    #[test]
    fn quota_and_table_exhaustion_are_distinct_errors() {
        let mut w = World::new();
        w.caps.set_owner_limit(3).unwrap();
        assert_eq!(w.caps.set_owner_limit(0), Err(CapError::Quota));
        let (o, root) = w.endpoint(A);
        w.caps.derive(root, A, Rights::SEND).unwrap();
        w.caps.derive(root, A, Rights::SEND).unwrap();
        assert_eq!(w.caps.derive(root, A, Rights::SEND), Err(CapError::Quota));
        assert_eq!(w.caps.mint_root(A, o, Kind::Endpoint), Err(CapError::Quota));
        // another owner is unaffected until the table itself is full (16 slots)
        for _ in 0..3 {
            w.caps.mint_root(B, o, Kind::Endpoint).unwrap();
        }
        w.caps.set_owner_limit(16).unwrap();
        while w.caps.live_count() < 16 {
            w.caps.mint_root(B, o, Kind::Endpoint).unwrap();
        }
        assert_eq!(
            w.caps.mint_root(B, o, Kind::Endpoint),
            Err(CapError::NoSlots)
        );
        assert_eq!(
            w.caps.derive(root, LIMBO, Rights::SEND),
            Err(CapError::NoSlots)
        );
    }

    #[test]
    fn slot_generation_wrap_retires_the_slot() {
        let mut w = World::new();
        let (o, _) = w.endpoint(A);
        let c = w.caps.mint_root(A, o, Kind::Endpoint).unwrap();
        w.caps.slots[c.index()].gen_ = MAX_GEN;
        let c = CapRef {
            index: c.index,
            gen_: MAX_GEN,
        };
        w.caps.close(A, c.handle()).unwrap();
        assert_eq!(w.caps.retired_count(), 1);
        assert!(!w.caps.is_live(c));
        // the retired slot is never handed out again
        for _ in 0..40 {
            if let Ok(n) = w.caps.mint_root(B, o, Kind::Endpoint) {
                assert_ne!(n.index(), c.index());
                w.caps.close(B, n.handle()).unwrap();
            }
        }
    }

    #[test]
    fn remove_owner_and_remove_object_clean_up() {
        let mut w = World::new();
        let (o1, r1) = w.endpoint(A);
        let (o2, r2) = w.endpoint(B);
        let g = w.caps.derive(r1, A, Rights::SEND | Rights::GRANT).unwrap();
        let on_b = w.caps.derive(g, B, Rights::SEND).unwrap();
        assert_eq!(w.caps.remove_owner(A), 2);
        assert_eq!(w.caps.owner_count(A), 0);
        assert!(w.caps.is_live(on_b), "B's derived copy outlives A");
        assert!(w.caps.is_live(r2));
        w.caps.check_invariants().unwrap();
        assert_eq!(w.caps.user_refs(o1), 1);
        assert_eq!(w.caps.remove_object(o1), 1);
        assert_eq!(w.caps.user_refs(o1), 0);
        assert_eq!(w.caps.user_refs(o2), 1);
        assert_eq!(w.caps.remove_owner(777), 0);
    }

    #[test]
    fn remove_all_empties_the_table_and_invalidates_every_handle() {
        let mut w = World::new();
        let (_, r1) = w.endpoint(A);
        let (_, r2) = w.endpoint(B);
        let g = w.caps.derive(r1, A, Rights::SEND | Rights::GRANT).unwrap();
        let l = w.caps.derive(g, LIMBO, Rights::SEND).unwrap();
        assert_eq!(w.caps.remove_all(), 4);
        assert_eq!(w.caps.live_count(), 0);
        for c in [r1, r2, g, l] {
            assert!(!w.caps.is_live(c));
        }
        assert_eq!(w.caps.remove_all(), 0);
        w.caps.check_invariants().unwrap();
    }

    #[test]
    fn discard_removes_by_reference_once() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let g = w
            .caps
            .derive(root, A, Rights::SEND | Rights::GRANT)
            .unwrap();
        let l = w.caps.derive(g, LIMBO, Rights::SEND).unwrap();
        assert!(w.caps.discard(l));
        assert!(!w.caps.discard(l));
        assert_eq!(w.caps.limbo_count(), 0);
    }

    #[test]
    fn invariant_checker_detects_a_corrupted_table() {
        let mut w = World::new();
        let (_, root) = w.endpoint(A);
        let mid = w.caps.derive(root, A, Rights::SEND).unwrap();
        let ch = w.caps.derive(mid, A, Rights::SEND).unwrap();
        w.caps.check_invariants().unwrap();
        // child with a right its parent lacks
        w.caps.slots[ch.index()].cap.as_mut().unwrap().rights = Rights::SEND | Rights::RECV;
        assert!(w.caps.check_invariants().is_err());
        w.caps.slots[ch.index()].cap.as_mut().unwrap().rights = Rights::SEND;
        // cycle
        w.caps.slots[mid.index()].cap.as_mut().unwrap().parent = Some(ch.index);
        assert!(w.caps.check_invariants().is_err());
        w.caps.slots[mid.index()].cap.as_mut().unwrap().parent = Some(root.index);
        // right foreign to the kind
        w.caps.slots[ch.index()].cap.as_mut().unwrap().rights = Rights::SIGNAL;
        assert!(w.caps.check_invariants().is_err());
        w.caps.slots[ch.index()].cap.as_mut().unwrap().rights = Rights::SEND;
        w.caps.check_invariants().unwrap();
    }
}
