//! Reserved SMP IPI vectors. C3 dynamic IRQ allocation remains 0x30..=0xEF.

pub const IPI_RESCHEDULE_VECTOR: u8 = 0xF0;
pub const IPI_CALL_FUNCTION_VECTOR: u8 = 0xF1;
pub const IPI_TLB_SHOOTDOWN_VECTOR: u8 = 0xF2;
pub const IPI_STOP_VECTOR: u8 = 0xF3;
pub const IPI_CPU_ONLINE_VECTOR: u8 = 0xF4;
pub const APIC_TIMER_VECTOR: u8 = 0xF5;
pub const APIC_SPURIOUS_VECTOR: u8 = 0xFF;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpiKind {
    Reschedule,
    CallFunction,
    TlbShootdown,
    Stop,
    CpuOnline,
}

impl IpiKind {
    pub const fn vector(self) -> u8 {
        match self {
            Self::Reschedule => IPI_RESCHEDULE_VECTOR,
            Self::CallFunction => IPI_CALL_FUNCTION_VECTOR,
            Self::TlbShootdown => IPI_TLB_SHOOTDOWN_VECTOR,
            Self::Stop => IPI_STOP_VECTOR,
            Self::CpuOnline => IPI_CPU_ONLINE_VECTOR,
        }
    }
}

pub const fn is_reserved_smp_vector(vector: u8) -> bool {
    (vector >= IPI_RESCHEDULE_VECTOR && vector <= 0xFE) || vector == APIC_SPURIOUS_VECTOR
}

pub trait IpiTransport {
    type Error;
    fn send_fixed(&mut self, destination_apic_id: u32, vector: u8) -> Result<(), Self::Error>;
}

pub fn send<T: IpiTransport>(
    transport: &mut T,
    destination_apic_id: u32,
    kind: IpiKind,
) -> Result<(), T::Error> {
    transport.send_fixed(destination_apic_id, kind.vector())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vectors_do_not_overlap_dynamic_c3_range() {
        for vector in 0x30..=0xEF {
            assert!(!is_reserved_smp_vector(vector));
        }
        for vector in 0xF0..=0xFE {
            assert!(is_reserved_smp_vector(vector));
        }
        assert!(is_reserved_smp_vector(APIC_SPURIOUS_VECTOR));
    }
    #[test]
    fn every_ipi_kind_has_a_distinct_vector() {
        let values = [
            IpiKind::Reschedule.vector(),
            IpiKind::CallFunction.vector(),
            IpiKind::TlbShootdown.vector(),
            IpiKind::Stop.vector(),
            IpiKind::CpuOnline.vector(),
        ];
        for i in 0..values.len() {
            for j in i + 1..values.len() {
                assert_ne!(values[i], values[j]);
            }
        }
    }
}
