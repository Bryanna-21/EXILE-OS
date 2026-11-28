//! Generation-checked kernel object table (Mission B3). SPDX-License-Identifier: Apache-2.0
//!
//! Every IPC object (endpoint, notification) occupies one slot of an [`ObjTable`]. An [`ObjId`]
//! is `(slot, generation)`: destroying an object bumps the slot's generation, so any id (and any
//! capability that still names it) becomes *stale* instead of silently aliasing whatever object
//! is created in the slot next. A slot whose generation counter would wrap is retired for good
//! rather than reused, so a stale id can never become valid again.
//!
//! The table stores only identity and kind. Per-object state (queues, signal words) lives in
//! parallel arrays owned by the IPC domain and indexed by [`ObjId::index`], which keeps this
//! module free of any knowledge about message formats or the scheduler.
//!
//! No allocation, no interior mutability; single CPU, callers serialise.
#![allow(dead_code)]

use super::cap_rights::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjError {
    /// The id names a destroyed (or never created) object.
    Stale,
    /// Every slot is in use or retired.
    NoObjects,
}

/// Reference to an object slot, valid only while the generation matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjId {
    index: u16,
    gen_: u32,
}

impl ObjId {
    pub const fn index(self) -> usize {
        self.index as usize
    }

    pub const fn generation(self) -> u32 {
        self.gen_
    }
}

#[derive(Clone, Copy)]
struct Slot {
    gen_: u32,
    retired: bool,
    kind: Option<Kind>,
}

pub struct ObjTable<const S: usize> {
    slots: [Slot; S],
}

impl<const S: usize> Default for ObjTable<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const S: usize> ObjTable<S> {
    pub const fn new() -> Self {
        assert!(
            S > 0 && S <= u16::MAX as usize,
            "slot index must fit in u16"
        );
        Self {
            slots: [Slot {
                gen_: 0,
                retired: false,
                kind: None,
            }; S],
        }
    }

    pub const fn capacity(&self) -> usize {
        S
    }

    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.kind.is_some()).count()
    }

    pub fn retired_count(&self) -> usize {
        self.slots.iter().filter(|s| s.retired).count()
    }

    /// Creates an object of `kind` in the first free slot.
    pub fn create(&mut self, kind: Kind) -> Result<ObjId, ObjError> {
        let i = self
            .slots
            .iter()
            .position(|s| s.kind.is_none() && !s.retired)
            .ok_or(ObjError::NoObjects)?;
        let s = &mut self.slots[i];
        s.kind = Some(kind);
        Ok(ObjId {
            index: i as u16,
            gen_: s.gen_,
        })
    }

    fn slot(&self, id: ObjId) -> Result<&Slot, ObjError> {
        let s = self.slots.get(id.index()).ok_or(ObjError::Stale)?;
        if s.gen_ != id.gen_ || s.kind.is_none() {
            return Err(ObjError::Stale);
        }
        Ok(s)
    }

    pub fn is_live(&self, id: ObjId) -> bool {
        self.slot(id).is_ok()
    }

    pub fn kind(&self, id: ObjId) -> Result<Kind, ObjError> {
        self.slot(id)?.kind.ok_or(ObjError::Stale)
    }

    /// Destroys the object; its id (and every copy of it) is stale from now on.
    pub fn destroy(&mut self, id: ObjId) -> Result<(), ObjError> {
        self.slot(id)?;
        let s = &mut self.slots[id.index()];
        s.kind = None;
        match s.gen_.checked_add(1) {
            Some(g) => s.gen_ = g,
            None => s.retired = true,
        }
        Ok(())
    }

    /// The id of the live object in slot `index`, if any.
    pub fn id_at(&self, index: usize) -> Option<ObjId> {
        let s = self.slots.get(index)?;
        s.kind.map(|_| ObjId {
            index: index as u16,
            gen_: s.gen_,
        })
    }

    /// Ids of every live object, in slot order.
    pub fn live_ids(&self) -> impl Iterator<Item = ObjId> + '_ {
        (0..S).filter_map(move |i| self.id_at(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_lookup_destroy_and_stale_ids() {
        let mut t = ObjTable::<4>::new();
        let a = t.create(Kind::Endpoint).unwrap();
        let b = t.create(Kind::Notification).unwrap();
        assert_ne!(a, b);
        assert_eq!(t.kind(a), Ok(Kind::Endpoint));
        assert_eq!(t.kind(b), Ok(Kind::Notification));
        assert_eq!(t.live_count(), 2);
        t.destroy(a).unwrap();
        assert_eq!(t.kind(a), Err(ObjError::Stale));
        assert_eq!(t.destroy(a), Err(ObjError::Stale), "double destroy");
        assert!(!t.is_live(a));
        assert!(t.is_live(b));
        assert_eq!(t.live_count(), 1);
    }

    #[test]
    fn reused_slot_gets_a_new_generation_so_old_ids_stay_stale() {
        let mut t = ObjTable::<1>::new();
        let a = t.create(Kind::Endpoint).unwrap();
        t.destroy(a).unwrap();
        let b = t.create(Kind::Notification).unwrap();
        assert_eq!(a.index(), b.index());
        assert_eq!(b.generation(), a.generation() + 1);
        assert_eq!(t.kind(a), Err(ObjError::Stale));
        assert_eq!(t.kind(b), Ok(Kind::Notification));
        assert_eq!(
            t.destroy(a),
            Err(ObjError::Stale),
            "stale id cannot destroy the new object"
        );
        assert!(t.is_live(b));
    }

    #[test]
    fn exhaustion_is_reported_and_recovers_after_destroy() {
        let mut t = ObjTable::<3>::new();
        let ids: [ObjId; 3] = core::array::from_fn(|_| t.create(Kind::Endpoint).unwrap());
        assert_eq!(t.create(Kind::Endpoint), Err(ObjError::NoObjects));
        t.destroy(ids[1]).unwrap();
        let n = t.create(Kind::Notification).unwrap();
        assert_eq!(n.index(), 1);
        assert_eq!(t.create(Kind::Endpoint), Err(ObjError::NoObjects));
    }

    #[test]
    fn slot_with_exhausted_generation_is_retired_not_reused() {
        let mut t = ObjTable::<2>::new();
        let a = t.create(Kind::Endpoint).unwrap();
        t.slots[a.index()].gen_ = u32::MAX; // pretend this slot has been recycled 2^32 times
        let a = ObjId {
            index: a.index,
            gen_: u32::MAX,
        };
        t.destroy(a).unwrap();
        assert_eq!(t.retired_count(), 1);
        assert!(!t.is_live(a));
        // The retired slot is skipped; the other slot is still usable, then the table is full.
        let b = t.create(Kind::Endpoint).unwrap();
        assert_ne!(b.index(), a.index());
        assert_eq!(t.create(Kind::Endpoint), Err(ObjError::NoObjects));
        assert_eq!(t.kind(a), Err(ObjError::Stale));
    }

    #[test]
    fn forged_ids_are_stale() {
        let mut t = ObjTable::<2>::new();
        let a = t.create(Kind::Endpoint).unwrap();
        let forged_gen = ObjId {
            index: a.index,
            gen_: a.gen_ + 1,
        };
        let forged_index = ObjId { index: 1, gen_: 0 };
        let out_of_range = ObjId { index: 9, gen_: 0 };
        for id in [forged_gen, forged_index, out_of_range] {
            assert_eq!(t.kind(id), Err(ObjError::Stale));
            assert_eq!(t.destroy(id), Err(ObjError::Stale));
        }
        assert!(t.is_live(a));
    }

    #[test]
    fn live_ids_and_id_at_enumerate_exactly_the_live_objects() {
        let mut t = ObjTable::<4>::new();
        let a = t.create(Kind::Endpoint).unwrap();
        let b = t.create(Kind::Endpoint).unwrap();
        let c = t.create(Kind::Notification).unwrap();
        t.destroy(b).unwrap();
        let live: [Option<ObjId>; 3] = {
            let mut it = t.live_ids();
            [it.next(), it.next(), it.next()]
        };
        assert_eq!(live, [Some(a), Some(c), None]);
        assert_eq!(t.id_at(1), None);
        assert_eq!(t.id_at(0), Some(a));
        assert_eq!(t.id_at(99), None);
    }
}
