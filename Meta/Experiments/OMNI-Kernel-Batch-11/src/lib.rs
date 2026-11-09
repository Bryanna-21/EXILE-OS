//! OMNI-Kernel Batch 11 (experimental, `no_std`): enforced capabilities and channel IPC.
//!
//! Before this batch the Rust kernel (Batches 7 to 10) had no capabilities and no IPC; they existed
//! only as Zig models (Batches 2 to 5). This crate implements a conformant SUBSET in Rust:
//!  * `caps` per-space handle tables: 32-bit handles (generation << 16 | index), rights, retirement of
//!    exhausted slots; every operation checks rights, a handle never confers authority by itself;
//!  * `ipc`  channel ends with bounded queues, blocking receive with timeout, move/copy capability
//!    transfer, object lifetime by reachability (cycles are collected), cleanup on space exit;
//!  * `sys`  syscalls 0x100..=0x107 with the Batch 5 argument layouts, on top of the Batch 10 ABI layer.
//!
//! Deviations from the Zig model are listed in docs/IPC_DESIGN.md. HOSTED TESTS ONLY: nothing ran under
//! QEMU or on hardware, and the scheduler is still cooperative and uniprocessor.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::manual_is_multiple_of)]

#[cfg(test)]
extern crate std;

pub mod caps;
pub mod ipc;
pub mod status;
pub mod sys;
