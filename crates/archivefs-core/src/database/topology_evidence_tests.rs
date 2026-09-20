use super::*;
use crate::media_set::*;
use std::fs;
use tempfile::tempdir;

fn archive_for(path: &Path) -> PersistedArchive {
    let metadata = fs::metadata(path).unwrap();
    PersistedArchive {
        id: 1,
        source_folder_id: 1,
        relative_path: path.file_name().unwrap().into(),
        absolute_path: path.to_path_buf(),
        archive_kind: "loose_file".into(),
        display_name: path.file_name().unwrap().to_string_lossy().into(),
        normalized_name: "orbit quest".into(),
        size_bytes: Some(metadata.len()),
        modified_time_unix_seconds: Some(
            metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
        ),
        platform: Some("PlayStation".into()),
        platform_source: Some("test".into()),
        last_known_health: "Healthy".into(),
        last_seen_at: "now".into(),
        last_verified_missing_at: None,
        identity_report: None,
    }
}

fn report(path: &Path, trusted: bool) -> TopologyReport {
    let mut record = media_record(path, Some("PlayStation"));
    record.availability = MediaAvailability::Observed;
    if trusted {
        let mut evidence = MediaEvidence::new(EvidenceKind::TrustedDat, "test-topology");
        evidence.provenance.version = Some("1".into());
        evidence.release = Some(IdentityKey::new("test-release", "orbit"));
        evidence.medium = Some(IdentityKey::new("test-medium", "disc-1"));
        evidence.equivalence = Equivalence::AuthorityMapping;
        evidence.ordinal = Some(MediaOrdinal {
            number: 1,
            unit: OrdinalUnit::Disc,
        });
        evidence.expected_count = Some(ExpectedCount {
            count: 1,
            unit: OrdinalUnit::Disc,
        });
        record.evidence.push(evidence);
    }
    TopologyReport {
        sets: resolve_index(index_media(vec![record])).sets,
        stats: TopologyStats::default(),
    }
}

#[test]
fn trusted_topology_persists_reloads_and_invalidates_on_source_change() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orbit.chd");
    fs::write(&path, b"disc").unwrap();
    let db_path = dir.path().join("library.sqlite3");
    let mut database = Database::open_or_create(&db_path).unwrap();
    let report = report(&path, true);
    assert_eq!(
        database
            .persist_media_topology_evidence(&report, "test-topology", "1", 7)
            .unwrap(),
        1
    );
    let reopened = Database::open_read_only(&db_path).unwrap();
    let current = vec![archive_for(&path)];
    let sets = reopened.load_media_topology_evidence(&current).unwrap();
    assert_eq!(
        sets[0].members[0].representations[0]
            .ordinal
            .as_ref()
            .unwrap()
            .number,
        1
    );
    assert!(
        sets[0].members[0].representations[0]
            .record
            .evidence
            .iter()
            .any(|e| {
                e.provenance.kind == EvidenceKind::TrustedDat
                    && e.provenance.version.as_deref() == Some("1")
            })
    );
    fs::write(&path, b"changed source identity").unwrap();
    assert!(
        reopened
            .load_media_topology_evidence(&[archive_for(&path)])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn filename_only_and_conflicting_topology_are_not_persisted() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orbit (Disc 1).chd");
    fs::write(&path, b"disc").unwrap();
    let mut database = Database::open_or_create(dir.path().join("library.sqlite3")).unwrap();
    assert_eq!(
        database
            .persist_media_topology_evidence(&report(&path, false), "test", "1", 1)
            .unwrap(),
        0
    );
    let mut conflicting = report(&path, true);
    conflicting.sets[0].state = MediaSetState::ConflictingSet;
    assert_eq!(
        database
            .persist_media_topology_evidence(&conflicting, "test", "1", 1)
            .unwrap(),
        0
    );
}
