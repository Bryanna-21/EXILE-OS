//! D4 on-target demonstration (built only with `--cfg euk_d4_demo`).
//! SPDX-License-Identifier: Apache-2.0
//!
//! Boots on real (emulated) x86-64 with a loaded IDT, a remapped 8259 PIC and a
//! 100 Hz PIT, enables interrupts, and proves timer-driven preemption:
//!
//! * two CPU-bound threads that NEVER yield or block both make progress,
//! * a register/SSE integrity thread keeps every register it set across
//!   involuntary preemptions (the other threads clobber xmm and GPRs),
//! * a sleeping thread is woken by hardware ticks at or after its deadline,
//! * a watchdog in the timer handler fails the run if the system stops making
//!   progress (this is what the `euk_d4_nopreempt` build trips on purpose).
//!
//! Output is serial text asserted by `scripts/boot_test.py`. Only deterministic
//! lines are asserted; tick and reschedule counts are printed as `info:` lines.
//!
//! Scope: single CPU, legacy PIC + PIT, no APIC, no user mode, no TLS, no
//! address-space switching, no hardware/SMP claim. Kernel stacks are not
//! guard-protected and no TSS/IST is loaded.
use crate::dispatch::{self, Kernel, KernelSlot};
use crate::irq::{self, IrqSpinLock};
use crate::kstack::{GUARD_BYTES, StackPool};
use crate::kthread::{Config, MemError, StackMemory, ThreadParams};
use crate::pic::{self, HwPorts};
use crate::preempt::IrqExitAction;
use crate::switch;
use crate::thread::{Handle, Priority};
use crate::timer;
use crate::trap;
use crate::{MAP_LIMIT, fail, finish, serial};
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const SLOTS: usize = 8;
const STACK_BYTES: u64 = 16 * 1024;
const POOL_BYTES: usize = SLOTS * (STACK_BYTES + GUARD_BYTES) as usize;
/// The handler fails the run after this many ticks (4 s at 100 Hz).
const WATCHDOG_TICKS: u64 = 400;
/// Ticks the register-integrity thread runs for (250 ms at 100 Hz).
const REGTEST_TICKS: u64 = 25;
const MIN_RESCHEDULES: u64 = 3;

#[repr(C, align(4096))]
struct PoolMem([u8; POOL_BYTES]);
static mut POOL: PoolMem = PoolMem([0; POOL_BYTES]);

/// Stack memory access restricted to `POOL` (volatile, no libc dependency).
struct RawMem {
    lo: usize,
    hi: usize,
}

impl RawMem {
    fn span(&self, addr: u64, len: usize) -> Result<usize, MemError> {
        let a = usize::try_from(addr).map_err(|_| MemError)?;
        let e = a.checked_add(len).ok_or(MemError)?;
        if a < self.lo || e > self.hi {
            return Err(MemError);
        }
        Ok(a)
    }
}

impl StackMemory for RawMem {
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), MemError> {
        let a = self.span(addr, data.len())?;
        for (i, b) in data.iter().enumerate() {
            // SAFETY: [a, a + len) is inside POOL (checked by `span`), which is
            // identity-mapped, writable and exclusively owned by the stack manager.
            unsafe { core::ptr::write_volatile((a + i) as *mut u8, *b) };
        }
        Ok(())
    }
    fn zero(&mut self, addr: u64, len: u64) -> Result<(), MemError> {
        let n = usize::try_from(len).map_err(|_| MemError)?;
        let a = self.span(addr, n)?;
        for i in 0..n {
            // SAFETY: as in `write`.
            unsafe { core::ptr::write_volatile((a + i) as *mut u8, 0) };
        }
        Ok(())
    }
}

static KERNEL: KernelSlot<SLOTS, RawMem> = KernelSlot::new();

struct Workers {
    spin_a: Option<Handle>,
    spin_b: Option<Handle>,
    regtest: Option<Handle>,
}
static WORKERS: IrqSpinLock<Workers> = IrqSpinLock::new(Workers {
    spin_a: None,
    spin_b: None,
    regtest: None,
});

static SPIN_A: AtomicU64 = AtomicU64::new(0);
static SPIN_B: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
static REG_BAD: AtomicU64 = AtomicU64::new(u64::MAX);
static REG_DONE: AtomicBool = AtomicBool::new(false);
static IRQ_RESCHEDULES: AtomicU64 = AtomicU64::new(0);
static SELF_DESTROY_REFUSED: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    /// Defined below. Returns the number of register mismatches (0 = intact).
    fn euk_d4_regtest(ticks: *const u64, until_tick: u64) -> u64;
}

// Sets 13 general registers and xmm0-7 to known patterns, then compares them
// against memory copies in a loop (rax, r11, xmm15 are scratch) until the tick
// counter reaches `until_tick`. Any change means a preemption lost or corrupted
// state. Local labels avoid 0/1-only names (binary-literal ambiguity).
global_asm!(
    ".section .rodata",
    ".p2align 4",
    "euk_d4_xpat:",
    ".fill 16, 1, 0x11",
    ".fill 16, 1, 0x22",
    ".fill 16, 1, 0x33",
    ".fill 16, 1, 0x44",
    ".fill 16, 1, 0x55",
    ".fill 16, 1, 0x66",
    ".fill 16, 1, 0x77",
    ".fill 16, 1, 0x88",
    ".p2align 3",
    "euk_d4_gpat:",
    ".quad 0x0b0b0b0b0b0b0b0b",
    ".quad 0x0c0c0c0c0c0c0c0c",
    ".quad 0x0d0d0d0d0d0d0d0d",
    ".quad 0x0e0e0e0e0e0e0e0e",
    ".quad 0x0f0f0f0f0f0f0f0f",
    ".quad 0x1010101010101010",
    ".quad 0x1212121212121212",
    ".quad 0x1313131313131313",
    ".quad 0x1414141414141414",
    ".quad 0x1515151515151515",
    ".quad 0x1616161616161616",
    ".quad 0x1717171717171717",
    ".quad 0x1818181818181818",
    ".text",
    ".p2align 4",
    ".global euk_d4_regtest",
    ".type euk_d4_regtest,@function",
    "euk_d4_regtest:",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    sub rsp, 24",
    "    mov qword ptr [rsp], rdi",
    "    mov qword ptr [rsp + 8], rsi",
    "    mov qword ptr [rsp + 16], 0",
    "    movdqa xmm0, xmmword ptr [rip + euk_d4_xpat]",
    "    movdqa xmm1, xmmword ptr [rip + euk_d4_xpat + 16]",
    "    movdqa xmm2, xmmword ptr [rip + euk_d4_xpat + 32]",
    "    movdqa xmm3, xmmword ptr [rip + euk_d4_xpat + 48]",
    "    movdqa xmm4, xmmword ptr [rip + euk_d4_xpat + 64]",
    "    movdqa xmm5, xmmword ptr [rip + euk_d4_xpat + 80]",
    "    movdqa xmm6, xmmword ptr [rip + euk_d4_xpat + 96]",
    "    movdqa xmm7, xmmword ptr [rip + euk_d4_xpat + 112]",
    "    mov rbx, qword ptr [rip + euk_d4_gpat]",
    "    mov rcx, qword ptr [rip + euk_d4_gpat + 8]",
    "    mov rdx, qword ptr [rip + euk_d4_gpat + 16]",
    "    mov rsi, qword ptr [rip + euk_d4_gpat + 24]",
    "    mov rdi, qword ptr [rip + euk_d4_gpat + 32]",
    "    mov rbp, qword ptr [rip + euk_d4_gpat + 40]",
    "    mov r8, qword ptr [rip + euk_d4_gpat + 48]",
    "    mov r9, qword ptr [rip + euk_d4_gpat + 56]",
    "    mov r10, qword ptr [rip + euk_d4_gpat + 64]",
    "    mov r12, qword ptr [rip + euk_d4_gpat + 72]",
    "    mov r13, qword ptr [rip + euk_d4_gpat + 80]",
    "    mov r14, qword ptr [rip + euk_d4_gpat + 88]",
    "    mov r15, qword ptr [rip + euk_d4_gpat + 96]",
    "2:",
    "    cmp rbx, qword ptr [rip + euk_d4_gpat]",
    "    jne 9f",
    "    cmp rcx, qword ptr [rip + euk_d4_gpat + 8]",
    "    jne 9f",
    "    cmp rdx, qword ptr [rip + euk_d4_gpat + 16]",
    "    jne 9f",
    "    cmp rsi, qword ptr [rip + euk_d4_gpat + 24]",
    "    jne 9f",
    "    cmp rdi, qword ptr [rip + euk_d4_gpat + 32]",
    "    jne 9f",
    "    cmp rbp, qword ptr [rip + euk_d4_gpat + 40]",
    "    jne 9f",
    "    cmp r8, qword ptr [rip + euk_d4_gpat + 48]",
    "    jne 9f",
    "    cmp r9, qword ptr [rip + euk_d4_gpat + 56]",
    "    jne 9f",
    "    cmp r10, qword ptr [rip + euk_d4_gpat + 64]",
    "    jne 9f",
    "    cmp r12, qword ptr [rip + euk_d4_gpat + 72]",
    "    jne 9f",
    "    cmp r13, qword ptr [rip + euk_d4_gpat + 80]",
    "    jne 9f",
    "    cmp r14, qword ptr [rip + euk_d4_gpat + 88]",
    "    jne 9f",
    "    cmp r15, qword ptr [rip + euk_d4_gpat + 96]",
    "    jne 9f",
    "    movdqa xmm15, xmm0",
    "    pcmpeqb xmm15, xmmword ptr [rip + euk_d4_xpat]",
    "    pmovmskb eax, xmm15",
    "    cmp eax, 0xffff",
    "    jne 9f",
    "    movdqa xmm15, xmm1",
    "    pcmpeqb xmm15, xmmword ptr [rip + euk_d4_xpat + 16]",
    "    pmovmskb eax, xmm15",
    "    cmp eax, 0xffff",
    "    jne 9f",
    "    movdqa xmm15, xmm2",
    "    pcmpeqb xmm15, xmmword ptr [rip + euk_d4_xpat + 32]",
    "    pmovmskb eax, xmm15",
    "    cmp eax, 0xffff",
    "    jne 9f",
    "    movdqa xmm15, xmm3",
    "    pcmpeqb xmm15, xmmword ptr [rip + euk_d4_xpat + 48]",
    "    pmovmskb eax, xmm15",
    "    cmp eax, 0xffff",
    "    jne 9f",
    "    movdqa xmm15, xmm4",
    "    pcmpeqb xmm15, xmmword ptr [rip + euk_d4_xpat + 64]",
    "    pmovmskb eax, xmm15",
    "    cmp eax, 0xffff",
    "    jne 9f",
    "    movdqa xmm15, xmm5",
    "    pcmpeqb xmm15, xmmword ptr [rip + euk_d4_xpat + 80]",
    "    pmovmskb eax, xmm15",
    "    cmp eax, 0xffff",
    "    jne 9f",
    "    movdqa xmm15, xmm6",
    "    pcmpeqb xmm15, xmmword ptr [rip + euk_d4_xpat + 96]",
    "    pmovmskb eax, xmm15",
    "    cmp eax, 0xffff",
    "    jne 9f",
    "    movdqa xmm15, xmm7",
    "    pcmpeqb xmm15, xmmword ptr [rip + euk_d4_xpat + 112]",
    "    pmovmskb eax, xmm15",
    "    cmp eax, 0xffff",
    "    jne 9f",
    "    mov rax, qword ptr [rsp]",
    "    mov rax, qword ptr [rax]",
    "    cmp rax, qword ptr [rsp + 8]",
    "    jb 2b",
    "    jmp 8f",
    "9:",
    "    inc qword ptr [rsp + 16]",
    "8:",
    "    mov rax, qword ptr [rsp + 16]",
    "    add rsp, 24",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    ret",
    ".size euk_d4_regtest, . - euk_d4_regtest",
);

fn serial_u64(mut n: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if let Ok(s) = core::str::from_utf8(&buf[i..]) {
        serial(s);
    }
}

fn serial_hex(n: u64) {
    serial("0x");
    let mut started = false;
    for shift in (0..16).rev() {
        let d = ((n >> (shift * 4)) & 0xf) as u8;
        if d != 0 || started || shift == 0 {
            started = true;
            let c = if d < 10 { b'0' + d } else { b'a' + d - 10 };
            if let Ok(s) = core::str::from_utf8(&[c]) {
                serial(s);
            }
        }
    }
}

fn ok<T, E>(r: Result<T, E>, what: &str) -> T {
    match r {
        Ok(v) => v,
        Err(_) => fail(what),
    }
}

extern "C" fn fatal_hook(vector: u64, error_code: u64, rip: u64) -> ! {
    serial("EUK FAIL: exception vector ");
    serial_u64(vector);
    serial(" error=");
    serial_hex(error_code);
    serial(" rip=");
    serial_hex(rip);
    serial("\n");
    finish(false)
}

/// IRQ0 handler: runs in interrupt context. It acknowledges the PIC, advances
/// the clock and asks the scheduler for a reschedule. It never switches.
extern "C" fn timer_irq(_vector: u64) {
    let _ = pic::eoi(&mut HwPorts, 0);
    let now = timer::CLOCK.tick();
    dispatch::tick(&KERNEL);
    if now > WATCHDOG_TICKS {
        fail("D4 watchdog: no progress");
    }
}

/// The interrupt-EXIT step: the only place a thread is preempted.
extern "C" fn irq_exit() {
    // The `euk_d4_nopreempt` build omits the switch on purpose, to show the demo
    // really depends on preemption (the watchdog then fails the run).
    #[cfg(not(euk_d4_nopreempt))]
    {
        // SAFETY: called from the trap path on the interrupted thread's kernel
        // stack, IF clear, outside interrupt context, static kernel, hooks set.
        match unsafe { dispatch::irq_exit_reschedule(&KERNEL) } {
            Ok(IrqExitAction::Reschedule) => {
                IRQ_RESCHEDULES.fetch_add(1, Ordering::SeqCst);
            }
            Ok(_) => {}
            Err(_) => fail("D4 interrupt-exit reschedule"),
        }
    }
    #[cfg(euk_d4_nopreempt)]
    let _ = IrqExitAction::None;
}

extern "C" fn first_run() {
    ok(KERNEL.with(|k| k.complete_switch()), "D4 complete_switch");
    // A new thread inherits IF=0 from the switcher; threads run with it on.
    irq::enable();
}

extern "C" fn exit_hook(status: i32) -> ! {
    // SAFETY: static kernel, hooks installed, no spinlock held here.
    unsafe {
        dispatch::exit_current_with(&KERNEL, status, |k| {
            let me = match k.sched().current() {
                Some(h) => h,
                None => fail("D4 exit without current"),
            };
            match k.destroy(me) {
                Err(crate::kthread::Error::Thread(crate::thread::Error::StillOnCpu)) => {
                    SELF_DESTROY_REFUSED.fetch_add(1, Ordering::SeqCst);
                }
                _ => fail("D4 reclaimed a stack that is still in use"),
            }
        })
    }
}

/// CPU-bound: never yields, blocks or sleeps. Thread B also clobbers xmm0-7 and
/// several general registers so a missing save/restore in the trap path is
/// caught by the register-integrity thread.
extern "C" fn spin(arg: u64) -> i32 {
    let counter = if arg == 0 { &SPIN_A } else { &SPIN_B };
    while !STOP.load(Ordering::Relaxed) {
        counter.fetch_add(1, Ordering::Relaxed);
        if arg == 1 {
            // SAFETY: only clobbers the listed registers; no memory access.
            unsafe {
                asm!(
                    "pxor xmm0, xmm0", "pxor xmm1, xmm1", "pxor xmm2, xmm2", "pxor xmm3, xmm3",
                    "pxor xmm4, xmm4", "pxor xmm5, xmm5", "pxor xmm6, xmm6", "pxor xmm7, xmm7",
                    "mov rcx, 0x5a5a", "mov rdx, 0x5a5a", "mov rsi, 0x5a5a", "mov rdi, 0x5a5a",
                    "mov r8, 0x5a5a", "mov r9, 0x5a5a", "mov r10, 0x5a5a",
                    "mov r12, 0x5a5a", "mov r13, 0x5a5a", "mov r14, 0x5a5a", "mov r15, 0x5a5a",
                    out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
                    out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
                    out("rcx") _, out("rdx") _, out("rsi") _, out("rdi") _,
                    out("r8") _, out("r9") _, out("r10") _,
                    out("r12") _, out("r13") _, out("r14") _, out("r15") _,
                    options(nomem, nostack, preserves_flags),
                );
            }
        }
    }
    0
}

extern "C" fn regcheck(_: u64) -> i32 {
    let until = timer::CLOCK.now() + REGTEST_TICKS;
    // SAFETY: `euk_d4_regtest` only reads the pointed-to counter and the stack.
    let bad = unsafe { euk_d4_regtest(timer::CLOCK.as_ptr(), until) };
    REG_BAD.store(bad, Ordering::SeqCst);
    REG_DONE.store(true, Ordering::SeqCst);
    0
}

extern "C" fn controller(_: u64) -> i32 {
    serial("EUK D4 controller start\n");
    let t0 = timer::CLOCK.now();
    // SAFETY (all dispatch calls): single CPU, static kernel, hooks installed,
    // not in interrupt context, preemption enabled.
    ok(unsafe { dispatch::sleep_current(&KERNEL, 5) }, "D4 sleep");
    if timer::CLOCK.now() < t0 + 5 {
        fail("D4 sleeper woke before its deadline");
    }
    serial("EUK D4 hardware-tick sleep woke at or after its deadline\n");
    // Wait for the register test; the spinners keep the CPU saturated meanwhile.
    while !REG_DONE.load(Ordering::SeqCst) {
        ok(unsafe { dispatch::sleep_current(&KERNEL, 2) }, "D4 sleep");
    }
    STOP.store(true, Ordering::SeqCst);
    let (a, b, r) = {
        let w = WORKERS.lock();
        (w.spin_a, w.spin_b, w.regtest)
    };
    for h in [a, b, r] {
        match h {
            Some(h) => {
                ok(unsafe { dispatch::join_wait(&KERNEL, h) }, "D4 join");
            }
            None => fail("D4 worker handle missing"),
        }
    }
    if SPIN_A.load(Ordering::SeqCst) == 0 || SPIN_B.load(Ordering::SeqCst) == 0 {
        fail("D4 a non-yielding thread never ran");
    }
    serial("EUK D4 two non-yielding threads both made progress (involuntary preemption)\n");
    if REG_BAD.load(Ordering::SeqCst) != 0 {
        fail("D4 register or SSE state corrupted across preemption");
    }
    serial("EUK D4 registers and SSE state intact across preemption\n");
    if IRQ_RESCHEDULES.load(Ordering::SeqCst) < MIN_RESCHEDULES {
        fail("D4 too few interrupt-exit reschedules");
    }
    serial("EUK D4 interrupt-exit reschedules observed\n");
    0
}

extern "C" fn idle(_: u64) -> i32 {
    loop {
        let live = KERNEL.with(|k| {
            k.reap_terminated(|_, _| {});
            k.sched().live_threads()
        });
        if live == 1 {
            let (stacks, created, destroyed, switches, consistent) = KERNEL.with(|k| {
                (
                    k.stacks().stacks_in_use(),
                    k.stacks().created_total(),
                    k.stacks().destroyed_total(),
                    k.switch_count(),
                    k.check_accounting().is_ok() && k.sched().check_invariants().is_ok(),
                )
            });
            let refused = SELF_DESTROY_REFUSED.load(Ordering::SeqCst);
            serial("EUK D4 self-destroy refused while on cpu: ");
            serial_u64(refused);
            serial("\n");
            serial("EUK D4 info: ticks=");
            serial_u64(timer::CLOCK.now());
            serial(" irq_reschedules=");
            serial_u64(IRQ_RESCHEDULES.load(Ordering::SeqCst));
            serial(" context_switches=");
            serial_u64(switches);
            serial(" spin_a=");
            serial_u64(SPIN_A.load(Ordering::SeqCst));
            serial(" spin_b=");
            serial_u64(SPIN_B.load(Ordering::SeqCst));
            serial("\n");
            if refused != 4
                || !consistent
                || stacks != 1
                || created != destroyed + 1
                || trap::unhandled_irqs() != 0
            {
                fail("D4 accounting");
            }
            serial("EUK D4 stacks_in_use=1 created=");
            serial_u64(created);
            serial(" destroyed=");
            serial_u64(destroyed);
            serial("\nEUK D4 DEMO_OK\n");
            finish(true);
        }
        // SAFETY: IF is set in thread context; `hlt` resumes on the next interrupt.
        unsafe { asm!("hlt", options(nomem, nostack)) };
    }
}

fn entry(f: extern "C" fn(u64) -> i32) -> u64 {
    f as *const () as usize as u64
}

pub fn run() -> ! {
    serial("EUK D4: demo start\n");
    trap::set_fatal_hook(fatal_hook);
    ok(trap::init(), "D4 idt init");
    serial("EUK D4: IDT loaded (48 vectors)\n");

    // Exception path: a breakpoint must be handled and execution resumed.
    // SAFETY: #BP has a gate; the handler counts it and returns with `iretq`.
    unsafe { asm!("int3") };
    if trap::breakpoints_seen() != 1 || irq::enabled() {
        fail("D4 breakpoint");
    }
    serial("EUK D4 breakpoint trap handled and resumed\n");

    // Deliberate faults for the expected-failure tests (build-time selected).
    #[cfg(euk_d4_fault_ud)]
    // SAFETY: raises #UD on purpose; the fatal hook reports it and exits.
    unsafe {
        asm!("ud2")
    };
    #[cfg(euk_d4_fault_gp)]
    // SAFETY: loads a selector beyond the GDT limit to raise #GP(0x28) on purpose.
    unsafe {
        asm!("mov ds, ax", in("ax") 0x28u16)
    };

    let divisor = ok(
        pic::init_timer_irq(&mut HwPorts, timer::DEFAULT_HZ),
        "D4 pic/pit",
    );
    ok(
        trap::set_irq_handler(pic::TIMER_VECTOR, timer_irq),
        "D4 timer handler",
    );
    trap::set_irq_exit_hook(irq_exit);
    serial("EUK D4: PIC remapped, PIT at 100 Hz (divisor ");
    serial_u64(u64::from(divisor));
    serial(")\n");

    switch::install_hooks(first_run, exit_hook);
    let lo = core::ptr::addr_of_mut!(POOL) as usize;
    let hi = lo + POOL_BYTES;
    let pool = ok(
        StackPool::new(lo as u64, POOL_BYTES as u64, STACK_BYTES, MAP_LIMIT as u64),
        "D4 stack pool",
    );
    let text_start = core::ptr::addr_of!(__text_start) as usize as u64;
    let text_end = core::ptr::addr_of!(__text_end) as usize as u64;
    let cfg = Config {
        text_start,
        text_end,
        trampoline: switch::trampoline_address(),
    };
    KERNEL.install(ok(
        Kernel::new(cfg, pool, RawMem { lo, hi }),
        "D4 kernel init",
    ));
    let p4 = ok(Priority::new(4), "D4 priority");
    let p5 = ok(Priority::new(5), "D4 priority");
    KERNEL.with(|k| {
        ok(k.create_idle(entry(idle), 0), "D4 create idle");
        let a = ok(
            k.create(ThreadParams {
                entry: entry(spin),
                arg: 0,
                priority: p4,
            }),
            "D4 create spin A",
        );
        let b = ok(
            k.create(ThreadParams {
                entry: entry(spin),
                arg: 1,
                priority: p4,
            }),
            "D4 create spin B",
        );
        let r = ok(
            k.create(ThreadParams {
                entry: entry(regcheck),
                arg: 0,
                priority: p4,
            }),
            "D4 create regcheck",
        );
        ok(
            k.create(ThreadParams {
                entry: entry(controller),
                arg: 0,
                priority: p5,
            }),
            "D4 create controller",
        );
        let mut w = WORKERS.lock();
        w.spin_a = Some(a);
        w.spin_b = Some(b);
        w.regtest = Some(r);
    });
    serial("EUK D4: idle and four workers created\n");
    // The first dispatch abandons this bootstrap context (IF stays clear here;
    // the first thread's first-run hook enables interrupts).
    // SAFETY: single CPU, static kernel, hooks installed.
    let r = unsafe { dispatch::reschedule(&KERNEL) };
    let _ = r;
    fail("D4 bootstrap context resumed")
}
