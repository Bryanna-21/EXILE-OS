//! Process lifecycle metadata only; no scheduler, address-space switching or executable launch.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Created,
    Ready,
    Running,
    Blocked,
    Exited,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId {
    slot: u16,
    generation: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    Capacity,
    InvalidId,
    InvalidTransition,
    AlreadyExited,
}
#[derive(Debug, Clone, Copy)]
struct Process {
    generation: u32,
    parent: Option<ProcessId>,
    state: ProcessState,
    exit_code: Option<i32>,
}
pub struct ProcessTable<const N: usize> {
    items: [Option<Process>; N],
    generations: [u32; N],
}
impl<const N: usize> ProcessTable<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            generations: [1; N],
        }
    }
    fn get(&self, id: ProcessId) -> Result<Process, ProcessError> {
        let p = self
            .items
            .get(id.slot as usize)
            .and_then(|x| *x)
            .ok_or(ProcessError::InvalidId)?;
        if p.generation != id.generation {
            return Err(ProcessError::InvalidId);
        }
        Ok(p)
    }
    pub fn create(&mut self, parent: Option<ProcessId>) -> Result<ProcessId, ProcessError> {
        if let Some(p) = parent {
            self.get(p)?;
        }
        let i = self
            .items
            .iter()
            .position(Option::is_none)
            .ok_or(ProcessError::Capacity)?;
        let slot = u16::try_from(i).map_err(|_| ProcessError::Capacity)?;
        let g = self.generations[i];
        self.items[i] = Some(Process {
            generation: g,
            parent,
            state: ProcessState::Created,
            exit_code: None,
        });
        Ok(ProcessId {
            slot,
            generation: g,
        })
    }
    pub fn state(&self, id: ProcessId) -> Result<ProcessState, ProcessError> {
        Ok(self.get(id)?.state)
    }
    pub fn parent(&self, id: ProcessId) -> Result<Option<ProcessId>, ProcessError> {
        Ok(self.get(id)?.parent)
    }
    pub fn transition(&mut self, id: ProcessId, next: ProcessState) -> Result<(), ProcessError> {
        let p = self
            .items
            .get_mut(id.slot as usize)
            .and_then(Option::as_mut)
            .ok_or(ProcessError::InvalidId)?;
        if p.generation != id.generation {
            return Err(ProcessError::InvalidId);
        }
        let ok = matches!(
            (p.state, next),
            (ProcessState::Created, ProcessState::Ready)
                | (ProcessState::Ready, ProcessState::Running)
                | (ProcessState::Running, ProcessState::Ready)
                | (ProcessState::Running, ProcessState::Blocked)
                | (ProcessState::Blocked, ProcessState::Ready)
                | (ProcessState::Created, ProcessState::Exited)
                | (ProcessState::Ready, ProcessState::Exited)
                | (ProcessState::Running, ProcessState::Exited)
                | (ProcessState::Blocked, ProcessState::Exited)
        );
        if !ok {
            return Err(if p.state == ProcessState::Exited {
                ProcessError::AlreadyExited
            } else {
                ProcessError::InvalidTransition
            });
        }
        p.state = next;
        Ok(())
    }
    pub fn exit(&mut self, id: ProcessId, code: i32) -> Result<(), ProcessError> {
        self.transition(id, ProcessState::Exited)?;
        let p = self
            .items
            .get_mut(id.slot as usize)
            .and_then(Option::as_mut)
            .ok_or(ProcessError::InvalidId)?;
        if p.generation != id.generation {
            return Err(ProcessError::InvalidId);
        }
        p.exit_code = Some(code);
        Ok(())
    }
    pub fn reap(&mut self, id: ProcessId) -> Result<i32, ProcessError> {
        let p = self.get(id)?;
        if p.state != ProcessState::Exited {
            return Err(ProcessError::InvalidTransition);
        }
        let code = p.exit_code.ok_or(ProcessError::InvalidTransition)?;
        self.items[id.slot as usize] = None;
        self.generations[id.slot as usize] =
            self.generations[id.slot as usize].wrapping_add(1).max(1);
        Ok(code)
    }
}
impl<const N: usize> Default for ProcessTable<N> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_lifecycle() {
        let mut t = ProcessTable::<2>::new();
        let p = t.create(None).unwrap();
        t.transition(p, ProcessState::Ready).unwrap();
        t.transition(p, ProcessState::Running).unwrap();
        t.exit(p, 9).unwrap();
        assert_eq!(t.reap(p), Ok(9));
        assert_eq!(t.state(p), Err(ProcessError::InvalidId));
        let q = t.create(None).unwrap();
        assert_ne!(p.generation, q.generation);
        assert_eq!(t.state(p), Err(ProcessError::InvalidId));
    }
    #[test]
    fn bad_transition_parent() {
        let mut t = ProcessTable::<2>::new();
        let p = t.create(None).unwrap();
        assert_eq!(
            t.transition(p, ProcessState::Running),
            Err(ProcessError::InvalidTransition)
        );
        let c = t.create(Some(p)).unwrap();
        assert_eq!(t.parent(c), Ok(Some(p)));
    }
}
