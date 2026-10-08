//! Family-scoped MAME reads must be exact restrictions of the full ones, and a
//! republish of unchanged member evidence must write nothing while any real
//! difference is still written.

archivefs_core::install_test_environment!();

use std::path::{Path, PathBuf};

use archivefs_core::Database;
use archivefs_core::dat::mame_arcade_join::{
    ArcadeJoinClass, ArcadeJoinEvidence, ArcadeJoinReport, ArcadeJoinSummary, ArcadeMemberEvidence,
    MAME_MEMBER_EVIDENCE_VERSION, MamePhysicalMemberEvidence, MemberEvidenceKind,
};

const DIGEST: &str = "digest-0174";
const SOURCE_ID: &str = "mame-arcade:0.174:digest-0174";

fn evidence(name: &str, clone_of: Option<&str>, digest: &str) -> ArcadeJoinEvidence {
    ArcadeJoinEvidence {
        logical_set_name: name.into(),
        dat_set_name: Some(name.into()),
        class: ArcadeJoinClass::ExactSetMatch,
        description: Some(name.into()),
        manufacturer: None,
        year: None,
        clone_of: clone_of.map(str::to_string),
        rom_of: clone_of.map(str::to_string),
        parent_description: None,
        runnable: Some("yes".into()),
        mechanical: false,
        is_bios: false,
        is_device: false,
        expected_member_count: 1,
        members: vec![ArcadeMemberEvidence {
            name: "target.bin".into(),
            kind: MemberEvidenceKind::Missing,
            current_name: None,
            checksum: Some("a".repeat(40)),
            observed_sha1: None,
            observed_crc32: None,
        }],
        dependencies: Vec::new(),
        launchable_normal_game: true,
        dat_version: "0.174".into(),
        dat_sha256: digest.into(),
        dat_path: "mame-0.174.dat".into(),
        audited_at: "test".into(),
    }
}

fn database(dir: &Path, sets: &[(&str, Option<&str>)]) -> Database {
    let report = ArcadeJoinReport {
        dat_path: PathBuf::from("mame-0.174.dat"),
        dat_sha256: DIGEST.into(),
        dat_version: "0.174".into(),
        dat_machine_count: sets.len(),
        scan_root: PathBuf::from("arcade"),
        evidence: sets
            .iter()
            .map(|(name, clone_of)| evidence(name, *clone_of, DIGEST))
            .collect(),
        summary: ArcadeJoinSummary::default(),
        layout_estimate: "split".into(),
    };
    let mut db = Database::open_or_create(dir.join("library.sqlite3")).unwrap();
    db.persist_mame_arcade_join(&report).unwrap();
    db
}

fn names(joins: &[(PathBuf, ArcadeJoinEvidence)]) -> Vec<String> {
    joins
        .iter()
        .map(|(_, e)| e.dat_set_name.clone().unwrap())
        .collect()
}

const SETS: &[(&str, Option<&str>)] = &[
    ("puck", None),
    ("pacm", Some("puck")),
    ("other", None),
    ("otherb", Some("other")),
    ("zzz", None),
];

#[test]
fn scoped_joins_are_an_exact_ordered_restriction_of_the_full_load() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path(), SETS);
    let full = db.mame_arcade_join_paths_for_dat(DIGEST).unwrap();
    assert_eq!(full.len(), SETS.len());
    assert_eq!(
        db.mame_arcade_join_paths_for_dat_sets(DIGEST, None)
            .unwrap(),
        full
    );
    for subset in [
        vec!["pacm", "puck"],
        vec!["zzz"],
        vec!["otherb", "other", "puck"],
        vec!["absent"],
        vec!["puck", "pacm", "other", "otherb", "zzz"],
    ] {
        let wanted: Vec<String> = subset.iter().map(|s| s.to_string()).collect();
        let expected: Vec<_> = full
            .iter()
            .filter(|(_, e)| wanted.contains(e.dat_set_name.as_ref().unwrap()))
            .cloned()
            .collect();
        let scoped = db
            .mame_arcade_join_paths_for_dat_sets(DIGEST, Some(&wanted))
            .unwrap();
        assert_eq!(scoped, expected, "{subset:?}");
    }
    assert!(
        db.mame_arcade_join_paths_for_dat_sets(DIGEST, Some(&[]))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn scoping_keeps_the_digest_and_staleness_trust_filters() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = database(dir.path(), SETS);
    let family: Vec<String> = vec!["puck".into(), "pacm".into()];
    assert_eq!(
        names(
            &db.mame_arcade_join_paths_for_dat_sets(DIGEST, Some(&family))
                .unwrap()
        )
        .len(),
        2
    );
    // A different DAT digest is never an implicit fallback.
    assert!(
        db.mame_arcade_join_paths_for_dat_sets("other-digest", Some(&family))
            .unwrap()
            .is_empty()
    );
    // Rows marked stale (for example after a source replacement) are not trusted.
    db.mark_dat_set_results_stale_for_source(SOURCE_ID).unwrap();
    assert!(
        db.mame_arcade_join_paths_for_dat_sets(DIGEST, Some(&family))
            .unwrap()
            .is_empty()
    );
    assert!(
        db.mame_arcade_join_paths_for_dat(DIGEST)
            .unwrap()
            .is_empty()
    );
}

fn row(set: &str, name: &str, observed_at: &str) -> MamePhysicalMemberEvidence {
    MamePhysicalMemberEvidence {
        logical_set_name: set.into(),
        source_path: PathBuf::from(format!("/arcade/{set}/{name}")),
        current_name: name.into(),
        file_size: 10,
        modified_time_ns: 5,
        sha1: Some("a".repeat(40)),
        crc32: Some("b".repeat(8)),
        target_set_name: Some(set.into()),
        target_member_name: Some("target.bin".into()),
        actionable: true,
        failure_reason: None,
        evidence_version: MAME_MEMBER_EVIDENCE_VERSION.into(),
        observed_at: observed_at.into(),
    }
}

struct Probe(rusqlite::Connection);

impl Probe {
    fn open(dir: &Path) -> Self {
        Self(rusqlite::Connection::open(dir.join("library.sqlite3")).unwrap())
    }
    fn mark(&self, set: &str) {
        self.0
            .execute(
                "UPDATE dat_expected_entries SET updated_at = 'sentinel'
                 WHERE dat_source_id = ?1 AND canonical_identity = ?2",
                rusqlite::params![SOURCE_ID, set],
            )
            .unwrap();
    }
    fn updated_at(&self, set: &str) -> String {
        self.0
            .query_row(
                "SELECT updated_at FROM dat_expected_entries
                 WHERE dat_source_id = ?1 AND canonical_identity = ?2",
                rusqlite::params![SOURCE_ID, set],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn metadata(&self, set: &str) -> Vec<u8> {
        self.0
            .query_row(
                "SELECT metadata_json FROM dat_expected_entries
                 WHERE dat_source_id = ?1 AND canonical_identity = ?2",
                rusqlite::params![SOURCE_ID, set],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn evidence_rows(&self, set: &str) -> usize {
        self.0
            .query_row(
                "SELECT COUNT(*) FROM mame_member_evidence
                 WHERE dat_source_id = ?1 AND logical_set_name = ?2",
                rusqlite::params![SOURCE_ID, set],
                |r| r.get::<_, i64>(0),
            )
            .unwrap() as usize
    }
}

#[test]
fn an_unchanged_republish_writes_nothing_and_any_difference_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = database(dir.path(), &[("puck", None)]);
    let probe = Probe::open(dir.path());
    let rows = vec![row("puck", "wrong.bin", "t1")];

    // First publish writes the rows and derives the join's member evidence.
    assert_eq!(
        db.persist_mame_member_evidence_set(SOURCE_ID, "puck", &rows)
            .unwrap(),
        1
    );
    let merged = probe.metadata("puck");
    let stored: ArcadeJoinEvidence = serde_json::from_slice(&merged).unwrap();
    assert_eq!(stored.members[0].kind, MemberEvidenceKind::Present);
    assert_eq!(stored.members[0].current_name.as_deref(), Some("wrong.bin"));

    // Identical republish: same answer, no write (the sentinel survives).
    probe.mark("puck");
    assert_eq!(
        db.persist_mame_member_evidence_set(SOURCE_ID, "puck", &rows)
            .unwrap(),
        1
    );
    assert_eq!(probe.updated_at("puck"), "sentinel");
    assert_eq!(probe.metadata("puck"), merged);

    // A changed observation is written.
    let mut changed = rows.clone();
    changed[0].observed_at = "t2".into();
    db.persist_mame_member_evidence_set(SOURCE_ID, "puck", &changed)
        .unwrap();
    assert_ne!(probe.updated_at("puck"), "sentinel");

    // The join rewritten without its member evidence is repaired by the same rows.
    probe.mark("puck");
    probe
        .0
        .execute(
            "UPDATE dat_expected_entries SET metadata_json = ?3
             WHERE dat_source_id = ?1 AND canonical_identity = ?2",
            rusqlite::params![
                SOURCE_ID,
                "puck",
                serde_json::to_vec(&evidence("puck", None, DIGEST)).unwrap()
            ],
        )
        .unwrap();
    db.persist_mame_member_evidence_set(SOURCE_ID, "puck", &changed)
        .unwrap();
    assert_ne!(probe.updated_at("puck"), "sentinel");
    assert_eq!(
        serde_json::from_slice::<ArcadeJoinEvidence>(&probe.metadata("puck")).unwrap(),
        stored
    );

    // A leftover stored row the new set no longer has is retired, not kept.
    probe
        .0
        .execute(
            "INSERT INTO mame_member_evidence
             (dat_source_id, logical_set_name, source_path, current_name, file_size,
              modified_time_ns, sha1, crc32, target_set_name, target_member_name,
              actionable, failure_reason, evidence_version, observed_at)
             VALUES (?1, 'puck', X'2f676f6e65', 'gone.bin', 1, 1, NULL, NULL, NULL, NULL,
                     0, 'x', ?2, 't')",
            rusqlite::params![SOURCE_ID, MAME_MEMBER_EVIDENCE_VERSION],
        )
        .unwrap();
    assert_eq!(probe.evidence_rows("puck"), 2);
    db.persist_mame_member_evidence_set(SOURCE_ID, "puck", &changed)
        .unwrap();
    assert_eq!(probe.evidence_rows("puck"), 1);

    // A set whose rows all go away is still emptied.
    db.persist_mame_member_evidence_set(SOURCE_ID, "puck", &[])
        .unwrap();
    assert_eq!(probe.evidence_rows("puck"), 0);
}

#[test]
fn row_order_does_not_make_a_republish_a_difference() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = database(dir.path(), &[("puck", None)]);
    let probe = Probe::open(dir.path());
    let rows = vec![row("puck", "a.bin", "t"), row("puck", "b.bin", "t")];
    // Two actionable rows for one target make the member ambiguous; the stored
    // evidence is cleared, and the same on every republish.
    db.persist_mame_member_evidence_set(SOURCE_ID, "puck", &rows)
        .unwrap();
    probe.mark("puck");
    let reversed: Vec<_> = rows.iter().rev().cloned().collect();
    db.persist_mame_member_evidence_set(SOURCE_ID, "puck", &reversed)
        .unwrap();
    assert_eq!(probe.updated_at("puck"), "sentinel");
    assert_eq!(probe.evidence_rows("puck"), 2);
}
