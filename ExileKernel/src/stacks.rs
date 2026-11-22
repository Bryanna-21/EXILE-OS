//! Kernel stack configuration and GDT/TSS installation (x86-64). SPDX-License-Identifier: Apache-2.0
//!
//! Provides the privilege-transition stack (TSS.RSP0) and the dedicated exception stacks (IST1:
//! double fault, IST2: NMI, IST3: machine check), wires them into the TSS, and loads the GDT,
//! segment registers and TR for the bootstrap processor.
//!
//! Split on purpose: `prepare` is pure and host-tested; `install_bsp` is the only privileged
//! entry point and has NOT been executed in this repository's test environment (no emulator was
//! available when it was written). The emulator case `--traps` in `scripts/boot_test.py` is the
//! test that must pass before this code is considered working.
//!
//! Limits, stated plainly:
//! * No guard pages exist (the bootstrap map is RWX 2 MiB pages), so a stack overflow silently
//!   corrupts the adjacent stack. Guard pages are a Mission B / small-page-mapping prerequisite.
//! * BSP only. Per-CPU copies belong with per-CPU state (a later task).
//! * The stacks are `.bss` and identity-mapped by the bootstrap; `map_limit` makes that
//!   assumption explicit and checked.
#![allow(dead_code)]

use crate::gdt::{
    DescriptorError, Gdt, Gdtr, KERNEL_CODE, KERNEL_DATA, TSS_LIMIT, TSS_SELECTOR, Tss,
};
use crate::idt::{IST_DOUBLE_FAULT, IST_MACHINE_CHECK, IST_NMI};
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

pub const STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
pub struct Stack([u8; STACK_SIZE]);
const _: () = assert!(core::mem::size_of::<Stack>() == STACK_SIZE);
const _: () = assert!(STACK_SIZE % 16 == 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackId {
    /// TSS.RSP0: used on a ring 3 to ring 0 transition.
    Kernel,
    DoubleFault,
    Nmi,
    MachineCheck,
}

impl StackId {
    /// IST slot (1..=7) this stack occupies, or `None` for RSP0.
    pub const fn ist_slot(self) -> Option<u8> {
        match self {
            StackId::Kernel => None,
            StackId::DoubleFault => Some(IST_DOUBLE_FAULT),
            StackId::Nmi => Some(IST_NMI),
            StackId::MachineCheck => Some(IST_MACHINE_CHECK),
        }
    }
}

const ALL: [StackId; 4] = [
    StackId::Kernel,
    StackId::DoubleFault,
    StackId::Nmi,
    StackId::MachineCheck,
];

#[repr(C)]
pub struct StackSet {
    kernel: Stack,
    double_fault: Stack,
    nmi: Stack,
    machine_check: Stack,
}

impl StackSet {
    pub const fn new() -> Self {
        Self {
            kernel: Stack([0; STACK_SIZE]),
            double_fault: Stack([0; STACK_SIZE]),
            nmi: Stack([0; STACK_SIZE]),
            machine_check: Stack([0; STACK_SIZE]),
        }
    }

    fn stack(&self, id: StackId) -> &Stack {
        match id {
            StackId::Kernel => &self.kernel,
            StackId::DoubleFault => &self.double_fault,
            StackId::Nmi => &self.nmi,
            StackId::MachineCheck => &self.machine_check,
        }
    }

    /// Lowest address of the stack.
    pub fn base(&self, id: StackId) -> u64 {
        self.stack(id) as *const Stack as u64
    }

    /// One past the highest byte: the value the CPU expects in RSP0/ISTn. 16-byte aligned.
    pub fn top(&self, id: StackId) -> Option<u64> {
        self.base(id).checked_add(STACK_SIZE as u64)
    }

    /// Checks every stack lies entirely below `map_limit` (the end of the identity-mapped
    /// region), is 16-byte aligned, and that no two stacks overlap.
    pub fn validate(&self, map_limit: u64) -> Result<(), InstallError> {
        for (i, a) in ALL.iter().enumerate() {
            let base = self.base(*a);
            let top = self.top(*a).ok_or(InstallError::StackOutsideMap)?;
            if base % 16 != 0 || top % 16 != 0 {
                return Err(InstallError::Descriptor(DescriptorError::MisalignedStack));
            }
            if top > map_limit {
                return Err(InstallError::StackOutsideMap);
            }
            for b in &ALL[i + 1..] {
                let (b_base, b_top) = (
                    self.base(*b),
                    self.top(*b).ok_or(InstallError::StackOutsideMap)?,
                );
                if base < b_top && b_base < top {
                    return Err(InstallError::StacksOverlap);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallError {
    Descriptor(DescriptorError),
    /// A stack is not fully inside the identity-mapped region, so it might not be mapped.
    StackOutsideMap,
    StacksOverlap,
    /// GDT/TSS were already loaded on this CPU; a second `ltr` of a busy TSS raises #GP.
    AlreadyInstalled,
}

impl From<DescriptorError> for InstallError {
    fn from(e: DescriptorError) -> Self {
        InstallError::Descriptor(e)
    }
}

/// Writes RSP0 and every IST slot into `tss`. On error the TSS may be partially written, so a
/// failed TSS must not be loaded.
pub fn configure_tss(tss: &mut Tss, stacks: &StackSet) -> Result<(), InstallError> {
    for id in ALL {
        let top = stacks.top(id).ok_or(InstallError::StackOutsideMap)?;
        match id.ist_slot() {
            None => tss.set_rsp0(top)?,
            Some(slot) => tss.set_ist(slot, top)?,
        }
    }
    Ok(())
}

/// Pure half of installation: validates stacks, fills the TSS and builds the GDT that refers
/// to it. Nothing here touches CPU state.
pub fn prepare(
    stacks: &StackSet,
    tss: &mut Tss,
    tss_base: u64,
    map_limit: u64,
) -> Result<Gdt, InstallError> {
    stacks.validate(map_limit)?;
    configure_tss(tss, stacks)?;
    Ok(Gdt::new(tss_base, TSS_LIMIT)?)
}

/// TSS aligned to 16 bytes; the hardware tolerates less but misalignment costs cycles on
/// every privilege transition.
#[repr(C, align(16))]
struct AlignedTss(Tss);

// Mutable statics, touched only by `install_bsp` (guarded by INSTALLED) and by the hardware
// (`ltr` sets the TSS descriptor busy bit in BSP_GDT, so it must live in writable memory).
static mut BSP_STACKS: StackSet = StackSet::new();
static mut BSP_TSS: AlignedTss = AlignedTss(Tss::new());
static mut BSP_GDT: MaybeUninit<Gdt> = MaybeUninit::uninit();
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Loads the GDT, reloads CS and the data segments, and loads TR for the bootstrap CPU.
///
/// # Safety
/// * Ring 0, interrupts disabled (no handler is installed yet that could run on a half-loaded
///   segment state).
/// * `gdtr` must describe a valid GDT whose entries match the `KERNEL_*`/`TSS_SELECTOR`
///   constants, located in writable memory that outlives the CPU's use of it.
/// * The TSS referenced by the GDT must be fully configured, and `TSS_SELECTOR` must refer to an
///   *available* (not busy) TSS; `ltr` on a busy TSS faults.
unsafe fn load_gdt_and_tss(gdtr: &Gdtr) {
    // SAFETY: caller contract. The far return reloads CS with the new kernel code selector; the
    // push/retfq pair uses the stack, so this asm must not be `nostack`.
    unsafe {
        core::arch::asm!(
            "lgdt [{gdtr}]",
            "push {cs}",
            "lea {tmp}, [2f + rip]",
            "push {tmp}",
            "retfq",
            // Not `1:`: LLVM mis-parses asm labels made only of 0/1 digits as binary literals.
            "2:",
            "mov ds, {data:x}",
            "mov es, {data:x}",
            "mov ss, {data:x}",
            "mov fs, {zero:x}",
            "mov gs, {zero:x}",
            "ltr {tr:x}",
            gdtr = in(reg) gdtr as *const Gdtr,
            cs = in(reg) u64::from(KERNEL_CODE),
            data = in(reg) u32::from(KERNEL_DATA),
            zero = in(reg) 0u32,
            tr = in(reg) u32::from(TSS_SELECTOR),
            // `out`, not `lateout`: tmp is written before the later inputs are read, so it must
            // not share a register with any of them.
            tmp = out(reg) _,
            options(preserves_flags),
        );
    }
}

/// Configures stacks, builds and loads the GDT and TSS for the bootstrap CPU. Call once.
///
/// `map_limit` is the end of the identity-mapped region (the bootstrap maps the first GiB).
///
/// # Safety
/// Ring 0 with interrupts disabled, after the bootstrap page tables are active. Must be called
/// at most once per CPU; a second call returns `AlreadyInstalled` before touching the CPU.
pub unsafe fn install_bsp(map_limit: u64) -> Result<(), InstallError> {
    // SAFETY: single caller by contract; raw pointers avoid references to `static mut`.
    let (stacks, tss, gdt_slot) = unsafe {
        (
            &*(&raw const BSP_STACKS),
            &mut *(&raw mut BSP_TSS),
            &mut *(&raw mut BSP_GDT),
        )
    };
    let tss_base = &raw const BSP_TSS as u64;
    let gdt = prepare(stacks, &mut tss.0, tss_base, map_limit)?;
    // Everything fallible and pure succeeded; only now claim the one-shot hardware load.
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return Err(InstallError::AlreadyInstalled);
    }
    let gdt = gdt_slot.write(gdt);
    let gdtr = gdt.pointer();
    // SAFETY: gdt lives in a static, the TSS above is configured and not yet busy (INSTALLED).
    unsafe { load_gdt_and_tss(&gdtr) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: u64 = u64::MAX;

    fn stacks() -> Box<StackSet> {
        Box::new(StackSet::new())
    }

    #[test]
    fn stack_tops_are_aligned_and_inside_their_stack() {
        let s = stacks();
        for id in ALL {
            let (base, top) = (s.base(id), s.top(id).unwrap());
            assert_eq!(top - base, STACK_SIZE as u64);
            assert_eq!(base % 16, 0, "{id:?}");
            assert_eq!(top % 16, 0, "{id:?}");
        }
    }

    #[test]
    fn stacks_are_disjoint() {
        let s = stacks();
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                let (ab, at) = (s.base(*a), s.top(*a).unwrap());
                let (bb, bt) = (s.base(*b), s.top(*b).unwrap());
                assert!(at <= bb || bt <= ab, "{a:?} overlaps {b:?}");
            }
        }
        assert_eq!(s.validate(LIMIT), Ok(()));
    }

    #[test]
    fn ist_slots_match_idt_assignments_and_are_unique() {
        assert_eq!(StackId::Kernel.ist_slot(), None);
        assert_eq!(StackId::DoubleFault.ist_slot(), Some(IST_DOUBLE_FAULT));
        assert_eq!(StackId::Nmi.ist_slot(), Some(IST_NMI));
        assert_eq!(StackId::MachineCheck.ist_slot(), Some(IST_MACHINE_CHECK));
        let slots: Vec<u8> = ALL.iter().filter_map(|i| i.ist_slot()).collect();
        let mut sorted = slots.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), slots.len());
        assert!(slots.iter().all(|s| (1..=7).contains(s)));
    }

    #[test]
    fn configure_tss_sets_rsp0_and_every_ist() {
        let s = stacks();
        let mut tss = Tss::new();
        configure_tss(&mut tss, &s).unwrap();
        assert_eq!(tss.rsp0(), s.top(StackId::Kernel).unwrap());
        assert_eq!(tss.ist(IST_DOUBLE_FAULT), s.top(StackId::DoubleFault));
        assert_eq!(tss.ist(IST_NMI), s.top(StackId::Nmi));
        assert_eq!(tss.ist(IST_MACHINE_CHECK), s.top(StackId::MachineCheck));
        // Slots not assigned to an exception stay zero (no stray stack).
        for slot in 4..=7 {
            assert_eq!(tss.ist(slot), Some(0), "IST{slot}");
        }
        // All distinct.
        let tops = [
            tss.rsp0(),
            tss.ist(1).unwrap(),
            tss.ist(2).unwrap(),
            tss.ist(3).unwrap(),
        ];
        for i in 0..tops.len() {
            for j in i + 1..tops.len() {
                assert_ne!(tops[i], tops[j]);
            }
        }
    }

    #[test]
    fn stack_outside_identity_map_is_rejected() {
        let s = stacks();
        let highest = ALL.iter().map(|i| s.top(*i).unwrap()).max().unwrap();
        assert_eq!(
            s.validate(highest),
            Ok(()),
            "limit exactly at the highest top is allowed"
        );
        assert_eq!(s.validate(highest - 1), Err(InstallError::StackOutsideMap));
        assert_eq!(s.validate(0), Err(InstallError::StackOutsideMap));
    }

    #[test]
    fn prepare_builds_gdt_for_the_given_tss() {
        let s = stacks();
        let mut tss = Tss::new();
        let base = 0xffff_8000_0000_1000;
        let gdt = prepare(&s, &mut tss, base, LIMIT).unwrap();
        let low = gdt.entry(5).unwrap();
        assert_eq!(low & 0xffff, u64::from(TSS_LIMIT));
        assert_eq!(
            ((low >> 16) & 0xff_ffff) | (((low >> 56) & 0xff) << 24),
            base & 0xffff_ffff
        );
        assert_eq!(gdt.entry(6), Some(base >> 32));
        assert_eq!(tss.rsp0(), s.top(StackId::Kernel).unwrap());
    }

    #[test]
    fn prepare_propagates_descriptor_errors_without_a_gdt() {
        let s = stacks();
        let mut tss = Tss::new();
        // Non-canonical TSS base: stacks are fine, the GDT cannot be built.
        let e = prepare(&s, &mut tss, 0x0000_8000_0000_0000, LIMIT).err();
        assert_eq!(
            e,
            Some(InstallError::Descriptor(DescriptorError::NonCanonical))
        );
    }

    #[test]
    fn prepare_rejects_before_touching_tss_when_stacks_invalid() {
        let s = stacks();
        let mut tss = Tss::new();
        assert_eq!(
            prepare(&s, &mut tss, 0x1000, 0).err(),
            Some(InstallError::StackOutsideMap)
        );
        assert_eq!(tss.rsp0(), 0, "validation must run before any TSS write");
    }

    #[test]
    fn selectors_used_by_loader_match_gdt_layout() {
        // load_gdt_and_tss relies on these indices being what Gdt::new builds.
        let gdt = Gdt::new(0x1000, TSS_LIMIT).unwrap();
        assert_eq!(
            gdt.entry(usize::from(KERNEL_CODE >> 3)),
            Some(crate::gdt::code64(0))
        );
        assert_eq!(
            gdt.entry(usize::from(KERNEL_DATA >> 3)),
            Some(crate::gdt::data(0))
        );
        assert_eq!(
            (gdt.entry(usize::from(TSS_SELECTOR >> 3)).unwrap() >> 40) & 0xff,
            0x89
        );
    }
}
