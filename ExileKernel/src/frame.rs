//! Interrupt/exception register frame (x86-64). SPDX-License-Identifier: Apache-2.0
//!
//! `TrapFrame` is the exact memory image built on the stack by `arch/x86_64/exceptions.S`.
//! Lowest address first:
//!
//! ```text
//! offset  field        written by
//!   0     r15 .. rax   stub common path (15 pushes; rax is nearest the vector)
//! 120     vector       per-vector stub (push $vector)
//! 128     error_code   CPU (vectors with a hardware error code) or stub (push $0)
//! 136     rip          CPU
//! 144     cs           CPU
//! 152     rflags       CPU
//! 160     rsp          CPU   (always pushed in 64-bit mode, even without a privilege change)
//! 168     ss           CPU
//! ```
//!
//! Changing this layout requires changing the assembly; the compile-time asserts below and the
//! host tests that execute the real stubs (see `traps.rs`) catch a mismatch.
#![allow(dead_code)]

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    /// Interrupt vector, 0..=255 (a larger value means the frame is corrupt).
    pub vector: u64,
    /// Hardware error code, or 0 for vectors that do not push one.
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

pub const FRAME_SIZE: usize = 176;
/// Number of general-purpose registers saved in the frame.
pub const GPR_COUNT: usize = 15;
const _: () = assert!(core::mem::size_of::<TrapFrame>() == FRAME_SIZE);
// 16-byte multiple: the stack stays aligned for the SysV call from the stub into Rust.
const _: () = assert!(FRAME_SIZE % 16 == 0);
const _: () = assert!(core::mem::align_of::<TrapFrame>() == 8);
const _: () = assert!(core::mem::offset_of!(TrapFrame, r15) == 0);
const _: () = assert!(core::mem::offset_of!(TrapFrame, rax) == 112);
const _: () = assert!(core::mem::offset_of!(TrapFrame, vector) == 120);
const _: () = assert!(core::mem::offset_of!(TrapFrame, error_code) == 128);
const _: () = assert!(core::mem::offset_of!(TrapFrame, rip) == 136);
const _: () = assert!(core::mem::offset_of!(TrapFrame, cs) == 144);
const _: () = assert!(core::mem::offset_of!(TrapFrame, rflags) == 152);
const _: () = assert!(core::mem::offset_of!(TrapFrame, rsp) == 160);
const _: () = assert!(core::mem::offset_of!(TrapFrame, ss) == 168);

pub const RFLAGS_IF: u64 = 1 << 9;
pub const RFLAGS_DF: u64 = 1 << 10;

/// Decoded page-fault error code (Intel SDM Vol. 3, Figure 4-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFaultError {
    /// false: not-present page; true: protection violation.
    pub protection_violation: bool,
    pub write: bool,
    pub user: bool,
    pub reserved_bit_set: bool,
    pub instruction_fetch: bool,
    pub protection_key: bool,
    pub shadow_stack: bool,
    pub sgx: bool,
}

impl PageFaultError {
    pub const fn decode(code: u64) -> Self {
        Self {
            protection_violation: code & 1 != 0,
            write: code & (1 << 1) != 0,
            user: code & (1 << 2) != 0,
            reserved_bit_set: code & (1 << 3) != 0,
            instruction_fetch: code & (1 << 4) != 0,
            protection_key: code & (1 << 5) != 0,
            shadow_stack: code & (1 << 6) != 0,
            sgx: code & (1 << 15) != 0,
        }
    }
}

/// Decoded selector error code, pushed by #TS, #NP, #SS and (when segment-related) #GP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectorError {
    /// Event was external to the program (e.g. hardware interrupt).
    pub external: bool,
    /// `index` refers to the IDT rather than the GDT/LDT.
    pub idt: bool,
    /// When `idt` is false: LDT (true) or GDT (false).
    pub ldt: bool,
    pub index: u16,
}

impl SelectorError {
    pub const fn decode(code: u64) -> Self {
        Self {
            external: code & 1 != 0,
            idt: code & 2 != 0,
            ldt: code & 4 != 0,
            index: ((code >> 3) & 0x1fff) as u16,
        }
    }
}

impl TrapFrame {
    /// Interrupted privilege level, from the saved CS.RPL (0 = kernel, 3 = user).
    pub const fn privilege_level(&self) -> u8 {
        (self.cs & 3) as u8
    }

    pub const fn came_from_user(&self) -> bool {
        self.privilege_level() == 3
    }

    pub const fn interrupts_were_enabled(&self) -> bool {
        self.rflags & RFLAGS_IF != 0
    }

    /// The vector as a byte, or `None` if the saved value is out of range (corrupt frame).
    pub const fn vector_u8(&self) -> Option<u8> {
        if self.vector <= 255 {
            Some(self.vector as u8)
        } else {
            None
        }
    }

    /// Page-fault error bits; `Some` only for vector 14.
    pub const fn page_fault_error(&self) -> Option<PageFaultError> {
        if self.vector == 14 {
            Some(PageFaultError::decode(self.error_code))
        } else {
            None
        }
    }

    /// Selector error; `Some` only for vectors 10, 11, 12 and 13. For #GP a zero code means the
    /// fault was not caused by a segment selector, which the caller must treat as "no selector".
    pub const fn selector_error(&self) -> Option<SelectorError> {
        match self.vector {
            10..=13 => Some(SelectorError::decode(self.error_code)),
            _ => None,
        }
    }

    /// The 15 saved general-purpose registers in frame order (r15 first, rax last).
    pub const fn gprs(&self) -> [u64; GPR_COUNT] {
        [
            self.r15, self.r14, self.r13, self.r12, self.r11, self.r10, self.r9, self.r8, self.rbp,
            self.rdi, self.rsi, self.rdx, self.rcx, self.rbx, self.rax,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered() -> TrapFrame {
        TrapFrame {
            r15: 1,
            r14: 2,
            r13: 3,
            r12: 4,
            r11: 5,
            r10: 6,
            r9: 7,
            r8: 8,
            rbp: 9,
            rdi: 10,
            rsi: 11,
            rdx: 12,
            rcx: 13,
            rbx: 14,
            rax: 15,
            vector: 16,
            error_code: 17,
            rip: 18,
            cs: 19,
            rflags: 20,
            rsp: 21,
            ss: 22,
        }
    }

    #[test]
    fn memory_image_is_22_ordered_qwords() {
        let f = numbered();
        let raw: [u64; 22] = unsafe { core::mem::transmute(f) };
        assert_eq!(raw, core::array::from_fn::<u64, 22, _>(|i| i as u64 + 1));
        assert_eq!(core::mem::size_of::<TrapFrame>(), FRAME_SIZE);
    }

    #[test]
    fn gprs_follow_push_order() {
        let g = numbered().gprs();
        assert_eq!(g.len(), GPR_COUNT);
        assert_eq!(g[0], 1, "r15 first");
        assert_eq!(g[14], 15, "rax last");
    }

    #[test]
    fn privilege_from_cs() {
        let mut f = TrapFrame::default();
        f.cs = 0x08;
        assert_eq!(f.privilege_level(), 0);
        assert!(!f.came_from_user());
        f.cs = 0x23;
        assert_eq!(f.privilege_level(), 3);
        assert!(f.came_from_user());
        f.cs = 0x2a; // RPL 2 is neither kernel nor user
        assert_eq!(f.privilege_level(), 2);
        assert!(!f.came_from_user());
    }

    #[test]
    fn interrupt_flag() {
        let mut f = TrapFrame::default();
        assert!(!f.interrupts_were_enabled());
        f.rflags = 0x202;
        assert!(f.interrupts_were_enabled());
        f.rflags = RFLAGS_DF;
        assert!(!f.interrupts_were_enabled());
    }

    #[test]
    fn vector_range() {
        let mut f = TrapFrame::default();
        f.vector = 255;
        assert_eq!(f.vector_u8(), Some(255));
        f.vector = 256;
        assert_eq!(f.vector_u8(), None);
        f.vector = u64::MAX;
        assert_eq!(f.vector_u8(), None);
    }

    #[test]
    fn page_fault_decode_each_bit() {
        let d = PageFaultError::decode;
        assert!(d(1).protection_violation && !d(1).write);
        assert!(d(2).write && !d(2).user);
        assert!(d(4).user);
        assert!(d(8).reserved_bit_set);
        assert!(d(0x10).instruction_fetch);
        assert!(d(0x20).protection_key);
        assert!(d(0x40).shadow_stack);
        assert!(d(0x8000).sgx);
        let none = d(0);
        assert!(
            !(none.protection_violation
                || none.write
                || none.user
                || none.reserved_bit_set
                || none.instruction_fetch
                || none.protection_key
                || none.shadow_stack
                || none.sgx)
        );
        // Typical user write to a not-present page.
        let e = d(0b110);
        assert!(!e.protection_violation && e.write && e.user);
    }

    #[test]
    fn page_fault_error_only_for_vector_14() {
        let mut f = TrapFrame::default();
        f.error_code = 0x2;
        for v in 0..=255u64 {
            f.vector = v;
            assert_eq!(f.page_fault_error().is_some(), v == 14, "vector {v}");
        }
    }

    #[test]
    fn selector_error_decode() {
        // index 5, GDT, not external: 5 << 3.
        let e = SelectorError::decode(5 << 3);
        assert_eq!(
            (e.index, e.idt, e.ldt, e.external),
            (5, false, false, false)
        );
        let e = SelectorError::decode((0x1fff << 3) | 0b111);
        assert_eq!(
            (e.index, e.idt, e.ldt, e.external),
            (0x1fff, true, true, true)
        );
        // Bits above the 16-bit selector error are ignored.
        assert_eq!(SelectorError::decode(0x1_0000_0000 | (3 << 3)).index, 3);
    }

    #[test]
    fn selector_error_only_for_vectors_10_to_13() {
        let mut f = TrapFrame::default();
        for v in 0..=255u64 {
            f.vector = v;
            assert_eq!(
                f.selector_error().is_some(),
                (10..=13).contains(&v),
                "vector {v}"
            );
        }
    }
}
