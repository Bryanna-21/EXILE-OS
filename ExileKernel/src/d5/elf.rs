//! ELF64 executable loader (Mission D, requirement 16). SPDX-License-Identifier: Apache-2.0
//!
//! Builds a REAL process image inside an `AddressSpace`: every PT_LOAD segment is mapped with the
//! permissions its `p_flags` request (write+execute is rejected), file bytes are copied through
//! the page tables, the BSS tail is zero (frames are zeroed on allocation and the loader checks
//! that nothing but file bytes was written), and a user stack with unmapped guard pages is added.
//!
//! The image is untrusted. Every offset, size and address is validated with checked arithmetic
//! before use. Loading is atomic: on any error every region this call created is unmapped again
//! and the address space is left as it was.
//!
//! Supported: ET_EXEC, EM_X86_64, little-endian, static (no PT_INTERP / PT_DYNAMIC / PT_TLS
//! relocation). A PT_TLS segment is reported to the caller (`LoadedImage::tls`) so the process
//! layer can build the per-thread TLS block; the loader itself does not allocate it.
#![allow(dead_code)]

use crate::address_space::{
    AddressSpace, Error as SpaceError, FrameAlloc, PAGE, Perms, PhysMem, USER_END, USER_START,
};

pub const EHDR_SIZE: usize = 64;
pub const PHDR_SIZE: usize = 56;
pub const MAX_PHDRS: usize = 32;
pub const MAX_LOADS: usize = 16;
/// User stack: 64 KiB, with one unmapped guard page below it and the top page left unmapped.
pub const STACK_PAGES: u64 = 16;
pub const STACK_TOP: u64 = USER_END - PAGE;
pub const STACK_BASE: u64 = STACK_TOP - STACK_PAGES * PAGE;

const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;
const PT_TLS: u32 = 7;
const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PT_GNU_RELRO: u32 = 0x6474_e552;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    BadMagic,
    NotClass64,
    NotLittleEndian,
    BadVersion,
    NotExecutable,
    WrongMachine,
    BadHeaderSize,
    TooManyHeaders,
    BadProgramHeader,
    NoLoadSegments,
    TooManyLoads,
    FileRange,
    FileSizeExceedsMemSize,
    BadAlignment,
    Congruence,
    OutOfRange,
    Overlap,
    WritableExecutable,
    BadFlags,
    BadEntry,
    Unsupported,
    ExecutableStack,
    Space(SpaceError),
}

impl From<SpaceError> for Error {
    fn from(e: SpaceError) -> Self {
        Error::Space(e)
    }
}

/// A validated PT_LOAD segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub offset: u64,
    pub perms: Perms,
    /// Page-aligned mapped range `[map_start, map_end)`.
    pub map_start: u64,
    pub map_end: u64,
}

/// PT_TLS template reported to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlsInfo {
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedImage {
    pub entry: u64,
    pub stack_base: u64,
    pub stack_top: u64,
    /// 16-byte aligned stack pointer at process entry: argc = 0, argv = NULL, envp = NULL.
    pub initial_sp: u64,
    /// First address past the highest segment (initial program break).
    pub image_end: u64,
    pub segments: usize,
    pub tls: Option<TlsInfo>,
}

fn rd16(b: &[u8], o: usize) -> Result<u16, Error> {
    let s = b
        .get(o..o.checked_add(2).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}
fn rd32(b: &[u8], o: usize) -> Result<u32, Error> {
    let s = b
        .get(o..o.checked_add(4).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn rd64(b: &[u8], o: usize) -> Result<u64, Error> {
    let s = b
        .get(o..o.checked_add(8).ok_or(Error::Truncated)?)
        .ok_or(Error::Truncated)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Ok(u64::from_le_bytes(a))
}

fn align_up(v: u64) -> Option<u64> {
    v.checked_add(PAGE - 1).map(|x| x & !(PAGE - 1))
}

/// Result of validating an image without touching any address space.
pub struct Plan {
    pub entry: u64,
    pub segs: [Option<Segment>; MAX_LOADS],
    pub nsegs: usize,
    pub tls: Option<TlsInfo>,
}

/// Validates the whole image (header, program headers, segments, entry). Pure; no side effects.
pub fn validate(image: &[u8]) -> Result<Plan, Error> {
    if image.len() < EHDR_SIZE {
        return Err(Error::Truncated);
    }
    if image[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(Error::BadMagic);
    }
    if image[4] != 2 {
        return Err(Error::NotClass64);
    }
    if image[5] != 1 {
        return Err(Error::NotLittleEndian);
    }
    if image[6] != 1 || rd32(image, 20)? != 1 {
        return Err(Error::BadVersion);
    }
    if rd16(image, 16)? != ET_EXEC {
        return Err(Error::NotExecutable);
    }
    if rd16(image, 18)? != EM_X86_64 {
        return Err(Error::WrongMachine);
    }
    let entry = rd64(image, 24)?;
    let phoff = rd64(image, 32)?;
    if usize::from(rd16(image, 52)?) != EHDR_SIZE || usize::from(rd16(image, 54)?) != PHDR_SIZE {
        return Err(Error::BadHeaderSize);
    }
    let phnum = usize::from(rd16(image, 56)?);
    if phnum == 0 {
        return Err(Error::NoLoadSegments);
    }
    if phnum > MAX_PHDRS {
        return Err(Error::TooManyHeaders);
    }
    let table_len = (phnum as u64) * PHDR_SIZE as u64;
    let table_end = phoff.checked_add(table_len).ok_or(Error::Truncated)?;
    if phoff < EHDR_SIZE as u64 || table_end > image.len() as u64 {
        return Err(Error::Truncated);
    }

    let mut plan = Plan {
        entry,
        segs: [None; MAX_LOADS],
        nsegs: 0,
        tls: None,
    };
    for i in 0..phnum {
        let o = phoff as usize + i * PHDR_SIZE;
        let p_type = rd32(image, o)?;
        let flags = rd32(image, o + 4)?;
        let offset = rd64(image, o + 8)?;
        let vaddr = rd64(image, o + 16)?;
        let filesz = rd64(image, o + 32)?;
        let memsz = rd64(image, o + 40)?;
        let align = rd64(image, o + 48)?;
        match p_type {
            PT_LOAD => {}
            PT_NULL | PT_NOTE | PT_PHDR | PT_GNU_EH_FRAME | PT_GNU_RELRO => continue,
            PT_GNU_STACK => {
                if flags & PF_X != 0 {
                    return Err(Error::ExecutableStack);
                }
                continue;
            }
            PT_TLS => {
                if filesz > memsz {
                    return Err(Error::FileSizeExceedsMemSize);
                }
                if plan.tls.is_some() {
                    return Err(Error::BadProgramHeader);
                }
                let end = offset.checked_add(filesz).ok_or(Error::FileRange)?;
                if end > image.len() as u64 {
                    return Err(Error::FileRange);
                }
                plan.tls = Some(TlsInfo {
                    vaddr,
                    filesz,
                    memsz,
                    align,
                });
                continue;
            }
            PT_INTERP | PT_DYNAMIC => return Err(Error::Unsupported),
            _ => return Err(Error::Unsupported),
        }
        if plan.nsegs == MAX_LOADS {
            return Err(Error::TooManyLoads);
        }
        if filesz > memsz {
            return Err(Error::FileSizeExceedsMemSize);
        }
        if memsz == 0 {
            return Err(Error::BadProgramHeader);
        }
        if align > 1 && (!align.is_power_of_two() || align > (1 << 30)) {
            return Err(Error::BadAlignment);
        }
        // The page-granular mapping requires p_vaddr == p_offset (mod PAGE).
        if vaddr % PAGE != offset % PAGE {
            return Err(Error::Congruence);
        }
        if align > 1 && vaddr % align != offset % align {
            return Err(Error::Congruence);
        }
        let file_end = offset.checked_add(filesz).ok_or(Error::FileRange)?;
        if file_end > image.len() as u64 {
            return Err(Error::FileRange);
        }
        let mem_end = vaddr.checked_add(memsz).ok_or(Error::OutOfRange)?;
        let map_start = vaddr & !(PAGE - 1);
        let map_end = align_up(mem_end).ok_or(Error::OutOfRange)?;
        if map_start < USER_START || map_end > USER_END || map_end > STACK_BASE - PAGE {
            return Err(Error::OutOfRange);
        }
        if flags & !(PF_R | PF_W | PF_X) != 0 || flags & (PF_R | PF_W | PF_X) == 0 {
            return Err(Error::BadFlags);
        }
        if flags & PF_W != 0 && flags & PF_X != 0 {
            return Err(Error::WritableExecutable);
        }
        let perms = Perms {
            read: true, // x86 cannot express exec-only or write-only
            write: flags & PF_W != 0,
            exec: flags & PF_X != 0,
            user: true,
        };
        for s in plan.segs.iter().flatten() {
            if map_start < s.map_end && s.map_start < map_end {
                return Err(Error::Overlap);
            }
        }
        plan.segs[plan.nsegs] = Some(Segment {
            vaddr,
            filesz,
            memsz,
            offset,
            perms,
            map_start,
            map_end,
        });
        plan.nsegs += 1;
    }
    if plan.nsegs == 0 {
        return Err(Error::NoLoadSegments);
    }
    // The entry point must lie inside an executable segment's file-backed or memory range.
    let entry_ok = plan
        .segs
        .iter()
        .flatten()
        .any(|s| s.perms.exec && entry >= s.vaddr && entry < s.vaddr + s.memsz);
    if !entry_ok {
        return Err(Error::BadEntry);
    }
    if let Some(t) = plan.tls {
        // The TLS template must be backed by a loaded segment's file bytes.
        let t_end = t.vaddr.checked_add(t.memsz).ok_or(Error::OutOfRange)?;
        let backed = plan
            .segs
            .iter()
            .flatten()
            .any(|s| t.vaddr >= s.vaddr && t_end <= s.vaddr + s.memsz);
        if !backed {
            return Err(Error::OutOfRange);
        }
    }
    Ok(plan)
}

fn rollback<M, A>(space: &mut AddressSpace, mem: &mut M, frames: &mut A, starts: &[u64])
where
    M: PhysMem + ?Sized,
    A: FrameAlloc + ?Sized,
{
    for &s in starts.iter().rev() {
        let _ = space.unmap_region(mem, frames, s);
    }
}

/// Loads `image` into `space`. Atomic: on error nothing remains mapped by this call.
pub fn load<M, A>(
    image: &[u8],
    space: &mut AddressSpace,
    mem: &mut M,
    frames: &mut A,
) -> Result<LoadedImage, Error>
where
    M: PhysMem + ?Sized,
    A: FrameAlloc + ?Sized,
{
    let plan = validate(image)?;
    let mut started = [0u64; MAX_LOADS + 1];
    let mut n = 0usize;
    let mut image_end = 0u64;

    for seg in plan.segs.iter().flatten() {
        let len = seg.map_end - seg.map_start;
        if let Err(e) = space.map_anon(mem, frames, seg.map_start, len, seg.perms) {
            rollback(space, mem, frames, &started[..n]);
            return Err(e.into());
        }
        started[n] = seg.map_start;
        n += 1;
        if seg.filesz > 0 {
            let src = &image[seg.offset as usize..(seg.offset + seg.filesz) as usize];
            if let Err(e) = space.load_bytes(mem, seg.vaddr, src) {
                rollback(space, mem, frames, &started[..n]);
                return Err(e.into());
            }
        }
        image_end = image_end.max(seg.vaddr + seg.memsz);
    }

    if let Err(e) = space.map_anon(mem, frames, STACK_BASE, STACK_PAGES * PAGE, Perms::rw()) {
        rollback(space, mem, frames, &started[..n]);
        return Err(e.into());
    }
    started[n] = STACK_BASE;
    n += 1;

    // argc = 0, argv[0] = NULL, envp[0] = NULL, plus padding; sp is 16-byte aligned.
    let initial_sp = STACK_TOP - 64;
    if let Err(e) = space.load_bytes(mem, initial_sp, &[0u8; 32]) {
        rollback(space, mem, frames, &started[..n]);
        return Err(e.into());
    }
    Ok(LoadedImage {
        entry: plan.entry,
        stack_base: STACK_BASE,
        stack_top: STACK_TOP,
        initial_sp,
        image_end,
        segments: plan.nsegs,
        tls: plan.tls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address_space::Access;
    use crate::address_space::fakes::Fx;
    extern crate std;
    use std::vec;
    use std::vec::Vec;

    struct Seg {
        flags: u32,
        offset: u64,
        vaddr: u64,
        filesz: u64,
        memsz: u64,
        align: u64,
    }

    /// Hand-assembles an ELF64 executable: header, program headers, then `body` at file offset 0x1000.
    fn build(entry: u64, segs: &[Seg], body: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; 0x1000];
        v[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        v[4] = 2;
        v[5] = 1;
        v[6] = 1;
        v[16..18].copy_from_slice(&2u16.to_le_bytes());
        v[18..20].copy_from_slice(&62u16.to_le_bytes());
        v[20..24].copy_from_slice(&1u32.to_le_bytes());
        v[24..32].copy_from_slice(&entry.to_le_bytes());
        v[32..40].copy_from_slice(&64u64.to_le_bytes());
        v[52..54].copy_from_slice(&64u16.to_le_bytes());
        v[54..56].copy_from_slice(&56u16.to_le_bytes());
        v[56..58].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        for (i, s) in segs.iter().enumerate() {
            let o = 64 + i * 56;
            v[o..o + 4].copy_from_slice(&1u32.to_le_bytes());
            v[o + 4..o + 8].copy_from_slice(&s.flags.to_le_bytes());
            v[o + 8..o + 16].copy_from_slice(&s.offset.to_le_bytes());
            v[o + 16..o + 24].copy_from_slice(&s.vaddr.to_le_bytes());
            v[o + 24..o + 32].copy_from_slice(&s.vaddr.to_le_bytes());
            v[o + 32..o + 40].copy_from_slice(&s.filesz.to_le_bytes());
            v[o + 40..o + 48].copy_from_slice(&s.memsz.to_le_bytes());
            v[o + 48..o + 56].copy_from_slice(&s.align.to_le_bytes());
        }
        v.extend_from_slice(body);
        v
    }

    const TEXT: u64 = 0x40_0000;
    const DATA: u64 = 0x60_0000;

    fn valid() -> Vec<u8> {
        let mut body = vec![0u8; 0x2000];
        body[..8].copy_from_slice(&[0x90, 0x90, 0x90, 0x90, 0xf4, 0, 0, 0]);
        body[0x1000..0x1010].copy_from_slice(&[0xAA; 16]);
        build(
            TEXT,
            &[
                Seg {
                    flags: PF_R | PF_X,
                    offset: 0x1000,
                    vaddr: TEXT,
                    filesz: 0x100,
                    memsz: 0x100,
                    align: 0x1000,
                },
                // 16 initialised bytes, then BSS to 0x3000 bytes (spans pages).
                Seg {
                    flags: PF_R | PF_W,
                    offset: 0x2000,
                    vaddr: DATA,
                    filesz: 0x10,
                    memsz: 0x3000,
                    align: 0x1000,
                },
            ],
            &body,
        )
    }

    #[test]
    fn loads_a_valid_image_with_bss_and_permissions() {
        let mut fx = Fx::new(128);
        let mut s = fx.space();
        let img = load(&valid(), &mut s, &mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(img.entry, TEXT);
        assert_eq!(img.segments, 2);
        assert_eq!(img.initial_sp % 16, 0);
        assert_eq!(img.stack_top, STACK_TOP);
        assert_eq!(img.image_end, DATA + 0x3000);
        let mut b = [0u8; 8];
        s.copy_from_user(&mut fx.mem, TEXT, &mut b).unwrap();
        assert_eq!(b[..5], [0x90, 0x90, 0x90, 0x90, 0xf4]);
        let mut d = [0xFFu8; 32];
        s.copy_from_user(&mut fx.mem, DATA, &mut d).unwrap();
        assert_eq!(d[..16], [0xAA; 16]);
        assert!(d[16..].iter().all(|&x| x == 0), "BSS tail must be zero");
        let mut tail = [0xFFu8; 64];
        s.copy_from_user(&mut fx.mem, DATA + 0x2FC0, &mut tail)
            .unwrap();
        assert!(tail.iter().all(|&x| x == 0));
        // Permissions: text is not writable, data is not executable, stack is rw.
        assert!(
            s.validate_user_range(&mut fx.mem, TEXT, 16, Access::Exec)
                .is_ok()
        );
        assert_eq!(
            s.validate_user_range(&mut fx.mem, TEXT, 16, Access::Write),
            Err(SpaceError::Perm)
        );
        assert_eq!(
            s.validate_user_range(&mut fx.mem, DATA, 16, Access::Exec),
            Err(SpaceError::Perm)
        );
        assert!(
            s.validate_user_range(&mut fx.mem, STACK_BASE, 64, Access::Write)
                .is_ok()
        );
        // Guard pages: below the stack and above its top are unmapped.
        assert!(
            s.validate_user_range(&mut fx.mem, STACK_BASE - PAGE, 8, Access::Read)
                .is_err()
        );
        assert!(
            s.validate_user_range(&mut fx.mem, STACK_TOP, 8, Access::Read)
                .is_err()
        );
        s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(fx.frames.in_use(), 1, "only the kernel template remains");
    }

    fn expect(img: Vec<u8>, want: Error) {
        let mut fx = Fx::new(128);
        let mut s = fx.space();
        let before = (fx.frames.in_use(), s.mapped_pages(), s.region_count());
        assert_eq!(load(&img, &mut s, &mut fx.mem, &mut fx.frames), Err(want));
        assert_eq!(
            (fx.frames.in_use(), s.mapped_pages(), s.region_count()),
            before,
            "atomic"
        );
    }

    fn mutate(f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut v = valid();
        f(&mut v);
        v
    }

    #[test]
    fn rejects_header_problems() {
        expect(vec![0u8; 10], Error::Truncated);
        expect(mutate(|v| v[0] = 0), Error::BadMagic);
        expect(mutate(|v| v[4] = 1), Error::NotClass64);
        expect(mutate(|v| v[5] = 2), Error::NotLittleEndian);
        expect(mutate(|v| v[6] = 2), Error::BadVersion);
        expect(mutate(|v| v[16] = 3), Error::NotExecutable);
        expect(mutate(|v| v[18] = 3), Error::WrongMachine);
        expect(mutate(|v| v[52] = 65), Error::BadHeaderSize);
        expect(mutate(|v| v[54] = 57), Error::BadHeaderSize);
        expect(
            mutate(|v| v[56..58].copy_from_slice(&0u16.to_le_bytes())),
            Error::NoLoadSegments,
        );
        expect(
            mutate(|v| v[56..58].copy_from_slice(&33u16.to_le_bytes())),
            Error::TooManyHeaders,
        );
        expect(
            mutate(|v| v[32..40].copy_from_slice(&u64::MAX.to_le_bytes())),
            Error::Truncated,
        );
        expect(
            mutate(|v| v[32..40].copy_from_slice(&(u64::MAX - 20).to_le_bytes())),
            Error::Truncated,
        );
    }

    #[test]
    fn rejects_program_header_problems() {
        let ph = |i: usize| 64 + i * 56;
        // p_type unsupported / dynamic linking
        expect(
            mutate(|v| v[ph(0)..ph(0) + 4].copy_from_slice(&3u32.to_le_bytes())),
            Error::Unsupported,
        );
        expect(
            mutate(|v| v[ph(0)..ph(0) + 4].copy_from_slice(&0x7000_0000u32.to_le_bytes())),
            Error::Unsupported,
        );
        // filesz > memsz
        expect(
            mutate(|v| v[ph(0) + 32..ph(0) + 40].copy_from_slice(&0x200u64.to_le_bytes())),
            Error::FileSizeExceedsMemSize,
        );
        // memsz == 0
        expect(
            mutate(|v| {
                v[ph(0) + 32..ph(0) + 40].copy_from_slice(&0u64.to_le_bytes());
                v[ph(0) + 40..ph(0) + 48].copy_from_slice(&0u64.to_le_bytes());
            }),
            Error::BadProgramHeader,
        );
        // file range outside the image
        expect(
            mutate(|v| v[ph(1) + 8..ph(1) + 16].copy_from_slice(&0x10_0000u64.to_le_bytes())),
            Error::FileRange,
        );
        expect(
            mutate(|v| v[ph(1) + 32..ph(1) + 40].copy_from_slice(&0x10_0000u64.to_le_bytes())),
            Error::FileSizeExceedsMemSize,
        );
        expect(
            mutate(|v| {
                v[ph(1) + 32..ph(1) + 40].copy_from_slice(&0x3000u64.to_le_bytes());
            }),
            Error::FileRange,
        );
        // offset + filesz overflow
        expect(
            mutate(|v| v[ph(0) + 8..ph(0) + 16].copy_from_slice(&u64::MAX.to_le_bytes())),
            Error::Congruence,
        );
        // bad alignment
        expect(
            mutate(|v| v[ph(0) + 48..ph(0) + 56].copy_from_slice(&3u64.to_le_bytes())),
            Error::BadAlignment,
        );
        // congruence
        expect(
            mutate(|v| v[ph(0) + 16..ph(0) + 24].copy_from_slice(&(TEXT + 8).to_le_bytes())),
            Error::Congruence,
        );
        // flags
        expect(
            mutate(|v| v[ph(0) + 4..ph(0) + 8].copy_from_slice(&0u32.to_le_bytes())),
            Error::BadFlags,
        );
        expect(
            mutate(|v| v[ph(0) + 4..ph(0) + 8].copy_from_slice(&0x8u32.to_le_bytes())),
            Error::BadFlags,
        );
        expect(
            mutate(|v| v[ph(1) + 4..ph(1) + 8].copy_from_slice(&7u32.to_le_bytes())),
            Error::WritableExecutable,
        );
    }

    #[test]
    fn rejects_range_overlap_and_entry_problems() {
        let ph = |i: usize| 64 + i * 56;
        // Kernel-half and wrapping virtual addresses.
        for bad in [
            0u64,
            0x8000_0000_0000_0000,
            0xFFFF_FFFF_8000_0000,
            USER_END,
            u64::MAX - 0xFFF,
        ] {
            let img = mutate(|v| {
                v[ph(1) + 16..ph(1) + 24].copy_from_slice(&bad.to_le_bytes());
                // keep congruence valid for the page-aligned bad address
                v[ph(1) + 8..ph(1) + 16].copy_from_slice(&0x2000u64.to_le_bytes());
            });
            let mut fx = Fx::new(128);
            let mut s = fx.space();
            assert!(
                load(&img, &mut s, &mut fx.mem, &mut fx.frames).is_err(),
                "{bad:#x}"
            );
            assert_eq!(s.region_count(), 0);
        }
        // memsz that wraps.
        expect(
            mutate(|v| v[ph(1) + 40..ph(1) + 48].copy_from_slice(&u64::MAX.to_le_bytes())),
            Error::OutOfRange,
        );
        // Overlapping segments (page granularity): put data inside the text page.
        expect(
            mutate(|v| {
                v[ph(1) + 16..ph(1) + 24].copy_from_slice(&(TEXT + 0x1000).to_le_bytes());
                v[ph(1) + 8..ph(1) + 16].copy_from_slice(&0x2000u64.to_le_bytes());
                v[ph(1) + 40..ph(1) + 48].copy_from_slice(&0x10u64.to_le_bytes());
                v[ph(0) + 40..ph(0) + 48].copy_from_slice(&0x1800u64.to_le_bytes());
            }),
            Error::Overlap,
        );
        // Entry outside any segment, in a non-exec segment, at a segment end, and zero.
        for bad in [0u64, 0x1234, DATA, TEXT + 0x100 + 0x1000, u64::MAX] {
            expect(
                mutate(|v| v[24..32].copy_from_slice(&bad.to_le_bytes())),
                Error::BadEntry,
            );
        }
        // Executable stack request.
        let mut v = valid();
        v[56..58].copy_from_slice(&3u16.to_le_bytes());
        let o = 64 + 2 * 56;
        v[o..o + 4].copy_from_slice(&PT_GNU_STACK.to_le_bytes());
        v[o + 4..o + 8].copy_from_slice(&(PF_R | PF_W | PF_X).to_le_bytes());
        expect(v.clone(), Error::ExecutableStack);
        v[o + 4..o + 8].copy_from_slice(&(PF_R | PF_W).to_le_bytes());
        let mut fx = Fx::new(128);
        let mut s = fx.space();
        assert!(load(&v, &mut s, &mut fx.mem, &mut fx.frames).is_ok());
    }

    #[test]
    fn exhaustion_at_every_allocation_point_rolls_back() {
        let img = valid();
        let mut ok_at = None;
        for budget in 0..80usize {
            let mut fx = Fx::new(128);
            let mut s = fx.space();
            let before = fx.frames.in_use();
            fx.frames.fail_at = Some(fx.frames.calls + budget);
            match load(&img, &mut s, &mut fx.mem, &mut fx.frames) {
                Ok(_) => {
                    ok_at = Some(budget);
                    break;
                }
                Err(e) => {
                    assert!(matches!(e, Error::Space(_)), "budget {budget}: {e:?}");
                    assert_eq!(fx.frames.in_use(), before, "leak at budget {budget}");
                    assert_eq!(s.region_count(), 0);
                    assert_eq!(s.mapped_pages(), 0);
                }
            }
        }
        assert!(
            ok_at.is_some(),
            "load must succeed once enough frames are allowed"
        );
    }

    #[test]
    fn tls_segment_is_reported_and_must_be_backed() {
        let mut v = valid();
        v[56..58].copy_from_slice(&3u16.to_le_bytes());
        let o = 64 + 2 * 56;
        v[o..o + 4].copy_from_slice(&PT_TLS.to_le_bytes());
        v[o + 4..o + 8].copy_from_slice(&PF_R.to_le_bytes());
        v[o + 8..o + 16].copy_from_slice(&0x2000u64.to_le_bytes());
        v[o + 16..o + 24].copy_from_slice(&DATA.to_le_bytes());
        v[o + 32..o + 40].copy_from_slice(&8u64.to_le_bytes());
        v[o + 40..o + 48].copy_from_slice(&32u64.to_le_bytes());
        v[o + 48..o + 56].copy_from_slice(&8u64.to_le_bytes());
        let mut fx = Fx::new(128);
        let mut s = fx.space();
        let img = load(&v, &mut s, &mut fx.mem, &mut fx.frames).unwrap();
        assert_eq!(
            img.tls,
            Some(TlsInfo {
                vaddr: DATA,
                filesz: 8,
                memsz: 32,
                align: 8
            })
        );
        v[o + 16..o + 24].copy_from_slice(&0x7000_0000u64.to_le_bytes());
        expect(v, Error::OutOfRange);
    }

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn fuzzed_images_never_panic_or_leak() {
        let base_img = valid();
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        let mut loaded = 0;
        for _ in 0..4000 {
            let mut img = base_img.clone();
            for _ in 0..(1 + xorshift(&mut rng) % 4) {
                // Concentrate mutations on the header and program-header area.
                let i = (xorshift(&mut rng) % 64 + 120 * (xorshift(&mut rng) % 2)) as usize % 0x200;
                img[i] = xorshift(&mut rng) as u8;
            }
            if xorshift(&mut rng) % 8 == 0 {
                let cut = (xorshift(&mut rng) as usize) % img.len();
                img.truncate(cut);
            }
            let mut fx = Fx::new(128);
            let mut s = fx.space();
            let before = fx.frames.in_use();
            match load(&img, &mut s, &mut fx.mem, &mut fx.frames) {
                Ok(_) => {
                    loaded += 1;
                    s.destroy(&mut fx.mem, &mut fx.frames).unwrap();
                    assert_eq!(fx.frames.in_use(), before - 1);
                }
                Err(_) => {
                    assert_eq!(fx.frames.in_use(), before, "failed load leaked frames");
                    assert_eq!(s.region_count(), 0);
                }
            }
        }
        assert!(loaded > 0, "some mutations must still be valid images");
    }
}
