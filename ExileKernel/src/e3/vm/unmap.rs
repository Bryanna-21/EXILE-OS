//! Unmap a 4 KiB leaf and reclaim empty tables only after local invalidation.
use super::{
    backend::PageTableMemory,
    entry::Entry,
    frame_source::FrameSource,
    geometry::{self, PageMode},
    space::{PageTableVm, UnmappedPage, VmError},
    table_store::TableToken,
    tlb::TlbInvalidator,
};

#[derive(Clone, Copy)]
struct Pruned {
    parent: u64,
    index: usize,
    old: u64,
    token: TableToken,
}

impl<F: FrameSource, B: PageTableMemory, T: TlbInvalidator, const N: usize, const M: usize>
    PageTableVm<F, B, T, N, M>
{
    pub fn unmap_4k(&mut self, v: u64) -> Result<UnmappedPage, VmError> {
        self.ensure_active()?;

        let slot = self.mapping_slot(v).ok_or(VmError::NotMapped)?;
        let rec = self.mappings[slot].ok_or(VmError::NotMapped)?;
        if rec.huge {
            return Err(VmError::Malformed);
        }

        let ix = geometry::indices(PageMode::FourLevel, v).map_err(|_| VmError::NonCanonical)?;
        let indices = [ix.pml4, ix.pdpt, ix.pd, ix.pt];
        let mut tables = [self.root.physical(); 4];

        for depth in 0..3 {
            let raw = self
                .backend
                .read_entry(tables[depth], indices[depth])
                .map_err(|_| VmError::Backend)?;
            let e = Entry::parse(raw, 4 - depth as u8).map_err(|_| VmError::Malformed)?;

            if e.huge_at_level(4 - depth as u8) {
                return Err(VmError::Malformed);
            }

            tables[depth + 1] = e.physical().map_err(|_| VmError::Malformed)?;

            if self.store.level_for_physical(tables[depth + 1]) != Some(3 - depth as u8) {
                return Err(VmError::Malformed);
            }
        }

        let leaf = self
            .backend
            .read_entry(tables[3], ix.pt)
            .map_err(|_| VmError::Backend)?;
        Entry::parse(leaf, 1).map_err(|_| VmError::Malformed)?;

        if leaf & super::geometry::PHYS_ADDR_MASK != rec.physical_address {
            return Err(VmError::Malformed);
        }

        self.backend
            .write_entry(tables[3], ix.pt, 0)
            .map_err(|_| VmError::Backend)?;

        if self.tlb.invalidate_page(v).is_err() {
            if self.backend.write_entry(tables[3], ix.pt, leaf).is_err() {
                self.poisoned = true;
            }
            return Err(VmError::TlbUncertain);
        }

        let mut pruned: [Option<Pruned>; 3] = [None; 3];
        let mut count = 0usize;

        for depth in (1..=3).rev() {
            let child = tables[depth];
            let mut empty = true;

            for i in 0..512 {
                match self.backend.read_entry(child, i) {
                    Ok(0) => {}
                    Ok(x) if x & 1 != 0 => {
                        empty = false;
                        break;
                    }
                    Ok(_) => {
                        if !self.recover_unmap(&pruned, count, tables[3], ix.pt, leaf) {
                            return Err(VmError::TlbUncertain);
                        }
                        return Err(VmError::Malformed);
                    }
                    Err(_) => {
                        if !self.recover_unmap(&pruned, count, tables[3], ix.pt, leaf) {
                            return Err(VmError::TlbUncertain);
                        }
                        return Err(VmError::Backend);
                    }
                }
            }

            if !empty {
                break;
            }

            let parent = tables[depth - 1];
            let index = indices[depth - 1];

            let old = match self.backend.read_entry(parent, index) {
                Ok(x) => x,
                Err(_) => {
                    if !self.recover_unmap(&pruned, count, tables[3], ix.pt, leaf) {
                        return Err(VmError::TlbUncertain);
                    }
                    return Err(VmError::Backend);
                }
            };

            let token = match self.store.token_for_physical(child) {
                Some(x) => x,
                None => {
                    if !self.recover_unmap(&pruned, count, tables[3], ix.pt, leaf) {
                        return Err(VmError::TlbUncertain);
                    }
                    return Err(VmError::Malformed);
                }
            };

            if self.backend.write_entry(parent, index, 0).is_err() {
                if !self.recover_unmap(&pruned, count, tables[3], ix.pt, leaf) {
                    return Err(VmError::TlbUncertain);
                }
                return Err(VmError::Backend);
            }

            pruned[count] = Some(Pruned {
                parent,
                index,
                old,
                token,
            });
            count += 1;
        }

        if count > 0 && self.tlb.invalidate_all().is_err() {
            if !self.recover_unmap(&pruned, count, tables[3], ix.pt, leaf) {
                return Err(VmError::TlbUncertain);
            }
            return Err(VmError::TlbUncertain);
        }

        // The hardware mapping is now detached and translations invalidated.
        // Commit the ledger before reclaiming detached page-table frames.
        let _ = self.forget(slot);

        for item in pruned[..count].iter().flatten() {
            match self.store.release(&mut self.backend, item.token) {
                Ok(true) => {}
                Ok(false) | Err(_) => return Err(VmError::Table),
            }
        }

        Ok(UnmappedPage {
            virtual_address: v,
            physical_address: rec.physical_address,
            permissions: rec.permissions,
            ownership: rec.ownership,
        })
    }

    fn recover_unmap(
        &mut self,
        pruned: &[Option<Pruned>; 3],
        count: usize,
        leaf_table: u64,
        leaf_index: usize,
        leaf_raw: u64,
    ) -> bool {
        let mut restored = true;

        // Restore parent links from the highest level downward.
        for item in pruned[..count].iter().rev().flatten() {
            if self
                .backend
                .write_entry(item.parent, item.index, item.old)
                .is_err()
            {
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

        // A full local flush is required because parent links may have changed.
        if self.tlb.invalidate_all().is_err() {
            restored = false;
        }

        if !restored {
            self.poisoned = true;
        }
        restored
    }
}
