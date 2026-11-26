//! Stable bus-qualified device identity; no pointer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BusKind {
    Platform,
    Pci,
    Usb,
    Acpi,
    Fdt,
    Virtual,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeviceId {
    bus: BusKind,
    segment: u16,
    address: u32,
    generation: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceIdError {
    ZeroGeneration,
}
impl DeviceId {
    pub const fn new(
        bus: BusKind,
        segment: u16,
        address: u32,
        generation: u32,
    ) -> Result<Self, DeviceIdError> {
        if generation == 0 {
            return Err(DeviceIdError::ZeroGeneration);
        }
        Ok(Self {
            bus,
            segment,
            address,
            generation,
        })
    }
    pub const fn is_valid(self) -> bool {
        self.generation != 0
    }
    pub const fn bus(self) -> BusKind {
        self.bus
    }
    pub const fn segment(self) -> u16 {
        self.segment
    }
    pub const fn address(self) -> u32 {
        self.address
    }
    pub const fn generation(self) -> u32 {
        self.generation
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_generation_and_orders() {
        assert_eq!(
            DeviceId::new(BusKind::Pci, 0, 1, 0),
            Err(DeviceIdError::ZeroGeneration)
        );
        let a = DeviceId::new(BusKind::Pci, 0, 1, 1).unwrap();
        let b = DeviceId::new(BusKind::Pci, 0, 2, 1).unwrap();
        assert!(a < b);
    }
}
