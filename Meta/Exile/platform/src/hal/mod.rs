//! Hardware abstraction layer: discovery and description of hardware.
//!
//! Capability levels used throughout (and in `Documentation/Architecture/Platform/HAL.md`):
//! **Discovery** (find and identify) < **Initialization** (program registers) < **Data transfer** < **Full driver**.
//! This crate reaches *Discovery* for most classes and *Initialization-logic* (register programming against a mock
//! register file) for the local APIC only. No data-transfer path or full driver exists.

pub mod acpi;
pub mod cpu;
pub mod devices;
pub mod fb;
pub mod lapic;
pub mod memmap;
pub mod pci;
pub mod registry;
