//! OMNI-Kernel Batch 10 (experimental, `no_std`).
//!
//! Addresses the software-fixable findings of the Batch 9 security review:
//!  * `txmap`  (SEC-14) mapping that rolls back completely on failure, and table-tree teardown;
//!  * `irqlock` (SEC-05) a spin lock that masks interrupts through an explicit control trait;
//!  * `abi`    (SEC-18, SEC-20) a syscall version query and a register-convention adaptor.
//!
//! EVERYTHING HERE IS TESTED HOSTED ONLY. No test ran in ring 0, under QEMU or on hardware.
//! A hosted model of interrupts is not proof that hardware interrupt synchronisation is correct.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::manual_is_multiple_of)]

#[cfg(test)]
extern crate std;

pub mod abi;
pub mod irqlock;
pub mod txmap;
