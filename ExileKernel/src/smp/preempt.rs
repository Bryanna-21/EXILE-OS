//! Nestable per-CPU preemption-disable accounting.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreemptError {
    Overflow,
    Underflow,
}

pub struct PreemptState {
    count: AtomicU32,
    interrupt_depth: AtomicU32,
}
impl PreemptState {
    pub const fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            interrupt_depth: AtomicU32::new(0),
        }
    }
    pub fn count(&self) -> u32 {
        self.count.load(Ordering::Acquire)
    }
    pub fn interrupt_depth(&self) -> u32 {
        self.interrupt_depth.load(Ordering::Acquire)
    }
    pub fn can_schedule(&self) -> bool {
        self.count() == 0 && self.interrupt_depth() == 0
    }
    pub fn disable(&self) -> Result<u32, PreemptError> {
        update(&self.count, true)
    }
    pub fn enable(&self) -> Result<u32, PreemptError> {
        update(&self.count, false)
    }
    pub fn enter_interrupt(&self) -> Result<u32, PreemptError> {
        update(&self.interrupt_depth, true)
    }
    pub fn leave_interrupt(&self) -> Result<u32, PreemptError> {
        update(&self.interrupt_depth, false)
    }
    pub fn guard(&self) -> Result<PreemptGuard<'_>, PreemptError> {
        self.disable()?;
        Ok(PreemptGuard { state: self })
    }
}
impl Default for PreemptState {
    fn default() -> Self {
        Self::new()
    }
}

fn update(counter: &AtomicU32, increment: bool) -> Result<u32, PreemptError> {
    let result = if increment {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1))
            .map(|old| old + 1)
            .map_err(|_| PreemptError::Overflow)
    } else {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_sub(1))
            .map(|old| old - 1)
            .map_err(|_| PreemptError::Underflow)
    };
    result
}

pub struct PreemptGuard<'a> {
    state: &'a PreemptState,
}
impl Drop for PreemptGuard<'_> {
    fn drop(&mut self) {
        let _ = self.state.enable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guard_restores_preempt_count() {
        let state = PreemptState::new();
        {
            let _guard = state.guard().unwrap();
            assert!(!state.can_schedule());
        }
        assert!(state.can_schedule());
    }
    #[test]
    fn interrupt_nesting_blocks_scheduling() {
        let state = PreemptState::new();
        state.enter_interrupt().unwrap();
        assert!(!state.can_schedule());
        state.leave_interrupt().unwrap();
        assert!(state.can_schedule());
    }
}
