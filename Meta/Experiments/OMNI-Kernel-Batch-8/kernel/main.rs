//! OMNI Batch 8 bare-metal kernel (x86-64, Multiboot v1, single CPU).
//!
//! Built as a `staticlib` and linked with `arch/x86_64/*.S` by `scripts/build.sh`.
//! Nothing here has been executed on a CPU or in QEMU in the authoring environment;
//! see REPORT.md. Build-time `--cfg` switches:
//!   test_exit        exit QEMU through isa-debug-exit (0x10 -> status 33, 0x11 -> status 35)
//!   omni8_ring3      also run the ring-3 / SYSCALL stage
//!   omni8_preempt    also run the tick-driven preemption stage (real PIT ticks, two kernel threads)
//!   omni8_user_fault (with omni8_ring3) run the ring-3 program that must fault
//!   omni8_fault_trap deliberately take an unexpected #UD to exercise the fatal path
#![no_std]
#![allow(unexpected_cfgs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Build variants compile different stages out, which leaves their inputs unused.
#![allow(dead_code)]

use core::arch::asm;
use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use omni_kernel7::sched::Sched;
use omni_kernel7::syscall::Kernel;
use omni_kernel7::uaccess::UserBacking;
use omni8::desc::{
    GDT_LIMIT, Gdt, IST_DOUBLE_FAULT, IST_FATAL, IST_IRQ, IST_MACHINE_CHECK, IST_NMI,
    SEL_KERNEL_CODE, SEL_TSS, Tss,
};
use omni8::frames::{CEILING, FRAME_SIZE, FrameAllocator};
use omni8::idt::{
    IDT_LIMIT, Idt, IdtEntry, TIMER_VECTOR, TrapFrame, VECTOR_USER_TEST, ist_for_vector,
};
use omni8::multiboot::{self, INFO_BYTES, Region};
use omni8::paging::{
    FrameSource, HUGE_SIZE, MapError, Mapper, PAGE_SIZE, Perms, PhysMem, UserWindow,
};
use omni8::preempt::Preempt;
use omni8::selftest::{self, Expectation, Observation, OrderLog};
use omni8::timer::{self, Ticks};

// ---- global state -----------------------------------------------------------------------

/// Interior-mutable global. The kernel is single-CPU and the only asynchronous code (the
/// trap dispatcher) touches atomics only, so unsynchronised access from the main flow is sound.
struct Global<T>(UnsafeCell<T>);
// SAFETY: see above; single CPU, no concurrent mutable access.
unsafe impl<T> Sync for Global<T> {}
impl<T> Global<T> {
    const fn new(v: T) -> Self {
        Global(UnsafeCell::new(v))
    }
    #[allow(clippy::mut_from_ref)]
    unsafe fn get(&self) -> &mut T {
        // SAFETY: caller guarantees exclusive use.
        unsafe { &mut *self.0.get() }
    }
}

const STACK_BYTES: usize = 16 * 1024;
#[repr(C, align(16))]
struct Stack([u8; STACK_BYTES]);
impl Stack {
    const fn new() -> Self {
        Stack([0; STACK_BYTES])
    }
    fn top(&self) -> u64 {
        self.0.as_ptr() as u64 + STACK_BYTES as u64
    }
}

static GDT: Global<Option<Gdt>> = Global::new(None);
static TSS: Global<Option<Tss>> = Global::new(None);
static IDT: Global<Idt> = Global::new(Idt::new());
static FRAMES: Global<FrameAllocator> = Global::new(FrameAllocator::new());
static RSP0_STACK: Stack = Stack::new();
static IST_STACKS: [Stack; 5] = [
    Stack::new(),
    Stack::new(),
    Stack::new(),
    Stack::new(),
    Stack::new(),
];
static TICKS: Ticks = Ticks::new();
/// Time-slice accounting for deferred preemption: only the timer interrupt writes it, atomics only.
static PREEMPT: Preempt = match Preempt::new(2) {
    Ok(p) => p,
    Err(_) => panic!("invalid preemption quantum"),
};
static ORDER_LOG: OrderLog = OrderLog::new();
static KERNEL: Kernel = Kernel::new(&NoUserBacking, false);

/// The Batch 7 syscalls this kernel enables (yield/exit/sleep/thread_self/space_self) never
/// copy user memory, so the backing store refuses everything.
struct NoUserBacking;
impl UserBacking for NoUserBacking {
    fn read(&self, _s: u16, _a: u64, _o: &mut [u8]) -> bool {
        false
    }
    fn write(&self, _s: u16, _a: u64, _d: &[u8]) -> bool {
        false
    }
}

// ---- assembly interface ------------------------------------------------------------------

unsafe extern "C" {
    static omni8_isr_table: [u64; 48];
    fn omni8_isr_user_test();
    fn omni8_probe_read(addr: u64);
    fn omni8_probe_write(addr: u64, value: u64);
    fn omni8_probe_exec(addr: u64);
    fn omni8_probe_ud();
    fn omni8_probe_bp();
    fn omni8_probe_de();
    fn omni8_probe_nop();
    fn omni8_enter_user(frame: *const u64) -> !;
    static omni8_probe_sp: u64;
    static omni8_probe_resume: u64;
    static omni8_user_blob_start: u8;
    static omni8_user_blob_end: u8;
    static omni8_user_fault_blob_start: u8;
    static omni8_user_fault_blob_end: u8;
    static omni8_boot_stack_top: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __image_end: u8;
}

macro_rules! sym {
    ($s:ident) => {
        // Taking the address of an extern static is safe; the value is never read.
        (&raw const $s) as u64
    };
}

// ---- port I/O and serial -----------------------------------------------------------------

const COM1: u16 = 0x3F8;
const DEBUG_EXIT: u16 = 0xF4;

fn outb(port: u16, v: u8) {
    // SAFETY: port I/O in ring 0.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags))
    }
}
fn outl(port: u16, v: u32) {
    // SAFETY: port I/O in ring 0.
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack, preserves_flags))
    }
}
fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: port I/O in ring 0.
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags))
    }
    v
}

fn putc(b: u8) {
    for _ in 0..100_000 {
        if inb(COM1 + 5) & 0x20 != 0 {
            outb(COM1, b);
            return;
        }
    }
    // transmitter never became ready: drop the byte rather than hang
}

struct Serial;
impl Write for Serial {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            putc(b);
        }
        Ok(())
    }
}

macro_rules! out {
    ($($a:tt)*) => {{ let _ = write!(Serial, $($a)*); }};
}

fn finish(ok: bool) -> ! {
    if ok {
        out!("OMNI8 TEST PASS\n");
    }
    #[cfg(test_exit)]
    outl(DEBUG_EXIT, if ok { 0x10 } else { 0x11 });
    #[cfg(not(test_exit))]
    let _ = (outl as fn(u16, u32), DEBUG_EXIT);
    loop {
        // SAFETY: stop this CPU for good.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) }
    }
}

fn fail(msg: &str) -> ! {
    out!("OMNI8 FAIL: {}\n", msg);
    finish(false)
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    out!("OMNI8 FAIL: Rust panic\n");
    out!("OMNI8   {}\n", info);
    finish(false)
}

/// The unoptimised build of `core` references the unwinding personality routine even though
/// panics abort. Nothing ever unwinds, so a call here is a bug: stop.
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() -> ! {
    fail("unwinding is not supported")
}

// ---- compiler runtime helpers (no libc is linked) ----------------------------------------

/// # Safety
/// Standard `memcpy` contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // SAFETY: caller contract; rep movsb copies forward.
    unsafe {
        asm!("rep movsb", inout("rcx") n => _, inout("rdi") dst => _, inout("rsi") src => _, options(nostack, preserves_flags))
    };
    dst
}

/// # Safety
/// Standard `memmove` contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dst as usize) <= (src as usize) || (dst as usize) >= (src as usize).wrapping_add(n) {
        // SAFETY: forward copy is safe when the destination does not overlap the tail.
        return unsafe { memcpy(dst, src, n) };
    }
    // SAFETY: overlapping with dst above src: copy backwards.
    unsafe {
        asm!("std", "rep movsb", "cld",
            inout("rcx") n => _,
            inout("rdi") (dst as usize).wrapping_add(n).wrapping_sub(1) => _,
            inout("rsi") (src as usize).wrapping_add(n).wrapping_sub(1) => _,
            options(nostack));
    }
    dst
}

/// # Safety
/// Standard `memset` contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dst: *mut u8, c: i32, n: usize) -> *mut u8 {
    // SAFETY: caller contract.
    unsafe {
        asm!("rep stosb", inout("rcx") n => _, inout("rdi") dst => _, in("al") c as u8, options(nostack, preserves_flags))
    };
    dst
}

/// # Safety
/// Standard `memcmp` contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        // SAFETY: caller contract.
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}

/// # Safety
/// Standard `bcmp` contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    // SAFETY: caller contract.
    unsafe { memcmp(a, b, n) }
}

// ---- control registers -------------------------------------------------------------------

fn read_cr2() -> u64 {
    let v: u64;
    // SAFETY: reading CR2 has no side effects.
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}
fn read_cr3() -> u64 {
    let v: u64;
    // SAFETY: reading CR3 has no side effects.
    unsafe { asm!("mov {}, cr3", out(reg) v, options(nomem, nostack, preserves_flags)) }
    v
}
fn write_cr3(v: u64) {
    // SAFETY: caller passes a root whose mappings cover the running code and stack.
    unsafe { asm!("mov cr3, {}", in(reg) v, options(nostack, preserves_flags)) }
}

// ---- trap dispatch -----------------------------------------------------------------------

const NO_VECTOR: u64 = u64::MAX;
static EXPECT_VECTOR: AtomicU64 = AtomicU64::new(NO_VECTOR);
static OBS_VALID: AtomicBool = AtomicBool::new(false);
static OBS_VECTOR: AtomicU64 = AtomicU64::new(0);
static OBS_ERROR: AtomicU64 = AtomicU64::new(0);
static OBS_CR2: AtomicU64 = AtomicU64::new(0);
static OBS_USER: AtomicBool = AtomicBool::new(false);
static USER_INT_SEEN: AtomicBool = AtomicBool::new(false);
static SPURIOUS: AtomicU64 = AtomicU64::new(0);

/// Called by `isr_common` with the saved register frame. Returning resumes the interrupted
/// context (possibly with a rewritten frame).
///
/// # Safety
/// `f` must point at the frame `isr_common` just built.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn omni8_trap_dispatch(f: *mut TrapFrame) {
    // SAFETY: contract above.
    let f = unsafe { &mut *f };
    let v = f.vector;
    if v == TIMER_VECTOR as u64 {
        TICKS.tick();
        // Lock-free time-slice accounting only: the interrupted thread may hold the scheduler lock.
        PREEMPT.on_tick();
        outb(timer::PIC1_CMD, timer::PIC_EOI);
        return;
    }
    if (0x21..=0x2F).contains(&v) {
        // Only IRQ0 is unmasked, so anything else is spurious. Acknowledge and count it.
        SPURIOUS.fetch_add(1, Ordering::Relaxed);
        if v >= 0x28 {
            outb(timer::PIC2_CMD, timer::PIC_EOI);
        }
        outb(timer::PIC1_CMD, timer::PIC_EOI);
        return;
    }
    if v == VECTOR_USER_TEST as u64 && f.from_user() {
        USER_INT_SEEN.store(true, Ordering::SeqCst);
        return;
    }
    if v == EXPECT_VECTOR.load(Ordering::SeqCst) && !f.from_user() {
        OBS_VECTOR.store(v, Ordering::SeqCst);
        OBS_ERROR.store(f.error, Ordering::SeqCst);
        OBS_CR2.store(read_cr2(), Ordering::SeqCst);
        OBS_USER.store(false, Ordering::SeqCst);
        OBS_VALID.store(true, Ordering::SeqCst);
        EXPECT_VECTOR.store(NO_VECTOR, Ordering::SeqCst);
        // SAFETY: both statics are written by the probe routine before it executes the
        // instruction that traps.
        unsafe {
            f.rip = core::ptr::read_volatile(core::ptr::addr_of!(omni8_probe_resume));
            f.rsp = core::ptr::read_volatile(core::ptr::addr_of!(omni8_probe_sp));
        }
        return;
    }
    let _ = write_fatal(f);
    finish(false);
}

fn write_fatal(f: &TrapFrame) -> fmt::Result {
    omni8::report::write_trap_report(&mut Serial, f, read_cr2(), read_cr3())?;
    writeln!(Serial, "OMNI8 FAIL: unexpected exception")
}

// ---- descriptor tables -------------------------------------------------------------------

#[repr(C, packed)]
struct DescPtr {
    limit: u16,
    base: u64,
}

fn install_gdt_tss() {
    let ist_tops = [
        IST_STACKS[0].top(),
        IST_STACKS[1].top(),
        IST_STACKS[2].top(),
        IST_STACKS[3].top(),
        IST_STACKS[4].top(),
        0,
        0,
    ];
    // IST numbering: 1 fatal, 2 #DF, 3 NMI, 4 #MC, 5 IRQ (see omni8::desc).
    debug_assert!(
        IST_FATAL == 1
            && IST_DOUBLE_FAULT == 2
            && IST_NMI == 3
            && IST_MACHINE_CHECK == 4
            && IST_IRQ == 5
    );
    let tss = match Tss::new(RSP0_STACK.top(), ist_tops) {
        Ok(t) => t,
        Err(_) => fail("tss construction"),
    };
    // SAFETY: single CPU, before any interrupt can use these tables.
    let (tss_ref, gdt_ref) = unsafe { (TSS.get(), GDT.get()) };
    *tss_ref = Some(tss);
    let tss_base = tss_ref
        .as_ref()
        .map(|t| t as *const Tss as u64)
        .unwrap_or(0);
    let gdt = match Gdt::new(tss_base) {
        Ok(g) => g,
        Err(_) => fail("gdt construction"),
    };
    if gdt.validate(tss_base).is_err() {
        fail("gdt validation");
    }
    *gdt_ref = Some(gdt);
    let base = gdt_ref
        .as_ref()
        .map(|g| g.entries.as_ptr() as u64)
        .unwrap_or(0);
    let ptr = DescPtr {
        limit: GDT_LIMIT,
        base,
    };
    // SAFETY: the GDT is 'static and valid; selectors reloaded match its layout; the TSS
    // descriptor at SEL_TSS is present and not busy.
    unsafe {
        asm!("lgdt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
        asm!(
            "pushq $0x08",
            "leaq 2f(%rip), %rax",
            "pushq %rax",
            "lretq",
            "2:",
            "movw $0x10, %ax",
            "movw %ax, %ds",
            "movw %ax, %es",
            "movw %ax, %ss",
            "xorl %eax, %eax",
            "movw %ax, %fs",
            "movw %ax, %gs",
            out("rax") _,
            options(att_syntax)
        );
        asm!("ltr ax", in("ax") SEL_TSS, options(nomem, nostack, preserves_flags));
    }
    out!("OMNI8 GDT: loaded\n");
    out!("OMNI8 TSS: loaded\n");
}

fn install_idt() {
    // SAFETY: single CPU, before interrupts are enabled.
    let idt = unsafe { IDT.get() };
    for v in 0..48usize {
        // SAFETY: the table is defined by isr.S with exactly 48 entries.
        let h = unsafe { omni8_isr_table[v] };
        let e = match IdtEntry::interrupt_gate(h, SEL_KERNEL_CODE, ist_for_vector(v), 0) {
            Ok(e) => e,
            Err(_) => fail("idt gate"),
        };
        if idt.set(v, e).is_err() {
            fail("idt set");
        }
    }
    let user_gate = match IdtEntry::interrupt_gate(
        omni8_isr_user_test as *const () as usize as u64,
        SEL_KERNEL_CODE,
        IST_IRQ,
        3,
    ) {
        Ok(e) => e,
        Err(_) => fail("idt user gate"),
    };
    if idt.set(VECTOR_USER_TEST as usize, user_gate).is_err()
        || idt
            .validate(&[TIMER_VECTOR as usize, VECTOR_USER_TEST as usize])
            .is_err()
    {
        fail("idt validation");
    }
    let ptr = DescPtr {
        limit: IDT_LIMIT,
        base: idt.entries.as_ptr() as u64,
    };
    // SAFETY: the IDT is 'static and every gate was validated.
    unsafe { asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags)) };
    out!("OMNI8 IDT: loaded\n");
}

// ---- memory ------------------------------------------------------------------------------

struct Identity;
impl PhysMem for Identity {
    fn read_u64(&self, phys: u64) -> u64 {
        // SAFETY: page-table frames come from the allocator and are identity mapped.
        unsafe { core::ptr::read_volatile(phys as *const u64) }
    }
    fn write_u64(&mut self, phys: u64, value: u64) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile(phys as *mut u64, value) }
    }
    fn zero_frame(&mut self, phys: u64) {
        // SAFETY: as above; a whole frame.
        unsafe { core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE as usize) }
    }
}

const USER_WINDOW: UserWindow = UserWindow {
    start: 0x4000_0000,
    end: 0x8000_0000,
};
const USER_CODE: u64 = 0x4000_0000;
const USER_STACK_BASE: u64 = 0x4001_0000;
const USER_STACK_TOP: u64 = USER_STACK_BASE + PAGE_SIZE;
const UNMAPPED_PROBE: u64 = 0x5000_0000;

static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);
static USER_ROOT: AtomicU64 = AtomicU64::new(0);

/// The kernel half of every address space: low memory read/write (not the null page), the
/// image mapped by section (text RX, rodata R, data+bss RW), then all RAM below `ram_top`
/// read/write/no-execute. No page is both writable and executable.
fn build_kernel_map<M: PhysMem, A: FrameSource>(
    m: &mut Mapper<M, A>,
    ram_top: u64,
) -> Result<(), MapError> {
    let (ts, te) = (sym!(__text_start), sym!(__text_end));
    let (rs, re) = (sym!(__rodata_start), sym!(__rodata_end));
    let (ds, de) = (sym!(__data_start), sym!(__data_end));
    let ie = sym!(__image_end);
    m.map_range_4k(PAGE_SIZE, PAGE_SIZE, ts - PAGE_SIZE, Perms::KERNEL_RW)?;
    m.map_range_4k(ts, ts, te - ts, Perms::KERNEL_RX)?;
    m.map_range_4k(rs, rs, re - rs, Perms::KERNEL_RO)?;
    m.map_range_4k(ds, ds, de - ds, Perms::KERNEL_RW)?;
    m.map_range_4k(ie, ie, HUGE_SIZE - ie, Perms::KERNEL_RW)?;
    m.map_range_best(HUGE_SIZE, HUGE_SIZE, ram_top - HUGE_SIZE, Perms::KERNEL_RW)
}

fn read_phys_u32(addr: u64) -> Option<u32> {
    if addr % 4 != 0 || addr < PAGE_SIZE || addr + 4 > CEILING {
        return None;
    }
    // SAFETY: inside the boot identity map (first GiB) and 4-aligned.
    Some(unsafe { core::ptr::read_volatile(addr as *const u32) })
}

/// Returns the highest usable RAM address (2 MiB aligned, below the 1 GiB ceiling).
fn init_memory(info_ptr: u64) -> u64 {
    if info_ptr % 4 != 0 || info_ptr < PAGE_SIZE || info_ptr + INFO_BYTES as u64 > CEILING {
        fail("multiboot info location");
    }
    // SAFETY: range checked against the boot identity map just above.
    let info_bytes = unsafe { core::slice::from_raw_parts(info_ptr as *const u8, INFO_BYTES) };
    let info = match multiboot::parse_info(info_bytes) {
        Ok(i) => i,
        Err(_) => fail("multiboot info"),
    };
    let (maddr, mlen) = match multiboot::validate_map_location(&info, CEILING) {
        Ok(x) => x,
        Err(_) => fail("multiboot map location"),
    };
    // SAFETY: validated to lie inside the identity map.
    let map_bytes = unsafe { core::slice::from_raw_parts(maddr as *const u8, mlen) };
    let mut regions = [Region {
        start: 0,
        end: 0,
        kind: 0,
    }; 64];
    let nreg = match multiboot::parse_map(map_bytes, &mut regions) {
        Ok(n) => n,
        Err(_) => fail("multiboot map"),
    };
    let mut reserved = [(0u64, 0u64); 33];
    let nres = match multiboot::reserved_ranges(
        info_ptr,
        info_bytes,
        read_phys_u32,
        &mut reserved[..32],
    ) {
        Ok(n) => n,
        Err(_) => fail("multiboot reserved ranges"),
    };
    // everything below the end of the kernel image is never handed out
    reserved[nres] = (0, sym!(__image_end));
    let mut usable = [(0u64, 0u64); 64];
    let nuse = match multiboot::usable_ranges(
        &regions[..nreg],
        &reserved[..nres + 1],
        CEILING,
        &mut usable,
    ) {
        Ok(n) => n,
        Err(_) => fail("usable ranges"),
    };
    // SAFETY: single CPU; the allocator is only used from the main flow.
    let frames = unsafe { FRAMES.get() };
    let top = usable[..nuse].iter().map(|r| r.1).max().unwrap_or(0);
    // Only RAM below `ram_top` gets mapped (2 MiB granularity), so only that RAM may be
    // handed out: a frame without a mapping would fault the first time it is used.
    let ram_top = top & !(HUGE_SIZE - 1);
    for &(s, e) in &usable[..nuse] {
        if s < ram_top {
            frames.add_range(s, e.min(ram_top));
        }
    }
    if ram_top < 4 * HUGE_SIZE || frames.free_frames() < 256 {
        fail("not enough memory");
    }
    out!("OMNI8 BOOT: multiboot ok\n");
    out!(
        "OMNI8 MEM: map parsed regions={} usable_ranges={}\n",
        nreg,
        nuse
    );
    out!(
        "OMNI8 INFO: free_frames={} ram_top={:#x}\n",
        frames.free_frames(),
        ram_top
    );
    ram_top
}

fn init_paging(ram_top: u64) {
    // SAFETY: single CPU, main flow only.
    let frames = unsafe { FRAMES.get() };
    let mut mem = Identity;
    let mut m = match Mapper::new(&mut mem, frames, USER_WINDOW) {
        Ok(m) => m,
        Err(_) => fail("paging root"),
    };
    if build_kernel_map(&mut m, ram_top).is_err() {
        fail("paging map");
    }
    let root = m.root();
    KERNEL_ROOT.store(root, Ordering::SeqCst);
    write_cr3(root);
    out!("OMNI8 PAGING: wx tables active\n");
}

fn selftest_frames() {
    // SAFETY: single CPU, main flow only.
    let f = unsafe { FRAMES.get() };
    let before = f.free_frames();
    let a = f.alloc();
    let b = f.alloc();
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        _ => fail("frame alloc"),
    };
    if a == b
        || a % FRAME_SIZE != 0
        || b % FRAME_SIZE != 0
        || a < sym!(__image_end)
        || f.is_free(a)
        || f.free_frames() != before - 2
    {
        fail("frame alloc invariants");
    }
    if f.free(a).is_err() || f.free(a).is_ok() || f.free(b).is_err() || f.free_frames() != before {
        fail("frame free invariants");
    }
    out!("OMNI8 FRAMES: alloc/free ok\n");
}

// ---- expected-fault self tests -----------------------------------------------------------

fn run_probe(name: &str, vector: u64, error: Option<u64>, cr2: Option<u64>, body: impl FnOnce()) {
    OBS_VALID.store(false, Ordering::SeqCst);
    EXPECT_VECTOR.store(vector, Ordering::SeqCst);
    body();
    EXPECT_VECTOR.store(NO_VECTOR, Ordering::SeqCst);
    let obs = if OBS_VALID.load(Ordering::SeqCst) {
        Some(Observation {
            vector: OBS_VECTOR.load(Ordering::SeqCst),
            error: OBS_ERROR.load(Ordering::SeqCst),
            cr2: OBS_CR2.load(Ordering::SeqCst),
            from_user: OBS_USER.load(Ordering::SeqCst),
        })
    } else {
        None
    };
    match selftest::check(obs, Expectation { vector, error, cr2 }) {
        Ok(()) => out!("OMNI8 TRAP: {} ok\n", name),
        Err(_) => {
            out!("OMNI8 FAIL: trap probe {} mismatch\n", name);
            if let Some(o) = obs {
                out!(
                    "OMNI8   saw vector={} err={:#x} cr2={:#x}\n",
                    o.vector,
                    o.error,
                    o.cr2
                );
            }
            finish(false)
        }
    }
}

fn selftest_traps() {
    // SAFETY: each probe routine is in isr/probe.S; traps are caught by the dispatcher.
    run_probe("breakpoint", 3, None, None, || unsafe { omni8_probe_bp() });
    run_probe("undefined-opcode", 6, None, None, || unsafe {
        omni8_probe_ud()
    });
    run_probe("divide-error", 0, None, None, || unsafe {
        omni8_probe_de()
    });
    let nop = omni8_probe_nop as *const () as usize as u64;
    // write to kernel text: present + write
    run_probe(
        "write-to-text pagefault",
        14,
        Some(0b00011),
        Some(nop),
        || unsafe { omni8_probe_write(nop, 0x90) },
    );
    // execute a no-exec data page: present + instruction fetch
    // SAFETY: single CPU, main flow only.
    let page = match unsafe { FRAMES.get() }.alloc() {
        Some(p) => p,
        None => fail("probe frame"),
    };
    run_probe(
        "exec-from-nx pagefault",
        14,
        Some(0b10001),
        Some(page),
        || unsafe { omni8_probe_exec(page) },
    );
    run_probe(
        "unmapped-read pagefault",
        14,
        Some(0),
        Some(UNMAPPED_PROBE),
        || unsafe { omni8_probe_read(UNMAPPED_PROBE) },
    );
    run_probe("null-read pagefault", 14, Some(0), Some(0), || unsafe {
        omni8_probe_read(0)
    });
    // positive controls: a mapped read, a write to RW memory and a call into text do not trap
    OBS_VALID.store(false, Ordering::SeqCst);
    EXPECT_VECTOR.store(14, Ordering::SeqCst);
    // SAFETY: `page` is a mapped RW frame; the nop routine is executable text.
    unsafe {
        omni8_probe_write(page, 0xA5);
        omni8_probe_read(page);
        omni8_probe_exec(nop);
    }
    EXPECT_VECTOR.store(NO_VECTOR, Ordering::SeqCst);
    if OBS_VALID.load(Ordering::SeqCst) {
        fail("positive control trapped");
    }
    // SAFETY: single CPU, main flow only; `page` was allocated above.
    if unsafe { FRAMES.get() }.free(page).is_err() {
        fail("probe frame free");
    }
    out!("OMNI8 TRAP: positive controls ok\n");
}

// ---- scheduler, timer --------------------------------------------------------------------

fn selftest_sched() {
    let sched: &Sched = &KERNEL.sched;
    match selftest::run_order_scenario(sched, &ORDER_LOG) {
        Ok(()) => out!(
            "OMNI8 SCHED: order ok switches={}\n",
            selftest::EXPECTED_SWITCHES
        ),
        Err(_) => fail("scheduler order scenario"),
    }
}

fn selftest_timer() {
    let pic = match timer::pic_sequence(0x20, 0x28, 0xFE, 0xFF) {
        Ok(s) => s,
        Err(_) => fail("pic sequence"),
    };
    let pit = match timer::pit_sequence(100) {
        Ok(s) => s,
        Err(_) => fail("pit sequence"),
    };
    for (p, v) in pic.iter().chain(pit.iter()) {
        outb(*p, *v);
    }
    // SAFETY: the IDT is loaded and IRQ0 is the only unmasked source.
    unsafe { asm!("sti", options(nomem, nostack)) };
    let mut spins = 0u64;
    while TICKS.get() < 5 && spins < 200_000_000 {
        // SAFETY: spin hint.
        unsafe { asm!("pause", options(nomem, nostack, preserves_flags)) };
        spins += 1;
    }
    // SAFETY: mask interrupts again; no preemption is ever performed.
    unsafe { asm!("cli", options(nomem, nostack)) };
    outb(timer::PIC1_DATA, 0xFF);
    outb(timer::PIC2_DATA, 0xFF);
    if TICKS.get() < 5 {
        fail("timer produced no ticks");
    }
    out!("OMNI8 TIMER: ticks ok\n");
}

// ---- tick-driven preemption (variant omni8_preempt) --------------------------------------

#[cfg(omni8_preempt)]
mod preempt_stage {
    use super::*;
    use core::sync::atomic::AtomicU32;
    use omni_kernel7::sched::{KERNEL_SPACE, RunExit};

    /// Yields each thread must perform because of a timer-raised request.
    const YIELDS_PER_THREAD: u32 = 3;
    const SPIN_GUARD: u64 = 400_000_000;
    static DONE: AtomicU32 = AtomicU32::new(0);
    static TIMED_OUT: AtomicBool = AtomicBool::new(false);

    extern "C" fn worker(_: usize) {
        let mut yields = 0u32;
        let mut spins = 0u64;
        while yields < YIELDS_PER_THREAD {
            // SAFETY: spin hint.
            unsafe { asm!("pause", options(nomem, nostack, preserves_flags)) };
            spins += 1;
            if spins > SPIN_GUARD {
                TIMED_OUT.store(true, Ordering::SeqCst);
                break;
            }
            // Safe point: honour a request raised by the timer interrupt.
            if PREEMPT.take_request() {
                yields += 1;
                let _ = KERNEL.sched.yield_now();
            }
        }
        DONE.fetch_add(1, Ordering::SeqCst);
    }

    pub fn run() {
        let pic = match timer::pic_sequence(0x20, 0x28, 0xFE, 0xFF) {
            Ok(s) => s,
            Err(_) => fail("preempt: pic sequence"),
        };
        let pit = match timer::pit_sequence(100) {
            Ok(s) => s,
            Err(_) => fail("preempt: pit sequence"),
        };
        for t in 0..2 {
            if KERNEL.sched.spawn(KERNEL_SPACE, worker, t).is_err() {
                fail("preempt: spawn");
            }
        }
        for (p, v) in pic.iter().chain(pit.iter()) {
            outb(*p, *v);
        }
        // SAFETY: the IDT is loaded; IRQ0 is the only unmasked source and its handler touches atomics only.
        unsafe { asm!("sti", options(nomem, nostack)) };
        let exit = KERNEL.sched.run(|_| false);
        // SAFETY: mask interrupts again.
        unsafe { asm!("cli", options(nomem, nostack)) };
        outb(timer::PIC1_DATA, 0xFF);
        outb(timer::PIC2_DATA, 0xFF);
        if exit != RunExit::AllDone {
            fail("preempt: scheduler did not finish");
        }
        if TIMED_OUT.load(Ordering::SeqCst) {
            fail("preempt: no timer-driven request arrived");
        }
        if DONE.load(Ordering::SeqCst) != 2 {
            fail("preempt: a thread did not finish");
        }
        if PREEMPT.honoured() != (2 * YIELDS_PER_THREAD) as u64 {
            fail("preempt: honoured request count");
        }
        if PREEMPT.requests() < PREEMPT.honoured() {
            fail("preempt: more yields than requests");
        }
        if KERNEL.sched.live() != 0 {
            fail("preempt: thread slot leaked");
        }
        out!("OMNI8 PREEMPT: tick-driven yields ok\n");
    }
}

// ---- ring 3 ------------------------------------------------------------------------------

#[cfg(omni8_ring3)]
mod ring3 {
    use super::*;
    use omni_kernel7::msr;
    use omni_kernel7::sched::RunExit;
    use omni_kernel7::syscall::{PerCpu, install_kernel};

    static SYSCALL_STACK: Stack = Stack::new();
    static PERCPU: Global<PerCpu> = Global::new(PerCpu {
        kernel_rsp: 0,
        user_rsp: 0,
    });
    static IRET: Global<[u64; 5]> = Global::new([0; 5]);

    fn space_hook(space: u16) {
        let root = if space == 0 {
            KERNEL_ROOT.load(Ordering::SeqCst)
        } else {
            USER_ROOT.load(Ordering::SeqCst)
        };
        write_cr3(root);
    }

    extern "C" fn user_thread(_: usize) {
        let frame = match omni8::usermode::iret_frame(USER_CODE, USER_STACK_TOP, USER_WINDOW) {
            Ok(f) => f,
            Err(_) => fail("iret frame"),
        };
        // SAFETY: single CPU; the frame array is only used here.
        let p = unsafe {
            *IRET.get() = frame;
            IRET.get().as_ptr()
        };
        // SAFETY: the frame was validated; user mappings exist in the loaded address space.
        unsafe { omni8_enter_user(p) }
    }

    pub fn run(ram_top: u64) {
        #[cfg(omni8_user_fault)]
        let (blob_s, blob_e) = (
            sym!(omni8_user_fault_blob_start),
            sym!(omni8_user_fault_blob_end),
        );
        #[cfg(not(omni8_user_fault))]
        let (blob_s, blob_e) = (sym!(omni8_user_blob_start), sym!(omni8_user_blob_end));
        let blob_len = (blob_e - blob_s) as usize;
        if blob_len == 0 || blob_len > PAGE_SIZE as usize {
            fail("user blob size");
        }
        // SAFETY: single CPU, main flow only.
        let frames = unsafe { FRAMES.get() };
        let (code, stack) = match (frames.alloc(), frames.alloc()) {
            (Some(c), Some(s)) => (c, s),
            _ => fail("user frames"),
        };
        // SAFETY: both frames are identity mapped RW; the blob lies in rodata.
        unsafe {
            core::ptr::write_bytes(code as *mut u8, 0, PAGE_SIZE as usize);
            core::ptr::write_bytes(stack as *mut u8, 0, PAGE_SIZE as usize);
            core::ptr::copy_nonoverlapping(blob_s as *const u8, code as *mut u8, blob_len);
        }
        let mut mem = Identity;
        let mut m = match Mapper::new(&mut mem, frames, USER_WINDOW) {
            Ok(m) => m,
            Err(_) => fail("user root"),
        };
        if build_kernel_map(&mut m, ram_top).is_err()
            || m.map_4k(USER_CODE, code, Perms::USER_RX).is_err()
            || m.map_4k(USER_STACK_BASE, stack, Perms::USER_RW).is_err()
        {
            fail("user mappings");
        }
        // W^X check on what was just built
        match (
            m.translate(USER_CODE),
            m.translate(USER_STACK_BASE),
            m.translate(sym!(__text_start)),
        ) {
            (Some(c), Some(s), Some(k))
                if c.user && c.exec && !c.write && s.user && s.write && !s.exec && !k.user => {}
            _ => fail("user mapping permissions"),
        }
        USER_ROOT.store(m.root(), Ordering::SeqCst);
        out!("OMNI8 RING3: user address space built\n");

        // SAFETY: single CPU; tables are 'static.
        unsafe {
            let tss = TSS.get().as_mut().map(|t| t.set_rsp0(SYSCALL_STACK.top()));
            if !matches!(tss, Some(Ok(()))) {
                fail("tss rsp0");
            }
            let pc = PERCPU.get();
            pc.kernel_rsp = SYSCALL_STACK.top();
            pc.user_rsp = 0;
            msr::init_syscall(pc as *mut PerCpu);
        }
        install_kernel(&KERNEL);
        KERNEL.sched.set_space_hook(Some(space_hook));
        let tid = match KERNEL.sched.spawn(1, user_thread, 0) {
            Ok(t) => t,
            Err(_) => fail("spawn user thread"),
        };
        let exit = KERNEL.sched.run(|_| false);
        write_cr3(KERNEL_ROOT.load(Ordering::SeqCst));
        KERNEL.sched.set_space_hook(None);
        if exit != RunExit::AllDone {
            fail("ring3 run did not complete");
        }
        #[cfg(omni8_user_fault)]
        {
            let _ = tid;
            fail("ring3 fault program returned instead of faulting");
        }
        #[cfg(not(omni8_user_fault))]
        {
            if !USER_INT_SEEN.load(Ordering::SeqCst) {
                fail("ring3 int 0x81 was not observed from CPL 3");
            }
            if KERNEL.sched.exit_code_of(tid) != Some(1) {
                fail("ring3 exit code");
            }
            out!("OMNI8 RING3: cpl3 int81 seen, syscall exit code=1\n");
        }
    }
}

// ---- entry -------------------------------------------------------------------------------

/// Called from boot.S in 64-bit mode on the boot stack.
#[unsafe(no_mangle)]
pub extern "C" fn omni8_main(magic: u32, info: u32) -> ! {
    // SAFETY: clear CR0.TS so SSE use cannot fault with #NM (no lazy FPU switching exists).
    unsafe { asm!("clts", options(nomem, nostack)) };
    out!("OMNI8 BOOT: start\n");
    if multiboot::check_magic(magic).is_err() {
        fail("boot magic");
    }
    // the boot stack must lie inside the image mapping that will be installed
    let sp: u64;
    // SAFETY: reading rsp.
    unsafe { asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags)) };
    if sp > sym!(omni8_boot_stack_top) || sp < sym!(__data_start) {
        fail("boot stack placement");
    }
    #[cfg(omni8_ring3)]
    out!("OMNI8 VARIANT: ring3\n");
    #[cfg(omni8_preempt)]
    out!("OMNI8 VARIANT: preempt\n");
    #[cfg(not(any(omni8_ring3, omni8_preempt)))]
    out!("OMNI8 VARIANT: base\n");
    let ram_top = init_memory(info as u64);
    install_gdt_tss();
    install_idt();
    #[cfg(omni8_fault_trap)]
    {
        // SAFETY: deliberate: nothing expects this #UD, so the fatal path must run.
        unsafe { asm!("ud2", options(nomem, nostack)) };
    }
    init_paging(ram_top);
    selftest_frames();
    selftest_traps();
    selftest_sched();
    selftest_timer();
    #[cfg(omni8_preempt)]
    preempt_stage::run();
    #[cfg(omni8_ring3)]
    ring3::run(ram_top);
    #[cfg(not(omni8_ring3))]
    let _ = ram_top;
    finish(true)
}
