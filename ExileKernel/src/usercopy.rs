//! Checked user-memory copy interface. Implementors must contain page faults and validate mappings.
#![no_std]
pub const MAX_COPY: usize = 65536;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserCopyError {
    Null,
    Overflow,
    TooLong,
    Unmapped,
    Permission,
    Fault,
}
pub trait UserMemory {
    fn read_user(&self, address: u64, out: &mut [u8]) -> Result<(), UserCopyError>;
    fn write_user(&mut self, address: u64, input: &[u8]) -> Result<(), UserCopyError>;
}
pub fn checked_user_range(a: u64, n: usize) -> Result<(), UserCopyError> {
    if n == 0 {
        return Ok(());
    }
    if a == 0 {
        return Err(UserCopyError::Null);
    }
    if n > MAX_COPY {
        return Err(UserCopyError::TooLong);
    }
    a.checked_add(u64::try_from(n).map_err(|_| UserCopyError::Overflow)?)
        .ok_or(UserCopyError::Overflow)?;
    Ok(())
}
pub fn copy_from_user<M: UserMemory>(m: &M, a: u64, out: &mut [u8]) -> Result<(), UserCopyError> {
    checked_user_range(a, out.len())?;
    if out.is_empty() {
        return Ok(());
    }
    m.read_user(a, out)
}
pub fn copy_to_user<M: UserMemory>(m: &mut M, a: u64, input: &[u8]) -> Result<(), UserCopyError> {
    checked_user_range(a, input.len())?;
    if input.is_empty() {
        return Ok(());
    }
    m.write_user(a, input)
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Mock([u8; 16]);
    impl UserMemory for Mock {
        fn read_user(&self, a: u64, o: &mut [u8]) -> Result<(), UserCopyError> {
            let s = a as usize;
            let e = s.checked_add(o.len()).ok_or(UserCopyError::Overflow)?;
            o.copy_from_slice(self.0.get(s..e).ok_or(UserCopyError::Unmapped)?);
            Ok(())
        }
        fn write_user(&mut self, a: u64, i: &[u8]) -> Result<(), UserCopyError> {
            let s = a as usize;
            let e = s.checked_add(i.len()).ok_or(UserCopyError::Overflow)?;
            self.0
                .get_mut(s..e)
                .ok_or(UserCopyError::Unmapped)?
                .copy_from_slice(i);
            Ok(())
        }
    }
    #[test]
    fn copies() {
        let mut m = Mock(*b"abcdefghijklmnop");
        let mut b = [0; 4];
        copy_from_user(&m, 2, &mut b).unwrap();
        assert_eq!(&b, b"cdef");
        copy_to_user(&mut m, 4, b"ZZ").unwrap();
        assert_eq!(&m.0[4..6], b"ZZ");
    }
    #[test]
    fn zero_length_copy_is_noop() {
        let mut m = Mock(*b"abcdefghijklmnop");
        assert_eq!(copy_from_user(&m, 0, &mut []), Ok(()));
        assert_eq!(copy_to_user(&mut m, 0, &[]), Ok(()));
    }
    #[test]
    fn rejects_ranges() {
        assert_eq!(checked_user_range(0, 1), Err(UserCopyError::Null));
        assert_eq!(
            checked_user_range(u64::MAX, 2),
            Err(UserCopyError::Overflow)
        );
        assert_eq!(
            checked_user_range(1, MAX_COPY + 1),
            Err(UserCopyError::TooLong)
        );
    }
}
