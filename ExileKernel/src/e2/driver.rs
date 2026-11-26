//! Driver descriptor and callback contract; descriptors do not touch hardware themselves.
use super::device::DeviceDescriptor;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DriverId(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverState {
    Registered,
    Probing,
    Bound,
    Started,
    Stopped,
    Unbound,
    Failed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    InvalidId,
    InvalidTransition,
    ProbeFailed,
    StartFailed,
    StopFailed,
    UnbindFailed,
}
pub type ProbeFn = fn(&DeviceDescriptor) -> Result<bool, DriverError>;
pub type DeviceFn = fn(&DeviceDescriptor) -> Result<(), DriverError>;
#[derive(Clone, Copy)]
pub struct DriverDescriptor {
    pub id: DriverId,
    pub name: &'static str,
    pub priority: u16,
    pub matches: &'static [super::driver_match::MatchRule],
    pub probe: ProbeFn,
    pub start: DeviceFn,
    pub stop: DeviceFn,
    pub unbind: DeviceFn,
}
impl DriverDescriptor {
    pub fn validate(&self) -> Result<(), DriverError> {
        if self.id.0 == 0 || self.name.is_empty() {
            Err(DriverError::InvalidId)
        } else {
            Ok(())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn probe(_: &DeviceDescriptor) -> Result<bool, DriverError> {
        Ok(true)
    }
    fn op(_: &DeviceDescriptor) -> Result<(), DriverError> {
        Ok(())
    }
    #[test]
    fn descriptor_validation() {
        let d = DriverDescriptor {
            id: DriverId(1),
            name: "mock",
            priority: 1,
            matches: &[],
            probe,
            start: op,
            stop: op,
            unbind: op,
        };
        assert!(d.validate().is_ok());
    }
}

/// Mutable lifecycle state for a registered driver instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverLifecycle {
    state: DriverState,
}
impl DriverLifecycle {
    pub const fn new() -> Self {
        Self {
            state: DriverState::Registered,
        }
    }
    pub const fn state(&self) -> DriverState {
        self.state
    }
    pub fn transition(&mut self, to: DriverState) -> Result<(), DriverError> {
        let valid = matches!(
            (self.state, to),
            (DriverState::Registered, DriverState::Probing)
                | (DriverState::Probing, DriverState::Bound)
                | (DriverState::Probing, DriverState::Failed)
                | (DriverState::Bound, DriverState::Started)
                | (DriverState::Bound, DriverState::Failed)
                | (DriverState::Started, DriverState::Stopped)
                | (DriverState::Started, DriverState::Failed)
                | (DriverState::Stopped, DriverState::Started)
                | (DriverState::Stopped, DriverState::Unbound)
                | (DriverState::Bound, DriverState::Unbound)
                | (DriverState::Stopped, DriverState::Failed)
        );
        if !valid {
            return Err(DriverError::InvalidTransition);
        }
        self.state = to;
        Ok(())
    }
}
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn valid_and_invalid_driver_transitions() {
        let mut s = DriverLifecycle::new();
        assert_eq!(
            s.transition(DriverState::Started),
            Err(DriverError::InvalidTransition)
        );
        s.transition(DriverState::Probing).unwrap();
        s.transition(DriverState::Bound).unwrap();
        s.transition(DriverState::Started).unwrap();
        s.transition(DriverState::Stopped).unwrap();
        assert_eq!(s.state(), DriverState::Stopped);
        s.transition(DriverState::Unbound).unwrap();
        assert_eq!(s.state(), DriverState::Unbound);
    }
}
