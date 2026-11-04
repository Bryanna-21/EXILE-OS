//! Multiboot handoff and memory-map tests, including negative cases.

use omni8::multiboot::*;

fn record(base: u64, len: u64, kind: u32) -> Vec<u8> {
    let mut v = vec![];
    v.extend_from_slice(&20u32.to_le_bytes());
    v.extend_from_slice(&base.to_le_bytes());
    v.extend_from_slice(&len.to_le_bytes());
    v.extend_from_slice(&kind.to_le_bytes());
    v
}

fn map(records: &[(u64, u64, u32)]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|&(b, l, k)| record(b, l, k))
        .collect()
}

fn info_bytes(flags: u32, len: u32, addr: u32) -> Vec<u8> {
    let mut v = vec![0u8; 88];
    v[0..4].copy_from_slice(&flags.to_le_bytes());
    v[44..48].copy_from_slice(&len.to_le_bytes());
    v[48..52].copy_from_slice(&addr.to_le_bytes());
    v
}

const R0: Region = Region {
    start: 0,
    end: 0,
    kind: 0,
};

#[test]
fn mb_01_magic() {
    assert_eq!(check_magic(0x2BADB002), Ok(()));
    assert_eq!(check_magic(0), Err(MbError::Magic));
    assert_eq!(
        check_magic(0x36D76289),
        Err(MbError::Magic),
        "Multiboot2 magic is not accepted"
    );
}

#[test]
fn mb_02_info_parsing() {
    let i = parse_info(&info_bytes(1 << 6, 48, 0x9000)).unwrap();
    assert_eq!((i.mmap_length, i.mmap_addr), (48, 0x9000));
    assert_eq!(
        parse_info(&info_bytes(0, 48, 0x9000)),
        Err(MbError::MapMissing)
    );
    assert_eq!(
        parse_info(&info_bytes(1 << 6, 48, 0x9000)[..51]),
        Err(MbError::InfoTruncated)
    );
    assert_eq!(parse_info(&[]), Err(MbError::InfoTruncated));
}

#[test]
fn mb_03_map_location_bounds() {
    let lim = 1 << 30;
    let ok = |len, addr| {
        validate_map_location(
            &Info {
                flags: 64,
                mmap_length: len,
                mmap_addr: addr,
            },
            lim,
        )
    };
    assert_eq!(ok(48, 0x9000), Ok((0x9000, 48)));
    assert_eq!(ok(0, 0x9000), Err(MbError::MapBounds));
    assert_eq!(ok(65537, 0x9000), Err(MbError::MapBounds));
    assert_eq!(ok(65536, 0x9000), Ok((0x9000, 65536)));
    assert_eq!(
        ok(48, 0xFFF),
        Err(MbError::MapBounds),
        "below the null page"
    );
    assert_eq!(ok(48, 0), Err(MbError::MapBounds));
    assert_eq!(
        ok(48, (1 << 30) - 47),
        Err(MbError::MapBounds),
        "runs past the mapped limit"
    );
    assert_eq!(
        ok(48, u32::MAX),
        Err(MbError::MapBounds),
        "address + length beyond the limit"
    );
}

#[test]
fn mb_04_parse_map_ok() {
    let m = map(&[
        (0, 0x9FC00, 1),
        (0x10_0000, 0x7EE_0000, 1),
        (0xFFFC_0000, 0x4_0000, 2),
    ]);
    let mut out = [R0; 8];
    let n = parse_map(&m, &mut out).unwrap();
    assert_eq!(n, 3);
    assert_eq!(
        out[1],
        Region {
            start: 0x10_0000,
            end: 0x10_0000 + 0x7EE_0000,
            kind: 1
        }
    );
}

#[test]
fn mb_05_parse_map_rejects_malformed_input() {
    let mut out = [R0; 8];
    assert_eq!(parse_map(&[], &mut out), Err(MbError::Empty));
    assert_eq!(parse_map(&[1, 2, 3], &mut out), Err(MbError::Truncated));
    let mut m = map(&[(0, 0x1000, 1)]);
    assert_eq!(parse_map(&m[..23], &mut out), Err(MbError::Truncated));
    m[0..4].copy_from_slice(&19u32.to_le_bytes());
    assert_eq!(parse_map(&m, &mut out), Err(MbError::RecordSize));
    let mut m = map(&[(0, 0x1000, 1)]);
    m[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(parse_map(&m, &mut out), Err(MbError::Truncated));
    assert_eq!(parse_map(&map(&[(0, 0, 1)]), &mut out), Err(MbError::Empty));
    assert_eq!(
        parse_map(&map(&[(u64::MAX - 5, 100, 1)]), &mut out),
        Err(MbError::Overflow)
    );
}

#[test]
fn mb_06_overlap_and_capacity() {
    let mut out = [R0; 8];
    assert_eq!(
        parse_map(&map(&[(0, 0x2000, 1), (0x1000, 0x2000, 2)]), &mut out),
        Err(MbError::Overlap)
    );
    assert_eq!(
        parse_map(&map(&[(0, 0x1000, 1), (0x1000, 0x1000, 1)]), &mut out),
        Ok(2),
        "touching is not overlapping"
    );
    let mut tiny = [R0; 1];
    assert_eq!(
        parse_map(&map(&[(0, 0x1000, 1), (0x1000, 0x1000, 1)]), &mut tiny),
        Err(MbError::TooMany)
    );
}

#[test]
fn mb_07_usable_ranges_subtract_reservations() {
    let regions = [
        Region {
            start: 0,
            end: 0x9_FC00,
            kind: 1,
        },
        Region {
            start: 0x10_0000,
            end: 0x800_0000,
            kind: 1,
        },
        Region {
            start: 0x9_FC00,
            end: 0x10_0000,
            kind: 2,
        },
    ];
    let mut out = [(0, 0); 16];
    // reserve the kernel image and a multiboot info block in the middle of RAM
    let n = usable_ranges(
        &regions,
        &[(0x10_0000, 0x15_2000), (0x20_0100, 0x20_0200)],
        1 << 30,
        &mut out,
    )
    .unwrap();
    assert_eq!(
        &out[..n],
        &[
            (0, 0x9_F000),
            (0x15_2000, 0x20_0000),
            (0x20_1000, 0x800_0000)
        ]
    );
}

#[test]
fn mb_08_usable_ranges_edge_cases() {
    let mut out = [(0, 0); 4];
    let r = [Region {
        start: 0x1001,
        end: 0x3FFF,
        kind: 1,
    }];
    assert_eq!(usable_ranges(&r, &[], 1 << 30, &mut out), Ok(1));
    assert_eq!(out[0], (0x2000, 0x3000), "narrowed inward");
    let r = [Region {
        start: 0x1000,
        end: 0x1800,
        kind: 1,
    }];
    assert_eq!(
        usable_ranges(&r, &[], 1 << 30, &mut out),
        Ok(0),
        "less than a page"
    );
    let r = [Region {
        start: 0x1000,
        end: 0x10_0000,
        kind: 2,
    }];
    assert_eq!(
        usable_ranges(&r, &[], 1 << 30, &mut out),
        Ok(0),
        "reserved type ignored"
    );
    // ceiling clips; reservation covering everything leaves nothing
    let r = [Region {
        start: 0x10_0000,
        end: 0x4000_0000_0000,
        kind: 1,
    }];
    assert_eq!(usable_ranges(&r, &[], 0x20_0000, &mut out), Ok(1));
    assert_eq!(out[0], (0x10_0000, 0x20_0000));
    assert_eq!(
        usable_ranges(&r, &[(0, u64::MAX - 0x1000)], 0x20_0000, &mut out),
        Ok(0)
    );
    // a reservation whose end cannot be page-aligned without overflow is an error, not a wrap
    assert_eq!(
        usable_ranges(&r, &[(0x10_0000, u64::MAX)], 0x20_0000, &mut out),
        Err(MbError::Overflow)
    );
    // empty or inverted reservation is ignored
    assert_eq!(
        usable_ranges(&r, &[(5, 5), (9, 3)], 0x20_0000, &mut out),
        Ok(1)
    );
    // output overflow is reported
    let many: Vec<Region> = (0..8)
        .map(|i| Region {
            start: 0x10_0000 + i * 0x2000,
            end: 0x10_1000 + i * 0x2000,
            kind: 1,
        })
        .collect();
    assert_eq!(
        usable_ranges(&many, &[], 1 << 30, &mut out),
        Err(MbError::TooMany)
    );
}

fn full_info(flags: u32) -> Vec<u8> {
    let mut v = vec![0u8; 88];
    v[0..4].copy_from_slice(&flags.to_le_bytes());
    v
}
fn put(v: &mut [u8], off: usize, x: u32) {
    v[off..off + 4].copy_from_slice(&x.to_le_bytes());
}

#[test]
fn mb_09_reserved_ranges_cover_everything_the_loader_gave_us() {
    let mut i = full_info((1 << 6) | (1 << 2) | (1 << 9) | (1 << 3));
    put(&mut i, 44, 96); // mmap len
    put(&mut i, 48, 0x9_0000); // mmap addr
    put(&mut i, 16, 0xA_0000); // cmdline
    put(&mut i, 64, 0xB_0000); // loader name
    put(&mut i, 20, 2); // two modules
    put(&mut i, 24, 0xC_0000); // module table
    let mem = |a: u64| -> Option<u32> {
        match a {
            0xC_0000 => Some(0x20_0000),
            0xC_0004 => Some(0x20_3000),
            0xC_0008 => Some(0xD_0000),
            0xC_0010 => Some(0x30_0000),
            0xC_0014 => Some(0x30_0000), // empty module image
            0xC_0018 => Some(0),
            _ => None,
        }
    };
    let mut out = [(0, 0); 16];
    let n = reserved_ranges(0x8_F000, &i, mem, &mut out).unwrap();
    assert_eq!(
        &out[..n],
        &[
            (0x8_F000, 0x8_F000 + 88),
            (0x9_0000, 0x9_0000 + 96),
            (0xA_0000, 0xA_0000 + 4096),
            (0xB_0000, 0xB_0000 + 4096),
            (0xC_0000, 0xC_0000 + 32),
            (0x20_0000, 0x20_3000),
            (0xD_0000, 0xD_0000 + 4096),
        ]
    );
}

#[test]
fn mb_10_reserved_ranges_negative_cases() {
    let none = |_: u64| -> Option<u32> { None };
    let mut out = [(0, 0); 16];
    assert_eq!(
        reserved_ranges(0x9000, &[0; 87], none, &mut out),
        Err(MbError::InfoTruncated)
    );
    // flags clear: only the info block itself
    assert_eq!(
        reserved_ranges(0x9000, &full_info(0), none, &mut out),
        Ok(1)
    );
    // too many modules
    let mut i = full_info(1 << 3);
    put(&mut i, 20, MAX_MODULES + 1);
    put(&mut i, 24, 0x1000);
    assert_eq!(
        reserved_ranges(0x9000, &i, none, &mut out),
        Err(MbError::Modules)
    );
    // unreadable module table
    put(&mut i, 20, 1);
    assert_eq!(
        reserved_ranges(0x9000, &i, none, &mut out),
        Err(MbError::Modules)
    );
    // module whose end is before its start
    let bad = |a: u64| -> Option<u32> { Some(if a == 0x1000 { 0x5000 } else { 0x4000 }) };
    assert_eq!(
        reserved_ranges(0x9000, &i, bad, &mut out),
        Err(MbError::Modules)
    );
    // output capacity
    let mut tiny = [(0, 0); 1];
    assert_eq!(
        reserved_ranges(0x9000, &full_info(1 << 6), none, &mut tiny),
        Err(MbError::TooMany)
    );
    // address arithmetic cannot wrap
    let mut i = full_info(1 << 6);
    put(&mut i, 44, 0x1000);
    put(&mut i, 48, u32::MAX);
    assert!(
        reserved_ranges(0x9000, &i, none, &mut out).is_ok(),
        "u32 + len fits in u64"
    );
    assert_eq!(
        reserved_ranges(u64::MAX - 10, &full_info(0), none, &mut out),
        Err(MbError::Overflow)
    );
}
