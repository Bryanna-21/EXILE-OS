//! Page-table memory access contract. Physical addresses are never dereferenced implicitly.
/// Implementors must validate access to each physical table frame. A physical address is
/// not itself a pointer: no implementation may cast it and dereference it implicitly.
/// `zero_table` must initialize all 512 entries before the frame is published.
pub trait PageTableMemory {
    fn zero_table(&mut self, physical: u64) -> Result<(), BackendError>;
    /// Reads one entry; indices outside `0..512` must be rejected.
    fn read_entry(&self, table: u64, index: usize) -> Result<u64, BackendError>;
    /// Writes one entry with architecture-appropriate ordering before TLB invalidation.
    fn write_entry(&mut self, table: u64, index: usize, value: u64) -> Result<(), BackendError>;
    /// Drops host-side table metadata after the backing frame has been returned. Hardware backends may leave this as a no-op.
    fn release_table(&mut self, _physical: u64) {}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendError {
    UnknownTable,
    Index,
    Capacity,
    InjectedFailure,
}
/// Host-only deterministic backend. Each registered table is an explicit 512-entry array.
#[cfg(test)]
pub struct MockBackend<const N: usize> {
    tables: [Option<(u64, [u64; 512])>; N],
    fail_after: Option<usize>,
    writes: usize,
}
#[cfg(test)]
impl<const N: usize> MockBackend<N> {
    pub const fn new() -> Self {
        Self {
            tables: [const { None }; N],
            fail_after: None,
            writes: 0,
        }
    }
    pub fn fail_after_writes(&mut self, n: usize) {
        self.fail_after = Some(n);
        self.writes = 0;
    }
    pub fn corrupt(&mut self, table: u64, index: usize, value: u64) -> Result<(), BackendError> {
        let t = self
            .tables
            .iter_mut()
            .flatten()
            .find(|x| x.0 == table)
            .ok_or(BackendError::UnknownTable)?;
        t.1[index] = value;
        Ok(())
    }
    pub fn table_count(&self) -> usize {
        self.tables.iter().filter(|x| x.is_some()).count()
    }
}
#[cfg(test)]
impl<const N: usize> PageTableMemory for MockBackend<N> {
    fn zero_table(&mut self, p: u64) -> Result<(), BackendError> {
        if self.tables.iter().flatten().any(|x| x.0 == p) {
            return Err(BackendError::UnknownTable);
        }
        let s = self
            .tables
            .iter_mut()
            .find(|x| x.is_none())
            .ok_or(BackendError::Capacity)?;
        *s = Some((p, [0; 512]));
        Ok(())
    }
    fn read_entry(&self, p: u64, i: usize) -> Result<u64, BackendError> {
        if i >= 512 {
            return Err(BackendError::Index);
        }
        self.tables
            .iter()
            .flatten()
            .find(|x| x.0 == p)
            .map(|x| x.1[i])
            .ok_or(BackendError::UnknownTable)
    }
    fn release_table(&mut self, p: u64) {
        if let Some(i) = self.tables.iter().position(|x| x.is_some_and(|t| t.0 == p)) {
            self.tables[i] = None;
        }
    }
    fn write_entry(&mut self, p: u64, i: usize, v: u64) -> Result<(), BackendError> {
        if i >= 512 {
            return Err(BackendError::Index);
        }
        if self.fail_after.is_some_and(|n| self.writes >= n) {
            self.fail_after = None;
            return Err(BackendError::InjectedFailure);
        }
        let t = self
            .tables
            .iter_mut()
            .flatten()
            .find(|x| x.0 == p)
            .ok_or(BackendError::UnknownTable)?;
        t.1[i] = v;
        self.writes += 1;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_zero_initialization_and_bounds() {
        let mut b = MockBackend::<1>::new();
        b.zero_table(0x1000).unwrap();
        for i in 0..512 {
            assert_eq!(b.read_entry(0x1000, i), Ok(0));
        }
        assert_eq!(b.read_entry(0x1000, 512), Err(BackendError::Index));
        assert_eq!(b.zero_table(0x2000), Err(BackendError::Capacity));
    }
}
