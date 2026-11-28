//! ACPI MADT processor topology parsing and validation.

pub const MAX_CPUS: usize = 64;
pub const MADT_TYPE_LOCAL_APIC: u8 = 0;
pub const MADT_TYPE_LOCAL_X2APIC: u8 = 9;
pub const CPU_FLAG_ENABLED: u32 = 1;
pub const CPU_FLAG_ONLINE_CAPABLE: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuInfo {
    pub uid: u32,
    pub apic_id: u32,
    pub flags: u32,
    pub is_bsp: bool,
}

impl CpuInfo {
    pub const EMPTY: Self = Self {
        uid: 0,
        apic_id: 0,
        flags: 0,
        is_bsp: false,
    };

    pub const fn is_usable(self) -> bool {
        self.flags & (CPU_FLAG_ENABLED | CPU_FLAG_ONLINE_CAPABLE) != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopologyError {
    TruncatedEntry,
    InvalidEntryLength,
    TooManyCpus,
    DuplicateUid,
    DuplicateApicId,
    MissingBsp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Topology {
    cpus: [Option<CpuInfo>; MAX_CPUS],
    count: usize,
    bsp_apic_id: Option<u32>,
}

impl Topology {
    pub const fn new() -> Self {
        Self {
            cpus: [None; MAX_CPUS],
            count: 0,
            bsp_apic_id: None,
        }
    }

    pub fn insert(&mut self, mut cpu: CpuInfo) -> Result<(), TopologyError> {
        if !cpu.is_usable() {
            return Ok(());
        }
        if self.iter().any(|old| old.uid == cpu.uid) {
            return Err(TopologyError::DuplicateUid);
        }
        if self.iter().any(|old| old.apic_id == cpu.apic_id) {
            return Err(TopologyError::DuplicateApicId);
        }
        if self.count == MAX_CPUS {
            return Err(TopologyError::TooManyCpus);
        }
        cpu.is_bsp = self.bsp_apic_id == Some(cpu.apic_id);
        self.cpus[self.count] = Some(cpu);
        self.count += 1;
        Ok(())
    }

    pub fn set_bsp(&mut self, apic_id: u32) {
        self.bsp_apic_id = Some(apic_id);
        for cpu in self.cpus.iter_mut().flatten() {
            cpu.is_bsp = cpu.apic_id == apic_id;
        }
    }

    pub fn bsp(&self) -> Option<&CpuInfo> {
        self.iter().find(|cpu| cpu.is_bsp)
    }

    pub const fn len(&self) -> usize {
        self.count
    }
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn get(&self, index: usize) -> Option<&CpuInfo> {
        self.cpus.get(index).and_then(Option::as_ref)
    }

    pub fn find_apic_id(&self, apic_id: u32) -> Option<&CpuInfo> {
        self.iter().find(|cpu| cpu.apic_id == apic_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &CpuInfo> {
        self.cpus[..self.count].iter().filter_map(Option::as_ref)
    }

    pub fn parse_madt_entries(entries: &[u8], bsp_apic_id: u32) -> Result<Self, TopologyError> {
        let mut result = Self::new();
        result.set_bsp(bsp_apic_id);
        let mut offset = 0usize;
        while offset < entries.len() {
            if entries.len() - offset < 2 {
                return Err(TopologyError::TruncatedEntry);
            }
            let kind = entries[offset];
            let len = entries[offset + 1] as usize;
            if len < 2
                || offset
                    .checked_add(len)
                    .filter(|end| *end <= entries.len())
                    .is_none()
            {
                return Err(TopologyError::InvalidEntryLength);
            }
            let entry = &entries[offset..offset + len];
            match kind {
                MADT_TYPE_LOCAL_APIC => {
                    if len < 8 {
                        return Err(TopologyError::InvalidEntryLength);
                    }
                    let flags = read_u32(entry, 4)?;
                    result.insert(CpuInfo {
                        uid: entry[2] as u32,
                        apic_id: entry[3] as u32,
                        flags,
                        is_bsp: false,
                    })?;
                }
                MADT_TYPE_LOCAL_X2APIC => {
                    if len < 16 {
                        return Err(TopologyError::InvalidEntryLength);
                    }
                    let apic_id = read_u32(entry, 4)?;
                    let flags = read_u32(entry, 8)?;
                    let uid = read_u32(entry, 12)?;
                    result.insert(CpuInfo {
                        uid,
                        apic_id,
                        flags,
                        is_bsp: false,
                    })?;
                }
                _ => {}
            }
            offset += len;
        }
        if result.bsp().is_none() {
            return Err(TopologyError::MissingBsp);
        }
        Ok(result)
    }
}

impl Default for Topology {
    fn default() -> Self {
        Self::new()
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, TopologyError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(TopologyError::TruncatedEntry)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_and_x2apic_entries() {
        let mut bytes = [0u8; 24];
        bytes[0] = 0;
        bytes[1] = 8;
        bytes[2] = 0;
        bytes[3] = 2;
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        bytes[8] = 9;
        bytes[9] = 16;
        bytes[12..16].copy_from_slice(&300u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        let topology = Topology::parse_madt_entries(&bytes, 2).unwrap();
        assert_eq!(topology.len(), 2);
        assert!(topology.bsp().is_some_and(|cpu| cpu.apic_id == 2));
        assert!(topology.find_apic_id(300).is_some());
    }

    #[test]
    fn rejects_duplicate_apic_ids() {
        let mut topology = Topology::new();
        topology
            .insert(CpuInfo {
                uid: 1,
                apic_id: 7,
                flags: 1,
                is_bsp: false,
            })
            .unwrap();
        assert_eq!(
            topology.insert(CpuInfo {
                uid: 2,
                apic_id: 7,
                flags: 1,
                is_bsp: false
            }),
            Err(TopologyError::DuplicateApicId)
        );
    }

    #[test]
    fn ignores_disabled_processors() {
        let mut topology = Topology::new();
        topology
            .insert(CpuInfo {
                uid: 1,
                apic_id: 7,
                flags: 0,
                is_bsp: false,
            })
            .unwrap();
        assert!(topology.is_empty());
    }
}
