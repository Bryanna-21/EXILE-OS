//! E3 virtual-memory foundation. Not wired into the boot path.
pub mod vm;
pub use vm::{PageTableVm, VmConfig, VmCreateError, VmError};
