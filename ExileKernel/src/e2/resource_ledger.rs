//! Fixed-capacity exclusive resource claims with generation-checked ownership tokens.
use super::device_id::DeviceId;
use super::resource::{Resource, ResourceError};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimToken {
    slot: u16,
    generation: u32,
}
impl ClaimToken {
    pub(crate) const fn from_parts(slot: u16, generation: u32) -> Self {
        Self { slot, generation }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerError {
    Full,
    Conflict,
    InvalidOwner,
    Stale,
    NotOwner,
    GenerationExhausted,
    Invalid(ResourceError),
}
#[derive(Clone, Copy)]
struct Claim {
    owner: DeviceId,
    resource: Resource,
    generation: u32,
}
pub struct ResourceLedger<const N: usize> {
    claims: [Option<Claim>; N],
    generation: [u32; N],
    retired: [bool; N],
}
impl<const N: usize> ResourceLedger<N> {
    pub const fn new() -> Self {
        Self {
            claims: [None; N],
            generation: [1; N],
            retired: [false; N],
        }
    }
    pub fn claim(&mut self, owner: DeviceId, r: Resource) -> Result<ClaimToken, LedgerError> {
        if !owner.is_valid() {
            return Err(LedgerError::InvalidOwner);
        }
        Resource::new(r.kind(), r.start(), r.length()).map_err(LedgerError::Invalid)?;
        if self
            .claims
            .iter()
            .flatten()
            .any(|c| c.resource.conflicts(r))
        {
            return Err(LedgerError::Conflict);
        }
        let i = (0..N.min(u16::MAX as usize + 1))
            .find(|i| self.claims[*i].is_none() && !self.retired[*i])
            .ok_or(LedgerError::Full)?;
        let slot = u16::try_from(i).map_err(|_| LedgerError::Full)?;
        let g = self.generation[i];
        self.claims[i] = Some(Claim {
            owner,
            resource: r,
            generation: g,
        });
        Ok(ClaimToken {
            slot,
            generation: g,
        })
    }
    fn index(&self, t: ClaimToken) -> Result<usize, LedgerError> {
        let i = t.slot as usize;
        let c = self
            .claims
            .get(i)
            .and_then(|x| *x)
            .ok_or(LedgerError::Stale)?;
        if c.generation != t.generation {
            return Err(LedgerError::Stale);
        }
        Ok(i)
    }
    pub fn owns(&self, t: ClaimToken, owner: DeviceId) -> bool {
        self.index(t)
            .ok()
            .is_some_and(|i| self.claims[i].is_some_and(|c| c.owner == owner))
    }
    pub fn release(&mut self, t: ClaimToken, owner: DeviceId) -> Result<Resource, LedgerError> {
        let i = self.index(t)?;
        let c = self.claims[i].ok_or(LedgerError::Stale)?;
        if c.owner != owner {
            return Err(LedgerError::NotOwner);
        }
        self.claims[i] = None;
        if let Some(next) = self.generation[i].checked_add(1) {
            self.generation[i] = next
        } else {
            self.retired[i] = true
        }
        Ok(c.resource)
    }
    pub fn check_invariants(&self) -> bool {
        for i in 0..N {
            if self.claims[i].is_some_and(|a| {
                (i + 1..N).any(|j| self.claims[j].is_some_and(|b| a.resource.conflicts(b.resource)))
            }) {
                return false;
            }
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2::device_id::BusKind;
    use crate::e2::resource::ResourceKind;
    fn owner(n: u32) -> DeviceId {
        DeviceId::new(BusKind::Platform, 0, n, 1).unwrap()
    }
    #[test]
    fn conflict_ownership_and_stale_token() {
        let mut l = ResourceLedger::<2>::new();
        let r = Resource::new(ResourceKind::Mmio, 0x1000, 0x100).unwrap();
        let t = l.claim(owner(1), r).unwrap();
        assert_eq!(l.claim(owner(2), r), Err(LedgerError::Conflict));
        assert_eq!(l.release(t, owner(2)), Err(LedgerError::NotOwner));
        l.release(t, owner(1)).unwrap();
        assert_eq!(l.release(t, owner(1)), Err(LedgerError::Stale));
        assert!(l.check_invariants());
    }
    #[test]
    fn nonoverlap_allowed() {
        let mut l = ResourceLedger::<2>::new();
        l.claim(
            owner(1),
            Resource::new(ResourceKind::IoPort, 0x100, 8).unwrap(),
        )
        .unwrap();
        l.claim(
            owner(2),
            Resource::new(ResourceKind::IoPort, 0x108, 8).unwrap(),
        )
        .unwrap();
    }
}
