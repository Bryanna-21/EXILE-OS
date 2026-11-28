//! Local APIC access contracts for xAPIC MMIO and x2APIC MSR modes.

pub const IA32_X2APIC_EOI: u32 = 0x80B;
pub const IA32_X2APIC_ICR: u32 = 0x830;
pub const APIC_REG_EOI: u32 = 0x0B0;
pub const APIC_REG_SVR: u32 = 0x0F0;
pub const APIC_REG_ICR_LOW: u32 = 0x300;
pub const APIC_REG_ICR_HIGH: u32 = 0x310;
pub const APIC_ICR_DELIVERY_PENDING: u32 = 1 << 12;
pub const APIC_ICR_LEVEL_ASSERT: u32 = 1 << 14;
pub const APIC_SVR_ENABLE: u32 = 1 << 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApicError {
    InvalidVector,
    DestinationOutOfRange,
    DeliveryTimeout,
}

pub trait ApicMmio {
    fn read32(&mut self, offset: u32) -> u32;
    fn write32(&mut self, offset: u32, value: u32);
}

pub trait ApicMsr {
    fn read_msr(&mut self, msr: u32) -> u64;
    fn write_msr(&mut self, msr: u32, value: u64);
}

pub trait LocalApicOps {
    fn eoi(&mut self);
    fn send_fixed_ipi(&mut self, destination_apic_id: u32, vector: u8) -> Result<(), ApicError>;
}

pub struct XApic<M> {
    mmio: M,
    poll_limit: usize,
}
impl<M: ApicMmio> XApic<M> {
    pub const fn new(mmio: M, poll_limit: usize) -> Self {
        Self { mmio, poll_limit }
    }
    pub fn into_inner(self) -> M {
        self.mmio
    }
    pub fn enable(&mut self, spurious_vector: u8) -> Result<(), ApicError> {
        if spurious_vector != crate::smp::ipi::APIC_SPURIOUS_VECTOR {
            return Err(ApicError::InvalidVector);
        }
        self.mmio
            .write32(APIC_REG_SVR, (spurious_vector as u32) | APIC_SVR_ENABLE);
        Ok(())
    }
}
impl<M: ApicMmio> LocalApicOps for XApic<M> {
    fn eoi(&mut self) {
        self.mmio.write32(APIC_REG_EOI, 0);
    }
    fn send_fixed_ipi(&mut self, destination_apic_id: u32, vector: u8) -> Result<(), ApicError> {
        if vector < 32 {
            return Err(ApicError::InvalidVector);
        }
        if destination_apic_id > u8::MAX as u32 {
            return Err(ApicError::DestinationOutOfRange);
        }
        let mut pending = true;
        for _ in 0..self.poll_limit {
            if self.mmio.read32(APIC_REG_ICR_LOW) & APIC_ICR_DELIVERY_PENDING == 0 {
                pending = false;
                break;
            }
            core::hint::spin_loop();
        }
        if pending {
            return Err(ApicError::DeliveryTimeout);
        }
        self.mmio
            .write32(APIC_REG_ICR_HIGH, destination_apic_id << 24);
        self.mmio
            .write32(APIC_REG_ICR_LOW, vector as u32 | APIC_ICR_LEVEL_ASSERT);
        for _ in 0..self.poll_limit {
            if self.mmio.read32(APIC_REG_ICR_LOW) & APIC_ICR_DELIVERY_PENDING == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(ApicError::DeliveryTimeout)
    }
}

pub struct X2Apic<M> {
    msr: M,
}
impl<M: ApicMsr> X2Apic<M> {
    pub const fn new(msr: M) -> Self {
        Self { msr }
    }
    pub fn into_inner(self) -> M {
        self.msr
    }
}
impl<M: ApicMsr> LocalApicOps for X2Apic<M> {
    fn eoi(&mut self) {
        self.msr.write_msr(IA32_X2APIC_EOI, 0);
    }
    fn send_fixed_ipi(&mut self, destination_apic_id: u32, vector: u8) -> Result<(), ApicError> {
        if vector < 32 {
            return Err(ApicError::InvalidVector);
        }
        let value = ((destination_apic_id as u64) << 32) | vector as u64 | (1 << 14);
        self.msr.write_msr(IA32_X2APIC_ICR, value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        regs: [u32; 0x320 / 4],
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                regs: [0; 0x320 / 4],
            }
        }
    }
    impl ApicMmio for Fake {
        fn read32(&mut self, offset: u32) -> u32 {
            self.regs[(offset / 4) as usize]
        }
        fn write32(&mut self, offset: u32, value: u32) {
            self.regs[(offset / 4) as usize] = value;
        }
    }
    #[test]
    fn xapic_programs_spurious_vector_and_eoi() {
        let mut apic = XApic::new(Fake::default(), 4);
        apic.enable(0xFF).unwrap();
        apic.eoi();
        let fake = apic.into_inner();
        assert_eq!(fake.regs[(APIC_REG_SVR / 4) as usize], 0x1FF);
        assert_eq!(fake.regs[(APIC_REG_EOI / 4) as usize], 0);
    }
    #[test]
    fn xapic_rejects_invalid_spurious_vectors() {
        let mut apic = XApic::new(Fake::default(), 4);
        assert_eq!(apic.enable(0xF0), Err(ApicError::InvalidVector));
        assert_eq!(apic.enable(0xFE), Err(ApicError::InvalidVector));
        assert_eq!(apic.enable(0x20), Err(ApicError::InvalidVector));
    }

    #[test]
    fn xapic_rejects_unrepresentable_destination() {
        let mut apic = XApic::new(Fake::default(), 4);
        assert_eq!(
            apic.send_fixed_ipi(300, 0xF0),
            Err(ApicError::DestinationOutOfRange)
        );
    }
}
