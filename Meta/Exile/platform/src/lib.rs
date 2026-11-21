//! EXILE platform foundations (Work Orders 2, 4 and 6).
//!
//! `#![no_std]`, no dependencies, no allocation: every structure is fixed-capacity or borrows caller memory, so the
//! same code can later be linked into a kernel or bootloader. Everything here is exercised on the **host** with mock
//! memory and mock hardware; nothing has run on a booted kernel, in QEMU, or on real hardware. Each module's docs
//! state which part is decoding/validation (testable on the host) and which part would need real hardware.
//!
//! Unsafe code is confined to [`sync`] (a ticket lock needs interior mutability); every `unsafe` block carries a
//! `SAFETY:` comment, and `unsafe_op_in_unsafe_fn` is denied.
#![no_std]

#[cfg(test)]
extern crate std;

/// Boot handoff and update state.
pub mod boot;
/// Hardware abstraction layer (parsers, decoders, registry).
pub mod hal;
/// Synchronisation primitives and lock-order validation.
pub mod sync;
