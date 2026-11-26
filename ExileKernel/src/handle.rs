//! Generational object handles. This table neither owns nor dereferences the referenced object.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle {
    slot: u16,
    generation: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleError {
    Capacity,
    Invalid,
    Closed,
    AlreadyClosed,
}
#[derive(Debug, Clone, Copy)]
struct Entry {
    object: u64,
    generation: u32,
    open: bool,
}
pub struct HandleTable<const N: usize> {
    items: [Option<Entry>; N],
}
impl<const N: usize> HandleTable<N> {
    pub const fn new() -> Self {
        Self { items: [None; N] }
    }
    pub fn insert(&mut self, obj: u64) -> Result<Handle, HandleError> {
        if obj == 0 {
            return Err(HandleError::Invalid);
        }
        let i = self
            .items
            .iter()
            .position(|x| x.is_none() || x.is_some_and(|e| !e.open))
            .ok_or(HandleError::Capacity)?;
        let slot = u16::try_from(i).map_err(|_| HandleError::Capacity)?;
        let g = self.items[i].map_or(1, |e| e.generation.wrapping_add(1).max(1));
        self.items[i] = Some(Entry {
            object: obj,
            generation: g,
            open: true,
        });
        Ok(Handle {
            slot,
            generation: g,
        })
    }
    pub fn get(&self, h: Handle) -> Result<u64, HandleError> {
        let e = self
            .items
            .get(h.slot as usize)
            .and_then(|x| *x)
            .ok_or(HandleError::Invalid)?;
        if e.generation != h.generation {
            return Err(HandleError::Invalid);
        }
        if !e.open {
            return Err(HandleError::Closed);
        }
        Ok(e.object)
    }
    pub fn close(&mut self, h: Handle) -> Result<u64, HandleError> {
        let e = self
            .items
            .get_mut(h.slot as usize)
            .and_then(Option::as_mut)
            .ok_or(HandleError::Invalid)?;
        if e.generation != h.generation {
            return Err(HandleError::Invalid);
        }
        if !e.open {
            return Err(HandleError::AlreadyClosed);
        }
        e.open = false;
        Ok(e.object)
    }
    pub fn open_count(&self) -> usize {
        self.items.iter().flatten().filter(|e| e.open).count()
    }
}
impl<const N: usize> Default for HandleTable<N> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle() {
        let mut t = HandleTable::<1>::new();
        let h = t.insert(42).unwrap();
        assert_eq!(t.get(h), Ok(42));
        t.close(h).unwrap();
        assert_eq!(t.get(h), Err(HandleError::Closed));
        assert_eq!(t.close(h), Err(HandleError::AlreadyClosed));
    }
    #[test]
    fn stale_after_reuse() {
        let mut t = HandleTable::<1>::new();
        let a = t.insert(1).unwrap();
        t.close(a).unwrap();
        let b = t.insert(2).unwrap();
        assert_ne!(a.generation, b.generation);
        assert_eq!(t.get(a), Err(HandleError::Invalid));
    }
}
