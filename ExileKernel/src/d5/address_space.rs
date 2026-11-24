//! Per-process virtual address spaces on real x86-64 4-level page tables (Mission D5).
//! SPDX-License-Identifier: Apache-2.0
//!
//! An `AddressSpace` owns one PML4 and every lower-level table it allocates. User mappings are
//! built with genuine PTEs (P, RW, US, NX; 4 KiB pages only) written into physical memory
//! through the `PhysMem` trait, with frames obtained from the `FrameAlloc` trait. Nothing here
//! touches hardware except the cfg-gated CR3 helpers at the bottom of the "activation" section;
//! the host tests run the same code over an arena-backed fake.
//!
//! # Layout and separation
//! * User range: `USER_START (0x1000) .. USER_END (0x0000_7FFF_FFFF_F000)`, exclusive. Page 0 and
//!   the top page of the canonical lower half are never mappable.
//! * PML4 entries 256..512 (the kernel half) are COPIED from a caller-supplied kernel template
//!   when the space is created and are never modified, walked for freeing, or freed afterwards.
//!   User mappings only ever touch PML4 entries 0..256.
//! * Every user leaf has US=1. Intermediate entries are P|RW|US; permissions are decided at the
//!   leaf (RW, NX). W^X is enforced: a region can never be writable and executable.
//!
//! # Ownership
//! * `map_anon` allocates zeroed frames; the space OWNS them and frees them on unmap/destroy.
//! * `map_frames` maps caller-owned frames; they are never freed by the space.
//! * Tables are freed when they become empty after an unmap, and all remaining tables (and the
//!   PML4) on `destroy`. `destroy` consumes the space; dropping one without `destroy` leaks.
//! * All failing operations roll back to the exact prior state (mappings, regions, tables).
//!
//! # Limitations
//! * Whole-region unmap/protect only (no partial unmap); 4 KiB pages only; no PCID, no huge pages.
//! * Single CPU: no TLB shootdown. After unmap/protect the CR3 is reloaded only if this space is
//!   the current one (`target_os = "none"` builds).
//! * `activate` is only sound once the kernel itself is mapped in the PML4 upper half. The boot
//!   tables in `entry.S` identity-map the first GiB in the LOWER half, so they cannot be used as
//!   the kernel template and this module must not be activated until the kernel is relocated or
//!   mapped high. Until then only host/model use and `cr3_value()` are meaningful.

use core::cmp::Ordering;

pub const PAGE: u64 = 4096;
pub const USER_START: u64 = PAGE;
pub const USER_END: u64 = 0x0000_7FFF_FFFF_F000;
pub const ENTRIES: usize = 512;
pub const KERNEL_HALF_FIRST: usize = 256;
pub const MAX_REGIONS: usize = 64;
/// Physical addresses must fit the 52-bit architectural maximum.
pub const PHYS_LIMIT: u64 = 1 << 52;

const P: u64 = 1;
const RW: u64 = 1 << 1;
const US: u64 = 1 << 2;
const PS: u64 = 1 << 7;
const NX: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    NotAllocated,
    Misaligned,
    OutOfRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Misaligned,
    ZeroLength,
    OutOfUserRange,
    Overlap,
    /// Write+execute requested.
    Wx,
    /// Not readable (x86 cannot express write-only or exec-only without read).
    BadPerms,
    /// User regions must be user-accessible.
    NotUser,
    NoRegions,
    NoFrames,
    BadFrame,
    NotMapped,
    /// Mapped, but the page's permissions forbid the access.
    Perm,
    NoSuchRegion,
    /// Physical memory access failed.
    Phys,
    /// Page tables contradict the space's own bookkeeping (or a huge page was found).
    Corrupt,
    FreeFailed,
}

/// Physical memory access. All methods are bounds-checked by the implementation.
pub trait PhysMem {
    fn read_u64(&self, pa: u64) -> Result<u64, MemError>;
    fn write_u64(&mut self, pa: u64, value: u64) -> Result<(), MemError>;
    fn read_bytes(&self, pa: u64, out: &mut [u8]) -> Result<(), MemError>;
    fn write_bytes(&mut self, pa: u64, data: &[u8]) -> Result<(), MemError>;
    /// Zeroes the whole 4 KiB page at `pa` (page aligned).
    fn zero_page(&mut self, pa: u64) -> Result<(), MemError>;
}

/// Source of 4 KiB physical frames.
pub trait FrameAlloc {
    /// Returns a 4 KiB-aligned frame, or `None` when exhausted.
    fn alloc_frame(&mut self) -> Option<u64>;
    /// Returns a frame; double or foreign frees are errors.
    fn free_frame(&mut self, pa: u64) -> Result<(), FrameError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Perms {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
    pub user: bool,
}

impl Perms {
    pub const fn ro() -> Self {
        Self {
            read: true,
            write: false,
            exec: false,
            user: true,
        }
    }
    pub const fn rw() -> Self {
        Self {
            read: true,
            write: true,
            exec: false,
            user: true,
        }
    }
    pub const fn rx() -> Self {
        Self {
            read: true,
            write: false,
            exec: true,
            user: true,
        }
    }
    fn check(self) -> Result<(), Error> {
        if !self.read {
            return Err(Error::BadPerms);
        }
        if self.write && self.exec {
            return Err(Error::Wx);
        }
        if !self.user {
            return Err(Error::NotUser);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Exec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    /// Exclusive.
    pub end: u64,
    pub perms: Perms,
    /// The space owns (and frees) the mapped frames.
    pub owned: bool,
}

pub struct AddressSpace {
    pml4: u64,
    regions: [Option<Region>; MAX_REGIONS],
    mapped: u64,
    tables: u64,
}

const fn idx(va: u64, level: u32) -> u64 {
    (va >> (12 + 9 * (level as u64 - 1))) & 511
}

fn pte_for(pa: u64, perms: Perms) -> u64 {
    let mut e = pa | P | US;
    if perms.write {
        e |= RW;
    }
    if !perms.exec {
        e |= NX;
    }
    e
}

fn perms_of(e: u64) -> Perms {
    Perms {
        read: true,
        write: e & RW != 0,
        exec: e & NX == 0,
        user: e & US != 0,
    }
}

fn frame_ok(pa: u64) -> bool {
    pa != 0 && pa % PAGE == 0 && pa < PHYS_LIMIT
}

fn check_range(start: u64, len: u64) -> Result<u64, Error> {
    if len == 0 {
        return Err(Error::ZeroLength);
    }
    if start % PAGE != 0 || len % PAGE != 0 {
        return Err(Error::Misaligned);
    }
    let end = start.checked_add(len).ok_or(Error::OutOfUserRange)?;
    if start < USER_START || end > USER_END {
        return Err(Error::OutOfUserRange);
    }
    Ok(end)
}

impl AddressSpace {
    /// Creates a space whose lower half is empty and whose upper half (PML4 entries 256..512) is
    /// copied from the kernel template PML4 at `kernel_pml4`.
    pub fn new<M, A>(mem: &mut M, frames: &mut A, kernel_pml4: u64) -> Result<Self, Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        if !frame_ok(kernel_pml4) {
            return Err(Error::BadFrame);
        }
        let pml4 = alloc_table(mem, frames)?;
        for i in KERNEL_HALF_FIRST..ENTRIES {
            let copied = mem
                .read_u64(kernel_pml4 + 8 * i as u64)
                .and_then(|e| mem.write_u64(pml4 + 8 * i as u64, e));
            if copied.is_err() {
                let _ = frames.free_frame(pml4);
                return Err(Error::Phys);
            }
        }
        Ok(Self {
            pml4,
            regions: [None; MAX_REGIONS],
            mapped: 0,
            tables: 1,
        })
    }

    /// Physical address of the PML4 (the CR3 value, without PCID or flags).
    pub fn cr3_value(&self) -> u64 {
        self.pml4
    }
    /// User pages currently mapped.
    pub fn mapped_pages(&self) -> u64 {
        self.mapped
    }
    /// Page-table frames owned by this space, including the PML4.
    pub fn table_pages(&self) -> u64 {
        self.tables
    }
    pub fn region_count(&self) -> usize {
        self.regions.iter().flatten().count()
    }
    pub fn regions(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter().flatten()
    }
    pub fn region_at(&self, start: u64) -> Option<&Region> {
        self.regions().find(|r| r.start == start)
    }

    fn descend<M: PhysMem + ?Sized>(
        &self,
        mem: &M,
        va: u64,
        to_level: u32,
    ) -> Result<Option<u64>, Error> {
        let mut t = self.pml4;
        let mut l = 4;
        while l > to_level {
            let e = mem.read_u64(t + 8 * idx(va, l)).map_err(|_| Error::Phys)?;
            if e & P == 0 {
                return Ok(None);
            }
            if l <= 3 && e & PS != 0 {
                return Err(Error::Corrupt);
            }
            t = e & ADDR_MASK;
            l -= 1;
        }
        Ok(Some(t))
    }

    fn ensure_leaf_slot<M, A>(&mut self, mem: &mut M, frames: &mut A, va: u64) -> Result<u64, Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        let mut t = self.pml4;
        let mut l = 4;
        while l > 1 {
            let slot = t + 8 * idx(va, l);
            let e = mem.read_u64(slot).map_err(|_| Error::Phys)?;
            if e & P == 0 {
                if e != 0 {
                    return Err(Error::Corrupt);
                }
                let f = alloc_table(mem, frames)?;
                if mem.write_u64(slot, f | P | RW | US).is_err() {
                    let _ = frames.free_frame(f);
                    return Err(Error::Phys);
                }
                self.tables += 1;
                t = f;
            } else {
                if l <= 3 && e & PS != 0 {
                    return Err(Error::Corrupt);
                }
                t = e & ADDR_MASK;
            }
            l -= 1;
        }
        Ok(t + 8 * idx(va, 1))
    }

    fn table_is_empty<M: PhysMem + ?Sized>(mem: &M, t: u64) -> Result<bool, Error> {
        for i in 0..ENTRIES as u64 {
            if mem.read_u64(t + 8 * i).map_err(|_| Error::Phys)? != 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Frees now-empty tables covering `[start, end)`, bottom-up.
    fn prune<M, A>(
        &mut self,
        mem: &mut M,
        frames: &mut A,
        start: u64,
        end: u64,
    ) -> Result<(), Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        // `child` is the level of the table being considered for freeing (1=PT, 2=PD, 3=PDPT).
        for child in 1u32..=3 {
            let span = 1u64 << (12 + 9 * child as u64);
            let mut va = start & !(span - 1);
            while va < end {
                if let Some(parent) = self.descend(mem, va, child + 1)? {
                    let slot = parent + 8 * idx(va, child + 1);
                    let e = mem.read_u64(slot).map_err(|_| Error::Phys)?;
                    if e & P != 0 {
                        let t = e & ADDR_MASK;
                        if Self::table_is_empty(mem, t)? {
                            mem.write_u64(slot, 0).map_err(|_| Error::Phys)?;
                            frames.free_frame(t).map_err(|_| Error::FreeFailed)?;
                            self.tables -= 1;
                        }
                    }
                }
                va += span;
            }
        }
        Ok(())
    }

    fn clear_pages<M, A>(
        &mut self,
        mem: &mut M,
        frames: &mut A,
        start: u64,
        n: u64,
        owned: bool,
    ) -> Result<(), Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        let mut first = None;
        for i in 0..n {
            let va = start + i * PAGE;
            let Some(pt) = self.descend(mem, va, 1)? else {
                first.get_or_insert(Error::Corrupt);
                continue;
            };
            let slot = pt + 8 * idx(va, 1);
            let e = mem.read_u64(slot).map_err(|_| Error::Phys)?;
            if e & P == 0 {
                first.get_or_insert(Error::Corrupt);
                continue;
            }
            mem.write_u64(slot, 0).map_err(|_| Error::Phys)?;
            if owned && frames.free_frame(e & ADDR_MASK).is_err() {
                first.get_or_insert(Error::FreeFailed);
            }
        }
        match first {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn map_common<M, A>(
        &mut self,
        mem: &mut M,
        frames: &mut A,
        start: u64,
        len: u64,
        perms: Perms,
        given: Option<&[u64]>,
    ) -> Result<(), Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        let end = check_range(start, len)?;
        perms.check()?;
        if self
            .regions
            .iter()
            .flatten()
            .any(|r| start < r.end && r.start < end)
        {
            return Err(Error::Overlap);
        }
        let slot = self
            .regions
            .iter()
            .position(Option::is_none)
            .ok_or(Error::NoRegions)?;
        let n = len / PAGE;
        if let Some(g) = given {
            if g.len() as u64 != n || !g.iter().all(|&f| frame_ok(f)) {
                return Err(Error::BadFrame);
            }
        }
        let owned = given.is_none();
        let mut done = 0u64;
        let mut failure = None;
        while done < n {
            let va = start + done * PAGE;
            let step = self.install_page(mem, frames, va, perms, given.map(|g| g[done as usize]));
            match step {
                Ok(()) => done += 1,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = failure {
            let _ = self.clear_pages(mem, frames, start, done, owned);
            let _ = self.prune(mem, frames, start, end);
            return Err(e);
        }
        self.regions[slot] = Some(Region {
            start,
            end,
            perms,
            owned,
        });
        self.mapped += n;
        Ok(())
    }

    /// Installs one page; on error nothing for this page is left behind.
    fn install_page<M, A>(
        &mut self,
        mem: &mut M,
        frames: &mut A,
        va: u64,
        perms: Perms,
        given: Option<u64>,
    ) -> Result<(), Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        let (pa, fresh) = match given {
            Some(pa) => (pa, false),
            None => (alloc_table(mem, frames)?, true),
        };
        let result = (|| {
            let slot = self.ensure_leaf_slot(mem, frames, va)?;
            if mem.read_u64(slot).map_err(|_| Error::Phys)? != 0 {
                return Err(Error::Corrupt);
            }
            mem.write_u64(slot, pte_for(pa, perms))
                .map_err(|_| Error::Phys)
        })();
        if result.is_err() && fresh {
            let _ = frames.free_frame(pa);
        }
        result
    }

    /// Maps `len` bytes of fresh zeroed frames owned by this space.
    pub fn map_anon<M, A>(
        &mut self,
        mem: &mut M,
        frames: &mut A,
        start: u64,
        len: u64,
        perms: Perms,
    ) -> Result<(), Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        self.map_common(mem, frames, start, len, perms, None)
    }

    /// Maps caller-owned frames (one per page). They are never freed by this space.
    pub fn map_frames<M, A>(
        &mut self,
        mem: &mut M,
        frames: &mut A,
        start: u64,
        frame_list: &[u64],
        perms: Perms,
    ) -> Result<(), Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        let len = (frame_list.len() as u64)
            .checked_mul(PAGE)
            .ok_or(Error::OutOfUserRange)?;
        self.map_common(mem, frames, start, len, perms, Some(frame_list))
    }

    /// Unmaps the whole region starting exactly at `start`.
    pub fn unmap_region<M, A>(
        &mut self,
        mem: &mut M,
        frames: &mut A,
        start: u64,
    ) -> Result<(), Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        let i = self
            .regions
            .iter()
            .position(|r| r.is_some_and(|r| r.start == start))
            .ok_or(Error::NoSuchRegion)?;
        let r = self.regions[i].ok_or(Error::NoSuchRegion)?;
        let n = (r.end - r.start) / PAGE;
        let cleared = self.clear_pages(mem, frames, r.start, n, r.owned);
        // The region is gone from the bookkeeping even if a free failed: the PTEs are cleared.
        self.regions[i] = None;
        self.mapped -= n;
        let pruned = self.prune(mem, frames, r.start, r.end);
        self.flush_if_current();
        cleared.and(pruned)
    }

    /// Changes the permissions of the whole region starting at `start`.
    pub fn protect<M: PhysMem + ?Sized>(
        &mut self,
        mem: &mut M,
        start: u64,
        perms: Perms,
    ) -> Result<(), Error> {
        perms.check()?;
        let i = self
            .regions
            .iter()
            .position(|r| r.is_some_and(|r| r.start == start))
            .ok_or(Error::NoSuchRegion)?;
        let r = self.regions[i].ok_or(Error::NoSuchRegion)?;
        let n = (r.end - r.start) / PAGE;
        // First pass proves every PTE is present, so the second pass cannot fail halfway for
        // table reasons; a physical-memory fault in the second pass restores the old flags.
        for k in 0..n {
            let va = r.start + k * PAGE;
            let pt = self.descend(mem, va, 1)?.ok_or(Error::Corrupt)?;
            let e = mem.read_u64(pt + 8 * idx(va, 1)).map_err(|_| Error::Phys)?;
            if e & P == 0 {
                return Err(Error::Corrupt);
            }
        }
        for k in 0..n {
            let va = r.start + k * PAGE;
            let pt = self.descend(mem, va, 1)?.ok_or(Error::Corrupt)?;
            let slot = pt + 8 * idx(va, 1);
            let e = mem.read_u64(slot).map_err(|_| Error::Phys)?;
            if mem.write_u64(slot, pte_for(e & ADDR_MASK, perms)).is_err() {
                for j in 0..k {
                    let vj = r.start + j * PAGE;
                    if let Ok(Some(ptj)) = self.descend(mem, vj, 1) {
                        let sj = ptj + 8 * idx(vj, 1);
                        if let Ok(ej) = mem.read_u64(sj) {
                            let _ = mem.write_u64(sj, pte_for(ej & ADDR_MASK, r.perms));
                        }
                    }
                }
                return Err(Error::Phys);
            }
        }
        self.regions[i] = Some(Region { perms, ..r });
        self.flush_if_current();
        Ok(())
    }

    /// Software walk: physical address (with page offset) and permissions of `va`, if mapped.
    pub fn translate<M: PhysMem + ?Sized>(
        &self,
        mem: &M,
        va: u64,
    ) -> Result<Option<(u64, Perms)>, Error> {
        if !(USER_START..USER_END).contains(&va) {
            return Err(Error::OutOfUserRange);
        }
        let Some(pt) = self.descend(mem, va, 1)? else {
            return Ok(None);
        };
        let e = mem.read_u64(pt + 8 * idx(va, 1)).map_err(|_| Error::Phys)?;
        if e & P == 0 {
            return Ok(None);
        }
        Ok(Some(((e & ADDR_MASK) | (va & (PAGE - 1)), perms_of(e))))
    }

    fn page_pte<M: PhysMem + ?Sized>(
        &self,
        mem: &M,
        va: u64,
        access: Access,
    ) -> Result<u64, Error> {
        let pt = self.descend(mem, va, 1)?.ok_or(Error::NotMapped)?;
        let e = mem.read_u64(pt + 8 * idx(va, 1)).map_err(|_| Error::Phys)?;
        if e & P == 0 {
            return Err(Error::NotMapped);
        }
        if e & US == 0 {
            return Err(Error::Perm);
        }
        match access {
            Access::Read => {}
            Access::Write if e & RW == 0 => return Err(Error::Perm),
            Access::Exec if e & NX != 0 => return Err(Error::Perm),
            _ => {}
        }
        Ok(e)
    }

    /// Checks, against the page tables themselves, that every page of `[start, start+len)` is
    /// present, user-accessible and allows `access`. A zero length is trivially valid.
    pub fn validate_user_range<M: PhysMem + ?Sized>(
        &self,
        mem: &M,
        start: u64,
        len: u64,
        access: Access,
    ) -> Result<(), Error> {
        if len == 0 {
            return Ok(());
        }
        let end = start.checked_add(len).ok_or(Error::OutOfUserRange)?;
        if start < USER_START || end > USER_END {
            return Err(Error::OutOfUserRange);
        }
        let mut va = start & !(PAGE - 1);
        while va < end {
            self.page_pte(mem, va, access)?;
            va += PAGE;
        }
        Ok(())
    }

    /// Copies into user memory after validating write access for the whole range.
    pub fn copy_to_user<M: PhysMem + ?Sized>(
        &self,
        mem: &mut M,
        va: u64,
        data: &[u8],
    ) -> Result<(), Error> {
        self.validate_user_range(mem, va, data.len() as u64, Access::Write)?;
        self.copy_in(mem, va, data)
    }

    /// Kernel-privileged write for the image loader: the pages must be mapped and user-owned but
    /// need not be writable (text segments are written once, before the process runs).
    pub fn load_bytes<M: PhysMem + ?Sized>(
        &self,
        mem: &mut M,
        va: u64,
        data: &[u8],
    ) -> Result<(), Error> {
        self.validate_user_range(mem, va, data.len() as u64, Access::Read)?;
        self.copy_in(mem, va, data)
    }

    fn copy_in<M: PhysMem + ?Sized>(&self, mem: &mut M, va: u64, data: &[u8]) -> Result<(), Error> {
        let mut done = 0usize;
        while done < data.len() {
            let cur = va + done as u64;
            let e = self.page_pte(mem, cur & !(PAGE - 1), Access::Read)?;
            let off = cur & (PAGE - 1);
            let chunk = ((PAGE - off) as usize).min(data.len() - done);
            mem.write_bytes((e & ADDR_MASK) + off, &data[done..done + chunk])
                .map_err(|_| Error::Phys)?;
            done += chunk;
        }
        Ok(())
    }

    /// Copies out of user memory after validating read access for the whole range.
    pub fn copy_from_user<M: PhysMem + ?Sized>(
        &self,
        mem: &M,
        va: u64,
        out: &mut [u8],
    ) -> Result<(), Error> {
        self.validate_user_range(mem, va, out.len() as u64, Access::Read)?;
        let mut done = 0usize;
        while done < out.len() {
            let cur = va + done as u64;
            let e = self.page_pte(mem, cur & !(PAGE - 1), Access::Read)?;
            let off = cur & (PAGE - 1);
            let chunk = ((PAGE - off) as usize).min(out.len() - done);
            mem.read_bytes((e & ADDR_MASK) + off, &mut out[done..done + chunk])
                .map_err(|_| Error::Phys)?;
            done += chunk;
        }
        Ok(())
    }

    /// Frees every owned frame, every table this space allocated and the PML4. The kernel half
    /// is never touched. Continues after an individual failure and returns the first error.
    pub fn destroy<M, A>(mut self, mem: &mut M, frames: &mut A) -> Result<(), Error>
    where
        M: PhysMem + ?Sized,
        A: FrameAlloc + ?Sized,
    {
        let mut first = None;
        for i in 0..MAX_REGIONS {
            if let Some(r) = self.regions[i].take() {
                let n = (r.end - r.start) / PAGE;
                if let Err(e) = self.clear_pages(mem, frames, r.start, n, r.owned) {
                    first.get_or_insert(e);
                }
                self.mapped -= n;
            }
        }
        for i in 0..KERNEL_HALF_FIRST as u64 {
            match mem.read_u64(self.pml4 + 8 * i) {
                Ok(e) if e & P != 0 => {
                    if let Err(e) = free_subtree(mem, frames, e & ADDR_MASK, 3, &mut self.tables) {
                        first.get_or_insert(e);
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    first.get_or_insert(Error::Phys);
                }
            }
        }
        if frames.free_frame(self.pml4).is_err() {
            first.get_or_insert(Error::FreeFailed);
        }
        self.tables -= 1;
        match first {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    // ---- activation ------------------------------------------------------------------------

    /// Loads this space's PML4 into CR3.
    ///
    /// # Safety
    /// Ring 0 only. The currently executing code, its stack and all data touched until the next
    /// switch must be mapped in this space's kernel half (see the module limitations).
    #[cfg(all(target_arch = "x86_64", target_os = "none", not(test)))]
    pub unsafe fn activate(&self) {
        // SAFETY: forwarded to the caller.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) self.pml4, options(nostack, preserves_flags));
        }
    }

    #[cfg(all(target_arch = "x86_64", target_os = "none", not(test)))]
    fn flush_if_current(&self) {
        let cr3: u64;
        // SAFETY: reading CR3 and rewriting the same value only flushes non-global TLB entries.
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
            if cr3 & ADDR_MASK == self.pml4 {
                core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
            }
        }
    }

    #[cfg(not(all(target_arch = "x86_64", target_os = "none", not(test))))]
    fn flush_if_current(&self) {}
}

fn alloc_table<M, A>(mem: &mut M, frames: &mut A) -> Result<u64, Error>
where
    M: PhysMem + ?Sized,
    A: FrameAlloc + ?Sized,
{
    let f = frames.alloc_frame().ok_or(Error::NoFrames)?;
    if !frame_ok(f) {
        let _ = frames.free_frame(f);
        return Err(Error::BadFrame);
    }
    if mem.zero_page(f).is_err() {
        let _ = frames.free_frame(f);
        return Err(Error::Phys);
    }
    Ok(f)
}

fn free_subtree<M, A>(
    mem: &mut M,
    frames: &mut A,
    table: u64,
    level: u32,
    counter: &mut u64,
) -> Result<(), Error>
where
    M: PhysMem + ?Sized,
    A: FrameAlloc + ?Sized,
{
    let mut first = None;
    for i in 0..ENTRIES as u64 {
        let e = mem.read_u64(table + 8 * i).map_err(|_| Error::Phys)?;
        if e & P == 0 {
            continue;
        }
        if level > 1 {
            if e & PS != 0 {
                first.get_or_insert(Error::Corrupt);
            } else if let Err(err) = free_subtree(mem, frames, e & ADDR_MASK, level - 1, counter) {
                first.get_or_insert(err);
            }
        } else {
            // A leaf still present after every region was cleared: bookkeeping bug.
            first.get_or_insert(Error::Corrupt);
        }
    }
    if frames.free_frame(table).is_err() {
        first.get_or_insert(Error::FreeFailed);
    }
    *counter -= 1;
    match first {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ---- kernel-side physical memory ------------------------------------------------------------

/// `PhysMem` over the bootstrap identity map: physical address == virtual address inside
/// `[base, limit)`. RELIES on the identity mapping established by `entry.S`; every access is
/// bounds-checked against the window, and `read_u64`/`write_u64` require 8-byte alignment.
pub struct IdentityPhys {
    base: u64,
    limit: u64,
}

impl IdentityPhys {
    /// # Safety
    /// `[base, limit)` must be identity mapped, writable RAM that nothing else uses as ordinary
    /// Rust objects while page-table code runs.
    pub const unsafe fn new(base: u64, limit: u64) -> Self {
        Self { base, limit }
    }

    fn addr(&self, pa: u64, len: u64) -> Result<usize, MemError> {
        let end = pa.checked_add(len).ok_or(MemError)?;
        if pa < self.base || end > self.limit {
            return Err(MemError);
        }
        Ok(pa as usize)
    }
}

impl PhysMem for IdentityPhys {
    fn read_u64(&self, pa: u64) -> Result<u64, MemError> {
        if pa % 8 != 0 {
            return Err(MemError);
        }
        let a = self.addr(pa, 8)?;
        // SAFETY: inside the identity-mapped window and 8-byte aligned (checked above).
        Ok(unsafe { core::ptr::read_volatile(a as *const u64) })
    }
    fn write_u64(&mut self, pa: u64, value: u64) -> Result<(), MemError> {
        if pa % 8 != 0 {
            return Err(MemError);
        }
        let a = self.addr(pa, 8)?;
        // SAFETY: as in `read_u64`.
        unsafe { core::ptr::write_volatile(a as *mut u64, value) };
        Ok(())
    }
    fn read_bytes(&self, pa: u64, out: &mut [u8]) -> Result<(), MemError> {
        let a = self.addr(pa, out.len() as u64)?;
        // SAFETY: the source range lies inside the window; `out` is a distinct Rust slice.
        unsafe { core::ptr::copy_nonoverlapping(a as *const u8, out.as_mut_ptr(), out.len()) };
        Ok(())
    }
    fn write_bytes(&mut self, pa: u64, data: &[u8]) -> Result<(), MemError> {
        let a = self.addr(pa, data.len() as u64)?;
        // SAFETY: the destination range lies inside the window; `data` is a distinct slice.
        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), a as *mut u8, data.len()) };
        Ok(())
    }
    fn zero_page(&mut self, pa: u64) -> Result<(), MemError> {
        if pa % PAGE != 0 {
            return Err(MemError);
        }
        let a = self.addr(pa, PAGE)?;
        // SAFETY: the page lies inside the window.
        unsafe { core::ptr::write_bytes(a as *mut u8, 0, PAGE as usize) };
        Ok(())
    }
}

/// Deterministic ordering helper used by callers that keep regions sorted by start.
pub fn region_order(a: &Region, b: &Region) -> Ordering {
    a.start.cmp(&b.start)
}

#[cfg(test)]
pub(crate) mod fakes {
    //! Test doubles (tests only): an arena-backed `PhysMem` and a bitmap `FrameAlloc` with
    //! double-free detection and exhaustion injection.
    use super::*;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    pub const ARENA_BASE: u64 = 0x10_0000;

    pub struct TestMem {
        pub data: Vec<u8>,
    }
    impl TestMem {
        pub fn new(pages: usize) -> Self {
            Self {
                data: vec![0xAA; pages * PAGE as usize],
            }
        }
        fn off(&self, pa: u64, len: usize) -> Result<usize, MemError> {
            let o = pa.checked_sub(ARENA_BASE).ok_or(MemError)? as usize;
            if o.checked_add(len).ok_or(MemError)? > self.data.len() {
                return Err(MemError);
            }
            Ok(o)
        }
    }
    impl PhysMem for TestMem {
        fn read_u64(&self, pa: u64) -> Result<u64, MemError> {
            if pa % 8 != 0 {
                return Err(MemError);
            }
            let o = self.off(pa, 8)?;
            Ok(u64::from_le_bytes(self.data[o..o + 8].try_into().unwrap()))
        }
        fn write_u64(&mut self, pa: u64, v: u64) -> Result<(), MemError> {
            if pa % 8 != 0 {
                return Err(MemError);
            }
            let o = self.off(pa, 8)?;
            self.data[o..o + 8].copy_from_slice(&v.to_le_bytes());
            Ok(())
        }
        fn read_bytes(&self, pa: u64, out: &mut [u8]) -> Result<(), MemError> {
            let o = self.off(pa, out.len())?;
            out.copy_from_slice(&self.data[o..o + out.len()]);
            Ok(())
        }
        fn write_bytes(&mut self, pa: u64, d: &[u8]) -> Result<(), MemError> {
            let o = self.off(pa, d.len())?;
            self.data[o..o + d.len()].copy_from_slice(d);
            Ok(())
        }
        fn zero_page(&mut self, pa: u64) -> Result<(), MemError> {
            if pa % PAGE != 0 {
                return Err(MemError);
            }
            let o = self.off(pa, PAGE as usize)?;
            self.data[o..o + PAGE as usize].fill(0);
            Ok(())
        }
    }

    pub struct TestFrames {
        used: Vec<bool>,
        /// Fail the allocation whose 0-based call index equals this value.
        pub fail_at: Option<usize>,
        pub calls: usize,
    }
    impl TestFrames {
        pub fn new(pages: usize) -> Self {
            Self {
                used: vec![false; pages],
                fail_at: None,
                calls: 0,
            }
        }
        pub fn in_use(&self) -> usize {
            self.used.iter().filter(|u| **u).count()
        }
        pub fn is_used(&self, pa: u64) -> bool {
            self.used[((pa - ARENA_BASE) / PAGE) as usize]
        }
    }
    impl FrameAlloc for TestFrames {
        fn alloc_frame(&mut self) -> Option<u64> {
            let call = self.calls;
            self.calls += 1;
            if self.fail_at == Some(call) {
                return None;
            }
            let i = self.used.iter().position(|u| !*u)?;
            self.used[i] = true;
            Some(ARENA_BASE + i as u64 * PAGE)
        }
        fn free_frame(&mut self, pa: u64) -> Result<(), FrameError> {
            if pa % PAGE != 0 {
                return Err(FrameError::Misaligned);
            }
            let i = pa.checked_sub(ARENA_BASE).ok_or(FrameError::OutOfRange)? / PAGE;
            let i = i as usize;
            if i >= self.used.len() {
                return Err(FrameError::OutOfRange);
            }
            if !self.used[i] {
                return Err(FrameError::NotAllocated);
            }
            self.used[i] = false;
            Ok(())
        }
    }

    /// Memory + allocator + a kernel template PML4 whose upper half holds sentinels.
    pub struct Fx {
        pub mem: TestMem,
        pub frames: TestFrames,
        pub kernel: u64,
    }
    impl Fx {
        pub fn new(pages: usize) -> Self {
            let mut mem = TestMem::new(pages);
            let mut frames = TestFrames::new(pages);
            let kernel = frames.alloc_frame().unwrap();
            mem.zero_page(kernel).unwrap();
            for i in KERNEL_HALF_FIRST..ENTRIES {
                mem.write_u64(kernel + 8 * i as u64, 0x5000_0000 + (i as u64) * 0x1000 + 3)
                    .unwrap();
            }
            frames.calls = 0;
            Self {
                mem,
                frames,
                kernel,
            }
        }
        pub fn space(&mut self) -> AddressSpace {
            AddressSpace::new(&mut self.mem, &mut self.frames, self.kernel).unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    const BASE: u64 = 0x40_0000;

    #[test]
    fn new_copies_kernel_half_and_leaves_user_half_empty() {
        let mut fx = Fx::new(64);
        let s = fx.space();
        for i in 0..ENTRIES as u64 {
            let e = fx.mem.read_u64(s.cr3_value() + 8 * i).unwrap();
            if i >= KERNEL_HALF_FIRST as u64 {
                assert_eq!(e, 0x5000_0000 + i * 0x1000 + 3);
            } else {
                assert_eq!(e, 0);
            }
        }
        assert_eq!(s.table_pages(), 1);
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1, "only the kernel template remains");
    }

    #[test]
    fn map_translate_and_pte_bits() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, 2 * PAGE, Perms::rw())
            .unwrap();
        s.map_anon(
            &mut fx.mem,
            &mut fx.frames,
            BASE + 0x10000,
            PAGE,
            Perms::rx(),
        )
        .unwrap();
        assert_eq!(s.mapped_pages(), 3);
        let (pa, p) = s.translate(&fx.mem, BASE + 0x10).unwrap().unwrap();
        assert_eq!(pa & (PAGE - 1), 0x10);
        assert!(p.write && !p.exec && p.user);
        let (_, p) = s.translate(&fx.mem, BASE + 0x10000).unwrap().unwrap();
        assert!(!p.write && p.exec);
        assert_eq!(s.translate(&fx.mem, BASE + 0x2000).unwrap(), None);
        // Fresh frames were zeroed even though the arena started as 0xAA.
        let mut b = [1u8; 64];
        s.copy_from_user(&fx.mem, BASE, &mut b).unwrap();
        assert_eq!(b, [0u8; 64]);
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1);
    }

    #[test]
    fn unmap_frees_frames_and_empty_tables() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        let base = fx.frames.in_use();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, 4 * PAGE, Perms::rw())
            .unwrap();
        assert!(
            fx.frames.in_use() > base + 4,
            "pages plus three table levels"
        );
        assert_eq!(s.table_pages(), 4);
        s.unmap_region(&mut fx.mem, &mut fx.frames, BASE).unwrap();
        assert_eq!(fx.frames.in_use(), base);
        assert_eq!(s.table_pages(), 1);
        assert_eq!(s.mapped_pages(), 0);
        assert_eq!(s.translate(&fx.mem, BASE).unwrap(), None);
        assert_eq!(
            s.unmap_region(&mut fx.mem, &mut fx.frames, BASE),
            Err(Error::NoSuchRegion)
        );
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
    }

    #[test]
    fn tables_shared_with_another_region_survive_unmap() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, Perms::rw())
            .unwrap();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE + PAGE, PAGE, Perms::rw())
            .unwrap();
        s.unmap_region(&mut fx.mem, &mut fx.frames, BASE).unwrap();
        assert_eq!(s.table_pages(), 4, "the shared PT/PD/PDPT are still in use");
        s.copy_to_user(&mut fx.mem, BASE + PAGE, &[7]).unwrap();
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1);
    }

    #[test]
    fn permission_violations_are_reported() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, Perms::ro())
            .unwrap();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE + PAGE, PAGE, Perms::rw())
            .unwrap();
        assert_eq!(s.copy_to_user(&mut fx.mem, BASE, &[1]), Err(Error::Perm));
        assert_eq!(
            s.validate_user_range(&fx.mem, BASE, 1, Access::Exec),
            Err(Error::Perm)
        );
        assert_eq!(
            s.validate_user_range(&fx.mem, BASE + PAGE, 1, Access::Exec),
            Err(Error::Perm)
        );
        assert!(s.copy_to_user(&mut fx.mem, BASE + PAGE, &[1]).is_ok());
        // A range spanning a read-only page is refused as a whole and nothing is written.
        s.copy_to_user(&mut fx.mem, BASE + PAGE, &[9, 9]).unwrap();
        assert_eq!(
            s.copy_to_user(&mut fx.mem, BASE + PAGE - 1, &[5, 5]),
            Err(Error::Perm)
        );
        let mut b = [0u8; 2];
        s.copy_from_user(&fx.mem, BASE + PAGE, &mut b).unwrap();
        assert_eq!(b, [9, 9]);
        assert_eq!(
            s.copy_from_user(&fx.mem, BASE + 2 * PAGE, &mut b),
            Err(Error::NotMapped)
        );
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
    }

    #[test]
    fn wx_and_bad_permissions_are_rejected_without_side_effects() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        let before = fx.frames.in_use();
        let wx = Perms {
            read: true,
            write: true,
            exec: true,
            user: true,
        };
        assert_eq!(
            s.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, wx),
            Err(Error::Wx)
        );
        let kern = Perms {
            user: false,
            ..Perms::rw()
        };
        assert_eq!(
            s.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, kern),
            Err(Error::NotUser)
        );
        let noread = Perms {
            read: false,
            ..Perms::ro()
        };
        assert_eq!(
            s.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, noread),
            Err(Error::BadPerms)
        );
        assert_eq!(fx.frames.in_use(), before);
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, Perms::rw())
            .unwrap();
        assert_eq!(s.protect(&mut fx.mem, BASE, wx), Err(Error::Wx));
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
    }

    #[test]
    fn protect_changes_enforcement() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, 2 * PAGE, Perms::rw())
            .unwrap();
        s.copy_to_user(&mut fx.mem, BASE, &[1, 2, 3]).unwrap();
        s.protect(&mut fx.mem, BASE, Perms::ro()).unwrap();
        assert_eq!(s.copy_to_user(&mut fx.mem, BASE, &[4]), Err(Error::Perm));
        s.protect(&mut fx.mem, BASE, Perms::rx()).unwrap();
        s.validate_user_range(&fx.mem, BASE, 2 * PAGE, Access::Exec)
            .unwrap();
        assert_eq!(s.region_at(BASE).unwrap().perms, Perms::rx());
        let mut b = [0u8; 3];
        s.copy_from_user(&fx.mem, BASE, &mut b).unwrap();
        assert_eq!(b, [1, 2, 3], "contents survive protect");
        assert_eq!(
            s.protect(&mut fx.mem, BASE + PAGE, Perms::rw()),
            Err(Error::NoSuchRegion)
        );
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
    }

    #[test]
    fn overlap_is_detected_for_every_arrangement() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        s.map_anon(
            &mut fx.mem,
            &mut fx.frames,
            BASE + PAGE,
            2 * PAGE,
            Perms::rw(),
        )
        .unwrap();
        let before = fx.frames.in_use();
        for (start, len) in [
            (BASE + PAGE, PAGE),         // identical start
            (BASE, 2 * PAGE),            // overlaps the head
            (BASE + 2 * PAGE, 2 * PAGE), // overlaps the tail
            (BASE, 4 * PAGE),            // encloses
            (BASE + 2 * PAGE, PAGE),     // inside
        ] {
            assert_eq!(
                s.map_anon(&mut fx.mem, &mut fx.frames, start, len, Perms::rw()),
                Err(Error::Overlap),
                "{start:#x}+{len:#x}"
            );
        }
        // Adjacent mappings are fine.
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, Perms::rw())
            .unwrap();
        s.map_anon(
            &mut fx.mem,
            &mut fx.frames,
            BASE + 3 * PAGE,
            PAGE,
            Perms::rw(),
        )
        .unwrap();
        assert!(fx.frames.in_use() > before);
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1);
    }

    #[test]
    fn invalid_wrapping_and_kernel_half_ranges() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        let before = fx.frames.in_use();
        let rw = Perms::rw();
        let cases: [(u64, u64, Error); 9] = [
            (0, PAGE, Error::OutOfUserRange),
            (BASE, 0, Error::ZeroLength),
            (BASE + 1, PAGE, Error::Misaligned),
            (BASE, PAGE + 1, Error::Misaligned),
            (USER_END - PAGE, 2 * PAGE, Error::OutOfUserRange),
            (USER_END, PAGE, Error::OutOfUserRange),
            (0xFFFF_8000_0000_0000, PAGE, Error::OutOfUserRange),
            (0xFFFF_FFFF_FFFF_F000, 2 * PAGE, Error::OutOfUserRange),
            (BASE, u64::MAX & !(PAGE - 1), Error::OutOfUserRange),
        ];
        for (start, len, want) in cases {
            assert_eq!(
                s.map_anon(&mut fx.mem, &mut fx.frames, start, len, rw),
                Err(want),
                "{start:#x}+{len:#x}"
            );
        }
        assert_eq!(fx.frames.in_use(), before);
        // The last legal page.
        s.map_anon(&mut fx.mem, &mut fx.frames, USER_END - PAGE, PAGE, rw)
            .unwrap();
        // Validation of untrusted ranges.
        assert_eq!(
            s.validate_user_range(&fx.mem, u64::MAX - 3, 8, Access::Read),
            Err(Error::OutOfUserRange)
        );
        assert_eq!(
            s.validate_user_range(&fx.mem, 0, 1, Access::Read),
            Err(Error::OutOfUserRange)
        );
        assert_eq!(
            s.validate_user_range(&fx.mem, 0xFFFF_8000_0000_0000, 1, Access::Read),
            Err(Error::OutOfUserRange)
        );
        assert_eq!(
            s.translate(&fx.mem, 0xFFFF_8000_0000_0000),
            Err(Error::OutOfUserRange)
        );
        // The kernel half of the PML4 is unchanged by user activity.
        for i in KERNEL_HALF_FIRST as u64..ENTRIES as u64 {
            assert_eq!(
                fx.mem.read_u64(s.cr3_value() + 8 * i).unwrap(),
                0x5000_0000 + i * 0x1000 + 3
            );
        }
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
    }

    #[test]
    fn two_spaces_are_isolated_and_switchable() {
        let mut fx = Fx::new(128);
        let mut a = fx.space();
        let mut b = fx.space();
        assert_ne!(a.cr3_value(), b.cr3_value());
        a.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, Perms::rw())
            .unwrap();
        b.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, Perms::rw())
            .unwrap();
        a.copy_to_user(&mut fx.mem, BASE, b"AAAA").unwrap();
        b.copy_to_user(&mut fx.mem, BASE, b"BBBB").unwrap();
        let (pa_a, _) = a.translate(&fx.mem, BASE).unwrap().unwrap();
        let (pa_b, _) = b.translate(&fx.mem, BASE).unwrap().unwrap();
        assert_ne!(pa_a, pa_b);
        let mut buf = [0u8; 4];
        a.copy_from_user(&fx.mem, BASE, &mut buf).unwrap();
        assert_eq!(&buf, b"AAAA");
        b.copy_from_user(&fx.mem, BASE, &mut buf).unwrap();
        assert_eq!(&buf, b"BBBB");
        // A page mapped only in `a` is invisible to `b`.
        a.map_anon(
            &mut fx.mem,
            &mut fx.frames,
            BASE + 0x1000_0000,
            PAGE,
            Perms::ro(),
        )
        .unwrap();
        assert_eq!(b.translate(&fx.mem, BASE + 0x1000_0000).unwrap(), None);
        // Both share the identical kernel half.
        for i in KERNEL_HALF_FIRST as u64..ENTRIES as u64 {
            assert_eq!(
                fx.mem.read_u64(a.cr3_value() + 8 * i).unwrap(),
                fx.mem.read_u64(b.cr3_value() + 8 * i).unwrap()
            );
        }
        // Destroying one leaves the other intact.
        a.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        b.copy_from_user(&fx.mem, BASE, &mut buf).unwrap();
        assert_eq!(&buf, b"BBBB");
        b.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1);
    }

    #[test]
    fn copy_crosses_page_boundaries_and_matches_physical_memory() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, 3 * PAGE, Perms::rw())
            .unwrap();
        let data: Vec<u8> = (0..6000u32).map(|i| (i % 251) as u8).collect();
        let at = BASE + PAGE - 100;
        s.copy_to_user(&mut fx.mem, at, &data).unwrap();
        let mut back = vec![0u8; data.len()];
        s.copy_from_user(&fx.mem, at, &mut back).unwrap();
        assert_eq!(back, data);
        // Byte 100 of the copy is the first byte of the second page's frame.
        let (pa, _) = s.translate(&fx.mem, BASE + PAGE).unwrap().unwrap();
        let mut b = [0u8; 1];
        fx.mem.read_bytes(pa, &mut b).unwrap();
        assert_eq!(b[0], data[100]);
        // Running off the end of the region is refused before anything is written.
        assert_eq!(
            s.copy_to_user(&mut fx.mem, BASE + 3 * PAGE - 2, &[1, 2, 3]),
            Err(Error::NotMapped)
        );
        s.copy_from_user(&fx.mem, BASE + 3 * PAGE - 2, &mut [0u8; 2])
            .unwrap();
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
    }

    #[test]
    fn map_frames_never_frees_caller_frames() {
        let mut fx = Fx::new(64);
        let f1 = fx.frames.alloc_frame().unwrap();
        let f2 = fx.frames.alloc_frame().unwrap();
        fx.mem.write_bytes(f1, b"shared").unwrap();
        let mut s = fx.space();
        s.map_frames(&mut fx.mem, &mut fx.frames, BASE, &[f1, f2], Perms::ro())
            .unwrap();
        let mut b = [0u8; 6];
        s.copy_from_user(&fx.mem, BASE, &mut b).unwrap();
        assert_eq!(&b, b"shared");
        assert!(!s.region_at(BASE).unwrap().owned);
        s.unmap_region(&mut fx.mem, &mut fx.frames, BASE).unwrap();
        assert!(fx.frames.is_used(f1) && fx.frames.is_used(f2));
        // Mapped again, then destroyed: still owned by the caller.
        s.map_frames(&mut fx.mem, &mut fx.frames, BASE, &[f1], Perms::ro())
            .unwrap();
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert!(fx.frames.is_used(f1));
        // Bad frame lists are rejected.
        let mut s = fx.space();
        for bad in [&[0u64][..], &[f1 + 1][..], &[1u64 << 52][..], &[][..]] {
            assert!(
                s.map_frames(&mut fx.mem, &mut fx.frames, BASE, bad, Perms::ro())
                    .is_err()
            );
        }
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
    }

    #[test]
    fn exhaustion_at_every_allocation_point_rolls_back_exactly() {
        // Three pages straddling a 1 GiB boundary force new PDPT/PD/PT levels on both sides.
        let start = (1u64 << 30) - 2 * PAGE;
        let mut saw_err = 0;
        let mut saw_ok = false;
        for k in 0..40 {
            let mut fx = Fx::new(64);
            let mut s = fx.space();
            let base_frames = fx.frames.in_use();
            fx.frames.calls = 0;
            fx.frames.fail_at = Some(k);
            let r = s.map_anon(&mut fx.mem, &mut fx.frames, start, 4 * PAGE, Perms::rw());
            fx.frames.fail_at = None;
            match r {
                Err(Error::NoFrames) => {
                    saw_err += 1;
                    assert_eq!(fx.frames.in_use(), base_frames, "k={k}: frames leaked");
                    assert_eq!(s.mapped_pages(), 0);
                    assert_eq!(s.table_pages(), 1);
                    assert_eq!(s.region_count(), 0);
                    for i in 0..KERNEL_HALF_FIRST as u64 {
                        assert_eq!(fx.mem.read_u64(s.cr3_value() + 8 * i).unwrap(), 0);
                    }
                    // The space is still fully usable afterwards.
                    s.map_anon(&mut fx.mem, &mut fx.frames, start, 4 * PAGE, Perms::rw())
                        .unwrap();
                }
                Ok(()) => {
                    saw_ok = true;
                    assert_eq!(s.mapped_pages(), 4);
                }
                Err(e) => panic!("k={k}: unexpected {e:?}"),
            }
            s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
            assert_eq!(fx.frames.in_use(), 1, "k={k}: leak after destroy");
        }
        assert!(
            saw_err >= 8,
            "injection must hit table and page allocations"
        );
        assert!(saw_ok);
    }

    #[test]
    fn exhaustion_creating_a_space_leaves_nothing_behind() {
        let mut fx = Fx::new(8);
        fx.frames.calls = 0;
        fx.frames.fail_at = Some(0);
        assert!(matches!(
            AddressSpace::new(&mut fx.mem, &mut fx.frames, fx.kernel),
            Err(Error::NoFrames)
        ));
        fx.frames.fail_at = None;
        assert_eq!(fx.frames.in_use(), 1);
        assert!(matches!(
            AddressSpace::new(&mut fx.mem, &mut fx.frames, 0x123),
            Err(Error::BadFrame)
        ));
    }

    #[test]
    fn real_exhaustion_of_the_arena_is_clean() {
        let mut fx = Fx::new(12);
        let mut s = fx.space();
        let before = fx.frames.in_use();
        assert_eq!(
            s.map_anon(&mut fx.mem, &mut fx.frames, BASE, 64 * PAGE, Perms::rw()),
            Err(Error::NoFrames)
        );
        assert_eq!(fx.frames.in_use(), before);
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1);
    }

    #[test]
    fn region_table_exhaustion_leaks_nothing() {
        let mut fx = Fx::new(512);
        let mut s = fx.space();
        for i in 0..MAX_REGIONS as u64 {
            s.map_anon(
                &mut fx.mem,
                &mut fx.frames,
                BASE + i * 2 * PAGE,
                PAGE,
                Perms::ro(),
            )
            .unwrap();
        }
        let before = fx.frames.in_use();
        assert_eq!(
            s.map_anon(
                &mut fx.mem,
                &mut fx.frames,
                BASE + 0x100_0000,
                PAGE,
                Perms::ro()
            ),
            Err(Error::NoRegions)
        );
        assert_eq!(fx.frames.in_use(), before);
        // Freeing one makes room again.
        s.unmap_region(&mut fx.mem, &mut fx.frames, BASE).unwrap();
        s.map_anon(
            &mut fx.mem,
            &mut fx.frames,
            BASE + 0x100_0000,
            PAGE,
            Perms::ro(),
        )
        .unwrap();
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1);
    }

    #[test]
    fn destroy_returns_everything_and_spares_kernel_template() {
        let mut fx = Fx::new(256);
        let mut s = fx.space();
        for (i, a) in [
            BASE,
            0x4000_0000,
            0x80_0000_0000,
            0x7F00_0000_0000,
            USER_END - 8 * PAGE,
        ]
        .into_iter()
        .enumerate()
        {
            let perms = if i % 2 == 0 { Perms::rw() } else { Perms::rx() };
            s.map_anon(&mut fx.mem, &mut fx.frames, a, (i as u64 + 1) * PAGE, perms)
                .unwrap();
        }
        assert!(s.table_pages() > 10);
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1);
        assert!(fx.frames.is_used(fx.kernel));
        for i in KERNEL_HALF_FIRST as u64..ENTRIES as u64 {
            assert_eq!(
                fx.mem.read_u64(fx.kernel + 8 * i).unwrap(),
                0x5000_0000 + i * 0x1000 + 3
            );
        }
    }

    #[test]
    fn repeated_create_map_destroy_does_not_drift() {
        let mut fx = Fx::new(64);
        for i in 0..200u64 {
            let mut s = fx.space();
            s.map_anon(
                &mut fx.mem,
                &mut fx.frames,
                BASE + (i % 7) * PAGE,
                2 * PAGE,
                Perms::rw(),
            )
            .unwrap();
            s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
            assert_eq!(fx.frames.in_use(), 1);
        }
    }

    #[test]
    fn frame_allocator_fake_detects_double_free() {
        let mut f = TestFrames::new(4);
        let a = f.alloc_frame().unwrap();
        f.free_frame(a).unwrap();
        assert_eq!(f.free_frame(a), Err(FrameError::NotAllocated));
        assert_eq!(f.free_frame(a + 1), Err(FrameError::Misaligned));
        assert_eq!(f.free_frame(0), Err(FrameError::OutOfRange));
    }

    #[test]
    fn identity_phys_is_bounds_checked_and_usable_for_tables() {
        // A real host buffer addressed by its own address, i.e. an identity "map".
        let buf: &'static mut [u8] =
            std::boxed::Box::leak(vec![0u8; 8 * PAGE as usize].into_boxed_slice());
        let base = (buf.as_ptr() as u64 + PAGE - 1) & !(PAGE - 1);
        let limit = base + 4 * PAGE;
        // SAFETY: [base, limit) lies inside the leaked buffer, which nothing else touches.
        let mut m = unsafe { IdentityPhys::new(base, limit) };
        m.write_u64(base + 8, 0xDEAD_BEEF).unwrap();
        assert_eq!(m.read_u64(base + 8).unwrap(), 0xDEAD_BEEF);
        m.write_bytes(base + 100, b"xyz").unwrap();
        let mut b = [0u8; 3];
        m.read_bytes(base + 100, &mut b).unwrap();
        assert_eq!(&b, b"xyz");
        m.zero_page(base).unwrap();
        assert_eq!(m.read_u64(base + 8).unwrap(), 0);
        assert!(m.read_u64(base + 1).is_err(), "unaligned");
        assert!(m.read_u64(limit).is_err());
        assert!(m.read_u64(base - 8).is_err());
        assert!(m.write_bytes(limit - 1, &[0, 0]).is_err());
        assert!(m.write_bytes(u64::MAX - 1, &[0, 0, 0]).is_err());
        assert!(m.zero_page(base + 8).is_err());
    }

    #[test]
    fn huge_page_in_tables_is_reported_not_followed() {
        let mut fx = Fx::new(64);
        let mut s = fx.space();
        s.map_anon(&mut fx.mem, &mut fx.frames, BASE, PAGE, Perms::rw())
            .unwrap();
        // Corrupt the PD entry into a 2 MiB page.
        let pdpt = fx.mem.read_u64(s.cr3_value()).unwrap() & ADDR_MASK;
        let pd = fx.mem.read_u64(pdpt).unwrap() & ADDR_MASK;
        let slot = pd + 8 * idx(BASE, 2);
        let e = fx.mem.read_u64(slot).unwrap();
        fx.mem.write_u64(slot, e | PS).unwrap();
        assert_eq!(s.translate(&fx.mem, BASE), Err(Error::Corrupt));
        assert_eq!(
            s.validate_user_range(&fx.mem, BASE, 1, Access::Read),
            Err(Error::Corrupt)
        );
    }
}
