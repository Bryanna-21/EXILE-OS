//! User address spaces and user-memory validation.
//!
//! An `AddressSpace` is the kernel's record of what a user space may touch. Every copy
//! validates the whole range first (overflow-safe, below `USER_TOP`, every byte inside a
//! region with the needed permission) and only then touches memory through a
//! `UserBacking`. On bare metal the backing is a fault-handled copy routine; in the hosted
//! tests it is a simulated memory. Nothing here switches page tables: that is the
//! scheduler's space hook (`Sched::set_space_hook`).

use crate::sched::{KERNEL_SPACE, SpaceId};
use crate::status;

/// First address of kernel space (canonical lower half ends here).
pub const USER_TOP: u64 = 0x0000_8000_0000_0000;
pub const PAGE: u64 = 4096;
pub const MAX_REGIONS: usize = 8;
pub const MAX_SPACES: usize = 8;
pub const MAX_COPY: usize = 1 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    Read,
    Write,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Perm {
    pub read: bool,
    pub write: bool,
}

impl Perm {
    pub const R: Perm = Perm {
        read: true,
        write: false,
    };
    pub const RW: Perm = Perm {
        read: true,
        write: true,
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserError {
    /// Bad mapping arguments.
    BadRange,
    /// Address range not fully accessible.
    Fault,
    /// The caller's space is not the space currently loaded.
    SpaceMismatch,
    /// No such space, or a kernel thread asked for user access.
    NoSpace,
    TooLong,
    Exists,
    Full,
}

impl UserError {
    pub fn status(self) -> i64 {
        match self {
            UserError::BadRange | UserError::TooLong => status::INVALID_ARGUMENT,
            UserError::Fault => status::FAULT,
            UserError::SpaceMismatch => status::INTERNAL,
            UserError::NoSpace => status::ACCESS_DENIED,
            UserError::Exists => status::BUSY,
            UserError::Full => status::QUOTA_EXCEEDED,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Region {
    start: u64,
    len: u64,
    perm: Perm,
}

impl Region {
    fn end(&self) -> u64 {
        self.start + self.len // validated at map time: cannot overflow
    }
    fn allows(&self, a: Access) -> bool {
        match a {
            Access::Read => self.perm.read,
            Access::Write => self.perm.write,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AddressSpace {
    id: SpaceId,
    regions: [Option<Region>; MAX_REGIONS],
}

impl AddressSpace {
    pub const fn new(id: SpaceId) -> Self {
        AddressSpace {
            id,
            regions: [None; MAX_REGIONS],
        }
    }
    pub fn id(&self) -> SpaceId {
        self.id
    }

    /// Record a mapping. Page-aligned, non-empty, above the null page, below `USER_TOP`,
    /// and disjoint from existing regions.
    pub fn map(&mut self, start: u64, len: u64, perm: Perm) -> Result<(), UserError> {
        if len == 0 || start % PAGE != 0 || len % PAGE != 0 || start < PAGE {
            return Err(UserError::BadRange);
        }
        let end = start.checked_add(len).ok_or(UserError::BadRange)?;
        if end > USER_TOP {
            return Err(UserError::BadRange);
        }
        for r in self.regions.iter().flatten() {
            if start < r.end() && r.start < end {
                return Err(UserError::BadRange);
            }
        }
        let slot = self
            .regions
            .iter_mut()
            .find(|r| r.is_none())
            .ok_or(UserError::Full)?;
        *slot = Some(Region { start, len, perm });
        Ok(())
    }

    fn find(&self, addr: u64) -> Option<&Region> {
        self.regions
            .iter()
            .flatten()
            .find(|r| addr >= r.start && addr < r.end())
    }

    /// Every byte of `[addr, addr+len)` must lie in a region that allows `access`.
    /// Adjacent regions may be spanned.
    pub fn check_range(&self, addr: u64, len: u64, access: Access) -> Result<(), UserError> {
        if len == 0 {
            return if addr <= USER_TOP {
                Ok(())
            } else {
                Err(UserError::Fault)
            };
        }
        let end = addr.checked_add(len).ok_or(UserError::Fault)?;
        if end > USER_TOP {
            return Err(UserError::Fault);
        }
        let mut cur = addr;
        while cur < end {
            let r = self.find(cur).ok_or(UserError::Fault)?;
            if !r.allows(access) {
                return Err(UserError::Fault);
            }
            cur = r.end();
        }
        Ok(())
    }
}

/// Access to the bytes of a user space. Contract: all-or-nothing; returns false on any
/// fault. Must only be called after `check_range` passed.
pub trait UserBacking: Sync {
    fn read(&self, space: SpaceId, addr: u64, out: &mut [u8]) -> bool;
    fn write(&self, space: SpaceId, addr: u64, data: &[u8]) -> bool;
}

pub struct Spaces {
    spaces: [Option<AddressSpace>; MAX_SPACES],
}

impl Default for Spaces {
    fn default() -> Self {
        Self::new()
    }
}

impl Spaces {
    pub const fn new() -> Self {
        Spaces {
            spaces: [None; MAX_SPACES],
        }
    }

    pub fn create(&mut self, id: SpaceId) -> Result<(), UserError> {
        if id == KERNEL_SPACE {
            return Err(UserError::BadRange);
        }
        if self.get(id).is_some() {
            return Err(UserError::Exists);
        }
        let slot = self
            .spaces
            .iter_mut()
            .find(|s| s.is_none())
            .ok_or(UserError::Full)?;
        *slot = Some(AddressSpace::new(id));
        Ok(())
    }

    pub fn destroy(&mut self, id: SpaceId) -> bool {
        for s in self.spaces.iter_mut() {
            if s.map(|a| a.id) == Some(id) {
                *s = None;
                return true;
            }
        }
        false
    }

    pub fn get(&self, id: SpaceId) -> Option<&AddressSpace> {
        self.spaces.iter().flatten().find(|s| s.id == id)
    }

    pub fn get_mut(&mut self, id: SpaceId) -> Option<&mut AddressSpace> {
        self.spaces.iter_mut().flatten().find(|s| s.id == id)
    }

    fn authorise(
        &self,
        caller: SpaceId,
        loaded: SpaceId,
        len: usize,
        addr: u64,
        access: Access,
    ) -> Result<(), UserError> {
        if caller == KERNEL_SPACE {
            return Err(UserError::NoSpace);
        }
        if caller != loaded {
            return Err(UserError::SpaceMismatch);
        }
        if len > MAX_COPY {
            return Err(UserError::TooLong);
        }
        let sp = self.get(caller).ok_or(UserError::NoSpace)?;
        sp.check_range(addr, len as u64, access)
    }

    pub fn copy_from_user(
        &self,
        backing: &dyn UserBacking,
        caller: SpaceId,
        loaded: SpaceId,
        addr: u64,
        out: &mut [u8],
    ) -> Result<(), UserError> {
        self.authorise(caller, loaded, out.len(), addr, Access::Read)?;
        if backing.read(caller, addr, out) {
            Ok(())
        } else {
            out.fill(0);
            Err(UserError::Fault)
        }
    }

    pub fn copy_to_user(
        &self,
        backing: &dyn UserBacking,
        caller: SpaceId,
        loaded: SpaceId,
        addr: u64,
        data: &[u8],
    ) -> Result<(), UserError> {
        self.authorise(caller, loaded, data.len(), addr, Access::Write)?;
        if backing.write(caller, addr, data) {
            Ok(())
        } else {
            Err(UserError::Fault)
        }
    }
}
