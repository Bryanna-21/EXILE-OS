//! Process-local handle tables (Mission D6, file 3). SPDX-License-Identifier: Apache-2.0
//!
//! A [`Handle`] is the only thing user code ever holds. It is a 32-bit number that is *not* a
//! pointer and not an index into any shared table: it is resolved against the calling process's
//! own [`HandleTable`] and means nothing anywhere else.
//!
//! # Encoding
//! `raw = generation << 16 | (slot + 1)`. Slot numbers start at 1 and generations at 1, so the raw
//! value `0` is never a valid handle. Closing a handle bumps its slot's generation; a slot whose
//! 16-bit generation is exhausted is retired rather than wrapped, so an old handle value can
//! never name a newer capability ([`Error::Stale`]).
//!
//! # Isolation
//! Each process owns a separate [`HandleTable`] (looked up by [`Owner`] in a [`HandleRegistry`]).
//! Guessing numeric values never reaches another process's capabilities: the value is resolved in
//! the caller's table only, and the capability table independently re-checks that the capability's
//! owner is the caller ([`Error::NotOwner`]).
//!
//! # Reference management
//! A handle holds no reference of its own; it names a capability, and each capability holds one
//! object reference (`capability.rs`). Closing a handle releases its capability, which releases the
//! object reference. A handle whose capability was removed behind it (revocation, endpoint
//! closure) is *dangling*: its next use fails with `Stale`/`Revoked` and the slot is reclaimed
//! lazily, so revoked authority is never usable but its slot may linger until touched or until
//! the process exits ([`HandleTable::close_all`]).
#![allow(dead_code)]

use crate::d6::capability::{CapId, CapTable, Capability, Rights};
use crate::d6::object::{Error, ObjectId, ObjectTable, Owner};

pub const MAX_HANDLES: usize = 32;
pub const MAX_PROCESSES: usize = 16;
const GEN_FIRST: u16 = 1;

/// A user-visible handle value. Untrusted: any `u32` can be wrapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(u32);

impl Handle {
    pub const INVALID: Handle = Handle(0);

    pub const fn from_raw(raw: u32) -> Handle {
        Handle(raw)
    }
    pub const fn raw(self) -> u32 {
        self.0
    }

    const fn new(slot: usize, generation: u16) -> Handle {
        Handle(((generation as u32) << 16) | (slot as u32 + 1))
    }
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u16,
    retired: bool,
    cap: Option<CapId>,
}

#[derive(Clone, Copy)]
pub struct HandleTable<const H: usize> {
    owner: Owner,
    slots: [Slot; H],
    live: usize,
}

impl<const H: usize> HandleTable<H> {
    pub const fn new(owner: Owner) -> Self {
        assert!(H > 0 && H < 0xFFFF);
        Self {
            owner,
            slots: [Slot {
                generation: GEN_FIRST,
                retired: false,
                cap: None,
            }; H],
            live: 0,
        }
    }

    pub const fn owner(&self) -> Owner {
        self.owner
    }
    pub const fn len(&self) -> usize {
        self.live
    }
    pub const fn is_empty(&self) -> bool {
        self.live == 0
    }
    pub const fn capacity(&self) -> usize {
        H
    }

    /// Slots that can still take a new handle.
    pub fn free_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.cap.is_none() && !s.retired)
            .count()
    }

    fn slot_of(&self, h: Handle) -> Result<usize, Error> {
        let low = (h.0 & 0xFFFF) as usize;
        let generation = (h.0 >> 16) as u16;
        if low == 0 || generation == 0 || low > H {
            return Err(Error::BadHandle);
        }
        let i = low - 1;
        let s = &self.slots[i];
        if s.cap.is_none() || s.generation != generation {
            return Err(Error::Stale);
        }
        Ok(i)
    }

    fn free_slot(&mut self, i: usize) {
        let s = &mut self.slots[i];
        s.cap = None;
        match s.generation.checked_add(1) {
            Some(g) => s.generation = g,
            None => s.retired = true,
        }
        self.live -= 1;
    }

    /// Stores `cap` (which must already be owned by this table's owner) under a new handle.
    pub fn insert(&mut self, cap: CapId) -> Result<Handle, Error> {
        let i = self
            .slots
            .iter()
            .position(|s| s.cap.is_none() && !s.retired)
            .ok_or(Error::HandlesExhausted)?;
        let s = &mut self.slots[i];
        s.cap = Some(cap);
        self.live += 1;
        Ok(Handle::new(i, s.generation))
    }

    /// The capability id behind a handle, with no authority check (kernel use).
    pub fn resolve(&self, h: Handle) -> Result<CapId, Error> {
        let i = self.slot_of(h)?;
        self.slots[i].cap.ok_or(Error::Stale)
    }

    /// Resolves `h` and checks that its capability is live, owned by this table's owner, names
    /// an active object and holds `required`. A handle whose capability is gone or whose object
    /// was revoked is reclaimed on the spot.
    pub fn lookup<const C: usize, const O: usize>(
        &mut self,
        caps: &CapTable<C>,
        objs: &ObjectTable<O>,
        h: Handle,
        required: Rights,
    ) -> Result<(CapId, Capability), Error> {
        let i = self.slot_of(h)?;
        let id = self.slots[i].cap.ok_or(Error::Stale)?;
        match caps.lookup(objs, id, self.owner, required) {
            Ok(c) => Ok((id, *c)),
            Err(e @ (Error::Stale | Error::Revoked)) => {
                self.free_slot(i);
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    /// Frees the slot of `h` without touching the capability (rollback of an installation).
    pub fn detach(&mut self, h: Handle) -> Result<CapId, Error> {
        let i = self.slot_of(h)?;
        let id = self.slots[i].cap.ok_or(Error::Stale)?;
        self.free_slot(i);
        Ok(id)
    }

    /// Closes a handle: releases its capability and frees the slot. A dangling handle (capability
    /// already removed) is simply freed.
    pub fn close<const C: usize, const O: usize>(
        &mut self,
        caps: &mut CapTable<C>,
        objs: &mut ObjectTable<O>,
        h: Handle,
    ) -> Result<(), Error> {
        let i = self.slot_of(h)?;
        let id = self.slots[i].cap.ok_or(Error::Stale)?;
        if caps.exists(id) {
            caps.release(objs, id, self.owner)?;
        }
        self.free_slot(i);
        Ok(())
    }

    /// Creates a second handle with `mask` rights (never more than the original has). Needs
    /// `DERIVE`. Either both the derived capability and the handle exist afterwards, or neither.
    pub fn duplicate<const C: usize, const O: usize>(
        &mut self,
        caps: &mut CapTable<C>,
        objs: &mut ObjectTable<O>,
        h: Handle,
        mask: Rights,
    ) -> Result<Handle, Error> {
        let i = self.slot_of(h)?;
        let id = self.slots[i].cap.ok_or(Error::Stale)?;
        if self.free_slots() == 0 {
            return Err(Error::HandlesExhausted);
        }
        let derived = caps.derive(objs, id, self.owner, mask, self.owner)?;
        match self.insert(derived) {
            Ok(nh) => Ok(nh),
            Err(e) => {
                caps.release(objs, derived, self.owner)?;
                Err(e)
            }
        }
    }

    /// Kernel operation: mints a root capability for this table's owner and stores it.
    pub fn grant<const C: usize, const O: usize>(
        &mut self,
        caps: &mut CapTable<C>,
        objs: &mut ObjectTable<O>,
        object: ObjectId,
        rights: Rights,
    ) -> Result<Handle, Error> {
        if self.free_slots() == 0 {
            return Err(Error::HandlesExhausted);
        }
        let cap = caps.mint(objs, self.owner, object, rights)?;
        match self.insert(cap) {
            Ok(h) => Ok(h),
            Err(e) => {
                caps.release(objs, cap, self.owner)?;
                Err(e)
            }
        }
    }

    /// Closes every handle (process exit). Returns how many were live.
    pub fn close_all<const C: usize, const O: usize>(
        &mut self,
        caps: &mut CapTable<C>,
        objs: &mut ObjectTable<O>,
    ) -> Result<usize, Error> {
        let mut n = 0;
        for i in 0..H {
            if let Some(id) = self.slots[i].cap {
                if caps.exists(id) && caps.peek(id)?.owner == self.owner {
                    caps.release(objs, id, self.owner)?;
                }
                self.free_slot(i);
                n += 1;
            }
        }
        Ok(n)
    }

    /// Capability ids currently stored (dangling ones included), for diagnostics.
    pub fn stored_caps(&self) -> impl Iterator<Item = CapId> + '_ {
        self.slots.iter().filter_map(|s| s.cap)
    }

    pub fn check_invariants<const C: usize, const O: usize>(
        &self,
        caps: &CapTable<C>,
        _objs: &ObjectTable<O>,
    ) -> Result<(), &'static str> {
        let mut live = 0;
        for s in &self.slots {
            let Some(id) = s.cap else { continue };
            live += 1;
            if s.retired {
                return Err("retired slot holds a handle");
            }
            if s.generation == 0 {
                return Err("zero generation");
            }
            // A dangling handle is legal; a live one must belong to this process.
            if let Ok(c) = caps.peek(id) {
                if c.owner != self.owner {
                    return Err("handle names another owner's capability");
                }
            }
        }
        if live != self.live {
            return Err("live count differs from the slots");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_generation_for_test(&mut self, slot: usize, generation: u16) {
        self.slots[slot].generation = generation;
    }
}

/// The per-process handle tables of the system.
pub struct HandleRegistry<const P: usize, const H: usize> {
    tables: [Option<HandleTable<H>>; P],
}

impl<const P: usize, const H: usize> Default for HandleRegistry<P, H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const P: usize, const H: usize> HandleRegistry<P, H> {
    pub const fn new() -> Self {
        Self { tables: [None; P] }
    }

    pub fn count(&self) -> usize {
        self.tables.iter().flatten().count()
    }

    pub fn contains(&self, owner: Owner) -> bool {
        self.position(owner).is_some()
    }

    fn position(&self, owner: Owner) -> Option<usize> {
        self.tables
            .iter()
            .position(|t| t.as_ref().is_some_and(|t| t.owner == owner))
    }

    /// Gives `owner` an empty handle table. Only real processes get one.
    pub fn create(&mut self, owner: Owner) -> Result<(), Error> {
        if !owner.is_process() {
            return Err(Error::ReservedOwner);
        }
        if self.contains(owner) {
            return Err(Error::ProcessExists);
        }
        let i = self
            .tables
            .iter()
            .position(|t| t.is_none())
            .ok_or(Error::ProcessesExhausted)?;
        self.tables[i] = Some(HandleTable::new(owner));
        Ok(())
    }

    pub fn get(&self, owner: Owner) -> Result<&HandleTable<H>, Error> {
        let i = self.position(owner).ok_or(Error::NoSuchProcess)?;
        self.tables[i].as_ref().ok_or(Error::NoSuchProcess)
    }

    pub fn get_mut(&mut self, owner: Owner) -> Result<&mut HandleTable<H>, Error> {
        let i = self.position(owner).ok_or(Error::NoSuchProcess)?;
        self.tables[i].as_mut().ok_or(Error::NoSuchProcess)
    }

    /// Two distinct tables at once (sender and receiver of a transfer).
    pub fn two_mut(
        &mut self,
        a: Owner,
        b: Owner,
    ) -> Result<(&mut HandleTable<H>, &mut HandleTable<H>), Error> {
        let ia = self.position(a).ok_or(Error::NoSuchProcess)?;
        let ib = self.position(b).ok_or(Error::NoSuchProcess)?;
        if ia == ib {
            return Err(Error::Busy);
        }
        let (lo, hi, swap) = if ia < ib { (ia, ib, false) } else { (ib, ia, true) };
        let (left, right) = self.tables.split_at_mut(hi);
        let x = left[lo].as_mut().ok_or(Error::NoSuchProcess)?;
        let y = right[0].as_mut().ok_or(Error::NoSuchProcess)?;
        Ok(if swap { (y, x) } else { (x, y) })
    }

    /// Process exit: closes every handle, removes every capability still held by the owner and
    /// drops the table.
    pub fn destroy<const C: usize, const O: usize>(
        &mut self,
        owner: Owner,
        caps: &mut CapTable<C>,
        objs: &mut ObjectTable<O>,
    ) -> Result<usize, Error> {
        let i = self.position(owner).ok_or(Error::NoSuchProcess)?;
        let n = self.tables[i]
            .as_mut()
            .ok_or(Error::NoSuchProcess)?
            .close_all(caps, objs)?;
        caps.sweep_owner(objs, owner)?;
        self.tables[i] = None;
        Ok(n)
    }

    pub fn check_invariants<const C: usize, const O: usize>(
        &self,
        caps: &CapTable<C>,
        objs: &ObjectTable<O>,
    ) -> Result<(), &'static str> {
        for (i, t) in self.tables.iter().enumerate() {
            let Some(t) = t else { continue };
            t.check_invariants(caps, objs)?;
            for (j, u) in self.tables.iter().enumerate() {
                if j != i && u.as_ref().is_some_and(|u| u.owner == t.owner) {
                    return Err("two tables for one owner");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d6::object::ObjectKind;

    type O = ObjectTable<16>;
    type C = CapTable<32>;
    type T = HandleTable<4>;

    fn p(n: u64) -> Owner {
        Owner::process(n).unwrap()
    }

    fn setup() -> (O, C, ObjectId) {
        let mut o = O::new();
        let obj = o.create(ObjectKind::Endpoint, p(1), 0).unwrap();
        (o, C::new(), obj)
    }

    #[test]
    fn handle_zero_and_garbage_are_invalid() {
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        let h = t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        assert_ne!(h.raw(), 0);
        for raw in [0u32, 0xFFFF_FFFF, 0x0001_0000, 0x0001_0005, 0xFFFF_0001, 0x0000_0001] {
            let r = t.lookup(&c, &o, Handle::from_raw(raw), Rights::NONE);
            assert!(
                matches!(r, Err(Error::BadHandle) | Err(Error::Stale)),
                "{raw:#x} -> {r:?}"
            );
        }
        assert_eq!(t.lookup(&c, &o, Handle::INVALID, Rights::NONE).err(), Some(Error::BadHandle));
        assert!(t.lookup(&c, &o, h, Rights::SEND).is_ok());
    }

    #[test]
    fn lookup_checks_rights() {
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        let h = t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        assert_eq!(t.lookup(&c, &o, h, Rights::RECV).err(), Some(Error::RightsDenied));
        let (id, cap) = t.lookup(&c, &o, h, Rights::SEND).unwrap();
        assert_eq!(cap.rights, Rights::SEND);
        assert_eq!(t.resolve(h), Ok(id));
    }

    #[test]
    fn duplication_attenuates_and_cannot_escalate() {
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        let rd = Rights::SEND.union(Rights::RECV).union(Rights::DERIVE);
        let h = t.grant(&mut c, &mut o, obj, rd).unwrap();
        let d = t.duplicate(&mut c, &mut o, h, Rights::RECV).unwrap();
        assert_eq!(t.lookup(&c, &o, d, Rights::RECV).unwrap().1.rights, Rights::RECV);
        assert_eq!(t.lookup(&c, &o, d, Rights::SEND).err(), Some(Error::RightsDenied));
        assert_eq!(
            t.duplicate(&mut c, &mut o, d, Rights::RECV).err(),
            Some(Error::RightsDenied),
            "the duplicate has no DERIVE"
        );
        assert_eq!(
            t.duplicate(&mut c, &mut o, h, rd.union(Rights::TRANSFER)).err(),
            Some(Error::RightsEscalation)
        );
        assert_eq!(o.get(obj).unwrap().refs, 2);
        c.check_invariants(&o).unwrap();
        t.check_invariants(&c, &o).unwrap();
    }

    #[test]
    fn duplication_at_capacity_leaves_no_orphan_capability() {
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        let h = t.grant(&mut c, &mut o, obj, Rights::DERIVE.union(Rights::SEND)).unwrap();
        for _ in 0..3 {
            t.duplicate(&mut c, &mut o, h, Rights::SEND).unwrap();
        }
        assert_eq!(t.free_slots(), 0);
        let before = (c.live(), o.get(obj).unwrap().refs);
        assert_eq!(t.duplicate(&mut c, &mut o, h, Rights::SEND).err(), Some(Error::HandlesExhausted));
        assert_eq!(t.grant(&mut c, &mut o, obj, Rights::SEND).err(), Some(Error::HandlesExhausted));
        assert_eq!((c.live(), o.get(obj).unwrap().refs), before);
        c.check_invariants(&o).unwrap();
    }

    #[test]
    fn close_releases_references_and_stales_the_handle() {
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        let h = t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        assert_eq!(o.get(obj).unwrap().refs, 1);
        t.close(&mut c, &mut o, h).unwrap();
        assert_eq!(o.get(obj).unwrap().refs, 0);
        assert_eq!(t.len(), 0);
        assert_eq!(t.close(&mut c, &mut o, h), Err(Error::Stale), "double close");
        assert_eq!(t.lookup(&c, &o, h, Rights::NONE).err(), Some(Error::Stale));
        // A new handle in the same slot must not be reachable through the old value.
        let h2 = t.grant(&mut c, &mut o, obj, Rights::RECV).unwrap();
        assert_ne!(h, h2);
        assert_eq!(t.lookup(&c, &o, h, Rights::NONE).err(), Some(Error::Stale));
        assert!(t.lookup(&c, &o, h2, Rights::RECV).is_ok());
    }

    #[test]
    fn table_exhaustion_and_reuse() {
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        let hs: [Handle; 4] =
            core::array::from_fn(|_| t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap());
        assert_eq!(t.grant(&mut c, &mut o, obj, Rights::SEND).err(), Some(Error::HandlesExhausted));
        t.close(&mut c, &mut o, hs[2]).unwrap();
        assert!(t.grant(&mut c, &mut o, obj, Rights::SEND).is_ok());
        t.check_invariants(&c, &o).unwrap();
    }

    #[test]
    fn generation_exhaustion_retires_the_slot_instead_of_wrapping() {
        let (mut o, mut c, obj) = setup();
        let mut t = HandleTable::<2>::new(p(1));
        t.set_generation_for_test(0, u16::MAX);
        let h = t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        assert_eq!(h.raw() >> 16, 0xFFFF);
        t.close(&mut c, &mut o, h).unwrap();
        let h2 = t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        assert_eq!(h2.raw() & 0xFFFF, 2, "slot 0 is retired, slot 1 is used");
        assert_eq!(t.free_slots(), 0);
        assert_eq!(t.lookup(&c, &o, h, Rights::NONE).err(), Some(Error::Stale));
        t.check_invariants(&c, &o).unwrap();
    }

    #[test]
    fn cross_process_isolation() {
        let (mut o, mut c, obj) = setup();
        let mut a = T::new(p(1));
        let mut b = T::new(p(2));
        let ha = a.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        // B has nothing: A's value does not resolve there.
        assert!(b.lookup(&c, &o, ha, Rights::NONE).is_err());
        // B gets its own, different capability in the same slot with the same generation: the
        // numeric value is the same but it names B's capability, not A's.
        let hb = b.grant(&mut c, &mut o, obj, Rights::RECV).unwrap();
        assert_eq!(ha, hb, "identical numeric handles in two processes");
        assert_eq!(b.lookup(&c, &o, ha, Rights::SEND).err(), Some(Error::RightsDenied));
        assert_ne!(a.resolve(ha).unwrap(), b.resolve(hb).unwrap());
        // Brute-forcing every nearby value in B never reaches A's capability.
        let a_cap = a.resolve(ha).unwrap();
        for raw in 0..0x2_0000u32 {
            if let Ok(id) = b.resolve(Handle::from_raw(raw)) {
                assert_ne!(id, a_cap);
            }
        }
        // Even a table that wrongly stored A's capability cannot use it: the owner check fails.
        let mut evil = T::new(p(3));
        let stolen = evil.insert(a.resolve(ha).unwrap()).unwrap();
        assert_eq!(evil.lookup(&c, &o, stolen, Rights::NONE).err(), Some(Error::NotOwner));
        assert!(evil.check_invariants(&c, &o).is_err());
    }

    #[test]
    fn revocation_reclaims_dangling_handles_lazily() {
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        let h = t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        o.revoke(obj).unwrap();
        assert_eq!(t.lookup(&c, &o, h, Rights::SEND).err(), Some(Error::Revoked));
        assert_eq!(t.len(), 0, "the revoked handle was reclaimed");
        assert_eq!(t.lookup(&c, &o, h, Rights::SEND).err(), Some(Error::Stale));
        // A capability removed behind the handle's back (sweep) is dangling, then stale.
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        let h = t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        c.sweep_object(&mut o, obj).unwrap();
        t.check_invariants(&c, &o).unwrap();
        assert_eq!(t.lookup(&c, &o, h, Rights::SEND).err(), Some(Error::Stale));
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn close_all_releases_everything() {
        let (mut o, mut c, obj) = setup();
        let mut t = T::new(p(1));
        for _ in 0..3 {
            t.grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        }
        assert_eq!(t.close_all(&mut c, &mut o), Ok(3));
        assert_eq!(o.get(obj).unwrap().refs, 0);
        assert_eq!(c.live(), 0);
        assert!(t.is_empty());
    }

    #[test]
    fn registry_lifecycle_and_pair_access() {
        let (mut o, mut c, obj) = setup();
        let mut r = HandleRegistry::<3, 4>::new();
        assert_eq!(r.create(Owner::KERNEL), Err(Error::ReservedOwner));
        r.create(p(1)).unwrap();
        assert_eq!(r.create(p(1)), Err(Error::ProcessExists));
        r.create(p(2)).unwrap();
        r.create(p(3)).unwrap();
        assert_eq!(r.create(p(4)), Err(Error::ProcessesExhausted));
        assert_eq!(r.get(p(9)).err(), Some(Error::NoSuchProcess));
        {
            let (a, b) = r.two_mut(p(2), p(1)).unwrap();
            assert_eq!(a.owner(), p(2));
            assert_eq!(b.owner(), p(1));
        }
        assert_eq!(r.two_mut(p(1), p(1)).err(), Some(Error::Busy));
        assert_eq!(r.two_mut(p(1), p(7)).err(), Some(Error::NoSuchProcess));
        r.get_mut(p(1)).unwrap().grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        r.get_mut(p(2)).unwrap().grant(&mut c, &mut o, obj, Rights::SEND).unwrap();
        r.check_invariants(&c, &o).unwrap();
        assert_eq!(r.destroy(p(1), &mut c, &mut o), Ok(1));
        assert!(!r.contains(p(1)));
        assert_eq!(o.get(obj).unwrap().refs, 1);
        assert_eq!(r.destroy(p(1), &mut c, &mut o), Err(Error::NoSuchProcess));
        r.create(p(4)).unwrap();
        c.check_invariants(&o).unwrap();
    }

    #[test]
    fn thousands_of_create_close_cycles_leak_nothing() {
        let (mut o, mut c, obj) = setup();
        let mut t = HandleTable::<8>::new(p(1));
        let base = t.grant(&mut c, &mut o, obj, Rights::DERIVE.union(Rights::SEND)).unwrap();
        for i in 0..5000 {
            let h = t.duplicate(&mut c, &mut o, base, Rights::SEND).unwrap();
            if i % 3 == 0 {
                let h2 = t.duplicate(&mut c, &mut o, base, Rights::NONE).unwrap();
                t.close(&mut c, &mut o, h2).unwrap();
            }
            t.close(&mut c, &mut o, h).unwrap();
            assert_eq!(c.live(), 1);
            assert_eq!(o.get(obj).unwrap().refs, 1);
        }
        c.check_invariants(&o).unwrap();
        t.check_invariants(&c, &o).unwrap();
    }
}
