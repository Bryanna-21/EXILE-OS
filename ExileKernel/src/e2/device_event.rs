//! Bounded FIFO device-event journal; overflow is reported, never silently overwritten.
use super::device::DeviceState;
use super::device_id::DeviceId;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEventKind {
    Added,
    Removed,
    StateChanged { from: DeviceState, to: DeviceState },
    DriverBound { driver: u32 },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceEvent {
    pub sequence: u64,
    pub device: DeviceId,
    pub kind: DeviceEventKind,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    Full,
    SequenceExhausted,
    Corrupt,
}
pub struct DeviceEvents<const N: usize> {
    items: [Option<DeviceEvent>; N],
    head: usize,
    len: usize,
    next: u64,
}
impl<const N: usize> DeviceEvents<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            head: 0,
            len: 0,
            next: 1,
        }
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub fn push(&mut self, device: DeviceId, kind: DeviceEventKind) -> Result<u64, EventError> {
        if self.len == N {
            return Err(EventError::Full);
        }
        let seq = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(EventError::SequenceExhausted)?;
        let i = (self.head + self.len) % N.max(1);
        self.items[i] = Some(DeviceEvent {
            sequence: seq,
            device,
            kind,
        });
        self.len += 1;
        Ok(seq)
    }
    pub fn pop(&mut self) -> Result<Option<DeviceEvent>, EventError> {
        if self.len == 0 {
            return Ok(None);
        }
        let e = self.items[self.head].take().ok_or(EventError::Corrupt)?;
        self.head = (self.head + 1) % N.max(1);
        self.len -= 1;
        Ok(Some(e))
    }
    pub fn check_invariants(&self) -> bool {
        self.len <= N && self.items.iter().filter(|x| x.is_some()).count() == self.len
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2::device_id::BusKind;
    #[test]
    fn bounded_sequence_and_fifo() {
        let id = DeviceId::new(BusKind::Virtual, 0, 1, 1).unwrap();
        let mut q = DeviceEvents::<1>::new();
        assert_eq!(q.push(id, DeviceEventKind::Added), Ok(1));
        assert_eq!(q.push(id, DeviceEventKind::Removed), Err(EventError::Full));
        assert_eq!(q.pop().unwrap().unwrap().kind, DeviceEventKind::Added);
        assert!(q.check_invariants());
    }
}
