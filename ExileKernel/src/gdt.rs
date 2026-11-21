//! x86-64 GDT and 64-bit TSS encoding and validation. SPDX-License-Identifier: Apache-2.0
//!
//! This module only *builds and validates* descriptor data. It contains no `lgdt`, segment
//! reload or `ltr`; those privileged steps are added together with their emulator tests, so
//! nothing here is claimed to load or run on hardware.
//!
//! Requirements the loader must honour (recorded here so they are not lost):
//! * The GDT must live in **writable** memory: `ltr` sets the TSS descriptor's busy bit in
//!   memory, so a GDT in `.rodata` would fault. (The bootstrap GDT in `entry.S` never loads a TSS.)
//! * `ltr` may be executed once per CPU per descriptor; a second `ltr` on a busy TSS raises #GP.
//! * Addresses are validated as canonical for 4-level paging (48-bit). The bootstrap does not
//!   set CR4.LA57; 5-level paging would need a wider check.
//! * Scheduling uses explicit software context switching. The TSS exists only for RSP0 and the IST.
#![allow(dead_code)]

/// Selector layout. User data precedes user code so SYSRET (STAR[63:48] = `0x10 | 3`) yields
/// SS = `0x18 | 3` and CS = `0x20 | 3`; SYSCALL (STAR[47:32] = `0x08`) yields CS = `0x08`, SS = `0x10`.
pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_DATA: u16 = 0x18 | 3;
pub const USER_CODE: u16 = 0x20 | 3;
pub const TSS_SELECTOR: u16 = 0x28;
/// Null, kernel code/data, user data/code, and a 16-byte TSS descriptor taking two slots.
pub const GDT_ENTRIES: usize = 7;

const ACCESS_PRESENT: u64 = 0x80;
const ACCESS_NONSYSTEM: u64 = 0x10;
const ACCESS_EXEC: u64 = 0x08;
const ACCESS_RW: u64 = 0x02;
const FLAG_GRANULARITY: u64 = 0x8;
const FLAG_DEFAULT32: u64 = 0x4; // D/B: must be clear for 64-bit code segments
const FLAG_LONG: u64 = 0x2;

const fn descriptor(access: u64, flags: u64) -> u64 {
    // Base is ignored in 64-bit mode for code/data. Limit is 0xFFFFF pages, also ignored in long
    // mode but kept identical to the bootstrap GDT so behaviour matches after the swap.
    0xffff | (access << 40) | (0xf << 48) | (flags << 52)
}

/// 64-bit code segment (L=1, D=0). `dpl` is masked to 0..=3.
pub const fn code64(dpl: u8) -> u64 {
    descriptor(
        ACCESS_PRESENT | ACCESS_NONSYSTEM | ACCESS_EXEC | ACCESS_RW | (((dpl & 3) as u64) << 5),
        FLAG_GRANULARITY | FLAG_LONG,
    )
}

/// Flat read/write data segment. `dpl` is masked to 0..=3.
pub const fn data(dpl: u8) -> u64 {
    descriptor(
        ACCESS_PRESENT | ACCESS_NONSYSTEM | ACCESS_RW | (((dpl & 3) as u64) << 5),
        FLAG_GRANULARITY | FLAG_DEFAULT32,
    )
}

/// Canonical for 48-bit virtual addresses.
pub const fn is_canonical(addr: u64) -> bool {
    (((addr as i64) << 16) >> 16) as u64 == addr
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorError {
    NonCanonical,
    LimitTooSmall,
    LimitTooLarge,
    ZeroStack,
    MisalignedStack,
    BadIstIndex,
}

/// 64-bit Task State Segment (Intel SDM Vol. 3, 7.7). No I/O permission bitmap is provided:
/// `iomap_base` equals the TSS size, which is at or beyond the limit.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Tss {
    _reserved0: u32,
    rsp: [u64; 3],
    _reserved1: u64,
    ist: [u64; 7],
    _reserved2: u64,
    _reserved3: u16,
    iomap_base: u16,
}

pub const TSS_SIZE: usize = core::mem::size_of::<Tss>();
/// Segment limit for a TSS with no I/O bitmap (size - 1).
pub const TSS_LIMIT: u32 = (TSS_SIZE - 1) as u32;
const _: () = assert!(TSS_SIZE == 104);
const _: () = assert!(core::mem::offset_of!(Tss, rsp) == 4);
const _: () = assert!(core::mem::offset_of!(Tss, ist) == 36);
const _: () = assert!(core::mem::offset_of!(Tss, iomap_base) == 102);

fn validate_stack_top(top: u64) -> Result<u64, DescriptorError> {
    if top == 0 {
        Err(DescriptorError::ZeroStack)
    } else if top % 16 != 0 {
        Err(DescriptorError::MisalignedStack)
    } else if !is_canonical(top) {
        Err(DescriptorError::NonCanonical)
    } else {
        Ok(top)
    }
}

impl Tss {
    pub const fn new() -> Self {
        Self {
            _reserved0: 0,
            rsp: [0; 3],
            _reserved1: 0,
            ist: [0; 7],
            _reserved2: 0,
            _reserved3: 0,
            iomap_base: TSS_SIZE as u16,
        }
    }

    /// Stack the CPU loads on a ring 3 to ring 0 transition. `top` is one past the highest
    /// usable byte, 16-byte aligned. Stack memory must stay mapped and writable.
    pub fn set_rsp0(&mut self, top: u64) -> Result<(), DescriptorError> {
        self.rsp[0] = validate_stack_top(top)?;
        Ok(())
    }

    /// Interrupt Stack Table slot `n` (1..=7, matching the IDT gate's IST field).
    pub fn set_ist(&mut self, n: u8, top: u64) -> Result<(), DescriptorError> {
        if !(1..=7).contains(&n) {
            return Err(DescriptorError::BadIstIndex);
        }
        self.ist[usize::from(n) - 1] = validate_stack_top(top)?;
        Ok(())
    }

    pub fn rsp0(&self) -> u64 {
        let rsp = self.rsp;
        rsp[0]
    }

    pub fn ist(&self, n: u8) -> Option<u64> {
        if !(1..=7).contains(&n) {
            return None;
        }
        let ist = self.ist;
        Some(ist[usize::from(n) - 1])
    }

    pub fn iomap_base(&self) -> u16 {
        self.iomap_base
    }
}

/// 16-byte system-segment descriptor for an available 64-bit TSS (type 0x9, DPL 0, present).
pub fn tss_descriptor(base: u64, limit: u32) -> Result<[u64; 2], DescriptorError> {
    if !is_canonical(base) {
        return Err(DescriptorError::NonCanonical);
    }
    if limit < TSS_LIMIT {
        return Err(DescriptorError::LimitTooSmall);
    }
    if limit > 0xf_ffff {
        return Err(DescriptorError::LimitTooLarge);
    }
    let limit = u64::from(limit);
    let low = (limit & 0xffff)
        | ((base & 0xff_ffff) << 16)
        | (0x89u64 << 40)
        | ((limit >> 16) << 48)
        | (((base >> 24) & 0xff) << 56);
    Ok([low, base >> 32])
}

/// Hardware table pointer for `lgdt`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Gdtr {
    pub limit: u16,
    pub base: u64,
}
const _: () = assert!(core::mem::size_of::<Gdtr>() == 10);

#[repr(C, align(16))]
pub struct Gdt {
    entries: [u64; GDT_ENTRIES],
}
const _: () = assert!(core::mem::size_of::<Gdt>() == 64);

impl Gdt {
    /// Builds the table for a TSS at `tss_base`. Selector order is fixed by the `*_CODE`,
    /// `*_DATA` and `TSS_SELECTOR` constants.
    pub fn new(tss_base: u64, tss_limit: u32) -> Result<Self, DescriptorError> {
        let tss = tss_descriptor(tss_base, tss_limit)?;
        Ok(Self {
            entries: [0, code64(0), data(0), data(3), code64(3), tss[0], tss[1]],
        })
    }

    pub fn entry(&self, index: usize) -> Option<u64> {
        self.entries.get(index).copied()
    }

    /// Pointer describing this table. The table must not move or be freed while loaded.
    pub fn pointer(&self) -> Gdtr {
        Gdtr {
            // Valid GDT limit is size - 1; 56 used bytes, padding is outside the limit.
            limit: (GDT_ENTRIES * 8 - 1) as u16,
            base: self as *const Self as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_bootstrap_gdt_in_entry_s() {
        // entry.S: .quad 0, 0x00af9a000000ffff, 0x00cf92000000ffff
        assert_eq!(code64(0), 0x00af_9a00_0000_ffff);
        assert_eq!(data(0), 0x00cf_9200_0000_ffff);
    }

    #[test]
    fn user_descriptors_have_dpl3() {
        assert_eq!(code64(3), 0x00af_fa00_0000_ffff);
        assert_eq!(data(3), 0x00cf_f200_0000_ffff);
        assert_eq!(code64(7), code64(3), "dpl is masked to two bits");
    }

    #[test]
    fn code_segment_is_long_mode_not_32bit() {
        let flags = (code64(0) >> 52) & 0xf;
        assert_eq!(flags & 0x2, 0x2, "L set");
        assert_eq!(flags & 0x4, 0, "D must be clear when L is set");
    }

    #[test]
    fn selector_layout_matches_syscall_sysret_rules() {
        assert_eq!(KERNEL_DATA, KERNEL_CODE + 8);
        let star_user_base = 0x10 | 3;
        assert_eq!(USER_DATA, star_user_base + 8);
        assert_eq!(USER_CODE, star_user_base + 16);
        assert_eq!(USER_CODE & 3, 3);
        assert_eq!(TSS_SELECTOR & 3, 0);
        assert_eq!(usize::from(TSS_SELECTOR >> 3), 5);
        assert_eq!(usize::from(TSS_SELECTOR >> 3) + 2, GDT_ENTRIES);
    }

    #[test]
    fn tss_layout() {
        assert_eq!(TSS_SIZE, 104);
        assert_eq!(TSS_LIMIT, 103);
        let t = Tss::new();
        assert_eq!(t.iomap_base(), 104);
        assert_eq!(t.rsp0(), 0);
        assert!((1..=7).all(|n| t.ist(n) == Some(0)));
    }

    #[test]
    fn tss_stack_configuration() {
        let mut t = Tss::new();
        t.set_rsp0(0xffff_8000_0010_0000).unwrap();
        t.set_ist(1, 0xffff_8000_0020_0000).unwrap();
        t.set_ist(7, 0xffff_8000_0030_0000).unwrap();
        assert_eq!(t.rsp0(), 0xffff_8000_0010_0000);
        assert_eq!(t.ist(1), Some(0xffff_8000_0020_0000));
        assert_eq!(t.ist(7), Some(0xffff_8000_0030_0000));
        assert_eq!(t.ist(2), Some(0), "unconfigured slots stay zero");
        assert_eq!(t.ist(0), None);
        assert_eq!(t.ist(8), None);
    }

    #[test]
    fn tss_rejects_bad_stacks_without_modifying_state() {
        let mut t = Tss::new();
        assert_eq!(t.set_rsp0(0), Err(DescriptorError::ZeroStack));
        assert_eq!(t.set_rsp0(0x1008), Err(DescriptorError::MisalignedStack));
        assert_eq!(
            t.set_rsp0(0x0000_8000_0000_0000),
            Err(DescriptorError::NonCanonical)
        );
        assert_eq!(t.set_ist(0, 0x1000), Err(DescriptorError::BadIstIndex));
        assert_eq!(t.set_ist(8, 0x1000), Err(DescriptorError::BadIstIndex));
        assert_eq!(t.rsp0(), 0);
        assert!((1..=7).all(|n| t.ist(n) == Some(0)));
    }

    #[test]
    fn canonical_addresses() {
        assert!(is_canonical(0));
        assert!(is_canonical(0x0000_7fff_ffff_ffff));
        assert!(!is_canonical(0x0000_8000_0000_0000));
        assert!(!is_canonical(0xffff_7fff_ffff_ffff));
        assert!(is_canonical(0xffff_8000_0000_0000));
        assert!(is_canonical(u64::MAX));
    }

    #[test]
    fn tss_descriptor_encoding() {
        let d = tss_descriptor(0xffff_8000_1234_5678, 103).unwrap();
        assert_eq!(d[0], 0x1200_8934_5678_0067);
        assert_eq!(d[1], 0x0000_0000_ffff_8000);
    }

    #[test]
    fn tss_descriptor_type_is_available_64bit_tss() {
        let d = tss_descriptor(0x1000, 103).unwrap();
        let access = (d[0] >> 40) & 0xff;
        assert_eq!(access, 0x89, "present, DPL 0, system, type 9 (not busy)");
        assert_eq!(access & 0x10, 0, "S must be clear for a system segment");
        assert_eq!(d[1] >> 32, 0, "upper reserved dword must be zero");
    }

    #[test]
    fn tss_descriptor_limit_high_bits() {
        let d = tss_descriptor(0x1000, 0xf_ffff).unwrap();
        assert_eq!(d[0] & 0xffff, 0xffff);
        assert_eq!((d[0] >> 48) & 0xf, 0xf);
    }

    #[test]
    fn tss_descriptor_validation() {
        assert_eq!(
            tss_descriptor(0x1000, 102),
            Err(DescriptorError::LimitTooSmall)
        );
        assert_eq!(
            tss_descriptor(0x1000, 0x10_0000),
            Err(DescriptorError::LimitTooLarge)
        );
        assert_eq!(
            tss_descriptor(0x0000_8000_0000_0000, 103),
            Err(DescriptorError::NonCanonical)
        );
    }

    #[test]
    fn gdt_contents_and_pointer() {
        let g = Gdt::new(0xffff_8000_1234_5678, TSS_LIMIT).unwrap();
        assert_eq!(g.entry(0), Some(0), "null descriptor");
        assert_eq!(g.entry(1), Some(code64(0)));
        assert_eq!(g.entry(2), Some(data(0)));
        assert_eq!(g.entry(3), Some(data(3)));
        assert_eq!(g.entry(4), Some(code64(3)));
        assert_eq!(g.entry(5), Some(0x1200_8934_5678_0067));
        assert_eq!(g.entry(6), Some(0xffff_8000));
        assert_eq!(g.entry(7), None);
        let p = g.pointer();
        assert_eq!({ p.limit }, 55);
        assert_eq!({ p.base } % 16, 0, "GDT is 16-byte aligned");
        assert_eq!({ p.base }, &g as *const Gdt as u64);
    }

    #[test]
    fn gdt_rejects_invalid_tss() {
        assert!(Gdt::new(0x0000_8000_0000_0000, TSS_LIMIT).is_err());
        assert!(Gdt::new(0x1000, 10).is_err());
    }
}
