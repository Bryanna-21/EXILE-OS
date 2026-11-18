//! PCI/PCIe configuration-space enumeration over an abstract `ConfigAccess`.
//!
//! Status: DISCOVERY only. Tested against a mock topology; never against real hardware.

/// Config-space accessor (port I/O CF8/CFC or ECAM implement this in arch code).
pub trait ConfigAccess {
    /// Read a dword; absent functions must return 0xFFFF_FFFF.
    fn read32(&mut self, bus: u8, dev: u8, func: u8, off: u16) -> u32;
    /// Write a dword.
    fn write32(&mut self, bus: u8, dev: u8, func: u8, off: u16, val: u32);
}

/// Coarse device classification from class/subclass/prog-if.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// NVMe controller (01:08:02).
    Nvme,
    /// AHCI SATA controller (01:06:01).
    Ahci,
    /// xHCI USB controller (0C:03:30).
    Xhci,
    /// Ethernet controller (02:00).
    Ethernet,
    /// Display controller (03).
    Display,
    /// PCI-PCI bridge (06:04).
    Bridge,
    /// Anything else.
    Other,
}

/// Classify by class code.
pub fn classify(class: u8, sub: u8, prog_if: u8) -> DeviceKind {
    match (class, sub, prog_if) {
        (1, 8, 2) => DeviceKind::Nvme,
        (1, 6, 1) => DeviceKind::Ahci,
        (0x0C, 3, 0x30) => DeviceKind::Xhci,
        (2, 0, _) => DeviceKind::Ethernet,
        (3, _, _) => DeviceKind::Display,
        (6, 4, _) => DeviceKind::Bridge,
        _ => DeviceKind::Other,
    }
}

/// A discovered function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDevice {
    /// Bus.
    pub bus: u8,
    /// Device.
    pub dev: u8,
    /// Function.
    pub func: u8,
    /// Vendor id.
    pub vendor: u16,
    /// Device id.
    pub device: u16,
    /// Classification.
    pub kind: DeviceKind,
    /// Header type (low 7 bits).
    pub header: u8,
}

/// A decoded BAR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    /// Base address.
    pub base: u64,
    /// Size in bytes (0 = unimplemented).
    pub size: u64,
    /// True for I/O-port BAR.
    pub io: bool,
    /// True for 64-bit memory BAR.
    pub wide: bool,
}

/// Enumeration errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciError {
    /// Output capacity exceeded.
    Full,
    /// Bus recursion depth exceeded (loop in bridge topology).
    TooDeep,
}

/// Enumerate every function reachable from bus 0, following bridges. A visited bitmap prevents
/// loops from malformed secondary-bus numbers.
pub fn enumerate<C: ConfigAccess, const N: usize>(
    cfg: &mut C,
) -> Result<([Option<PciDevice>; N], usize), PciError> {
    let mut st = Scan {
        visited: [false; 256],
        out: [None; N],
        n: 0,
    };
    scan_bus(cfg, 0, &mut st, 0)?;
    Ok((st.out, st.n))
}

struct Scan<const N: usize> {
    visited: [bool; 256],
    out: [Option<PciDevice>; N],
    n: usize,
}

fn scan_bus<C: ConfigAccess, const N: usize>(
    cfg: &mut C,
    bus: u8,
    st: &mut Scan<N>,
    depth: u8,
) -> Result<(), PciError> {
    if depth > 8 {
        return Err(PciError::TooDeep);
    }
    if st.visited[bus as usize] {
        return Ok(());
    }
    st.visited[bus as usize] = true;
    for dev in 0..32u8 {
        if cfg.read32(bus, dev, 0, 0) & 0xFFFF == 0xFFFF {
            continue;
        }
        let multi = (cfg.read32(bus, dev, 0, 0x0C) >> 16) & 0x80 != 0;
        for func in 0..(if multi { 8 } else { 1 }) {
            let id = cfg.read32(bus, dev, func, 0);
            if id & 0xFFFF == 0xFFFF {
                continue;
            }
            let cls = cfg.read32(bus, dev, func, 8);
            let hdr = ((cfg.read32(bus, dev, func, 0x0C) >> 16) & 0x7F) as u8;
            let kind = classify((cls >> 24) as u8, (cls >> 16) as u8, (cls >> 8) as u8);
            if st.n >= N {
                return Err(PciError::Full);
            }
            st.out[st.n] = Some(PciDevice {
                bus,
                dev,
                func,
                vendor: id as u16,
                device: (id >> 16) as u16,
                kind,
                header: hdr,
            });
            st.n += 1;
            if hdr == 1 && kind == DeviceKind::Bridge {
                let sec = ((cfg.read32(bus, dev, func, 0x18) >> 8) & 0xFF) as u8;
                if sec != 0 {
                    scan_bus(cfg, sec, st, depth + 1)?;
                }
            }
        }
    }
    Ok(())
}

/// Probe BAR `idx` (0..6) of a type-0 function. Memory/IO decode is disabled during the
/// size probe and the original command register and BAR values are restored.
pub fn probe_bar<C: ConfigAccess>(cfg: &mut C, d: &PciDevice, idx: u8) -> Option<Bar> {
    if d.header != 0 || idx > 5 {
        return None;
    }
    let off = 0x10 + u16::from(idx) * 4;
    let cmd = cfg.read32(d.bus, d.dev, d.func, 4);
    cfg.write32(d.bus, d.dev, d.func, 4, cmd & !0b11);
    let orig = cfg.read32(d.bus, d.dev, d.func, off);
    let io = orig & 1 == 1;
    let wide = !io && (orig >> 1) & 3 == 2;
    let mut orig_hi = 0;
    let result = (|| {
        cfg.write32(d.bus, d.dev, d.func, off, 0xFFFF_FFFF);
        let lo = cfg.read32(d.bus, d.dev, d.func, off);
        let mask_lo = if io { lo & !3 } else { lo & !0xF };
        if mask_lo == 0 {
            return Some(Bar {
                base: 0,
                size: 0,
                io,
                wide,
            });
        }
        if wide {
            if idx == 5 {
                return None;
            }
            orig_hi = cfg.read32(d.bus, d.dev, d.func, off + 4);
            cfg.write32(d.bus, d.dev, d.func, off + 4, 0xFFFF_FFFF);
            let hi = cfg.read32(d.bus, d.dev, d.func, off + 4);
            let mask = (u64::from(hi) << 32) | u64::from(mask_lo);
            let base = (u64::from(orig_hi) << 32) | u64::from(orig & !0xF);
            return Some(Bar {
                base,
                size: (!mask).wrapping_add(1),
                io,
                wide,
            });
        }
        let size = u64::from((!mask_lo).wrapping_add(1));
        let base = u64::from(if io { orig & !3 } else { orig & !0xF });
        Some(Bar {
            base,
            size,
            io,
            wide,
        })
    })();
    cfg.write32(d.bus, d.dev, d.func, off, orig);
    if wide && idx < 5 {
        cfg.write32(d.bus, d.dev, d.func, off + 4, orig_hi);
    }
    cfg.write32(d.bus, d.dev, d.func, 4, cmd);
    result
}

/// Find a capability id in the legacy capability list. Bounded to 48 hops.
pub fn find_capability<C: ConfigAccess>(cfg: &mut C, d: &PciDevice, id: u8) -> Option<u8> {
    if cfg.read32(d.bus, d.dev, d.func, 4) >> 16 & 0x10 == 0 {
        return None;
    }
    let mut p = (cfg.read32(d.bus, d.dev, d.func, 0x34) & 0xFC) as u8;
    for _ in 0..48 {
        if p < 0x40 {
            return None;
        }
        let w = cfg.read32(d.bus, d.dev, d.func, u16::from(p));
        if (w & 0xFF) as u8 == id {
            return Some(p);
        }
        p = ((w >> 8) & 0xFC) as u8;
    }
    None
}

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use std::collections::BTreeMap;
    /// Mock config space with BAR size masks and write-log.
    pub struct Mock {
        pub regs: BTreeMap<(u8, u8, u8, u16), u32>,
        pub bar_mask: BTreeMap<(u8, u8, u8, u16), u32>,
        pub writes: std::vec::Vec<(u8, u8, u8, u16, u32)>,
    }
    impl Mock {
        pub fn new() -> Self {
            Mock {
                regs: BTreeMap::new(),
                bar_mask: BTreeMap::new(),
                writes: std::vec::Vec::new(),
            }
        }
        pub fn set(&mut self, b: u8, d: u8, f: u8, o: u16, v: u32) {
            self.regs.insert((b, d, f, o), v);
        }
    }
    impl ConfigAccess for Mock {
        fn read32(&mut self, b: u8, d: u8, f: u8, o: u16) -> u32 {
            *self.regs.get(&(b, d, f, o)).unwrap_or(&0xFFFF_FFFF)
        }
        fn write32(&mut self, b: u8, d: u8, f: u8, o: u16, v: u32) {
            self.writes.push((b, d, f, o, v));
            if let Some(m) = self.bar_mask.get(&(b, d, f, o)).copied()
                && v == 0xFFFF_FFFF
            {
                let keep = self.regs.get(&(b, d, f, o)).copied().unwrap_or(0) & 0xF;
                self.regs.insert((b, d, f, o), m | keep);
                return;
            }
            self.regs.insert((b, d, f, o), v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::Mock;
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn dev(m: &mut Mock, b: u8, d: u8, f: u8, ven: u16, did: u16, class: u32, hdr: u32) {
        m.set(b, d, f, 0, u32::from(did) << 16 | u32::from(ven));
        m.set(b, d, f, 8, class << 8);
        m.set(b, d, f, 0x0C, hdr << 16);
        m.set(b, d, f, 4, 0x0007 | 0x0010_0000);
    }
    #[test]
    fn classify_known_classes() {
        assert_eq!(classify(1, 8, 2), DeviceKind::Nvme);
        assert_eq!(classify(1, 6, 1), DeviceKind::Ahci);
        assert_eq!(classify(0xC, 3, 0x30), DeviceKind::Xhci);
        assert_eq!(
            classify(1, 8, 1),
            DeviceKind::Other,
            "NVM Express prog-if must match"
        );
    }
    #[test]
    fn enumerates_through_bridge_and_multifunction() {
        let mut m = Mock::new();
        dev(&mut m, 0, 0, 0, 0x8086, 0x1111, 0x060000, 0);
        dev(&mut m, 0, 1, 0, 0x8086, 0x2222, 0x060400, 1);
        m.set(0, 1, 0, 0x18, 1 << 8); // secondary bus 1
        dev(&mut m, 1, 0, 0, 0x144D, 0xA808, 0x010802, 0x80);
        dev(&mut m, 1, 0, 3, 0x144D, 0xA809, 0x020000, 0);
        let (o, n) = enumerate::<_, 16>(&mut m).unwrap();
        assert_eq!(n, 4);
        assert_eq!(o[2].unwrap().kind, DeviceKind::Nvme);
        assert_eq!(o[3].unwrap().func, 3);
    }
    #[test]
    fn bridge_loop_and_capacity_are_bounded() {
        let mut m = Mock::new();
        dev(&mut m, 0, 1, 0, 1, 1, 0x060400, 1);
        m.set(0, 1, 0, 0x18, 0 << 8);
        let mut m2 = Mock::new();
        dev(&mut m2, 0, 1, 0, 1, 1, 0x060400, 1);
        m2.set(0, 1, 0, 0x18, 1 << 8);
        dev(&mut m2, 1, 0, 0, 1, 2, 0x060400, 1);
        m2.set(1, 0, 0, 0x18, 1 << 8); // points back at itself
        assert_eq!(enumerate::<_, 8>(&mut m2).unwrap().1, 2);
        assert_eq!(enumerate::<_, 1>(&mut m2).unwrap_err(), PciError::Full);
        assert_eq!(enumerate::<_, 8>(&mut m).unwrap().1, 1);
    }
    #[test]
    fn bar_probe_sizes_and_restores_state() {
        let mut m = Mock::new();
        dev(&mut m, 0, 2, 0, 1, 1, 0x010802, 0);
        m.set(0, 2, 0, 0x10, 0xFE00_0004); // 64-bit mem BAR
        m.set(0, 2, 0, 0x14, 0x0000_0001);
        m.bar_mask.insert((0, 2, 0, 0x10), 0xFFFF_C000); // 16 KiB
        m.bar_mask.insert((0, 2, 0, 0x14), 0xFFFF_FFFF);
        m.set(0, 2, 0, 0x18, 0xC001); // IO BAR
        m.bar_mask.insert((0, 2, 0, 0x18), 0xFFFF_FF00); // 256 bytes
        m.set(0, 2, 0, 0x1C, 0); // unimplemented BAR reads back zero
        m.bar_mask.insert((0, 2, 0, 0x1C), 0);
        let d = enumerate::<_, 4>(&mut m).unwrap().0[0].unwrap();
        let before = (
            m.read32(0, 2, 0, 4),
            m.read32(0, 2, 0, 0x10),
            m.read32(0, 2, 0, 0x14),
        );
        let b = probe_bar(&mut m, &d, 0).unwrap();
        assert!(b.wide && !b.io);
        assert_eq!(b.size, 0x4000);
        assert_eq!(b.base, 0x1_FE00_0000);
        assert_eq!(
            before,
            (
                m.read32(0, 2, 0, 4),
                m.read32(0, 2, 0, 0x10),
                m.read32(0, 2, 0, 0x14)
            ),
            "BAR/command not restored"
        );
        let io = probe_bar(&mut m, &d, 2).unwrap();
        assert!(io.io);
        assert_eq!((io.size, io.base), (0x100, 0xC000));
        let none = probe_bar(&mut m, &d, 3).unwrap();
        assert_eq!(none.size, 0);
    }
    #[test]
    fn decode_is_disabled_during_probe() {
        let mut m = Mock::new();
        dev(&mut m, 0, 2, 0, 1, 1, 0x010802, 0);
        m.set(0, 2, 0, 0x10, 0xFE00_0000);
        m.bar_mask.insert((0, 2, 0, 0x10), 0xFFFF_0000);
        let d = enumerate::<_, 2>(&mut m).unwrap().0[0].unwrap();
        probe_bar(&mut m, &d, 0).unwrap();
        let w = &m.writes;
        let first_cmd = w.iter().position(|x| x.3 == 4).unwrap();
        let first_bar = w.iter().position(|x| x.3 == 0x10).unwrap();
        assert!(
            first_cmd < first_bar && w[first_cmd].4 & 3 == 0,
            "decode must be off before sizing"
        );
        assert_eq!(w.last().unwrap().3, 4, "command register restored last");
    }
    #[test]
    fn capability_walk_is_bounded() {
        let mut m = Mock::new();
        dev(&mut m, 0, 3, 0, 1, 1, 0x010802, 0);
        m.set(0, 3, 0, 0x34, 0x40);
        m.set(0, 3, 0, 0x40, 0x0000_5001); // id 1 -> next 0x50
        m.set(0, 3, 0, 0x50, 0x0000_0011); // id 0x11 (MSI-X), end of list
        let d = enumerate::<_, 2>(&mut m).unwrap().0[0].unwrap();
        assert_eq!(find_capability(&mut m, &d, 0x11), Some(0x50));
        assert_eq!(find_capability(&mut m, &d, 0x05), None);
        m.set(0, 3, 0, 0x50, 0x0000_4001); // loops back to 0x40
        assert_eq!(
            find_capability(&mut m, &d, 0x99),
            None,
            "cycle must terminate"
        );
    }
}
