//! Capabilities and bounded IPC, integrated with the kernel (Missions B3 and B4).
//! SPDX-License-Identifier: Apache-2.0
//!
//! B3 modules (capability model, object table, messages, endpoints, notifications, the
//! scheduler-integrated `IpcDomain` and the `0x40..=0x48` syscall ABI) plus the B4 kernel
//! integration modules:
//!
//! | module | role |
//! |---|---|
//! | `kernel_domain` | the one authoritative, lock-protected `IpcDomain` and its lock order |
//! | `scheduler_bridge` | genuine blocking: park/switch away, interrupt a blocked thread |
//! | `resume` | pending-operation table and completion of blocked syscalls |
//! | `user_memory` | `UserMemory` over a process address space |
//! | `ipc_service` | kernel `IpcService`: process/thread ownership, accurate errors |
//! | `syscall_dispatch` | routes `0x40..=0x48` out of the D5 dispatch path |
//! | `thread_lifecycle` | IPC cleanup when a thread exits or is killed |
//! | `process_lifecycle` | IPC cleanup when a process exits |
//! | `initialization` | ordered init / shutdown |
//!
//! The B3 files refer to one another with `super::`, so the same sources work both as the
//! children of this module (kernel) and as flat modules at a crate root (`scripts/test_b3.py`).
#![allow(dead_code)]

pub mod cap_rights;
pub mod cap_space;
pub mod ipc;
pub mod ipc_abi;
pub mod ipc_message;
pub mod kobject;

mod ipc_endpoint;
mod ipc_notify;

pub mod initialization;
pub mod ipc_service;
pub mod kernel_domain;
pub mod process_lifecycle;
pub mod resume;
pub mod scheduler_bridge;
pub mod syscall_dispatch;
pub mod thread_lifecycle;
pub mod user_memory;

#[allow(unused_imports)]
pub use cap_rights::{Kind, Rights};
#[allow(unused_imports)]
pub use kobject::ObjId;
