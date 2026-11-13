//! OMNI-Kernel Batch 8: the hosted-testable core of the bootable experimental kernel.
//!
//! Everything in this crate is pure logic (descriptor encoders, table builders, allocators,
//! parsers, report formatting). It performs no port I/O, touches no control register and
//! dereferences no raw address, so it is unit-tested on an ordinary host. The code that
//! installs these structures into a CPU lives in `kernel/` and the assembly files and is
//! only ever exercised by a boot, never by `cargo test`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
// `is_multiple_of` needs Rust 1.90; this crate keeps an MSRV of 1.88.
#![allow(clippy::manual_is_multiple_of)]

#[cfg(test)]
extern crate std;

pub mod desc;
pub mod frames;
pub mod idt;
pub mod multiboot;
pub mod paging;
pub mod preempt;
pub mod report;
pub mod selftest;
pub mod timer;
pub mod usermode;

/// Is `addr` a canonical 48-bit virtual address (bits 63:47 all equal)?
pub const fn is_canonical(addr: u64) -> bool {
    let top = (addr as i64) >> 47;
    top == 0 || top == -1
}
