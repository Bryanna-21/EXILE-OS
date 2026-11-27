//! Bounded hierarchy audit: validates entries, ownership, and metadata correspondence.
use super::{
    backend::PageTableMemory,
    entry::Entry,
    frame_source::FrameSource,
    space::{PageTableVm, VmError},
    tlb::TlbInvalidator,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditReport {
    pub tables: usize,
    pub leaves_4k: usize,
    pub leaves_2m: usize,
}
fn visit<F: FrameSource, B: PageTableMemory, T: TlbInvalidator, const N: usize, const M: usize>(
    vm: &PageTableVm<F, B, T, N, M>,
    phys: u64,
    level: u8,
    seen: &mut [u64; N],
    used: &mut usize,
    report: &mut AuditReport,
    inherited_user: bool,
) -> Result<(), VmError> {
    if *used >= N || seen[..*used].contains(&phys) {
        return Err(VmError::Malformed);
    }
    if !vm.store.contains(phys) || vm.store.level_for_physical(phys) != Some(level) {
        return Err(VmError::Malformed);
    }
    seen[*used] = phys;
    *used += 1;
    report.tables += 1;
    for i in 0..512 {
        let raw = vm
            .backend
            .read_entry(phys, i)
            .map_err(|_| VmError::Backend)?;
        if raw & 1 == 0 {
            if raw != 0 {
                return Err(VmError::Malformed);
            }
            continue;
        }
        let e = Entry::parse(raw, level).map_err(|_| VmError::Malformed)?;
        if level == 1 {
            if e.user() && !inherited_user {
                return Err(VmError::Malformed);
            }
            report.leaves_4k += 1;
            continue;
        }
        if level == 2 && e.huge_at_level(level) {
            if e.user() && !inherited_user {
                return Err(VmError::Malformed);
            }
            if e.physical().map_err(|_| VmError::Malformed)? & (super::geometry::HUGE_2M - 1) != 0 {
                return Err(VmError::Malformed);
            }
            report.leaves_2m += 1;
            continue;
        }
        if e.huge_at_level(level) {
            return Err(VmError::Malformed);
        }
        let child = e.physical().map_err(|_| VmError::Malformed)?;
        visit(
            vm,
            child,
            level - 1,
            seen,
            used,
            report,
            inherited_user && e.user(),
        )?;
    }
    Ok(())
}
pub fn audit<
    F: FrameSource,
    B: PageTableMemory,
    T: TlbInvalidator,
    const N: usize,
    const M: usize,
>(
    vm: &PageTableVm<F, B, T, N, M>,
) -> Result<AuditReport, VmError> {
    if !vm.active {
        return Err(VmError::Inactive);
    }
    if !vm.store.check_invariants() {
        return Err(VmError::Malformed);
    }
    if vm.data_frames.iter().flatten().count() != vm.data_frame_len
        || vm
            .data_frames
            .iter()
            .flatten()
            .any(|l| !vm.store.validate_data_frame(*l))
    {
        return Err(VmError::Malformed);
    }
    for (i, l) in vm.data_frames.iter().enumerate() {
        if let Some(token) = l {
            if vm.data_frames[i + 1..].iter().flatten().any(|x| x == token) {
                return Err(VmError::Malformed);
            }
        }
    }
    for m in vm.mappings() {
        if let super::space::MapOwnership::Owned(l) = m.ownership {
            if !vm.data_frames.iter().any(|x| *x == Some(l)) {
                return Err(VmError::Malformed);
            }
        }
    }
    let mut seen = [0u64; N];
    let mut used = 0;
    let mut report = AuditReport {
        tables: 0,
        leaves_4k: 0,
        leaves_2m: 0,
    };
    visit(
        vm,
        vm.root.physical(),
        4,
        &mut seen,
        &mut used,
        &mut report,
        true,
    )?;
    if report.leaves_4k + report.leaves_2m != vm.mapping_len || used != vm.store.live() {
        return Err(VmError::Malformed);
    }
    for m in vm.mappings() {
        let t = vm.translate(m.virtual_address)?;
        if t.physical != m.physical_address {
            return Err(VmError::Malformed);
        }
        if m.permissions.user && !t.permissions.user
            || m.permissions.write && !t.permissions.write
            || m.permissions.execute && !t.permissions.execute
        {
            return Err(VmError::Malformed);
        }
    }
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_report_type() {
        let r = AuditReport {
            tables: 4,
            leaves_4k: 1,
            leaves_2m: 0,
        };
        assert_eq!(r.tables, 4);
    }
}
