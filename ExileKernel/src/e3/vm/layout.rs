//! Default 48-bit x86-64 virtual layout. The top canonical page is reserved.
use super::geometry::{KERNEL_CANONICAL_START, PAGE_SIZE, USER_CANONICAL_END, is_canonical};
pub const USER_START: u64 = PAGE_SIZE;
pub const USER_END: u64 = USER_CANONICAL_END;
pub const KERNEL_START: u64 = KERNEL_CANONICAL_START;
pub const KERNEL_END: u64 = u64::MAX - (PAGE_SIZE - 1);
pub const NULL_GUARD_END: u64 = PAGE_SIZE;
pub const TOP_GUARD_START: u64 = 0xffff_ffff_ffff_f000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    User,
    Kernel,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    NonCanonical,
    Overflow,
    OutsideClass,
    ReservedGuard,
}
pub fn validate_range(start: u64, length: u64, class: AddressClass) -> Result<u64, LayoutError> {
    if length == 0 || !is_canonical(start) {
        return Err(LayoutError::NonCanonical);
    }
    let end = start.checked_add(length).ok_or(LayoutError::Overflow)?;
    if !is_canonical(end - 1) {
        return Err(LayoutError::NonCanonical);
    }
    match class {
        AddressClass::User if start >= USER_START && end <= USER_END => {}
        AddressClass::Kernel if start >= KERNEL_START && end <= KERNEL_END => {}
        _ => return Err(LayoutError::OutsideClass),
    }
    if start < TOP_GUARD_START && end > TOP_GUARD_START {
        return Err(LayoutError::ReservedGuard);
    }
    Ok(end)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_halves() {
        assert!(validate_range(0x4000, 4096, AddressClass::User).is_ok());
        assert!(validate_range(KERNEL_START, 4096, AddressClass::Kernel).is_ok());
        assert_eq!(
            validate_range(KERNEL_START, 4096, AddressClass::User),
            Err(LayoutError::OutsideClass)
        );
        assert_eq!(
            validate_range(0, 4096, AddressClass::User),
            Err(LayoutError::OutsideClass)
        );
    }
}
