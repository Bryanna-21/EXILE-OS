//! Legacy 8259 PIC remap and 8254 PIT programming: constants, parameter validation and the
//! exact port-write sequences (so the sequences can be unit-tested; `kernel/` performs them).

use core::sync::atomic::{AtomicU64, Ordering};

pub const PIC1_CMD: u16 = 0x20;
pub const PIC1_DATA: u16 = 0x21;
pub const PIC2_CMD: u16 = 0xA0;
pub const PIC2_DATA: u16 = 0xA1;
pub const PIT_CH0: u16 = 0x40;
pub const PIT_CMD: u16 = 0x43;
pub const PIT_BASE_HZ: u32 = 1_193_182;
pub const PIC_EOI: u8 = 0x20;
/// Channel 0, lobyte/hibyte access, mode 2 (rate generator), binary.
pub const PIT_CMD_RATE: u8 = 0x34;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimerError {
    /// Requested frequency outside 19 Hz ..= base frequency.
    Frequency,
    /// PIC vector offset is not a multiple of 8, or collides with CPU exceptions.
    Offset,
}

/// 16-bit PIT reload value for `hz` (rounded to nearest). A reload of 0 would mean 65536,
/// so frequencies below 19 Hz are refused instead of being mapped onto it.
pub fn divisor_for(hz: u32) -> Result<u16, TimerError> {
    if !(19..=PIT_BASE_HZ).contains(&hz) {
        return Err(TimerError::Frequency);
    }
    let d = (PIT_BASE_HZ + hz / 2) / hz;
    if d == 0 || d > 65535 {
        return Err(TimerError::Frequency);
    }
    Ok(d as u16)
}

/// The `(port, value)` writes that remap both PICs to `base1`/`base2` and then apply the
/// interrupt masks (a set bit masks the line). Cascade on IRQ2.
pub fn pic_sequence(
    base1: u8,
    base2: u8,
    mask1: u8,
    mask2: u8,
) -> Result<[(u16, u8); 10], TimerError> {
    if base1 < 32 || base2 < 32 || base1 % 8 != 0 || base2 % 8 != 0 {
        return Err(TimerError::Offset);
    }
    if base1.abs_diff(base2) < 8 {
        return Err(TimerError::Offset);
    }
    Ok([
        (PIC1_CMD, 0x11),
        (PIC2_CMD, 0x11),
        (PIC1_DATA, base1),
        (PIC2_DATA, base2),
        (PIC1_DATA, 4),
        (PIC2_DATA, 2),
        (PIC1_DATA, 0x01),
        (PIC2_DATA, 0x01),
        (PIC1_DATA, mask1),
        (PIC2_DATA, mask2),
    ])
}

/// `(port, value)` writes that start the PIT at `hz`.
pub fn pit_sequence(hz: u32) -> Result<[(u16, u8); 3], TimerError> {
    let d = divisor_for(hz)?;
    Ok([
        (PIT_CMD, PIT_CMD_RATE),
        (PIT_CH0, d as u8),
        (PIT_CH0, (d >> 8) as u8),
    ])
}

/// Monotonic tick counter updated from the timer interrupt.
pub struct Ticks(AtomicU64);

impl Default for Ticks {
    fn default() -> Self {
        Self::new()
    }
}

impl Ticks {
    pub const fn new() -> Self {
        Ticks(AtomicU64::new(0))
    }
    pub fn tick(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
