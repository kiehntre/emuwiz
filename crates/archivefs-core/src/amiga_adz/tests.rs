use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;

use super::*;

/// A minimal, structurally valid flat AmigaDOS floppy image (no RDB
/// wrapper) - the same shape `ingestion::tests::minimal_flat_adf` /
/// `amiga_disk::tests::flat_adf` build: a `DOS\xN` boot block, a
/// root-block pointer, and a valid `ST_ROOT` root block with a volume
/// label. This is what `.adf` content inspection validates through the
/// existing bounded OFS/FFS reader - duplicated here as a tiny synthetic
/// fixture (no copyrighted game data), not a second parser.
fn minimal_flat_adf(dos: u8, volume: &[u8]) -> Vec<u8> {
    const SECTORS: usize = 128;
    const ROOT: usize = SECTORS / 2;
    let mut img = vec![0u8; SECTORS * 512];
    img[..3].copy_from_slice(b"DOS");
    img[3] = dos;
    put32(&mut img, 8, ROOT as u32);
    let mut root = [0u8; 512];
    put32(&mut root, 0, 2); // T_HEADER
    put32(&mut root, 12, 72); // hash-table size
    let name_len = volume.len().min(30);
    root[0x1B0] = name_len as u8;
    root[0x1B1..0x1B1 + name_len].copy_from_slice(&volume[..name_len]);
    put32(&mut root, 508, 1); // ST_ROOT
    let mut sum = 0u32;
    for offset in (0..512).step_by(4) {
        if offset != 20 {
            sum = sum.wrapping_add(u32::from_be_bytes(
                root[offset..offset + 4].try_into().unwrap(),
            ));
        }
    }
    put32(&mut root, 20, (sum as i32).wrapping_neg() as u32);
    img[ROOT * 512..(ROOT + 1) * 512].copy_from_slice(&root);
    img
}

fn put32(v: &mut [u8], at: usize, n: u32) {
    v[at..at + 4].copy_from_slice(&n.to_be_bytes());
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn write_adz(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, gzip(bytes)).unwrap();
    path
}

#[test]
fn valid_gzipped_adf_is_inspected_as_amiga_floppy() {
    let dir = tempfile::tempdir().unwrap();
    let adf = minimal_flat_adf(0, b"PuzzleDisk");
    let path = write_adz(dir.path(), "game.adz", &adf);

    let inspection = inspect_adz(&path).unwrap();
    assert_eq!(inspection.container_path, path);
    assert_eq!(inspection.decompressed_bytes, adf.len() as u64);
    assert_eq!(
        inspection.floppy.filesystem().unwrap().family,
        amiga_disk::AmigaDosFamily::Ofs
    );
}

#[test]
fn decompressed_adf_evidence_matches_the_raw_fixture() {
    // The same fixture bytes inspected directly via `inspect_amiga_floppy`
    // (as a raw `.adf` would be) and via `inspect_adz` (gzip-wrapped) must
    // produce the identical filesystem evidence - proving the ADZ path adds
    // no ADF semantics of its own.
    let dir = tempfile::tempdir().unwrap();
    let adf = minimal_flat_adf(1, b"FastFileDisk");
    let raw_path = dir.path().join("raw.adf");
    std::fs::write(&raw_path, &adf).unwrap();
    let raw = amiga_disk::inspect_amiga_floppy(&raw_path).unwrap();

    let adz_path = write_adz(dir.path(), "same.adz", &adf);
    let inspection = inspect_adz(&adz_path).unwrap();

    assert_eq!(inspection.floppy.evidence, raw.evidence);
    assert_eq!(inspection.floppy.disk.rdb, raw.disk.rdb);
}

#[test]
fn adz_preserves_plausible_logical_evidence_without_container_or_game_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut adf = vec![0_u8; 880 * 1024];
    adf[..4].copy_from_slice(b"DOS\x01");
    put32(&mut adf, 8, 880);
    let raw_path = dir.path().join("raw.adf");
    std::fs::write(&raw_path, &adf).unwrap();
    let raw = amiga_disk::inspect_amiga_floppy(&raw_path).unwrap();
    let path = write_adz(dir.path(), "Famous Game (Disk 1).adz", &adf);
    let before = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let inspection = inspect_adz(&path).unwrap();
    assert!(inspection.floppy.filesystem().is_none());
    assert_eq!(inspection.floppy.evidence, raw.evidence);
    assert_eq!(inspection.container_path, path);
    assert_ne!(inspection.floppy.disk.path, path);
    assert_eq!(inspection.decompressed_bytes, adf.len() as u64);
    let evidence = amiga_disk::structural_amiga_floppy_observation(&inspection.floppy);
    assert_eq!(
        evidence.claim_strength,
        crate::platform_evidence_fusion::evidence_lineage::ClaimStrength::Weak
    );
    assert_eq!(evidence.release_candidate, None);
    assert_eq!(evidence.hash_or_value, None);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn malformed_gzip_fails_soft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("truncated.adz");
    // Real gzip magic, then bytes that do not form a valid deflate stream.
    std::fs::write(&path, [0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00]).unwrap();

    let error = inspect_adz(&path).unwrap_err();
    assert!(matches!(error, AdzError::MalformedGzip(_)));
}

#[test]
fn truncated_gzip_fails_soft() {
    let dir = tempfile::tempdir().unwrap();
    let adf = minimal_flat_adf(0, b"PuzzleDisk");
    let full = gzip(&adf);
    let path = dir.path().join("cutoff.adz");
    std::fs::write(&path, &full[..full.len() / 2]).unwrap();

    let error = inspect_adz(&path).unwrap_err();
    assert!(matches!(error, AdzError::MalformedGzip(_)));
}

#[test]
fn decompression_limit_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let oversized = vec![0u8; (MAX_ADZ_DECOMPRESSED_BYTES + 4096) as usize];
    let path = write_adz(dir.path(), "bomb.adz", &oversized);

    let error = inspect_adz(&path).unwrap_err();
    assert!(matches!(error, AdzError::DecompressedTooLarge { .. }));
}

#[test]
fn gzip_of_non_adf_bytes_is_rejected_as_not_adf() {
    let dir = tempfile::tempdir().unwrap();
    // A plausible size, but not a DOS\0..DOS\7 boot signature at all.
    let random = vec![0x42u8; 4096];
    let path = write_adz(dir.path(), "notadf.adz", &random);

    let error = inspect_adz(&path).unwrap_err();
    assert!(matches!(error, AdzError::NotAdf(_)));
}

#[test]
fn container_and_logical_provenance_stay_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let adf = minimal_flat_adf(0, b"PuzzleDisk");
    let path = write_adz(dir.path(), "distinct.adz", &adf);

    let inspection = inspect_adz(&path).unwrap();
    // The container path names the .adz; the logical disk's own `path`
    // (set by `inspect_amiga_floppy`) names the private temp file it was
    // actually read from - never claimed to be the same identity, and
    // never the .adz path substituted in as if it were the raw ADF.
    assert_eq!(inspection.container_path, path);
    assert_ne!(inspection.floppy.disk.path, path);
    assert_eq!(inspection.floppy.disk.image_size, adf.len() as u64);
}

#[test]
fn inspection_never_leaves_a_named_file_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let adf = minimal_flat_adf(0, b"PuzzleDisk");
    let path = write_adz(dir.path(), "clean.adz", &adf);

    let before: std::collections::BTreeSet<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    inspect_adz(&path).unwrap();
    let after: std::collections::BTreeSet<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        before, after,
        "inspection must not create any new directory entry"
    );

    // The temp directory itself never gains a leftover archivefs-adz file
    // either (covers the O_TMPFILE-unavailable fallback path).
    let leftovers: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".archivefs-adz-")
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn not_gzip_is_rejected_outright() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.adz");
    std::fs::write(&path, b"just a normal ADF-looking file, not gzip").unwrap();

    assert!(matches!(inspect_adz(&path), Err(AdzError::NotGzip)));
}

#[test]
fn oversized_container_is_refused_before_decompression() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.adz");
    let mut oversized = vec![0u8; (MAX_ADZ_COMPRESSED_BYTES + 4096) as usize];
    oversized[0] = GZIP_MAGIC[0];
    oversized[1] = GZIP_MAGIC[1];
    std::fs::write(&path, &oversized).unwrap();

    let error = inspect_adz(&path).unwrap_err();
    assert!(matches!(error, AdzError::ContainerTooLarge { .. }));
}

#[test]
fn concatenated_members_validate_one_logical_disk() {
    let dir = tempfile::tempdir().unwrap();
    let tiny = minimal_flat_adf(1, b"SplitDisk");
    let mut adf = vec![0_u8; 901_120];
    adf[..1024].copy_from_slice(&tiny[..1024]);
    put32(&mut adf, 8, 880);
    adf[880 * 512..881 * 512].copy_from_slice(&tiny[64 * 512..65 * 512]);
    let mut members = gzip(&adf[..adf.len() / 2]);
    members.extend(gzip(&adf[adf.len() / 2..]));
    let path = dir.path().join("split.adz");
    std::fs::write(&path, &members).unwrap();
    let inspection = inspect_adz(&path).expect("all gzip members form one valid ADF");
    assert_eq!(inspection.decompressed_bytes, adf.len() as u64);
    assert!(inspection.floppy.filesystem().is_some());
    assert_eq!(std::fs::read(path).unwrap(), members);
}

#[test]
fn every_gzip_member_and_trailing_byte_must_validate() {
    let dir = tempfile::tempdir().unwrap();
    let first = gzip(&minimal_flat_adf(1, b"First"));
    let second = gzip(&[0_u8; 1024]);
    let mut bad_crc = second.clone();
    let crc = bad_crc.len() - 8;
    bad_crc[crc] ^= 1;
    let mut bad_size = second.clone();
    let size = bad_size.len() - 4;
    bad_size[size] ^= 1;
    for suffix in [
        b"garbage".to_vec(),
        vec![0],
        second[..second.len() - 1].to_vec(),
        bad_crc,
        bad_size,
    ] {
        let path = dir.path().join("bad-tail.adz");
        let mut bytes = first.clone();
        bytes.extend(suffix);
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            inspect_adz(&path),
            Err(AdzError::MalformedGzip(_))
        ));
    }
}

#[test]
fn aggregate_member_output_obeys_the_decompression_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = gzip(&minimal_flat_adf(1, b"First"));
    bytes.extend(gzip(&vec![0_u8; MAX_ADZ_DECOMPRESSED_BYTES as usize]));
    let path = dir.path().join("aggregate-bomb.adz");
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        inspect_adz(&path),
        Err(AdzError::DecompressedTooLarge { .. })
    ));
}

#[test]
fn compressed_read_limit_is_enforced_even_after_file_growth() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_adz(dir.path(), "growing.adz", &minimal_flat_adf(1, b"Growing"));
    let mut file = File::open(&path).unwrap();
    let before = file.metadata().unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_ADZ_COMPRESSED_BYTES + 1)
        .unwrap();
    assert!(matches!(
        read_compressed_file(&mut file, &before),
        Err(AdzError::ContainerTooLarge { .. })
    ));
    assert_eq!(
        std::io::Seek::stream_position(&mut file).unwrap(),
        MAX_ADZ_COMPRESSED_BYTES + 1
    );
}

#[test]
fn changed_input_and_host_read_failure_are_not_gzip_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_adz(
        dir.path(),
        "changing.adz",
        &minimal_flat_adf(1, b"Changing"),
    );
    let mut file = File::open(&path).unwrap();
    let before = file.metadata().unwrap();
    File::options()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"extra")
        .unwrap();
    assert!(matches!(
        read_compressed_file(&mut file, &before),
        Err(AdzError::ChangedWhileReading)
    ));
    let mut write_only = File::options().write(true).open(&path).unwrap();
    let before = write_only.metadata().unwrap();
    assert!(matches!(
        read_compressed_file(&mut write_only, &before),
        Err(AdzError::Io(_))
    ));
    assert!(matches!(
        inspect_adz(&dir.path().join("missing.adz")),
        Err(AdzError::Io(_))
    ));
}

#[test]
fn staging_failure_preserves_the_environmental_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_adz(dir.path(), "valid.adz", &minimal_flat_adf(1, b"Valid"));
    assert!(matches!(
        inspect_adz_in_temp_dir(&path, &dir.path().join("unavailable")),
        Err(AdzError::TempFileUnavailable(_))
    ));
}

#[test]
fn fifo_without_a_writer_and_directories_are_refused_without_blocking() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pipe.adz");
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a valid NUL-terminated path in our private fixture directory.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || send.send(inspect_adz(&path)).unwrap());
    let result = receive
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("opening a FIFO with no writer must not block inspection");
    assert!(matches!(result, Err(AdzError::NotRegularFile)));
    assert!(matches!(
        inspect_adz(dir.path()),
        Err(AdzError::NotRegularFile)
    ));
}

#[test]
fn exact_aggregate_limit_still_validates_the_last_trailer() {
    let dir = tempfile::tempdir().unwrap();
    let adf = minimal_flat_adf(1, b"Limit");
    let mut bytes = gzip(&adf);
    let mut last = gzip(&vec![0_u8; MAX_ADZ_DECOMPRESSED_BYTES as usize - adf.len()]);
    let path = dir.path().join("at-limit.adz");
    bytes.extend(&last);
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        inspect_adz(&path).unwrap().decompressed_bytes,
        MAX_ADZ_DECOMPRESSED_BYTES
    );
    let crc = last.len() - 8;
    last[crc] ^= 1;
    let mut bad = gzip(&adf);
    bad.extend(last);
    std::fs::write(&path, bad).unwrap();
    assert!(matches!(
        inspect_adz(&path),
        Err(AdzError::MalformedGzip(_))
    ));
}

#[test]
fn long_optional_gzip_fields_are_a_decoder_limit_not_corrupt_content() {
    let dir = tempfile::tempdir().unwrap();
    let gzip = gzip(&minimal_flat_adf(1, b"LongHeader"));
    for flag in [8, 16] {
        // FNAME and FCOMMENT
        let mut bytes = gzip[..10].to_vec();
        bytes[3] = flag;
        bytes.extend(vec![b'x'; 65_536]);
        bytes.push(0);
        bytes.extend(&gzip[10..]);
        let path = dir.path().join("long-header.adz");
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            inspect_adz(&path),
            Err(AdzError::UnsupportedGzip(_))
        ));
    }
}
