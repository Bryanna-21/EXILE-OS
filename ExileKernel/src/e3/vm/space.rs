//! Address-space page-table root and explicit lifecycle.
use super::{
    backend::PageTableMemory,
    frame_source::{FrameLease, FrameSource},
    guard::GuardSet,
    table_store::{StoreError, TableStore, TableToken},
    tlb::TlbInvalidator,
};
use crate::page_perms::Perms;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmError {
    Inactive,
    Poisoned,
    AlreadyTornDown,
    Capacity,
    Unaligned,
    NonCanonical,
    OutsideLayout,
    Guarded,
    Overlap,
    NotMapped,
    Malformed,
    Permission,
    PhysicalAddress,
    Backend,
    Frame,
    Table,
    InvalidOwnership,
    TlbUncertain,
    MappingsRemain,
    FramesRemain,
    UnsupportedFeature,
}
/// CPU paging capabilities supplied by the boot-time feature detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmConfig {
    pub nx_supported: bool,
    pub physical_address_bits: u8,
}
impl VmConfig {
    pub const fn new(nx_supported: bool, physical_address_bits: u8) -> Self {
        Self {
            nx_supported,
            physical_address_bits,
        }
    }
    fn validate(self) -> Result<(), VmError> {
        if self.physical_address_bits < 32
            || self.physical_address_bits > super::geometry::PHYS_ADDR_BITS
        {
            Err(VmError::UnsupportedFeature)
        } else {
            Ok(())
        }
    }
    pub const fn physical_limit(self) -> u64 {
        1u64 << self.physical_address_bits
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapOwnership {
    Borrowed,
    Owned(FrameLease),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappingRecord {
    pub virtual_address: u64,
    pub physical_address: u64,
    pub permissions: Perms,
    pub ownership: MapOwnership,
    pub huge: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnmappedPage {
    pub virtual_address: u64,
    pub physical_address: u64,
    pub permissions: Perms,
    pub ownership: MapOwnership,
}
/// Owns one non-shared four-level root. The backend controls how physical table memory is accessed.
pub struct PageTableVm<
    F: FrameSource,
    B: PageTableMemory,
    T: TlbInvalidator,
    const TABLES: usize,
    const MAPS: usize,
> {
    pub(super) store: TableStore<F, TABLES>,
    pub(super) backend: B,
    pub(super) tlb: T,
    pub(super) root: TableToken,
    pub(super) config: VmConfig,
    pub(super) mappings: [Option<MappingRecord>; MAPS],
    pub(super) mapping_len: usize,
    pub(super) data_frames: [Option<FrameLease>; MAPS],
    pub(super) data_frame_len: usize,
    pub(super) guards: GuardSet<MAPS>,
    pub(super) active: bool,
    pub(super) poisoned: bool,
}

/// Owns all resources supplied to a failed address-space construction.
pub struct VmCreateError<F, B, T, const TABLES: usize> {
    error: VmError,
    store_error: Option<StoreError>,
    store: TableStore<F, TABLES>,
    backend: B,
    tlb: T,
}

impl<F, B, T, const TABLES: usize> core::fmt::Debug for VmCreateError<F, B, T, TABLES> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VmCreateError")
            .field("error", &self.error)
            .field("store_error", &self.store_error)
            .finish_non_exhaustive()
    }
}

impl<F, B, T, const TABLES: usize> VmCreateError<F, B, T, TABLES> {
    fn new(
        error: VmError,
        store_error: Option<StoreError>,
        store: TableStore<F, TABLES>,
        backend: B,
        tlb: T,
    ) -> Self {
        Self {
            error,
            store_error,
            store,
            backend,
            tlb,
        }
    }

    pub const fn kind(&self) -> VmError {
        self.error
    }

    pub const fn store_error(&self) -> Option<StoreError> {
        self.store_error
    }

    pub fn into_parts(self) -> (VmError, Option<StoreError>, TableStore<F, TABLES>, B, T) {
        (
            self.error,
            self.store_error,
            self.store,
            self.backend,
            self.tlb,
        )
    }
}

impl<F: FrameSource, B: PageTableMemory, T, const TABLES: usize> VmCreateError<F, B, T, TABLES> {
    /// Retry releasing every pending lease and live table record.
    pub fn retry_cleanup(&mut self) -> Result<(), VmError> {
        self.store
            .release_all(&mut self.backend)
            .map_err(|_| VmError::Frame)
    }

    pub fn pending_cleanup_count(&self) -> usize {
        self.store.pending_count()
    }

    pub fn live_table_count(&self) -> usize {
        self.store.live()
    }
}

impl<F: FrameSource, B: PageTableMemory, T: TlbInvalidator, const TABLES: usize, const MAPS: usize>
    PageTableVm<F, B, T, TABLES, MAPS>
{
    pub fn new(
        frames: F,
        mut backend: B,
        tlb: T,
        config: VmConfig,
    ) -> Result<Self, VmCreateError<F, B, T, TABLES>> {
        let mut store = TableStore::new(frames);

        if let Err(error) = config.validate() {
            return Err(VmCreateError::new(error, None, store, backend, tlb));
        }

        let root = match store.allocate(&mut backend, 4) {
            Ok(root) => root,
            Err(store_error) => {
                let error = match store_error {
                    StoreError::CleanupPending | StoreError::Frame(_) => VmError::Frame,
                    StoreError::Backend(_) => VmError::Backend,
                    _ => VmError::Table,
                };
                return Err(VmCreateError::new(
                    error,
                    Some(store_error),
                    store,
                    backend,
                    tlb,
                ));
            }
        };

        if root
            .physical()
            .checked_add(super::geometry::PAGE_SIZE)
            .is_none_or(|end| end > config.physical_limit())
        {
            // If release fails, the live root record remains in the returned store.
            let _ = store.release(&mut backend, root);
            return Err(VmCreateError::new(
                VmError::PhysicalAddress,
                None,
                store,
                backend,
                tlb,
            ));
        }

        Ok(Self {
            store,
            backend,
            tlb,
            root,
            config,
            mappings: [None; MAPS],
            mapping_len: 0,
            data_frames: [None; MAPS],
            data_frame_len: 0,
            guards: GuardSet::new(),
            active: true,
            poisoned: false,
        })
    }
    pub(super) fn ensure_active(&self) -> Result<(), VmError> {
        if self.poisoned {
            return Err(VmError::Poisoned);
        }
        if !self.active {
            return Err(VmError::Inactive);
        }
        Ok(())
    }

    pub fn retry_pending_cleanup(&mut self) -> Result<(), VmError> {
        self.store
            .retry_pending(&mut self.backend)
            .map_err(|_| VmError::Frame)
    }
    pub fn pending_cleanup_count(&self) -> usize {
        self.store.pending_count()
    }
    pub const fn root_physical(&self) -> u64 {
        self.root.physical()
    }
    pub const fn mapping_count(&self) -> usize {
        self.mapping_len
    }
    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn tlb_mut(&mut self) -> &mut T {
        &mut self.tlb
    }
    pub fn allocate_frame(&mut self) -> Result<FrameLease, VmError> {
        self.ensure_active()?;
        let slot = self
            .data_frames
            .iter()
            .position(Option::is_none)
            .ok_or(VmError::Capacity)?;
        let lease = self
            .store
            .allocate_data_frame()
            .map_err(|_| VmError::Frame)?;
        if lease
            .physical()
            .checked_add(super::geometry::PAGE_SIZE)
            .is_none_or(|end| end > self.config.physical_limit())
        {
            let _ = self.store.discard_data_frame(lease);
            return Err(VmError::PhysicalAddress);
        }
        self.data_frames[slot] = Some(lease);
        self.data_frame_len += 1;
        Ok(lease)
    }
    pub fn release_frame(&mut self, l: FrameLease) -> Result<(), VmError> {
        self.ensure_active()?;
        let slot = self
            .data_frames
            .iter()
            .position(|x| *x == Some(l))
            .ok_or(VmError::InvalidOwnership)?;
        if self.mappings.iter().flatten().any(|m| {
            let size = if m.huge {
                super::geometry::HUGE_2M
            } else {
                super::geometry::PAGE_SIZE
            };
            l.physical() >= m.physical_address && l.physical() < m.physical_address + size
        }) {
            return Err(VmError::InvalidOwnership);
        }
        self.store
            .release_data_frame(l)
            .map_err(|_| VmError::Frame)?;
        self.data_frames[slot] = None;
        self.data_frame_len -= 1;
        Ok(())
    }
    pub fn reserve_guard(&mut self, start: u64, end: u64) -> Result<(), VmError> {
        if !self.active {
            return Err(VmError::Inactive);
        }
        if start >= end
            || !super::geometry::is_canonical(start)
            || !super::geometry::is_canonical(end - 1)
        {
            return Err(VmError::NonCanonical);
        }
        if start < super::geometry::USER_CANONICAL_END && end > super::geometry::USER_CANONICAL_END
        {
            return Err(VmError::NonCanonical);
        }
        self.guards
            .reserve(start, end)
            .map_err(|_| VmError::Guarded)
    }
    pub fn mappings(&self) -> impl Iterator<Item = &MappingRecord> {
        self.mappings.iter().filter_map(Option::as_ref)
    }
    pub fn teardown(&mut self) -> Result<(), VmError> {
        if self.poisoned {
            return Err(VmError::Poisoned);
        }
        if !self.active {
            return Err(VmError::AlreadyTornDown);
        }
        if self.mapping_len != 0 {
            return Err(VmError::MappingsRemain);
        }
        if self.data_frame_len != 0 {
            return Err(VmError::FramesRemain);
        }
        self.tlb
            .invalidate_all()
            .map_err(|_| VmError::TlbUncertain)?;
        self.store
            .release_all(&mut self.backend)
            .map_err(|_| VmError::Table)?;
        self.active = false;
        Ok(())
    }
    pub(super) fn mapping_slot(&self, v: u64) -> Option<usize> {
        self.mappings
            .iter()
            .position(|x| x.is_some_and(|m| m.virtual_address == v))
    }
    pub(super) fn free_mapping_slot(&self) -> Result<usize, VmError> {
        self.mappings
            .iter()
            .position(Option::is_none)
            .ok_or(VmError::Capacity)
    }
    pub(super) fn empty_window(&self, count: usize, allow: usize) -> Option<usize> {
        if count > MAPS {
            return None;
        }
        (0..=MAPS - count)
            .find(|&start| (start..start + count).all(|i| self.mappings[i].is_none() || i == allow))
    }
    pub(super) fn table_frame_valid(&self, token: TableToken) -> bool {
        let physical = token.physical();

        physical != 0
            && physical & (super::geometry::PAGE_SIZE - 1) == 0
            && physical & !super::geometry::PHYS_ADDR_MASK == 0
            && physical
                .checked_add(super::geometry::PAGE_SIZE)
                .is_some_and(|end| end <= self.config.physical_limit())
            && self.store.token_for_physical(physical) == Some(token)
    }

    pub(super) fn record(&mut self, i: usize, m: MappingRecord) {
        self.mappings[i] = Some(m);
        self.mapping_len += 1;
    }
    pub(super) fn forget(&mut self, i: usize) -> Option<MappingRecord> {
        let m = self.mappings[i].take();
        if m.is_some() {
            self.mapping_len -= 1;
        }
        m
    }
    pub fn audit(&self) -> Result<super::audit::AuditReport, VmError> {
        super::audit::audit(self)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{backend::MockBackend, frame_source::MockFrames, tlb::MockTlb};
    use super::*;
    use crate::page_perms::Perms;
    type TestVm = PageTableVm<MockFrames<32>, MockBackend<32>, MockTlb, 32, 520>;
    const CFG: VmConfig = VmConfig::new(true, 52);
    fn new_vm() -> TestVm {
        PageTableVm::new(
            MockFrames::new(0x10_0000),
            MockBackend::new(),
            MockTlb::new(),
            CFG,
        )
        .unwrap()
    }
    fn rw() -> Perms {
        Perms {
            read: true,
            write: true,
            execute: false,
            user: true,
        }
    }
    #[test]
    fn maps_translates_and_unmaps_four_level_path() {
        let mut vm = new_vm();
        for i in 0..512 {
            assert_eq!(vm.backend.read_entry(vm.root_physical(), i).unwrap(), 0);
        }
        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();
        let t = vm.translate(0x4abc).unwrap();
        assert_eq!(t.physical, 0x9abc);
        assert!(t.permissions.write);
        assert!(!t.permissions.execute);
        assert_eq!(vm.audit().unwrap().tables, 4);
        let gone = vm.unmap_4k(0x4000).unwrap();
        assert_eq!(gone.physical_address, 0x9000);
        assert_eq!(vm.mapping_count(), 0);
        assert_eq!(vm.audit().unwrap().tables, 1);
        assert_eq!(
            vm.store.live(),
            1,
            "only root table remains; borrowed leaf frame was not released"
        );
    }
    #[test]
    fn failed_4k_unmap_retains_detached_tables_for_teardown_retry() {
        let mut frames = MockFrames::<32>::new(0x10_0000);
        frames.fail_next_releases(1);

        let mut vm: TestVm =
            PageTableVm::new(frames, MockBackend::new(), MockTlb::new(), CFG).unwrap();

        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();

        assert_eq!(vm.unmap_4k(0x4000), Err(VmError::Table));
        assert_eq!(vm.mapping_count(), 0);

        // Root plus three detached intermediate tables remain owned.
        assert_eq!(vm.store.live(), 4);
        assert_eq!(vm.audit(), Err(VmError::Malformed));

        // Teardown retries reclamation of every retained table.
        vm.teardown().unwrap();
        assert_eq!(vm.store.live(), 0);
        assert_eq!(vm.teardown(), Err(VmError::AlreadyTornDown));
    }

    #[test]
    fn user_permission_is_propagated_through_every_level() {
        let mut vm = new_vm();
        let p = Perms {
            read: true,
            write: false,
            execute: true,
            user: true,
        };
        vm.map_4k(0x40_0000, 0x80_0000, p, MapOwnership::Borrowed)
            .unwrap();
        let t = vm.translate(0x40_0123).unwrap();
        assert!(t.permissions.user);
        assert!(!t.permissions.write);
        assert!(t.permissions.execute);
    }
    #[test]
    fn guard_and_wx_rejections_leave_tables_unchanged() {
        let mut vm = new_vm();
        vm.reserve_guard(0x8000, 0x9000).unwrap();
        assert_eq!(
            vm.map_4k(0x8000, 0xa000, rw(), MapOwnership::Borrowed),
            Err(VmError::Guarded)
        );
        let wx = Perms {
            read: true,
            write: true,
            execute: true,
            user: false,
        };
        assert_eq!(
            vm.map_4k(0xffff_8000_0000_4000, 0xa000, wx, MapOwnership::Borrowed),
            Err(VmError::Permission)
        );
        assert_eq!(vm.store.live(), 1);
    }
    #[test]
    fn owned_leaf_frame_is_returned_not_implicitly_freed() {
        let mut vm = new_vm();
        let frame = vm.allocate_frame().unwrap();
        let phys = frame.physical();
        vm.map_4k(0x4000, phys, rw(), MapOwnership::Owned(frame))
            .unwrap();
        let out = vm.unmap_4k(0x4000).unwrap();
        assert_eq!(out.ownership, MapOwnership::Owned(frame));
        vm.release_frame(frame).unwrap();
    }
    #[test]
    fn backend_write_failure_rolls_back_intermediate_tables_and_can_retry() {
        let mut vm = new_vm();
        vm.backend.fail_after_writes(2);
        assert_eq!(
            vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed),
            Err(VmError::Backend)
        );
        assert_eq!(vm.store.live(), 1);
        assert_eq!(vm.backend.table_count(), 1);
        assert_eq!(vm.mapping_count(), 0);
        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();
        assert_eq!(vm.translate(0x4000).unwrap().physical, 0x9000);
    }
    #[test]
    fn table_pool_exhaustion_rolls_back_all_intermediate_allocations() {
        let mut vm: PageTableVm<MockFrames<8>, MockBackend<8>, MockTlb, 2, 4> = PageTableVm::new(
            MockFrames::new(0x20_0000),
            MockBackend::new(),
            MockTlb::new(),
            CFG,
        )
        .unwrap();
        assert_eq!(
            vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed),
            Err(VmError::Capacity)
        );
        assert_eq!(vm.store.live(), 1);
        assert_eq!(vm.backend.table_count(), 1);
        assert_eq!(vm.mapping_count(), 0);
    }
    #[test]
    fn maps_multiple_pages_and_rolls_back_partial_range() {
        let mut vm = new_vm();
        vm.map_contiguous_borrowed(0x20_000, 0x40_000, 3, rw())
            .unwrap();
        assert_eq!(vm.mapping_count(), 3);
        assert_eq!(vm.translate(0x22_123).unwrap().physical, 0x42_123);
        for v in [0x20_000, 0x21_000, 0x22_000] {
            vm.unmap_4k(v).unwrap();
        }
        assert_eq!(vm.mapping_count(), 0);
        let mut small: PageTableVm<MockFrames<16>, MockBackend<16>, MockTlb, 8, 1> =
            PageTableVm::new(
                MockFrames::new(0x30_0000),
                MockBackend::new(),
                MockTlb::new(),
                CFG,
            )
            .unwrap();
        assert_eq!(
            small.map_contiguous_borrowed(0x20_000, 0x40_000, 2, rw()),
            Err(VmError::Capacity)
        );
        assert_eq!(small.mapping_count(), 0);
    }
    #[test]
    fn inherited_parent_write_restriction_is_effective() {
        let mut vm = new_vm();
        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();
        let ix =
            crate::e3::vm::geometry::indices(crate::e3::vm::geometry::PageMode::FourLevel, 0x4000)
                .unwrap();
        let raw = vm.backend.read_entry(vm.root_physical(), ix.pml4).unwrap();
        vm.backend
            .corrupt(vm.root_physical(), ix.pml4, raw & !2)
            .unwrap();
        assert!(!vm.translate(0x4000).unwrap().permissions.write);
    }
    #[test]
    fn malformed_root_entry_is_detected_by_audit() {
        let mut vm = new_vm();
        vm.backend
            .corrupt(vm.root_physical(), 0, 1 | (1 << 52))
            .unwrap();
        assert_eq!(vm.audit(), Err(VmError::Malformed));
    }
    #[test]
    fn invalidation_is_recorded_after_mapping_and_unmapping() {
        let mut vm = new_vm();
        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();
        assert_eq!(vm.tlb.events[0], 0x4000);
        vm.unmap_4k(0x4000).unwrap();
        assert_eq!(vm.tlb.events[1], 0x4000);
    }
    #[test]
    fn failed_unmap_restores_the_original_mapping() {
        let mut vm = new_vm();
        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();
        vm.tlb.fail = true;
        assert_eq!(vm.unmap_4k(0x4000), Err(VmError::TlbUncertain));
        assert_eq!(vm.translate(0x4000).unwrap().physical, 0x9000);
        assert_eq!(vm.mapping_count(), 1);
        vm.tlb.fail = false;
        vm.unmap_4k(0x4000).unwrap();
    }
    #[test]
    fn teardown_requires_all_caller_owned_frames_returned() {
        let mut vm = new_vm();
        let frame = vm.allocate_frame().unwrap();
        assert_eq!(vm.teardown(), Err(VmError::FramesRemain));
        vm.release_frame(frame).unwrap();
        vm.teardown().unwrap();
    }
    #[test]
    fn teardown_tlb_failure_keeps_root_owned_for_retry() {
        let mut vm = new_vm();
        vm.tlb.fail = true;
        assert_eq!(vm.teardown(), Err(VmError::TlbUncertain));
        assert_eq!(vm.store.live(), 1);
        vm.tlb.fail = false;
        vm.teardown().unwrap();
    }
    #[test]
    fn teardown_is_one_shot_and_refuses_live_mappings() {
        let mut vm = new_vm();
        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();
        assert_eq!(vm.teardown(), Err(VmError::MappingsRemain));
        vm.unmap_4k(0x4000).unwrap();
        vm.teardown().unwrap();
        assert_eq!(vm.teardown(), Err(VmError::AlreadyTornDown));
    }
    #[test]
    fn kernel_mapping_is_confined_to_kernel_half() {
        let mut vm = new_vm();
        let p = Perms {
            read: true,
            write: true,
            execute: false,
            user: false,
        };
        let va = 0xffff_8000_0000_4000;
        vm.map_4k(va, 0x9000, p, MapOwnership::Borrowed).unwrap();
        assert!(!vm.translate(va).unwrap().permissions.user);
        assert_eq!(
            vm.map_4k(0x4000, 0xa000, p, MapOwnership::Borrowed),
            Err(VmError::OutsideLayout)
        );
    }
    #[test]
    fn partial_contiguous_map_failure_preserves_preexisting_page() {
        let mut vm = new_vm();
        vm.map_4k(0x21_000, 0x51_000, rw(), MapOwnership::Borrowed)
            .unwrap();
        assert_eq!(
            vm.map_contiguous_borrowed(0x20_000, 0x40_000, 2, rw()),
            Err(VmError::Overlap)
        );
        assert_eq!(vm.mapping_count(), 1);
        assert_eq!(vm.translate(0x20_000), Err(VmError::NotMapped));
        assert_eq!(vm.translate(0x21_000).unwrap().physical, 0x51_000);
    }
    #[test]
    fn reclaimed_table_frames_can_be_reused() {
        let mut vm: PageTableVm<MockFrames<8>, MockBackend<8>, MockTlb, 4, 4> = PageTableVm::new(
            MockFrames::new(0x50_0000),
            MockBackend::new(),
            MockTlb::new(),
            CFG,
        )
        .unwrap();
        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();
        vm.unmap_4k(0x4000).unwrap();
        assert_eq!(vm.store.live(), 1);
        vm.map_4k(0x5000, 0xa000, rw(), MapOwnership::Borrowed)
            .unwrap();
        assert_eq!(vm.translate(0x5000).unwrap().physical, 0xa000);
    }
    #[test]
    fn invalid_configuration_returns_all_constructor_resources() {
        let mut error = PageTableVm::<MockFrames<4>, MockBackend<4>, MockTlb, 4, 4>::new(
            MockFrames::new(0x1000),
            MockBackend::new(),
            MockTlb::new(),
            VmConfig::new(true, 64),
        )
        .err()
        .unwrap();

        assert_eq!(error.kind(), VmError::UnsupportedFeature);
        assert_eq!(error.store_error(), None);
        assert_eq!(error.live_table_count(), 0);
        assert_eq!(error.retry_cleanup(), Ok(()));
    }

    #[test]
    fn failed_root_initialization_preserves_pending_cleanup_for_retry() {
        let mut frames = MockFrames::<4>::new(0x1000);
        frames.fail_next_releases(1);

        let mut error = PageTableVm::<MockFrames<4>, MockBackend<0>, MockTlb, 4, 4>::new(
            frames,
            MockBackend::new(),
            MockTlb::new(),
            CFG,
        )
        .err()
        .unwrap();

        assert_eq!(error.kind(), VmError::Frame);
        assert_eq!(error.store_error(), Some(StoreError::CleanupPending));
        assert_eq!(error.pending_cleanup_count(), 1);
        assert_eq!(error.live_table_count(), 0);

        assert_eq!(error.retry_cleanup(), Ok(()));
        assert_eq!(error.pending_cleanup_count(), 0);
        assert_eq!(error.live_table_count(), 0);
    }

    #[test]
    fn invalid_root_address_preserves_live_root_when_release_fails() {
        let mut frames = MockFrames::<4>::new(0x1_0000_0000);
        frames.fail_next_releases(1);

        let mut error = PageTableVm::<MockFrames<4>, MockBackend<4>, MockTlb, 4, 4>::new(
            frames,
            MockBackend::new(),
            MockTlb::new(),
            VmConfig::new(true, 32),
        )
        .err()
        .unwrap();

        assert_eq!(error.kind(), VmError::PhysicalAddress);
        assert_eq!(error.live_table_count(), 1);
        assert_eq!(error.pending_cleanup_count(), 0);

        assert_eq!(error.retry_cleanup(), Ok(()));
        assert_eq!(error.live_table_count(), 0);
    }

    #[test]
    fn configuration_rejects_unsupported_width_and_missing_nx() {
        assert!(matches!(
            PageTableVm::<MockFrames<4>, MockBackend<4>, MockTlb, 4, 4>::new(
                MockFrames::new(0x1000),
                MockBackend::new(),
                MockTlb::new(),
                VmConfig::new(true, 64)
            ),
            Err(error) if error.kind() == VmError::UnsupportedFeature
        ));
        assert!(matches!(
            PageTableVm::<MockFrames<4>, MockBackend<4>, MockTlb, 4, 4>::new(
                MockFrames::new(0x1_0000_0000),
                MockBackend::new(),
                MockTlb::new(),
                VmConfig::new(true, 32)
            ),
            Err(error) if error.kind() == VmError::PhysicalAddress
        ));
        let mut vm: PageTableVm<MockFrames<8>, MockBackend<8>, MockTlb, 8, 4> = PageTableVm::new(
            MockFrames::new(0x1000),
            MockBackend::new(),
            MockTlb::new(),
            VmConfig::new(false, 36),
        )
        .unwrap();
        assert_eq!(
            vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed),
            Err(VmError::UnsupportedFeature)
        );
        let executable = Perms {
            read: true,
            write: false,
            execute: true,
            user: true,
        };
        assert!(
            vm.map_4k(0x4000, 0x9000, executable, MapOwnership::Borrowed)
                .is_ok()
        );
    }
    #[test]
    fn invalid_address_and_duplicate_mapping_are_rejected() {
        let mut vm = new_vm();
        assert_eq!(
            vm.map_4k(0x0000_8000_0000_0000, 0x9000, rw(), MapOwnership::Borrowed),
            Err(VmError::NonCanonical)
        );
        vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed)
            .unwrap();
        assert_eq!(
            vm.map_4k(0x4000, 0xa000, rw(), MapOwnership::Borrowed),
            Err(VmError::Overlap)
        );
    }
    #[test]
    fn tlb_failure_rolls_back_new_mapping() {
        let mut vm = new_vm();
        vm.tlb.fail = true;
        assert_eq!(
            vm.map_4k(0x4000, 0x9000, rw(), MapOwnership::Borrowed),
            Err(VmError::TlbUncertain)
        );
        assert_eq!(vm.mapping_count(), 0);
    }
}
