//! SMP and advanced-interrupt foundations for EUK Mission C4.
//!
//! This module deliberately exposes hardware through narrow traits. The default
//! kernel boot path does not enable SMP; platform code must establish APIC MMIO
//! mappings, per-CPU stacks/GDT/TSS state, and an AP trampoline before invoking
//! the startup interfaces.

pub mod cpu_local;
pub mod fpu;
pub mod ipi;
pub mod local_apic;
pub mod msi;
pub mod preempt;
pub mod shootdown;
pub mod startup;
pub mod ticket_lock;
pub mod timer;
pub mod topology;

pub const MAX_CPUS: usize = topology::MAX_CPUS;
