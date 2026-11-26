//! Architecture-neutral syscall registry; architecture entry/return assembly is intentionally absent.
#![no_std]
pub const MAX_SYSCALLS: usize = 128;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallArgs {
    pub values: [u64; 6],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallError {
    OutOfRange,
    Duplicate,
    Unregistered,
    HandlerFailure,
    Capacity,
}
pub type Handler = fn(SyscallArgs) -> Result<u64, SyscallError>;
#[derive(Clone, Copy)]
struct Entry {
    number: u16,
    handler: Handler,
}
pub struct SyscallTable<const N: usize> {
    entries: [Option<Entry>; N],
    len: usize,
}
impl<const N: usize> SyscallTable<N> {
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            len: 0,
        }
    }
    pub fn register(&mut self, n: u16, h: Handler) -> Result<(), SyscallError> {
        if n as usize >= MAX_SYSCALLS {
            return Err(SyscallError::OutOfRange);
        }
        if self.entries.iter().flatten().any(|e| e.number == n) {
            return Err(SyscallError::Duplicate);
        }
        let s = self
            .entries
            .iter_mut()
            .find(|e| e.is_none())
            .ok_or(SyscallError::Capacity)?;
        *s = Some(Entry {
            number: n,
            handler: h,
        });
        self.len += 1;
        Ok(())
    }
    pub fn dispatch(&self, n: u64, a: SyscallArgs) -> Result<u64, SyscallError> {
        let n = u16::try_from(n).map_err(|_| SyscallError::OutOfRange)?;
        if n as usize >= MAX_SYSCALLS {
            return Err(SyscallError::OutOfRange);
        }
        (self
            .entries
            .iter()
            .flatten()
            .find(|e| e.number == n)
            .ok_or(SyscallError::Unregistered)?
            .handler)(a)
    }
    pub const fn len(&self) -> usize {
        self.len
    }
}
impl<const N: usize> Default for SyscallTable<N> {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn add(a: SyscallArgs) -> Result<u64, SyscallError> {
        Ok(a.values[0] + a.values[1])
    }
    #[test]
    fn register_dispatch() {
        let mut t = SyscallTable::<2>::new();
        t.register(1, add).unwrap();
        assert_eq!(
            t.dispatch(
                1,
                SyscallArgs {
                    values: [2, 3, 0, 0, 0, 0]
                }
            ),
            Ok(5)
        );
        assert_eq!(
            t.dispatch(2, SyscallArgs { values: [0; 6] }),
            Err(SyscallError::Unregistered)
        );
    }
    #[test]
    fn duplicate_capacity_and_range() {
        let mut t = SyscallTable::<1>::new();
        t.register(1, add).unwrap();
        assert_eq!(t.register(1, add), Err(SyscallError::Duplicate));
        assert_eq!(t.register(2, add), Err(SyscallError::Capacity));
        assert_eq!(
            t.register(MAX_SYSCALLS as u16, add),
            Err(SyscallError::OutOfRange)
        );
        assert_eq!(
            t.dispatch(u64::MAX, SyscallArgs { values: [0; 6] }),
            Err(SyscallError::OutOfRange)
        );
    }
}
