//! Fixed-capacity device registry with identity duplicate protection.
use super::device::{DeviceDescriptor, DeviceError};
use super::device_id::DeviceId;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    Full,
    Duplicate,
    NotFound,
    Invalid(DeviceError),
    Corrupt,
}
pub struct DeviceRegistry<const N: usize> {
    items: [Option<DeviceDescriptor>; N],
    len: usize,
}
impl<const N: usize> DeviceRegistry<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub fn register(&mut self, d: DeviceDescriptor) -> Result<(), RegistryError> {
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
    pub fn get(&self, id: DeviceId) -> Option<&DeviceDescriptor> {
        self.items.iter().flatten().find(|x| x.id == id)
    }
    pub fn get_mut(&mut self, id: DeviceId) -> Option<&mut DeviceDescriptor> {
        self.items
            .iter_mut()
            .filter_map(Option::as_mut)
            .find(|x| x.id == id)
    }
    pub fn remove(&mut self, id: DeviceId) -> Result<DeviceDescriptor, RegistryError> {
        let i = self
            .items
            .iter()
            .position(|x| x.is_some_and(|d| d.id == id))
            .ok_or(RegistryError::NotFound)?;
        let d = self.items[i].take().ok_or(RegistryError::Corrupt)?;
        self.len -= 1;
        Ok(d)
    }
    pub fn iter(&self) -> impl Iterator<Item = &DeviceDescriptor> {
        self.items.iter().filter_map(Option::as_ref)
    }
    pub fn check_invariants(&self) -> bool {
        self.len == self.items.iter().filter(|x| x.is_some()).count()
            && self.iter().all(|d| d.validate().is_ok())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2::device::{DeviceClass, DeviceState};
    use crate::e2::device_id::BusKind;
    fn d(n: u32) -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId::new(BusKind::Platform, 0, n, 1).unwrap(),
            parent: None,
            class: DeviceClass::Other,
            vendor: 0,
            product: 0,
            revision: 0,
            state: DeviceState::Discovered,
            failure: None,
        }
    }
    #[test]
    fn register_lookup_remove_and_capacity() {
        let mut r = DeviceRegistry::<1>::new();
        r.register(d(1)).unwrap();
        assert_eq!(r.register(d(1)), Err(RegistryError::Duplicate));
        assert_eq!(r.register(d(2)), Err(RegistryError::Full));
        let id = d(1).id;
        assert!(r.get(id).is_some());
        r.remove(id).unwrap();
        assert!(r.get(id).is_none());
        assert!(r.check_invariants());
    }
}
