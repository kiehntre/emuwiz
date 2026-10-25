//! Malformed CHD CD-ROM hunks (cdlz / cdzl / cdzs) must produce errors, never
//! decoder panics. Synthetic fixtures only (tests/fixtures/chd_cdrom/README.md).
//!
//! Background: `chd::compression::cdrom::CdCodec::decompress` used to index and
//! slice the compressed hunk with lengths read from the hunk itself. The hunk
//! CRC is verified only after decoding, so a damaged hunk in an otherwise
//! structurally valid CHD reached that code.
use archivefs_core::chd_logical_media::open_chd_track_logical_media;
use archivefs_core::logical_media::LogicalMedia;
use chd::codecs::{CdLzmaCodec, CdZlibCodec, CdZstdCodec, CodecImplementation};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/chd_cdrom");
const HUNK0_HEADER: usize = 229; // ecc flag byte, then 2-byte BE stream length
const HUNK_BYTES: usize = 8 * 2448;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{DIR}/{name}")).unwrap()
}

fn expected_user_data() -> Vec<u8> {
    (0..16usize)
        .flat_map(|i| (0..2048usize).map(move |j| ((i * 31 + j * 7) % 251) as u8))
        .collect()
}

fn read_all(data: &[u8]) -> Result<Vec<u8>, String> {
    let media = open_chd_track_logical_media(data).map_err(|e| format!("open: {e:?}"))?;
    let mut out = vec![0u8; media.len() as usize];
    media.read_at(0, &mut out).map_err(|e| format!("{e:?}"))?;
    Ok(out)
}

/// Hunk 0's compressed bytes (header + streams), located from the sector
/// stream length the fixture declares.
fn hunk0_payload(data: &[u8]) -> &[u8] {
    // hunk 0 = 3 header bytes + sector stream + subcode stream; the subcode
    // stream is small and ends where the next hunk (or the map) begins.
    let map = u64::from_be_bytes(data[0x28..0x30].try_into().unwrap()) as usize;

    &data[HUNK0_HEADER..map]
}

#[test]
fn valid_cd_codecs_still_decode_the_reference_image() {
    let want = expected_user_data();
    for name in ["cdzs.chd", "cdlz.chd", "cdzl.chd"] {
        let got = read_all(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn reader_reports_error_for_hunk_with_overrunning_declared_length() {
    // Exact preserved trigger, and the same edit applied to the other codecs.
    let preserved = fixture("cdzs-declared-length-overrun.chd");
    let mut cases = vec![("cdzs-declared-length-overrun.chd", preserved)];
    for name in ["cdlz.chd", "cdzl.chd"] {
        let mut d = fixture(name);
        d[HUNK0_HEADER + 1] = 0xFF;
        d[HUNK0_HEADER + 2] = 0xFF;
        cases.push((name, d));
    }
    for (name, data) in cases {
        let err = read_all(&data).expect_err(name);
        assert!(
            !err.starts_with("open:"),
            "{name} must open, then fail: {err}"
        );
    }
}

fn run_codec(kind: u8, input: &[u8], frames: usize) -> chd::Result<()> {
    let hunk = (frames * 2448) as u32;
    let mut out = vec![0u8; frames * 2448];
    match kind {
        0 => CdZstdCodec::new(hunk)?
            .decompress(input, &mut out)
            .map(drop),
        1 => CdLzmaCodec::new(hunk)?
            .decompress(input, &mut out)
            .map(drop),
        _ => CdZlibCodec::new(hunk)?
            .decompress(input, &mut out)
            .map(drop),
    }
}

#[test]
fn codec_rejects_inputs_shorter_than_their_own_header() {
    // 2-byte length form (hunk < 65536) and 3-byte form (hunk >= 65536).
    for (frames, header) in [(8usize, 3usize), (27, 3 + 1 + 1)] {
        for kind in 0..3u8 {
            for n in 0..=header + 1 {
                let input = vec![0u8; n];
                assert!(
                    run_codec(kind, &input, frames).is_err(),
                    "kind {kind} frames {frames} len {n}"
                );
            }
        }
    }
}

#[test]
fn codec_rejects_every_declared_length_beyond_the_hunk_for_all_codecs() {
    for (kind, name) in [(0u8, "cdzs.chd"), (1, "cdlz.chd"), (2, "cdzl.chd")] {
        let file = fixture(name);
        let mut hunk = hunk0_payload(&file).to_vec();
        let real = u16::from_be_bytes([hunk[1], hunk[2]]) as usize;
        for declared in (0..=u16::MAX).step_by(7).chain([real as u16 + 1, u16::MAX]) {
            hunk[1..3].copy_from_slice(&declared.to_be_bytes());
            let res = run_codec(kind, &hunk, 8);
            if declared as usize > hunk.len() - 3 {
                assert!(res.is_err(), "{name} declared {declared} must be an error");
            }
        }
    }
}

#[test]
fn codec_survives_every_truncation_of_a_valid_hunk() {
    for (kind, name) in [(0u8, "cdzs.chd"), (1, "cdlz.chd"), (2, "cdzl.chd")] {
        let file = fixture(name);
        let hunk = hunk0_payload(&file).to_vec();
        for n in 0..hunk.len() {
            // Must return (Ok or Err); a panic fails the test.
            let _ = run_codec(kind, &hunk[..n], 8);
        }
        // and a single-byte sweep over the 3 header bytes.
        for pos in 0..3 {
            for v in 0..=255u8 {
                let mut h = hunk.clone();
                h[pos] = v;
                let _ = run_codec(kind, &h, 8);
            }
        }
    }
}

#[test]
fn hunk_buffer_constants_match_the_fixtures() {
    assert_eq!(HUNK_BYTES, 19584);
}
