//! x86-64 4-level page tables: entry encoding, a mapper over an abstract physical memory,
//! and the permission rules this kernel enforces (no writable+executable mapping, a
//! dedicated user window that holds only user mappings and nothing else).
//!
//! The mapper never dereferences a raw address: it works through `PhysMem`, which the boot
//! code implements over the identity map and the tests implement over a simulated memory.
//! TLB maintenance is the caller's job (`invlpg` / CR3 reload live in `kernel/`).

use crate::is_canonical;

pub const PAGE_SIZE: u64 = 4096;
pub const HUGE_SIZE: u64 = 2 * 1024 * 1024;
pub const ENTRIES: usize = 512;

pub const PRESENT: u64 = 1 << 0;
pub const WRITE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const HUGE: u64 = 1 << 7;
pub const NX: u64 = 1 << 63;
/// Bits 51:12 of an entry.
pub const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
/// Physical addresses at or above this are refused (architectural maximum is 52 bits).
pub const PHYS_LIMIT: u64 = 1 << 52;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Perms {
    pub write: bool,
    pub exec: bool,
    pub user: bool,
}

impl Perms {
    pub const KERNEL_RO: Perms = Perms {
        write: false,
        exec: false,
        user: false,
    };
    pub const KERNEL_RW: Perms = Perms {
        write: true,
        exec: false,
        user: false,
    };
    pub const KERNEL_RX: Perms = Perms {
        write: false,
        exec: true,
        user: false,
    };
    pub const USER_RO: Perms = Perms {
        write: false,
        exec: false,
        user: true,
    };
    pub const USER_RW: Perms = Perms {
        write: true,
        exec: false,
        user: true,
    };
    pub const USER_RX: Perms = Perms {
        write: false,
        exec: true,
        user: true,
    };

    fn leaf_flags(self) -> u64 {
        PRESENT
            | if self.write { WRITE } else { 0 }
            | if self.user { USER } else { 0 }
            | if self.exec { 0 } else { NX }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapError {
    Misaligned,
    NonCanonical,
    PhysRange,
    /// Requested mapping is both writable and executable.
    WxViolation,
    /// A user mapping outside the user window, or a kernel mapping inside it.
    UserWindow,
    NoFrame,
    AlreadyMapped,
    /// A 4 KiB operation hit a 2 MiB mapping, or the reverse.
    HugeConflict,
    NotMapped,
    Overflow,
}

pub trait PhysMem {
    fn read_u64(&self, phys: u64) -> u64;
    fn write_u64(&mut self, phys: u64, value: u64);
    fn zero_frame(&mut self, phys: u64);
}

pub trait FrameSource {
    fn alloc_frame(&mut self) -> Option<u64>;
}

/// Half-open virtual range `[start, end)` reserved for user mappings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UserWindow {
    pub start: u64,
    pub end: u64,
}

impl UserWindow {
    pub fn contains(&self, virt: u64, len: u64) -> bool {
        match virt.checked_add(len) {
            Some(e) => virt >= self.start && e <= self.end,
            None => false,
        }
    }
    fn overlaps(&self, virt: u64, len: u64) -> bool {
        match virt.checked_add(len) {
            Some(e) => virt < self.end && e > self.start,
            None => true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Translation {
    pub phys: u64,
    pub size: u64,
    /// Effective permissions: write and user are the AND over all levels, exec is false if
    /// any level has NX.
    pub write: bool,
    pub exec: bool,
    pub user: bool,
}

pub struct Mapper<'a, M: PhysMem, A: FrameSource> {
    mem: &'a mut M,
    frames: &'a mut A,
    root: u64,
    window: UserWindow,
}

fn indexes(virt: u64) -> [usize; 4] {
    [
        ((virt >> 39) & 0x1FF) as usize,
        ((virt >> 30) & 0x1FF) as usize,
        ((virt >> 21) & 0x1FF) as usize,
        ((virt >> 12) & 0x1FF) as usize,
    ]
}

impl<'a, M: PhysMem, A: FrameSource> Mapper<'a, M, A> {
    /// Allocate and zero a fresh root table.
    pub fn new(mem: &'a mut M, frames: &'a mut A, window: UserWindow) -> Result<Self, MapError> {
        let root = frames.alloc_frame().ok_or(MapError::NoFrame)?;
        if root % PAGE_SIZE != 0 || root >= PHYS_LIMIT {
            return Err(MapError::PhysRange);
        }
        mem.zero_frame(root);
        Ok(Mapper {
            mem,
            frames,
            root,
            window,
        })
    }

    /// Continue editing an existing root.
    pub fn from_root(
        mem: &'a mut M,
        frames: &'a mut A,
        root: u64,
        window: UserWindow,
    ) -> Result<Self, MapError> {
        if root % PAGE_SIZE != 0 || root >= PHYS_LIMIT || root == 0 {
            return Err(MapError::PhysRange);
        }
        Ok(Mapper {
            mem,
            frames,
            root,
            window,
        })
    }

    pub fn root(&self) -> u64 {
        self.root
    }

    fn check_common(&self, virt: u64, phys: u64, size: u64, perms: Perms) -> Result<(), MapError> {
        if virt % size != 0 || phys % size != 0 {
            return Err(MapError::Misaligned);
        }
        if !is_canonical(virt) {
            return Err(MapError::NonCanonical);
        }
        match phys.checked_add(size) {
            Some(e) if e <= PHYS_LIMIT => {}
            _ => return Err(MapError::PhysRange),
        }
        if perms.write && perms.exec {
            return Err(MapError::WxViolation);
        }
        // A user mapping must lie wholly inside the window; a kernel mapping must not touch it.
        if perms.user {
            if !self.window.contains(virt, size) {
                return Err(MapError::UserWindow);
            }
        } else if self.window.overlaps(virt, size) {
            return Err(MapError::UserWindow);
        }
        Ok(())
    }

    /// Return the next-level table for `index`, creating it if `create`. `user` makes sure
    /// the entry carries the USER bit so ring 3 can walk through it (leaf bits still decide).
    fn descend(
        &mut self,
        table: u64,
        index: usize,
        user: bool,
        create: bool,
        level_is_pd: bool,
    ) -> Result<Option<u64>, MapError> {
        let slot = table + index as u64 * 8;
        let entry = self.mem.read_u64(slot);
        if entry & PRESENT != 0 {
            if entry & HUGE != 0 {
                // PDPT/PD entry that is a large page, not a table.
                let _ = level_is_pd;
                return Err(MapError::HugeConflict);
            }
            if user && entry & USER == 0 {
                self.mem.write_u64(slot, entry | USER);
            }
            return Ok(Some(entry & ADDR_MASK));
        }
        if !create {
            return Ok(None);
        }
        let frame = self.frames.alloc_frame().ok_or(MapError::NoFrame)?;
        if frame % PAGE_SIZE != 0 || frame >= PHYS_LIMIT {
            return Err(MapError::PhysRange);
        }
        self.mem.zero_frame(frame);
        self.mem
            .write_u64(slot, frame | PRESENT | WRITE | if user { USER } else { 0 });
        Ok(Some(frame))
    }

    pub fn map_4k(&mut self, virt: u64, phys: u64, perms: Perms) -> Result<(), MapError> {
        self.check_common(virt, phys, PAGE_SIZE, perms)?;
        let ix = indexes(virt);
        let pdpt = self
            .descend(self.root, ix[0], perms.user, true, false)?
            .ok_or(MapError::NoFrame)?;
        let pd = self
            .descend(pdpt, ix[1], perms.user, true, false)?
            .ok_or(MapError::NoFrame)?;
        let pt = self
            .descend(pd, ix[2], perms.user, true, true)?
            .ok_or(MapError::NoFrame)?;
        let slot = pt + ix[3] as u64 * 8;
        if self.mem.read_u64(slot) & PRESENT != 0 {
            return Err(MapError::AlreadyMapped);
        }
        self.mem.write_u64(slot, phys | perms.leaf_flags());
        Ok(())
    }

    pub fn map_2m(&mut self, virt: u64, phys: u64, perms: Perms) -> Result<(), MapError> {
        self.check_common(virt, phys, HUGE_SIZE, perms)?;
        let ix = indexes(virt);
        let pdpt = self
            .descend(self.root, ix[0], perms.user, true, false)?
            .ok_or(MapError::NoFrame)?;
        let pd = self
            .descend(pdpt, ix[1], perms.user, true, false)?
            .ok_or(MapError::NoFrame)?;
        let slot = pd + ix[2] as u64 * 8;
        let existing = self.mem.read_u64(slot);
        if existing & PRESENT != 0 {
            return Err(if existing & HUGE != 0 {
                MapError::AlreadyMapped
            } else {
                MapError::HugeConflict
            });
        }
        self.mem.write_u64(slot, phys | perms.leaf_flags() | HUGE);
        Ok(())
    }

    /// Map `len` bytes (a multiple of 4 KiB) page by page.
    pub fn map_range_4k(
        &mut self,
        virt: u64,
        phys: u64,
        len: u64,
        perms: Perms,
    ) -> Result<(), MapError> {
        if len % PAGE_SIZE != 0 {
            return Err(MapError::Misaligned);
        }
        virt.checked_add(len).ok_or(MapError::Overflow)?;
        phys.checked_add(len).ok_or(MapError::Overflow)?;
        let mut off = 0;
        while off < len {
            self.map_4k(virt + off, phys + off, perms)?;
            off += PAGE_SIZE;
        }
        Ok(())
    }

    /// Map with 2 MiB pages where both addresses are 2 MiB aligned and enough remains,
    /// falling back to 4 KiB pages for the ragged ends.
    pub fn map_range_best(
        &mut self,
        virt: u64,
        phys: u64,
        len: u64,
        perms: Perms,
    ) -> Result<(), MapError> {
        if len % PAGE_SIZE != 0 {
            return Err(MapError::Misaligned);
        }
        virt.checked_add(len).ok_or(MapError::Overflow)?;
        phys.checked_add(len).ok_or(MapError::Overflow)?;
        let mut off = 0;
        while off < len {
            let v = virt + off;
            let p = phys + off;
            if v % HUGE_SIZE == 0 && p % HUGE_SIZE == 0 && len - off >= HUGE_SIZE {
                self.map_2m(v, p, perms)?;
                off += HUGE_SIZE;
            } else {
                self.map_4k(v, p, perms)?;
                off += PAGE_SIZE;
            }
        }
        Ok(())
    }

    /// Remove a 4 KiB mapping and return the frame it pointed at. Does not free the frame
    /// and does not flush the TLB.
    pub fn unmap_4k(&mut self, virt: u64) -> Result<u64, MapError> {
        if virt % PAGE_SIZE != 0 {
            return Err(MapError::Misaligned);
        }
        if !is_canonical(virt) {
            return Err(MapError::NonCanonical);
        }
        let ix = indexes(virt);
        let pdpt = self
            .descend(self.root, ix[0], false, false, false)?
            .ok_or(MapError::NotMapped)?;
        let pd = self
            .descend(pdpt, ix[1], false, false, false)?
            .ok_or(MapError::NotMapped)?;
        let pt = self
            .descend(pd, ix[2], false, false, true)?
            .ok_or(MapError::NotMapped)?;
        let slot = pt + ix[3] as u64 * 8;
        let e = self.mem.read_u64(slot);
        if e & PRESENT == 0 {
            return Err(MapError::NotMapped);
        }
        self.mem.write_u64(slot, 0);
        Ok(e & ADDR_MASK)
    }

    pub fn translate(&self, virt: u64) -> Option<Translation> {
        translate(&*self.mem, self.root, virt)
    }
}

/// Software page walk: what would the MMU do for `virt` under `root`?
pub fn translate<M: PhysMem>(mem: &M, root: u64, virt: u64) -> Option<Translation> {
    if !is_canonical(virt) {
        return None;
    }
    let ix = indexes(virt);
    let mut table = root;
    let mut write = true;
    let mut user = true;
    let mut exec = true;
    for (level, &i) in ix.iter().enumerate() {
        let e = mem.read_u64(table + i as u64 * 8);
        if e & PRESENT == 0 {
            return None;
        }
        write &= e & WRITE != 0;
        user &= e & USER != 0;
        exec &= e & NX == 0;
        let is_leaf = level == 3 || (level == 2 && e & HUGE != 0);
        if level < 2 && e & HUGE != 0 {
            // 1 GiB pages are never created by this kernel; refuse to interpret them.
            return None;
        }
        if is_leaf {
            let size = if level == 3 { PAGE_SIZE } else { HUGE_SIZE };
            let base = e & ADDR_MASK & !(size - 1);
            return Some(Translation {
                phys: base + (virt & (size - 1)),
                size,
                write,
                exec,
                user,
            });
        }
        table = e & ADDR_MASK;
    }
    None
}
