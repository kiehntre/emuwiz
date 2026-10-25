//! RZX gameplay recordings are persisted as visible preservation-only library
//! records: never a game image, never a launch input, never silently dropped,
//! and stable across a database reopen and rescan.
archivefs_core::install_test_environment!();

use archivefs_core::ingestion::{ContentKind, SkipReason, ValidationState};
use archivefs_core::launch::{
    CanonicalIdentityStatus, LaunchContainerKind, LaunchContentRef, LaunchReadiness,
    ResolvedIdentity, build_launch_plan,
};
use archivefs_core::{ArchiveKind, Config, Database, scan_and_persist};
use archivefs_core::{DiscoveryDetailFilter, DiscoveryRunId, PreservationFreshness};
use std::path::Path;

fn rzx(major: u8, flags: u32, input: bool) -> Vec<u8> {
    let mut b = b"RZX!".to_vec();
    b.extend_from_slice(&[major, 13]);
    b.extend_from_slice(&flags.to_le_bytes());
    let mut push = |id: u8, body: &[u8]| {
        b.push(id);
        b.extend_from_slice(&((body.len() + 5) as u32).to_le_bytes());
        b.extend_from_slice(body);
    };
    push(0x10, &[0; 24]);
    if flags & 1 != 0 {
        push(0x20, &[0; 8]);
    }
    if input {
        push(0x80, &[0; 13]);
    }
    b
}
fn pzx() -> Vec<u8> {
    let mut b = b"PZXT\x02\0\0\0\x01\0".to_vec();
    b.extend_from_slice(b"DATA");
    b.extend_from_slice(&9u32.to_le_bytes());
    b.extend_from_slice(&[8, 0, 0, 0, 0, 0, 0, 0, 0xFF]);
    b
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn config(root: &Path, mount: &Path) -> Config {
    Config {
        source_folders: vec![root.into()],
        mount_root: mount.into(),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    }
}

#[test]
fn rzx_recordings_persist_as_preservation_records_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("incoming");
    std::fs::create_dir(&source).unwrap();
    let mut cut = rzx(0, 0, true);
    cut.pop();
    let fixtures: Vec<(&str, Vec<u8>)> = vec![
        ("Manic Miner.rzx", rzx(0, 0, true)),
        ("Manic Miner.pzx", pzx()),
        ("signed.rzx", rzx(0, 1, true)),
        ("cut.rzx", cut),
        ("future.rzx", rzx(1, 0, true)),
        ("empty.rzx", rzx(0, 0, false)),
    ];
    for (name, bytes) in &fixtures {
        std::fs::write(source.join(name), bytes).unwrap();
    }
    let config = config(&source, &dir.path().join("mount"));
    let db_path = dir.path().join("library.sqlite3");
    let mut database = Database::open_or_create(&db_path).unwrap();
    let first = scan_and_persist(&mut database, &config, "rzx-first").unwrap();

    let kinds = |database: &Database| {
        let mut rows: Vec<_> = database
            .load_archives()
            .unwrap()
            .into_iter()
            .map(|a| {
                (
                    a.relative_path.to_string_lossy().into_owned(),
                    a.archive_kind.clone(),
                    a.platform.clone(),
                    a.id,
                )
            })
            .collect();
        rows.sort();
        rows
    };
    let before = kinds(&database);
    let summary: Vec<_> = before
        .iter()
        .map(|(name, kind, platform, _)| (name.as_str(), kind.as_str(), platform.as_deref()))
        .collect();
    assert_eq!(
        summary,
        [
            ("Manic Miner.pzx", "direct_game_image", Some("ZX Spectrum")),
            (
                "Manic Miner.rzx",
                "preservation_recording",
                Some("ZX Spectrum")
            ),
            ("signed.rzx", "preservation_recording", Some("ZX Spectrum")),
        ]
    );
    assert!(
        ArchiveKind::from_storage("preservation_recording")
            .is_some_and(|kind| kind.is_preservation_only() && !kind.is_mount_input())
    );

    // Scanner accounting keeps an unsupported variant apart from a corrupt
    // recording, and counts the kept recordings as recordings, not games.
    let scanner_reason = |name: &str| {
        first
            .skipped_files
            .iter()
            .find(|f| f.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} not in skipped files"))
            .reason
    };
    assert_eq!(
        scanner_reason("future.rzx"),
        archivefs_core::SkipReason::UnsupportedVariant
    );
    assert_eq!(
        scanner_reason("cut.rzx"),
        archivefs_core::SkipReason::InvalidContent
    );
    assert_eq!(
        scanner_reason("empty.rzx"),
        archivefs_core::SkipReason::InvalidContent
    );
    // 5 RZX candidates: "recordings" counts only the 2 kept as Library
    // entries; the 3 refused ones are counted as refusals, never as kept, and
    // none counts as a game.
    assert_eq!(first.ingestion_stats.recordings, 2);
    assert_eq!(
        first.ingestion_skip_reasons.invalid_content
            + first.ingestion_skip_reasons.unsupported_content,
        3
    );
    assert_eq!(first.ingestion_skip_reasons.invalid_content, 2);
    assert_eq!(first.ingestion_skip_reasons.unsupported_content, 1);
    assert_eq!(
        first.ingestion_stats.loose_roms + first.ingestion_stats.snapshots,
        0
    );
    let recordings = database
        .query_discovery_details(
            first.scan_run_id as DiscoveryRunId,
            0,
            100,
            DiscoveryDetailFilter::PreservationOnly,
        )
        .unwrap();
    // Two kept, plus the three refused ones still classed as recordings.
    assert_eq!(recordings.total_matching, 5);

    // Every RZX is accounted for in the persisted discovery details, with a
    // reason for each one that was not kept.
    let page = database
        .query_discovery_details(
            first.scan_run_id as DiscoveryRunId,
            0,
            100,
            DiscoveryDetailFilter::All,
        )
        .unwrap();
    let detail = |name: &str| {
        page.rows
            .iter()
            .find(|row| row.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} not in discovery details"))
    };
    let kept = detail("Manic Miner.rzx");
    assert_eq!(kept.validation_state, ValidationState::Accepted);
    assert_eq!(kept.content, Some(ContentKind::GameplayRecording));
    assert!(
        kept.explanation
            .contains("Spectrum gameplay recording (preservation-only)")
    );
    assert!(kept.explanation.contains("Play is not offered"));
    assert!(detail("signed.rzx").explanation.contains("Protected"));
    assert!(matches!(
        detail("cut.rzx").skip_reason,
        Some(SkipReason::InvalidContent(_))
    ));
    assert!(matches!(
        detail("future.rzx").skip_reason,
        Some(SkipReason::UnsupportedContent(_))
    ));
    assert!(
        detail("future.rzx")
            .explanation
            .contains("Unsupported variant")
    );
    assert!(
        detail("empty.rzx")
            .explanation
            .contains("no input-recording block")
    );

    // The import-time fingerprint is the SHA-256 of the exact bytes, recorded
    // by the scan itself (no DAT verification ran), and only for recordings.
    assert!(first.preservation_fingerprint_warnings.is_empty());
    let id_of = |rows: &[(String, String, Option<String>, i64)], name: &str| {
        rows.iter().find(|row| row.0 == name).unwrap().3
    };
    let recording_id = id_of(&before, "Manic Miner.rzx");
    let signed_id = id_of(&before, "signed.rzx");
    let expected = sha256_hex(&rzx(0, 0, true));
    assert_eq!(
        database.preservation_fingerprint(recording_id).unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        database.preservation_fingerprint(signed_id).unwrap(),
        Some(sha256_hex(&rzx(0, 1, true)))
    );
    assert_eq!(
        database
            .preservation_fingerprint(id_of(&before, "Manic Miner.pzx"))
            .unwrap(),
        None
    );

    // Restart: reopen the database and rescan through the fingerprint cache.
    drop(database);
    let mut database = Database::open_or_create(&db_path).unwrap();
    assert_eq!(kinds(&database), before);
    assert_eq!(
        database.preservation_fingerprint(recording_id).unwrap(),
        Some(expected.clone())
    );
    let again = scan_and_persist(&mut database, &config, "rzx-restart").unwrap();
    let kept = |database: &Database| {
        database
            .load_archives()
            .unwrap()
            .iter()
            .filter(|a| a.archive_kind == "preservation_recording")
            .count()
    };
    assert_eq!(kept(&database), 2);
    assert_eq!(again.ingestion_stats.recordings, kept(&database));
    assert_eq!(again.counts.archives_missing, 0);
    assert_eq!(again.counts.archives_added, 0);
    assert_eq!(kinds(&database), before);
    assert_eq!(
        database
            .verify_preservation_fingerprint(recording_id)
            .unwrap(),
        PreservationFreshness::Fresh {
            sha256: expected.clone()
        }
    );

    // Same size, mtime put back: the scan cannot see it, freshness can, and
    // the import-time fingerprint is not silently re-recorded.
    let path = source.join("Manic Miner.rzx");
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut tampered = rzx(0, 0, true);
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    std::fs::write(&path, &tampered).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    scan_and_persist(&mut database, &config, "rzx-tampered").unwrap();
    assert_eq!(
        database.preservation_fingerprint(recording_id).unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        database
            .verify_preservation_fingerprint(recording_id)
            .unwrap(),
        PreservationFreshness::Changed {
            recorded: expected.clone(),
            current: sha256_hex(&tampered),
        }
    );

    // A visible change (new size) clears the old fingerprint on upsert and the
    // scan records one for the new bytes.
    let mut grown = rzx(0, 0, true);
    grown.extend_from_slice(&[0x80, 10, 0, 0, 0, 0, 0, 0, 0, 0]);
    std::fs::write(&path, &grown).unwrap();
    let changed = scan_and_persist(&mut database, &config, "rzx-grown").unwrap();
    assert_eq!(changed.counts.archives_changed, 1);
    assert_eq!(
        database.preservation_fingerprint(recording_id).unwrap(),
        Some(sha256_hex(&grown))
    );

    // Missing, then back with different bytes: the restored row must not
    // keep the hash of the bytes it had before it went missing.
    std::fs::remove_file(&path).unwrap();
    let gone = scan_and_persist(&mut database, &config, "rzx-missing").unwrap();
    assert_eq!(gone.counts.archives_missing, 1);
    let mut returned = grown.clone();
    returned.extend_from_slice(&[0x80, 7, 0, 0, 0, 1, 2]);
    std::fs::write(&path, &returned).unwrap();
    let back = scan_and_persist(&mut database, &config, "rzx-restored").unwrap();
    assert_eq!(back.counts.archives_restored, 1);
    assert!(back.preservation_fingerprint_warnings.is_empty());
    assert_eq!(
        database.preservation_fingerprint(recording_id).unwrap(),
        Some(sha256_hex(&returned))
    );

    // Gone: no current fingerprint is claimed.
    std::fs::remove_file(source.join("signed.rzx")).unwrap();
    assert!(matches!(
        database.verify_preservation_fingerprint(signed_id).unwrap(),
        PreservationFreshness::Unavailable { .. }
    ));

    // Even with a resolved identity, a recording is never a launch input.
    let recording = source.join("Manic Miner.rzx");
    let plan = build_launch_plan(
        &CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "ZX Spectrum".into(),
            game_key: "manic-miner".into(),
        }),
        &LaunchContentRef {
            kind: None,
            container: Some(LaunchContainerKind::PlainFile),
            resolved_path: Some(recording),
            requires_mount: false,
            provenance: "persisted catalogue".into(),
        },
        &[],
        &archivefs_core::emulator_environment::retroarch::RetroArchEnvironmentReport {
            format_version: 1,
            profiles: vec![],
            diagnostics: vec![],
        },
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Blocked);
    assert!(
        plan.candidates[0].blockers[0]
            .detail
            .contains("preservation-only")
    );

    // Scanning, fingerprinting and freshness never wrote to the files this
    // test did not rewrite or remove itself.
    for (name, bytes) in fixtures {
        if matches!(name, "Manic Miner.rzx" | "signed.rzx") {
            continue;
        }
        assert_eq!(
            std::fs::read(source.join(name)).unwrap(),
            bytes,
            "{name} changed"
        );
    }
}
