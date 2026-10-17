use super::*;
use archivefs_core::ingestion::{
    bounded_import::{apply_bounded_import, begin_bounded_preview, prepare_bounded_import},
    preview::{SpectrumCatalogue, preview_media},
};
use archivefs_core::{Config, Database};

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    database: PathBuf,
    config: PathBuf,
    dat: PathBuf,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("zxs");
        std::fs::create_dir(&root).unwrap();
        let database = temp.path().join("copy.sqlite3");
        let config = temp.path().join("config.toml");
        archivefs_core::add_source_folder_at(&config, &database, &root).unwrap();
        let mut db = Database::open_or_create(&database).unwrap();
        let source = db.list_source_folders().unwrap().remove(0);
        let review = db.review_source_rebind(source.id).unwrap();
        db.confirm_source_rebind(&review).unwrap();
        for n in 0..count {
            let mut bytes = vec![0; 49182];
            bytes[7] = 128;
            bytes[29] = 1;
            bytes[100..108].copy_from_slice(&(n as u64).to_le_bytes());
            std::fs::write(root.join(format!("{n:03}.z80")), bytes).unwrap();
        }
        std::fs::write(root.join("readme.txt"), b"Excluded support document").unwrap();
        let dat = temp.path().join("fixture.dat");
        std::fs::write(&dat, "<datafile><header><name>Sinclair ZX Spectrum - Games - [Z80]</name></header><game name=\"Unrelated catalogue entry\"><rom name=\"unrelated\" size=\"1\" sha1=\"0000000000000000000000000000000000000000\"/></game></datafile>").unwrap();
        Self {
            temp,
            root,
            database,
            config,
            dat,
        }
    }
    fn discover(&self) -> MediaPreview {
        let db = Database::open_read_only(&self.database).unwrap();
        let catalogue = SpectrumCatalogue::from_parsed(
            &archivefs_core::dat::parsers::parse_dat_file(&self.dat, Default::default())
                .unwrap()
                .dat,
        )
        .unwrap();
        let config = Config {
            source_folders: vec![self.root.clone()],
            mount_root: self.temp.path().join("mount"),
            ratarmount_bin: String::new(),
            master_rom_root: None,
        };
        let mut report = preview_media(
            &config,
            &[],
            &db.load_archives().unwrap(),
            &[catalogue],
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();
        report.bounded_import = Some(
            prepare_bounded_import(&report, begin_bounded_preview(&db, &self.config).unwrap())
                .unwrap(),
        );
        report
    }
    fn prepare(&self, journey: &mut Journey) -> SubsetPreview {
        journey
            .prepare_first(
                &self.database,
                &self.config,
                &AtomicBool::new(false),
                &mut |_, _| {},
            )
            .unwrap()
            .unwrap()
    }
}

#[test]
fn automatic_batches_resume_from_committed_receipts_without_replaying_or_duplicating_records() {
    let f = Fixture::new(205);
    let before = std::fs::read(&f.database).unwrap();
    let report = f.discover();
    let mut journey = Journey::new(&report, report.bounded_import.as_ref().unwrap()).unwrap();
    assert_eq!(journey.total_batches, 3);
    assert_eq!(
        journey.manual_evidence().1.len(),
        205,
        "advanced controls retain the complete native candidate pool"
    );
    assert_eq!(journey.eligible, 205);
    assert!(journey.excluded >= 1);
    let first = f.prepare(&mut journey);
    assert_eq!(first.plan.projected_insertions(), 100);
    let exact_rows = super::super::native_media_subset::subset_report(first.clone());
    assert_eq!(
        exact_rows
            .items
            .iter()
            .filter(|i| i.disposition == MediaDisposition::Eligible)
            .count(),
        100,
        "first batch presents only its exact allowlist"
    );
    assert_eq!(
        std::fs::read(&f.database).unwrap(),
        before,
        "discovery and batch preparation are read-only"
    );
    let result =
        apply_bounded_import(&first.plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    let operation = result.operation_id.unwrap();
    assert_eq!(result.inserted.len(), 100);
    // Simulate lost worker feedback: discard the UI before it sees a result.
    drop(journey);
    let db = Database::open_read_only(&f.database).unwrap();
    let history = db.discovery_import_history(0, 50).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].operation_id, operation);
    assert_eq!(history[0].items.len(), 100);
    assert!(
        apply_bounded_import(&first.plan, &AtomicBool::new(false), &mut |_, _| {}).is_err(),
        "committed stale plan cannot replay"
    );
    let report = f.discover();
    let mut resumed = Journey::new(&report, report.bounded_import.as_ref().unwrap()).unwrap();
    assert_eq!(resumed.existing, 100);
    assert_eq!(resumed.eligible, 105);
    assert_eq!(resumed.total_batches, 2);
    let next = f.prepare(&mut resumed);
    assert_eq!(next.plan.projected_insertions(), 100);
    let original_paths: std::collections::BTreeSet<_> =
        history[0].items.iter().map(|i| i.path()).collect();
    assert!(
        next.plan
            .selection_rows()
            .iter()
            .all(|r| !original_paths.contains(&r.path))
    );
    apply_bounded_import(&next.plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    resumed.submitted();
    let last = f.prepare(&mut resumed);
    assert_eq!(last.plan.projected_insertions(), 5);
    apply_bounded_import(&last.plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    let rows = db.load_archives().unwrap();
    assert_eq!(rows.len(), 205);
    assert_eq!(
        rows.iter()
            .map(|r| &r.absolute_path)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        205
    );
    let undo = db.preview_discovery_import_undo(&operation).unwrap();
    assert!(undo.can_undo());
    drop(db);
    let db = Database::open_or_create(&f.database).unwrap();
    let result = Database::undo_discovery_import(&undo).unwrap();
    assert_eq!(result.removed_records, 100);
    assert_eq!(db.load_archives().unwrap().len(), 105);
    assert_eq!(
        std::fs::read_dir(&f.root).unwrap().count(),
        206,
        "Undo preserves every fixture file"
    );
}

#[test]
fn changed_files_and_cancelled_preparation_never_become_an_approved_import() {
    let f = Fixture::new(2);
    let report = f.discover();
    let mut journey = Journey::new(&report, report.bounded_import.as_ref().unwrap()).unwrap();
    let before = std::fs::read(&f.database).unwrap();
    assert!(
        journey
            .prepare_first(
                &f.database,
                &f.config,
                &AtomicBool::new(true),
                &mut |_, _| {}
            )
            .is_err()
    );
    let mut bytes = std::fs::read(f.root.join("000.z80")).unwrap();
    bytes[101] = 77;
    std::fs::write(f.root.join("000.z80"), bytes).unwrap();
    let p = f.prepare(&mut journey);
    assert_eq!(p.plan.projected_insertions(), 1);
    assert_eq!(p.rejected.len(), 1);
    assert!(p.rejected[0].1.contains("changed"));
    assert_eq!(std::fs::read(&f.database).unwrap(), before);
}

#[test]
fn unsealed_discovery_rows_are_counted_as_needing_assessment_without_expanding_import() {
    let f = Fixture::new(2);
    let mut report = f.discover();
    // Native preview may observe a supported path but exhaust its comparison
    // budget before hashing it. Such a presentation row grants no authority.
    let unassessed = f.root.join("unassessed.z80");
    report
        .items
        .push(archivefs_core::ingestion::preview::MediaPreviewItem {
            path: unassessed.clone(),
            disposition: MediaDisposition::Eligible,
            explanation: "Comparison input budget exhausted; not freshly assessed.".into(),
            alternate: None,
            multipart: None,
            relationship_checks_complete: false,
        });
    let mut journey = Journey::new(&report, report.bounded_import.as_ref().unwrap()).unwrap();
    assert_eq!(journey.eligible, 2);
    assert_eq!(
        journey.not_ready, 1,
        "observed unassessed media must remain visible in the overall summary"
    );
    let prepared = f.prepare(&mut journey);
    assert_eq!(prepared.plan.projected_insertions(), 2);
    assert!(
        prepared
            .plan
            .selection_rows()
            .iter()
            .all(|r| r.path != unassessed)
    );
}
