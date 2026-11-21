//! Boot handoff and safe-update state.
/// Versioned bootloader -> kernel handoff structure.
pub mod info;
/// A/B slot state machine with boot counting and rollback.
pub mod slots;
