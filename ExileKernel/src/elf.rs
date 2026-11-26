//! Bounded ELF64 little-endian PT_LOAD validator. Does not load or execute images.
#![no_std]
#[cfg(test)]
extern crate std;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    Truncated,
    BadMagic,
    UnsupportedClass,
    UnsupportedEndian,
    UnsupportedVersion,
    BadType,
    BadMachine,
    BadHeader,
    TooManySegments,
    BadSegment,
    Overflow,
    WriteExecute,
    Overlap,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub offset: u64,
    pub virtual_address: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub flags: u32,
    pub alignment: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElfInfo<const N: usize> {
    pub entry: u64,
    items: [Option<Segment>; N],
    len: usize,
}
impl<const N: usize> ElfInfo<N> {
    pub fn segments(&self) -> impl Iterator<Item = &Segment> {
        self.items[..self.len].iter().filter_map(Option::as_ref)
    }
}
fn u16at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
pub fn validate_elf64<const N: usize>(b: &[u8], machine: u16) -> Result<ElfInfo<N>, ElfError> {
    if b.len() < 64 {
        return Err(ElfError::Truncated);
    }
    if &b[..4] != b"\x7fELF" {
        return Err(ElfError::BadMagic);
    }
    if b[4] != 2 {
        return Err(ElfError::UnsupportedClass);
    }
    if b[5] != 1 {
        return Err(ElfError::UnsupportedEndian);
    }
    if b[6] != 1 {
        return Err(ElfError::UnsupportedVersion);
    }
    if !matches!(u16at(b, 16), 2 | 3) {
        return Err(ElfError::BadType);
    }
    if u16at(b, 18) != machine {
        return Err(ElfError::BadMachine);
    }
    if u32at(b, 20) != 1 || u16at(b, 52) != 64 {
        return Err(ElfError::BadHeader);
    }
    let entry = u64at(b, 24);
    let po = usize::try_from(u64at(b, 32)).map_err(|_| ElfError::Overflow)?;
    let es = u16at(b, 54) as usize;
    let count = u16at(b, 56) as usize;
    if es < 56 {
        return Err(ElfError::BadHeader);
    }
    if count > 0 && po < 64 {
        return Err(ElfError::BadHeader);
    }
    let end = po
        .checked_add(es.checked_mul(count).ok_or(ElfError::Overflow)?)
        .ok_or(ElfError::Overflow)?;
    if end > b.len() {
        return Err(ElfError::Truncated);
    }
    let mut out = ElfInfo {
        entry,
        items: [None; N],
        len: 0,
    };
    for i in 0..count {
        let o = po + i * es;
        if u32at(b, o) != 1 {
            continue;
        }
        let flags = u32at(b, o + 4);
        let off = u64at(b, o + 8);
        let va = u64at(b, o + 16);
        let fs = u64at(b, o + 32);
        let ms = u64at(b, o + 40);
        let al = u64at(b, o + 48);
        if fs > ms || ms == 0 {
            return Err(ElfError::BadSegment);
        }
        let file_end = off.checked_add(fs).ok_or(ElfError::Overflow)?;
        let mem_end = va.checked_add(ms).ok_or(ElfError::Overflow)?;
        if file_end > b.len() as u64 {
            return Err(ElfError::Truncated);
        }
        if al > 1 && (!al.is_power_of_two() || va % al != off % al) {
            return Err(ElfError::BadSegment);
        }
        if flags & !7 != 0 {
            return Err(ElfError::BadSegment);
        }
        if flags & 3 == 3 {
            return Err(ElfError::WriteExecute);
        }
        if out.len == N {
            return Err(ElfError::TooManySegments);
        }
        for old in out.segments() {
            let oe = old
                .virtual_address
                .checked_add(old.memory_size)
                .ok_or(ElfError::Overflow)?;
            if va < oe && old.virtual_address < mem_end {
                return Err(ElfError::Overlap);
            }
        }
        out.items[out.len] = Some(Segment {
            offset: off,
            virtual_address: va,
            file_size: fs,
            memory_size: ms,
            flags,
            alignment: al,
        });
        out.len += 1;
    }
    if out.len == 0
        || !out.segments().any(|s| {
            s.flags & 1 != 0
                && entry >= s.virtual_address
                && entry < s.virtual_address + s.memory_size
        })
    {
        return Err(ElfError::BadSegment);
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn image() -> std::vec::Vec<u8> {
        let mut b = std::vec![0;120];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        b[16..18].copy_from_slice(&2u16.to_le_bytes());
        b[18..20].copy_from_slice(&62u16.to_le_bytes());
        b[20..24].copy_from_slice(&1u32.to_le_bytes());
        b[24..32].copy_from_slice(&0x400000u64.to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[52..54].copy_from_slice(&64u16.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&1u16.to_le_bytes());
        b[64..68].copy_from_slice(&1u32.to_le_bytes());
        b[68..72].copy_from_slice(&5u32.to_le_bytes());
        b[72..80].copy_from_slice(&120u64.to_le_bytes());
        b[80..88].copy_from_slice(&0x400000u64.to_le_bytes());
        b[104..112].copy_from_slice(&0x1000u64.to_le_bytes());
        b[112..120].copy_from_slice(&1u64.to_le_bytes());
        b
    }
    #[test]
    fn valid() {
        assert_eq!(
            validate_elf64::<4>(&image(), 62)
                .unwrap()
                .segments()
                .count(),
            1
        );
    }
    #[test]
    fn ignores_non_load_headers_when_bounding_load_segments() {
        let mut b = image();
        b.resize(176, 0);
        b[56..58].copy_from_slice(&2u16.to_le_bytes());
        b[120..124].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(validate_elf64::<1>(&b, 62).unwrap().segments().count(), 1);
    }
    #[test]
    fn malformed() {
        let mut b = image();
        b[0] = 0;
        assert_eq!(validate_elf64::<4>(&b, 62), Err(ElfError::BadMagic));
        let mut b = image();
        b[68..72].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(validate_elf64::<4>(&b, 62), Err(ElfError::WriteExecute));
        assert_eq!(
            validate_elf64::<4>(&image()[..30], 62),
            Err(ElfError::Truncated)
        );
    }
}
