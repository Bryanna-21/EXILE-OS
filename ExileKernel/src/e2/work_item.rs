//! Small, copyable deferred-work identity and state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkId(u64);
impl WorkId {
    pub const fn new(v: u64) -> Option<Self> {
        if v == 0 { None } else { Some(Self(v)) }
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkState {
    Pending,
    Running,
    Completed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkError {
    InvalidTransition,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkItem {
    pub id: WorkId,
    pub class: u8,
    state: WorkState,
}
impl WorkItem {
    pub fn new(id: WorkId, class: u8) -> Self {
        Self {
            id,
            class,
            state: WorkState::Pending,
        }
    }
    pub const fn state(&self) -> WorkState {
        self.state
    }
    pub fn start(&mut self) -> Result<(), WorkError> {
        if self.state != WorkState::Pending {
            return Err(WorkError::InvalidTransition);
        }
        self.state = WorkState::Running;
        Ok(())
    }
    pub fn complete(&mut self) -> Result<(), WorkError> {
        if self.state != WorkState::Running {
            return Err(WorkError::InvalidTransition);
        }
        self.state = WorkState::Completed;
        Ok(())
    }
    pub fn cancel(&mut self) -> Result<(), WorkError> {
        if self.state != WorkState::Pending {
            return Err(WorkError::InvalidTransition);
        }
        self.state = WorkState::Cancelled;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transitions_are_one_way() {
        let id = WorkId::new(7).unwrap();
        let mut w = WorkItem::new(id, 1);
        assert_eq!(w.complete(), Err(WorkError::InvalidTransition));
        w.start().unwrap();
        assert_eq!(w.cancel(), Err(WorkError::InvalidTransition));
        w.complete().unwrap();
        assert_eq!(w.start(), Err(WorkError::InvalidTransition));
    }
    #[test]
    fn zero_id_rejected() {
        assert_eq!(WorkId::new(0), None);
    }
}
