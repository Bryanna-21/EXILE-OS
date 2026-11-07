//! Multiboot v1 handoff validation and memory-map ingestion.
//!
//! Pure and bounds-checked: callers pass byte slices they obtained safely. Policy is the same
//! fail-closed policy as `ExileKernel/src/memory.rs` (overlapping map entries are rejected).
//! That module is private to a binary crate and has no reserved-range subtraction, which
//! the frame allocator needs, so the parsing is reimplemented here rather than imported.

pub const BOOT_MAGIC: u32 = 0x2BAD_B002;
pub const FLAG_MMAP: u32 = 1 << 6;
pub const INFO_MIN_BYTES: usize = 52;
pub const MMAP_MAX_BYTES: usize = 65536;
pub const KIND_USABLE: u32 = 1;
pub const PAGE: u64 = 4096;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MbError {
    Magic,
    InfoTruncated,
    MapMissing,
    MapBounds,
    Truncated,
    RecordSize,
    Overflow,
    Empty,
    TooMany,
    Overlap,
    /// Module table unreadable, too long, or inconsistent.
    Modules,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Info {
    pub flags: u32,
    pub mmap_length: u32,
    pub mmap_addr: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub kind: u32,
}

pub fn check_magic(magic: u32) -> Result<(), MbError> {
    if magic == BOOT_MAGIC {
        Ok(())
    } else {
        Err(MbError::Magic)
    }
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn u64_at(b: &[u8], i: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(a)
}

pub fn parse_info(bytes: &[u8]) -> Result<Info, MbError> {
    if bytes.len() < INFO_MIN_BYTES {
        return Err(MbError::InfoTruncated);
    }
    let flags = u32_at(bytes, 0);
    if flags & FLAG_MMAP == 0 {
        return Err(MbError::MapMissing);
    }
    Ok(Info {
        flags,
        mmap_length: u32_at(bytes, 44),
        mmap_addr: u32_at(bytes, 48),
    })
}

/// The loader-provided map must lie inside `[4096, limit)` and be 1..=64 KiB long.
pub fn validate_map_location(info: &Info, limit: u64) -> Result<(u64, usize), MbError> {
    let len = info.mmap_length as usize;
    let addr = info.mmap_addr as u64;
    if len == 0 || len > MMAP_MAX_BYTES || addr < PAGE {
        return Err(MbError::MapBounds);
    }
    match addr.checked_add(len as u64) {
        Some(end) if end <= limit => Ok((addr, len)),
        _ => Err(MbError::MapBounds),
    }
}

pub fn parse_map(bytes: &[u8], out: &mut [Region]) -> Result<usize, MbError> {
    let mut offset = 0usize;
    let mut count = 0usize;
    while offset < bytes.len() {
        if bytes.len() - offset < 4 {
            return Err(MbError::Truncated);
        }
        let size = u32_at(bytes, offset) as usize;
        if size < 20 {
            return Err(MbError::RecordSize);
        }
        let next = offset
            .checked_add(4)
            .and_then(|x| x.checked_add(size))
            .ok_or(MbError::Overflow)?;
        if next > bytes.len() {
            return Err(MbError::Truncated);
        }
        let start = u64_at(bytes, offset + 4);
        let length = u64_at(bytes, offset + 12);
        let kind = u32_at(bytes, offset + 20);
        if length == 0 {
            return Err(MbError::Empty);
        }
        let end = start.checked_add(length).ok_or(MbError::Overflow)?;
        let region = Region { start, end, kind };
        if count == out.len() {
            return Err(MbError::TooMany);
        }
        if out[..count]
            .iter()
            .any(|r| region.start < r.end && r.start < region.end)
        {
            return Err(MbError::Overlap);
        }
        out[count] = region;
        count += 1;
        offset = next;
    }
    if count == 0 {
        return Err(MbError::Empty);
    }
    Ok(count)
}

const fn align_up(v: u64) -> Option<u64> {
    match v.checked_add(PAGE - 1) {
        Some(x) => Some(x & !(PAGE - 1)),
        None => None,
    }
}

/// Usable RAM as page-aligned `(start, end)` ranges below `ceiling`, with every `reserved`
/// range removed (reserved ranges are widened outward to page boundaries, usable ranges
/// narrowed inward). Fails closed with `TooMany` if `out` or the scratch list overflows.
pub fn usable_ranges(
    regions: &[Region],
    reserved: &[(u64, u64)],
    ceiling: u64,
    out: &mut [(u64, u64)],
) -> Result<usize, MbError> {
    const SCRATCH: usize = 64;
    let mut count = 0usize;
    for r in regions.iter().filter(|r| r.kind == KIND_USABLE) {
        let start = match align_up(r.start) {
            Some(s) => s,
            None => continue,
        };
        let end = r.end.min(ceiling) & !(PAGE - 1);
        if start >= end {
            continue;
        }
        let mut pieces = [(0u64, 0u64); SCRATCH];
        pieces[0] = (start, end);
        let mut n = 1usize;
        for &(rs, re) in reserved {
            if rs >= re {
                continue;
            }
            let rs = rs & !(PAGE - 1);
            let re = align_up(re).ok_or(MbError::Overflow)?;
            let mut next = [(0u64, 0u64); SCRATCH];
            let mut m = 0usize;
            for &(ps, pe) in &pieces[..n] {
                if re <= ps || rs >= pe {
                    if m == SCRATCH {
                        return Err(MbError::TooMany);
                    }
                    next[m] = (ps, pe);
                    m += 1;
                    continue;
                }
                if ps < rs {
                    if m == SCRATCH {
                        return Err(MbError::TooMany);
                    }
                    next[m] = (ps, rs);
                    m += 1;
                }
                if re < pe {
                    if m == SCRATCH {
                        return Err(MbError::TooMany);
                    }
                    next[m] = (re, pe);
                    m += 1;
                }
            }
            pieces = next;
            n = m;
        }
        for &p in &pieces[..n] {
            if count == out.len() {
                return Err(MbError::TooMany);
            }
            out[count] = p;
            count += 1;
        }
    }
    Ok(count)
}

/// Bytes of the Multiboot information structure the kernel reads (the loader's is larger).
pub const INFO_BYTES: usize = 88;
/// Strings referenced by the loader (command line, loader name, module names) have unknown
/// length; this many bytes after each pointer are reserved so the allocator never reuses them.
pub const STRING_RESERVE: u64 = 4096;
pub const MAX_MODULES: u32 = 16;

fn push_range(out: &mut [(u64, u64)], n: &mut usize, start: u64, len: u64) -> Result<(), MbError> {
    let end = start.checked_add(len).ok_or(MbError::Overflow)?;
    if *n == out.len() {
        return Err(MbError::TooMany);
    }
    out[*n] = (start, end);
    *n += 1;
    Ok(())
}

/// Every physical range the loader handed over that must not be given to the allocator:
/// the info structure, the memory map, command line, loader name, the module table, each
/// module image and each module string. `read_u32` reads loader memory (the boot code backs
/// it with bounds-checked reads; tests back it with a simulated memory).
pub fn reserved_ranges<F: Fn(u64) -> Option<u32>>(
    info_addr: u64,
    info: &[u8],
    read_u32: F,
    out: &mut [(u64, u64)],
) -> Result<usize, MbError> {
    if info.len() < INFO_BYTES {
        return Err(MbError::InfoTruncated);
    }
    let flags = u32_at(info, 0);
    let mut n = 0usize;
    push_range(out, &mut n, info_addr, INFO_BYTES as u64)?;
    if flags & FLAG_MMAP != 0 {
        push_range(
            out,
            &mut n,
            u32_at(info, 48) as u64,
            u32_at(info, 44) as u64,
        )?;
    }
    if flags & (1 << 2) != 0 && u32_at(info, 16) != 0 {
        push_range(out, &mut n, u32_at(info, 16) as u64, STRING_RESERVE)?;
    }
    if flags & (1 << 9) != 0 && u32_at(info, 64) != 0 {
        push_range(out, &mut n, u32_at(info, 64) as u64, STRING_RESERVE)?;
    }
    if flags & (1 << 3) != 0 {
        let count = u32_at(info, 20);
        let table = u32_at(info, 24) as u64;
        if count > MAX_MODULES {
            return Err(MbError::Modules);
        }
        if count > 0 {
            push_range(out, &mut n, table, 16 * count as u64)?;
        }
        for i in 0..count as u64 {
            let base = table + 16 * i;
            let start = read_u32(base).ok_or(MbError::Modules)? as u64;
            let end = read_u32(base + 4).ok_or(MbError::Modules)? as u64;
            let name = read_u32(base + 8).ok_or(MbError::Modules)? as u64;
            if end < start {
                return Err(MbError::Modules);
            }
            if end > start {
                push_range(out, &mut n, start, end - start)?;
            }
            if name != 0 {
                push_range(out, &mut n, name, STRING_RESERVE)?;
            }
        }
    }
    Ok(n)
}
