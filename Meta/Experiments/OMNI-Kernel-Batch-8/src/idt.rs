//! IDT: gate encoding, vector metadata, the trap frame layout shared with `isr.S`, and
//! page-fault error decoding.

use crate::desc::{
    DescError, IST_DOUBLE_FAULT, IST_FATAL, IST_IRQ, IST_MACHINE_CHECK, IST_NMI, SEL_KERNEL_CODE,
};
use crate::is_canonical;

pub const IDT_ENTRIES: usize = 256;
pub const IDT_LIMIT: u16 = (IDT_ENTRIES * 16 - 1) as u16;
pub const EXCEPTION_VECTORS: usize = 32;
/// First vector of the remapped master PIC (IRQ0 = timer).
pub const IRQ_BASE: u8 = 0x20;
pub const TIMER_VECTOR: u8 = IRQ_BASE;
/// Software interrupt vector reserved for the user-mode test return path. DPL 3 gate.
pub const VECTOR_USER_TEST: u8 = 0x81;

pub const EXCEPTION_NAMES: [&str; EXCEPTION_VECTORS] = [
    "#DE divide error",
    "#DB debug",
    "NMI",
    "#BP breakpoint",
    "#OF overflow",
    "#BR bound range",
    "#UD invalid opcode",
    "#NM device not available",
    "#DF double fault",
    "coprocessor segment overrun",
    "#TS invalid TSS",
    "#NP segment not present",
    "#SS stack fault",
    "#GP general protection",
    "#PF page fault",
    "reserved (15)",
    "#MF x87 floating point",
    "#AC alignment check",
    "#MC machine check",
    "#XM SIMD floating point",
    "#VE virtualization",
    "#CP control protection",
    "reserved (22)",
    "reserved (23)",
    "reserved (24)",
    "reserved (25)",
    "reserved (26)",
    "reserved (27)",
    "#HV hypervisor injection",
    "#VC VMM communication",
    "#SX security",
    "reserved (31)",
];

pub fn exception_name(vector: u64) -> &'static str {
    match EXCEPTION_NAMES.get(vector as usize) {
        Some(n) => n,
        None => "external interrupt",
    }
}

/// Does the CPU push an error code for this exception vector?
pub const fn has_error_code(vector: usize) -> bool {
    matches!(vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30)
}

/// IST stack used by a vector. Exceptions share the fatal stack except the ones that can
/// arrive on a corrupt stack; IRQs use their own so the red zone of interrupted code is
/// never overwritten regardless of how the interrupted code was compiled.
pub const fn ist_for_vector(vector: usize) -> usize {
    match vector {
        8 => IST_DOUBLE_FAULT,
        2 => IST_NMI,
        18 => IST_MACHINE_CHECK,
        0..=31 => IST_FATAL,
        _ => IST_IRQ,
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl IdtEntry {
    pub const MISSING: IdtEntry = IdtEntry {
        offset_low: 0,
        selector: 0,
        ist: 0,
        type_attr: 0,
        offset_mid: 0,
        offset_high: 0,
        zero: 0,
    };

    /// 64-bit interrupt gate (type 0xE: clears IF on entry).
    pub fn interrupt_gate(
        handler: u64,
        selector: u16,
        ist: usize,
        dpl: u8,
    ) -> Result<IdtEntry, DescError> {
        if handler == 0 {
            return Err(DescError::Null);
        }
        if !is_canonical(handler) {
            return Err(DescError::NonCanonical);
        }
        if ist > 7 || dpl > 3 || selector != SEL_KERNEL_CODE {
            return Err(DescError::BadEntry(ist));
        }
        Ok(IdtEntry {
            offset_low: handler as u16,
            selector,
            ist: ist as u8,
            type_attr: 0x80 | (dpl << 5) | 0x0E,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            zero: 0,
        })
    }

    pub fn is_present(&self) -> bool {
        self.type_attr & 0x80 != 0
    }
    pub fn handler(&self) -> u64 {
        (self.offset_low as u64)
            | ((self.offset_mid as u64) << 16)
            | ((self.offset_high as u64) << 32)
    }
    pub fn ist(&self) -> u8 {
        self.ist
    }
    pub fn dpl(&self) -> u8 {
        (self.type_attr >> 5) & 3
    }
    pub fn gate_type(&self) -> u8 {
        self.type_attr & 0x0F
    }
    pub fn selector(&self) -> u16 {
        self.selector
    }
}

#[repr(C, align(16))]
pub struct Idt {
    pub entries: [IdtEntry; IDT_ENTRIES],
}

impl Default for Idt {
    fn default() -> Self {
        Self::new()
    }
}

impl Idt {
    pub const fn new() -> Idt {
        Idt {
            entries: [IdtEntry::MISSING; IDT_ENTRIES],
        }
    }

    pub fn set(&mut self, vector: usize, entry: IdtEntry) -> Result<(), DescError> {
        if vector >= IDT_ENTRIES {
            return Err(DescError::BadEntry(vector));
        }
        self.entries[vector] = entry;
        Ok(())
    }

    /// Every vector 0..=31 and every vector in `extra` must be a present, kernel-selector,
    /// interrupt-type gate with a canonical non-zero handler. Absent gates elsewhere are
    /// legal (a stray interrupt then raises #NP and is fatal, never silently ignored).
    pub fn validate(&self, extra: &[usize]) -> Result<(), DescError> {
        let check = |v: usize| -> Result<(), DescError> {
            let e = &self.entries[v];
            let ok = e.is_present()
                && e.selector() == SEL_KERNEL_CODE
                && e.gate_type() == 0x0E
                && e.handler() != 0
                && is_canonical(e.handler())
                && e.ist() <= 7;
            if ok {
                Ok(())
            } else {
                Err(DescError::BadEntry(v))
            }
        };
        for v in 0..EXCEPTION_VECTORS {
            check(v)?;
        }
        for &v in extra {
            if v >= IDT_ENTRIES {
                return Err(DescError::BadEntry(v));
            }
            check(v)?;
        }
        Ok(())
    }
}

/// Register state as built by `isr.S`: fifteen general registers pushed by the stub, then
/// the vector and error code (dummy 0 where the CPU pushes none), then the CPU frame.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

pub const TRAP_FRAME_SIZE: usize = 22 * 8;

impl TrapFrame {
    /// Did the trap come from ring 3?
    pub fn from_user(&self) -> bool {
        self.cs & 3 == 3
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageFaultError {
    pub present: bool,
    pub write: bool,
    pub user: bool,
    pub reserved_bit: bool,
    pub instruction_fetch: bool,
    pub protection_key: bool,
    pub shadow_stack: bool,
}

impl PageFaultError {
    pub fn decode(code: u64) -> PageFaultError {
        PageFaultError {
            present: code & 1 != 0,
            write: code & 2 != 0,
            user: code & 4 != 0,
            reserved_bit: code & 8 != 0,
            instruction_fetch: code & 16 != 0,
            protection_key: code & 32 != 0,
            shadow_stack: code & 64 != 0,
        }
    }
}
