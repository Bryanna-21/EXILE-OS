//! Validated resource descriptors and overlap semantics; no hardware mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKind {
    Mmio,
    IoPort,
    Irq,
    Dma,
    DeviceSpecific,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resource {
    kind: ResourceKind,
    start: u64,
    length: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceError {
    ZeroLength,
    Overflow,
    InvalidIrq,
    InvalidPort,
}
impl Resource {
    pub fn new(kind: ResourceKind, start: u64, length: u64) -> Result<Self, ResourceError> {
        if length == 0 {
            return Err(ResourceError::ZeroLength);
        }
        let end = start.checked_add(length).ok_or(ResourceError::Overflow)?;
        if kind == ResourceKind::Irq && length != 1 {
            return Err(ResourceError::InvalidIrq);
        }
        if kind == ResourceKind::IoPort && (start > u16::MAX as u64 || end > u16::MAX as u64 + 1) {
            return Err(ResourceError::InvalidPort);
        }
        Ok(Self {
            kind,
            start,
            length,
        })
    }
    pub const fn kind(self) -> ResourceKind {
        self.kind
    }
    pub const fn start(self) -> u64 {
        self.start
    }
    pub const fn length(self) -> u64 {
        self.length
    }
    pub fn end(self) -> u64 {
        self.start + self.length
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.kind == other.kind && self.start < other.end() && other.start < self.end()
    }
    pub fn conflicts(self, other: Self) -> bool {
        self.overlaps(other)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_checked_and_half_open() {
        let a = Resource::new(ResourceKind::Mmio, 100, 10).unwrap();
        let b = Resource::new(ResourceKind::Mmio, 110, 2).unwrap();
        let c = Resource::new(ResourceKind::Mmio, 109, 2).unwrap();
        assert!(!a.conflicts(b));
        assert!(a.conflicts(c));
        assert_eq!(
            Resource::new(ResourceKind::Mmio, u64::MAX, 2),
            Err(ResourceError::Overflow)
        );
        assert_eq!(
            Resource::new(ResourceKind::Irq, 4, 2),
            Err(ResourceError::InvalidIrq)
        );
    }
}
