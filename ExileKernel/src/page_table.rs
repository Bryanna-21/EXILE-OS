//! Software leaf-entry encoders only; no page-table allocation or register writes.
#![no_std]
pub const PAGE_SIZE: u64 = 4096;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub user: bool,
}
impl Permissions {
    pub const KERNEL_RW: Self = Self {
        read: true,
        write: true,
        execute: false,
        user: false,
    };
    pub const KERNEL_RX: Self = Self {
        read: true,
        write: false,
        execute: true,
        user: false,
    };
    pub fn validate(self) -> Result<(), MapError> {
        if !self.read {
            return Err(MapError::NoAccess);
        }
        if self.write && self.execute {
            return Err(MapError::WriteExecute);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    Unaligned,
    WriteExecute,
    NoAccess,
    AddressTooWide,
    NxUnavailable,
}
fn check(p: u64, perm: Permissions) -> Result<(), MapError> {
    perm.validate()?;
    if p == 0 || p & 4095 != 0 {
        return Err(MapError::Unaligned);
    }
    Ok(())
}
pub fn x86_64_leaf(p: u64, perm: Permissions, nx: bool) -> Result<u64, MapError> {
    check(p, perm)?;
    if p >= 1 << 52 {
        return Err(MapError::AddressTooWide);
    }
    if !nx && !perm.execute {
        return Err(MapError::NxUnavailable);
    }
    let mut e = p | 1;
    if perm.write {
        e |= 2
    }
    if perm.user {
        e |= 4
    }
    if !perm.execute && nx {
        e |= 1 << 63
    }
    Ok(e)
}
pub fn aarch64_leaf(p: u64, perm: Permissions) -> Result<u64, MapError> {
    check(p, perm)?;
    if p >= 1 << 48 {
        return Err(MapError::AddressTooWide);
    }
    let mut e = p | 3 | (3 << 8) | (1 << 10);
    if perm.user {
        e |= 1 << 6
    }
    if !perm.write {
        e |= 1 << 7
    }
    if !perm.user {
        e |= 1 << 54
    }
    if !perm.execute {
        e |= (1 << 53) | (1 << 54)
    } else if perm.user {
        e |= 1 << 53
    }
    Ok(e)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodes_flags() {
        let e = x86_64_leaf(0x4000, Permissions::KERNEL_RW, true).unwrap();
        assert_ne!(e & 2, 0);
        assert_ne!(e >> 63, 0);
        let ro = Permissions {
            read: true,
            write: false,
            execute: false,
            user: false,
        };
        let arm = aarch64_leaf(0x8000, ro).unwrap();
        assert_eq!(arm & 3, 3);
        assert_ne!(arm & (1 << 10), 0);
        assert_ne!(arm & (1 << 7), 0);
        assert_ne!(arm & (1 << 54), 0);
    }
    #[test]
    fn rejects_invalid() {
        let wx = Permissions {
            execute: true,
            ..Permissions::KERNEL_RW
        };
        assert_eq!(x86_64_leaf(0x4000, wx, true), Err(MapError::WriteExecute));
        assert_eq!(
            x86_64_leaf(0x1001, Permissions::KERNEL_RW, true),
            Err(MapError::Unaligned)
        );
        assert_eq!(
            x86_64_leaf(0x1000, Permissions::KERNEL_RW, false),
            Err(MapError::NxUnavailable)
        );
    }
}
