#![no_std]
#![no_main]
#[allow(dead_code)]
mod bootinfo;
mod early_alloc;
mod memmap_norm;
mod memory;
mod phys_handoff;
mod range;
mod reserve;
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
// Linker-provided bounds of the loaded kernel image (physical == virtual here).
unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}
const BOOTSTRAP_PREFIX: u64 = 16 * 1024 * 1024;
const MULTIBOOT_INFO_BYTES: u64 = 116;
type KernelAllocator = early_alloc::EarlyAllocator<64, 16, 64>;

/// A2: builds the reservation set from validated boot data and initializes the
/// early physical allocator. No heap is used. Returns a static reason on failure.
#[inline(never)]
fn init_physical_memory(
    info: u64,
    map_address: u64,
    map_length: u64,
    regions: &[Region],
    alloc: &mut KernelAllocator,
) -> Result<(), &'static str> {
    use reserve::ReserveKind;
    let mut intervals = [memmap_norm::Interval::EMPTY; 256];
    let n = memmap_norm::normalize_regions(regions, &mut intervals)
        .map_err(|_| "memory map normalize")?;
    let mut res = reserve::ReservationSet::<16>::new();
    let kernel_start = core::ptr::addr_of!(__kernel_start) as u64;
    let kernel_end = core::ptr::addr_of!(__kernel_end) as u64;
    let mut protect = |start: u64, end: u64, kind: ReserveKind| -> Result<(), &'static str> {
        let r = range::PhysRange::new(start, end).map_err(|_| "reservation range")?;
        res.add(r, kind).map_err(|_| "reservation capacity")
    };
    // Legacy low memory (BIOS data, EBDA, VGA, loader scratch) and the whole
    // bootstrap prefix that the linker script guarantees the image fits in.
    protect(0, BOOTSTRAP_PREFIX, ReserveKind::Bootstrap)?;
    protect(kernel_start, kernel_end, ReserveKind::KernelImage)?;
    protect(info, info + MULTIBOOT_INFO_BYTES, ReserveKind::BootInfo)?;
    protect(map_address, map_address + map_length, ReserveKind::MemoryMap)?;
    #[cfg(fault_a2_nomem)]
    let ceiling = 0u64;
    #[cfg(not(fault_a2_nomem))]
    let ceiling = MAP_LIMIT as u64;
    alloc
        .init(memmap_norm::usable_ranges(&intervals[..n]), res, ceiling)
        .map_err(|_| "early allocator init")
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
    if let Err(reason) = init_physical_memory(info as u64, address as u64, length as u64, &regions[..count], &mut alloc) {
        fail(reason);
    }
    // Self-check: one page comes from above the bootstrap prefix and the audit holds.
    match alloc.alloc_pages(1) {
        Ok(page) if page.start() >= BOOTSTRAP_PREFIX && page.end() <= MAP_LIMIT as u64 => {}
        _ => fail("early allocator self-check"),
    }
    if alloc.check_invariants().is_err() {
        fail("early allocator audit");
    }
    serial("EUK: early allocator ready\nEUK BOOT_OK\n");
    finish(true)
}
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    fail("Rust panic")
}
