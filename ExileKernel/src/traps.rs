//! IDT construction/loading and the Rust side of exception handling (x86-64).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Flow: `arch/x86_64/exceptions.S` stub -> shared entry path -> `euk_trap_dispatch` (defined in
//! `kernel.rs`, which owns the serial console) -> `handle` here, which classifies the vector and
//! writes a report into a fixed buffer. No heap, no formatting machinery, no panics on the
//! report path.
//!
//! Status: `build_idt`, the load-readiness check, `handle` and `Report` are host-tested, and the
//! real assembly stubs are executed on the host CPU by the tests below. `load_idt`/`init_bsp`
//! issue privileged instructions and have not been run; the `--traps` emulator cases in
//! `scripts/boot_test.py` are the tests that must pass for them.
#![allow(dead_code)]

use crate::frame::TrapFrame;
use crate::gdt::KERNEL_CODE;
use crate::idt::{
    Class, Gate, GateError, GateKind, Idt, Idtr, VECTORS, default_dpl, default_ist, exception_info,
};
use crate::stacks::{self, InstallError};
use core::sync::atomic::{AtomicBool, Ordering};

unsafe extern "C" {
    /// Handler address for each of the 256 vectors, emitted by `exceptions.S`.
    static euk_isr_table: [u64; VECTORS];
}

/// Builds an IDT with every vector present: exceptions get their architectural handling (IST for
/// #DF/NMI/#MC, DPL 3 only for #BP) and every other vector gets its stub, which reports an
/// unexpected interrupt instead of jumping to garbage. All gates are interrupt gates (IF cleared
/// on entry) so a handler is never re-entered by a maskable interrupt.
pub fn build_idt(table: &[u64; VECTORS], selector: u16) -> Result<Idt, GateError> {
    let mut idt = Idt::new();
    fill_idt(&mut idt, table, selector)?;
    Ok(idt)
}

/// In-place form of [`build_idt`]. The kernel uses this one: it writes the 256 gates straight
/// into the static IDT, so no 4 KiB temporary is zeroed or copied (the freestanding kernel image
/// must not depend on `memset`/`memcpy` being provided). On error the gates written so far
/// remain, so a failed table must not be loaded; `init_bsp` never loads after an error.
pub fn fill_idt(idt: &mut Idt, table: &[u64; VECTORS], selector: u16) -> Result<(), GateError> {
    for (v, handler) in table.iter().enumerate() {
        let vector = v as u8; // VECTORS == 256, so this never truncates
        let gate = Gate::new(
            *handler,
            selector,
            GateKind::Interrupt,
            default_dpl(vector),
            default_ist(vector),
        )?;
        idt.set(v, gate)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    /// A required exception gate or a default-handler gate is missing; loading would leave a
    /// vector that raises #GP/#DF instead of a diagnosable report.
    Incomplete,
    Gate(GateError),
    Install(InstallError),
    AlreadyLoaded,
}

/// Returns the `lidt` operand only if every vector has a present gate (and therefore every
/// CPU exception handler is installed). Interrupts must stay disabled until this has succeeded.
pub fn checked_pointer(idt: &Idt) -> Result<Idtr, LoadError> {
    let all_present = (0..VECTORS).all(|v| idt.get(v).is_some_and(Gate::present));
    if !idt.exceptions_installed() || !all_present {
        return Err(LoadError::Incomplete);
    }
    Ok(idt.pointer())
}

/// # Safety
/// Ring 0. `idtr` must describe an IDT in memory that stays mapped and unmodified for as long
/// as it is loaded; every gate must reference a mapped, executable handler, and every non-zero
/// IST index must have a valid stack in the loaded TSS.
unsafe fn load_idt(idtr: &Idtr) {
    // SAFETY: caller contract. `lidt` only reads the 10-byte descriptor.
    unsafe {
        core::arch::asm!(
            "lidt [{}]",
            in(reg) idtr as *const Idtr,
            options(readonly, nostack, preserves_flags),
        );
    }
}

/// Reads CR2 (the faulting linear address after a page fault).
///
/// # Safety
/// Ring 0.
pub unsafe fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: reading CR2 has no side effects; caller guarantees ring 0.
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

static mut BSP_IDT: Idt = Idt::new();
static IDT_LOADED: AtomicBool = AtomicBool::new(false);

/// Installs the bootstrap CPU's stacks, GDT, TSS and IDT, in that order (IST gates need TR).
/// Everything fallible and pure is checked *before* any CPU state changes. Interrupts are not
/// enabled; they must stay off until IRQ routing exists.
///
/// # Safety
/// Ring 0, interrupts disabled, bootstrap page tables active and identity-mapping
/// `0..map_limit`. Call once.
pub unsafe fn init_bsp(map_limit: u64) -> Result<(), LoadError> {
    if IDT_LOADED.load(Ordering::Acquire) {
        // Checked first so a loaded IDT is never rewritten.
        return Err(LoadError::AlreadyLoaded);
    }
    // SAFETY: `euk_isr_table` is a 256-entry read-only table defined in exceptions.S.
    let table = unsafe { &euk_isr_table };
    // SAFETY: single caller by contract and not yet loaded, so nothing else reads BSP_IDT.
    let idt = unsafe { &mut *(&raw mut BSP_IDT) };
    fill_idt(idt, table, KERNEL_CODE).map_err(LoadError::Gate)?;
    let idtr = checked_pointer(idt)?;
    // SAFETY: forwarded contract.
    unsafe { stacks::install_bsp(map_limit) }.map_err(LoadError::Install)?;
    IDT_LOADED.store(true, Ordering::Release);
    // SAFETY: gates were validated above; handlers are in the kernel image, mapped by the
    // bootstrap; the TSS with IST stacks was loaded by install_bsp; BSP_IDT is static and is
    // never modified after this point.
    unsafe { load_idt(&idtr) };
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Fault policy and reporting
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Return to the interrupted context (the saved frame is restored by iretq).
    Resume,
    /// Report and halt. Nothing here can safely continue.
    Fatal,
}

/// Recovery policy. Only #BP resumes. Everything else is fatal until the subsystems that can
/// recover from it exist (page-fault handling needs the virtual memory manager; NMI/#MC need
/// reentrancy-safe handling; #NM needs FPU state management).
pub const fn classify(vector: u8) -> Action {
    match vector {
        3 => Action::Resume,
        _ => Action::Fatal,
    }
}

pub const REPORT_CAPACITY: usize = 1024;
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Fixed-size text buffer for fault reports. Overflow truncates and sets a flag; it never
/// panics or allocates, so it is safe to use inside an exception handler.
pub struct Report {
    buf: [u8; REPORT_CAPACITY],
    len: usize,
    truncated: bool,
}

impl Report {
    pub const fn new() -> Self {
        Self {
            buf: [0; REPORT_CAPACITY],
            len: 0,
            truncated: false,
        }
    }

    /// Empties the report. Does not rewrite the buffer (no `memset`).
    pub fn clear(&mut self) {
        self.len = 0;
        self.truncated = false;
    }

    fn push_byte(&mut self, b: u8) {
        if self.len < REPORT_CAPACITY {
            self.buf[self.len] = b;
            self.len += 1;
        } else {
            self.truncated = true;
        }
    }

    pub fn push_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.push_byte(b);
        }
    }

    /// Appends `digits` (1..=16) lowercase hex digits of `value`, most significant first.
    pub fn push_hex(&mut self, value: u64, digits: u32) {
        let digits = digits.clamp(1, 16);
        for i in (0..digits).rev() {
            self.push_byte(HEX[((value >> (4 * i)) & 0xf) as usize]);
        }
    }

    fn push_reg(&mut self, name: &str, value: u64) {
        self.push_str(name);
        self.push_str("=");
        self.push_hex(value, 16);
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn as_str(&self) -> &str {
        // Only ASCII is ever written, so this cannot fail; the fallback avoids any panic path.
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("EUK TRAP <report encoding error>\n")
    }
}

fn push_page_fault_flags(out: &mut Report, frame: &TrapFrame) {
    if let Some(pf) = frame.page_fault_error() {
        out.push_str("EUK pf=");
        for (set, ch) in [
            (pf.protection_violation, "P"),
            (pf.write, "W"),
            (pf.user, "U"),
            (pf.reserved_bit_set, "R"),
            (pf.instruction_fetch, "I"),
            (pf.protection_key, "K"),
            (pf.shadow_stack, "S"),
            (pf.sgx, "G"),
        ] {
            out.push_str(if set { ch } else { "-" });
        }
        out.push_str("\n");
    }
}

fn push_selector_error(out: &mut Report, frame: &TrapFrame) {
    if let Some(sel) = frame.selector_error() {
        if frame.error_code == 0 {
            return; // e.g. a #GP not caused by a selector: nothing meaningful to decode
        }
        out.push_str("EUK sel index=");
        out.push_hex(u64::from(sel.index), 4);
        out.push_str(if sel.idt {
            " table=IDT"
        } else if sel.ldt {
            " table=LDT"
        } else {
            " table=GDT"
        });
        out.push_str(if sel.external {
            " external\n"
        } else {
            " internal\n"
        });
    }
}

/// Writes the complete fault report. `cr2` is only meaningful for vector 14 and is ignored
/// otherwise.
pub fn write_fault_report(frame: &TrapFrame, cr2: u64, out: &mut Report) {
    let Some(vector) = frame.vector_u8() else {
        out.push_str("EUK TRAP INVALID-VECTOR vec=");
        out.push_hex(frame.vector, 16);
        out.push_str("\n");
        return;
    };
    out.push_str("EUK TRAP ");
    match exception_info(vector) {
        Some(info) => {
            out.push_str(info.mnemonic);
            out.push_str(" ");
            out.push_str(info.name);
            if info.class == Class::Reserved {
                out.push_str(" (reserved vector)");
            }
        }
        None => out.push_str("INT Unexpected interrupt"),
    }
    out.push_str(" vec=");
    out.push_hex(u64::from(vector), 2);
    out.push_str(" err=");
    out.push_hex(frame.error_code, 16);
    out.push_str(" cpl=");
    out.push_hex(u64::from(frame.privilege_level()), 1);
    out.push_str("\n");
    out.push_str("EUK ");
    out.push_reg("rip", frame.rip);
    out.push_str(" ");
    out.push_reg("cs", frame.cs);
    out.push_str(" ");
    out.push_reg("rfl", frame.rflags);
    out.push_str("\nEUK ");
    out.push_reg("rsp", frame.rsp);
    out.push_str(" ");
    out.push_reg("ss", frame.ss);
    out.push_str("\n");
    // Register dump: four per line, frame order is not used so related registers stay together.
    let regs: [(&str, u64); 15] = [
        ("rax", frame.rax),
        ("rbx", frame.rbx),
        ("rcx", frame.rcx),
        ("rdx", frame.rdx),
        ("rsi", frame.rsi),
        ("rdi", frame.rdi),
        ("rbp", frame.rbp),
        ("r8", frame.r8),
        ("r9", frame.r9),
        ("r10", frame.r10),
        ("r11", frame.r11),
        ("r12", frame.r12),
        ("r13", frame.r13),
        ("r14", frame.r14),
        ("r15", frame.r15),
    ];
    for (i, (name, value)) in regs.iter().enumerate() {
        if i % 4 == 0 {
            out.push_str("EUK ");
        } else {
            out.push_str(" ");
        }
        out.push_reg(name, *value);
        if i % 4 == 3 || i == regs.len() - 1 {
            out.push_str("\n");
        }
    }
    if vector == 14 {
        out.push_str("EUK ");
        out.push_reg("cr2", cr2);
        out.push_str("\n");
        push_page_fault_flags(out, frame);
    }
    push_selector_error(out, frame);
}

/// Classifies the trap and writes its report. Pure: all hardware reads (CR2) are done by the
/// caller, so this is exercised with synthetic frames on the host.
pub fn handle(frame: &mut TrapFrame, cr2: u64, out: &mut Report) -> Action {
    let Some(vector) = frame.vector_u8() else {
        write_fault_report(frame, cr2, out);
        return Action::Fatal;
    };
    match classify(vector) {
        Action::Resume => {
            out.push_str("EUK TRAP ");
            out.push_str(exception_info(vector).map_or("INT", |i| i.mnemonic));
            out.push_str(" resumed ");
            out.push_reg("rip", frame.rip);
            out.push_str("\n");
            Action::Resume
        }
        Action::Fatal => {
            write_fault_report(frame, cr2, out);
            Action::Fatal
        }
    }
}

static mut TRAP_REPORT: Report = Report::new();

/// The handler's report buffer, emptied. It lives in `.bss` rather than on the (possibly small
/// IST) stack, and its zero-initialisation is done by the loader, not by a runtime `memset`.
///
/// # Safety
/// Call only after `enter_trap()` returned true, and stop using the reference when the handler
/// resumes (`leave_trap`) or halts. `enter_trap` is what makes the access exclusive.
pub unsafe fn trap_report() -> &'static mut Report {
    // SAFETY: exclusivity by the caller contract above.
    let report = unsafe { &mut *(&raw mut TRAP_REPORT) };
    report.clear();
    report
}

static IN_TRAP: AtomicBool = AtomicBool::new(false);

/// Marks entry into the handler. Returns `false` if a trap handler is already running (a fault
/// while reporting a fault), in which case the caller must halt without touching any more
/// state. Pair with `leave_trap` only when the handler resumes.
pub fn enter_trap() -> bool {
    !IN_TRAP.swap(true, Ordering::Acquire)
}

pub fn leave_trap() {
    IN_TRAP.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idt::{IST_DOUBLE_FAULT, IST_MACHINE_CHECK, IST_NMI, has_error_code};

    const BASE: u64 = 0xffff_8000_0010_0000;

    fn fake_table() -> [u64; VECTORS] {
        core::array::from_fn(|v| BASE + 16 * v as u64)
    }

    // ----- IDT construction ---------------------------------------------------------------

    #[test]
    fn every_vector_has_a_present_gate_pointing_at_its_stub() {
        let t = fake_table();
        let idt = build_idt(&t, KERNEL_CODE).unwrap();
        for v in 0..VECTORS {
            let g = idt.get(v).unwrap();
            assert!(g.present(), "vector {v}");
            assert_eq!(g.handler(), t[v], "vector {v}");
            assert_eq!(g.selector(), KERNEL_CODE);
            assert_eq!(g.kind(), Some(GateKind::Interrupt));
        }
        assert!(idt.exceptions_installed());
    }

    #[test]
    fn critical_exceptions_use_ist_and_only_breakpoint_is_user_callable() {
        let idt = build_idt(&fake_table(), KERNEL_CODE).unwrap();
        for v in 0..VECTORS {
            let g = idt.get(v).unwrap();
            let expect_ist = match v {
                8 => IST_DOUBLE_FAULT,
                2 => IST_NMI,
                18 => IST_MACHINE_CHECK,
                _ => 0,
            };
            assert_eq!(g.ist(), expect_ist, "vector {v}");
            assert_eq!(g.dpl(), if v == 3 { 3 } else { 0 }, "vector {v}");
        }
    }

    #[test]
    fn invalid_stub_addresses_are_rejected() {
        let mut t = fake_table();
        t[77] = 0;
        assert_eq!(
            build_idt(&t, KERNEL_CODE).err(),
            Some(GateError::NullHandler)
        );
        let mut t = fake_table();
        t[200] += 8;
        assert_eq!(
            build_idt(&t, KERNEL_CODE).err(),
            Some(GateError::MisalignedHandler)
        );
        let mut t = fake_table();
        t[5] = 0x0000_8000_0000_0000;
        assert_eq!(
            build_idt(&t, KERNEL_CODE).err(),
            Some(GateError::NonCanonicalHandler)
        );
        assert_eq!(
            build_idt(&fake_table(), 0).err(),
            Some(GateError::NullSelector)
        );
    }

    #[test]
    fn load_is_refused_until_the_table_is_complete() {
        let empty = Idt::new();
        assert_eq!(checked_pointer(&empty).err(), Some(LoadError::Incomplete));

        // Exceptions only: the 224 default-handler gates are still missing.
        let t = fake_table();
        let mut partial = Idt::new();
        for v in 0..32 {
            partial
                .set(
                    v,
                    Gate::new(t[v], KERNEL_CODE, GateKind::Interrupt, 0, 0).unwrap(),
                )
                .unwrap();
        }
        assert!(partial.exceptions_installed());
        assert_eq!(checked_pointer(&partial).err(), Some(LoadError::Incomplete));

        // Everything but one required exception.
        let mut almost = build_idt(&t, KERNEL_CODE).unwrap();
        almost.set(14, Gate::missing()).unwrap();
        assert_eq!(checked_pointer(&almost).err(), Some(LoadError::Incomplete));

        // Everything but one default-handler vector.
        let mut almost = build_idt(&t, KERNEL_CODE).unwrap();
        almost.set(255, Gate::missing()).unwrap();
        assert_eq!(checked_pointer(&almost).err(), Some(LoadError::Incomplete));

        let full = build_idt(&t, KERNEL_CODE).unwrap();
        let p = checked_pointer(&full).unwrap();
        assert_eq!({ p.limit }, 4095);
        assert_eq!({ p.base }, &full as *const Idt as u64);
    }

    // ----- policy and reports -------------------------------------------------------------

    fn frame(vector: u64, error_code: u64) -> TrapFrame {
        TrapFrame {
            vector,
            error_code,
            rip: 0xffff_8000_0012_3456,
            cs: 0x08,
            rflags: 0x46,
            rsp: 0xffff_8000_0040_0000,
            ss: 0x10,
            rax: 0xa1,
            rbx: 0xb2,
            r15: 0xf15,
            ..TrapFrame::default()
        }
    }

    #[test]
    fn only_breakpoint_resumes() {
        for v in 0..=255u8 {
            let expect = if v == 3 {
                Action::Resume
            } else {
                Action::Fatal
            };
            assert_eq!(classify(v), expect, "vector {v}");
        }
    }

    #[test]
    fn breakpoint_is_resumed_and_frame_is_untouched() {
        let mut f = frame(3, 0);
        let before = f;
        let mut r = Report::new();
        assert_eq!(handle(&mut f, 0, &mut r), Action::Resume);
        assert_eq!(f, before);
        assert!(
            r.as_str()
                .starts_with("EUK TRAP #BP resumed rip=ffff800000123456"),
            "{}",
            r.as_str()
        );
    }

    #[test]
    fn invalid_opcode_is_fatal_with_full_report() {
        let mut f = frame(6, 0);
        let mut r = Report::new();
        assert_eq!(handle(&mut f, 0, &mut r), Action::Fatal);
        let s = r.as_str();
        assert!(
            s.starts_with("EUK TRAP #UD Invalid Opcode vec=06 err=0000000000000000 cpl=0\n"),
            "{s}"
        );
        assert!(s.contains("rip=ffff800000123456"));
        assert!(s.contains("cs=0000000000000008"));
        assert!(s.contains("rsp=ffff800000400000"));
        assert!(s.contains("rax=00000000000000a1"));
        assert!(s.contains("rbx=00000000000000b2"));
        assert!(s.contains("r15=0000000000000f15"));
        assert!(!s.contains("cr2="), "cr2 is only reported for page faults");
        assert!(!r.truncated());
    }

    #[test]
    fn page_fault_report_includes_cr2_and_decoded_error() {
        let mut f = frame(14, 0b0010_0111); // P, W, U, protection key
        let mut r = Report::new();
        assert_eq!(handle(&mut f, 0xdead_beef_000, &mut r), Action::Fatal);
        let s = r.as_str();
        assert!(s.contains("#PF Page Fault vec=0e"), "{s}");
        assert!(s.contains("cr2=00000deadbeef000"), "{s}");
        assert!(s.contains("pf=PWU--K--"), "{s}");
    }

    #[test]
    fn selector_error_is_decoded_for_gp_and_skipped_when_zero() {
        let mut r = Report::new();
        handle(&mut frame(13, (7 << 3) | 0b001), 0, &mut r);
        assert!(r.as_str().contains("#GP General Protection vec=0d"));
        assert!(
            r.as_str().contains("sel index=0007 table=GDT external"),
            "{}",
            r.as_str()
        );

        let mut r = Report::new();
        handle(&mut frame(13, 0), 0, &mut r);
        assert!(!r.as_str().contains("EUK sel"), "{}", r.as_str());

        let mut r = Report::new();
        handle(&mut frame(11, (3 << 3) | 0b010), 0, &mut r);
        assert!(r.as_str().contains("table=IDT internal"), "{}", r.as_str());
    }

    #[test]
    fn double_fault_report_names_the_vector() {
        let mut r = Report::new();
        assert_eq!(handle(&mut frame(8, 0), 0, &mut r), Action::Fatal);
        assert!(
            r.as_str().starts_with("EUK TRAP #DF Double Fault vec=08"),
            "{}",
            r.as_str()
        );
    }

    #[test]
    fn reserved_and_unexpected_vectors_are_reported_not_ignored() {
        let mut r = Report::new();
        assert_eq!(handle(&mut frame(15, 0), 0, &mut r), Action::Fatal);
        assert!(r.as_str().contains("(reserved vector)"), "{}", r.as_str());

        let mut r = Report::new();
        assert_eq!(handle(&mut frame(0x20, 0), 0, &mut r), Action::Fatal);
        assert!(
            r.as_str()
                .starts_with("EUK TRAP INT Unexpected interrupt vec=20"),
            "{}",
            r.as_str()
        );
    }

    #[test]
    fn corrupt_vector_is_fatal_and_cannot_index_anything() {
        for v in [256u64, 0x1_0000_0000, u64::MAX] {
            let mut r = Report::new();
            assert_eq!(handle(&mut frame(v, 0), 0, &mut r), Action::Fatal);
            assert!(
                r.as_str().starts_with("EUK TRAP INVALID-VECTOR"),
                "{}",
                r.as_str()
            );
        }
    }

    #[test]
    fn worst_case_report_fits_without_truncation() {
        let max = TrapFrame {
            r15: u64::MAX,
            r14: u64::MAX,
            r13: u64::MAX,
            r12: u64::MAX,
            r11: u64::MAX,
            r10: u64::MAX,
            r9: u64::MAX,
            r8: u64::MAX,
            rbp: u64::MAX,
            rdi: u64::MAX,
            rsi: u64::MAX,
            rdx: u64::MAX,
            rcx: u64::MAX,
            rbx: u64::MAX,
            rax: u64::MAX,
            vector: 14,
            error_code: u64::MAX,
            rip: u64::MAX,
            cs: u64::MAX,
            rflags: u64::MAX,
            rsp: u64::MAX,
            ss: u64::MAX,
        };
        for v in 0..=255u64 {
            let mut f = max;
            f.vector = v;
            let mut r = Report::new();
            handle(&mut f, u64::MAX, &mut r);
            assert!(!r.truncated(), "vector {v}");
            assert!(r.as_str().len() < REPORT_CAPACITY);
        }
    }

    #[test]
    fn report_truncates_instead_of_overflowing() {
        let mut r = Report::new();
        for _ in 0..REPORT_CAPACITY + 10 {
            r.push_str("x");
        }
        assert!(r.truncated());
        assert_eq!(r.as_str().len(), REPORT_CAPACITY);
        r.push_hex(u64::MAX, 16); // still safe after overflow
        assert_eq!(r.as_str().len(), REPORT_CAPACITY);
    }

    #[test]
    fn clear_empties_the_report_and_resets_truncation() {
        let mut r = Report::new();
        for _ in 0..REPORT_CAPACITY + 1 {
            r.push_str("x");
        }
        assert!(r.truncated());
        r.clear();
        assert_eq!(r.as_str(), "");
        assert!(!r.truncated());
        r.push_str("ok");
        assert_eq!(r.as_str(), "ok");
    }

    #[test]
    fn fill_idt_in_place_matches_build_idt_and_overwrites() {
        let t = fake_table();
        let built = build_idt(&t, KERNEL_CODE).unwrap();
        let mut idt = Idt::new();
        idt.set(
            77,
            Gate::new(BASE + 0x9000, 0x18, GateKind::Trap, 3, 5).unwrap(),
        )
        .unwrap();
        fill_idt(&mut idt, &t, KERNEL_CODE).unwrap();
        for v in 0..VECTORS {
            assert_eq!(idt.get(v), built.get(v), "vector {v}");
        }
    }

    #[test]
    fn hex_formatting_and_digit_clamping() {
        let mut r = Report::new();
        r.push_hex(0xabc, 4);
        r.push_hex(0x1f, 2);
        r.push_hex(0xffff_ffff_ffff_ffff, 16);
        r.push_hex(0x5, 0); // clamped to 1 digit
        r.push_hex(0x5, 99); // clamped to 16 digits
        let expect = format!(
            "{:04x}{:02x}{:016x}{:x}{:016x}",
            0xabc,
            0x1f,
            u64::MAX,
            5,
            5
        );
        assert_eq!(r.as_str(), expect);
    }

    #[test]
    fn reentry_guard_detects_nested_traps() {
        assert!(enter_trap());
        assert!(!enter_trap(), "second entry while active must be refused");
        leave_trap();
        assert!(enter_trap(), "usable again after leave");
        leave_trap();
    }

    #[test]
    fn error_code_policy_agrees_with_idt_table_for_reports() {
        // Vectors the CPU gives an error code must be decoded as such by the frame helpers.
        for v in 0..=255u8 {
            let mut f = TrapFrame::default();
            f.vector = u64::from(v);
            if f.selector_error().is_some() {
                assert!(has_error_code(v), "vector {v}");
            }
            if f.page_fault_error().is_some() {
                assert!(has_error_code(v));
            }
        }
    }

    // ----- real assembly, executed on the host CPU ---------------------------------------
    //
    // The kernel's stubs and shared entry path are linked into this test binary and run. A
    // synthetic hardware interrupt frame is built exactly as the CPU would build it; control
    // transfers to the real stub, which calls our recording `euk_trap_dispatch`, and the real
    // `iretq` returns to a harness label. Ring 3 -> ring 3 iretq is legal, so this runs
    // unprivileged. It proves frame layout, error-code normalisation, register preservation,
    // DF handling and stack alignment of the actual assembly. It does NOT prove IDT loading or
    // real exception delivery by the CPU.
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    mod exec {
        use super::*;
        use std::sync::Mutex;

        static SERIAL: Mutex<()> = Mutex::new(());
        static mut SEEN: TrapFrame = TrapFrame {
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rbp: 0,
            rdi: 0,
            rsi: 0,
            rdx: 0,
            rcx: 0,
            rbx: 0,
            rax: 0,
            vector: 0,
            error_code: 0,
            rip: 0,
            cs: 0,
            rflags: 0,
            rsp: 0,
            ss: 0,
        };
        static mut SEEN_ADDR: usize = 0;
        static mut SEEN_FLAGS: u64 = 0;
        static mut CALLS: u32 = 0;
        static mut SAVE_RSP: u64 = 0;
        static mut SAVE_RES: u64 = 0;
        static mut SAVE_STUB: u64 = 0;

        unsafe extern "C" {
            fn euk_isr_stubs();
            fn euk_test_enter(
                stub: u64,
                rsp: u64,
                regs: *const u64,
                result: *mut u64,
                slot: *mut u64,
                live_flags: u64,
            );
            fn euk_test_resume();
        }

        /// Stands in for the kernel's dispatcher: records what the stub gave us, then edits the
        /// frame so the test can see that edits are restored by the return path.
        #[unsafe(no_mangle)]
        extern "C" fn euk_trap_dispatch(frame: &mut TrapFrame) {
            let flags: u64;
            // SAFETY: reads RFLAGS via the stack; serialised by SERIAL in `run_stub`.
            unsafe {
                core::arch::asm!("pushfq", "pop {}", out(reg) flags);
                SEEN = *frame;
                SEEN_ADDR = frame as *mut TrapFrame as usize;
                SEEN_FLAGS = flags;
                CALLS += 1;
            }
            frame.rax = !frame.rax;
            frame.rflags |= 1; // set CF
        }

        // Intel syntax (the default for global_asm). Arguments (SysV): rdi=stub, rsi=stack
        // pointer at stub entry, rdx=initial GPRs (frame order), rcx=result[16], r8=&hw_frame.rsp,
        // r9=RFLAGS live at stub entry (the interrupted code's flags: hardware leaves DF alone).
        core::arch::global_asm!(
            ".global euk_test_enter",
            "euk_test_enter:",
            "push rbx",
            "push rbp",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "mov [rip + {save_rsp}], rsp",
            "mov [rip + {save_res}], rcx",
            "mov [rip + {save_stub}], rdi",
            "mov [r8], rsp",              // hardware frame's saved RSP = host stack to resume on
            "mov rsp, rsi",
            "push r9",
            "popfq", // live flags at stub entry = the interrupted code's, notably DF
            "mov r15, rdx",
            "mov r14, [r15 + 8]",
            "mov r13, [r15 + 16]",
            "mov r12, [r15 + 24]",
            "mov r11, [r15 + 32]",
            "mov r10, [r15 + 40]",
            "mov r9,  [r15 + 48]",
            "mov r8,  [r15 + 56]",
            "mov rbp, [r15 + 64]",
            "mov rdi, [r15 + 72]",
            "mov rsi, [r15 + 80]",
            "mov rdx, [r15 + 88]",
            "mov rcx, [r15 + 96]",
            "mov rbx, [r15 + 104]",
            "mov rax, [r15 + 112]",
            "mov r15, [r15]",
            "jmp qword ptr [rip + {save_stub}]",
            ".global euk_test_resume",
            "euk_test_resume:",
            "pushfq",                     // capture RFLAGS (incl. DF) exactly as iretq restored it
            "cld",                        // the Rust caller we return to requires DF clear
            "push rax",
            "push rbx",
            "push rcx",
            "push rdx",
            "push rsi",
            "push rdi",
            "push rbp",
            "push r8",
            "push r9",
            "push r10",
            "push r11",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "mov rdi, [rip + {save_res}]",
            "xor ecx, ecx",
            "2:",
            "mov rax, [rsp + rcx*8]",
            "mov [rdi + rcx*8], rax",
            "inc rcx",
            "cmp rcx, 16",
            "jne 2b",
            "add rsp, 128",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbp",
            "pop rbx",
            "ret",
            save_rsp = sym SAVE_RSP,
            save_res = sym SAVE_RES,
            save_stub = sym SAVE_STUB,
        );

        pub struct Run {
            pub seen: TrapFrame,
            pub dispatch_frame_addr: usize,
            pub dispatch_flags: u64,
            /// r15..rax as they were when control came back, then RFLAGS.
            pub after: [u64; 16],
            pub hw_rsp_field: u64,
            pub hw_rip: u64,
            pub hw_cs: u64,
            pub hw_ss: u64,
        }

        #[repr(C, align(16))]
        struct Scratch([u8; 65536]);

        fn live_segments() -> (u64, u64) {
            let (cs, ss): (u16, u16);
            // SAFETY: reads segment selectors, no side effects.
            unsafe {
                core::arch::asm!("mov {0:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags));
                core::arch::asm!("mov {0:x}, ss", out(reg) ss, options(nomem, nostack, preserves_flags));
            }
            (u64::from(cs), u64::from(ss))
        }

        pub fn stub_address(vector: u8) -> u64 {
            euk_isr_stubs as *const () as usize as u64 + 16 * u64::from(vector)
        }

        /// Executes the real stub for `vector` with a hardware-shaped frame.
        pub fn run_stub(vector: u8, error_code: u64, regs: [u64; 15], rflags: u64) -> Run {
            // Refuse to execute a stub whose error-code policy disagrees with idt.rs: running it
            // would misalign the frame and crash the whole test process instead of failing one
            // test with a message. (`push $0` first <=> no hardware error code.)
            // SAFETY: reading the first two bytes of the stub's own code.
            let head = unsafe { core::slice::from_raw_parts(stub_address(vector) as *const u8, 2) };
            assert_eq!(
                head == [0x6a, 0x00],
                !has_error_code(vector),
                "vector {vector}: stub error-code policy disagrees with idt.rs"
            );
            let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
            let mut scratch = Box::new(Scratch([0; 65536]));
            let top = scratch.0.as_mut_ptr() as u64 + 65536;
            assert_eq!(top % 16, 0);
            let (cs, ss) = live_segments();
            let words = scratch.0.as_mut_ptr().cast::<u64>();
            let put = |back: u64, value: u64| {
                // SAFETY: `back` is a small positive offset below `top`, inside `scratch`.
                unsafe { words.add(((65536 - back) / 8) as usize).write(value) }
            };
            // What the CPU pushes, highest address first. The CPU aligns RSP to 16 before this.
            put(8, ss);
            put(16, 0); // RSP field, patched by euk_test_enter
            put(24, rflags);
            put(32, cs);
            put(40, euk_test_resume as *const () as usize as u64);
            let entry_rsp = if has_error_code(vector) {
                put(48, error_code);
                top - 48
            } else {
                top - 40
            };
            let slot = (top - 16) as *mut u64;
            let mut result = [0u64; 16];
            // SAFETY: the harness jumps into the real stub with a valid stack and returns here
            // through the real iretq path; all registers the Rust caller needs are restored.
            unsafe {
                CALLS = 0;
                euk_test_enter(
                    stub_address(vector),
                    entry_rsp,
                    regs.as_ptr(),
                    result.as_mut_ptr(),
                    slot,
                    rflags,
                );
                let calls = CALLS;
                assert_eq!(calls, 1, "dispatch must run exactly once");
                Run {
                    seen: SEEN,
                    dispatch_frame_addr: SEEN_ADDR,
                    dispatch_flags: SEEN_FLAGS,
                    after: result,
                    hw_rsp_field: *slot,
                    hw_rip: euk_test_resume as *const () as usize as u64,
                    hw_cs: cs,
                    hw_ss: ss,
                }
            }
        }

        fn pattern(vector: u8) -> [u64; 15] {
            core::array::from_fn(|i| {
                0x1111_0000_0000_0000 * (i as u64 + 1) + u64::from(vector) * 0x101 + i as u64
            })
        }

        #[test]
        fn every_stub_delivers_a_correct_frame_and_restores_state() {
            const ARITH_AND_DF: u64 = 0xcd5;
            for vector in 0..=255u8 {
                let err = if has_error_code(vector) {
                    0xe000_0000 + u64::from(vector)
                } else {
                    0
                };
                let regs = pattern(vector);
                let rflags_in = 0x246; // ZF PF, reserved bit 1
                let run = run_stub(vector, err, regs, rflags_in);
                let s = run.seen;

                assert_eq!(s.vector, u64::from(vector), "vector field");
                assert_eq!(
                    s.error_code, err,
                    "vector {vector}: error code normalisation"
                );
                assert_eq!(s.rip, run.hw_rip, "vector {vector}: rip");
                assert_eq!(s.cs, run.hw_cs, "vector {vector}: cs");
                assert_eq!(s.rflags, rflags_in, "vector {vector}: rflags");
                assert_eq!(s.rsp, run.hw_rsp_field, "vector {vector}: rsp");
                assert_eq!(s.ss, run.hw_ss, "vector {vector}: ss");
                assert_eq!(s.gprs(), regs, "vector {vector}: GPRs as seen by Rust");

                assert_eq!(
                    run.dispatch_frame_addr % 16,
                    0,
                    "vector {vector}: frame (= RSP at the call) must be 16-aligned"
                );

                // After return: every register intact except rax, which the dispatcher edited.
                let mut want = regs;
                want[14] = !regs[14];
                assert_eq!(
                    &run.after[..15],
                    &want[..],
                    "vector {vector}: registers after iretq"
                );
                assert_eq!(
                    run.after[15] & ARITH_AND_DF,
                    (rflags_in | 1) & ARITH_AND_DF,
                    "vector {vector}: rflags after iretq"
                );
            }
        }

        #[test]
        fn direction_flag_is_cleared_for_rust_and_restored_on_return() {
            const DF: u64 = 1 << 10;
            for vector in [0u8, 3, 8, 14, 32, 255] {
                let err = if has_error_code(vector) { 0x10 } else { 0 };
                let run = run_stub(vector, err, pattern(vector), 0x246 | DF);
                assert_eq!(
                    run.seen.rflags & DF,
                    DF,
                    "vector {vector}: saved frame keeps the interrupted DF"
                );
                assert_eq!(
                    run.dispatch_flags & DF,
                    0,
                    "vector {vector}: DF must be clear inside Rust"
                );
                assert_eq!(
                    run.after[15] & DF,
                    DF,
                    "vector {vector}: DF restored by iretq"
                );
            }
        }

        #[test]
        fn dispatcher_edits_to_the_frame_are_what_iretq_restores() {
            // euk_trap_dispatch above complements rax and sets CF; seeing both after the return
            // proves the registers come from the frame, not from stale live values.
            let regs = pattern(3);
            let run = run_stub(3, 0, regs, 0x246);
            assert_eq!(run.after[14], !regs[14]);
            assert_eq!(run.after[15] & 1, 1, "CF set by dispatcher");
        }

        #[test]
        fn stub_encoding_pins_the_error_code_policy() {
            // First bytes of each stub: `push $0` (no hardware error code) then `push $vector`,
            // then a jump. Independent of execution, so a wrong policy is caught even for
            // vectors a test could not safely run.
            for vector in 0..=255u8 {
                let a = stub_address(vector);
                assert_eq!(a % 16, 0, "vector {vector}: stub alignment");
                // SAFETY: reading 16 bytes of the stub's own code.
                let code = unsafe { core::slice::from_raw_parts(a as *const u8, 16) };
                let mut i = 0;
                if has_error_code(vector) {
                    assert_ne!(
                        &code[0..2],
                        &[0x6a, 0x00],
                        "vector {vector}: must not push a dummy"
                    );
                } else {
                    assert_eq!(
                        &code[0..2],
                        &[0x6a, 0x00],
                        "vector {vector}: must push a dummy error code"
                    );
                    i = 2;
                }
                if vector < 128 {
                    assert_eq!(
                        &code[i..i + 2],
                        &[0x6a, vector],
                        "vector {vector}: push imm8"
                    );
                    i += 2;
                } else {
                    assert_eq!(
                        &code[i..i + 5],
                        &[0x68, vector, 0, 0, 0],
                        "vector {vector}: push imm32"
                    );
                    i += 5;
                }
                assert!(
                    code[i] == 0xe9 || code[i] == 0xeb,
                    "vector {vector}: jmp to common path"
                );
            }
        }

        #[test]
        fn real_vector_table_matches_stub_layout_and_builds_a_valid_idt() {
            // SAFETY: reading the assembled table.
            let table = unsafe { &euk_isr_table };
            for v in 0..VECTORS {
                assert_eq!(table[v], stub_address(v as u8), "vector {v}");
            }
            let idt =
                build_idt(table, KERNEL_CODE).expect("assembled table must satisfy gate rules");
            assert!(checked_pointer(&idt).is_ok());
        }
    }
}
