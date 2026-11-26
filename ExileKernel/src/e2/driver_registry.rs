//! Fixed-capacity driver descriptor registry with stable insertion order.
use super::driver::{DriverDescriptor, DriverError, DriverId};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    Full,
    Duplicate,
    NotFound,
    Invalid(DriverError),
}
pub struct DriverRegistry<const N: usize> {
    items: [Option<DriverDescriptor>; N],
    len: usize,
}
impl<const N: usize> DriverRegistry<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub fn register(&mut self, d: DriverDescriptor) -> Result<(), RegistryError> {
        d.validate().map_err(RegistryError::Invalid)?;
        if self.items.iter().flatten().any(|x| x.id == d.id) {
            return Err(RegistryError::Duplicate);
        }
        let s = self
            .items
            .iter_mut()
            .find(|x| x.is_none())
            .ok_or(RegistryError::Full)?;
        *s = Some(d);
        self.len += 1;
        Ok(())
    }
    pub fn get(&self, id: DriverId) -> Option<&DriverDescriptor> {
        self.items.iter().flatten().find(|d| d.id == id)
    }
    pub fn remove(&mut self, id: DriverId) -> Result<DriverDescriptor, RegistryError> {
        let i = self
            .items
            .iter()
            .position(|x| x.is_some_and(|d| d.id == id))
            .ok_or(RegistryError::NotFound)?;
        let d = self.items[i].take().ok_or(RegistryError::NotFound)?;
        self.len -= 1;
        Ok(d)
    }
    pub fn iter(&self) -> impl Iterator<Item = &DriverDescriptor> {
        self.items.iter().filter_map(Option::as_ref)
    }
    pub fn check_invariants(&self) -> bool {
        self.len == self.items.iter().filter(|x| x.is_some()).count()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn p(_: &crate::e2::device::DeviceDescriptor) -> Result<bool, crate::e2::driver::DriverError> {
        Ok(true)
    }
    fn op(_: &crate::e2::device::DeviceDescriptor) -> Result<(), crate::e2::driver::DriverError> {
        Ok(())
    }
    fn d(id: u32) -> DriverDescriptor {
        DriverDescriptor {
            id: DriverId(id),
            name: "test",
            priority: 0,
            matches: &[],
            probe: p,
            start: op,
            stop: op,
            unbind: op,
        }
    }
    #[test]
    fn duplicate_capacity_and_remove() {
        let mut r = DriverRegistry::<1>::new();
        r.register(d(1)).unwrap();
        assert_eq!(r.register(d(1)), Err(RegistryError::Duplicate));
        assert_eq!(r.register(d(2)), Err(RegistryError::Full));
        r.remove(DriverId(1)).unwrap();
        assert!(r.check_invariants());
    }
}
