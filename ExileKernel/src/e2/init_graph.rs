//! Fixed-size dependency DAG with deterministic lowest-index topological order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(u8);
impl NodeId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
    pub(crate) const fn from_index(i: usize) -> Self {
        Self(i as u8)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphError {
    Capacity,
    InvalidName,
    Duplicate,
    MissingDependency,
    SelfDependency,
    Cycle,
    OutputTooSmall,
    InvalidNode,
}
#[derive(Clone, Copy)]
struct Node {
    name: &'static str,
    deps: u64,
}
pub struct InitGraph<const N: usize> {
    nodes: [Option<Node>; N],
    len: usize,
}
impl<const N: usize> InitGraph<N> {
    pub const fn new() -> Self {
        Self {
            nodes: [None; N],
            len: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub fn add_node(&mut self, name: &'static str) -> Result<NodeId, GraphError> {
        if name.is_empty() {
            return Err(GraphError::InvalidName);
        }
        if self.len == N || self.len >= 64 {
            return Err(GraphError::Capacity);
        }
        if self.nodes.iter().flatten().any(|n| n.name == name) {
            return Err(GraphError::Duplicate);
        }
        let id = NodeId(self.len as u8);
        self.nodes[self.len] = Some(Node { name, deps: 0 });
        self.len += 1;
        Ok(id)
    }
    pub fn add_dependency(&mut self, node: NodeId, dependency: NodeId) -> Result<(), GraphError> {
        if node.index() >= self.len || self.nodes[node.index()].is_none() {
            return Err(GraphError::InvalidNode);
        }
        if dependency.index() >= self.len || self.nodes[dependency.index()].is_none() {
            return Err(GraphError::MissingDependency);
        }
        if node == dependency {
            return Err(GraphError::SelfDependency);
        }
        let bit = 1u64 << dependency.index();
        let n = self.nodes[node.index()]
            .as_mut()
            .ok_or(GraphError::InvalidNode)?;
        if n.deps & bit != 0 {
            return Err(GraphError::Duplicate);
        }
        n.deps |= bit;
        Ok(())
    }
    pub fn add(&mut self, name: &'static str, deps: &[NodeId]) -> Result<NodeId, GraphError> {
        let id = self.add_node(name)?;
        for d in deps {
            if let Err(e) = self.add_dependency(id, *d) {
                self.nodes[id.index()] = None;
                self.len -= 1;
                return Err(e);
            }
        }
        Ok(id)
    }
    pub fn name(&self, id: NodeId) -> Result<&'static str, GraphError> {
        self.nodes
            .get(id.index())
            .and_then(|x| *x)
            .map(|n| n.name)
            .ok_or(GraphError::InvalidNode)
    }
    pub fn dependencies(&self, id: NodeId) -> Result<u64, GraphError> {
        self.nodes
            .get(id.index())
            .and_then(|x| *x)
            .map(|n| n.deps)
            .ok_or(GraphError::InvalidNode)
    }
    pub fn order(&self, out: &mut [NodeId]) -> Result<usize, GraphError> {
        if out.len() < self.len {
            return Err(GraphError::OutputTooSmall);
        }
        let mut done = 0u64;
        let mut count = 0;
        while count < self.len {
            let mut chosen = None;
            for i in 0..self.len {
                let bit = 1u64 << i;
                if done & bit == 0 && self.nodes[i].is_some_and(|n| n.deps & !done == 0) {
                    chosen = Some(i);
                    break;
                }
            }
            let i = chosen.ok_or(GraphError::Cycle)?;
            done |= 1u64 << i;
            out[count] = NodeId(i as u8);
            count += 1
        }
        Ok(count)
    }
    pub fn check_invariants(&self) -> bool {
        self.len <= N
            && self.len <= 64
            && self.nodes[..self.len].iter().all(Option::is_some)
            && self.nodes[self.len..].iter().all(Option::is_none)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_order_and_cycle() {
        let mut g = InitGraph::<4>::new();
        let a = g.add("a", &[]).unwrap();
        let b = g.add("b", &[a]).unwrap();
        let c = g.add("c", &[a]).unwrap();
        let mut o = [a; 4];
        assert_eq!(g.order(&mut o), Ok(3));
        assert_eq!(&o[..3], &[a, b, c]);
        assert_eq!(g.add("a", &[]), Err(GraphError::Duplicate));
        let mut cyclic = InitGraph::<2>::new();
        let x = cyclic.add_node("x").unwrap();
        let y = cyclic.add_node("y").unwrap();
        cyclic.add_dependency(x, y).unwrap();
        cyclic.add_dependency(y, x).unwrap();
        assert_eq!(cyclic.order(&mut [x; 2]), Err(GraphError::Cycle));
    }
    #[test]
    fn missing_dependency_and_capacity() {
        let mut g = InitGraph::<1>::new();
        assert_eq!(g.add("x", &[NodeId(1)]), Err(GraphError::MissingDependency));
        g.add("x", &[]).unwrap();
        assert_eq!(g.add("y", &[]), Err(GraphError::Capacity));
    }
}
