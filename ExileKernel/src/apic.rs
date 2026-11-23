//! Local APIC driver (xAPIC MMIO and, only when requested and supported, x2APIC MSRs)
//! (Mission C3.4, part 1). SPDX-License-Identifier: Apache-2.0
//!
//! Hardware access is behind two traits, [`Msr`] and [`Mmio`], so everything here is host-tested
//! with fakes and no privileged instruction, MSR or MMIO access happens on the host. `HwMsr` and
//! `VolatileMmio` are the real implementations.
//!
//! **Mapping constraint.** The bootstrap page tables identity-map only the first GiB. The local
//! APIC (usually 0xFEE00000), IOAPIC (0xFEC00000) and HPET (0xFED00000) registers are outside
//! it, so `VolatileMmio` must not be created for them until the memory subsystem maps those
//! pages uncached. [`mmio_reachable`] is the check the kernel uses; this file performs no
//! mapping itself.
//!
//! Scope: BSP only. No inter-processor interrupts, no application-processor start-up, no
//! per-CPU state. The APIC timer is not used here.
#![allow(dead_code)]
#![allow(
    clippy::manual_is_multiple_of,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::identity_op,
    clippy::unusual_byte_groupings
)]

use crate::cpu::{CpuInfo, Feature};
use crate::irq::{ControllerError, InterruptController, SPURIOUS_VECTOR};

/// End of the identity-mapped window the bootstrap provides.
pub const BOOTSTRAP_MAP_LIMIT: u64 = 1 << 30;

/// True if `[base, base+len)` lies wholly inside the mapped window `limit`.
pub fn mmio_reachable(base: u64, len: u64, limit: u64) -> bool {
    len != 0 && base.checked_add(len).is_some_and(|end| end <= limit)
}

pub trait Msr {
    fn rdmsr(&mut self, msr: u32) -> u64;
    fn wrmsr(&mut self, msr: u32, value: u64);
}

/// Memory-mapped register window. Offsets are relative to the window base.
pub trait Mmio {
    fn read32(&mut self, offset: usize) -> u32;
    fn write32(&mut self, offset: usize, value: u32);
    fn read64(&mut self, offset: usize) -> u64;
    fn write64(&mut self, offset: usize, value: u64);
}

#[cfg(target_arch = "x86_64")]
pub struct HwMsr(());

#[cfg(target_arch = "x86_64")]
impl HwMsr {
    /// # Safety
    /// Ring 0, and the MSRs accessed through it exist on this CPU (the callers check CPUID).
    pub const unsafe fn new() -> Self {
        Self(())
    }
}

#[cfg(target_arch = "x86_64")]
impl Msr for HwMsr {
    fn rdmsr(&mut self, msr: u32) -> u64 {
        let (lo, hi): (u32, u32);
        // SAFETY: `HwMsr::new` contract: ring 0 and the MSR exists.
        unsafe {
            core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
                options(nomem, nostack, preserves_flags));
        }
        (hi as u64) << 32 | lo as u64
    }
    fn wrmsr(&mut self, msr: u32, value: u64) {
        // SAFETY: as above; the callers write only values validated by this module.
        unsafe {
            core::arch::asm!("wrmsr", in("ecx") msr, in("eax") value as u32,
                in("edx") (value >> 32) as u32, options(nomem, nostack, preserves_flags));
        }
    }
}

/// Real MMIO through volatile accesses.
pub struct VolatileMmio {
    base: usize,
    len: usize,
}

impl VolatileMmio {
    /// # Safety
    /// `base..base+len` must be mapped, uncached device memory of the intended device for as
    /// long as this value is used, `base` must be 8-byte aligned, and nothing else may access
    /// the registers concurrently.
    pub const unsafe fn new(base: usize, len: usize) -> Self {
        Self { base, len }
    }
    fn check(&self, offset: usize, width: usize) {
        // A bad offset is a kernel bug, not a runtime condition; stop rather than touch memory
        // outside the window.
        assert!(
            offset % width == 0 && offset.checked_add(width).is_some_and(|e| e <= self.len),
            "MMIO offset outside window"
        );
    }
}

impl Mmio for VolatileMmio {
    fn read32(&mut self, offset: usize) -> u32 {
        self.check(offset, 4);
        // SAFETY: bounds and alignment checked; mapping is the `new` contract.
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }
    fn write32(&mut self, offset: usize, value: u32) {
        self.check(offset, 4);
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) }
    }
    fn read64(&mut self, offset: usize) -> u64 {
        self.check(offset, 8);
        // SAFETY: as above.
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u64) }
    }
    fn write64(&mut self, offset: usize, value: u64) {
        self.check(offset, 8);
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u64, value) }
    }
}

// ---------------------------------------------------------------------------------------------
// IA32_APIC_BASE
// ---------------------------------------------------------------------------------------------

pub const IA32_APIC_BASE: u32 = 0x1B;
const BASE_BSP: u64 = 1 << 8;
const BASE_X2APIC: u64 = 1 << 10;
const BASE_ENABLE: u64 = 1 << 11;
/// Bits 12..51: the physical base (architectural maximum width).
const BASE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;
/// Reserved: 0..7, 9, and 52..63.
const BASE_RESERVED: u64 = 0xFF | (1 << 9) | !0x000F_FFFF_FFFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApicBase(pub u64);

impl ApicBase {
    pub const fn address(self) -> u64 {
        self.0 & BASE_ADDR_MASK
    }
    pub const fn enabled(self) -> bool {
        self.0 & BASE_ENABLE != 0
    }
    pub const fn x2apic(self) -> bool {
        self.0 & BASE_X2APIC != 0
    }
    pub const fn is_bsp(self) -> bool {
        self.0 & BASE_BSP != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApicError {
    /// CPUID does not advertise a local APIC.
    NoApic,
    /// x2APIC requested but not advertised.
    X2apicUnsupported,
    ReservedBits,
    /// EXTD set without EN: an architecturally invalid MSR state.
    InvalidState,
    BadBase,
    /// The MSR's base differs from the address the firmware tables gave.
    BaseMismatch,
    /// The APIC is already in x2APIC mode but xAPIC mode was requested (it cannot be undone
    /// without disabling the APIC).
    AlreadyX2apic,
    /// The version register reads as absent (all ones / zero) or below 0x10.
    NoDevice,
    BadVector,
    /// The MSR did not take the value written.
    WriteFailed,
}

/// Decodes and validates a raw IA32_APIC_BASE value.
pub fn validate_base(raw: u64, expected: Option<u64>) -> Result<ApicBase, ApicError> {
    if raw & BASE_RESERVED != 0 {
        return Err(ApicError::ReservedBits);
    }
    let b = ApicBase(raw);
    if b.x2apic() && !b.enabled() {
        return Err(ApicError::InvalidState);
    }
    if b.address() == 0 {
        return Err(ApicError::BadBase);
    }
    if let Some(e) = expected {
        if b.address() != e {
            return Err(ApicError::BaseMismatch);
        }
    }
    Ok(b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    XApic,
    X2Apic,
}

/// What the caller asked for. x2APIC is never chosen implicitly.
pub type Requested = Mode;

pub fn select_mode(info: &CpuInfo, requested: Requested) -> Result<Mode, ApicError> {
    if !info.features.has(Feature::Apic) {
        return Err(ApicError::NoApic);
    }
    match requested {
        Mode::XApic => Ok(Mode::XApic),
        Mode::X2Apic if info.features.has(Feature::X2apic) => Ok(Mode::X2Apic),
        Mode::X2Apic => Err(ApicError::X2apicUnsupported),
    }
}

/// Validates the APIC base MSR and brings the APIC to the requested mode. Never relocates the
/// APIC: `expected_base` (from the MADT) must match what the MSR says. Enabling x2APIC goes
/// through enabled-xAPIC first, as the SDM requires (disabled -> x2APIC is invalid).
pub fn configure_msr<S: Msr>(
    msr: &mut S,
    mode: Mode,
    expected_base: Option<u64>,
) -> Result<ApicBase, ApicError> {
    let cur = validate_base(msr.rdmsr(IA32_APIC_BASE), expected_base)?;
    match mode {
        Mode::XApic => {
            if cur.x2apic() {
                return Err(ApicError::AlreadyX2apic);
            }
            if !cur.enabled() {
                msr.wrmsr(IA32_APIC_BASE, cur.0 | BASE_ENABLE);
            }
        }
        Mode::X2Apic => {
            if !cur.enabled() {
                msr.wrmsr(IA32_APIC_BASE, cur.0 | BASE_ENABLE);
            }
            if !cur.x2apic() {
                msr.wrmsr(IA32_APIC_BASE, (cur.0 | BASE_ENABLE) | BASE_X2APIC);
            }
        }
    }
    let after = validate_base(msr.rdmsr(IA32_APIC_BASE), expected_base)?;
    let ok = after.enabled() && after.x2apic() == (mode == Mode::X2Apic);
    if ok {
        Ok(after)
    } else {
        Err(ApicError::WriteFailed)
    }
}

// ---------------------------------------------------------------------------------------------
// Registers
// ---------------------------------------------------------------------------------------------

pub const REG_ID: u32 = 0x20;
pub const REG_VERSION: u32 = 0x30;
pub const REG_TPR: u32 = 0x80;
pub const REG_EOI: u32 = 0xB0;
pub const REG_SVR: u32 = 0xF0;
pub const REG_ESR: u32 = 0x280;
pub const REG_LVT_CMCI: u32 = 0x2F0;
pub const REG_LVT_TIMER: u32 = 0x320;
pub const REG_LVT_THERMAL: u32 = 0x330;
pub const REG_LVT_PERF: u32 = 0x340;
pub const REG_LINT0: u32 = 0x350;
pub const REG_LINT1: u32 = 0x360;
pub const REG_LVT_ERROR: u32 = 0x370;
const REG_LIMIT: u32 = 0x400;

pub const SVR_ENABLE: u32 = 1 << 8;
pub const LVT_MASKED: u32 = 1 << 16;
const X2APIC_MSR_BASE: u32 = 0x800;

/// Register access in either mode. `reg` is the xAPIC MMIO offset; the x2APIC implementation
/// derives the MSR number (0x800 + offset/16) from it.
pub trait ApicRegs {
    fn mode(&self) -> Mode;
    fn read(&mut self, reg: u32) -> u32;
    fn write(&mut self, reg: u32, value: u32);
}

pub struct XApicRegs<M: Mmio>(pub M);

impl<M: Mmio> ApicRegs for XApicRegs<M> {
    fn mode(&self) -> Mode {
        Mode::XApic
    }
    fn read(&mut self, reg: u32) -> u32 {
        debug_assert!(reg < REG_LIMIT && reg % 16 == 0);
        self.0.read32(reg as usize)
    }
    fn write(&mut self, reg: u32, value: u32) {
        debug_assert!(reg < REG_LIMIT && reg % 16 == 0);
        self.0.write32(reg as usize, value)
    }
}

pub struct X2ApicRegs<S: Msr>(pub S);

impl<S: Msr> ApicRegs for X2ApicRegs<S> {
    fn mode(&self) -> Mode {
        Mode::X2Apic
    }
    fn read(&mut self, reg: u32) -> u32 {
        debug_assert!(reg < REG_LIMIT && reg % 16 == 0);
        self.0.rdmsr(X2APIC_MSR_BASE + (reg >> 4)) as u32
    }
    fn write(&mut self, reg: u32, value: u32) {
        debug_assert!(reg < REG_LIMIT && reg % 16 == 0);
        self.0.wrmsr(X2APIC_MSR_BASE + (reg >> 4), value as u64)
    }
}

pub struct LocalApic<R: ApicRegs> {
    regs: R,
    spurious_vector: u8,
    enabled: bool,
    spurious_seen: u64,
}

impl<R: ApicRegs> LocalApic<R> {
    pub fn new(regs: R) -> Self {
        Self {
            regs,
            spurious_vector: SPURIOUS_VECTOR,
            enabled: false,
            spurious_seen: 0,
        }
    }

    pub fn mode(&self) -> Mode {
        self.regs.mode()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn spurious_seen(&self) -> u64 {
        self.spurious_seen
    }

    pub fn regs_mut(&mut self) -> &mut R {
        &mut self.regs
    }

    /// Local APIC ID (8 bits in xAPIC mode, 32 in x2APIC).
    pub fn id(&mut self) -> u32 {
        let raw = self.regs.read(REG_ID);
        match self.regs.mode() {
            Mode::XApic => raw >> 24,
            Mode::X2Apic => raw,
        }
    }

    /// Reads and sanity-checks the version register; returns (version, max LVT entry).
    pub fn version(&mut self) -> Result<(u8, u8), ApicError> {
        let raw = self.regs.read(REG_VERSION);
        let version = raw as u8;
        if raw == 0xFFFF_FFFF || raw == 0 || version < 0x10 {
            return Err(ApicError::NoDevice);
        }
        Ok((version, (raw >> 16) as u8))
    }

    /// Software-enables the APIC with every local vector masked and the task priority at zero.
    /// Interrupts must be disabled and the IDT must have a gate for `spurious_vector` (the
    /// kernel IDT has one for every vector). Does not touch the I/O APIC or enable CPU
    /// interrupts.
    pub fn init(&mut self, spurious_vector: u8) -> Result<(), ApicError> {
        if spurious_vector != SPURIOUS_VECTOR {
            return Err(ApicError::BadVector);
        }
        let (_, max_lvt) = self.version()?;
        // Mask every local vector table entry this APIC implements, so nothing fires before a
        // driver configures it.
        let lvts: [(u32, u8); 7] = [
            (REG_LVT_TIMER, 0),
            (REG_LINT0, 3),
            (REG_LINT1, 3),
            (REG_LVT_ERROR, 3),
            (REG_LVT_PERF, 4),
            (REG_LVT_THERMAL, 5),
            (REG_LVT_CMCI, 6),
        ];
        for (reg, min_max_lvt) in lvts {
            if max_lvt >= min_max_lvt {
                self.regs.write(reg, LVT_MASKED);
            }
        }
        self.regs.write(REG_TPR, 0);
        if self.regs.mode() == Mode::XApic && max_lvt >= 3 {
            // Error status: write-then-read latches and clears it (xAPIC only).
            self.regs.write(REG_ESR, 0);
            let _ = self.regs.read(REG_ESR);
        }
        self.regs
            .write(REG_SVR, SVR_ENABLE | spurious_vector as u32);
        self.spurious_vector = spurious_vector;
        self.enabled = true;
        Ok(())
    }

    /// Software-disables the APIC (SVR enable bit cleared, LVTs masked).
    pub fn disable(&mut self) {
        let svr = self.regs.read(REG_SVR);
        self.regs.write(REG_SVR, svr & !SVR_ENABLE);
        self.enabled = false;
    }

    /// End of interrupt. The register must be written with zero in both modes.
    pub fn eoi(&mut self) {
        self.regs.write(REG_EOI, 0);
    }
}

impl<R: ApicRegs> InterruptController for LocalApic<R> {
    fn name(&self) -> &'static str {
        "local APIC"
    }
    /// The APIC spurious vector never sets an in-service bit and must not be acknowledged.
    fn is_spurious(&mut self, vector: u8) -> bool {
        if self.enabled && vector == self.spurious_vector {
            self.spurious_seen += 1;
            true
        } else {
            false
        }
    }
    fn eoi(&mut self, _vector: u8) {
        if self.enabled {
            LocalApic::eoi(self);
        }
    }
    /// Per-vector masking lives in the I/O APIC (`ioapic::ApicController`).
    fn mask(&mut self, _vector: u8) -> Result<(), ControllerError> {
        Err(ControllerError::Unsupported)
    }
    fn unmask(&mut self, _vector: u8) -> Result<(), ControllerError> {
        Err(ControllerError::Unsupported)
    }
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    pub struct FakeMsr {
        pub regs: BTreeMap<u32, u64>,
        pub writes: Vec<(u32, u64)>,
        /// MSR numbers whose writes are ignored (a broken/locked MSR).
        pub ignore_writes: Vec<u32>,
    }
    impl Msr for FakeMsr {
        fn rdmsr(&mut self, msr: u32) -> u64 {
            *self.regs.get(&msr).unwrap_or(&0)
        }
        fn wrmsr(&mut self, msr: u32, value: u64) {
            self.writes.push((msr, value));
            if !self.ignore_writes.contains(&msr) {
                self.regs.insert(msr, value);
            }
        }
    }

    /// Fake MMIO: a sparse register file that records every access. Rejects any access outside
    /// `len` or misaligned, like the real `VolatileMmio`.
    pub struct FakeMmio {
        pub regs: BTreeMap<usize, u64>,
        pub len: usize,
        pub log: Vec<(usize, u64, bool)>, // offset, value, was_write
    }
    impl FakeMmio {
        pub fn new(len: usize) -> Self {
            Self {
                regs: BTreeMap::new(),
                len,
                log: Vec::new(),
            }
        }
        pub fn set(&mut self, off: usize, v: u64) {
            self.regs.insert(off, v);
        }
        pub fn get(&self, off: usize) -> u64 {
            *self.regs.get(&off).unwrap_or(&0)
        }
        pub fn writes(&self) -> Vec<(usize, u64)> {
            self.log
                .iter()
                .filter(|e| e.2)
                .map(|e| (e.0, e.1))
                .collect()
        }
        fn check(&self, off: usize, w: usize) {
            assert!(
                off % w == 0 && off + w <= self.len,
                "bad MMIO access {off:#x}/{w}"
            );
        }
    }
    impl Mmio for FakeMmio {
        fn read32(&mut self, off: usize) -> u32 {
            self.check(off, 4);
            let v = self.get(off) as u32;
            self.log.push((off, v as u64, false));
            v
        }
        fn write32(&mut self, off: usize, v: u32) {
            self.check(off, 4);
            self.log.push((off, v as u64, true));
            self.regs.insert(off, v as u64);
        }
        fn read64(&mut self, off: usize) -> u64 {
            self.check(off, 8);
            let v = self.get(off);
            self.log.push((off, v, false));
            v
        }
        fn write64(&mut self, off: usize, v: u64) {
            self.check(off, 8);
            self.log.push((off, v, true));
            self.regs.insert(off, v);
        }
    }

    pub fn xapic(version: u32) -> XApicRegs<FakeMmio> {
        let mut m = FakeMmio::new(0x1000);
        m.set(REG_VERSION as usize, version as u64);
        m.set(REG_ID as usize, 3 << 24);
        XApicRegs(m)
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;
    use crate::cpu::CpuidLeaf;

    const VER_6LVT: u32 = 0x0006_0014; // version 0x14, max LVT 6
    const VER_P6: u32 = 0x0003_0014; // max LVT 3

    fn cpu(apic: bool, x2: bool) -> CpuInfo {
        CpuInfo::detect_with(|leaf, _| match leaf {
            0 => CpuidLeaf {
                eax: 1,
                ebx: 0x756e6547,
                edx: 0x49656e69,
                ecx: 0x6c65746e,
            },
            1 => CpuidLeaf {
                eax: 0x000306A9,
                ebx: 0,
                ecx: if x2 { 1 << 21 } else { 0 },
                edx: if apic { 1 << 9 } else { 0 },
            },
            _ => CpuidLeaf::default(),
        })
        .unwrap()
    }

    #[test]
    fn mmio_reachability_against_the_bootstrap_map() {
        assert!(mmio_reachable(0x1000, 0x1000, BOOTSTRAP_MAP_LIMIT));
        assert!(mmio_reachable(
            BOOTSTRAP_MAP_LIMIT - 0x1000,
            0x1000,
            BOOTSTRAP_MAP_LIMIT
        ));
        assert!(!mmio_reachable(
            BOOTSTRAP_MAP_LIMIT,
            0x1000,
            BOOTSTRAP_MAP_LIMIT
        ));
        assert!(
            !mmio_reachable(0xFEE0_0000, 0x1000, BOOTSTRAP_MAP_LIMIT),
            "LAPIC is not mapped"
        );
        assert!(
            !mmio_reachable(0xFED0_0000, 0x400, BOOTSTRAP_MAP_LIMIT),
            "HPET is not mapped"
        );
        assert!(
            !mmio_reachable(0xFEC0_0000, 0x20, BOOTSTRAP_MAP_LIMIT),
            "IOAPIC is not mapped"
        );
        assert!(!mmio_reachable(u64::MAX - 1, 0x1000, u64::MAX));
        assert!(!mmio_reachable(0x1000, 0, BOOTSTRAP_MAP_LIMIT));
    }

    #[test]
    fn apic_base_decoding_and_validation() {
        let raw = 0xFEE0_0000 | BASE_BSP | BASE_ENABLE;
        let b = validate_base(raw, Some(0xFEE0_0000)).unwrap();
        assert_eq!(b.address(), 0xFEE0_0000);
        assert!(b.enabled() && b.is_bsp() && !b.x2apic());
        assert_eq!(
            validate_base(raw, Some(0xFEE0_1000)),
            Err(ApicError::BaseMismatch)
        );
        assert_eq!(
            validate_base(raw, None).map(|b| b.address()),
            Ok(0xFEE0_0000)
        );
        // Reserved bits: low byte, bit 9, bit 52+.
        for bit in [0, 3, 7, 9, 52, 63] {
            assert_eq!(
                validate_base(raw | 1 << bit, None),
                Err(ApicError::ReservedBits),
                "bit {bit}"
            );
        }
        // EXTD without EN.
        assert_eq!(
            validate_base(0xFEE0_0000 | BASE_X2APIC, None),
            Err(ApicError::InvalidState)
        );
        assert_eq!(validate_base(BASE_ENABLE, None), Err(ApicError::BadBase));
        assert!(
            validate_base(0xFEE0_0000 | BASE_ENABLE | BASE_X2APIC, None)
                .unwrap()
                .x2apic()
        );
    }

    #[test]
    fn mode_selection_never_picks_x2apic_implicitly() {
        assert_eq!(select_mode(&cpu(true, true), Mode::XApic), Ok(Mode::XApic));
        assert_eq!(
            select_mode(&cpu(true, true), Mode::X2Apic),
            Ok(Mode::X2Apic)
        );
        assert_eq!(
            select_mode(&cpu(true, false), Mode::X2Apic),
            Err(ApicError::X2apicUnsupported)
        );
        assert_eq!(
            select_mode(&cpu(false, false), Mode::XApic),
            Err(ApicError::NoApic)
        );
        assert_eq!(
            select_mode(&cpu(false, true), Mode::X2Apic),
            Err(ApicError::NoApic)
        );
    }

    #[test]
    fn xapic_enable_sets_only_the_enable_bit_and_keeps_the_base() {
        let mut msr = FakeMsr::default();
        msr.regs.insert(IA32_APIC_BASE, 0xFEE0_0000 | BASE_BSP);
        let b = configure_msr(&mut msr, Mode::XApic, Some(0xFEE0_0000)).unwrap();
        assert!(b.enabled() && !b.x2apic());
        assert_eq!(
            msr.writes,
            vec![(IA32_APIC_BASE, 0xFEE0_0000 | BASE_BSP | BASE_ENABLE)]
        );
        // Already enabled: no write at all.
        let mut msr = FakeMsr::default();
        msr.regs.insert(IA32_APIC_BASE, 0xFEE0_0000 | BASE_ENABLE);
        configure_msr(&mut msr, Mode::XApic, None).unwrap();
        assert!(msr.writes.is_empty());
    }

    #[test]
    fn x2apic_is_entered_via_enabled_xapic() {
        let mut msr = FakeMsr::default();
        msr.regs.insert(IA32_APIC_BASE, 0xFEE0_0000 | BASE_BSP); // globally disabled
        let b = configure_msr(&mut msr, Mode::X2Apic, None).unwrap();
        assert!(b.enabled() && b.x2apic());
        let w: Vec<u64> = msr.writes.iter().map(|w| w.1).collect();
        assert_eq!(w.len(), 2);
        assert_eq!(
            w[0] & (BASE_ENABLE | BASE_X2APIC),
            BASE_ENABLE,
            "xAPIC first"
        );
        assert_eq!(
            w[1] & (BASE_ENABLE | BASE_X2APIC),
            BASE_ENABLE | BASE_X2APIC
        );
        // Never an EXTD-without-EN write.
        assert!(
            w.iter()
                .all(|v| v & BASE_X2APIC == 0 || v & BASE_ENABLE != 0)
        );
    }

    #[test]
    fn xapic_requested_while_x2apic_is_active_is_refused_and_a_dead_msr_is_detected() {
        let mut msr = FakeMsr::default();
        msr.regs
            .insert(IA32_APIC_BASE, 0xFEE0_0000 | BASE_ENABLE | BASE_X2APIC);
        assert_eq!(
            configure_msr(&mut msr, Mode::XApic, None),
            Err(ApicError::AlreadyX2apic)
        );
        assert!(msr.writes.is_empty());
        let mut msr = FakeMsr::default();
        msr.regs.insert(IA32_APIC_BASE, 0xFEE0_0000);
        msr.ignore_writes.push(IA32_APIC_BASE);
        assert_eq!(
            configure_msr(&mut msr, Mode::XApic, None),
            Err(ApicError::WriteFailed)
        );
        // A base that disagrees with the MADT is refused before anything is written.
        let mut msr = FakeMsr::default();
        msr.regs.insert(IA32_APIC_BASE, 0xFEE0_0000);
        assert_eq!(
            configure_msr(&mut msr, Mode::XApic, Some(0xFEE0_1000)),
            Err(ApicError::BaseMismatch)
        );
        assert!(msr.writes.is_empty());
    }

    #[test]
    fn init_masks_every_lvt_clears_tpr_and_enables_with_the_spurious_vector() {
        let mut a = LocalApic::new(xapic(VER_6LVT));
        a.init(SPURIOUS_VECTOR).unwrap();
        assert!(a.is_enabled());
        let m = &a.regs_mut().0;
        for reg in [
            REG_LVT_TIMER,
            REG_LINT0,
            REG_LINT1,
            REG_LVT_ERROR,
            REG_LVT_PERF,
            REG_LVT_THERMAL,
            REG_LVT_CMCI,
        ] {
            assert_eq!(
                m.get(reg as usize),
                LVT_MASKED as u64,
                "LVT {reg:#x} must be masked"
            );
        }
        assert_eq!(m.get(REG_TPR as usize), 0);
        assert_eq!(m.get(REG_SVR as usize), 0x1FF);
        // SVR is written last, after everything is masked.
        assert_eq!(m.writes().last().unwrap().0, REG_SVR as usize);
        // The ESR write-then-read protocol was followed.
        assert!(m.log.iter().any(|e| e.0 == REG_ESR as usize && e.2));
        assert!(m.log.iter().any(|e| e.0 == REG_ESR as usize && !e.2));
    }

    #[test]
    fn init_touches_only_lvts_the_version_register_says_exist() {
        let mut a = LocalApic::new(xapic(VER_P6));
        a.init(SPURIOUS_VECTOR).unwrap();
        let m = &a.regs_mut().0;
        let touched: Vec<usize> = m.writes().iter().map(|w| w.0).collect();
        assert!(!touched.contains(&(REG_LVT_PERF as usize)));
        assert!(!touched.contains(&(REG_LVT_THERMAL as usize)));
        assert!(!touched.contains(&(REG_LVT_CMCI as usize)));
        assert!(touched.contains(&(REG_LVT_ERROR as usize)));
    }

    #[test]
    fn init_rejects_absent_hardware_and_wrong_vectors_without_writing() {
        for bad in [0xFFFF_FFFFu32, 0, 0x0003_0005] {
            let mut a = LocalApic::new(xapic(bad));
            assert_eq!(
                a.init(SPURIOUS_VECTOR),
                Err(ApicError::NoDevice),
                "{bad:#x}"
            );
            assert!(a.regs_mut().0.writes().is_empty());
            assert!(!a.is_enabled());
        }
        let mut a = LocalApic::new(xapic(VER_6LVT));
        assert_eq!(a.init(0x40), Err(ApicError::BadVector));
        assert_eq!(a.init(0xFE), Err(ApicError::BadVector));
        assert!(a.regs_mut().0.writes().is_empty());
    }

    #[test]
    fn eoi_writes_zero_to_the_eoi_register_and_ids_decode_per_mode() {
        let mut a = LocalApic::new(xapic(VER_6LVT));
        a.init(SPURIOUS_VECTOR).unwrap();
        a.regs_mut().0.log.clear();
        a.eoi();
        assert_eq!(a.regs_mut().0.writes(), vec![(REG_EOI as usize, 0)]);
        assert_eq!(a.id(), 3, "xAPIC id is bits 31:24");
    }

    #[test]
    fn x2apic_registers_map_to_msrs() {
        let mut msr = FakeMsr::default();
        msr.regs.insert(0x800 + (REG_VERSION >> 4), VER_6LVT as u64);
        msr.regs.insert(0x800 + (REG_ID >> 4), 0x1234);
        let mut a = LocalApic::new(X2ApicRegs(msr));
        assert_eq!(a.mode(), Mode::X2Apic);
        assert_eq!(a.id(), 0x1234, "x2APIC id is the full 32 bits");
        a.init(SPURIOUS_VECTOR).unwrap();
        a.eoi();
        let msr = &a.regs_mut().0;
        assert_eq!(msr.regs[&0x80F], 0x1FF, "SVR = MSR 0x80F");
        assert_eq!(msr.regs[&0x80B], 0, "EOI = MSR 0x80B");
        assert_eq!(msr.regs[&0x808], 0, "TPR = MSR 0x808");
        assert!(msr.writes.iter().all(|w| (0x800..0x840).contains(&w.0)));
        assert!(
            !msr.writes.iter().any(|w| w.0 == 0x828),
            "no ESR protocol in x2APIC mode"
        );
    }

    #[test]
    fn spurious_vector_is_recognised_never_acked_and_only_when_enabled() {
        let mut a = LocalApic::new(xapic(VER_6LVT));
        assert!(
            !InterruptController::is_spurious(&mut a, SPURIOUS_VECTOR),
            "not enabled yet"
        );
        a.init(SPURIOUS_VECTOR).unwrap();
        assert!(InterruptController::is_spurious(&mut a, SPURIOUS_VECTOR));
        assert!(!InterruptController::is_spurious(&mut a, 0x40));
        assert_eq!(a.spurious_seen(), 1);
        // Through dispatch: counted, and no EOI register write happens.
        let t = crate::irq::IrqTable::new();
        a.regs_mut().0.log.clear();
        assert_eq!(
            crate::irq::dispatch(&t, &mut a, SPURIOUS_VECTOR),
            crate::irq::Outcome::Spurious
        );
        assert!(
            a.regs_mut().0.writes().is_empty(),
            "spurious must not be EOI'd"
        );
        assert_eq!(
            InterruptController::mask(&mut a, 0x40),
            Err(ControllerError::Unsupported)
        );
    }

    #[test]
    fn disable_clears_the_enable_bit_only() {
        let mut a = LocalApic::new(xapic(VER_6LVT));
        a.init(SPURIOUS_VECTOR).unwrap();
        a.disable();
        assert!(!a.is_enabled());
        assert_eq!(a.regs_mut().0.get(REG_SVR as usize), 0xFF);
        // After disable, EOI via the controller trait is a no-op.
        a.regs_mut().0.log.clear();
        InterruptController::eoi(&mut a, 0x40);
        assert!(a.regs_mut().0.writes().is_empty());
    }

    #[test]
    fn volatile_mmio_rejects_out_of_window_and_misaligned_accesses() {
        // Never dereferences: the checks fire first (base is not a real mapping).
        let mut m = unsafe { VolatileMmio::new(0x1000, 0x20) };
        for (off, w) in [
            (0x20usize, 4usize),
            (0x1E, 4),
            (0x2, 4),
            (usize::MAX, 4),
            (0x4, 8),
        ] {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if w == 4 {
                    m.read32(off);
                } else {
                    m.read64(off);
                }
            }));
            assert!(r.is_err(), "offset {off:#x} width {w} must be refused");
        }
    }
}
