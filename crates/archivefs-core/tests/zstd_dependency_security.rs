//! Authored synthetic RVZ/Zstandard streams, never production disc images.
//! RVZ offsets/layout follow the pinned nod 1.4.4 reader (see its PATCH.md).
use sha1::{Digest, Sha1};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

const CHUNK: usize = 0x8000;
fn be32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_be_bytes());
}
fn be64(out: &mut [u8], at: usize, value: u64) {
    out[at..at + 8].copy_from_slice(&value.to_be_bytes());
}
fn sha1(bytes: &[u8]) -> Vec<u8> {
    Sha1::digest(bytes).to_vec()
}
fn legacy_frame(bytes: &[u8]) -> Vec<u8> {
    // Zstandard single-segment frame with a raw final block: independent of
    // the upgraded encoder, valid for old and current Zstandard decoders.
    let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0xa0];
    frame.extend((bytes.len() as u32).to_le_bytes());
    let header = ((bytes.len() as u32) << 3) | 1;
    frame.extend(&header.to_le_bytes()[..3]);
    frame.extend(bytes);
    frame
}
fn frame(bytes: &[u8], compressed: bool) -> Vec<u8> {
    if compressed {
        zstd::encode_all(bytes, 3).unwrap()
    } else {
        legacy_frame(bytes)
    }
}
fn append(out: &mut Vec<u8>, bytes: &[u8]) -> usize {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let offset = out.len();
    out.extend(bytes);
    offset
}
fn rvz(compressed: bool) -> (Vec<u8>, Vec<u8>, Vec<usize>) {
    rvz_with_truncated_group(compressed, None)
}
fn rvz_with_truncated_group(
    compressed: bool,
    truncate: Option<usize>,
) -> (Vec<u8>, Vec<u8>, Vec<usize>) {
    let mut iso: Vec<u8> = (0..3 * CHUNK)
        .map(|i| ((i * 17 + i / 8192) % 251) as u8)
        .collect();
    iso[..0x80].fill(0);
    iso[..6].copy_from_slice(b"TSTX01");
    be32(&mut iso, 0x1c, 0xc2339f3d); // GameCube magic
    iso[0x20..0x38].copy_from_slice(b"Synthetic security disc\0");
    let mut out = vec![0; 0x48 + 0xdc];
    // One raw-data span, backed by three independent compressed groups.
    let mut raw = vec![0; 24];
    be64(&mut raw, 0, 0x80);
    be64(&mut raw, 8, (iso.len() - 0x80) as u64);
    be32(&mut raw, 20, 3);
    let raw_frame = frame(&raw, compressed);
    let raw_offset = append(&mut out, &raw_frame);
    let mut group_headers = vec![0; 36];
    let mut group_offsets = Vec::new();
    for (i, chunk) in iso.chunks_exact(CHUNK).enumerate() {
        let mut encoded = frame(chunk, compressed);
        if compressed {
            assert!(encoded.len() < chunk.len() / 4);
        }
        if truncate == Some(i) {
            encoded.truncate(encoded.len() - 1);
        }
        let offset = append(&mut out, &encoded);
        group_offsets.push(offset);
        be32(&mut group_headers, i * 12, (offset / 4) as u32);
        be32(
            &mut group_headers,
            i * 12 + 4,
            (encoded.len() as u32) | 0x8000_0000,
        );
    }
    let groups_frame = frame(&group_headers, compressed);
    let group_offset = append(&mut out, &groups_frame);
    let mut disc = vec![0; 0xdc];
    be32(&mut disc, 0, 1);
    be32(&mut disc, 4, 5); // GC, Zstandard
    be32(&mut disc, 8, 3);
    be32(&mut disc, 12, CHUNK as u32);
    disc[0x10..0x90].copy_from_slice(&iso[..0x80]);
    be32(&mut disc, 0x94, 48); // WIAPartition size, no Wii partitions
    be64(&mut disc, 0x98, 0x124);
    disc[0xa0..0xb4].copy_from_slice(&sha1(&[]));
    be32(&mut disc, 0xb4, 1);
    be64(&mut disc, 0xb8, raw_offset as u64);
    be32(&mut disc, 0xc0, raw_frame.len() as u32);
    be32(&mut disc, 0xc4, 3);
    be64(&mut disc, 0xc8, group_offset as u64);
    be32(&mut disc, 0xd0, groups_frame.len() as u32);
    out[0x48..0x124].copy_from_slice(&disc);
    out[..4].copy_from_slice(b"RVZ\x01");
    be32(&mut out, 4, 0x0100_0000);
    be32(&mut out, 8, 0x0100_0000);
    be32(&mut out, 12, disc.len() as u32);
    out[0x10..0x24].copy_from_slice(&sha1(&disc));
    be64(&mut out, 0x24, iso.len() as u64);
    let size = out.len() as u64;
    be64(&mut out, 0x2c, size);
    let hash = sha1(&out[..0x34]);
    out[0x34..0x48].copy_from_slice(&hash);
    (out, iso, group_offsets)
}
fn open(bytes: Vec<u8>) -> nod::Result<nod::Disc> {
    nod::Disc::new_stream(Box::new(Cursor::new(bytes)))
}
fn verify_roundtrip(compressed: bool) {
    let (bytes, expected, _) = rvz(compressed);
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.rvz");
    std::fs::write(&source, &bytes).unwrap();
    let mut disc = nod::Disc::new(&source).unwrap();
    assert!(disc.header().is_gamecube());
    assert_eq!(disc.disc_size(), expected.len() as u64);
    for offset in [2 * CHUNK + 3, CHUNK - 7, 0, CHUNK + 23] {
        disc.seek(SeekFrom::Start(offset as u64)).unwrap();
        let mut window = [0; 128];
        disc.read_exact(&mut window).unwrap();
        assert_eq!(window, expected[offset..offset + 128]);
    }
    disc.seek(SeekFrom::Start(0)).unwrap();
    let mut converted = Vec::new();
    // The tiny fixture represents only this advertised logical span, not a
    // full-size padded GameCube disc. Bound the reader to that span.
    std::io::copy(&mut disc.take(expected.len() as u64), &mut converted).unwrap();
    assert_eq!(converted, expected);
    assert_eq!(
        std::fs::read(&source).unwrap(),
        bytes,
        "input must remain unchanged"
    );
}
#[test]
fn legacy_zstandard_rvz_reconstructs_exact_bytes_across_groups_and_seeks() {
    verify_roundtrip(false);
}
#[test]
fn compressed_zstandard_rvz_reconstructs_exact_bytes_across_groups_and_seeks() {
    verify_roundtrip(true);
}
#[test]
fn malformed_and_truncated_rvz_frames_refuse_without_partial_success() {
    let (bytes, _, groups) = rvz(true);
    for offset in &groups {
        let mut bad = bytes.clone();
        bad[*offset] ^= 0xff;
        let result = open(bad).and_then(|mut disc| {
            disc.seek(SeekFrom::Start(0)).unwrap();
            let length = disc.disc_size();
            let mut output = Vec::new();
            let count = disc
                .take(length)
                .read_to_end(&mut output)
                .map_err(|e| nod::Error::Other(e.to_string()))?;
            if count as u64 != length {
                return Err(nod::Error::Other("short decoded span".into()));
            }
            Ok(count)
        });
        assert!(result.is_err(), "group offset {offset}");
    }
    for size in [0, 4, 0x48, 0x124, groups[0] + 5, bytes.len() - 1] {
        assert!(open(bytes[..size].to_vec()).is_err(), "truncation {size}");
    }
}
#[test]
fn reused_encoder_context_discards_abandoned_stream_bytes() {
    let mut context = zstd::zstd_safe::CCtx::create();
    {
        let mut abandoned = zstd::stream::write::Encoder::with_context(Vec::new(), &mut context);
        abandoned
            .write_all(b"abandoned private stream bytes")
            .unwrap();
    }
    let mut fresh = zstd::stream::write::Encoder::with_context(Vec::new(), &mut context);
    fresh.write_all(b"fresh frame only").unwrap();
    let encoded = fresh.finish().unwrap();
    assert_eq!(
        zstd::decode_all(encoded.as_slice()).unwrap(),
        b"fresh frame only"
    );
}
#[test]
fn malformed_decoder_stream_does_not_contaminate_the_next_context_user() {
    let mut context = zstd::zstd_safe::DCtx::create();
    {
        let mut bad = zstd::stream::write::Decoder::with_context(Vec::new(), &mut context);
        assert!(bad.write_all(b"not a Zstandard frame").is_err());
    }
    let compressed = zstd::encode_all(b"independent payload".as_slice(), 3).unwrap();
    let mut decoder = zstd::stream::write::Decoder::with_context(Vec::new(), &mut context);
    decoder.write_all(&compressed).unwrap();
    decoder.flush().unwrap();
    assert_eq!(decoder.into_inner(), b"independent payload");
}
#[test]
fn workspace_lock_has_only_patched_zstandard_wrappers() {
    let lock: toml::Value = include_str!("../../../Cargo.lock").parse().unwrap();
    for (name, minimum) in [("zstd", (0, 14, 3)), ("zstd-safe", (8, 2, 0))] {
        let packages: Vec<_> = lock["package"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["name"].as_str() == Some(name))
            .collect();
        assert_eq!(
            packages.len(),
            1,
            "{name} must not retain an older parallel version"
        );
        let version: Vec<u64> = packages[0]["version"]
            .as_str()
            .unwrap()
            .split('.')
            .map(|v| v.parse().unwrap())
            .collect();
        assert!((version[0], version[1], version[2]) >= minimum);
    }
}

#[test]
fn truncated_later_zstandard_group_refuses_with_valid_container_tables() {
    let (bytes, _, _) = rvz_with_truncated_group(true, Some(1));
    let mut disc = open(bytes).expect("header, tables and first group remain valid");
    disc.seek(SeekFrom::Start(CHUNK as u64)).unwrap();
    assert!(disc.read_exact(&mut [0; 128]).is_err());
}
#[test]
fn reused_decoder_context_discards_pending_bytes_from_abandoned_frame() {
    use zstd::stream::raw::{Decoder, Operation};
    let old = zstd::encode_all(vec![b'S'; CHUNK].as_slice(), 3).unwrap();
    let fresh = zstd::encode_all(b"fresh frame only".as_slice(), 3).unwrap();
    let mut context = zstd::zstd_safe::DCtx::create();
    {
        let mut abandoned = Decoder::with_context(&mut context);
        let mut one_byte = [0];
        let status = abandoned.run_on_buffers(&old, &mut one_byte).unwrap();
        assert_eq!(status.bytes_written, 1);
        assert_eq!(one_byte, [b'S']);
    }
    let mut decoder = Decoder::with_context(&mut context);
    let mut output = [0; 64];
    let status = decoder.run_on_buffers(&fresh, &mut output).unwrap();
    assert_eq!(status.remaining, 0);
    assert_eq!(&output[..status.bytes_written], b"fresh frame only");
}
