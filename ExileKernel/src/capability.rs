//! Generation-checked capability table with explicit rights and revocation.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights(u16);
impl Rights {
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const EXECUTE: Self = Self(4);
    pub const CONTROL: Self = Self(8);
    pub const fn union(self, r: Self) -> Self {
        Self(self.0 | r.0)
    }
    pub const fn contains(self, r: Self) -> bool {
        self.0 & r.0 == r.0
    }
    pub const fn bits(self) -> u16 {
        self.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    index: u16,
    generation: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityError {
    Capacity,
    Invalid,
    Revoked,
    InsufficientRights,
    InvalidObject,
}
#[derive(Debug, Clone, Copy)]
struct Entry {
    object: u64,
    rights: Rights,
    generation: u32,
    live: bool,
}
pub struct CapabilityTable<const N: usize> {
    items: [Option<Entry>; N],
}
impl<const N: usize> CapabilityTable<N> {
    pub const fn new() -> Self {
        Self { items: [None; N] }
    }
    pub fn grant(&mut self, obj: u64, r: Rights) -> Result<Capability, CapabilityError> {
        if obj == 0 || r.bits() == 0 {
            return Err(CapabilityError::InvalidObject);
        }
        let i = self
            .items
            .iter()
            .position(|x| x.is_none() || x.is_some_and(|e| !e.live))
            .ok_or(CapabilityError::Capacity)?;
        let index = u16::try_from(i).map_err(|_| CapabilityError::Capacity)?;
        let g = self.items[i].map_or(1, |e| e.generation.wrapping_add(1).max(1));
        self.items[i] = Some(Entry {
            object: obj,
            rights: r,
            generation: g,
            live: true,
        });
        Ok(Capability {
            index,
            generation: g,
        })
    }
    fn get(&self, c: Capability) -> Result<Entry, CapabilityError> {
        let e = self
            .items
            .get(c.index as usize)
            .and_then(|x| *x)
            .ok_or(CapabilityError::Invalid)?;
        if e.generation != c.generation {
            return Err(CapabilityError::Invalid);
        }
        if !e.live {
            return Err(CapabilityError::Revoked);
        }
        Ok(e)
    }
    pub fn check(&self, c: Capability, r: Rights) -> Result<u64, CapabilityError> {
        let e = self.get(c)?;
        if !e.rights.contains(r) {
            return Err(CapabilityError::InsufficientRights);
        }
        Ok(e.object)
    }
    pub fn revoke(&mut self, c: Capability) -> Result<(), CapabilityError> {
        let e = self
            .items
            .get_mut(c.index as usize)
            .and_then(Option::as_mut)
            .ok_or(CapabilityError::Invalid)?;
        if e.generation != c.generation {
            return Err(CapabilityError::Invalid);
        }
        if !e.live {
            return Err(CapabilityError::Revoked);
        }
        e.live = false;
        Ok(())
    }
}
impl<const N: usize> Default for CapabilityTable<N> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rights_revoke() {
        let mut t = CapabilityTable::<1>::new();
        let c = t.grant(7, Rights::READ.union(Rights::WRITE)).unwrap();
        assert_eq!(t.check(c, Rights::READ), Ok(7));
        assert_eq!(
            t.check(c, Rights::EXECUTE),
            Err(CapabilityError::InsufficientRights)
        );
        t.revoke(c).unwrap();
        assert_eq!(t.check(c, Rights::READ), Err(CapabilityError::Revoked));
    }
    #[test]
    fn stale_generation() {
        let mut t = CapabilityTable::<1>::new();
        let a = t.grant(1, Rights::READ).unwrap();
        t.revoke(a).unwrap();
        let b = t.grant(2, Rights::READ).unwrap();
        assert_ne!(a.generation, b.generation);
        assert_eq!(t.check(a, Rights::READ), Err(CapabilityError::Invalid));
    }
}
