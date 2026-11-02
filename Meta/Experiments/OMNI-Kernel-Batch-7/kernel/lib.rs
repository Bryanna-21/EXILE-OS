//! OMNI-Kernel Batch 7: kernel-side scheduler, context switch, syscall entry logic and
//! user-address validation, written as a `no_std` library for the experimental
//! `ExileKernel/` (Rust, x86-64).
//!
//! WHAT RUNS WHERE (see docs/ARCHITECTURE.md for the full table):
//!  * `context` + `sched`: REAL code. The x86-64 context switch is real assembly; the
//!    hosted tests run it on real stacks in an ordinary process (they switch stacks and
//!    registers for real) with a real scheduler on top. It has NOT run in ring 0.
//!  * `syscall` + `uaccess`: real logic, tested hosted. A "user thread" in the tests is a
//!    kernel-mode coroutine that calls the same function the entry stub calls.
//!  * `msr` (+ the entry stub in `arch/x86_64/syscall_entry.S`): real code that is
//!    ASSEMBLED and byte-checked but NEVER EXECUTED. It needs ring 0.
//!
//! Nothing here is wired into `ExileKernel/` or any boot image.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
// `is_multiple_of` needs Rust 1.90; this crate keeps an MSRV of 1.88.
#![allow(clippy::manual_is_multiple_of)]

#[cfg(test)]
extern crate std;

pub mod context;
pub mod msr;
pub mod sched;
pub mod status;
pub mod sync;
pub mod syscall;
pub mod uaccess;

#[cfg(test)]
mod tests;
