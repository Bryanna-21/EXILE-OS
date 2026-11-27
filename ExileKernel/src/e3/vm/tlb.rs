//! Local TLB invalidation contract; deliberately no SMP shootdown abstraction.
/// Local-CPU invalidation only. Implementations do not imply remote CPU shootdown.
pub trait TlbInvalidator {
    fn invalidate_page(&mut self, virtual_address: u64) -> Result<(), TlbError>;
    fn invalidate_all(&mut self) -> Result<(), TlbError>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlbError {
    Unsupported,
    HardwareFault,
    InjectedFailure,
}
#[cfg(test)]
pub struct MockTlb {
    pub events: [u64; 64],
    pub len: usize,
    pub fail: bool,
}
#[cfg(test)]
impl MockTlb {
    pub const fn new() -> Self {
        Self {
            events: [0; 64],
            len: 0,
            fail: false,
        }
    }
}
#[cfg(test)]
impl TlbInvalidator for MockTlb {
    fn invalidate_page(&mut self, v: u64) -> Result<(), TlbError> {
        if self.fail {
            return Err(TlbError::InjectedFailure);
        }
        if self.len == 64 {
            return Err(TlbError::InjectedFailure);
        }
        self.events[self.len] = v;
        self.len += 1;
        Ok(())
    }
    fn invalidate_all(&mut self) -> Result<(), TlbError> {
        if self.fail || self.len == 64 {
            return Err(TlbError::InjectedFailure);
        }
        self.events[self.len] = u64::MAX;
        self.len += 1;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalidations_are_ordered_and_fail_explicitly() {
        let mut t = MockTlb::new();
        t.invalidate_page(0x4000).unwrap();
        t.invalidate_page(0x5000).unwrap();
        assert_eq!(&t.events[..2], &[0x4000, 0x5000]);
        t.fail = true;
        assert_eq!(t.invalidate_page(0x6000), Err(TlbError::InjectedFailure));
    }
}
