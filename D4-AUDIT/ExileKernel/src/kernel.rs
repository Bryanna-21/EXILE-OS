#![no_std]
#![no_main]
mod cpu;
mod gdt;
mod idt;
mod memory;
// Mission D3. Declared so the kernel image contains and links the real
// scheduler, stack manager and context-switch assembly; they only RUN under the
// `euk_d3_demo` cfg (see d3_demo.rs). The normal boot path is unchanged.
#[allow(dead_code)]
#[path = "arch/x86_64/context.rs"]
mod context;
#[cfg(euk_d3_demo)]
mod d3_demo;
// Mission D4: interrupt layer, timer and preemption. Declared so the image
// contains and links them; they only RUN under `euk_d4_demo` (see d4_demo.rs).
#[cfg(euk_d4_demo)]
mod d4_demo;
#[allow(dead_code)]
mod dispatch;
#[allow(dead_code)]
mod irq;
#[cfg(euk_host_mem)]
mod hostmem;
#[allow(dead_code)]
mod kstack;
#[allow(dead_code)]
mod kthread;
#[allow(dead_code)]
#[path = "arch/x86_64/pic.rs"]
mod pic;
#[allow(dead_code)]
mod preempt;
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
#[allow(dead_code)]
mod timer;
#[allow(dead_code)]
#[path = "arch/x86_64/trap.rs"]
mod trap;
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
    after_boot()
}
#[cfg(not(any(euk_d3_demo, euk_d4_demo)))]
fn after_boot() -> ! {
    serial("EUK BOOT_OK\n");
    finish(true)
}
#[cfg(euk_d3_demo)]
fn after_boot() -> ! {
    d3_demo::run()
}
#[cfg(euk_d4_demo)]
fn after_boot() -> ! {
    d4_demo::run()
}
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    fail("Rust panic")
}
