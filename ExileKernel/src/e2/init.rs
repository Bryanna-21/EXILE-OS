//! Dependency-ordered initialization runner. It invokes only caller-supplied callbacks.
use super::init_graph::{GraphError, InitGraph, NodeId};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitState {
    Pending,
    Ready,
    Running,
    Completed,
    Failed,
    Blocked,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitError {
    Graph(GraphError),
    Callback { node: NodeId, code: u16 },
    AlreadyRun,
    Corrupt,
}
impl From<GraphError> for InitError {
    fn from(e: GraphError) -> Self {
        Self::Graph(e)
    }
}
pub trait InitExecutor {
    fn initialize(&mut self, node: NodeId, name: &'static str) -> Result<(), u16>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitReport {
    pub completed: usize,
    pub failed: usize,
    pub blocked: usize,
    pub first_failure: Option<(NodeId, u16)>,
}
pub struct InitRunner<const N: usize> {
    states: [InitState; N],
    ran: bool,
}
impl<const N: usize> InitRunner<N> {
    pub const fn new() -> Self {
        Self {
            states: [InitState::Pending; N],
            ran: false,
        }
    }
    pub fn state(&self, id: NodeId) -> Option<InitState> {
        self.states.get(id.index()).copied()
    }
    pub fn run<E: InitExecutor>(
        &mut self,
        g: &InitGraph<N>,
        exec: &mut E,
    ) -> Result<InitReport, InitError> {
        if self.ran {
            return Err(InitError::AlreadyRun);
        }
        let mut order = [NodeId::from_index(0); N];
        let count = g.order(&mut order)?;
        let mut report = InitReport {
            completed: 0,
            failed: 0,
            blocked: 0,
            first_failure: None,
        };
        for id in order[..count].iter().copied() {
            let i = id.index();
            let deps = g.dependencies(id)?;
            let mut blocked = false;
            for prior in 0..N.min(64) {
                if deps & (1u64 << prior) != 0
                    && (self.states[prior] == InitState::Failed
                        || self.states[prior] == InitState::Blocked)
                {
                    blocked = true;
                    break;
                }
            }
            if blocked {
                self.states[i] = InitState::Blocked;
                report.blocked += 1;
                continue;
            }
            self.states[i] = InitState::Ready;
            self.states[i] = InitState::Running;
            match exec.initialize(id, g.name(id)?) {
                Ok(()) => {
                    self.states[i] = InitState::Completed;
                    report.completed += 1
                }
                Err(code) => {
                    self.states[i] = InitState::Failed;
                    report.failed += 1;
                    if report.first_failure.is_none() {
                        report.first_failure = Some((id, code))
                    }
                }
            }
        }
        self.ran = true;
        Ok(report)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct X {
        fail: u8,
        calls: usize,
    }
    impl InitExecutor for X {
        fn initialize(&mut self, n: NodeId, _: &'static str) -> Result<(), u16> {
            self.calls += 1;
            if n.index() == self.fail as usize {
                Err(7)
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn failure_blocks_dependent_but_not_independent() {
        let mut g = InitGraph::<4>::new();
        let a = g.add("a", &[]).unwrap();
        let b = g.add("b", &[a]).unwrap();
        let _c = g.add("c", &[]).unwrap();
        let mut r = InitRunner::<4>::new();
        let mut x = X { fail: 0, calls: 0 };
        let report = r.run(&g, &mut x).unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.blocked, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(r.state(b), Some(InitState::Blocked));
        assert_eq!(r.run(&g, &mut x), Err(InitError::AlreadyRun));
    }
}
