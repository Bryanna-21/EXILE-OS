#![no_std]
#![no_main]
mod boot_foundation;
mod bootinfo;
mod cpu;
mod frame;
mod gdt;
mod idt;
mod memory;

// E3 virtual-memory foundation and its shared primitives.
#[allow(dead_code)]
mod page_perms;
#[allow(dead_code)]
mod frame_alloc;
#[allow(dead_code)]
mod virt_addr;
#[allow(dead_code)]
mod e3;

// Mission A2: early physical-memory allocator and reservation contracts.
#[allow(dead_code)]
mod early_alloc;
mod memmap_norm;
#[allow(dead_code)]
mod phys_handoff;
mod range;
mod reserve;
mod stacks;
mod traps;
// Mission C3: interrupt controllers, ACPI discovery and timers. Declared so the image
// compiles and links them; they only RUN under the `euk_irq` cfg (build.py --irq), see
// `irq_selftest`. The default boot path is unchanged and never enables interrupts.
#[allow(dead_code)]
mod acpi;
#[allow(dead_code)]
mod apic;
#[allow(dead_code)]
mod hpet;
#[allow(dead_code)]
mod ioapic;
#[allow(dead_code)]
mod irq;
#[allow(dead_code)]
mod pic;
#[allow(dead_code)]
mod pit;
#[allow(dead_code)]
mod timer;
// Mission C4 SMP modules are compiled but remain opt-in; the default boot path stays BSP-only.
#[allow(dead_code)]
mod smp;
// Mission D3. Declared so the kernel image contains and links the real
// scheduler, stack manager and context-switch assembly; they only RUN under the
// `euk_d3_demo` cfg (see d3_demo.rs). The normal boot path is unchanged.
#[allow(dead_code)]
#[path = "arch/x86_64/context.rs"]
mod context;
#[cfg(euk_d3_demo)]
mod d3_demo;
#[allow(dead_code)]
mod dispatch;
#[cfg(euk_host_mem)]
mod hostmem;
#[allow(dead_code)]
mod kstack;
#[allow(dead_code)]
mod kthread;
#[allow(dead_code)]
mod runqueue;
#[allow(dead_code)]
mod sched;
#[allow(dead_code)]
mod spin;
#[allow(dead_code)]
#[path = "arch/x86_64/switch.rs"]
mod switch;
#[allow(dead_code)]
mod thread;

// D6 and its D5 dependencies are compiled only for the explicit --d6 build.
// They are not activated by the normal boot path.
#[cfg(euk_d6)]
#[allow(dead_code)]
#[path = "d5/address_space.rs"]
mod address_space;
#[cfg(euk_d6)]
#[allow(dead_code)]
#[path = "d5/elf.rs"]
mod elf;
#[cfg(euk_d6)]
#[allow(dead_code)]
#[path = "d5/preempt.rs"]
mod preempt;
#[cfg(euk_d6)]
#[allow(dead_code)]
#[path = "d5/process.rs"]
mod process;
#[cfg(euk_d6)]
#[allow(dead_code)]
#[path = "d5/sync.rs"]
mod sync;
#[cfg(euk_d6)]
#[allow(dead_code)]
#[path = "d5/tls.rs"]
mod tls;
#[cfg(euk_d6)]
#[allow(dead_code)]
#[path = "d5/usermode.rs"]
mod usermode;
// B3/B4 capability and IPC integration is compiled only with explicit --d6.
#[cfg(euk_d6)]
#[allow(dead_code)]
mod b3;

#[cfg(euk_d6)]
#[allow(dead_code)]
#[path = "d6/mod.rs"]
mod d6;
use core::arch::asm;
use core::panic::PanicInfo;
use memory::{Region, parse_map};
const MAP_LIMIT: usize = 1024 * 1024 * 1024;
unsafe fn out(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
fn serial(text: &str) {
    for byte in text.bytes() {
        let mut ready = false;
        for _ in 0..100_000 {
            let status: u8;
            unsafe {
                asm!("in al, dx", in("dx") 0x3fdu16, out("al") status, options(nomem, nostack, preserves_flags));
            }
            if status & 0x20 != 0 {
                ready = true;
                break;
            }
        }
        if ready {
            unsafe {
                out(0x3f8, byte);
            }
        }
    }
}
fn finish(success: bool) -> ! {
    #[cfg(test_exit)]
    unsafe {
        asm!("out dx, eax", in("dx") 0xf4u16, in("eax") if success { 0x10u32 } else { 0x11u32 }, options(nomem, nostack));
    }
    let _ = success;
    loop {
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}
fn fail(reason: &str) -> ! {
    serial("EUK FAIL: ");
    serial(reason);
    serial("\n");
    finish(false)
}

// Linker-provided bounds of the loaded kernel image.
unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

const BOOTSTRAP_PREFIX: u64 = 16 * 1024 * 1024;
const MULTIBOOT_INFO_BYTES: u64 = 116;
type KernelAllocator = early_alloc::EarlyAllocator<64, 16, 64>;

#[inline(never)]
fn init_physical_memory(
    info: u64,
    map_address: u64,
    map_length: u64,
    regions: &[Region],
    alloc: &mut KernelAllocator,
) -> Result<(), &'static str> {
    use reserve::ReserveKind;

    let info_end = info
        .checked_add(MULTIBOOT_INFO_BYTES)
        .filter(|end| *end <= MAP_LIMIT as u64)
        .ok_or("boot info reservation bounds")?;
    let map_end = map_address
        .checked_add(map_length)
        .filter(|end| *end <= MAP_LIMIT as u64)
        .ok_or("memory map reservation bounds")?;

    let mut intervals = [memmap_norm::Interval::EMPTY; 256];
    let n = memmap_norm::normalize_regions(regions, &mut intervals)
        .map_err(|_| "memory map normalize")?;

    let mut reservations = reserve::ReservationSet::<16>::new();
    let kernel_start = core::ptr::addr_of!(__kernel_start) as u64;
    let kernel_end = core::ptr::addr_of!(__kernel_end) as u64;

    let mut protect = |start: u64, end: u64, kind: ReserveKind| -> Result<(), &'static str> {
        let range = range::PhysRange::new(start, end).map_err(|_| "reservation range")?;
        reservations
            .add(range, kind)
            .map_err(|_| "reservation capacity")
    };

    protect(0, BOOTSTRAP_PREFIX, ReserveKind::Bootstrap)?;
    protect(kernel_start, kernel_end, ReserveKind::KernelImage)?;
    protect(info, info_end, ReserveKind::BootInfo)?;
    protect(map_address, map_end, ReserveKind::MemoryMap)?;

    #[cfg(fault_a2_nomem)]
    let ceiling = 0u64;
    #[cfg(not(fault_a2_nomem))]
    let ceiling = MAP_LIMIT as u64;

    alloc
        .init(
            memmap_norm::usable_ranges(&intervals[..n]),
            reservations,
            ceiling,
        )
        .map_err(|_| "early allocator init")
}

#[cfg(euk_traps)]
#[unsafe(no_mangle)]
extern "C" fn euk_trap_dispatch(frame: &mut frame::TrapFrame) {
    // External interrupts (vector >= 32) go to the IRQ layer once a controller is installed.
    // Exceptions (< 32) and every vector while no controller exists take the unchanged fatal
    // path below. The interrupt gate cleared IF, so this runs uninterrupted.
    #[cfg(euk_irq)]
    if let Some(v) = frame.vector_u8() {
        if irq::is_external_vector(v)
            && CONTROLLER_READY.load(core::sync::atomic::Ordering::Acquire)
        {
            dispatch_irq(v);
            return;
        }
    }

    if !traps::enter_trap() {
        fail("nested trap");
    }

    let cr2 = if frame.vector == 14 {
        unsafe { traps::read_cr2() }
    } else {
        0
    };

    let report = unsafe { traps::trap_report() };
    let action = traps::handle(frame, cr2, report);
    serial(report.as_str());

    match action {
        traps::Action::Resume => traps::leave_trap(),
        traps::Action::Fatal => finish(false),
    }
}

#[cfg(euk_irq)]
static mut CONTROLLER: Option<pic::Pic<pic::HwPorts>> = None;
#[cfg(euk_irq)]
static CONTROLLER_READY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Runs in interrupt context (IF = 0, one CPU).
#[cfg(euk_irq)]
fn dispatch_irq(vector: u8) {
    // SAFETY: CONTROLLER is written once, before CONTROLLER_READY is set, and afterwards is
    // touched only with interrupts disabled (interrupt context, or `irq_selftest` between its
    // `cli` windows), so this is the only live reference.
    let ctrl = unsafe { &mut *(&raw mut CONTROLLER) };
    if let Some(c) = ctrl {
        irq::dispatch(&irq::IRQS, c, vector);
    }
}

#[cfg(euk_irq)]
fn with_pic<R>(f: impl FnOnce(&mut pic::Pic<pic::HwPorts>) -> R) -> R {
    // SAFETY: callers run with interrupts disabled; see `dispatch_irq`.
    match unsafe { &mut *(&raw mut CONTROLLER) } {
        Some(p) => f(p),
        None => fail("pic missing"),
    }
}

#[cfg(euk_irq)]
fn serial_hex(value: u64) {
    let mut buf = [b'0'; 18];
    buf[1] = b'x';
    for i in 0..16 {
        buf[2 + i] = b"0123456789abcdef"[((value >> (60 - 4 * i)) & 0xf) as usize];
    }
    serial(core::str::from_utf8(&buf).unwrap_or("?"));
}

#[cfg(euk_irq)]
fn serial_dec(mut value: u64) {
    let mut buf = [0u8; 20];
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    serial(core::str::from_utf8(&buf[at..]).unwrap_or("?"));
}

/// Waits (bounded) with interrupts enabled until `done()` holds; interrupts are disabled again
/// before returning. Returns whether the condition was reached.
#[cfg(euk_irq)]
fn wait_with_interrupts(done: impl Fn() -> bool) -> bool {
    // SAFETY: only called from `irq_selftest`, after the IDT is loaded, the PIC is initialised,
    // and every handler/lock requirement in irq.rs holds (handlers use atomics only).
    unsafe { irq::enable_interrupts() };
    let mut reached = false;
    for _ in 0..400_000_000u32 {
        if done() {
            reached = true;
            break;
        }
        core::hint::spin_loop();
    }
    // SAFETY: ring 0.
    unsafe { irq::disable_interrupts() };
    reached
}

/// C3 hardware-facing self-test. Order follows the C3 rules: IDT already loaded (trap_selftest),
/// controller initialised with every line masked, handlers registered, EOI path exercised, timer
/// started, and only then interrupts enabled, for bounded windows.
#[cfg(euk_irq)]
fn irq_selftest() {
    use timer::TimerSource;
    let limit = MAP_LIMIT as u64;

    // 1. Controller.
    // SAFETY: ring 0, interrupts disabled since `_start`; nothing else drives the PIC ports.
    let mut p = pic::Pic::with_default_base(unsafe { pic::HwPorts::new() });
    if p.init().is_err() {
        fail("pic init");
    }
    // SAFETY: first and only write; READY is set after it.
    unsafe { *(&raw mut CONTROLLER) = Some(p) };
    CONTROLLER_READY.store(true, core::sync::atomic::Ordering::Release);
    serial("EUK: PIC remapped to 0x20\n");

    // 2. ACPI discovery (read-only, bounded to the identity-mapped GiB).
    // SAFETY: 0..MAP_LIMIT is identity mapped by the bootstrap.
    let reader = unsafe { acpi::IdentityReader::new(limit) };
    match acpi::madt::discover_bios(&reader) {
        Ok(m) => {
            serial("EUK: MADT cpus=");
            serial_dec(m.cpu_count as u64);
            serial(" ioapics=");
            serial_dec(m.ioapic_count as u64);
            serial(" lapic=");
            serial_hex(m.lapic_address);
            serial("\n");
            if apic::mmio_reachable(m.lapic_address, 0x1000, limit) {
                serial("EUK: local APIC window mapped (driver not started in boot selftest)\n");
            } else {
                serial("EUK: APIC MMIO outside bootstrap map; PIC fallback in use\n");
            }
        }
        Err(_) => serial("EUK: MADT unavailable\n"),
    }
    let hpet_table = hpet::discover_bios(&reader).ok();
    let hpet_reachable = hpet_table
        .is_some_and(|t| apic::mmio_reachable(t.base, hpet::HPET_WINDOW_LEN as u64, limit));
    let kind = timer::choose_source(hpet_table.is_some(), hpet_reachable);

    // 3. Timer source.
    let mut pit_source = pit::Pit::new(
        // SAFETY: ring 0; the PIT ports are not used elsewhere.
        unsafe { pic::HwPorts::new() },
    );
    let mut hpet_source = match (kind, hpet_table) {
        (timer::SourceKind::Hpet, Some(t)) => {
            // SAFETY: `hpet_reachable` proved the window lies inside the identity map.
            let mmio = unsafe { apic::VolatileMmio::new(t.base as usize, hpet::HPET_WINDOW_LEN) };
            hpet::Hpet::new(mmio, t.min_tick)
                .ok()
                .map(hpet::HpetTimer::new)
        }
        _ => None,
    };
    let source: &mut dyn TimerSource = match hpet_source.as_mut() {
        Some(h) => {
            serial("EUK: timer source HPET\n");
            h
        }
        None => {
            serial("EUK: timer source PIT\n");
            &mut pit_source
        }
    };
    let rate = match source.start_periodic(100) {
        Ok(r) => r,
        Err(_) => fail("timer start"),
    };
    if timer::CLOCK.configure(rate).is_err() {
        fail("timer rate");
    }
    let vector = irq::LEGACY_BASE; // IRQ0

    // 4. Unhandled-interrupt containment: nobody owns the vector, so the dispatcher must
    //    acknowledge each tick and mask the line at the threshold.
    with_pic(|p| p.unmask_irq(0)).unwrap_or_else(|_| fail("unmask irq0"));
    let storm =
        wait_with_interrupts(|| irq::IRQS.unhandled_count(vector) >= irq::UNHANDLED_MASK_THRESHOLD);
    if !storm {
        fail("no timer irq delivered");
    }
    let masked = with_pic(|p| p.masks() & 1 != 0);
    if !masked || irq::IRQS.unhandled_count(vector) != irq::UNHANDLED_MASK_THRESHOLD {
        fail("unhandled irq not contained");
    }
    serial("EUK: unhandled irq contained\n");

    // 5. Registered handler: ticks must be counted and acknowledged.
    if timer::install(&irq::IRQS, vector).is_err() {
        fail("timer install");
    }
    timer::CLOCK.reset();
    with_pic(|p| p.unmask_irq(0)).unwrap_or_else(|_| fail("unmask irq0"));
    if !wait_with_interrupts(|| timer::CLOCK.ticks() >= 5) {
        fail("timer ticks not counted");
    }
    with_pic(|p| p.mask_irq(0)).unwrap_or_else(|_| fail("mask irq0"));
    source.stop();
    let ticks = timer::CLOCK.ticks();
    let ns = timer::CLOCK.now_ns().unwrap_or(0);
    if timer::uninstall(&irq::IRQS, vector).is_err() {
        fail("timer uninstall");
    }
    serial("EUK: PIC timer ok ticks=");
    serial_dec(ticks);
    serial(" ns=");
    serial_dec(ns);
    serial("\n");

    // 6. Quiesce: everything masked, interrupts off.
    with_pic(|p| p.disable_all()).unwrap_or_else(|_| fail("pic quiesce"));
    serial("EUK: irq selftest ok\n");
}

#[cfg(euk_traps)]
fn trap_selftest() {
    if unsafe { traps::init_bsp(MAP_LIMIT as u64) }.is_err() {
        fail("trap setup");
    }

    serial("EUK: GDT/TSS/IDT loaded\n");

    #[cfg(fault_trap_ud)]
    unsafe {
        asm!("ud2", options(noreturn));
    }

    #[cfg(fault_trap_df)]
    unsafe {
        asm!("mov rsp, 0x80000000", "int3", options(noreturn));
    }

    #[cfg(not(any(fault_trap_ud, fault_trap_df)))]
    {
        unsafe {
            asm!("int3");
        }
        serial("EUK: breakpoint returned\n");
    }
}
/// All unsafe loader accesses stay inside the mapped GiB with bounded lengths.
/// Loader authenticity and physical RAM validity are bootloader trust assumptions.
unsafe fn read32(address: usize) -> u32 {
    unsafe { core::ptr::read_unaligned(address as *const u32) }
}
#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(magic: u32, info: u32) -> ! {
    serial("EUK: x86-64 long mode\n");
    if magic != 0x2badb002 {
        fail("boot magic");
    }
    #[cfg_attr(fault_handoff, allow(unused_variables))]
    let info = info as usize;
    #[cfg(fault_handoff)]
    let info = MAP_LIMIT - 4;
    if info < 4096 || info.checked_add(52).is_none_or(|end| end > MAP_LIMIT) {
        fail("handoff bounds");
    }
    let flags = unsafe { read32(info) };
    #[cfg(fault_map_missing)]
    let flags = flags & !(1 << 6);
    if flags & (1 << 6) == 0 {
        fail("memory map missing");
    }
    #[cfg_attr(fault_map_length, allow(unused_variables))]
    let length = unsafe { read32(info + 44) } as usize;
    #[cfg(fault_map_length)]
    let length = 65537;
    let address = unsafe { read32(info + 48) } as usize;
    if length == 0
        || length > 65536
        || address < 4096
        || address
            .checked_add(length)
            .is_none_or(|end| end > MAP_LIMIT)
    {
        fail("memory map bounds");
    }
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    #[cfg(fault_map_truncated)]
    let bytes = &bytes[..3];
    let mut regions = [Region {
        start: 0,
        end: 0,
        kind: 0,
    }; 128];
    let count = match parse_map(bytes, &mut regions) {
        Ok(n) => n,
        Err(_) => fail("memory map invalid"),
    };
    if !regions[..count]
        .iter()
        .any(|r| memory::usable_pages(*r, 16 * 1024 * 1024, MAP_LIMIT as u64).is_some())
    {
        fail("usable memory missing");
    }
    serial("EUK: bounded memory map parsed\n");

    let mut alloc = KernelAllocator::new();
    if let Err(reason) = init_physical_memory(
        info as u64,
        address as u64,
        length as u64,
        &regions[..count],
        &mut alloc,
    ) {
        fail(reason);
    }

    // Verify one real page is allocated outside the protected bootstrap prefix.
    match alloc.alloc_pages(1) {
        Ok(page) if page.start() >= BOOTSTRAP_PREFIX && page.end() <= MAP_LIMIT as u64 => {}
        _ => fail("early allocator self-check"),
    }

    if alloc.check_invariants().is_err() {
        fail("early allocator audit");
    }
    serial("EUK: early allocator ready\n");

    #[cfg(euk_traps)]
    trap_selftest();
    #[cfg(euk_irq)]
    irq_selftest();
    after_boot()
}
#[cfg(not(euk_d3_demo))]
fn after_boot() -> ! {
    serial("EUK BOOT_OK\n");
    finish(true)
}
#[cfg(euk_d3_demo)]
fn after_boot() -> ! {
    d3_demo::run()
}
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    fail("Rust panic")
}
