//! 4 KiB leaf-entry encoding and W^X policy checks for x86-64 and AArch64.
//! This module does not allocate page tables or install mappings; callers must
//! provide an audited table allocator and architecture-specific TLB shootdown.
#![allow(dead_code)]
pub const PAGE_SIZE: u64 = 4096;
const X86_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
// 4 KiB AArch64 descriptors use bits 9:8 for shareability here.
// 52-bit PA requires a separately configured extended-address regime.
const AARCH64_ADDRESS_MASK: u64 = 0x0000_ffff_ffff_f000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Perms {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub user: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermError {
    Empty,
    ReadUnavailable,
    WriteExecute,
    Misaligned,
    AddressOutOfRange,
    NxUnavailable,
}

impl Perms {
    pub fn validate(self) -> Result<(), PermError> {
        if !self.read && !self.write && !self.execute {
            return Err(PermError::Empty);
        }
        if !self.read {
            return Err(PermError::ReadUnavailable);
        }
        if self.write && self.execute {
            return Err(PermError::WriteExecute);
        }
        Ok(())
    }
}

pub fn x86_64_pte(phys: u64, p: Perms, nx_supported: bool) -> Result<u64, PermError> {
    p.validate()?;
    if phys & (PAGE_SIZE - 1) != 0 {
        return Err(PermError::Misaligned);
    }
    if phys & !X86_ADDRESS_MASK != 0 {
        return Err(PermError::AddressOutOfRange);
    }
    if !p.execute && !nx_supported {
        return Err(PermError::NxUnavailable);
    }
    let mut e = (phys & X86_ADDRESS_MASK) | 1; // Present.
    if p.write {
        e |= 1 << 1;
    }
    if p.user {
        e |= 1 << 2;
    }
    if !p.execute {
        e |= 1 << 63;
    } // NX.
    Ok(e)
}

pub fn aarch64_page_desc(phys: u64, p: Perms) -> Result<u64, PermError> {
    p.validate()?;
    if phys & (PAGE_SIZE - 1) != 0 {
        return Err(PermError::Misaligned);
    }
    if phys & !AARCH64_ADDRESS_MASK != 0 {
        return Err(PermError::AddressOutOfRange);
    }
    // Valid + page descriptor, AF, inner-shareable, AttrIdx=0 (normal WB memory).
    let mut e = (phys & AARCH64_ADDRESS_MASK) | 0b11 | (1 << 10) | (0b11 << 8);
    if !p.write {
        e |= 1 << 7;
    } // AP[2]: EL1 read-only.
    if p.user {
        e |= 1 << 6;
    } // AP[1]: EL0 accessible.
    if !p.execute {
        e |= (1 << 53) | (1 << 54); // PXN + UXN.
    } else if p.user {
        e |= 1 << 53; // PXN: user code must not execute at EL1.
    } else {
        e |= 1 << 54; // UXN: kernel code is not user executable.
    }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_writable_executable() {
        assert_eq!(
            Perms {
                read: true,
                write: true,
                execute: true,
                user: false
            }
            .validate(),
            Err(PermError::WriteExecute)
        );
    }
    #[test]
    fn x86_nx_is_enforced_in_entry() {
        let p = Perms {
            read: true,
            write: false,
            execute: false,
            user: false,
        };
        assert_ne!(x86_64_pte(0x2000, p, true).unwrap() & (1 << 63), 0);
        assert_eq!(x86_64_pte(0x2000, p, false), Err(PermError::NxUnavailable));
    }
    #[test]
    fn arm_ro_and_xn_bits_are_set() {
        let p = Perms {
            read: true,
            write: false,
            execute: false,
            user: false,
        };
        let e = aarch64_page_desc(0x4000, p).unwrap();
        assert_ne!(e & (1 << 7), 0);
        assert_ne!(e & (1 << 53), 0);
        assert_ne!(e & (1 << 54), 0);
    }
    #[test]
    fn rejects_unaligned_and_out_of_range() {
        let p = Perms {
            read: true,
            write: false,
            execute: true,
            user: false,
        };
        assert_eq!(x86_64_pte(1, p, true), Err(PermError::Misaligned));
        assert_eq!(
            aarch64_page_desc(0x0010_0000_0000_0000, p),
            Err(PermError::AddressOutOfRange)
        );
    }

    #[test]
    fn rejects_execute_only_permissions() {
        let p = Perms {
            read: false,
            write: false,
            execute: true,
            user: false,
        };
        assert_eq!(p.validate(), Err(PermError::ReadUnavailable));
        assert_eq!(x86_64_pte(0x2000, p, true), Err(PermError::ReadUnavailable));
        assert_eq!(
            aarch64_page_desc(0x2000, p),
            Err(PermError::ReadUnavailable)
        );
    }

    #[test]
    fn user_executable_page_is_privileged_execute_never() {
        let p = Perms {
            read: true,
            write: false,
            execute: true,
            user: true,
        };
        let e = aarch64_page_desc(0x4000, p).unwrap();
        assert_eq!(e & (1 << 53), 1 << 53); // PXN
        assert_eq!(e & (1 << 54), 0); // EL0 execution remains allowed
    }

    #[test]
    fn kernel_executable_page_is_user_execute_never() {
        let p = Perms {
            read: true,
            write: false,
            execute: true,
            user: false,
        };
        let e = aarch64_page_desc(0x4000, p).unwrap();
        assert_eq!(e & (1 << 53), 0); // EL1 execution remains allowed
        assert_eq!(e & (1 << 54), 1 << 54); // UXN
    }

    #[test]
    fn aarch64_accepts_48_bit_page_address_boundary() {
        let p = Perms {
            read: true,
            write: false,
            execute: true,
            user: false,
        };
        assert!(aarch64_page_desc(0x0000_ffff_ffff_f000, p).is_ok());
        assert_eq!(
            aarch64_page_desc(0x0001_0000_0000_0000, p),
            Err(PermError::AddressOutOfRange)
        );
    }

    #[test]
    fn x86_retains_52_bit_physical_address_limit() {
        let p = Perms {
            read: true,
            write: false,
            execute: true,
            user: false,
        };
        assert!(x86_64_pte(0x000f_ffff_ffff_f000, p, true).is_ok());
        assert_eq!(
            x86_64_pte(0x0010_0000_0000_0000, p, true),
            Err(PermError::AddressOutOfRange)
        );
    }
}
