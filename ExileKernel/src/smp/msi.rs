//! PCI MSI message construction and ordered capability programming.

pub const MSI_ADDRESS_BASE: u32 = 0xFEE0_0000;
pub const MSI_VECTOR_MIN: u8 = 0x30;
pub const MSI_VECTOR_MAX: u8 = 0xEF;
pub const MSI_CONTROL_ENABLE: u16 = 1;
pub const MSI_CONTROL_64BIT: u16 = 1 << 7;
pub const MSI_CONTROL_MME_MASK: u16 = 0b111 << 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsiError {
    InvalidVector,
    DestinationOutOfRange,
    UnsupportedAddressWidth,
    InvalidMessage,
    Device,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MsiMessage {
    pub address_low: u32,
    pub address_high: u32,
    pub data: u16,
}
impl MsiMessage {
    pub fn fixed_delivery(destination_apic_id: u32, vector: u8) -> Result<Self, MsiError> {
        if !(MSI_VECTOR_MIN..=MSI_VECTOR_MAX).contains(&vector) {
            return Err(MsiError::InvalidVector);
        }
        if destination_apic_id > 0xFF {
            return Err(MsiError::DestinationOutOfRange);
        }
        let address_low = MSI_ADDRESS_BASE | (destination_apic_id << 12);
        Ok(Self {
            address_low,
            address_high: 0,
            data: vector as u16,
        })
    }
}

pub trait PciMsiConfig {
    fn read_control(&mut self) -> Result<u16, ()>;
    fn write_control(&mut self, value: u16) -> Result<(), ()>;
    fn write_address_low(&mut self, value: u32) -> Result<(), ()>;
    fn write_address_high(&mut self, value: u32) -> Result<(), ()>;
    fn write_data(&mut self, value: u16) -> Result<(), ()>;
}

/// Programs MSI with the enable bit written last, after address and data are valid.
pub fn configure_msi(
    device: &mut impl PciMsiConfig,
    message: MsiMessage,
    is_64bit: bool,
) -> Result<(), MsiError> {
    let old = device.read_control().map_err(|_| MsiError::Device)?;

    if is_64bit && old & MSI_CONTROL_64BIT == 0 {
        return Err(MsiError::UnsupportedAddressWidth);
    }

    let vector = message.data as u8;
    if message.data > u8::MAX as u16
        || !(MSI_VECTOR_MIN..=MSI_VECTOR_MAX).contains(&vector)
        || message.address_low & 0xFFF0_0FFF != MSI_ADDRESS_BASE
        || (!is_64bit && message.address_high != 0)
    {
        return Err(MsiError::InvalidMessage);
    }

    device
        .write_control(old & !MSI_CONTROL_ENABLE)
        .map_err(|_| MsiError::Device)?;
    device
        .write_address_low(message.address_low)
        .map_err(|_| MsiError::Device)?;
    if is_64bit {
        device
            .write_address_high(message.address_high)
            .map_err(|_| MsiError::Device)?;
    }
    device
        .write_data(message.data)
        .map_err(|_| MsiError::Device)?;
    // This implementation configures exactly one message vector.
    // The 64-bit-capable bit is read-only hardware capability information.
    // Preserve it; only clear MME and set MSI Enable after programming.
    let control = old & !(MSI_CONTROL_ENABLE | MSI_CONTROL_MME_MASK);
    device
        .write_control(control | MSI_CONTROL_ENABLE)
        .map_err(|_| MsiError::Device)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        control: u16,
        low: u32,
        high: u32,
        data: u16,
        writes: u8,
    }
    impl PciMsiConfig for Fake {
        fn read_control(&mut self) -> Result<u16, ()> {
            Ok(self.control)
        }
        fn write_control(&mut self, v: u16) -> Result<(), ()> {
            self.control = v;
            self.writes += 1;
            Ok(())
        }
        fn write_address_low(&mut self, v: u32) -> Result<(), ()> {
            self.low = v;
            self.writes += 1;
            Ok(())
        }
        fn write_address_high(&mut self, v: u32) -> Result<(), ()> {
            self.high = v;
            self.writes += 1;
            Ok(())
        }
        fn write_data(&mut self, v: u16) -> Result<(), ()> {
            self.data = v;
            self.writes += 1;
            Ok(())
        }
    }
    #[test]
    fn builds_and_programs_64_bit_message() {
        let message = MsiMessage::fixed_delivery(2, 0x40).unwrap();
        let mut fake = Fake {
            control: MSI_CONTROL_64BIT | MSI_CONTROL_MME_MASK,
            ..Fake::default()
        };
        configure_msi(&mut fake, message, true).unwrap();
        assert_eq!(fake.low, MSI_ADDRESS_BASE | (2 << 12));
        assert_eq!(fake.data, 0x40);
        assert_ne!(fake.control & MSI_CONTROL_ENABLE, 0);
        assert_ne!(fake.control & MSI_CONTROL_64BIT, 0);
        assert_eq!(fake.control & MSI_CONTROL_MME_MASK, 0);
        assert_eq!(fake.writes, 5);
    }
    #[test]
    fn preserves_64_bit_capability_when_programming_32_bit_message() {
        let message = MsiMessage::fixed_delivery(1, 0x41).unwrap();
        let mut fake = Fake {
            control: MSI_CONTROL_64BIT,
            ..Fake::default()
        };

        configure_msi(&mut fake, message, false).unwrap();

        assert_ne!(fake.control & MSI_CONTROL_64BIT, 0);
        assert_ne!(fake.control & MSI_CONTROL_ENABLE, 0);
        assert_eq!(fake.high, 0);
    }

    #[test]
    fn rejects_unsupported_64_bit_capability_before_writing() {
        let message = MsiMessage::fixed_delivery(1, 0x40).unwrap();
        let mut fake = Fake::default();
        assert_eq!(
            configure_msi(&mut fake, message, true),
            Err(MsiError::UnsupportedAddressWidth)
        );
        assert_eq!(fake.writes, 0);
    }

    #[test]
    fn rejects_reserved_smp_vectors() {
        assert_eq!(
            MsiMessage::fixed_delivery(1, 0xF0),
            Err(MsiError::InvalidVector)
        );
    }
}
