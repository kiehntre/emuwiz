//! Compatibility and bounded-memory coverage for the vendored `chd` 0.3.4 patch
//! (third_party/chd-0.3.4/PATCH.md): CHD Zstandard codecs must keep decoding
//! valid hunks, reject malformed ones, and never allocate unboundedly for a
//! tiny hostile input (RUSTSEC-2026-0344). Synthetic frames only.
use chd::codecs::{CdZstdCodec, CodecImplementation, ZstdCodec};

fn frame(blocks: &[(u8, bool, &[u8])], content_size: u32) -> Vec<u8> {
    // magic, FHD = single segment + 4-byte frame content size.
    let mut out = vec![0x28, 0xB5, 0x2F, 0xFD, 0xA0];
    out.extend(content_size.to_le_bytes());
    for (i, (kind, _, body)) in blocks.iter().enumerate() {
        let last = u32::from(i + 1 == blocks.len());
        // RLE blocks carry one byte but declare their regenerated size.
        let size = if *kind == 1 {
            u32::from_le_bytes([body[1], body[2], body[3], body[4]])
        } else {
            body.len() as u32
        };
        let header = last | (u32::from(*kind) << 1) | (size << 3);
        out.extend(&header.to_le_bytes()[..3]);
        out.extend(if *kind == 1 { &body[..1] } else { body });
    }
    out
}
fn raw(data: &[u8]) -> Vec<u8> {
    frame(&[(0, false, data)], data.len() as u32)
}
/// A 5-byte compressed block that regenerates `size` literal bytes (RLE
/// literals, zero sequences): a decompression-bomb shape, bounded here to 1 MiB.
fn rle_literal_block(size: u32) -> Vec<u8> {
    let literals = 1u32 | (3 << 2) | (size << 4);
    let mut body = literals.to_le_bytes()[..3].to_vec();
    body.push(0xAA);
    body.push(0); // zero sequences
    body
}
fn bomb(declared: u32, size: u32) -> Vec<u8> {
    frame(&[(2, false, &rle_literal_block(size))], declared)
}

#[test]
fn zstd_codec_decodes_valid_hunks() {
    let mut codec = ZstdCodec::new(4096).unwrap();
    let data: Vec<u8> = (0..4096u32).map(|i| (i * 7) as u8).collect();
    let mut out = vec![0u8; 4096];
    codec.decompress(&raw(&data), &mut out).unwrap();
    assert_eq!(out, data);
    // RLE block: one byte repeated.
    let mut rle_body = vec![0x55u8];
    rle_body.extend(4096u32.to_le_bytes());
    let rle = frame(&[(1, false, &rle_body)], 4096);
    let mut out = vec![0u8; 4096];
    codec.decompress(&rle, &mut out).unwrap();
    assert!(out.iter().all(|b| *b == 0x55));
}

#[test]
fn zstd_codec_rejects_malformed_and_short_hunks() {
    let mut codec = ZstdCodec::new(4096).unwrap();
    let data = vec![1u8; 4096];
    let good = raw(&data);
    let mut out = vec![0u8; 4096];
    // wrong output length, truncation at every prefix, bad magic, garbage
    assert!(codec.decompress(&good, &mut vec![0u8; 4000]).is_err());
    for n in 0..good.len().min(64) {
        assert!(
            codec.decompress(&good[..n], &mut out).is_err(),
            "prefix {n}"
        );
    }
    let mut bad = good.clone();
    bad[0] ^= 0xFF;
    assert!(codec.decompress(&bad, &mut out).is_err());
    assert!(codec.decompress(&[0xFF; 64], &mut out).is_err());
    assert!(codec.decompress(&[], &mut out).is_err());
}

#[test]
fn cd_zstd_codec_decodes_and_swizzles_a_valid_hunk() {
    const FRAMES: usize = 8;
    let sectors: Vec<u8> = (0..FRAMES * 2352).map(|i| (i % 251) as u8).collect();
    let subcode: Vec<u8> = (0..FRAMES * 96).map(|i| (i % 13) as u8).collect();
    let sector_frame = raw(&sectors);
    let sub_frame = raw(&subcode);
    // header: ceil(frames/8) ECC-flag bytes (all clear), then 2-byte BE sector length.
    let mut input = vec![0u8];
    input.extend((sector_frame.len() as u16).to_be_bytes());
    input.extend(&sector_frame);
    input.extend(&sub_frame);
    let mut codec = CdZstdCodec::new((FRAMES * 2448) as u32).unwrap();
    let mut out = vec![0u8; FRAMES * 2448];
    codec.decompress(&input, &mut out).unwrap();
    for i in 0..FRAMES {
        assert_eq!(
            &out[i * 2448..i * 2448 + 2352],
            &sectors[i * 2352..(i + 1) * 2352]
        );
        assert_eq!(
            &out[i * 2448 + 2352..(i + 1) * 2448],
            &subcode[i * 96..(i + 1) * 96]
        );
    }
    // the same hunk with its sector stream corrupted is rejected, not guessed.
    let mut corrupt = input.clone();
    corrupt[3] ^= 0xFF;
    assert!(codec.decompress(&corrupt, &mut out).is_err());
    // A declared sector length beyond the input is an error (see
    // chd_cdrom_decoder_bounds.rs and PATCH.md), not a panic.
    let mut overrun = input.clone();
    overrun[1..3].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(codec.decompress(&overrun, &mut out).is_err());
}

fn vm_size_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmSize:"))
        .unwrap()
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .unwrap();
    kb * 1024
}

/// Child half of the bounded-memory test: runs under a hard address-space
/// limit so a runaway allocation aborts this process, never the host.
#[test]
fn bounded_child() {
    if std::env::var_os("EMUWIZ_CHD_BOMB_CHILD").is_none() {
        return;
    }
    let limit = libc::rlimit {
        rlim_cur: vm_size_bytes() + 192 * 1024 * 1024,
        rlim_max: vm_size_bytes() + 192 * 1024 * 1024,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) }, 0);
    let mut codec = ZstdCodec::new(16384).unwrap();
    let mut out = vec![0u8; 16384];
    // block regenerates 1 MiB (> the 128 KiB Block_Maximum_Size), tiny input.
    for (declared, size) in [
        (1_000_000u32, 1_000_000u32),
        (16384, 1_000_000),
        (16384, 1_048_575),
    ] {
        let input = bomb(declared, size);
        assert!(input.len() < 32);
        assert!(
            codec.decompress(&input, &mut out).is_err(),
            "{declared}/{size}"
        );
    }
    // the same shape through the CD wrapper's sector stream.
    let sector = bomb(1_000_000, 1_000_000);
    let mut cd_in = vec![0u8];
    cd_in.extend((sector.len() as u16).to_be_bytes());
    cd_in.extend(&sector);
    cd_in.extend(&raw(&[0u8; 768]));
    let mut cd = CdZstdCodec::new(8 * 2448).unwrap();
    assert!(cd.decompress(&cd_in, &mut vec![0u8; 8 * 2448]).is_err());
}

#[test]
fn hostile_small_input_is_rejected_within_a_bounded_address_space() {
    let exe = std::env::current_exe().unwrap();
    let status = std::process::Command::new(exe)
        .args([
            "--exact",
            "bounded_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("EMUWIZ_CHD_BOMB_CHILD", "1")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "child aborted or failed under the address-space limit: {status:?}"
    );
}

#[test]
fn lockfile_resolves_no_vulnerable_ruzstd() {
    let lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
    let mut versions = Vec::new();
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == "name = \"ruzstd\"" {
            versions.push(lines.next().unwrap().to_string());
        }
    }
    assert!(!versions.is_empty());
    for v in versions {
        let v = v.trim_start_matches("version = \"").trim_end_matches('"');
        let parts: Vec<u64> = v.split('.').map(|p| p.parse().unwrap()).collect();
        assert!(
            parts >= vec![0, 9, 1],
            "ruzstd {v} is affected by RUSTSEC-2026-0344"
        );
    }
}
