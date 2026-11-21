#![no_std]
#![no_main]
// Mission C: CPU identification, descriptor encoders, exception frame, stacks and trap setup.
// Nothing below runs in the default image. `--cfg euk_traps` (build.py --traps) additionally
// loads the GDT/TSS/IDT and self-tests exception delivery; that path is exercised only by the
// emulator cases in scripts/boot_test.py.
mod cpu;
mod frame;
mod gdt;
mod idt;
mod memory;
mod thread;
mod runqueue;
mod sched;
mod spin;
mod kstack;
#[path = "arch/x86_64/context.rs"]
mod context;
mod kthread;
mod stacks;
mod traps;
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
/// Called by `exceptions.S` with the saved register frame; the frame is 16-byte aligned and
/// stays valid until the stub returns. Runs with interrupts disabled on whatever stack the gate
/// selected (an IST stack for #DF/NMI/#MC). Must not allocate: there is no heap.
#[cfg(euk_traps)]
#[unsafe(no_mangle)]
extern "C" fn euk_trap_dispatch(frame: &mut frame::TrapFrame) {
    if !traps::enter_trap() {
        // A fault while reporting a fault: print nothing that could fault again.
        fail("nested trap");
    }
    let cr2 = if frame.vector == 14 {
        // SAFETY: ring 0; CR2 read has no side effects.
        unsafe { traps::read_cr2() }
    } else {
        0
    };
    // SAFETY: enter_trap() succeeded above, so this handler owns the buffer until it resumes
    // (leave_trap) or halts.
    let report = unsafe { traps::trap_report() };
    let action = traps::handle(frame, cr2, report);
    serial(report.as_str());
    match action {
        traps::Action::Resume => traps::leave_trap(),
        traps::Action::Fatal => finish(false),
    }
}

/// Loads GDT/TSS/IDT and proves that exceptions are delivered. Variants are selected by
/// `--cfg fault_trap_*` for the negative emulator cases.
#[cfg(euk_traps)]
fn trap_selftest() {
    // SAFETY: ring 0, interrupts disabled since _start, bootstrap map active and covering the
    // first GiB; called exactly once.
    if unsafe { traps::init_bsp(MAP_LIMIT as u64) }.is_err() {
        fail("trap setup");
    }
    serial("EUK: GDT/TSS/IDT loaded\n");
    #[cfg(fault_trap_ud)]
    // SAFETY: deliberately raises #UD; the handler reports and halts.
    unsafe {
        asm!("ud2", options(noreturn));
    }
    #[cfg(fault_trap_df)]
    // SAFETY: deliberately breaks the stack so delivering #BP faults (#PF) and delivering that
    // faults again (#DF), which must be taken on the IST1 stack; the handler reports and halts.
    unsafe {
        asm!("mov rsp, 0x80000000", "int3", options(noreturn));
    }
    #[cfg(not(any(fault_trap_ud, fault_trap_df)))]
    {
        // SAFETY: #BP is resumable; the handler returns and execution continues here.
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
    #[cfg(euk_traps)]
    trap_selftest();
    serial("EUK: bounded memory map parsed\nEUK BOOT_OK\n");
    finish(true)
}
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    fail("Rust panic")
}
