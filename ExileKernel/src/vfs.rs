//! Bounded VFS node metadata and path validation; not a disk-backed filesystem.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Directory,
    Regular,
    Symlink,
    Device,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    pub id: u32,
    pub parent: u32,
    pub kind: NodeKind,
    pub size: u64,
    pub mode: u16,
    pub name: [u8; 64],
    pub name_len: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    BadPath,
    TooLong,
    InvalidName,
    Duplicate,
    NotFound,
    NotDirectory,
    NotEmpty,
    Capacity,
    RootMutation,
}
pub const MAX_PATH: usize = 256;
pub fn validate_path(p: &str) -> Result<(), VfsError> {
    if p.is_empty() || !p.starts_with('/') {
        return Err(VfsError::BadPath);
    }
    if p.len() > MAX_PATH {
        return Err(VfsError::TooLong);
    }
    for c in p.split('/').filter(|x| !x.is_empty()) {
        if c == "." || c == ".." || c.len() > 63 || c.as_bytes().contains(&0) {
            return Err(VfsError::InvalidName);
        }
    }
    Ok(())
}
pub struct Vfs<const N: usize> {
    items: [Option<Node>; N],
    len: usize,
    next: u32,
}
impl<const N: usize> Vfs<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
            next: 1,
        }
    }
    pub fn init_root(&mut self) -> Result<u32, VfsError> {
        if self.len != 0 {
            return Err(VfsError::Duplicate);
        }
        if N == 0 {
            return Err(VfsError::Capacity);
        }
        self.items[0] = Some(Node {
            id: 1,
            parent: 1,
            kind: NodeKind::Directory,
            size: 0,
            mode: 0o755,
            name: [0; 64],
            name_len: 0,
        });
        self.len = 1;
        self.next = 2;
        Ok(1)
    }
    pub fn create(
        &mut self,
        parent: u32,
        name: &str,
        kind: NodeKind,
        mode: u16,
    ) -> Result<u32, VfsError> {
        if name.is_empty()
            || name.len() > 63
            || name.contains('/')
            || name == "."
            || name == ".."
            || name.as_bytes().contains(&0)
        {
            return Err(VfsError::InvalidName);
        }
        let p = self
            .items
            .iter()
            .flatten()
            .find(|n| n.id == parent)
            .ok_or(VfsError::NotFound)?;
        if p.kind != NodeKind::Directory {
            return Err(VfsError::NotDirectory);
        }
        if self.items.iter().flatten().any(|n| {
            n.parent == parent
                && n.name_len as usize == name.len()
                && &n.name[..n.name_len as usize] == name.as_bytes()
        }) {
            return Err(VfsError::Duplicate);
        }
        let slot = self
            .items
            .iter()
            .position(Option::is_none)
            .ok_or(VfsError::Capacity)?;
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or(VfsError::Capacity)?;
        let mut stored = [0u8; 64];
        stored[..name.len()].copy_from_slice(name.as_bytes());
        self.items[slot] = Some(Node {
            id,
            parent,
            kind,
            size: 0,
            mode,
            name: stored,
            name_len: name.len() as u8,
        });
        self.len += 1;
        Ok(id)
    }
    pub fn lookup_id(&self, id: u32) -> Result<Node, VfsError> {
        self.items
            .iter()
            .flatten()
            .find(|n| n.id == id)
            .copied()
            .ok_or(VfsError::NotFound)
    }
    pub fn remove(&mut self, id: u32) -> Result<Node, VfsError> {
        if id == 1 {
            return Err(VfsError::RootMutation);
        }
        let i = self
            .items
            .iter()
            .position(|x| x.is_some_and(|n| n.id == id))
            .ok_or(VfsError::NotFound)?;
        if self.items.iter().flatten().any(|n| n.parent == id) {
            return Err(VfsError::NotEmpty);
        }
        let n = self.items[i].take().ok_or(VfsError::NotFound)?;
        self.len -= 1;
        Ok(n)
    }
    pub const fn len(&self) -> usize {
        self.len
    }
}
impl<const N: usize> Default for Vfs<N> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_validation() {
        assert!(validate_path("/usr/bin").is_ok());
        assert_eq!(validate_path("../etc"), Err(VfsError::BadPath));
        assert_eq!(validate_path("/a/../b"), Err(VfsError::InvalidName));
    }
    #[test]
    fn create_remove() {
        let mut v = Vfs::<3>::new();
        let r = v.init_root().unwrap();
        let d = v.create(r, "etc", NodeKind::Directory, 0o755).unwrap();
        let f = v.create(d, "config", NodeKind::Regular, 0o600).unwrap();
        assert_eq!(
            v.create(d, "config", NodeKind::Regular, 0o600),
            Err(VfsError::Duplicate)
        );
        assert_eq!(v.remove(d), Err(VfsError::NotEmpty));
        v.remove(f).unwrap();
        v.remove(d).unwrap();
        assert_eq!(v.remove(r), Err(VfsError::RootMutation));
    }
}
