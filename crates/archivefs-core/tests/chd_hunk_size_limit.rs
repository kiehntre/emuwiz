//! CHD v5 hunk-size limit enforced inside the vendored decoder
//! (third_party/chd-0.3.4/PATCH.md, "Additional patch: V5 hunk-size limit").
//!
//! `Chd::open` must refuse a header advertising an oversized, zero or
//! otherwise invalid hunk before the map or any codec buffer is sized from it,
//! even when the caller never ran `read_chd_v5_header`. Headers are 124 bytes
//! and advertise extreme sizes; nothing here allocates them.
use archivefs_core::dat::archive::chd::{CHD_V5_MAX_HUNK_BYTES, read_chd_v5_header};
use chd::Chd;
use std::io::Cursor;

const MIB: u32 = 1024 * 1024;
const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/chd_hunk");

/// A V5 header (124 bytes, zero padded) with the given compressors, hunk and unit size.
fn header(compressors: [&[u8; 4]; 4], logical: u64, hunk: u32, unit: u32) -> Vec<u8> {
    // The decoder reads a little past the 124-byte header; pad so the size
    // check, not an early EOF, is what decides the outcome.
    let mut b = vec![0u8; 124 + 64];
    b[0..8].copy_from_slice(b"MComprHD");
    b[8..12].copy_from_slice(&124u32.to_be_bytes());
    b[12..16].copy_from_slice(&5u32.to_be_bytes());
    for (i, c) in compressors.iter().enumerate() {
        b[16 + i * 4..20 + i * 4].copy_from_slice(*c);
    }
    b[32..40].copy_from_slice(&logical.to_be_bytes());
    b[40..48].copy_from_slice(&124u64.to_be_bytes()); // map offset
    b[56..60].copy_from_slice(&hunk.to_be_bytes());
    b[60..64].copy_from_slice(&unit.to_be_bytes());
    b
}
const NONE: &[u8; 4] = &[0; 4];

fn open(bytes: Vec<u8>) -> Result<Chd<Cursor<Vec<u8>>>, chd::Error> {
    Chd::open(Cursor::new(bytes), None)
}

#[test]
fn limits_agree() {
    assert_eq!(CHD_V5_MAX_HUNK_BYTES, MIB);
    assert_eq!(chd::header::MAX_V5_HUNK_BYTES, MIB);
}

#[test]
fn oversized_hunks_are_refused_before_any_codec_is_built() {
    // CD codecs size three buffers from the hunk; use hunks that are whole
    // CD frames (2448) so only the size, not the geometry check, can refuse.
    let frame = 2448u32;
    let near_4gib = (u32::MAX / frame) * frame;
    for codec in [
        b"cdlz", b"cdzl", b"cdzs", b"cdfl", b"zstd", b"lzma", b"zlib", b"flac",
    ] {
        for hunk in [MIB + 1, 2 * MIB, 16 * MIB, near_4gib, u32::MAX] {
            let h = header([codec, NONE, NONE, NONE], u64::from(hunk) * 4, hunk, 2448);
            let result = open(h);
            assert!(
                matches!(result, Err(chd::Error::InvalidParameter)),
                "{} hunk {hunk}: {:?}",
                String::from_utf8_lossy(codec),
                result.err()
            );
        }
    }
    // all four slots declared, each a CD codec
    let h = header(
        [b"cdlz", b"cdzl", b"cdzs", b"cdfl"],
        1 << 40,
        near_4gib,
        2448,
    );
    assert!(matches!(open(h), Err(chd::Error::InvalidParameter)));
}

#[test]
fn zero_and_degenerate_geometry_is_an_error_not_a_panic() {
    let cases = [(0, 512), (4096, 0), (0, 0)];
    for (hunk, unit) in cases {
        let h = header([NONE, NONE, NONE, NONE], 1 << 20, hunk, unit);
        assert!(open(h).is_err(), "hunk {hunk} unit {unit}");
    }
}

#[test]
fn boundary_values_pass_the_size_check() {
    // Exactly the limit and the old 512 KiB limit are not refused for size:
    // the header is accepted and the open then fails later (no map in this
    // 124-byte file), which is a different error from InvalidParameter.
    for hunk in [512 * 1024, MIB - 512, MIB] {
        let h = header([NONE, NONE, NONE, NONE], u64::from(hunk) * 2, hunk, 512);
        if let Err(chd::Error::InvalidParameter) = open(h) {
            panic!("hunk {hunk} refused for size");
        }
    }
    // One byte over is refused for size.
    let h = header([NONE, NONE, NONE, NONE], u64::from(MIB + 1) * 2, MIB + 1, 1);
    assert!(matches!(open(h), Err(chd::Error::InvalidParameter)));
}

#[test]
fn real_chdman_files_at_512_kib_and_1_mib_open_and_read() {
    for (name, hunk) in [("hunk_512kib.chd", 512 * 1024u32), ("hunk_1mib.chd", MIB)] {
        let data = std::fs::read(format!("{DIR}/{name}")).unwrap();
        // EmuWiz's own header reader accepts it ...
        let parsed = read_chd_v5_header(&mut Cursor::new(&data)).unwrap();
        assert_eq!(parsed.hunk_bytes, hunk);
        // ... and the vendored decoder opens and reads every hunk.
        let mut chd = open(data).unwrap();
        assert_eq!(chd.header().hunk_size(), hunk);
        let mut out = chd.get_hunksized_buffer();
        let mut compressed = Vec::new();
        for n in 0..chd.header().hunk_count() {
            let mut h = chd.hunk(n).unwrap();
            assert_eq!(
                h.read_hunk_in(&mut compressed, &mut out).unwrap(),
                out.len()
            );
            assert!(out.iter().all(|b| *b == 0), "{name} hunk {n}");
        }
    }
}
