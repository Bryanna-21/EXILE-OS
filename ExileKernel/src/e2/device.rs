//! Device metadata and lifecycle state machine; no register access.
use super::device_id::DeviceId;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    System,
    Storage,
    Network,
    Display,
    Input,
    Bus,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Discovered,
    Registered,
    Bound,
    Started,
    Stopping,
    Failed,
    Removed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceError {
    InvalidTransition,
    InvalidDescriptor,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub id: DeviceId,
    pub parent: Option<DeviceId>,
    pub class: DeviceClass,
    pub vendor: u16,
    pub product: u16,
    pub revision: u32,
    pub state: DeviceState,
    pub failure: Option<u16>,
}
impl DeviceDescriptor {
    pub fn validate(&self) -> Result<(), DeviceError> {
        if !self.id.is_valid()
            || self.parent.is_some_and(|p| !p.is_valid() || p == self.id)
            || ((self.state == DeviceState::Failed) != self.failure.is_some())
        {
            return Err(DeviceError::InvalidDescriptor);
        }
        Ok(())
    }
    fn can_transition(&self, to: DeviceState) -> bool {
        matches!((self.state,to),(DeviceState::Discovered,DeviceState::Registered)|(DeviceState::Registered,DeviceState::Bound)|(DeviceState::Bound,DeviceState::Started)|(DeviceState::Started,DeviceState::Stopping)|(DeviceState::Stopping,DeviceState::Bound)|(DeviceState::Bound,DeviceState::Registered)|(DeviceState::Discovered,DeviceState::Failed)|(DeviceState::Registered,DeviceState::Failed)|(DeviceState::Bound,DeviceState::Failed)|(DeviceState::Started,DeviceState::Failed)|(DeviceState::Stopping,DeviceState::Failed)|(_,DeviceState::Removed) if self.state!=DeviceState::Removed)
    }
    pub fn transition(&mut self, to: DeviceState) -> Result<(), DeviceError> {
        if to == DeviceState::Failed || !self.can_transition(to) {
            return Err(DeviceError::InvalidTransition);
        }
        self.state = to;
        self.failure = None;
        Ok(())
    }
    pub fn fail(&mut self, code: u16) -> Result<(), DeviceError> {
        if !self.can_transition(DeviceState::Failed) {
            return Err(DeviceError::InvalidTransition);
        }
        self.state = DeviceState::Failed;
        self.failure = Some(code);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2::device_id::BusKind;
    fn dev() -> DeviceDescriptor {
        DeviceDescriptor {
            id: DeviceId::new(BusKind::Platform, 0, 1, 1).unwrap(),
            parent: None,
            class: DeviceClass::Other,
            vendor: 0,
            product: 0,
            revision: 0,
            state: DeviceState::Discovered,
            failure: None,
        }
    }
    #[test]
    fn lifecycle() {
        let mut d = dev();
        assert_eq!(
            d.transition(DeviceState::Started),
            Err(DeviceError::InvalidTransition)
        );
        d.transition(DeviceState::Registered).unwrap();
        d.transition(DeviceState::Bound).unwrap();
        d.transition(DeviceState::Started).unwrap();
        d.fail(9).unwrap();
        assert_eq!(d.failure, Some(9));
    }
    #[test]
    fn self_parent_rejected() {
        let mut d = dev();
        d.parent = Some(d.id);
        assert_eq!(d.validate(), Err(DeviceError::InvalidDescriptor));
    }
    #[test]
    fn failure_code_must_match_failed_state() {
        let mut d = dev();
        d.failure = Some(7);
        assert_eq!(d.validate(), Err(DeviceError::InvalidDescriptor));
        d.failure = None;
        d.state = DeviceState::Failed;
        assert_eq!(d.validate(), Err(DeviceError::InvalidDescriptor));
    }
}
