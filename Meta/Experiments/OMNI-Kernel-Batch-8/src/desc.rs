//! GDT and TSS: descriptor encoders, table layout, and validation.
//!
//! Layout (selectors), chosen to match Batch 7's SYSCALL/SYSRET assumptions
//! (`omni_kernel7::msr`): STAR[63:48] = 0x10 gives SYSRET CS = 0x23 and SS = 0x1B.
//!
//! | index | selector | descriptor |
//! |---|---|---|
//! | 0 | 0x00 | null |
//! | 1 | 0x08 | kernel code, 64-bit |
//! | 2 | 0x10 | kernel data |
//! | 3 | 0x18 | user data (selector 0x1B with RPL 3) |
//! | 4 | 0x20 | user code, 64-bit (selector 0x23 with RPL 3) |
//! | 5-6 | 0x28 | TSS (16-byte system descriptor) |

use crate::is_canonical;
use omni_kernel7::msr;

pub const SEL_NULL: u16 = 0x00;
pub const SEL_KERNEL_CODE: u16 = 0x08;
pub const SEL_KERNEL_DATA: u16 = 0x10;
/// User data selector including RPL 3.
pub const SEL_USER_DATA: u16 = 0x1B;
/// User code selector including RPL 3.
pub const SEL_USER_CODE: u16 = 0x23;
pub const SEL_TSS: u16 = 0x28;

pub const GDT_ENTRIES: usize = 7;
/// `lgdt` limit: size in bytes minus one.
pub const GDT_LIMIT: u16 = (GDT_ENTRIES * 8 - 1) as u16;

pub const IST_FATAL: usize = 1;
pub const IST_DOUBLE_FAULT: usize = 2;
pub const IST_NMI: usize = 3;
pub const IST_MACHINE_CHECK: usize = 4;
pub const IST_IRQ: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DescError {
    /// A stack or base address was zero.
    Null,
    /// Address is not canonical.
    NonCanonical,
    /// Address does not meet the required alignment.
    Misaligned,
    /// A table entry differs from the layout this kernel requires.
    BadEntry(usize),
    /// Selector arithmetic does not match the SYSCALL/SYSRET layout.
    SelectorLayout,
}

/// 64-bit available-TSS system descriptor (two quadwords), DPL 0, present.
pub const fn tss_descriptor(base: u64, limit: u32) -> [u64; 2] {
    let low = (limit as u64 & 0xFFFF)
        | ((base & 0xFF_FFFF) << 16)
        | (0x89u64 << 40)
        | (((limit as u64 >> 16) & 0xF) << 48)
        | (((base >> 24) & 0xFF) << 56);
    [low, base >> 32]
}

#[repr(C, align(8))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Gdt {
    pub entries: [u64; GDT_ENTRIES],
}

impl Gdt {
    /// Build the table for a TSS at `tss_base` (8-aligned, canonical, non-zero).
    pub fn new(tss_base: u64) -> Result<Gdt, DescError> {
        if tss_base == 0 {
            return Err(DescError::Null);
        }
        if !is_canonical(tss_base) {
            return Err(DescError::NonCanonical);
        }
        if tss_base % 8 != 0 {
            return Err(DescError::Misaligned);
        }
        let t = tss_descriptor(tss_base, TSS_SIZE as u32 - 1);
        let g = Gdt {
            entries: [
                0,
                msr::GDT_KERNEL_CODE64,
                msr::GDT_KERNEL_DATA,
                msr::GDT_USER_DATA,
                msr::GDT_USER_CODE64,
                t[0],
                t[1],
            ],
        };
        g.validate(tss_base)?;
        Ok(g)
    }

    /// Check an arbitrary table image (for example one read back from memory).
    pub fn validate(&self, tss_base: u64) -> Result<(), DescError> {
        validate_entries(&self.entries, tss_base)
    }
}

/// Selector layout must match the SYSCALL/SYSRET derivation in Batch 7.
pub fn validate_selector_layout() -> Result<(), DescError> {
    let (cs, ss) = msr::sysret_selectors(msr::SYSRET_BASE);
    let ok = cs == SEL_USER_CODE
        && ss == SEL_USER_DATA
        && msr::KERNEL_CS == SEL_KERNEL_CODE
        && msr::KERNEL_SS == SEL_KERNEL_DATA
        && (SEL_USER_DATA & 3) == 3
        && (SEL_USER_CODE & 3) == 3
        && SEL_TSS == 5 * 8;
    if ok {
        Ok(())
    } else {
        Err(DescError::SelectorLayout)
    }
}

pub fn validate_entries(e: &[u64], tss_base: u64) -> Result<(), DescError> {
    validate_selector_layout()?;
    if e.len() != GDT_ENTRIES {
        return Err(DescError::BadEntry(e.len()));
    }
    let expect = [
        (0usize, 0u64),
        (1, msr::GDT_KERNEL_CODE64),
        (2, msr::GDT_KERNEL_DATA),
        (3, msr::GDT_USER_DATA),
        (4, msr::GDT_USER_CODE64),
    ];
    for (i, v) in expect {
        if e[i] != v {
            return Err(DescError::BadEntry(i));
        }
    }
    let t = tss_descriptor(tss_base, TSS_SIZE as u32 - 1);
    if e[5] != t[0] {
        return Err(DescError::BadEntry(5));
    }
    if e[6] != t[1] {
        return Err(DescError::BadEntry(6));
    }
    // Defence in depth: decode the access bytes rather than trusting equality alone.
    let access = |v: u64| (v >> 40) as u8;
    let flags = |v: u64| ((v >> 52) & 0xF) as u8;
    // kernel code: present, DPL0, code, readable; L=1, D=0
    if access(e[1]) != 0x9A || flags(e[1]) & 0b0110 != 0b0010 {
        return Err(DescError::BadEntry(1));
    }
    if access(e[2]) != 0x92 {
        return Err(DescError::BadEntry(2));
    }
    if access(e[3]) != 0xF2 {
        return Err(DescError::BadEntry(3));
    }
    if access(e[4]) != 0xFA || flags(e[4]) & 0b0110 != 0b0010 {
        return Err(DescError::BadEntry(4));
    }
    // TSS: present, DPL 0, type 9 (available 64-bit TSS)
    if access(e[5]) != 0x89 {
        return Err(DescError::BadEntry(5));
    }
    Ok(())
}

pub const TSS_SIZE: usize = 104;

/// 64-bit Task State Segment. No I/O permission bitmap: `iomap_base` points at the limit,
/// so every port access from CPL 3 faults.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub struct Tss {
    reserved0: u32,
    pub rsp: [u64; 3],
    reserved1: u64,
    pub ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    pub iomap_base: u16,
}

impl Tss {
    /// `rsp0` is the kernel stack top for ring 3 -> ring 0 transitions; `ist[i]` is the top
    /// of IST stack `i + 1` (zero means unused). All must be 16-byte aligned and canonical.
    pub fn new(rsp0: u64, ist: [u64; 7]) -> Result<Tss, DescError> {
        let check = |v: u64, required: bool| -> Result<(), DescError> {
            if v == 0 {
                return if required {
                    Err(DescError::Null)
                } else {
                    Ok(())
                };
            }
            if !is_canonical(v) {
                return Err(DescError::NonCanonical);
            }
            if v % 16 != 0 {
                return Err(DescError::Misaligned);
            }
            Ok(())
        };
        check(rsp0, true)?;
        for &s in &ist {
            check(s, false)?;
        }
        Ok(Tss {
            reserved0: 0,
            rsp: [rsp0, 0, 0],
            reserved1: 0,
            ist,
            reserved2: 0,
            reserved3: 0,
            iomap_base: TSS_SIZE as u16,
        })
    }

    pub fn rsp0(&self) -> u64 {
        self.rsp[0]
    }
    pub fn set_rsp0(&mut self, v: u64) -> Result<(), DescError> {
        if v == 0 {
            return Err(DescError::Null);
        }
        if !is_canonical(v) {
            return Err(DescError::NonCanonical);
        }
        if v % 16 != 0 {
            return Err(DescError::Misaligned);
        }
        self.rsp[0] = v;
        Ok(())
    }
}
