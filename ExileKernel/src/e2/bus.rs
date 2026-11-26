//! Bus enumeration adapter and bounded discovery sink; no PCI/USB/ACPI parser.
use super::device::DeviceDescriptor;
use super::device_id::DeviceId;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusDescriptor {
    pub id: u32,
    pub kind: super::device_id::BusKind,
    pub parent: Option<DeviceId>,
}
impl BusDescriptor {
    pub fn validate(self) -> Result<Self, BusError> {
        if self.id == 0 || self.parent.is_some_and(|p| !p.is_valid()) {
            Err(BusError::InvalidBus)
        } else {
            Ok(self)
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusError {
    InvalidBus,
    Full,
    Duplicate,
    InvalidTopology,
    Partial { discovered: usize, code: u16 },
}
pub struct DeviceSink<const N: usize> {
    items: [Option<DeviceDescriptor>; N],
    len: usize,
}
impl<const N: usize> DeviceSink<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub fn emit(&mut self, d: DeviceDescriptor) -> Result<(), BusError> {
        if d.validate().is_err() {
            return Err(BusError::InvalidTopology);
        }
        if d.parent
            .is_some_and(|p| p == d.id || !self.items.iter().flatten().any(|x| x.id == p))
        {
            return Err(BusError::InvalidTopology);
        }
        if self.items.iter().flatten().any(|x| x.id == d.id) {
            return Err(BusError::Duplicate);
        }
        let s = self
            .items
            .iter_mut()
            .find(|x| x.is_none())
            .ok_or(BusError::Full)?;
        *s = Some(d);
        self.len += 1;
        Ok(())
    }
    pub fn iter(&self) -> impl Iterator<Item = &DeviceDescriptor> {
        self.items.iter().filter_map(Option::as_ref)
    }
}
pub trait BusEnumerator {
    fn descriptor(&self) -> BusDescriptor;
    fn enumerate<const N: usize>(&self, sink: &mut DeviceSink<N>) -> Result<(), BusError>;
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2::device::{DeviceClass, DeviceState};
    use crate::e2::device_id::BusKind;
    fn d(n: u32, parent: Option<DeviceId>) -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId::new(BusKind::Platform, 0, n, 1).unwrap(),
            parent,
            class: DeviceClass::Other,
            vendor: 0,
            product: 0,
            revision: 0,
            state: DeviceState::Discovered,
            failure: None,
        }
    }
    #[test]
    fn empty_bounded_duplicate_and_topology() {
        let mut s = DeviceSink::<1>::new();
        assert_eq!(s.len(), 0);
        s.emit(d(1, None)).unwrap();
        assert_eq!(s.emit(d(1, None)), Err(BusError::Duplicate));
        assert_eq!(s.emit(d(2, None)), Err(BusError::Full));
        let mut x = DeviceSink::<2>::new();
        assert_eq!(
            x.emit(d(2, Some(d(1, None).id))),
            Err(BusError::InvalidTopology)
        );
    }
}
