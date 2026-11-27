//! Checked four-level page-table walk with inherited permissions.
use super::{
    backend::PageTableMemory,
    entry::Entry,
    frame_source::FrameSource,
    geometry::{self, Indices, PageMode},
    space::{PageTableVm, VmError},
    tlb::TlbInvalidator,
};
use crate::page_perms::Perms;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkResult {
    pub table: u64,
    pub index: usize,
    pub entry: u64,
    pub leaf_level: u8,
    pub indices: Indices,
    pub effective_user: bool,
    pub effective_write: bool,
    pub effective_execute: bool,
}
impl<F: FrameSource, B: PageTableMemory, T: TlbInvalidator, const N: usize, const M: usize>
    PageTableVm<F, B, T, N, M>
{
    pub fn walk(&self, v: u64) -> Result<WalkResult, VmError> {
        self.ensure_active()?;
        let ix = geometry::indices(PageMode::FourLevel, v).map_err(|_| VmError::NonCanonical)?;
        let mut table = self.root.physical();
        let (mut user, mut write, mut execute) = (true, true, true);
        for (level, index) in [(4, ix.pml4), (3, ix.pdpt), (2, ix.pd), (1, ix.pt)] {
            let raw = self
                .backend
                .read_entry(table, index)
                .map_err(|_| VmError::Backend)?;
            if raw & 1 == 0 {
                return Err(if raw == 0 {
                    VmError::NotMapped
                } else {
                    VmError::Malformed
                });
            }
            let e = Entry::parse(raw, level).map_err(|_| VmError::Malformed)?;
            user &= e.user();
            write &= e.writable();
            execute &= e.executable();
            if level == 2
                && e.huge_at_level(level)
                && e.physical().map_err(|_| VmError::Malformed)? & (super::geometry::HUGE_2M - 1)
                    != 0
            {
                return Err(VmError::Malformed);
            }
            if level == 1 || (level == 2 && e.huge_at_level(level)) {
                return Ok(WalkResult {
                    table,
                    index,
                    entry: raw,
                    leaf_level: level,
                    indices: ix,
                    effective_user: user,
                    effective_write: write,
                    effective_execute: execute,
                });
            }
            if level == 2 && e.huge_at_level(level) {
                return Err(VmError::Malformed);
            }
            let child = e.physical().map_err(|_| VmError::Malformed)?;
            if !self.store.contains(child)
                || self.store.level_for_physical(child) != Some(level - 1)
            {
                return Err(VmError::Malformed);
            }
            table = child;
        }
        Err(VmError::Malformed)
    }
}
pub fn effective_permissions(w: WalkResult) -> Perms {
    Perms {
        read: true,
        write: w.effective_write,
        execute: w.effective_execute,
        user: w.effective_user,
    }
}
