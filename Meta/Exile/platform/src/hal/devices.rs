//! Register-level decoders for storage/USB controllers and block/net traits.
//!
//! Status: DISCOVERY-LEVEL ONLY. These decode capability registers a driver would read. There is
//! NO command submission, NO DMA, NO data transfer here: no NVMe/AHCI/xHCI/network *driver* exists.

/// NVMe controller capabilities (CAP register, 64-bit) and version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvmeCaps {
    /// Max queue entries supported (MQES + 1).
    pub max_queue_entries: u32,
    /// Doorbell stride in bytes (4 << DSTRD).
    pub doorbell_stride: u32,
    /// Worst-case ready timeout, in ms (CAP.TO * 500ms).
    pub timeout_ms: u32,
    /// Supports NVM command set.
    pub nvm_command_set: bool,
    /// Min memory page size, bytes (4096 << MPSMIN).
    pub min_page: u32,
    /// Max memory page size, bytes (4096 << MPSMAX).
    pub max_page: u32,
}

/// Decode NVMe CAP.
pub fn decode_nvme_cap(cap: u64) -> NvmeCaps {
    NvmeCaps {
        max_queue_entries: (cap & 0xFFFF) as u32 + 1,
        timeout_ms: ((cap >> 24) & 0xFF) as u32 * 500,
        doorbell_stride: 4 << ((cap >> 32) & 0xF),
        nvm_command_set: (cap >> 37) & 1 == 1,
        min_page: 4096 << ((cap >> 48) & 0xF),
        max_page: 4096 << ((cap >> 52) & 0xF),
    }
}
/// Decode NVMe VS to (major, minor, tertiary).
pub fn decode_nvme_vs(vs: u32) -> (u16, u8, u8) {
    ((vs >> 16) as u16, (vs >> 8) as u8, vs as u8)
}

/// AHCI host capabilities summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AhciCaps {
    /// Number of ports the controller implements as a maximum (NP + 1).
    pub max_ports: u8,
    /// Command slots per port (NCS + 1).
    pub command_slots: u8,
    /// 64-bit addressing supported.
    pub addr64: bool,
    /// Native command queueing supported.
    pub ncq: bool,
    /// Bitmask of implemented ports (PI).
    pub ports_implemented: u32,
}

/// Decode AHCI CAP and PI.
pub fn decode_ahci(cap: u32, pi: u32) -> AhciCaps {
    AhciCaps {
        max_ports: (cap & 0x1F) as u8 + 1,
        command_slots: ((cap >> 8) & 0x1F) as u8 + 1,
        addr64: cap >> 31 & 1 == 1,
        ncq: cap >> 30 & 1 == 1,
        ports_implemented: pi,
    }
}
/// AHCI port signature classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SataDevice {
    /// ATA disk.
    Ata,
    /// ATAPI (optical).
    Atapi,
    /// Port multiplier.
    PortMultiplier,
    /// Enclosure management bridge.
    Enclosure,
    /// Unknown signature.
    Unknown,
}
/// Classify by port signature register, requiring device-present + PHY-established (SSTS.DET==3, IPM==1).
pub fn classify_sata(ssts: u32, sig: u32) -> Option<SataDevice> {
    if ssts & 0xF != 3 || (ssts >> 8) & 0xF != 1 {
        return None;
    }
    Some(match sig {
        0x0000_0101 => SataDevice::Ata,
        0xEB14_0101 => SataDevice::Atapi,
        0x9669_0101 => SataDevice::PortMultiplier,
        0xC33C_0101 => SataDevice::Enclosure,
        _ => SataDevice::Unknown,
    })
}

/// xHCI capability registers summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XhciCaps {
    /// Capability length (offset to operational registers).
    pub cap_length: u8,
    /// Interface version (BCD).
    pub version: u16,
    /// Device slots.
    pub max_slots: u8,
    /// Interrupters.
    pub max_interrupters: u16,
    /// Root-hub ports.
    pub max_ports: u8,
    /// 64-bit addressing.
    pub addr64: bool,
}
/// Decode xHCI CAPLENGTH/HCIVERSION dword and HCSPARAMS1/HCCPARAMS1. `None` if CAPLENGTH < 0x20 (spec minimum).
pub fn decode_xhci(cap_ver: u32, hcs1: u32, hcc1: u32) -> Option<XhciCaps> {
    let cap_length = cap_ver as u8;
    if cap_length < 0x20 {
        return None;
    }
    Some(XhciCaps {
        cap_length,
        version: (cap_ver >> 16) as u16,
        max_slots: hcs1 as u8,
        max_interrupters: ((hcs1 >> 8) & 0x7FF) as u16,
        max_ports: (hcs1 >> 24) as u8,
        addr64: hcc1 & 1 == 1,
    })
}

/// Block device interface a future driver must implement.
pub trait BlockDevice {
    /// Logical block size in bytes.
    fn block_size(&self) -> u32;
    /// Total blocks.
    fn block_count(&self) -> u64;
    /// Read blocks.
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    /// Write blocks.
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
}
/// Network device interface.
pub trait NetDevice {
    /// MAC address.
    fn mac(&self) -> [u8; 6];
    /// Transmit one frame.
    fn send(&mut self, frame: &[u8]) -> Result<(), BlockError>;
    /// Receive one frame into `buf`, returning its length, or None if none pending.
    fn recv(&mut self, buf: &mut [u8]) -> Option<usize>;
}
/// I/O failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// Range outside device or buffer not a block multiple.
    OutOfRange,
    /// Device error.
    Io,
}
/// Validate an I/O request against a device geometry (overflow-safe). Returns block count.
pub fn check_io(
    block_size: u32,
    block_count: u64,
    lba: u64,
    buf_len: usize,
) -> Result<u64, BlockError> {
    if block_size == 0
        || buf_len == 0
        || buf_len / block_size as usize * block_size as usize != buf_len
    {
        return Err(BlockError::OutOfRange);
    }
    let n = (buf_len / block_size as usize) as u64;
    match lba.checked_add(n) {
        Some(end) if end <= block_count => Ok(n),
        _ => Err(BlockError::OutOfRange),
    }
}

/// In-memory block device for host tests. It is a MOCK: passing tests against it says nothing
/// about any physical controller.
pub struct RamDisk {
    /// Backing store.
    pub data: &'static mut [u8],
    /// Block size.
    pub bs: u32,
}
impl BlockDevice for RamDisk {
    fn block_size(&self) -> u32 {
        self.bs
    }
    fn block_count(&self) -> u64 {
        self.data.len() as u64 / u64::from(self.bs)
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check_io(self.bs, self.block_count(), lba, buf.len())?;
        let o = (lba * u64::from(self.bs)) as usize;
        buf.copy_from_slice(&self.data[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        check_io(self.bs, self.block_count(), lba, buf.len())?;
        let o = (lba * u64::from(self.bs)) as usize;
        self.data[o..o + buf.len()].copy_from_slice(buf);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nvme_cap_decoding() {
        // MQES=0x3FF, TO=0x14, DSTRD=0, CSS bit37, MPSMIN=0, MPSMAX=4
        let cap = 0x3FF | 0x14 << 24 | 1 << 37 | 4 << 52;
        let c = decode_nvme_cap(cap);
        assert_eq!(
            (c.max_queue_entries, c.timeout_ms, c.doorbell_stride),
            (1024, 10_000, 4)
        );
        assert!(c.nvm_command_set);
        assert_eq!((c.min_page, c.max_page), (4096, 65536));
        assert_eq!(decode_nvme_vs(0x0001_0400), (1, 4, 0));
        assert_eq!(decode_nvme_cap(2 << 32).doorbell_stride, 16);
    }
    #[test]
    fn ahci_decoding_and_signatures() {
        let c = decode_ahci(1 << 31 | 1 << 30 | 31 << 8 | 5, 0b101);
        assert_eq!(
            (
                c.max_ports,
                c.command_slots,
                c.addr64,
                c.ncq,
                c.ports_implemented
            ),
            (6, 32, true, true, 5)
        );
        let up = 3 | 1 << 8;
        assert_eq!(classify_sata(up, 0x101), Some(SataDevice::Ata));
        assert_eq!(classify_sata(up, 0xEB140101), Some(SataDevice::Atapi));
        assert_eq!(classify_sata(up, 0x1234), Some(SataDevice::Unknown));
        assert_eq!(
            classify_sata(1 | 1 << 8, 0x101),
            None,
            "DET!=3: no phy comm"
        );
        assert_eq!(classify_sata(3 | 2 << 8, 0x101), None, "IPM!=active");
    }
    #[test]
    fn xhci_decoding() {
        let x = decode_xhci(0x0110_0020, 64 | 8 << 8 | 4 << 24, 1).unwrap();
        assert_eq!(
            (
                x.cap_length,
                x.version,
                x.max_slots,
                x.max_interrupters,
                x.max_ports,
                x.addr64
            ),
            (0x20, 0x110, 64, 8, 4, true)
        );
        assert!(decode_xhci(0x0110_0010, 0, 0).is_none());
    }
    #[test]
    fn io_range_checks_are_overflow_safe() {
        assert_eq!(check_io(512, 100, 98, 1024), Ok(2));
        assert_eq!(check_io(512, 100, 99, 1024), Err(BlockError::OutOfRange));
        assert_eq!(
            check_io(512, 100, u64::MAX, 512),
            Err(BlockError::OutOfRange)
        );
        assert_eq!(check_io(512, 100, 0, 100), Err(BlockError::OutOfRange));
        assert_eq!(check_io(512, 100, 0, 0), Err(BlockError::OutOfRange));
        assert_eq!(check_io(0, 100, 0, 512), Err(BlockError::OutOfRange));
    }
    #[test]
    fn ramdisk_round_trip_and_bounds() {
        let d: &'static mut [u8] = std::boxed::Box::leak(std::vec![0u8; 2048].into_boxed_slice());
        let mut r = RamDisk { data: d, bs: 512 };
        r.write(1, &[7u8; 512]).unwrap();
        let mut b = [0u8; 512];
        r.read(1, &mut b).unwrap();
        assert_eq!(b, [7u8; 512]);
        r.read(0, &mut b).unwrap();
        assert_eq!(b, [0u8; 512]);
        assert_eq!(r.write(4, &[0u8; 512]), Err(BlockError::OutOfRange));
    }
}
