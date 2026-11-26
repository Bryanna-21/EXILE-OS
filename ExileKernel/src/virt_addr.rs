//! Checked virtual-address primitives; no paging hardware access.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtError {
    NonCanonical,
    Empty,
    Inverted,
    Overflow,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtAddr(u64);
impl VirtAddr {
    pub const fn new(v: u64) -> Result<Self, VirtError> {
        let u = v >> 48;
        let s = (v >> 47) & 1;
        if (s == 0 && u == 0) || (s == 1 && u == 0xffff) {
            Ok(Self(v))
        } else {
            Err(VirtError::NonCanonical)
        }
    }
    pub const fn get(self) -> u64 {
        self.0
    }
    pub const fn page_offset(self) -> u16 {
        (self.0 & 4095) as u16
    }
    pub fn checked_add(self, n: u64) -> Result<Self, VirtError> {
        Self::new(self.0.checked_add(n).ok_or(VirtError::Overflow)?)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtRange {
    start: VirtAddr,
    end: VirtAddr,
}
impl VirtRange {
    pub fn new(a: VirtAddr, b: VirtAddr) -> Result<Self, VirtError> {
        if a.get() >= b.get() {
            return Err(if a == b {
                VirtError::Empty
            } else {
                VirtError::Inverted
            });
        }
        if (a.get() >> 47) != ((b.get() - 1) >> 47) {
            return Err(VirtError::NonCanonical);
        }
        Ok(Self { start: a, end: b })
    }
    pub fn from_len(a: VirtAddr, n: u64) -> Result<Self, VirtError> {
        if n == 0 {
            return Err(VirtError::Empty);
        }
        Self::new(a, a.checked_add(n)?)
    }
    pub const fn start(self) -> VirtAddr {
        self.start
    }
    pub const fn end(self) -> VirtAddr {
        self.end
    }
    pub const fn len(self) -> u64 {
        self.end.get() - self.start.get()
    }
    pub const fn contains(self, a: VirtAddr) -> bool {
        self.start.get() <= a.get() && a.get() < self.end.get()
    }
    pub const fn overlaps(self, b: Self) -> bool {
        self.start.get() < b.end.get() && b.start.get() < self.end.get()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical() {
        assert!(VirtAddr::new(0x7fff_ffff_ffff).is_ok());
        assert!(VirtAddr::new(0xffff_8000_0000_0000).is_ok());
        assert_eq!(
            VirtAddr::new(0x8000_0000_0000),
            Err(VirtError::NonCanonical)
        );
    }
    #[test]
    fn range_and_overflow() {
        let a = VirtAddr::new(0x1000).unwrap();
        let r = VirtRange::from_len(a, 8192).unwrap();
        assert!(r.contains(VirtAddr::new(0x2fff).unwrap()));
        assert!(!r.contains(VirtAddr::new(0x3000).unwrap()));
        assert_eq!(a.checked_add(u64::MAX), Err(VirtError::Overflow));
    }
}
