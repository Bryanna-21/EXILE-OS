//! x86-64 interrupt/exception entry, IDT construction and dispatch.
//! SPDX-License-Identifier: Apache-2.0
//!
//! # Entry path
//! Every vector has a 16-byte-aligned stub (`euk_vec_N`) that pushes a dummy
//! error code when the CPU does not (so the frame is uniform), pushes the vector
//! number and jumps to `euk_trap_common`, which saves all 15 general registers
//! and the x87/SSE state (FXSAVE, 512 bytes), calls `euk_trap_dispatch`, then
//! restores everything and `iretq`s. Because ALL register state, including
//! SSE, is saved on the interrupted thread's own kernel stack, a thread that is
//! preempted and later resumed sees every register unchanged, regardless of what
//! code the compiler generated.
//!
//! `TrapFrame` mirrors the push order; asm offsets come from `offset_of!`
//! constants so layout and asm cannot silently drift.
//!
//! # Policy
//! * All gates are interrupt gates (IF cleared on entry), IST 0. No TSS is
//!   loaded, so a double fault runs on the current stack and a stack overflow can
//!   still triple-fault. Stack guard pages are not unmapped either.
//! * Exceptions: #BP is counted and resumed; every other exception calls the
//!   fatal hook (diagnostic, never returns). No recovery from faults.
//! * Hardware IRQs (vectors 32..48): the registered handler runs with
//!   `HARDIRQ` raised; after it returns and `HARDIRQ` is lowered, the interrupt-
//!   EXIT hook runs. That is where the scheduler may switch threads, never in
//!   the handler itself.
//! * The kernel code selector is the boot GDT's 0x08 (`entry.S`).
//!
//! Single CPU. No NMI/IST handling, no APIC, no nested-interrupt support (gates
//! keep IF clear, so interrupts do not nest).
#![allow(dead_code)]
use crate::gdt::KERNEL_CODE;
use crate::idt::{self, Gate, GateError, GateKind, Idt};
use crate::pic;
use crate::preempt::HARDIRQ;
use core::arch::global_asm;
use core::mem::offset_of;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// Vectors with stubs: 0..32 exceptions, 32..48 the PIC's 16 IRQ lines.
pub const STUB_COUNT: usize = 48;
pub const IRQ_LINES: usize = 16;

/// Register state at interrupt entry, lowest address first. Matches the push
/// order in `euk_trap_common` followed by the CPU-pushed frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}
const _: () = assert!(core::mem::size_of::<TrapFrame>() == 22 * 8);
const _: () = assert!(offset_of!(TrapFrame, vector) == 15 * 8);

macro_rules! stub_table {
    () => {
        concat!(
            "EUK_VEC 0, 0\n",
            "EUK_VEC 1, 0\n",
            "EUK_VEC 2, 0\n",
            "EUK_VEC 3, 0\n",
            "EUK_VEC 4, 0\n",
            "EUK_VEC 5, 0\n",
            "EUK_VEC 6, 0\n",
            "EUK_VEC 7, 0\n",
            "EUK_VEC 8, 1\n",
            "EUK_VEC 9, 0\n",
            "EUK_VEC 10, 1\n",
            "EUK_VEC 11, 1\n",
            "EUK_VEC 12, 1\n",
            "EUK_VEC 13, 1\n",
            "EUK_VEC 14, 1\n",
            "EUK_VEC 15, 0\n",
            "EUK_VEC 16, 0\n",
            "EUK_VEC 17, 1\n",
            "EUK_VEC 18, 0\n",
            "EUK_VEC 19, 0\n",
            "EUK_VEC 20, 0\n",
            "EUK_VEC 21, 1\n",
            "EUK_VEC 22, 0\n",
            "EUK_VEC 23, 0\n",
            "EUK_VEC 24, 0\n",
            "EUK_VEC 25, 0\n",
            "EUK_VEC 26, 0\n",
            "EUK_VEC 27, 0\n",
            "EUK_VEC 28, 0\n",
            "EUK_VEC 29, 1\n",
            "EUK_VEC 30, 1\n",
            "EUK_VEC 31, 0\n",
            "EUK_VEC 32, 0\n",
            "EUK_VEC 33, 0\n",
            "EUK_VEC 34, 0\n",
            "EUK_VEC 35, 0\n",
            "EUK_VEC 36, 0\n",
            "EUK_VEC 37, 0\n",
            "EUK_VEC 38, 0\n",
            "EUK_VEC 39, 0\n",
            "EUK_VEC 40, 0\n",
            "EUK_VEC 41, 0\n",
            "EUK_VEC 42, 0\n",
            "EUK_VEC 43, 0\n",
            "EUK_VEC 44, 0\n",
            "EUK_VEC 45, 0\n",
            "EUK_VEC 46, 0\n",
            "EUK_VEC 47, 0\n",
        )
    };
}

/// The `EUK_VEC n, has_error_code` lines, exposed so a test can check them
/// against `idt::has_error_code`.
pub const STUB_SOURCE: &str = stub_table!();

global_asm!(
    ".macro EUK_VEC n, has_err",
    ".p2align 4",
    "euk_vec_\\n:",
    ".if \\has_err == 0",
    "    push 0",
    ".endif",
    "    push \\n",
    "    jmp euk_trap_common",
    ".pushsection .data.rel.ro.euk_vec_table,\"aw\"",
    "    .quad euk_vec_\\n",
    ".popsection",
    ".endm",
    ".pushsection .data.rel.ro.euk_vec_table,\"aw\"",
    ".p2align 3",
    ".global euk_vec_table",
    "euk_vec_table:",
    ".popsection",
    ".text",
    stub_table!(),
    ".p2align 4",
    ".global euk_trap_common",
    ".type euk_trap_common,@function",
    "euk_trap_common:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rbx",
    "    push rbp",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov rdi, qword ptr [rsp + {vec}]",
    "    mov rsi, rsp",
    "    sub rsp, 512",
    "    fxsave [rsp]",
    "    cld",
    "    call {dispatch}",
    "    fxrstor [rsp]",
    "    add rsp, 512",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rbp",
    "    pop rbx",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    add rsp, 16",
    "    iretq",
    ".size euk_trap_common, . - euk_trap_common",
    vec = const offset_of!(TrapFrame, vector),
    dispatch = sym euk_trap_dispatch,
);

unsafe extern "C" {
    static euk_vec_table: [u64; STUB_COUNT];
}

/// Addresses of the entry stubs, vector order.
pub fn stub_addresses() -> [u64; STUB_COUNT] {
    // SAFETY: `euk_vec_table` is a read-only table of STUB_COUNT u64 emitted by the
    // asm above; it is never written.
    unsafe { core::ptr::read(core::ptr::addr_of!(euk_vec_table)) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapError {
    AlreadyInitialized,
    Gate(GateError),
    /// Vector is not a hardware IRQ vector (32..48).
    NotIrqVector,
    /// A handler is already registered for that vector.
    AlreadyRegistered,
}

impl From<GateError> for TrapError {
    fn from(e: GateError) -> Self {
        TrapError::Gate(e)
    }
}

/// Fills `idt` for vectors 0..STUB_COUNT from `stubs`. All gates are validated
/// first, so on error `idt` is left unchanged.
pub fn fill_idt(idt: &mut Idt, stubs: &[u64; STUB_COUNT], selector: u16) -> Result<(), GateError> {
    let mut gates = [Gate::missing(); STUB_COUNT];
    for (v, g) in gates.iter_mut().enumerate() {
        let dpl = if v < 32 { idt::default_dpl(v as u8) } else { 0 };
        *g = Gate::new(stubs[v], selector, GateKind::Interrupt, dpl, 0)?;
    }
    for (v, g) in gates.iter().enumerate() {
        idt.set(v, *g)?;
    }
    Ok(())
}

static mut IDT: Idt = Idt::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Builds the IDT and loads it with `lidt`. Interrupts stay disabled. Once only.
/// Call from the single boot context before interrupts are enabled.
pub fn init() -> Result<(), TrapError> {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return Err(TrapError::AlreadyInitialized);
    }
    let stubs = stub_addresses();
    // SAFETY: the swap above guarantees this is the only access to IDT, and the
    // caller runs it on the boot CPU before interrupts are enabled.
    let idt = unsafe { &mut *core::ptr::addr_of_mut!(IDT) };
    if let Err(e) = fill_idt(idt, &stubs, KERNEL_CODE) {
        INITIALIZED.store(false, Ordering::SeqCst);
        return Err(e.into());
    }
    if !idt.exceptions_installed() {
        INITIALIZED.store(false, Ordering::SeqCst);
        return Err(TrapError::Gate(GateError::BadVector));
    }
    let ptr = idt.pointer();
    // SAFETY: `ptr` describes the static IDT, which lives for the whole run and
    // holds valid gates pointing at the stubs above.
    unsafe {
        core::arch::asm!("lidt [{p}]", p = in(reg) &ptr, options(readonly, nostack, preserves_flags));
    }
    Ok(())
}

/// Hardware IRQ handler: runs in interrupt context; must not block or switch.
pub type IrqHandler = extern "C" fn(vector: u64);
/// Runs after the handler, outside interrupt context, still with IF clear.
pub type IrqExitHook = extern "C" fn();
/// Called for any exception other than #BP. Never returns.
pub type FatalHook = extern "C" fn(vector: u64, error_code: u64, rip: u64) -> !;

const ZERO: AtomicUsize = AtomicUsize::new(0);
static HANDLERS: [AtomicUsize; IRQ_LINES] = [ZERO; IRQ_LINES];
static EXIT_HOOK: AtomicUsize = AtomicUsize::new(0);
static FATAL_HOOK: AtomicUsize = AtomicUsize::new(0);
static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);
static UNHANDLED_IRQS: AtomicU64 = AtomicU64::new(0);
static SPURIOUS_IRQS: AtomicU64 = AtomicU64::new(0);

/// Registers the handler for IRQ vector `vector` (32..48). Fails rather than
/// replacing an existing handler.
pub fn set_irq_handler(vector: u8, handler: IrqHandler) -> Result<(), TrapError> {
    let idx = usize::from(vector)
        .checked_sub(32)
        .filter(|i| *i < IRQ_LINES)
        .ok_or(TrapError::NotIrqVector)?;
    HANDLERS[idx]
        .compare_exchange(0, handler as usize, Ordering::SeqCst, Ordering::SeqCst)
        .map(|_| ())
        .map_err(|_| TrapError::AlreadyRegistered)
}

pub fn set_irq_exit_hook(hook: IrqExitHook) {
    EXIT_HOOK.store(hook as usize, Ordering::SeqCst);
}

pub fn set_fatal_hook(hook: FatalHook) {
    FATAL_HOOK.store(hook as usize, Ordering::SeqCst);
}

pub fn breakpoints_seen() -> u64 {
    BREAKPOINTS.load(Ordering::SeqCst)
}
pub fn unhandled_irqs() -> u64 {
    UNHANDLED_IRQS.load(Ordering::SeqCst)
}
pub fn spurious_irqs() -> u64 {
    SPURIOUS_IRQS.load(Ordering::SeqCst)
}

fn fatal(vector: u64, error_code: u64, rip: u64) -> ! {
    let h = FATAL_HOOK.load(Ordering::SeqCst);
    if h != 0 {
        // SAFETY: only `set_fatal_hook` stores here, always a `FatalHook`.
        let f: FatalHook = unsafe { core::mem::transmute::<usize, FatalHook>(h) };
        f(vector, error_code, rip)
    }
    loop {
        // SAFETY: halting the CPU with interrupts off has no memory effects.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Hardware-IRQ half of dispatch, generic over port access so it is testable.
pub(crate) fn dispatch_irq<P: pic::PortIo>(vector: u64, ports: &mut P) {
    let irq = (vector - 32) as usize;
    if HARDIRQ.inc().is_err() {
        fatal(vector, 0, 0);
    }
    let spurious = match irq {
        7 => pic::is_spurious_irq7(ports),
        15 => pic::is_spurious_irq15(ports),
        _ => false,
    };

    if spurious {
        SPURIOUS_IRQS.fetch_add(1, Ordering::SeqCst);
        if irq == 15 {
            pic::acknowledge_spurious_irq15(ports);
        }
    } else {
        let h = HANDLERS[irq].load(Ordering::SeqCst);
        if h != 0 {
            // SAFETY: only `set_irq_handler` stores here, always an `IrqHandler`.
            let f: IrqHandler = unsafe { core::mem::transmute::<usize, IrqHandler>(h) };
            f(vector);
        } else {
            UNHANDLED_IRQS.fetch_add(1, Ordering::SeqCst);
            // Acknowledge so the line does not stay blocked; there is nothing to run.
            let _ = pic::eoi(ports, irq as u8);
        }
    }
    // Leaving interrupt context BEFORE the exit hook: the hook may switch to
    // another thread, which must not observe a stale "in interrupt" state.
    let _ = HARDIRQ.dec();
    let e = EXIT_HOOK.load(Ordering::SeqCst);
    if e != 0 {
        // SAFETY: only `set_irq_exit_hook` stores here, always an `IrqExitHook`.
        let f: IrqExitHook = unsafe { core::mem::transmute::<usize, IrqExitHook>(e) };
        f();
    }
}

/// Called by `euk_trap_common` with the vector and a pointer to the saved frame.
#[unsafe(no_mangle)]
pub extern "C" fn euk_trap_dispatch(vector: u64, frame: *mut TrapFrame) {
    // SAFETY: `frame` points at the register block `euk_trap_common` just pushed on
    // the current stack; it is valid and exclusively ours for this call.
    let f = unsafe { &*frame };
    match vector {
        3 => {
            BREAKPOINTS.fetch_add(1, Ordering::SeqCst);
        }
        0..=31 => fatal(vector, f.error_code, f.rip),
        32..=47 => dispatch_irq(vector, &mut pic::HwPorts),
        _ => fatal(vector, f.error_code, f.rip),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicU32;

    /// HARDIRQ and the hooks are process-global; tests that touch them take the
    /// same lock as the real-switch tests in `dispatch`/`switch`.
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::switch::HOST_SWITCH_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn trap_frame_layout_matches_the_push_order() {
        assert_eq!(core::mem::size_of::<TrapFrame>(), 176);
        assert_eq!(offset_of!(TrapFrame, r15), 0);
        assert_eq!(offset_of!(TrapFrame, rax), 14 * 8);
        assert_eq!(offset_of!(TrapFrame, vector), 120);
        assert_eq!(offset_of!(TrapFrame, error_code), 128);
        assert_eq!(offset_of!(TrapFrame, rip), 136);
        assert_eq!(offset_of!(TrapFrame, ss), 168);
    }

    #[test]
    fn stub_error_code_flags_match_idt_policy() {
        let mut seen = 0;
        for line in STUB_SOURCE.lines() {
            let rest = line.trim().strip_prefix("EUK_VEC ").expect(line);
            let (v, e) = rest.split_once(", ").expect(line);
            let v: u8 = v.parse().unwrap();
            let e: u8 = e.parse().unwrap();
            assert_eq!(seen, usize::from(v), "stubs must be listed in vector order");
            if v < 32 {
                assert_eq!(e == 1, idt::has_error_code(v), "vector {v}");
            } else {
                assert_eq!(e, 0, "hardware IRQ {v} pushes no error code");
            }
            seen += 1;
        }
        assert_eq!(seen, STUB_COUNT);
    }

    #[test]
    fn stub_table_is_aligned_unique_and_encodable() {
        let t = stub_addresses();
        for (v, a) in t.iter().enumerate() {
            assert_ne!(*a, 0, "vector {v}");
            assert_eq!(a % idt::HANDLER_ALIGN, 0, "vector {v} misaligned");
            assert!(Gate::new(*a, KERNEL_CODE, GateKind::Interrupt, 0, 0).is_ok());
        }
        let mut s = t.to_vec();
        s.sort_unstable();
        s.dedup();
        assert_eq!(s.len(), STUB_COUNT, "every vector needs its own stub");
    }

    #[test]
    fn fill_idt_installs_exceptions_and_irqs_only() {
        let mut idt = Idt::new();
        fill_idt(&mut idt, &stub_addresses(), KERNEL_CODE).unwrap();
        assert!(idt.exceptions_installed());
        for v in 0..STUB_COUNT {
            let g = idt.get(v).unwrap();
            assert!(g.present(), "vector {v}");
            assert_eq!(g.kind(), Some(GateKind::Interrupt));
            assert_eq!(g.selector(), KERNEL_CODE);
            assert_eq!(g.ist(), 0);
            assert_eq!(g.dpl(), if v == 3 { 3 } else { 0 }, "vector {v}");
            assert_eq!(g.handler(), stub_addresses()[v]);
        }
        for v in STUB_COUNT..256 {
            assert!(
                !idt.get(v).unwrap().present(),
                "vector {v} must stay missing"
            );
        }
    }

    #[test]
    fn fill_idt_is_atomic_on_error() {
        let mut idt = Idt::new();
        let mut stubs = stub_addresses();
        stubs[40] = stubs[40] + 1; // misaligned
        assert_eq!(
            fill_idt(&mut idt, &stubs, KERNEL_CODE),
            Err(GateError::MisalignedHandler)
        );
        for v in 0..256 {
            assert!(
                !idt.get(v).unwrap().present(),
                "vector {v} was partially installed"
            );
        }
        let mut stubs = stub_addresses();
        stubs[0] = 0;
        assert_eq!(
            fill_idt(&mut idt, &stubs, KERNEL_CODE),
            Err(GateError::NullHandler)
        );
        assert_eq!(
            fill_idt(&mut idt, &stub_addresses(), 0),
            Err(GateError::NullSelector)
        );
        assert!(!idt.get(0).unwrap().present());
    }

    #[test]
    fn irq_handler_registration_rules() {
        extern "C" fn h(_: u64) {}
        assert_eq!(set_irq_handler(31, h), Err(TrapError::NotIrqVector));
        assert_eq!(set_irq_handler(48, h), Err(TrapError::NotIrqVector));
        assert_eq!(set_irq_handler(0, h), Err(TrapError::NotIrqVector));
        assert_eq!(set_irq_handler(255, h), Err(TrapError::NotIrqVector));
        assert_eq!(set_irq_handler(44, h), Ok(()));
        assert_eq!(set_irq_handler(44, h), Err(TrapError::AlreadyRegistered));
    }

    #[derive(Default)]
    struct Ports {
        writes: Vec<(u16, u8)>,
        reads: Vec<u16>,
        isr: u8,
    }
    impl pic::PortIo for Ports {
        fn outb(&mut self, p: u16, v: u8) {
            self.writes.push((p, v));
        }
        fn inb(&mut self, p: u16) -> u8 {
            self.reads.push(p);
            self.isr
        }
    }

    static SEEN_DEPTH: AtomicU32 = AtomicU32::new(99);
    static EXIT_SAW_DEPTH: AtomicU32 = AtomicU32::new(99);
    static CALLS: AtomicU32 = AtomicU32::new(0);

    #[test]
    fn irq_dispatch_runs_handler_in_interrupt_context_then_exit_hook_outside_it() {
        let _g = lock();
        extern "C" fn handler(_: u64) {
            SEEN_DEPTH.store(HARDIRQ.depth(), Ordering::SeqCst);
            CALLS.fetch_add(1, Ordering::SeqCst);
        }
        extern "C" fn exit_hook() {
            EXIT_SAW_DEPTH.store(HARDIRQ.depth(), Ordering::SeqCst);
        }
        // IRQ 9 = vector 41 (not used by other tests).
        set_irq_handler(41, handler).unwrap();
        set_irq_exit_hook(exit_hook);
        let mut p = Ports::default();
        dispatch_irq(41, &mut p);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(
            SEEN_DEPTH.load(Ordering::SeqCst),
            1,
            "handler runs inside interrupt context"
        );
        assert_eq!(
            EXIT_SAW_DEPTH.load(Ordering::SeqCst),
            0,
            "the switch point is outside it"
        );
        assert_eq!(HARDIRQ.depth(), 0);
        assert!(p.writes.is_empty(), "a registered handler owns its own EOI");
        // The exit hook is process-global; clear it so other tests are unaffected.
        EXIT_HOOK.store(0, Ordering::SeqCst);
    }

    #[test]
    fn unhandled_irq_is_counted_and_acknowledged() {
        let _g = lock();
        let before = unhandled_irqs();
        let mut p = Ports::default();
        dispatch_irq(42, &mut p); // IRQ 10, slave
        assert_eq!(unhandled_irqs(), before + 1);
        assert_eq!(p.writes, vec![(0xa0, 0x20), (0x20, 0x20)]);
        assert_eq!(HARDIRQ.depth(), 0);
    }

    #[test]
    fn spurious_irq7_is_counted_and_not_acknowledged() {
        let _g = lock();
        let before = spurious_irqs();
        let unhandled = unhandled_irqs();
        let mut p = Ports::default(); // ISR bit 7 clear = spurious
        dispatch_irq(39, &mut p);
        assert_eq!(spurious_irqs(), before + 1);
        assert_eq!(unhandled_irqs(), unhandled);
        assert_eq!(
            p.writes,
            vec![(0x20, 0x0b)],
            "only the ISR read command, no EOI"
        );
    }

    #[test]
    fn spurious_irq7_bypasses_registered_handler() {
        let _g = lock();
        extern "C" fn handler(_: u64) {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }

        set_irq_handler(39, handler).unwrap();
        let calls = CALLS.load(Ordering::SeqCst);
        let spurious = spurious_irqs();
        let mut p = Ports::default();

        dispatch_irq(39, &mut p);

        assert_eq!(CALLS.load(Ordering::SeqCst), calls);
        assert_eq!(spurious_irqs(), spurious + 1);
        assert_eq!(p.reads, vec![(pic::MASTER_CMD)]);
        assert_eq!(p.writes, vec![(pic::MASTER_CMD, 0x0b)]);
        assert_eq!(HARDIRQ.depth(), 0);
    }

    #[test]
    fn spurious_irq15_bypasses_registered_handler_and_eois_master_only() {
        let _g = lock();
        extern "C" fn handler(_: u64) {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }

        set_irq_handler(47, handler).unwrap();
        let calls = CALLS.load(Ordering::SeqCst);
        let spurious = spurious_irqs();
        let mut p = Ports::default();

        dispatch_irq(47, &mut p);

        assert_eq!(CALLS.load(Ordering::SeqCst), calls);
        assert_eq!(spurious_irqs(), spurious + 1);
        assert_eq!(p.reads, vec![pic::SLAVE_CMD]);
        assert_eq!(
            p.writes,
            vec![
                (pic::SLAVE_CMD, 0x0b),
                (pic::MASTER_CMD, 0x20),
            ]
        );
        assert_eq!(HARDIRQ.depth(), 0);
    }

    #[test]
    fn real_irq7_with_isr_bit_set_is_acknowledged_as_unhandled() {
        let _g = lock();
        let unhandled = unhandled_irqs();
        let mut p = Ports {
            isr: 0x80,
            ..Default::default()
        };
        dispatch_irq(39, &mut p);
        assert_eq!(unhandled_irqs(), unhandled + 1);
        assert_eq!(p.writes.last(), Some(&(0x20, 0x20)));
    }

    #[test]
    fn breakpoint_dispatch_counts_and_returns() {
        let _g = lock();
        let before = breakpoints_seen();
        let mut f = TrapFrame {
            vector: 3,
            ..Default::default()
        };
        euk_trap_dispatch(3, &mut f);
        euk_trap_dispatch(3, &mut f);
        assert_eq!(breakpoints_seen(), before + 2);
    }
}
