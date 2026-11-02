//! MSR and GDT encodings for SYSCALL/SYSRET, plus the (never executed here) init routine.
//!
//! GDT layout this batch assumes (selector: descriptor):
//!   0x08 kernel code64   0x10 kernel data   0x18 user data   0x20 user code64
//! SYSCALL loads CS = STAR[47:32], SS = that + 8. SYSRET (64-bit) loads
//! CS = STAR[63:48] + 16, SS = STAR[63:48] + 8, with RPL forced to 3. With
//! STAR[63:48] = 0x10 that gives CS = 0x23 and SS = 0x1B.
//! ExileKernel's own GDT (entry.S) has none of this: it has no user segments, no TSS.

pub const IA32_EFER: u32 = 0xC000_0080;
pub const IA32_STAR: u32 = 0xC000_0081;
pub const IA32_LSTAR: u32 = 0xC000_0082;
pub const IA32_FMASK: u32 = 0xC000_0084;
pub const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;
pub const EFER_SCE: u64 = 1;

pub const KERNEL_CS: u16 = 0x08;
pub const KERNEL_SS: u16 = 0x10;
pub const SYSRET_BASE: u16 = 0x10;
pub const USER_SS: u16 = 0x1B;
pub const USER_CS: u16 = 0x23;

pub const fn star_value(kernel_cs: u16, sysret_base: u16) -> u64 {
    ((sysret_base as u64) << 48) | ((kernel_cs as u64) << 32)
}

/// RFLAGS bits cleared on entry: TF, IF, DF, NT, AC.
pub const FMASK: u64 = 0x100 | 0x200 | 0x400 | 0x4000 | 0x4_0000;

/// Selectors SYSRET will load for a given STAR[63:48], RPL forced to 3.
pub const fn sysret_selectors(base: u16) -> (u16, u16) {
    ((base + 16) | 3, (base + 8) | 3)
}

/// 64-bit GDT descriptor with base 0 and limit 0xFFFFF.
pub const fn gdt_descriptor(access: u8, flags: u8) -> u64 {
    0xFFFF | ((access as u64) << 40) | (0xF << 48) | (((flags & 0xF) as u64) << 52)
}

pub const GDT_KERNEL_CODE64: u64 = gdt_descriptor(0x9A, 0xA);
pub const GDT_KERNEL_DATA: u64 = gdt_descriptor(0x92, 0xC);
pub const GDT_USER_DATA: u64 = gdt_descriptor(0xF2, 0xC);
pub const GDT_USER_CODE64: u64 = gdt_descriptor(0xFA, 0xA);

/// SYSRET to a non-canonical or kernel-half rip faults in ring 0 on Intel; refuse it.
pub const fn sysret_target_ok(rip: u64) -> bool {
    rip < crate::uaccess::USER_TOP
}

/// CPUID: does this CPU report SYSCALL/SYSRET in long mode? Executable on any x86-64 host.
pub fn supports_syscall() -> bool {
    use core::arch::x86_64::__cpuid;
    // CPUID is available on every x86-64 CPU.
    if __cpuid(0x8000_0000).eax < 0x8000_0001 {
        return false;
    }
    __cpuid(0x8000_0001).edx & (1 << 11) != 0
}

/// # Safety
/// Ring 0 only. Not executed in this batch (it would fault in user mode).
pub unsafe fn wrmsr(msr: u32, value: u64) {
    // SAFETY: caller contract.
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags));
    }
}

/// # Safety
/// Ring 0 only.
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: caller contract.
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

/// Enable SYSCALL on this CPU and point it at the entry stub.
///
/// # Safety
/// Ring 0 only; the GDT must match the layout above and `percpu` must outlive the CPU.
/// NOT EXECUTED in Batch 7.
pub unsafe fn init_syscall(percpu: *mut crate::syscall::PerCpu) {
    // SAFETY: caller contract.
    unsafe {
        wrmsr(IA32_EFER, rdmsr(IA32_EFER) | EFER_SCE);
        wrmsr(IA32_STAR, star_value(KERNEL_CS, SYSRET_BASE));
        wrmsr(
            IA32_LSTAR,
            crate::syscall::omni_syscall_entry as *const () as usize as u64,
        );
        wrmsr(IA32_FMASK, FMASK);
        wrmsr(IA32_KERNEL_GS_BASE, percpu as u64);
    }
}
