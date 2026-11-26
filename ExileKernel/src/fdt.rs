//! Bounded flattened-device-tree (FDT) header and structure-token parser.
//! SPDX-License-Identifier: Apache-2.0
//! No allocation, no pointer casts, and no firmware-controlled unchecked offsets.

#![allow(dead_code)]

pub const FDT_MAGIC: u32 = 0xd00d_feed;
pub const MAX_FDT_SIZE: usize = 16 * 1024 * 1024;
const HEADER_LEN: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdtError {
    Truncated,
    BadMagic,
    BadTotalSize,
    TooLarge,
    BadVersion,
    BadAlignment,
    BadBlock,
    BadReserveMap,
    BadToken,
    BadNesting,
    BadRoot,
    MissingEnd,
    BadString,
}

#[derive(Debug, Clone, Copy)]
pub struct Fdt<'a> {
    bytes: &'a [u8],
    struct_start: usize,
    struct_end: usize,
    strings_start: usize,
    strings_end: usize,
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn checked_range(start: u32, len: u32, total: usize) -> Result<(usize, usize), FdtError> {
    let s = start as usize;
    let e = s.checked_add(len as usize).ok_or(FdtError::BadBlock)?;
    if s < HEADER_LEN || e > total || s > e {
        return Err(FdtError::BadBlock);
    }
    Ok((s, e))
}
fn align4(n: usize) -> Result<usize, FdtError> {
    n.checked_add(3).map(|v| v & !3).ok_or(FdtError::BadBlock)
}

impl<'a> Fdt<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, FdtError> {
        if bytes.len() < HEADER_LEN {
            return Err(FdtError::Truncated);
        }
        if be32(bytes, 0) != FDT_MAGIC {
            return Err(FdtError::BadMagic);
        }
        let total = be32(bytes, 4) as usize;
        if total < HEADER_LEN || total > bytes.len() {
            return Err(FdtError::BadTotalSize);
        }
        if total > MAX_FDT_SIZE {
            return Err(FdtError::TooLarge);
        }
        let struct_off = be32(bytes, 8);
        let strings_off = be32(bytes, 12);
        let reserve_off = be32(bytes, 16) as usize;
        let version = be32(bytes, 20);
        let last = be32(bytes, 24);
        let strings_len = be32(bytes, 32);
        let struct_len = be32(bytes, 36);
        if version < 17 || last > 17 || version < last {
            return Err(FdtError::BadVersion);
        }
        if struct_off & 3 != 0 || reserve_off & 7 != 0 {
            return Err(FdtError::BadAlignment);
        }
        let (ss, se) = checked_range(struct_off, struct_len, total)?;
        let (ts, te) = checked_range(strings_off, strings_len, total)?;
        if ss < te && ts < se {
            return Err(FdtError::BadBlock);
        }
        if reserve_off < HEADER_LEN || reserve_off >= total {
            return Err(FdtError::BadReserveMap);
        }

        // Reserve-map entries are pairs of big-endian u64; require a terminator.
        let mut p = reserve_off;
        let mut terminated = false;
        while p.checked_add(16).filter(|e| *e <= total).is_some() {
            let a = u64::from_be_bytes(bytes[p..p + 8].try_into().unwrap());
            let n = u64::from_be_bytes(bytes[p + 8..p + 16].try_into().unwrap());
            p += 16;
            if a == 0 && n == 0 {
                terminated = true;
                break;
            }
            if a.checked_add(n).is_none() {
                return Err(FdtError::BadReserveMap);
            }
        }
        if !terminated {
            return Err(FdtError::BadReserveMap);
        }
        // The reserve map must not alias either data block.
        if (reserve_off < se && ss < p) || (reserve_off < te && ts < p) {
            return Err(FdtError::BadReserveMap);
        }
        let fdt = Self {
            bytes: &bytes[..total],
            struct_start: ss,
            struct_end: se,
            strings_start: ts,
            strings_end: te,
        };
        fdt.validate()?;
        Ok(fdt)
    }

    pub fn validate(self) -> Result<(), FdtError> {
        let mut tokens = self.tokens();
        loop {
            match tokens.next() {
                Some(Ok(Token::End)) => return Ok(()),
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error),
                None => return Err(FdtError::MissingEnd),
            }
        }
    }

    pub fn tokens(self) -> Tokens<'a> {
        Tokens {
            fdt: self,
            pos: self.struct_start,
            depth: 0,
            root_seen: false,
            root_closed: false,
            ended: false,
            failed: false,
        }
    }

    fn string(self, off: u32) -> Result<&'a str, FdtError> {
        let start = self
            .strings_start
            .checked_add(off as usize)
            .ok_or(FdtError::BadString)?;
        if start >= self.strings_end {
            return Err(FdtError::BadString);
        }
        let tail = &self.bytes[start..self.strings_end];
        let end = tail
            .iter()
            .position(|b| *b == 0)
            .ok_or(FdtError::BadString)?;
        core::str::from_utf8(&tail[..end]).map_err(|_| FdtError::BadString)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token<'a> {
    BeginNode(&'a str),
    EndNode,
    Property { name: &'a str, value: &'a [u8] },
    Nop,
    End,
}

pub struct Tokens<'a> {
    fdt: Fdt<'a>,
    pos: usize,
    depth: u32,
    root_seen: bool,
    root_closed: bool,
    ended: bool,
    failed: bool,
}
impl<'a> Iterator for Tokens<'a> {
    type Item = Result<Token<'a>, FdtError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.ended || self.failed {
            return None;
        }
        let result = self.next_inner();
        if result.is_err() {
            self.failed = true;
        }
        if matches!(result, Ok(Token::End)) {
            self.ended = true;
        }
        Some(result)
    }
}
impl<'a> Tokens<'a> {
    fn next_inner(&mut self) -> Result<Token<'a>, FdtError> {
        if self
            .pos
            .checked_add(4)
            .filter(|e| *e <= self.fdt.struct_end)
            .is_none()
        {
            return Err(FdtError::MissingEnd);
        }
        let tag = be32(self.fdt.bytes, self.pos);
        self.pos += 4;
        match tag {
            1 => {
                let tail = &self.fdt.bytes[self.pos..self.fdt.struct_end];
                let n = tail
                    .iter()
                    .position(|b| *b == 0)
                    .ok_or(FdtError::BadString)?;
                let name = core::str::from_utf8(&tail[..n]).map_err(|_| FdtError::BadString)?;
                if self.depth == 0 {
                    if self.root_seen {
                        return Err(FdtError::BadNesting);
                    }
                    if !name.is_empty() {
                        return Err(FdtError::BadRoot);
                    }
                    self.root_seen = true;
                }
                self.pos = align4(self.pos.checked_add(n + 1).ok_or(FdtError::BadBlock)?)?;
                if self.pos > self.fdt.struct_end {
                    return Err(FdtError::BadBlock);
                }
                self.depth = self.depth.checked_add(1).ok_or(FdtError::BadNesting)?;
                Ok(Token::BeginNode(name))
            }
            2 => {
                if self.depth == 0 {
                    return Err(FdtError::BadNesting);
                }
                self.depth -= 1;
                if self.depth == 0 {
                    self.root_closed = true;
                }
                Ok(Token::EndNode)
            }
            3 => {
                if self.depth == 0 || self.pos + 8 > self.fdt.struct_end {
                    return Err(FdtError::BadToken);
                }
                let len = be32(self.fdt.bytes, self.pos) as usize;
                let nameoff = be32(self.fdt.bytes, self.pos + 4);
                self.pos += 8;
                let end = self.pos.checked_add(len).ok_or(FdtError::BadBlock)?;
                if end > self.fdt.struct_end {
                    return Err(FdtError::BadBlock);
                }
                let value = &self.fdt.bytes[self.pos..end];
                self.pos = align4(end)?;
                if self.pos > self.fdt.struct_end {
                    return Err(FdtError::BadBlock);
                }
                Ok(Token::Property {
                    name: self.fdt.string(nameoff)?,
                    value,
                })
            }
            4 => Ok(Token::Nop),
            9 => {
                if self.depth != 0 || !self.root_seen || !self.root_closed {
                    return Err(FdtError::BadRoot);
                }
                if self.pos != self.fdt.struct_end {
                    return Err(FdtError::BadToken);
                }
                Ok(Token::End)
            }
            _ => Err(FdtError::BadToken),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn minimal() -> [u8; 76] {
        let mut b = [0u8; 76];
        b[0..4].copy_from_slice(&FDT_MAGIC.to_be_bytes());
        b[4..8].copy_from_slice(&76u32.to_be_bytes());
        b[8..12].copy_from_slice(&60u32.to_be_bytes());
        b[12..16].copy_from_slice(&56u32.to_be_bytes());
        b[16..20].copy_from_slice(&40u32.to_be_bytes());
        b[20..24].copy_from_slice(&17u32.to_be_bytes());
        b[24..28].copy_from_slice(&16u32.to_be_bytes());
        b[32..36].copy_from_slice(&4u32.to_be_bytes());
        b[36..40].copy_from_slice(&16u32.to_be_bytes());
        // Empty reserve-map terminator occupies bytes 40..55.
        // Strings block: 56..59. Structure block: 60..75.
        b[56] = b'x';
        b[60..64].copy_from_slice(&1u32.to_be_bytes());
        b[64..68].copy_from_slice(&0u32.to_be_bytes());
        b[68..72].copy_from_slice(&2u32.to_be_bytes());
        b[72..76].copy_from_slice(&9u32.to_be_bytes());
        b
    }
    #[test]
    fn rejects_bad_magic() {
        let mut b = minimal();
        b[0] = 0;
        assert_eq!(Fdt::parse(&b).err(), Some(FdtError::BadMagic));
    }
    #[test]
    fn rejects_truncated_header() {
        assert_eq!(Fdt::parse(&[0; 8]).err(), Some(FdtError::Truncated));
    }
    #[test]
    fn accepts_minimal_empty_tree() {
        let b = minimal();
        let f = Fdt::parse(&b).unwrap();
        let t: Vec<_> = f.tokens().collect();
        assert!(matches!(t[0], Ok(Token::BeginNode(""))));
        assert!(matches!(t[2], Ok(Token::End)));
    }
    #[test]
    fn rejects_missing_fdt_end() {
        let mut b = minimal();
        b[72..76].copy_from_slice(&4u32.to_be_bytes());
        assert_eq!(Fdt::parse(&b).err(), Some(FdtError::MissingEnd));
    }

    #[test]
    fn rejects_data_after_fdt_end() {
        let mut b = [0u8; 80];
        b[..76].copy_from_slice(&minimal());
        b[4..8].copy_from_slice(&80u32.to_be_bytes());
        b[36..40].copy_from_slice(&20u32.to_be_bytes());
        b[76..80].copy_from_slice(&4u32.to_be_bytes());
        assert_eq!(Fdt::parse(&b).err(), Some(FdtError::BadToken));
    }

    #[test]
    fn rejects_unterminated_reserve_map() {
        let mut b = minimal();
        b[40..48].copy_from_slice(&1u64.to_be_bytes());
        assert_eq!(Fdt::parse(&b).err(), Some(FdtError::BadReserveMap));
    }

    #[test]
    fn rejects_end_without_root_node() {
        let mut b = minimal();
        b[60..64].copy_from_slice(&9u32.to_be_bytes());
        assert_eq!(Fdt::parse(&b).err(), Some(FdtError::BadRoot));
    }

    #[test]
    fn rejects_nonempty_root_node_name() {
        let mut b = minimal();
        b[64] = b'x';
        assert_eq!(Fdt::parse(&b).err(), Some(FdtError::BadRoot));
    }

    #[test]
    fn rejects_multiple_root_nodes() {
        let original = minimal();
        let mut b = [0u8; 88];
        b[..original.len()].copy_from_slice(&original);
        b[4..8].copy_from_slice(&88u32.to_be_bytes());
        b[36..40].copy_from_slice(&28u32.to_be_bytes());
        b[72..76].copy_from_slice(&1u32.to_be_bytes());
        b[76..80].copy_from_slice(&0u32.to_be_bytes());
        b[80..84].copy_from_slice(&2u32.to_be_bytes());
        b[84..88].copy_from_slice(&9u32.to_be_bytes());
        assert_eq!(Fdt::parse(&b).err(), Some(FdtError::BadNesting));
    }

}
