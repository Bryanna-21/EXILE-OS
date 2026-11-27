//! Structured x86 page-fault decoding; no automatic recovery is attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFaultBits {
    pub present: bool,
    pub write: bool,
    pub user: bool,
    pub reserved: bool,
    pub instruction: bool,
    pub protection_key: bool,
    pub shadow_stack: bool,
}
impl PageFaultBits {
    pub const fn decode(code: u64) -> Self {
        Self {
            present: code & 1 != 0,
            write: code & 2 != 0,
            user: code & 4 != 0,
            reserved: code & 8 != 0,
            instruction: code & 16 != 0,
            protection_key: code & 32 != 0,
            shadow_stack: code & 64 != 0,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultClass {
    UserNotPresent,
    UserProtection,
    KernelNotPresent,
    KernelProtection,
    ReservedBit,
    ProtectionKey,
    ShadowStack,
    Malformed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultInfo {
    pub address: u64,
    pub instruction_pointer: u64,
    pub bits: PageFaultBits,
    pub from_user: bool,
}
pub fn classify(f: FaultInfo) -> FaultClass {
    if f.bits.reserved {
        return FaultClass::ReservedBit;
    }
    if f.bits.protection_key {
        return FaultClass::ProtectionKey;
    }
    if f.bits.shadow_stack {
        return FaultClass::ShadowStack;
    }
    if f.bits.user != f.from_user {
        return FaultClass::Malformed;
    }
    if f.from_user {
        if f.bits.present {
            FaultClass::UserProtection
        } else {
            FaultClass::UserNotPresent
        }
    } else if f.bits.present {
        FaultClass::KernelProtection
    } else {
        FaultClass::KernelNotPresent
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classifies_access_and_corruption() {
        let mk = |code, from_user| FaultInfo {
            address: 0x4000,
            instruction_pointer: 0x5000,
            bits: PageFaultBits::decode(code),
            from_user,
        };
        assert_eq!(classify(mk(4, true)), FaultClass::UserNotPresent);
        assert_eq!(classify(mk(7, true)), FaultClass::UserProtection);
        assert_eq!(classify(mk(8, true)), FaultClass::ReservedBit);
        assert_eq!(classify(mk(4, false)), FaultClass::Malformed);
        assert_eq!(classify(mk(0, false)), FaultClass::KernelNotPresent);
        assert_eq!(classify(mk(1, false)), FaultClass::KernelProtection);
    }
}
