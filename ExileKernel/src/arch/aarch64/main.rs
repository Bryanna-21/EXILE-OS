#![no_std]
#![no_main]
//! Minimal QEMU virt AArch64 entry. MMU remains disabled in this A4 stage.
//! The DTB pointer is reported but not dereferenced until the bounded A5 FDT
//! parser is integrated. No UEFI/DTB support claim is made here.

use core::arch::asm;
use core::panic::PanicInfo;

const UART0: usize = 0x0900_0000; // QEMU virt PL011.

fn uart_write(reg: usize, value: u32) {
    // SAFETY: QEMU virt platform contract maps PL011 registers at UART0.
    unsafe { core::ptr::write_volatile((UART0 + reg) as *mut u32, value) };
}

fn uart_init() {
    // QEMU virt PL011 is clocked at 24 MHz; 13 + 1/64 gives ~115.2 kbaud.
    uart_write(0x30, 0); // CR: disable while configuring.
    uart_write(0x44, 0x7ff); // ICR: clear pending interrupts.
    uart_write(0x24, 13); // IBRD.
    uart_write(0x28, 1); // FBRD.
    uart_write(0x2c, 0x70); // 8 data bits, FIFO enabled, 1 stop, no parity.
    uart_write(0x30, 0x301); // UARTEN | TXE | RXE.
}

fn uart_put(byte: u8) {
    // PL011 FR bit 5 is TXFF. A bounded wait prevents a wedged device hanging
    // diagnostics forever; this is an early-boot platform-specific access.
    let fr = (UART0 + 0x18) as *const u32;
    for _ in 0..100_000 {
        // SAFETY: QEMU virt platform contract maps PL011 at UART0 with FR at +0x18.
        if unsafe { core::ptr::read_volatile(fr) } & (1 << 5) == 0 {
            // SAFETY: QEMU virt platform contract maps the PL011 data register at UART0.
            unsafe { core::ptr::write_volatile(UART0 as *mut u32, byte as u32) };
            return;
        }
        core::hint::spin_loop();
    }
}

fn serial(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            uart_put(b'\r');
        }
        uart_put(b);
    }
}

fn hex(mut n: u64) {
    let mut buf = [b'0'; 16];
    for i in (0..16).rev() {
        let d = (n & 0xf) as usize;
        buf[i] = b"0123456789abcdef"[d];
        n >>= 4;
    }
    serial("0x");
    for b in buf {
        uart_put(b);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn aarch64_main(dtb: u64) -> ! {
    uart_init();
    serial("EUK: AArch64 EL1 entry\n");
    serial("EUK: MMU disabled; identity physical addressing\n");
    serial("EUK: QEMU virt PL011 ready; DTB pointer=");
    hex(dtb);
    serial("\nEUK: A4 boot path reached\n");
    halt()
}

#[unsafe(no_mangle)]
pub extern "C" fn aarch64_exception(kind: u64) -> ! {
    uart_init();
    serial("EUK FAIL: AArch64 exception vector=");
    hex(kind);
    serial(" ESR_EL1=");
    let esr: u64;
    // SAFETY: this handler is entered from an EL1 exception vector.
    unsafe {
        asm!("mrs {0}, esr_el1", out(reg) esr, options(nomem, nostack, preserves_flags));
    }
    hex(esr);
    serial(" ELR_EL1=");
    let elr: u64;
    // SAFETY: this handler is entered from an EL1 exception vector.
    unsafe {
        asm!("mrs {0}, elr_el1", out(reg) elr, options(nomem, nostack, preserves_flags));
    }
    hex(elr);
    serial("\n");
    halt()
}

fn halt() -> ! {
    // SAFETY: privileged EL1 boot context; interrupts are masked before WFE.
    unsafe {
        asm!("msr daifset, #0xf", "1: wfe", "b 1b", options(noreturn));
    }
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    uart_init();
    serial("EUK FAIL: Rust panic\n");
    halt()
}
