//! E2: bounded kernel infrastructure contracts.
//!
//! These modules are intentionally not wired into `kernel.rs`. They provide
//! allocation-free metadata and adapter contracts; they do not perform device
//! I/O, program timers, dispatch hardware IRQs, or schedule threads.

pub mod bus;
pub mod deadline;
pub mod deadline_admission;
pub mod deadline_queue;
pub mod device;
pub mod device_event;
pub mod device_id;
pub mod device_registry;
pub mod driver;
pub mod driver_match;
pub mod driver_registry;
pub mod init;
pub mod init_graph;
pub mod resource;
pub mod resource_ledger;
pub mod resource_scope;
pub mod softirq;
pub mod work_item;
pub mod work_queue;
