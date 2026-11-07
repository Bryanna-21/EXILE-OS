//! Fatal-exception report formatting. Output is line oriented and deterministic so a boot
//! test can match it exactly.

use crate::idt::{PageFaultError, TrapFrame, exception_name};
use core::fmt::{self, Write};

pub fn write_page_fault<W: Write>(w: &mut W, err: u64, cr2: u64) -> fmt::Result {
    let d = PageFaultError::decode(err);
    writeln!(
        w,
        "OMNI8   page-fault: {} {} {}{}{} cr2={:#018x}",
        if d.instruction_fetch {
            "fetch"
        } else if d.write {
            "write"
        } else {
            "read"
        },
        if d.present {
            "protection"
        } else {
            "not-present"
        },
        if d.user { "user" } else { "supervisor" },
        if d.reserved_bit { " reserved-bit" } else { "" },
        if d.protection_key { " pkey" } else { "" },
        cr2
    )
}

pub fn write_trap_report<W: Write>(w: &mut W, f: &TrapFrame, cr2: u64, cr3: u64) -> fmt::Result {
    writeln!(
        w,
        "OMNI8 FATAL EXCEPTION vector={} name={} err={:#018x}",
        f.vector,
        exception_name(f.vector),
        f.error
    )?;
    if f.vector == 14 {
        write_page_fault(w, f.error, cr2)?;
    }
    writeln!(
        w,
        "OMNI8   rip={:#018x} cs={:#06x} ring{} rflags={:#018x} rsp={:#018x} ss={:#06x}",
        f.rip,
        f.cs,
        f.cs & 3,
        f.rflags,
        f.rsp,
        f.ss
    )?;
    writeln!(
        w,
        "OMNI8   rax={:#018x} rbx={:#018x} rcx={:#018x} rdx={:#018x}",
        f.rax, f.rbx, f.rcx, f.rdx
    )?;
    writeln!(
        w,
        "OMNI8   rsi={:#018x} rdi={:#018x} rbp={:#018x} cr3={:#018x}",
        f.rsi, f.rdi, f.rbp, cr3
    )?;
    writeln!(
        w,
        "OMNI8   r8={:#018x} r9={:#018x} r10={:#018x} r11={:#018x}",
        f.r8, f.r9, f.r10, f.r11
    )?;
    writeln!(
        w,
        "OMNI8   r12={:#018x} r13={:#018x} r14={:#018x} r15={:#018x}",
        f.r12, f.r13, f.r14, f.r15
    )
}
