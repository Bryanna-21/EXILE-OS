//! x86-64 IDT gate encoding and exception-vector metadata. SPDX-License-Identifier: Apache-2.0
//!
//! Encoders and policy tables only: no `lidt`, no handler stubs, no interrupt enabling. Those
//! need emulator validation and arrive in a later batch. Selectors are plain `u16` so this file
//! stands alone; use the constants in `gdt.rs`.
#![allow(dead_code)]

/// Handler entry points must be 16-byte aligned. This is kernel policy for our stubs (emitted
/// with `.p2align 4`), not a hardware requirement.
pub const HANDLER_ALIGN: u64 = 16;
pub const VECTORS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateKind {
    /// Clears IF on entry (type 0xE).
    Interrupt,
    /// Leaves IF unchanged on entry (type 0xF).
    Trap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateError {
    NullHandler,
    NonCanonicalHandler,
    MisalignedHandler,
    NullSelector,
    BadPrivilege,
    BadIst,
    BadVector,
}

fn is_canonical(addr: u64) -> bool {
    (((addr as i64) << 16) >> 16) as u64 == addr
}

/// 16-byte long-mode gate descriptor.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}
const _: () = assert!(core::mem::size_of::<Gate>() == 16);

impl Gate {
    /// Not-present gate: delivering this vector raises #NP/#GP rather than jumping anywhere.
    pub const fn missing() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            attributes: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    /// Builds a present gate. `ist` is 0 (use the current stack rules) or 1..=7 (TSS IST slot).
    /// `selector` must be a non-null code selector (the caller supplies the kernel code selector).
    pub fn new(
        handler: u64,
        selector: u16,
        kind: GateKind,
        dpl: u8,
        ist: u8,
    ) -> Result<Self, GateError> {
        if handler == 0 {
            return Err(GateError::NullHandler);
        }
        if !is_canonical(handler) {
            return Err(GateError::NonCanonicalHandler);
        }
        if handler % HANDLER_ALIGN != 0 {
            return Err(GateError::MisalignedHandler);
        }
        if selector >> 3 == 0 {
            return Err(GateError::NullSelector);
        }
        if dpl > 3 {
            return Err(GateError::BadPrivilege);
        }
        if ist > 7 {
            return Err(GateError::BadIst);
        }
        let gate_type: u8 = match kind {
            GateKind::Interrupt => 0xe,
            GateKind::Trap => 0xf,
        };
        Ok(Self {
            offset_low: handler as u16,
            selector,
            ist,
            attributes: 0x80 | (dpl << 5) | gate_type,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        })
    }

    pub fn present(&self) -> bool {
        self.attributes & 0x80 != 0
    }

    pub fn handler(&self) -> u64 {
        u64::from(self.offset_low)
            | (u64::from(self.offset_mid) << 16)
            | (u64::from(self.offset_high) << 32)
    }

    pub fn selector(&self) -> u16 {
        self.selector
    }

    pub fn ist(&self) -> u8 {
        self.ist & 7
    }

    pub fn dpl(&self) -> u8 {
        (self.attributes >> 5) & 3
    }

    pub fn kind(&self) -> Option<GateKind> {
        match self.attributes & 0xf {
            0xe => Some(GateKind::Interrupt),
            0xf => Some(GateKind::Trap),
            _ => None,
        }
    }
}

/// Hardware table pointer for `lidt`.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Idtr {
    pub limit: u16,
    pub base: u64,
}
const _: () = assert!(core::mem::size_of::<Idtr>() == 10);

#[repr(C, align(16))]
pub struct Idt {
    gates: [Gate; VECTORS],
}
const _: () = assert!(core::mem::size_of::<Idt>() == 4096);

impl Idt {
    /// Every vector starts not-present; installation is explicit.
    pub const fn new() -> Self {
        Self {
            gates: [Gate::missing(); VECTORS],
        }
    }

    pub fn set(&mut self, vector: usize, gate: Gate) -> Result<(), GateError> {
        *self.gates.get_mut(vector).ok_or(GateError::BadVector)? = gate;
        Ok(())
    }

    pub fn get(&self, vector: usize) -> Option<&Gate> {
        self.gates.get(vector)
    }

    /// True when every CPU exception vector (see `exception_info`) that is architecturally
    /// defined is present. Gate this before enabling interrupts.
    pub fn exceptions_installed(&self) -> bool {
        (0..32).all(|v| match exception_info(v as u8) {
            Some(i) if i.class != Class::Reserved => self.gates[v].present(),
            _ => true,
        })
    }

    pub fn pointer(&self) -> Idtr {
        Idtr {
            limit: (core::mem::size_of::<Self>() - 1) as u16,
            base: self as *const Self as u64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Fault,
    Trap,
    Abort,
    /// Not caused by the instruction stream (NMI).
    Interrupt,
    /// Fault or trap depending on the cause (#DB).
    FaultOrTrap,
    Reserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExceptionInfo {
    pub vector: u8,
    pub mnemonic: &'static str,
    pub name: &'static str,
    pub class: Class,
    /// CPU pushes an error code for this vector. Stubs for the others must push a dummy.
    pub error_code: bool,
}

const fn e(
    vector: u8,
    mnemonic: &'static str,
    name: &'static str,
    class: Class,
    error_code: bool,
) -> ExceptionInfo {
    ExceptionInfo {
        vector,
        mnemonic,
        name,
        class,
        error_code,
    }
}

/// Vectors 0..=31 per Intel SDM Vol. 3 Table 6-1 and AMD APM Vol. 2 Table 8-1. #HV, #VC and
/// #SX are AMD/virtualization-specific. Vector 9 is legacy and not generated by modern CPUs.
pub const EXCEPTIONS: [ExceptionInfo; 32] = [
    e(0, "#DE", "Divide Error", Class::Fault, false),
    e(1, "#DB", "Debug", Class::FaultOrTrap, false),
    e(2, "NMI", "Non-Maskable Interrupt", Class::Interrupt, false),
    e(3, "#BP", "Breakpoint", Class::Trap, false),
    e(4, "#OF", "Overflow", Class::Trap, false),
    e(5, "#BR", "Bound Range Exceeded", Class::Fault, false),
    e(6, "#UD", "Invalid Opcode", Class::Fault, false),
    e(7, "#NM", "Device Not Available", Class::Fault, false),
    e(8, "#DF", "Double Fault", Class::Abort, true),
    e(
        9,
        "---",
        "Coprocessor Segment Overrun (legacy)",
        Class::Fault,
        false,
    ),
    e(10, "#TS", "Invalid TSS", Class::Fault, true),
    e(11, "#NP", "Segment Not Present", Class::Fault, true),
    e(12, "#SS", "Stack-Segment Fault", Class::Fault, true),
    e(13, "#GP", "General Protection", Class::Fault, true),
    e(14, "#PF", "Page Fault", Class::Fault, true),
    e(15, "---", "Reserved", Class::Reserved, false),
    e(
        16,
        "#MF",
        "x87 Floating-Point Exception",
        Class::Fault,
        false,
    ),
    e(17, "#AC", "Alignment Check", Class::Fault, true),
    e(18, "#MC", "Machine Check", Class::Abort, false),
    e(
        19,
        "#XM",
        "SIMD Floating-Point Exception",
        Class::Fault,
        false,
    ),
    e(20, "#VE", "Virtualization Exception", Class::Fault, false),
    e(
        21,
        "#CP",
        "Control Protection Exception",
        Class::Fault,
        true,
    ),
    e(22, "---", "Reserved", Class::Reserved, false),
    e(23, "---", "Reserved", Class::Reserved, false),
    e(24, "---", "Reserved", Class::Reserved, false),
    e(25, "---", "Reserved", Class::Reserved, false),
    e(26, "---", "Reserved", Class::Reserved, false),
    e(27, "---", "Reserved", Class::Reserved, false),
    e(
        28,
        "#HV",
        "Hypervisor Injection Exception",
        Class::Fault,
        false,
    ),
    e(29, "#VC", "VMM Communication Exception", Class::Fault, true),
    e(30, "#SX", "Security Exception", Class::Fault, true),
    e(31, "---", "Reserved", Class::Reserved, false),
];

pub fn exception_info(vector: u8) -> Option<&'static ExceptionInfo> {
    EXCEPTIONS.get(usize::from(vector))
}

/// True if the CPU pushes an error code for `vector`. Vectors >= 32 never do.
pub fn has_error_code(vector: u8) -> bool {
    exception_info(vector).is_some_and(|i| i.error_code)
}

/// IST slots reserved for exceptions that must not run on a possibly-corrupt stack. They map to
/// TSS `set_ist` indices.
pub const IST_DOUBLE_FAULT: u8 = 1;
pub const IST_NMI: u8 = 2;
pub const IST_MACHINE_CHECK: u8 = 3;

/// IST slot for `vector`, or 0 for the normal stack rules.
pub fn default_ist(vector: u8) -> u8 {
    match vector {
        8 => IST_DOUBLE_FAULT,
        2 => IST_NMI,
        18 => IST_MACHINE_CHECK,
        _ => 0,
    }
}

/// Minimum privilege allowed to raise the vector with a software `int`. Only #BP is reachable
/// from ring 3; everything else is hardware-raised or kernel-only.
pub fn default_dpl(vector: u8) -> u8 {
    if vector == 3 { 3 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CS: u16 = 0x08;
    const H: u64 = 0xffff_8000_dead_be00;

    #[test]
    fn gate_encoding() {
        let g = Gate::new(H, CS, GateKind::Interrupt, 0, 0).unwrap();
        assert!(g.present());
        assert_eq!(g.handler(), H);
        assert_eq!(g.selector(), CS);
        assert_eq!(g.kind(), Some(GateKind::Interrupt));
        assert_eq!(g.dpl(), 0);
        assert_eq!(g.ist(), 0);
        // Raw layout: offset[15:0], selector, ist, attrs, offset[31:16], offset[63:32], reserved.
        let raw: [u8; 16] = unsafe { core::mem::transmute(g) };
        assert_eq!(&raw[0..2], &0xbe00u16.to_le_bytes());
        assert_eq!(&raw[2..4], &CS.to_le_bytes());
        assert_eq!(raw[4], 0);
        assert_eq!(raw[5], 0x8e);
        assert_eq!(&raw[6..8], &0xdeadu16.to_le_bytes());
        assert_eq!(&raw[8..12], &0xffff_8000u32.to_le_bytes());
        assert_eq!(&raw[12..16], &[0; 4]);
    }

    #[test]
    fn trap_gate_attributes_and_dpl() {
        let g = Gate::new(H, CS, GateKind::Trap, 3, 0).unwrap();
        let raw: [u8; 16] = unsafe { core::mem::transmute(g) };
        assert_eq!(raw[5], 0xef, "P=1, DPL=3, type=0xF");
        assert_eq!(g.kind(), Some(GateKind::Trap));
        assert_eq!(g.dpl(), 3);
    }

    #[test]
    fn ist_selection() {
        for ist in 0..=7 {
            assert_eq!(
                Gate::new(H, CS, GateKind::Interrupt, 0, ist).unwrap().ist(),
                ist
            );
        }
        assert_eq!(
            Gate::new(H, CS, GateKind::Interrupt, 0, 8),
            Err(GateError::BadIst)
        );
    }

    #[test]
    fn gate_validation() {
        assert_eq!(
            Gate::new(0, CS, GateKind::Interrupt, 0, 0),
            Err(GateError::NullHandler)
        );
        assert_eq!(
            Gate::new(0x0000_8000_0000_0000, CS, GateKind::Interrupt, 0, 0),
            Err(GateError::NonCanonicalHandler)
        );
        assert_eq!(
            Gate::new(H + 8, CS, GateKind::Interrupt, 0, 0),
            Err(GateError::MisalignedHandler)
        );
        assert_eq!(
            Gate::new(H, 0, GateKind::Interrupt, 0, 0),
            Err(GateError::NullSelector)
        );
        assert_eq!(
            Gate::new(H, 3, GateKind::Interrupt, 0, 0),
            Err(GateError::NullSelector),
            "RPL bits alone are still null"
        );
        assert_eq!(
            Gate::new(H, CS, GateKind::Interrupt, 4, 0),
            Err(GateError::BadPrivilege)
        );
    }

    #[test]
    fn missing_gate_is_not_present() {
        let g = Gate::missing();
        assert!(!g.present());
        assert_eq!(g.kind(), None);
    }

    #[test]
    fn idtr_covers_exactly_256_gates() {
        let idt = Idt::new();
        let p = idt.pointer();
        assert_eq!({ p.limit }, 4095);
        assert_eq!({ p.base } % 16, 0);
        assert_eq!({ p.base }, &idt as *const Idt as u64);
    }

    #[test]
    fn idt_bounds() {
        let mut idt = Idt::new();
        let g = Gate::new(H, CS, GateKind::Interrupt, 0, 0).unwrap();
        assert_eq!(idt.set(256, g), Err(GateError::BadVector));
        assert!(idt.get(256).is_none());
        idt.set(255, g).unwrap();
        assert!(idt.get(255).unwrap().present());
        assert!(!idt.get(254).unwrap().present());
    }

    #[test]
    fn error_code_vectors_match_architecture() {
        let with: Vec<u8> = (0..=255u8).filter(|v| has_error_code(*v)).collect();
        // #DF, #TS, #NP, #SS, #GP, #PF, #AC, #CP, #VC, #SX
        assert_eq!(with, vec![8, 10, 11, 12, 13, 14, 17, 21, 29, 30]);
    }

    #[test]
    fn table_is_indexed_by_vector() {
        for (i, info) in EXCEPTIONS.iter().enumerate() {
            assert_eq!(usize::from(info.vector), i);
        }
        assert!(exception_info(32).is_none());
    }

    #[test]
    fn required_mission_exceptions_are_named() {
        for (v, m) in [
            (0, "#DE"),
            (1, "#DB"),
            (2, "NMI"),
            (3, "#BP"),
            (4, "#OF"),
            (5, "#BR"),
            (6, "#UD"),
            (7, "#NM"),
            (8, "#DF"),
            (10, "#TS"),
            (11, "#NP"),
            (12, "#SS"),
            (13, "#GP"),
            (14, "#PF"),
            (16, "#MF"),
            (17, "#AC"),
            (18, "#MC"),
            (19, "#XM"),
            (20, "#VE"),
        ] {
            assert_eq!(exception_info(v).unwrap().mnemonic, m);
        }
    }

    #[test]
    fn critical_exceptions_use_dedicated_stacks() {
        assert_eq!(default_ist(8), IST_DOUBLE_FAULT);
        assert_eq!(default_ist(2), IST_NMI);
        assert_eq!(default_ist(18), IST_MACHINE_CHECK);
        let slots = [IST_DOUBLE_FAULT, IST_NMI, IST_MACHINE_CHECK];
        assert!(slots.iter().all(|s| (1..=7).contains(s)));
        assert!(slots[0] != slots[1] && slots[1] != slots[2] && slots[0] != slots[2]);
        assert_eq!(default_ist(14), 0);
    }

    #[test]
    fn only_breakpoint_is_user_reachable() {
        assert_eq!(default_dpl(3), 3);
        assert!((0..=255u8).filter(|v| *v != 3).all(|v| default_dpl(v) == 0));
    }

    #[test]
    fn exceptions_installed_gate_blocks_until_complete() {
        let mut idt = Idt::new();
        assert!(!idt.exceptions_installed());
        let g = Gate::new(H, CS, GateKind::Interrupt, 0, 0).unwrap();
        for v in 0..32 {
            let reserved = EXCEPTIONS[v].class == Class::Reserved;
            if !reserved {
                assert!(!idt.exceptions_installed(), "vector {v} still missing");
                idt.set(v, g).unwrap();
            }
        }
        assert!(
            idt.exceptions_installed(),
            "reserved vectors need no handler"
        );
    }
}
