//! Validated x86-64 page-table entries; leaf encoding reuses page_perms.rs.
use super::geometry::{PAGE_SIZE, PHYS_ADDR_MASK};
use crate::page_perms::{self, PermError, Perms};
pub const PRESENT: u64 = 1;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const HUGE: u64 = 1 << 7;
pub const NX: u64 = 1 << 63;
const LOW_ALLOWED: u64 = 0x0000_0000_0000_0fff;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryError {
    NotPresent,
    ReservedBits,
    Misaligned,
    AddressWidth,
    InvalidFlags,
    Permission(PermError),
    HugeAtWrongLevel,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry(u64);
impl Entry {
    pub const fn raw(self) -> u64 {
        self.0
    }
    pub const fn present(self) -> bool {
        self.0 & PRESENT != 0
    }
    pub const fn user(self) -> bool {
        self.0 & USER != 0
    }
    pub const fn writable(self) -> bool {
        self.0 & WRITABLE != 0
    }
    pub const fn executable(self) -> bool {
        self.0 & NX == 0
    }
    /// True only for a supported 2 MiB leaf at page-directory level.
    /// Bit 7 at level 1 is PAT, not a huge-page indicator.
    pub const fn huge_at_level(self, level: u8) -> bool {
        level == 2 && self.0 & HUGE != 0
    }
    pub fn physical(self) -> Result<u64, EntryError> {
        if !self.present() {
            return Err(EntryError::NotPresent);
        }
        Ok(self.0 & PHYS_ADDR_MASK)
    }
    pub fn parse(raw: u64, level: u8) -> Result<Self, EntryError> {
        if !(1..=4).contains(&level) {
            return Err(EntryError::InvalidFlags);
        }
        if raw & PRESENT == 0 {
            return Err(EntryError::NotPresent);
        }
        if raw & !(PHYS_ADDR_MASK | LOW_ALLOWED | NX) != 0 {
            return Err(EntryError::ReservedBits);
        }
        if raw & HUGE != 0 && level != 1 && level != 2 {
            return Err(EntryError::HugeAtWrongLevel);
        }
        if (level == 1 || (level == 2 && raw & HUGE != 0)) && raw & WRITABLE != 0 && raw & NX == 0 {
            return Err(EntryError::InvalidFlags);
        }
        let p = raw & PHYS_ADDR_MASK;
        if p == 0 {
            return Err(EntryError::AddressWidth);
        }
        if p & (PAGE_SIZE - 1) != 0 {
            return Err(EntryError::Misaligned);
        }
        Ok(Self(raw))
    }
    pub fn leaf(phys: u64, perms: Perms, nx_supported: bool) -> Result<Self, EntryError> {
        if phys == 0 {
            return Err(EntryError::AddressWidth);
        }
        let raw =
            page_perms::x86_64_pte(phys, perms, nx_supported).map_err(EntryError::Permission)?;
        Self::parse(raw, 1)
    }
    pub fn table(phys: u64, user: bool) -> Result<Self, EntryError> {
        if phys == 0 || phys & 4095 != 0 {
            return Err(EntryError::Misaligned);
        }
        if phys & !PHYS_ADDR_MASK != 0 {
            return Err(EntryError::AddressWidth);
        }
        Ok(Self(
            phys | PRESENT | WRITABLE | if user { USER } else { 0 },
        ))
    }
    pub fn huge_2m(phys: u64, perms: Perms, nx_supported: bool) -> Result<Self, EntryError> {
        if phys & (super::geometry::HUGE_2M - 1) != 0 {
            return Err(EntryError::Misaligned);
        }
        let base = Self::leaf(phys, perms, nx_supported)?;
        Ok(Self(base.0 | HUGE))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_wx_and_bad_pa() {
        let wx = Perms {
            read: true,
            write: true,
            execute: true,
            user: false,
        };
        assert!(matches!(
            Entry::leaf(0x2000, wx, true),
            Err(EntryError::Permission(PermError::WriteExecute))
        ));
        assert_eq!(
            Entry::leaf(
                0x0010_0000_0000_0000,
                Perms {
                    read: true,
                    write: false,
                    execute: true,
                    user: false
                },
                true
            ),
            Err(EntryError::Permission(PermError::AddressOutOfRange))
        );
    }
    #[test]
    fn rejects_reserved_and_bad_huge() {
        assert_eq!(
            Entry::parse(1 | (1 << 52), 1),
            Err(EntryError::ReservedBits)
        );
        let pat_leaf = Entry::parse(0x2000 | 1 | HUGE, 1).unwrap();
        assert!(
            !pat_leaf.huge_at_level(1),
            "bit 7 is PAT, not a huge-page marker, in a 4 KiB leaf"
        );

        let huge_leaf = Entry::parse(0x20_0000 | 1 | HUGE, 2).unwrap();
        assert!(huge_leaf.huge_at_level(2));
        assert!(!huge_leaf.huge_at_level(1));
        assert_eq!(
            Entry::parse(0x2000 | 1 | HUGE, 4),
            Err(EntryError::HugeAtWrongLevel)
        );
        assert!(Entry::table(0x4000, true).unwrap().user());
    }
}
