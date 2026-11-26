//! DMA region constraints and ownership contract; no IOMMU, cache, or device operations.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaConstraints {
    pub alignment: u64,
    pub max_address: u64,
    pub boundary: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaRegion {
    pub physical: u64,
    pub length: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaError {
    InvalidConstraints,
    ZeroLength,
    Overflow,
    Unaligned,
    AddressLimit,
    BoundaryCrossed,
    NotOwned,
}
impl DmaConstraints {
    pub fn validate(self) -> Result<(), DmaError> {
        if self.alignment == 0
            || !self.alignment.is_power_of_two()
            || (self.boundary != 0 && !self.boundary.is_power_of_two())
        {
            return Err(DmaError::InvalidConstraints);
        }
        Ok(())
    }
    pub fn check(self, r: DmaRegion) -> Result<(), DmaError> {
        self.validate()?;
        if r.length == 0 {
            return Err(DmaError::ZeroLength);
        }
        if r.physical % self.alignment != 0 {
            return Err(DmaError::Unaligned);
        }
        let end = r
            .physical
            .checked_add(r.length - 1)
            .ok_or(DmaError::Overflow)?;
        if end > self.max_address {
            return Err(DmaError::AddressLimit);
        }
        if self.boundary != 0 && r.physical / self.boundary != end / self.boundary {
            return Err(DmaError::BoundaryCrossed);
        }
        Ok(())
    }
}
pub trait DmaAllocator {
    fn allocate_dma(&mut self, length: u64, c: DmaConstraints) -> Result<DmaRegion, DmaError>;
    fn release_dma(&mut self, r: DmaRegion) -> Result<(), DmaError>;
}
#[derive(Debug, PartialEq, Eq)]
pub struct DmaBuffer {
    region: DmaRegion,
    owned: bool,
}
impl DmaBuffer {
    pub fn allocate<A: DmaAllocator>(
        a: &mut A,
        n: u64,
        c: DmaConstraints,
    ) -> Result<Self, DmaError> {
        let r = a.allocate_dma(n, c)?;
        if let Err(e) = c.check(r) {
            let _ = a.release_dma(r);
            return Err(e);
        }
        Ok(Self {
            region: r,
            owned: true,
        })
    }
    pub const fn region(&self) -> DmaRegion {
        self.region
    }
    pub const fn is_owned(&self) -> bool {
        self.owned
    }
    pub fn release<A: DmaAllocator>(&mut self, a: &mut A) -> Result<(), DmaError> {
        if !self.owned {
            return Err(DmaError::NotOwned);
        }
        a.release_dma(self.region)?;
        self.owned = false;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct A {
        next: u64,
        freed: bool,
    }
    impl DmaAllocator for A {
        fn allocate_dma(&mut self, n: u64, c: DmaConstraints) -> Result<DmaRegion, DmaError> {
            let p = (self.next + c.alignment - 1) & !(c.alignment - 1);
            self.next = p + n;
            Ok(DmaRegion {
                physical: p,
                length: n,
            })
        }
        fn release_dma(&mut self, _: DmaRegion) -> Result<(), DmaError> {
            self.freed = true;
            Ok(())
        }
    }
    #[test]
    fn ownership() {
        let c = DmaConstraints {
            alignment: 4096,
            max_address: 0xffff,
            boundary: 65536,
        };
        let mut a = A {
            next: 0x1000,
            freed: false,
        };
        let mut b = DmaBuffer::allocate(&mut a, 8192, c).unwrap();
        b.release(&mut a).unwrap();
        assert!(a.freed);
        assert_eq!(b.release(&mut a), Err(DmaError::NotOwned));
    }
    #[test]
    fn rejects_constraints() {
        let c = DmaConstraints {
            alignment: 4096,
            max_address: 0xffff,
            boundary: 4096,
        };
        assert_eq!(
            c.check(DmaRegion {
                physical: 0x1000,
                length: 8192
            }),
            Err(DmaError::BoundaryCrossed)
        );
        assert_eq!(
            c.check(DmaRegion {
                physical: 0x1001,
                length: 1
            }),
            Err(DmaError::Unaligned)
        );
        assert_eq!(
            c.check(DmaRegion {
                physical: 0x1000,
                length: 0
            }),
            Err(DmaError::ZeroLength)
        );
    }
}
