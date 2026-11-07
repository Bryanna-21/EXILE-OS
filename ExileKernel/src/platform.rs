//! Explicit capability contract for the isolated EUK milestone.
//!
//! This is deliberately separate from the inherited `Kernel/` HAL. It describes
//! only facilities that EUK's boot path actually initializes; it does not turn
//! the inherited kernel interfaces into EUK implementations.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    Aarch64,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Implemented,
    NotImplemented,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facility {
    BootEntry,
    BootMemoryMap,
    SerialOutput,
    ExceptionDispatch,
    InterruptController,
    Timer,
    VirtualMemory,
    Smp,
    Pci,
    Dma,
    PowerManagement,
    NativeUserspace,
    CompatibilityRuntime,
    GuestVirtualization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootProfile {
    pub architecture: Architecture,
    pub boot_entry: Support,
    pub boot_memory_map: Support,
    pub serial_output: Support,
    pub exception_dispatch: Support,
    pub interrupt_controller: Support,
    pub timer: Support,
    pub virtual_memory: Support,
    pub smp: Support,
    pub pci: Support,
    pub dma: Support,
    pub power_management: Support,
    pub native_userspace: Support,
    pub compatibility_runtime: Support,
    pub guest_virtualization: Support,
}

impl BootProfile {
    pub const fn supports(self, facility: Facility) -> Support {
        match facility {
            Facility::BootEntry => self.boot_entry,
            Facility::BootMemoryMap => self.boot_memory_map,
            Facility::SerialOutput => self.serial_output,
            Facility::ExceptionDispatch => self.exception_dispatch,
            Facility::InterruptController => self.interrupt_controller,
            Facility::Timer => self.timer,
            Facility::VirtualMemory => self.virtual_memory,
            Facility::Smp => self.smp,
            Facility::Pci => self.pci,
            Facility::Dma => self.dma,
            Facility::PowerManagement => self.power_management,
            Facility::NativeUserspace => self.native_userspace,
            Facility::CompatibilityRuntime => self.compatibility_runtime,
            Facility::GuestVirtualization => self.guest_virtualization,
        }
    }
}

const NO: Support = Support::NotImplemented;
const YES: Support = Support::Implemented;

/// The current EUK x86-64 milestone enters through Multiboot v1, parses a
/// bounded loader memory map, and emits bounded early serial diagnostics. It
/// does not yet initialize interrupts, timers, user mode, or a scheduler.
pub const fn boot_profile() -> BootProfile {
    #[cfg(target_arch = "x86_64")]
    {
        BootProfile {
            architecture: Architecture::X86_64,
            boot_entry: YES,
            boot_memory_map: YES,
            serial_output: YES,
            exception_dispatch: NO,
            interrupt_controller: NO,
            timer: NO,
            virtual_memory: NO,
            smp: NO,
            pci: NO,
            dma: NO,
            power_management: NO,
            native_userspace: NO,
            compatibility_runtime: NO,
            guest_virtualization: NO,
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        BootProfile {
            architecture: Architecture::Aarch64,
            boot_entry: NO,
            boot_memory_map: NO,
            serial_output: NO,
            exception_dispatch: NO,
            interrupt_controller: NO,
            timer: NO,
            virtual_memory: NO,
            smp: NO,
            pci: NO,
            dma: NO,
            power_management: NO,
            native_userspace: NO,
            compatibility_runtime: NO,
            guest_virtualization: NO,
        }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        BootProfile {
            architecture: Architecture::Unsupported,
            boot_entry: NO,
            boot_memory_map: NO,
            serial_output: NO,
            exception_dispatch: NO,
            interrupt_controller: NO,
            timer: NO,
            virtual_memory: NO,
            smp: NO,
            pci: NO,
            dma: NO,
            power_management: NO,
            native_userspace: NO,
            compatibility_runtime: NO,
            guest_virtualization: NO,
        }
    }
}

pub const fn architecture_name() -> &'static str {
    match boot_profile().architecture {
        Architecture::X86_64 => "x86_64",
        Architecture::Aarch64 => "aarch64 (backend not implemented)",
        Architecture::Unsupported => "unsupported",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_target_has_consistent_architecture_profile() {
        let profile = boot_profile();
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(profile.architecture, Architecture::X86_64);
            assert_eq!(profile.supports(Facility::BootEntry), Support::Implemented);
            assert_eq!(profile.supports(Facility::BootMemoryMap), Support::Implemented);
            assert_eq!(profile.supports(Facility::SerialOutput), Support::Implemented);
            assert_eq!(profile.supports(Facility::InterruptController), Support::NotImplemented);
        }
        #[cfg(target_arch = "aarch64")]
        {
            assert_eq!(profile.architecture, Architecture::Aarch64);
            assert_eq!(profile.supports(Facility::BootEntry), Support::NotImplemented);
        }
    }

    #[test]
    fn no_userspace_environment_is_claimed_for_euk_milestone_one() {
        let profile = boot_profile();
        assert_eq!(profile.supports(Facility::NativeUserspace), Support::NotImplemented);
        assert_eq!(profile.supports(Facility::CompatibilityRuntime), Support::NotImplemented);
        assert_eq!(profile.supports(Facility::GuestVirtualization), Support::NotImplemented);
    }
}
