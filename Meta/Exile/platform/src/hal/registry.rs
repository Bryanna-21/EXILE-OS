//! Device registry and driver lifecycle state machine.
//!
//! Lifecycle: Discovered -> Bound -> Started -> Stopped; any step may go to Failed.
//! A driver reporting success only means its `probe`/`start` returned Ok; whether the hardware works
//! is a property of the driver and device, not of this registry.

use super::pci::{DeviceKind, PciDevice};

/// Lifecycle state of one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Found by enumeration, no driver yet.
    Discovered,
    /// Driver chosen and probe succeeded.
    Bound,
    /// Driver started.
    Started,
    /// Driver stopped cleanly.
    Stopped,
    /// A lifecycle step failed.
    Failed,
}

/// Driver interface.
pub trait Driver {
    /// Which kind this driver handles.
    fn kind(&self) -> DeviceKind;
    /// Validate the device and claim resources.
    fn probe(&mut self, d: &PciDevice) -> Result<(), DriverError>;
    /// Bring the device to operational state.
    fn start(&mut self, d: &PciDevice) -> Result<(), DriverError>;
    /// Quiesce the device.
    fn stop(&mut self, d: &PciDevice);
}

/// Driver failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    /// Device not supported.
    Unsupported,
    /// Hardware did not respond / timed out.
    Timeout,
    /// Other failure.
    Failed,
}

/// Registry errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    /// No room.
    Full,
    /// Index out of range.
    NoSuchDevice,
    /// Operation illegal in the device's current state.
    BadState(State),
    /// No driver for this kind.
    NoDriver,
    /// Driver returned an error.
    Driver(DriverError),
}

/// A registered device.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// Device.
    pub dev: PciDevice,
    /// State.
    pub state: State,
}

/// Fixed-capacity registry.
pub struct Registry<const N: usize> {
    entries: [Option<Entry>; N],
    len: usize,
}

impl<const N: usize> Default for Registry<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Registry<N> {
    /// Empty.
    pub const fn new() -> Self {
        Registry {
            entries: [None; N],
            len: 0,
        }
    }
    /// Register a discovered device; returns its index.
    pub fn add(&mut self, dev: PciDevice) -> Result<usize, RegistryError> {
        if self.len == N {
            return Err(RegistryError::Full);
        }
        self.entries[self.len] = Some(Entry {
            dev,
            state: State::Discovered,
        });
        self.len += 1;
        Ok(self.len - 1)
    }
    /// Count.
    pub fn len(&self) -> usize {
        self.len
    }
    /// True if empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Entry accessor.
    pub fn get(&self, i: usize) -> Option<&Entry> {
        self.entries.get(i)?.as_ref()
    }
    fn entry(&mut self, i: usize) -> Result<&mut Entry, RegistryError> {
        self.entries
            .get_mut(i)
            .and_then(|e| e.as_mut())
            .ok_or(RegistryError::NoSuchDevice)
    }
    /// Probe + bind. Requires Discovered.
    pub fn bind(&mut self, i: usize, drv: &mut dyn Driver) -> Result<(), RegistryError> {
        let e = self.entry(i)?;
        if e.state != State::Discovered {
            return Err(RegistryError::BadState(e.state));
        }
        if drv.kind() != e.dev.kind {
            return Err(RegistryError::NoDriver);
        }
        match drv.probe(&e.dev) {
            Ok(()) => {
                e.state = State::Bound;
                Ok(())
            }
            Err(x) => {
                e.state = State::Failed;
                Err(RegistryError::Driver(x))
            }
        }
    }
    /// Start. Requires Bound.
    pub fn start(&mut self, i: usize, drv: &mut dyn Driver) -> Result<(), RegistryError> {
        let e = self.entry(i)?;
        if e.state != State::Bound {
            return Err(RegistryError::BadState(e.state));
        }
        match drv.start(&e.dev) {
            Ok(()) => {
                e.state = State::Started;
                Ok(())
            }
            Err(x) => {
                e.state = State::Failed;
                Err(RegistryError::Driver(x))
            }
        }
    }
    /// Stop. Requires Started.
    pub fn stop(&mut self, i: usize, drv: &mut dyn Driver) -> Result<(), RegistryError> {
        let e = self.entry(i)?;
        if e.state != State::Started {
            return Err(RegistryError::BadState(e.state));
        }
        drv.stop(&e.dev);
        e.state = State::Stopped;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct D {
        kind: DeviceKind,
        fail_probe: bool,
        fail_start: bool,
        log: std::vec::Vec<&'static str>,
    }
    impl Driver for D {
        fn kind(&self) -> DeviceKind {
            self.kind
        }
        fn probe(&mut self, _: &PciDevice) -> Result<(), DriverError> {
            self.log.push("probe");
            if self.fail_probe {
                Err(DriverError::Unsupported)
            } else {
                Ok(())
            }
        }
        fn start(&mut self, _: &PciDevice) -> Result<(), DriverError> {
            self.log.push("start");
            if self.fail_start {
                Err(DriverError::Timeout)
            } else {
                Ok(())
            }
        }
        fn stop(&mut self, _: &PciDevice) {
            self.log.push("stop");
        }
    }
    fn dev(kind: DeviceKind) -> PciDevice {
        PciDevice {
            bus: 0,
            dev: 1,
            func: 0,
            vendor: 1,
            device: 2,
            kind,
            header: 0,
        }
    }
    fn drv(kind: DeviceKind) -> D {
        D {
            kind,
            fail_probe: false,
            fail_start: false,
            log: std::vec::Vec::new(),
        }
    }

    #[test]
    fn full_lifecycle_in_order() {
        let mut r = Registry::<4>::new();
        let i = r.add(dev(DeviceKind::Nvme)).unwrap();
        let mut d = drv(DeviceKind::Nvme);
        r.bind(i, &mut d).unwrap();
        r.start(i, &mut d).unwrap();
        r.stop(i, &mut d).unwrap();
        assert_eq!(r.get(i).unwrap().state, State::Stopped);
        assert_eq!(d.log, ["probe", "start", "stop"]);
    }
    #[test]
    fn illegal_transitions_rejected_without_calling_driver() {
        let mut r = Registry::<4>::new();
        let i = r.add(dev(DeviceKind::Nvme)).unwrap();
        let mut d = drv(DeviceKind::Nvme);
        assert_eq!(
            r.start(i, &mut d),
            Err(RegistryError::BadState(State::Discovered))
        );
        assert_eq!(
            r.stop(i, &mut d),
            Err(RegistryError::BadState(State::Discovered))
        );
        r.bind(i, &mut d).unwrap();
        assert_eq!(
            r.bind(i, &mut d),
            Err(RegistryError::BadState(State::Bound))
        );
        assert_eq!(d.log, ["probe"]);
    }
    #[test]
    fn wrong_driver_kind_is_refused_and_device_stays_discovered() {
        let mut r = Registry::<4>::new();
        let i = r.add(dev(DeviceKind::Ahci)).unwrap();
        let mut d = drv(DeviceKind::Nvme);
        assert_eq!(r.bind(i, &mut d), Err(RegistryError::NoDriver));
        assert_eq!(r.get(i).unwrap().state, State::Discovered);
        assert!(d.log.is_empty());
    }
    #[test]
    fn failures_mark_failed_and_are_terminal() {
        let mut r = Registry::<4>::new();
        let i = r.add(dev(DeviceKind::Nvme)).unwrap();
        let mut d = drv(DeviceKind::Nvme);
        d.fail_start = true;
        r.bind(i, &mut d).unwrap();
        assert_eq!(
            r.start(i, &mut d),
            Err(RegistryError::Driver(DriverError::Timeout))
        );
        assert_eq!(r.get(i).unwrap().state, State::Failed);
        assert_eq!(
            r.start(i, &mut d),
            Err(RegistryError::BadState(State::Failed))
        );
        let j = r.add(dev(DeviceKind::Nvme)).unwrap();
        d.fail_probe = true;
        assert_eq!(
            r.bind(j, &mut d),
            Err(RegistryError::Driver(DriverError::Unsupported))
        );
    }
    #[test]
    fn capacity_and_bad_index() {
        let mut r = Registry::<1>::new();
        r.add(dev(DeviceKind::Other)).unwrap();
        assert_eq!(r.add(dev(DeviceKind::Other)), Err(RegistryError::Full));
        let mut d = drv(DeviceKind::Other);
        assert_eq!(r.bind(5, &mut d), Err(RegistryError::NoSuchDevice));
        assert!(r.get(5).is_none());
    }
}
