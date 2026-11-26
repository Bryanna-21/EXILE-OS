//! Architecture-neutral, bounded CPU topology records. No AP startup/SMP here.
#![allow(dead_code)]
pub const MAX_CPUS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    AArch64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cpu {
    pub logical_id: u16,
    pub hardware_id: u64,
    pub architecture: Architecture,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyError {
    TooMany,
    DuplicateHardwareId,
    WrongArchitecture,
}

pub struct Topology {
    cpus: [Option<Cpu>; MAX_CPUS],
    len: usize,
    arch: Architecture,
}
impl Topology {
    pub const fn new(arch: Architecture) -> Self {
        Self {
            cpus: [None; MAX_CPUS],
            len: 0,
            arch,
        }
    }
    pub fn push(&mut self, hardware_id: u64, enabled: bool) -> Result<u16, TopologyError> {
        if self.len == MAX_CPUS {
            return Err(TopologyError::TooMany);
        }
        if self.cpus[..self.len]
            .iter()
            .flatten()
            .any(|c| c.hardware_id == hardware_id)
        {
            return Err(TopologyError::DuplicateHardwareId);
        }
        let logical_id = self.len as u16;
        self.cpus[self.len] = Some(Cpu {
            logical_id,
            hardware_id,
            architecture: self.arch,
            enabled,
        });
        self.len += 1;
        Ok(logical_id)
    }
    pub fn ensure_boot_cpu(&mut self, hardware_id: u64) -> Result<(), TopologyError> {
        for cpu in self.cpus[..self.len].iter_mut().flatten() {
            if cpu.hardware_id == hardware_id {
                cpu.enabled = true;
                return Ok(());
            }
        }
        self.push(hardware_id, true)?;
        Ok(())
    }
    pub fn cpus(&self) -> impl Iterator<Item = &Cpu> {
        self.cpus[..self.len].iter().flatten()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logical_ids_are_stable_in_insertion_order() {
        let mut t = Topology::new(Architecture::AArch64);
        assert_eq!(t.push(0x100, true), Ok(0));
        assert_eq!(t.push(0x102, false), Ok(1));
        assert_eq!(
            t.cpus().map(|c| c.hardware_id).collect::<Vec<_>>(),
            vec![0x100, 0x102]
        );
    }
    #[test]
    fn duplicate_ids_fail_closed() {
        let mut t = Topology::new(Architecture::X86_64);
        t.push(7, true).unwrap();
        assert_eq!(t.push(7, false), Err(TopologyError::DuplicateHardwareId));
    }
    #[test]
    fn safe_single_cpu_fallback() {
        let mut t = Topology::new(Architecture::X86_64);
        t.ensure_boot_cpu(2).unwrap();
        t.ensure_boot_cpu(2).unwrap();
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn boot_cpu_is_inserted_when_other_cpus_exist() {
        let mut t = Topology::new(Architecture::X86_64);
        t.push(4, true).unwrap();
        t.ensure_boot_cpu(2).unwrap();
        assert_eq!(t.len(), 2);
        let boot = t.cpus().find(|cpu| cpu.hardware_id == 2).unwrap();
        assert!(boot.enabled);
        assert_eq!(boot.logical_id, 1);
    }

    #[test]
    fn disabled_boot_cpu_is_enabled() {
        let mut t = Topology::new(Architecture::AArch64);
        t.push(7, false).unwrap();
        t.ensure_boot_cpu(7).unwrap();
        assert!(t.cpus().find(|cpu| cpu.hardware_id == 7).unwrap().enabled);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn boot_cpu_insertion_respects_capacity() {
        let mut t = Topology::new(Architecture::X86_64);
        for id in 0..MAX_CPUS as u64 {
            t.push(id, true).unwrap();
        }
        assert_eq!(t.ensure_boot_cpu(100), Err(TopologyError::TooMany));
    }
}
