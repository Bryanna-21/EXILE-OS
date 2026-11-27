//! Borrowed 2 MiB mappings and lossless splitting into 512 borrowed 4 KiB leaves.
use super::{
    backend::PageTableMemory,
    entry::Entry,
    frame_source::FrameSource,
    geometry::{self, HUGE_2M, PAGE_SIZE, PageMode},
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
    pub fn map_2m_borrowed(&mut self, v: u64, p: u64, perms: Perms) -> Result<(), VmError> {
        self.ensure_active()?;
        if v & (HUGE_2M - 1) != 0 || p & (HUGE_2M - 1) != 0 || p == 0 {
            return Err(VmError::Unaligned);
        }
        let ix = geometry::indices(PageMode::FourLevel, v).map_err(|_| VmError::NonCanonical)?;
        layout::validate_range(
            v,
            HUGE_2M,
            if perms.user {
                AddressClass::User
            } else {
                AddressClass::Kernel
            },
        )
        .map_err(|_| VmError::OutsideLayout)?;
        let end = v.checked_add(HUGE_2M).ok_or(VmError::NonCanonical)?;
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
            || p.checked_add(HUGE_2M)
                .is_none_or(|end| end > self.config.physical_limit())
        {
            return Err(VmError::PhysicalAddress);
        }
        if self.mapping_slot(v).is_some() {
            return Err(VmError::Overlap);
        }
        let slot = self.free_mapping_slot()?;
        let mut table = self.root.physical();
        let indices = [ix.pml4, ix.pdpt];
        let mut made: [Option<Created>; 2] = [None; 2];
        let mut n = 0;
        for depth in 0..2 {
            let raw = match self.backend.read_entry(table, indices[depth]) {
                Ok(x) => x,
                Err(_) => return self.fail_huge(&made, n, VmError::Backend),
            };
            if raw & 1 != 0 {
                let e = match Entry::parse(raw, 4 - depth as u8) {
                    Ok(x) => x,
                    Err(_) => return self.fail_huge(&made, n, VmError::Malformed),
                };
                if e.huge_at_level(4 - depth as u8) {
                    return self.fail_huge(&made, n, VmError::Overlap);
                }
                if (perms.user && !e.user())
                    || (perms.write && !e.writable())
                    || (perms.execute && !e.executable())
                {
                    return self.fail_huge(&made, n, VmError::Permission);
                }
                table = e.physical().map_err(|_| VmError::Malformed)?;
                if !self.store.contains(table)
                    || self.store.level_for_physical(table) != Some(3 - depth as u8)
                {
                    return self.fail_huge(&made, n, VmError::Malformed);
                }
            } else {
                if raw != 0 {
                    return self.fail_huge(&made, n, VmError::Malformed);
                }
                let token = match self.store.allocate(&mut self.backend, 3 - depth as u8) {
                    Ok(t) => t,
                    Err(_) => return self.fail_huge(&made, n, VmError::Capacity),
                };
                if !self.table_frame_valid(token) {
                    let _ = self.store.release(&mut self.backend, token);
                    return self.fail_huge(&made, n, VmError::PhysicalAddress);
                };
                let raw = match Entry::table(token.physical(), perms.user) {
                    Ok(e) => e.raw(),
                    Err(_) => {
                        let _ = self.store.release(&mut self.backend, token);
                        return self.fail_huge(&made, n, VmError::PhysicalAddress);
                    }
                };
                if self
                    .backend
                    .write_entry(table, indices[depth], raw)
                    .is_err()
                {
                    let _ = self.store.release(&mut self.backend, token);
                    return self.fail_huge(&made, n, VmError::Backend);
                }
                made[n] = Some(Created {
                    parent: table,
                    index: indices[depth],
                    token,
                });
                n += 1;
                table = token.physical();
            }
        }
        let old = match self.backend.read_entry(table, ix.pd) {
            Ok(x) => x,
            Err(_) => return self.fail_huge(&made, n, VmError::Backend),
        };
        if old & 1 != 0 {
            return self.fail_huge(&made, n, VmError::Overlap);
        }
        if old != 0 {
            return self.fail_huge(&made, n, VmError::Malformed);
        }
        let leaf = match Entry::huge_2m(p, perms, self.config.nx_supported) {
            Ok(e) => e.raw(),
            Err(_) => return self.fail_huge(&made, n, VmError::Permission),
        };
        if self.backend.write_entry(table, ix.pd, leaf).is_err() {
            return self.fail_huge(&made, n, VmError::Backend);
        }
        if self.tlb.invalidate_page(v).is_err() {
            if self.backend.write_entry(table, ix.pd, 0).is_err()
                || self.tlb.invalidate_page(v).is_err()
            {
                return Err(VmError::TlbUncertain);
            }
            return self.fail_huge(&made, n, VmError::TlbUncertain);
        }
        self.record(
            slot,
            MappingRecord {
                virtual_address: v,
                physical_address: p,
                permissions: perms,
                ownership: MapOwnership::Borrowed,
                huge: true,
            },
        );
        Ok(())
    }
    fn fail_huge<R>(
        &mut self,
        m: &[Option<Created>; 2],
        n: usize,
        e: VmError,
    ) -> Result<R, VmError> {
        let mut ok = true;
        for x in m[..n].iter().rev().flatten() {
            if self.backend.write_entry(x.parent, x.index, 0).is_ok() {
                if self.store.release(&mut self.backend, x.token).is_err() {
                    ok = false
                }
            } else {
                ok = false
            }
        }
        if ok {
            Err(e)
        } else {
            Err(VmError::TlbUncertain)
        }
    }
    fn pd_for(&self, v: u64) -> Result<(u64, usize), VmError> {
        let ix = geometry::indices(PageMode::FourLevel, v).map_err(|_| VmError::NonCanonical)?;
        let e4 = Entry::parse(
            self.backend
                .read_entry(self.root.physical(), ix.pml4)
                .map_err(|_| VmError::Backend)?,
            4,
        )
        .map_err(|_| VmError::Malformed)?;
        let pdpt = e4.physical().map_err(|_| VmError::Malformed)?;
        let e3 = Entry::parse(
            self.backend
                .read_entry(pdpt, ix.pdpt)
                .map_err(|_| VmError::Backend)?,
            3,
        )
        .map_err(|_| VmError::Malformed)?;
        let pd = e3.physical().map_err(|_| VmError::Malformed)?;
        if self.store.level_for_physical(pd) != Some(2)
            || self.store.level_for_physical(pdpt) != Some(3)
        {
            return Err(VmError::Malformed);
        }
        Ok((pd, ix.pd))
    }
    pub fn split_2m(&mut self, v: u64) -> Result<(), VmError> {
        self.ensure_active()?;
        let slot = self.mapping_slot(v).ok_or(VmError::NotMapped)?;
        let rec = self.mappings[slot].ok_or(VmError::NotMapped)?;
        if !rec.huge || rec.ownership != MapOwnership::Borrowed {
            return Err(VmError::InvalidOwnership);
        }
        let start = self.empty_window(512, slot).ok_or(VmError::Capacity)?;
        let (pd, index) = self.pd_for(v)?;
        let old = self
            .backend
            .read_entry(pd, index)
            .map_err(|_| VmError::Backend)?;
        let huge = Entry::parse(old, 2).map_err(|_| VmError::Malformed)?;
        if !huge.huge_at_level(2)
            || huge.physical().map_err(|_| VmError::Malformed)? != rec.physical_address
            || huge.user() != rec.permissions.user
            || huge.writable() != rec.permissions.write
            || huge.executable() != rec.permissions.execute
        {
            return Err(VmError::Malformed);
        }
        let pt = self
            .store
            .allocate(&mut self.backend, 1)
            .map_err(|_| VmError::Capacity)?;
        if !self.table_frame_valid(pt) {
            let _ = self.store.release(&mut self.backend, pt);
            return Err(VmError::PhysicalAddress);
        };
        for i in 0..512 {
            let phys = match rec.physical_address.checked_add(i as u64 * PAGE_SIZE) {
                Some(x) => x,
                None => {
                    let _ = self.store.release(&mut self.backend, pt);
                    return Err(VmError::PhysicalAddress);
                }
            };
            let raw = match Entry::leaf(phys, rec.permissions, self.config.nx_supported) {
                Ok(e) => e.raw(),
                Err(_) => {
                    let _ = self.store.release(&mut self.backend, pt);
                    return Err(VmError::Permission);
                }
            };
            if self.backend.write_entry(pt.physical(), i, raw).is_err() {
                let _ = self.store.release(&mut self.backend, pt);
                return Err(VmError::Backend);
            }
        }
        let table_entry = match Entry::table(pt.physical(), rec.permissions.user) {
            Ok(e) => e.raw(),
            Err(_) => {
                let _ = self.store.release(&mut self.backend, pt);
                return Err(VmError::PhysicalAddress);
            }
        };
        if self.backend.write_entry(pd, index, table_entry).is_err() {
            let _ = self.store.release(&mut self.backend, pt);
            return Err(VmError::Backend);
        }
        if self.tlb.invalidate_all().is_err() {
            if self.backend.write_entry(pd, index, old).is_err()
                || self.tlb.invalidate_all().is_err()
            {
                return Err(VmError::TlbUncertain);
            }
            let _ = self.store.release(&mut self.backend, pt);
            return Err(VmError::TlbUncertain);
        }
        self.mappings[slot] = None;
        self.mapping_len -= 1;
        for i in 0..512 {
            self.mappings[start + i] = Some(MappingRecord {
                virtual_address: v + i as u64 * PAGE_SIZE,
                physical_address: rec.physical_address + i as u64 * PAGE_SIZE,
                permissions: rec.permissions,
                ownership: MapOwnership::Borrowed,
                huge: false,
            });
            self.mapping_len += 1;
        }
        Ok(())
    }
    pub fn unmap_2m(&mut self, v: u64) -> Result<MappingRecord, VmError> {
        self.ensure_active()?;

        let slot = self.mapping_slot(v).ok_or(VmError::NotMapped)?;
        let rec = self.mappings[slot].ok_or(VmError::NotMapped)?;
        if !rec.huge {
            return Err(VmError::Malformed);
        }

        let ix = geometry::indices(PageMode::FourLevel, v).map_err(|_| VmError::NonCanonical)?;
        let root = self.root.physical();

        let e4 = Entry::parse(
            self.backend
                .read_entry(root, ix.pml4)
                .map_err(|_| VmError::Backend)?,
            4,
        )
        .map_err(|_| VmError::Malformed)?;
        let pdpt = e4.physical().map_err(|_| VmError::Malformed)?;

        let e3 = Entry::parse(
            self.backend
                .read_entry(pdpt, ix.pdpt)
                .map_err(|_| VmError::Backend)?,
            3,
        )
        .map_err(|_| VmError::Malformed)?;
        let pd = e3.physical().map_err(|_| VmError::Malformed)?;

        let raw = self
            .backend
            .read_entry(pd, ix.pd)
            .map_err(|_| VmError::Backend)?;
        let e = Entry::parse(raw, 2).map_err(|_| VmError::Malformed)?;

        let physical = e.physical().map_err(|_| VmError::Malformed)?;
        if !e.huge_at_level(2) || physical & (HUGE_2M - 1) != 0 || physical != rec.physical_address
        {
            return Err(VmError::Malformed);
        }

        self.backend
            .write_entry(pd, ix.pd, 0)
            .map_err(|_| VmError::Backend)?;

        let mut pruned: [Option<(u64, usize, u64, TableToken)>; 2] = [None; 2];
        let mut count = 0usize;

        if self.tlb.invalidate_page(v).is_err() {
            if self.backend.write_entry(pd, ix.pd, raw).is_err() {
                self.poisoned = true;
            }
            return Err(VmError::TlbUncertain);
        }

        for (child, parent, index) in [(pd, pdpt, ix.pdpt), (pdpt, root, ix.pml4)] {
            let mut empty = true;

            for i in 0..512 {
                match self.backend.read_entry(child, i) {
                    Ok(0) => {}
                    Ok(x) if x & 1 != 0 => {
                        empty = false;
                        break;
                    }
                    Ok(_) => {
                        if !self.recover_huge_unmap(&pruned, count, pd, ix.pd, raw) {
                            return Err(VmError::TlbUncertain);
                        }
                        return Err(VmError::Malformed);
                    }
                    Err(_) => {
                        if !self.recover_huge_unmap(&pruned, count, pd, ix.pd, raw) {
                            return Err(VmError::TlbUncertain);
                        }
                        return Err(VmError::Backend);
                    }
                }
            }

            if !empty {
                break;
            }

            let old = match self.backend.read_entry(parent, index) {
                Ok(x) => x,
                Err(_) => {
                    if !self.recover_huge_unmap(&pruned, count, pd, ix.pd, raw) {
                        return Err(VmError::TlbUncertain);
                    }
                    return Err(VmError::Backend);
                }
            };

            let token = match self.store.token_for_physical(child) {
                Some(x) => x,
                None => {
                    if !self.recover_huge_unmap(&pruned, count, pd, ix.pd, raw) {
                        return Err(VmError::TlbUncertain);
                    }
                    return Err(VmError::Malformed);
                }
            };

            if self.backend.write_entry(parent, index, 0).is_err() {
                if !self.recover_huge_unmap(&pruned, count, pd, ix.pd, raw) {
                    return Err(VmError::TlbUncertain);
                }
                return Err(VmError::Backend);
            }

            pruned[count] = Some((parent, index, old, token));
            count += 1;
        }

        if count > 0 && self.tlb.invalidate_all().is_err() {
            if !self.recover_huge_unmap(&pruned, count, pd, ix.pd, raw) {
                return Err(VmError::TlbUncertain);
            }
            return Err(VmError::TlbUncertain);
        }

        // Commit the mapping removal before attempting frame reclamation.
        let _ = self.forget(slot);

        for item in pruned[..count].iter().flatten() {
            match self.store.release(&mut self.backend, item.3) {
                Ok(true) => {}
                Ok(false) | Err(_) => return Err(VmError::Table),
            }
        }

        Ok(rec)
    }

    fn recover_huge_unmap(
        &mut self,
        pruned: &[Option<(u64, usize, u64, TableToken)>; 2],
        count: usize,
        leaf_table: u64,
        leaf_index: usize,
        leaf_raw: u64,
    ) -> bool {
        let mut restored = true;

        // Restore parent links from the highest level downward.
        for item in pruned[..count].iter().rev().flatten() {
            if self.backend.write_entry(item.0, item.1, item.2).is_err() {
                restored = false;
            }
        }

        if self
            .backend
            .write_entry(leaf_table, leaf_index, leaf_raw)
            .is_err()
        {
            restored = false;
        }

        if self.tlb.invalidate_all().is_err() {
            restored = false;
        }

        if !restored {
            self.poisoned = true;
        }
        restored
    }
}
#[cfg(test)]
mod tests {
    use super::super::{backend::MockBackend, frame_source::MockFrames, tlb::MockTlb};
    use super::*;
    type V = PageTableVm<MockFrames<16>, MockBackend<16>, MockTlb, 16, 520>;
    const CFG: super::super::space::VmConfig = super::super::space::VmConfig::new(true, 52);
    fn rw() -> Perms {
        Perms {
            read: true,
            write: true,
            execute: false,
            user: true,
        }
    }
    #[test]
    fn huge_translation_split_preserves_offsets_and_permissions() {
        let mut v: V = PageTableVm::new(
            MockFrames::new(0x10_0000),
            MockBackend::new(),
            MockTlb::new(),
            CFG,
        )
        .unwrap();
        v.map_2m_borrowed(0x20_0000, 0x40_0000, rw()).unwrap();
        assert_eq!(v.translate(0x20_1234).unwrap().physical, 0x40_1234);
        assert!(v.translate(0x20_1234).unwrap().permissions.write);
        assert_eq!(v.audit().unwrap().tables, 3);
        v.split_2m(0x20_0000).unwrap();
        assert_eq!(v.mapping_count(), 512);
        assert_eq!(v.translate(0x20_1234).unwrap().physical, 0x40_1234);
        assert_eq!(v.audit().unwrap().leaves_4k, 512);
        let u = v.unmap_4k(0x20_1000).unwrap();
        assert_eq!(u.physical_address, 0x40_1000);
    }
    #[test]
    fn failed_huge_unmap_restores_the_original_mapping() {
        let mut v: V = PageTableVm::new(
            MockFrames::new(0x10_0000),
            MockBackend::new(),
            MockTlb::new(),
            CFG,
        )
        .unwrap();

        v.map_2m_borrowed(0x20_0000, 0x80_0000, rw()).unwrap();

        v.tlb.fail = true;
        assert_eq!(v.unmap_2m(0x20_0000), Err(VmError::TlbUncertain));

        let translated = v.translate(0x20_1234).unwrap();
        assert_eq!(translated.physical, 0x80_1234);
        assert_eq!(v.mapping_count(), 1);

        v.tlb.fail = false;
        let unmapped = v.unmap_2m(0x20_0000).unwrap();

        assert_eq!(unmapped.physical_address, 0x80_0000);
        assert_eq!(v.mapping_count(), 0);
        assert_eq!(v.audit().unwrap().tables, 1);
    }

    #[test]
    fn failed_huge_unmap_retains_detached_tables_for_teardown_retry() {
        let mut frames = MockFrames::<16>::new(0x10_0000);
        frames.fail_next_releases(1);

        let mut v: V = PageTableVm::new(frames, MockBackend::new(), MockTlb::new(), CFG).unwrap();

        v.map_2m_borrowed(0x20_0000, 0x80_0000, rw()).unwrap();

        assert_eq!(v.unmap_2m(0x20_0000), Err(VmError::Table));
        assert_eq!(v.mapping_count(), 0);

        // Detached table frames must remain owned until cleanup succeeds.
        assert_eq!(v.store.live(), 3);
        assert_eq!(v.audit(), Err(VmError::Malformed));

        // Teardown retries reclamation of every retained table.
        v.teardown().unwrap();
        assert_eq!(v.store.live(), 0);
        assert_eq!(v.teardown(), Err(VmError::AlreadyTornDown));
    }

    #[test]
    fn huge_unmap_and_guard_rejection() {
        let mut guarded: V = PageTableVm::new(
            MockFrames::new(0x10_0000),
            MockBackend::new(),
            MockTlb::new(),
            CFG,
        )
        .unwrap();
        guarded.reserve_guard(0x20_0000, 0x40_0000).unwrap();
        assert_eq!(
            guarded.map_2m_borrowed(0x20_0000, 0x80_0000, rw()),
            Err(VmError::Guarded)
        );
        let mut v: V = PageTableVm::new(
            MockFrames::new(0x20_0000),
            MockBackend::new(),
            MockTlb::new(),
            CFG,
        )
        .unwrap();
        v.map_2m_borrowed(0x20_0000, 0x80_0000, rw()).unwrap();
        let m = v.unmap_2m(0x20_0000).unwrap();
        assert_eq!(m.physical_address, 0x80_0000);
        assert_eq!(v.mapping_count(), 0);
        assert_eq!(v.audit().unwrap().tables, 1);
    }
}
