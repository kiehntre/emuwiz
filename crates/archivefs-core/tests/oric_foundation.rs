use archivefs_core::content_detector::stream_probe::probe_content_stream;
use archivefs_core::content_detector::{ContentDetectionOutcome, ContentDetector};
use archivefs_core::content_evidence::{
    ContentEvidence, ContentEvidenceConfidence, ContentEvidenceKind,
};
use archivefs_core::disk_format::oric::*;
use archivefs_core::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};
use archivefs_core::oric_media::*;
use archivefs_core::oric_tape::*;
use archivefs_core::platform::{DetectionRequest, detect_platform_report, platform_for_alias};
use archivefs_core::platform_evidence_fusion::identity_orchestrator::{
    IdentityInspectionInput, inspect_identity,
};
use archivefs_core::platform_evidence_fusion::{FusionOutcome, fuse_platform_evidence};
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::tape_analysis::{ChecksumState, TapeAnalysisError, TapeFormat, analyze_tape};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "emuwiz-oric-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn tap(name: &[u8], start: u16, data: &[u8]) -> Vec<u8> {
    let end = start as usize + data.len() - 1;
    assert!(end <= 65535);
    let mut b = vec![
        0x16,
        0x16,
        0x16,
        0x24,
        0,
        0,
        0x80,
        0xc7,
        (end >> 8) as u8,
        end as u8,
        (start >> 8) as u8,
        start as u8,
        0,
    ];
    b.extend(name);
    b.push(0);
    b.extend(data);
    b
}

fn crc(b: &[u8]) -> u16 {
    // Independent polynomial long division for generated sector fixtures.
    let mut remainder = 0xffffu32;
    for v in b {
        remainder ^= (*v as u32) << 8;
        for _ in 0..8 {
            remainder <<= 1;
            if remainder & 0x10000 != 0 {
                remainder ^= 0x11021;
            }
        }
    }
    remainder as u16
}
fn record(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend(bytes);
    out.extend(crc(bytes).to_be_bytes());
}

fn mfm(sides: usize, tracks: usize, sectors: usize) -> Vec<u8> {
    let mut b = vec![0; 256];
    b[..8].copy_from_slice(b"MFM_DISK");
    b[8..12].copy_from_slice(&(sides as u32).to_le_bytes());
    b[12..16].copy_from_slice(&(tracks as u32).to_le_bytes());
    b[16..20].copy_from_slice(&1u32.to_le_bytes());
    for side in 0..sides {
        for cylinder in 0..tracks {
            let mut track = vec![0x4e; if sectors == 18 { 0 } else { 60 }];
            for sector in 1..=sectors {
                track.extend([0; 12]);
                record(
                    &mut track,
                    &[
                        0xa1,
                        0xa1,
                        0xa1,
                        0xfe,
                        cylinder as u8,
                        side as u8,
                        sector as u8,
                        1,
                    ],
                );
                track.extend([0x22; 22]);
                track.extend([0; 12]);
                let mut data = vec![0xa1, 0xa1, 0xa1, 0xfb];
                data.extend([0x35; 256]);
                record(&mut track, &data);
                track.extend(vec![0x4e; if sectors == 18 { 34 } else { 38 }]);
            }
            assert!(track.len() <= 6400);
            track.resize(6400, 0x4e);
            b.extend(track);
        }
    }
    b
}

#[test]
fn oric_registry_and_aliases_are_family_scoped() {
    let p = archivefs_core::platform::platform_by_id("Oric").unwrap();
    assert!(p.strong_extensions.is_empty());
    assert!(p.magic.is_empty());
    assert!(p.preferred_emulator.is_none());
    for (kind, value) in [
        (ContentEvidenceKind::TapeFormat, ORIC_TAP_EVIDENCE),
        (ContentEvidenceKind::DiskFormat, ORIC_DSK_EVIDENCE),
    ] {
        assert_eq!(
            archivefs_core::content_evidence_scope::scope_of(kind, value),
            archivefs_core::content_evidence_scope::EvidenceScope::PlatformSpecific("Oric")
        );
    }
    let registry = archivefs_core::platform::PLATFORMS;
    let position = registry.iter().position(|p| p.id == "Oric").unwrap();
    assert_eq!(registry[position - 1].id, "NGage");
    assert_eq!(registry[position + 1].id, "PC");
    for alias in ["Oric", "ORIC-1", "Oric Atmos"] {
        assert_eq!(platform_for_alias(alias).unwrap().id, "Oric");
    }
    for alias in [
        "Telestrat",
        "Atmos",
        "Loriciels",
        "historic",
        "Oric collection",
    ] {
        assert!(platform_for_alias(alias).is_none(), "{alias}");
    }
}

#[test]
fn extensions_register_candidates_without_proving_oric() {
    let dir = Temp::new();
    for ext in ["tap", "dsk"] {
        assert!(archivefs_core::media_registry::kind_for_extension(ext).is_some());
        assert!(
            archivefs_core::ingestion::content_registry::content_kind_for_extension(ext).is_some()
        );
        let p = dir.put(&format!("unidentified.{ext}"), b"not valid media");
        let r = detect_platform_report(&DetectionRequest::new(&p, &dir.0).inspecting_content());
        assert_ne!(r.platform, Some("Oric"));
        assert!(!OricMediaDetector.detect(b"not valid media").is_recognized());
    }
}

#[test]
fn standard_tap_fields_and_shared_tape_analysis() {
    let bytes = tap(b"HEADER ONLY", 0x500, &[1, 2, 3]);
    let parsed = parse_oric_tap(&bytes).unwrap();
    let h = &parsed.blocks[0];
    assert_eq!(h.filename, "HEADER ONLY");
    assert_eq!(h.start_address, 0x500);
    assert_eq!(h.end_address, 0x502);
    assert_eq!(h.payload_length, 3);
    assert_eq!(h.program_kind, OricProgramKind::MachineCode);
    assert_eq!(h.auto_start, OricAutoStart::MachineCode);
    let analysis = analyze_tape(&bytes).unwrap();
    assert_eq!(analysis.format, TapeFormat::OricTap);
    assert_eq!(analysis.checksum, ChecksumState::NotPresent);
    assert_eq!(analysis.entries[0].load_address, Some(0x500));
    assert_eq!(
        observe_oric_media(&bytes).unwrap().machine_compatibility(),
        OricMachineCompatibility::Undetermined
    );
}

#[test]
fn basic_and_non_autostart_headers_and_multiple_segments() {
    let mut first = tap(b"", 0x501, &[1]);
    first[6] = 0;
    first[7] = 0x80;
    let mut second = tap(b"SECOND", 0xffff, &[2]);
    second[7] = 0;
    first.extend(second);
    let parsed = parse_oric_tap(&first).unwrap();
    assert_eq!(parsed.blocks.len(), 2);
    assert_eq!(parsed.blocks[0].program_kind, OricProgramKind::Basic);
    assert_eq!(parsed.blocks[0].auto_start, OricAutoStart::Basic);
    assert_eq!(parsed.blocks[1].auto_start, OricAutoStart::Disabled);
}

#[test]
fn tap_rejects_every_truncated_prefix() {
    let b = tap(b"TEST", 0x500, &[1, 2, 3]);
    for n in 0..b.len() {
        assert!(parse_oric_tap(&b[..n]).is_err(), "prefix {n}");
    }
}

#[test]
fn tap_rejects_bad_flags_addresses_names_and_trailing_bytes() {
    let b = tap(b"TEST", 0x500, &[1, 2, 3]);
    for (offset, value) in [
        (3, 0x23),
        (4, 1),
        (5, 1),
        (6, 7),
        (7, 7),
        (8, 0),
        (12, 1),
        (13, 0xff),
    ] {
        let mut bad = b.clone();
        bad[offset] = value;
        assert!(parse_oric_tap(&bad).is_err(), "offset {offset}");
    }
    assert!(parse_oric_tap(&tap(b"ABCDEFGHIJKLMNOPQ", 0x500, &[1])).is_err());
    let mut bad = b;
    bad.push(0);
    assert!(parse_oric_tap(&bad).is_err());
    assert_eq!(
        parse_oric_tap(&vec![0x16; MAX_ORIC_MEDIA_BYTES + 1]),
        Err(TapeAnalysisError::TooLarge)
    );
    assert_eq!(
        parse_oric_tap(&vec![0x16; MAX_ORIC_LEADER_BYTES + 1]),
        Err(TapeAnalysisError::TooLarge)
    );
}

#[test]
fn tap_has_no_checksum_and_payload_changes_do_not_fake_integrity() {
    let mut b = tap(b"TEST", 0x500, &[1, 2, 3]);
    *b.last_mut().unwrap() ^= 1;
    assert_eq!(
        parse_oric_tap(&b).unwrap().analysis().checksum,
        ChecksumState::NotPresent
    );
    assert!(parse_oric_tap(&tap(b"ABCDEFGHIJKLMNOP", 0, &[0])).is_ok());
}

#[test]
fn non_oric_tape_formats_are_never_promoted() {
    for b in [
        b"C64-TAPE-RAW".as_slice(),
        b"ZXTape!\x1a",
        &[2, 0, 0xff, 0xff],
    ] {
        assert!(!OricMediaDetector.detect(b).is_recognized());
    }
    assert_eq!(
        analyze_tape(&[2, 0, 0xff, 0xff]).unwrap().format,
        TapeFormat::ZxTap
    );
}

#[test]
fn mfm_geometry_and_all_sector_crcs_are_validated() {
    for sectors in 15..=18 {
        assert!(parse_oric_mfm(&mfm(1, 1, sectors)).is_ok());
    }
    let b = mfm(2, 2, 17);
    let original = b.clone();
    let d = parse_oric_mfm(&b).unwrap();
    assert_eq!(d.sides, 2);
    assert_eq!(d.tracks_per_side, 2);
    assert_eq!(d.sectors_per_track, 17);
    assert_eq!(d.sector_bytes, 256);
    assert_eq!(b, original);
    let r = fuse_platform_evidence(observe_oric_media(&b).unwrap().evidence());
    assert_eq!(r.resolved_platform, Some("Oric"));
}

#[test]
fn mfm_rejects_geometry_truncation_and_other_dsk_containers() {
    let b = mfm(1, 1, 17);
    for offset in [0, 8, 12, 16] {
        let mut bad = b.clone();
        bad[offset] = 0;
        assert!(parse_oric_mfm(&bad).is_err());
    }
    for (offset, value) in [(8, 3), (12, 255), (16, 2)] {
        let mut bad = b.clone();
        bad[offset] = value;
        assert!(parse_oric_mfm(&bad).is_err());
    }
    for n in [0, 7, 255, 256, b.len() - 1] {
        assert!(parse_oric_mfm(&b[..n]).is_err());
    }
    let mut extra = b;
    extra.push(0);
    assert!(parse_oric_mfm(&extra).is_err());
    for prefix in [
        b"MV - CPCEMU Disk-File\r\nDisk-Info\r\n".as_slice(),
        b"EXTENDED CPC DSK File\r\nDisk-Info\r\n",
        b"ORICDISK",
    ] {
        let mut other = vec![0u8; 256];
        other[..prefix.len()].copy_from_slice(prefix);
        assert!(parse_oric_mfm(&other).is_err());
        assert!(!OricMediaDetector.detect(&other).is_recognized());
    }
    assert!(parse_oric_mfm(&vec![0; MAX_ORIC_MFM_BYTES + 1]).is_err());
}

#[test]
fn mfm_rejects_sector_id_data_crc_and_nonstandard_sector_shapes() {
    let b = mfm(1, 1, 17);
    let id = 256 + 72;
    let data = id + 10 + 34;
    for off in [
        id + 4,
        id + 5,
        id + 6,
        id + 7,
        id + 8,
        data + 3,
        data + 4,
        data + 260,
    ] {
        let mut bad = b.clone();
        bad[off] ^= 1;
        assert!(parse_oric_mfm(&bad).is_err(), "{off}");
    }
}

#[test]
fn loose_detection_and_freshness_reuse_shared_primitives() {
    let dir = Temp::new();
    for (name, b) in [
        ("media.tap", tap(b"TEST", 0x500, &[1])),
        ("media.dsk", mfm(1, 1, 17)),
    ] {
        let p = dir.put(name, &b);
        let binding =
            inspect_oric_file(&p, &TrustedRoots::none(), &AtomicBool::new(false)).unwrap();
        binding
            .revalidate(&TrustedRoots::none(), &AtomicBool::new(false))
            .unwrap();
        let r = detect_platform_report(&DetectionRequest::new(&p, &dir.0).inspecting_content());
        assert_eq!(r.platform, Some("Oric"));
        let archive = archivefs_core::Archive::from_path(&p).unwrap();
        assert_eq!(archive.identity.platform.as_deref(), Some("Oric"));
        assert_eq!(
            archive.identity.platform_provenance,
            Some(archivefs_core::PlatformProvenance::RegistryDetector)
        );
        assert_eq!(fs::read(&p).unwrap(), b);
        if name.ends_with("dsk") {
            assert_eq!(
                inspect_disk_format(
                    &p,
                    &TrustedRoots::none(),
                    DiskFormatContext::default(),
                    None
                )
                .format,
                Some(DiskFormat::OricMfm)
            );
        }
        let mut changed = b;
        *changed.last_mut().unwrap() ^= 1;
        fs::write(&p, changed).unwrap();
        assert!(
            binding
                .revalidate(&TrustedRoots::none(), &AtomicBool::new(false))
                .is_err()
        );
    }
    let p = dir.put("unknown.bin", &tap(b"TEST", 0x500, &[1]));
    assert!(inspect_oric_file(&p, &TrustedRoots::none(), &AtomicBool::new(false)).is_ok());
    assert!(inspect_oric_file(&p, &TrustedRoots::none(), &AtomicBool::new(true)).is_err());
}

#[test]
fn source_same_size_same_time_changes_and_path_replacement_are_rejected() {
    let dir = Temp::new();
    let b = tap(b"TEST", 0x500, &[1]);
    let p = dir.put("game.tap", &b);
    let bound = inspect_oric_file(&p, &TrustedRoots::none(), &AtomicBool::new(false)).unwrap();
    let time = fs::metadata(&p).unwrap().modified().unwrap();
    let mut changed = b.clone();
    *changed.last_mut().unwrap() = 2;
    fs::write(&p, changed).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(time)
        .unwrap();
    assert!(
        bound
            .revalidate(&TrustedRoots::none(), &AtomicBool::new(false))
            .is_err()
    );
    fs::rename(&p, dir.0.join("old")).unwrap();
    fs::write(&p, b).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(time)
        .unwrap();
    assert!(
        bound
            .revalidate(&TrustedRoots::none(), &AtomicBool::new(false))
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn source_symlink_requires_existing_trusted_root_policy() {
    let dir = Temp::new();
    let p = dir.put("game.tap", &tap(b"TEST", 0x500, &[1]));
    let link = dir.0.join("linked.tap");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert!(inspect_oric_file(&link, &TrustedRoots::none(), &AtomicBool::new(false)).is_err());
    assert!(
        inspect_oric_file(
            &link,
            &TrustedRoots::from_paths([&dir.0]),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}

fn zip(path: &Path, entries: &[(&str, Vec<u8>)]) {
    let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        z.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        z.write_all(bytes).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn zip_dispatch_reads_complete_media_and_preserves_member_provenance() {
    let dir = Temp::new();
    let path = dir.0.join("misleading.zip");
    let mut large = tap(b"A", 0, &[1; 40000]);
    large.extend(tap(b"B", 0, &[2; 40000]));
    let disk = mfm(1, 40, 17);
    zip(
        &path,
        &[
            ("folder/a.tap", large.clone()),
            ("folder/b.dsk", disk.clone()),
        ],
    );
    let original = fs::read(&path).unwrap();
    let result =
        archivefs_core::archive_member_content_evidence::observe_zip_member_content(&path).unwrap();
    assert_eq!(result.archive_path, path);
    assert_eq!(result.members.len(), 2);
    for (index, member) in result.members.iter().enumerate() {
        assert_eq!(member.member_index, index);
        assert_eq!(member.member_name, ["folder/a.tap", "folder/b.dsk"][index]);
        assert_eq!(
            member.declared_size,
            [large.len(), disk.len()][index] as u64
        );
        assert_eq!(
            fuse_platform_evidence(member.evidence.clone()).resolved_platform,
            Some("Oric")
        );
    }
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn archive_prefix_and_budget_exhaustion_cannot_forge_complete_validation() {
    let detector = OricMediaDetector;
    let ds: [&dyn ContentDetector; 1] = [&detector];
    // First valid segment ends exactly at the old 64 KiB prefix boundary.
    let mut bytes = tap(b"", 0, &vec![0x55; 65536 - 14]);
    assert_eq!(bytes.len(), 65536);
    bytes.extend(b"malformed later content");
    for mut budget in [0usize, 65536] {
        let (_, result) = probe_content_stream(
            bytes.as_slice(),
            bytes.len() as u64,
            65536,
            &mut budget,
            &ds,
        )
        .unwrap();
        assert!(result.evidence.is_empty());
    }
    let mut budget = 0;
    let (_, result) =
        probe_content_stream(&bytes[..65536], 65537, 65536, &mut budget, &ds).unwrap();
    assert!(result.evidence.is_empty());
}

#[test]
fn dat_release_identity_is_hash_led_not_tape_filename() {
    use archivefs_core::dat::{
        audit::{AuditVerdict, KnownFileEvidence, audit_files},
        identity::identify_dat_source,
        index::DatIndex,
        limits::DatLimits,
        parsers::parse_dat_file,
    };
    let dir = Temp::new();
    let b = tap(b"FALSE TITLE", 0x500, &[1, 2, 3]);
    let p = dir.put("also-not-a-title.tap", &b);
    let inspected = inspect_oric_file(&p, &TrustedRoots::none(), &AtomicBool::new(false)).unwrap();
    let datpath = dir.put("catalogue.dat", format!("<datafile><header><name>Oric Atmos</name></header><game name=\"Verified release (France) (Rev 2)\"><rom name=\"different.tap\" size=\"{}\" sha256=\"{}\"/></game></datafile>", b.len(), inspected.hashes().sha256).as_bytes());
    let parsed = parse_dat_file(&datpath, DatLimits::default()).unwrap().dat;
    let dat_identity = identify_dat_source(&parsed);
    let result = inspect_identity(IdentityInspectionInput {
        content_evidence: inspected.observation().evidence(),
        dat: Some(dat_identity),
        ..Default::default()
    });
    assert!(!result.has_conflict());
    assert!(result.combined.unwrap().relationship.is_agreement());
    let index = DatIndex::build(&parsed);
    let no_hash = KnownFileEvidence::new("different.tap", "different.tap");
    assert!(
        !audit_files(&[no_hash], &index).entries[0]
            .verdict
            .is_confident()
    );
    let known = KnownFileEvidence::new(p.to_string_lossy(), "also-not-a-title.tap")
        .with_size(b.len() as u64)
        .with_sha256(&inspected.hashes().sha256);
    assert!(matches!(&audit_files(&[known], &index).entries[0].verdict,
        AuditVerdict::Exact { game_name, .. } if game_name == "Verified release (France) (Rev 2)"));
    assert!(
        inspected
            .observation()
            .evidence()
            .iter()
            .all(|e| e.kind != ContentEvidenceKind::ProductCode)
    );
}

#[test]
fn conflicting_strong_content_and_dat_fail_closed() {
    use archivefs_core::dat::identity::*;
    let mut evidence = observe_oric_media(&tap(b"TEST", 0x500, &[1]))
        .unwrap()
        .evidence();
    let dat = resolve_dat_platform_identity([DatPlatformEvidence {
        platform: "Saturn".into(),
        machine_key: None,
        kind: DatPlatformEvidenceKind::HeaderName,
        confidence: DatPlatformConfidence::Strong,
        detail: "synthetic catalogue".into(),
    }]);
    let result = inspect_identity(IdentityInspectionInput {
        content_evidence: evidence.clone(),
        dat: Some(dat),
        ..Default::default()
    });
    assert!(result.has_conflict());
    evidence.push(ContentEvidence::new(
        ContentEvidenceKind::BootStructure,
        "SEGA SEGASATURN",
        ContentEvidenceConfidence::Strong,
        "synthetic conflict",
    ));
    let result = fuse_platform_evidence(evidence);
    assert_eq!(result.outcome, FusionOutcome::Conflict);
    assert!(result.resolved_platform.is_none());
}

#[test]
fn registry_parity_reports_existing_media_routes() {
    for ext in ["tap", "dsk"] {
        let row = archivefs_core::registry_parity::media_registry_parity()
            .into_iter()
            .find(|r| r.extension == ext)
            .unwrap();
        assert!(row.media_kind.is_some());
        assert!(row.content_kind.is_some());
        assert!(row.identity_dispatched);
        assert!(!row.claiming_platforms.contains(&"Oric"));
    }
}

#[test]
fn malformed_detector_retains_no_strong_fact() {
    assert!(
        matches!(OricMediaDetector.detect(b"\x16\x16\x16\x24"), ContentDetectionOutcome::Malformed { evidence, .. } if evidence.is_empty())
    );
}

#[test]
fn zx_tap_with_near_miss_oric_leader_keeps_existing_semantics() {
    let mut zx = vec![0x16, 0x16];
    zx.extend(vec![0x16; 0x1616 - 1]);
    let xor = zx[2..].iter().fold(0, |sum, b| sum ^ b);
    zx.push(xor);
    assert_eq!(analyze_tape(&zx).unwrap().format, TapeFormat::ZxTap);
    assert!(!OricMediaDetector.detect(&zx).is_recognized());
}

#[test]
fn archive_declared_size_is_not_a_substitute_for_eof() {
    let bytes = tap(b"", 0, &vec![0x55; 65536 - 14]);
    let mut extra = bytes.clone();
    extra.push(0);
    let ds: [&dyn ContentDetector; 1] = [&OricMediaDetector];
    let mut budget = 10;
    assert!(probe_content_stream(extra.as_slice(), 65536, 65536, &mut budget, &ds).is_err());
    let mut budget = 10;
    let (_, result) =
        probe_content_stream(bytes.as_slice(), 65536, 65536, &mut budget, &ds).unwrap();
    assert_eq!(
        fuse_platform_evidence(result.evidence).resolved_platform,
        Some("Oric")
    );
}

#[test]
fn corrupt_complete_member_reads_still_consume_archive_budget() {
    struct BadCrc(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for BadCrc {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = std::io::Read::read(&mut self.0, out)?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "synthetic decoder CRC failure",
                ));
            }
            Ok(n)
        }
    }
    let mut bytes = tap(b"ONE", 0, &vec![0; 65536]);
    bytes.extend(tap(b"TWO", 0, &[1, 2, 3]));
    let mut budget = bytes.len() - 65536 + 1;
    let result = probe_content_stream(
        BadCrc(std::io::Cursor::new(bytes.clone())),
        bytes.len() as u64,
        65536,
        &mut budget,
        &[&OricMediaDetector],
    );
    assert!(result.is_err());
    assert_eq!(budget, 1, "failed reads must charge every returned byte");
    let (_, subsequent) = probe_content_stream(
        bytes.as_slice(),
        bytes.len() as u64,
        65536,
        &mut budget,
        &[&OricMediaDetector],
    )
    .unwrap();
    assert!(subsequent.evidence.is_empty());
}

#[test]
fn sevenz_uses_the_same_complete_observer_and_provenance() {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let dir = Temp::new();
    let path = dir.0.join("fixture.7z");
    let bytes = mfm(1, 40, 17);
    let mut writer = ArchiveWriter::new(fs::File::create(&path).unwrap()).unwrap();
    let mut entry = ArchiveEntry::new_file("nested/media.dsk");
    entry.size = bytes.len() as u64;
    writer
        .push_archive_entry(entry, Some(std::io::Cursor::new(&bytes)))
        .unwrap();
    writer.finish().unwrap();
    let original = fs::read(&path).unwrap();
    let result = archivefs_core::archive_member_content_evidence::observe_sevenz_member_content(
        &path,
        &TrustedRoots::none(),
        archivefs_core::dat::archive::limits::ArchiveLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.members.len(), 1);
    let m = &result.members[0];
    assert_eq!(m.member_index, 0);
    assert_eq!(m.member_name, "nested/media.dsk");
    assert_eq!(m.declared_size, bytes.len() as u64);
    assert_eq!(
        fuse_platform_evidence(m.evidence.clone()).resolved_platform,
        Some("Oric")
    );
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn tap_segment_limit_is_enforced_without_retaining_partial_identity() {
    let block = tap(b"", 0, &[0]);
    let bytes = block.repeat(archivefs_core::tape_analysis::MAX_ANALYSIS_ENTRIES + 1);
    assert_eq!(parse_oric_tap(&bytes), Err(TapeAnalysisError::TooLarge));
    assert!(OricMediaDetector.detect(&bytes).evidence().is_empty());
}

#[test]
fn mame_dat_software_list_alias_reuses_canonical_resolution() {
    let dir = Temp::new();
    let p = dir.put("synthetic.xml", b"<softwarelist name=\"oric1_cass\" description=\"Tangerine Oric-1 cassettes\"><software name=\"synthetic\"><description>Synthetic</description><year>2026</year><publisher>Test</publisher><part name=\"cass\" interface=\"oric1_cass\"><dataarea name=\"cass\" size=\"1\"><rom name=\"x.tap\" size=\"1\" crc=\"00000000\"/></dataarea></part></software></softwarelist>");
    let parsed = archivefs_core::dat::parsers::parse_dat_file(
        &p,
        archivefs_core::dat::limits::DatLimits::default(),
    )
    .unwrap();
    use archivefs_core::dat::identity::{
        DatPlatformConfidence, DatPlatformIdentity, gather_dat_platform_evidence,
        identify_dat_source,
    };
    let evidence = gather_dat_platform_evidence(&parsed.dat);
    assert!(evidence.iter().any(|e| e.platform == "Oric"
        && e.machine_key.as_deref() == Some("oric1_cass")
        && e.confidence == DatPlatformConfidence::Weak));
    // Existing MAME namespace hardening must not be weakened for Oric.
    assert_eq!(
        identify_dat_source(&parsed.dat),
        DatPlatformIdentity::Unknown
    );
}
