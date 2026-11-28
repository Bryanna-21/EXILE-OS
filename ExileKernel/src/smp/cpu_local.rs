//! Cache-line-aligned per-CPU state and GS-base installation contract.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

pub const IA32_GS_BASE: u32 = 0xC000_0101;
pub const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

#[repr(C, align(64))]
pub struct CpuLocal {
    cpu_id: AtomicU32,
    apic_id: AtomicU32,
    preempt_count: AtomicU32,
    interrupt_depth: AtomicU32,
    online: AtomicBool,
    current_thread: AtomicUsize,
    kernel_stack_top: AtomicUsize,
}

impl CpuLocal {
    pub const fn new(cpu_id: u32, apic_id: u32) -> Self {
        Self {
            cpu_id: AtomicU32::new(cpu_id),
            apic_id: AtomicU32::new(apic_id),
            preempt_count: AtomicU32::new(0),
            interrupt_depth: AtomicU32::new(0),
            online: AtomicBool::new(false),
            current_thread: AtomicUsize::new(0),
            kernel_stack_top: AtomicUsize::new(0),
        }
    }

    pub fn cpu_id(&self) -> u32 {
        self.cpu_id.load(Ordering::Relaxed)
    }
    pub fn apic_id(&self) -> u32 {
        self.apic_id.load(Ordering::Relaxed)
    }
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Acquire)
    }
    pub fn set_online(&self, online: bool) {
        self.online.store(online, Ordering::Release);
    }
    pub fn preempt_count(&self) -> u32 {
        self.preempt_count.load(Ordering::Acquire)
    }
    pub fn interrupt_depth(&self) -> u32 {
        self.interrupt_depth.load(Ordering::Acquire)
    }
    pub fn current_thread(&self) -> usize {
        self.current_thread.load(Ordering::Acquire)
    }
    pub fn set_current_thread(&self, value: usize) {
        self.current_thread.store(value, Ordering::Release);
    }
    pub fn kernel_stack_top(&self) -> usize {
        self.kernel_stack_top.load(Ordering::Acquire)
    }
    pub fn set_kernel_stack_top(&self, value: usize) {
        self.kernel_stack_top.store(value, Ordering::Release);
    }

    pub fn enter_interrupt(&self) -> Result<u32, CounterError> {
        increment(&self.interrupt_depth)
    }
    pub fn leave_interrupt(&self) -> Result<u32, CounterError> {
        decrement(&self.interrupt_depth)
    }
    pub fn disable_preemption(&self) -> Result<u32, CounterError> {
        increment(&self.preempt_count)
    }
    pub fn enable_preemption(&self) -> Result<u32, CounterError> {
        decrement(&self.preempt_count)
    }
    pub fn may_preempt(&self) -> bool {
        self.preempt_count() == 0 && self.interrupt_depth() == 0 && self.is_online()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CounterError {
    Overflow,
    Underflow,
}

fn increment(counter: &AtomicU32) -> Result<u32, CounterError> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1))
        .map(|old| old + 1)
        .map_err(|_| CounterError::Overflow)
}
fn decrement(counter: &AtomicU32) -> Result<u32, CounterError> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_sub(1))
        .map(|old| old - 1)
        .map_err(|_| CounterError::Underflow)
}

/// Implemented by the architecture layer; the operation is privileged on x86-64.
pub trait MsrWriter {
    /// # Safety
    /// Caller must execute at CPL0 on the CPU whose GS base is being installed.
    unsafe fn write_msr(&self, msr: u32, value: u64);
}

/// # Safety
/// `local` must be pinned for the lifetime of this CPU's GS-base reference, mapped
/// writable, and not aliased through a mutable reference while the CPU is running.
pub unsafe fn install_gs_base(writer: &impl MsrWriter, local: &'static CpuLocal) {
    let address = local as *const CpuLocal as u64;
    // SAFETY: delegated to this function's caller and the MsrWriter contract.
    unsafe {
        writer.write_msr(IA32_GS_BASE, address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_and_preempt_nesting_are_balanced() {
        let cpu = CpuLocal::new(0, 2);
        cpu.set_online(true);
        assert!(cpu.may_preempt());
        cpu.disable_preemption().unwrap();
        cpu.enter_interrupt().unwrap();
        assert!(!cpu.may_preempt());
        cpu.leave_interrupt().unwrap();
        cpu.enable_preemption().unwrap();
        assert!(cpu.may_preempt());
    }

    #[test]
    fn underflow_is_reported() {
        let cpu = CpuLocal::new(0, 0);
        assert_eq!(cpu.enable_preemption(), Err(CounterError::Underflow));
    }
}
