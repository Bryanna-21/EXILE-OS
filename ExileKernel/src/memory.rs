//! Original bounded Multiboot map algorithms. No allocation or pointer dereferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    RecordSize,
    Overflow,
    Empty,
    TooMany,
    Overlap,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub kind: u32,
}
impl Region {
    pub fn new(start: u64, length: u64, kind: u32) -> Result<Self, Error> {
        if length == 0 {
            return Err(Error::Empty);
        }
        Ok(Self {
            start,
            end: start.checked_add(length).ok_or(Error::Overflow)?,
            kind,
        })
    }
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes([
        b[i],
        b[i + 1],
        b[i + 2],
        b[i + 3],
        b[i + 4],
        b[i + 5],
        b[i + 6],
        b[i + 7],
    ])
}
pub fn parse_map(bytes: &[u8], out: &mut [Region]) -> Result<usize, Error> {
    let mut offset = 0usize;
    let mut count = 0;
    while offset < bytes.len() {
        if bytes.len() - offset < 4 {
            return Err(Error::Truncated);
        }
        let size = u32_at(bytes, offset) as usize;
        if size < 20 {
            return Err(Error::RecordSize);
        }
        let next = offset
            .checked_add(4)
            .and_then(|x| x.checked_add(size))
            .ok_or(Error::Overflow)?;
        if next > bytes.len() {
            return Err(Error::Truncated);
        }
        let region = Region::new(
            u64_at(bytes, offset + 4),
            u64_at(bytes, offset + 12),
            u32_at(bytes, offset + 20),
        )?;
        if count == out.len() {
            return Err(Error::TooMany);
        }
        if out[..count]
            .iter()
            .any(|r| region.start < r.end && r.start < region.end)
        {
            return Err(Error::Overlap);
        }
        out[count] = region;
        count += 1;
        offset = next;
    }
    if count == 0 {
        return Err(Error::Empty);
    }
    Ok(count)
}
/// Returns an aligned usable range, clipped above a reserved prefix and below a ceiling.
/// Not a live allocator: loader/kernel/module reservations must be accounted for before use.
pub fn usable_pages(region: Region, reserved_end: u64, ceiling: u64) -> Option<(u64, u64)> {
    if region.kind != 1 {
        return None;
    }
    let start = region.start.max(reserved_end).checked_add(4095)? & !4095;
    let end = region.end.min(ceiling) & !4095;
    if start >= end {
        return None;
    }
    Some((start, (end - start) / 4096))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record(start: u64, len: u64, kind: u32) -> Vec<u8> {
        let mut v = 20u32.to_le_bytes().to_vec();
        v.extend(start.to_le_bytes());
        v.extend(len.to_le_bytes());
        v.extend(kind.to_le_bytes());
        v
    }
    fn parse(b: &[u8]) -> Result<usize, Error> {
        parse_map(
            b,
            &mut [Region {
                start: 0,
                end: 0,
                kind: 0,
            }; 4],
        )
    }
    #[test]
    fn valid() {
        assert_eq!(parse(&record(4096, 8192, 1)), Ok(1));
    }
    #[test]
    fn empty_map() {
        assert_eq!(parse(&[]), Err(Error::Empty));
    }
    #[test]
    fn short_header() {
        assert_eq!(parse(&[0; 3]), Err(Error::Truncated));
    }
    #[test]
    fn short_record() {
        assert_eq!(parse(&record(0, 4096, 1)[..23]), Err(Error::Truncated));
    }
    #[test]
    fn small_record() {
        assert_eq!(parse(&19u32.to_le_bytes()), Err(Error::RecordSize));
    }
    #[test]
    fn huge_record() {
        assert_eq!(parse(&u32::MAX.to_le_bytes()), Err(Error::Truncated));
    }
    #[test]
    fn zero_range() {
        assert_eq!(parse(&record(0, 0, 1)), Err(Error::Empty));
    }
    #[test]
    fn overflowing_range() {
        assert_eq!(parse(&record(u64::MAX, 1, 1)), Err(Error::Overflow));
    }
    #[test]
    fn overlap() {
        let mut b = record(0, 8192, 1);
        b.extend(record(4096, 8192, 2));
        assert_eq!(parse(&b), Err(Error::Overlap));
    }
    #[test]
    fn adjacent() {
        let mut b = record(0, 4096, 1);
        b.extend(record(4096, 4096, 2));
        assert_eq!(parse(&b), Ok(2));
    }
    #[test]
    fn unordered() {
        let mut b = record(8192, 4096, 1);
        b.extend(record(0, 4096, 2));
        assert_eq!(parse(&b), Ok(2));
    }
    #[test]
    fn bounded_capacity() {
        assert_eq!(parse_map(&record(0, 4096, 1), &mut []), Err(Error::TooMany));
    }
    #[test]
    fn extended_record() {
        let mut b = record(0, 4096, 1);
        b[..4].copy_from_slice(&24u32.to_le_bytes());
        b.extend([0; 4]);
        assert_eq!(parse(&b), Ok(1));
    }
    #[test]
    fn reserved_kind() {
        assert_eq!(
            usable_pages(Region::new(0, 8192, 2).unwrap(), 0, 8192),
            None
        );
    }
    #[test]
    fn page_alignment() {
        assert_eq!(
            usable_pages(Region::new(1, 12287, 1).unwrap(), 0, 12288),
            Some((4096, 2))
        );
    }
    #[test]
    fn reserved_prefix() {
        assert_eq!(
            usable_pages(Region::new(0, 16384, 1).unwrap(), 8192, 16384),
            Some((8192, 2))
        );
    }
    #[test]
    fn ceiling() {
        assert_eq!(
            usable_pages(Region::new(0, 16384, 1).unwrap(), 0, 8192),
            Some((0, 2))
        );
    }
    #[test]
    fn alignment_overflow() {
        assert_eq!(
            usable_pages(Region::new(u64::MAX - 1, 1, 1).unwrap(), 0, u64::MAX),
            None
        );
    }
}
