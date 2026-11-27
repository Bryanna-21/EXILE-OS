//! Transactional 4 KiB mapping with rollback of newly allocated intermediate tables.
use super::{
    backend::PageTableMemory,
    entry::Entry,
    frame_source::FrameSource,
    geometry::{self, PAGE_SIZE, PageMode},
    layout::{self, AddressClass},
    space::{MapOwnership, MappingRecord, PageTableVm, VmError},
    table_store::TableToken,
    tlb::TlbInvalidator,
};
use crate::page_perms::Perms;
#[derive(Clone, Copy)]
struct Created {
    parent: u64,
    index: usize,
    token: TableToken,
}
impl<F: FrameSource, B: PageTableMemory, T: TlbInvalidator, const N: usize, const M: usize>
    PageTableVm<F, B, T, N, M>
{
    pub fn map_4k(
        &mut self,
        v: u64,
        p: u64,
        perms: Perms,
        ownership: MapOwnership,
    ) -> Result<(), VmError> {
        self.ensure_active()?;
        if v & (PAGE_SIZE - 1) != 0 || p & (PAGE_SIZE - 1) != 0 || p == 0 {
            return Err(VmError::Unaligned);
        }
        geometry::indices(PageMode::FourLevel, v).map_err(|_| VmError::NonCanonical)?;
        layout::validate_range(
            v,
            PAGE_SIZE,
            if perms.user {
                AddressClass::User
            } else {
                AddressClass::Kernel
            },
        )
        .map_err(|_| VmError::OutsideLayout)?;
        let end = v.checked_add(PAGE_SIZE).ok_or(VmError::NonCanonical)?;
        if self.guards.blocks(v, end) {
            return Err(VmError::Guarded);
        }
        if perms.validate().is_err() {
            return Err(VmError::Permission);
        }
        if !self.config.nx_supported && !perms.execute {
            return Err(VmError::UnsupportedFeature);
        }
        if p & !geometry::PHYS_ADDR_MASK != 0
            || p.checked_add(PAGE_SIZE)
                .is_none_or(|end| end > self.config.physical_limit())
        {
            return Err(VmError::PhysicalAddress);
        }
        if let MapOwnership::Owned(l) = ownership {
            if l.physical() != p
                || !self.store.validate_data_frame(l)
                || !self.data_frames.iter().any(|x| *x == Some(l))
            {
                return Err(VmError::InvalidOwnership);
            }
        }
        if self.mapping_slot(v).is_some() {
            return Err(VmError::Overlap);
        }
        let slot = self.free_mapping_slot()?;
        let ix = geometry::indices(PageMode::FourLevel, v).map_err(|_| VmError::NonCanonical)?;
        let indices = [ix.pml4, ix.pdpt, ix.pd];
        let mut table = self.root.physical();
        let mut created: [Option<Created>; 3] = [None; 3];
        let mut n = 0usize;
        for depth in 0..3 {
            let raw = match self.backend.read_entry(table, indices[depth]) {
                Ok(x) => x,
                Err(_) => return self.fail_after_rollback(&created, n, VmError::Backend),
            };
            if raw & 1 != 0 {
                let e = match Entry::parse(raw, 4 - depth as u8) {
                    Ok(x) => x,
                    Err(_) => return self.fail_after_rollback(&created, n, VmError::Malformed),
                };
                if e.huge_at_level(4 - depth as u8) {
                    return self.fail_after_rollback(&created, n, VmError::Overlap);
                }
                if (perms.user && !e.user())
                    || (perms.write && !e.writable())
                    || (perms.execute && !e.executable())
                {
                    return self.fail_after_rollback(&created, n, VmError::Permission);
                }
                table = match e.physical() {
                    Ok(x) => x,
                    Err(_) => return self.fail_after_rollback(&created, n, VmError::Malformed),
                };
                if !self.store.contains(table) {
                    return self.fail_after_rollback(&created, n, VmError::Malformed);
                }
            } else {
                if raw != 0 {
                    return self.fail_after_rollback(&created, n, VmError::Malformed);
                }
                let token = match self.store.allocate(&mut self.backend, 3 - depth as u8) {
                    Ok(t) => t,
                    Err(_) => return self.fail_after_rollback(&created, n, VmError::Capacity),
                };
                if !self.table_frame_valid(token) {
                    let _ = self.store.release(&mut self.backend, token);
                    return self.fail_after_rollback(&created, n, VmError::PhysicalAddress);
                };
                let entry = match Entry::table(token.physical(), perms.user) {
                    Ok(e) => e.raw(),
                    Err(_) => {
                        let _ = self.store.release(&mut self.backend, token);
                        return self.fail_after_rollback(&created, n, VmError::PhysicalAddress);
                    }
                };
                if self
                    .backend
                    .write_entry(table, indices[depth], entry)
                    .is_err()
                {
                    let _ = self.store.release(&mut self.backend, token);
                    return self.fail_after_rollback(&created, n, VmError::Backend);
                }
                created[n] = Some(Created {
                    parent: table,
                    index: indices[depth],
                    token,
                });
                n += 1;
                table = token.physical();
            }
        }
        let leaf = match Entry::leaf(p, perms, self.config.nx_supported) {
            Ok(e) => e.raw(),
            Err(_) => return self.fail_after_rollback(&created, n, VmError::Permission),
        };
        let old = match self.backend.read_entry(table, ix.pt) {
            Ok(x) => x,
            Err(_) => return self.fail_after_rollback(&created, n, VmError::Backend),
        };
        if old & 1 != 0 {
            return self.fail_after_rollback(&created, n, VmError::Overlap);
        }
        if old != 0 {
            return self.fail_after_rollback(&created, n, VmError::Malformed);
        }
        if self.backend.write_entry(table, ix.pt, leaf).is_err() {
            return self.fail_after_rollback(&created, n, VmError::Backend);
        }
        if self.tlb.invalidate_page(v).is_err() {
            if self.backend.write_entry(table, ix.pt, 0).is_err()
                || self.tlb.invalidate_page(v).is_err()
            {
                self.poisoned = true;
                return Err(VmError::TlbUncertain);
            }
            return self.fail_after_rollback(&created, n, VmError::TlbUncertain);
        }
        self.record(
            slot,
            MappingRecord {
                virtual_address: v,
                physical_address: p,
                permissions: perms,
                ownership,
                huge: false,
            },
        );
        Ok(())
    }
    pub fn map_contiguous_borrowed(
        &mut self,
        v: u64,
        p: u64,
        pages: usize,
        perms: Perms,
    ) -> Result<(), VmError> {
        if pages == 0 {
            return Err(VmError::Unaligned);
        }
        let bytes = (pages as u64)
            .checked_mul(PAGE_SIZE)
            .ok_or(VmError::PhysicalAddress)?;
        v.checked_add(bytes).ok_or(VmError::NonCanonical)?;
        p.checked_add(bytes).ok_or(VmError::PhysicalAddress)?;
        if pages > M - self.mapping_len {
            return Err(VmError::Capacity);
        }
        let mut done = 0usize;
        while done < pages {
            let va = v
                .checked_add(done as u64 * PAGE_SIZE)
                .ok_or(VmError::NonCanonical)?;
            let pa = p
                .checked_add(done as u64 * PAGE_SIZE)
                .ok_or(VmError::PhysicalAddress)?;
            if let Err(e) = self.map_4k(va, pa, perms, MapOwnership::Borrowed) {
                for i in (0..done).rev() {
                    let addr = v + i as u64 * PAGE_SIZE;
                    if self.unmap_4k(addr).is_err() {
                        return Err(VmError::TlbUncertain);
                    }
                }
                return Err(e);
            }
            done += 1;
        }
        Ok(())
    }
    fn fail_after_rollback<R>(
        &mut self,
        c: &[Option<Created>; 3],
        n: usize,
        error: VmError,
    ) -> Result<R, VmError> {
        if self.rollback(c, n) {
            Err(error)
        } else {
            self.poisoned = true;
            Err(VmError::TlbUncertain)
        }
    }
    fn rollback(&mut self, c: &[Option<Created>; 3], n: usize) -> bool {
        // First detach every newly created table. If any write fails, retain
        // every frame: some parent entries may still reference those tables.
        for item in c[..n].iter().rev().flatten() {
            if self
                .backend
                .write_entry(item.parent, item.index, 0)
                .is_err()
            {
                return false;
            }
        }

        // Do not reclaim detached tables until stale translations and paging
        // structure references have been invalidated on this CPU.
        if n > 0 && self.tlb.invalidate_all().is_err() {
            return false;
        }

        // A failed release leaves its ownership record in TableStore.
        for item in c[..n].iter().rev().flatten() {
            if self.store.release(&mut self.backend, item.token).is_err() {
                return false;
            }
        }
        true
    }
}
