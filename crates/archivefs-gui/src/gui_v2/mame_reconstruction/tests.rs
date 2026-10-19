//! Synthetic MAME reconstruction journeys: tiny catalogues and ZIPs in
//! temporary directories, private journal directories, no real collection.
use super::*;
use archivefs_core::dat::model::{
    DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry, DatSource, ParsedDat,
};
use archivefs_core::platform_evidence_fusion::dat_hash_representation::hash_bytes;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::Instant;

#[test]
fn mame_correctness_replacement_refuses_unresolved_support_metadata() {
    let mut value =
        serde_json::to_value(reconstruction_dependencies(&catalogue(), "clone").unwrap()).unwrap();
    value["unresolved_relationships"] = serde_json::json!(["cyclic device metadata"]);
    let dependencies: ReconstructionDependencies = serde_json::from_value(value).unwrap();
    let refusals = replacement_refusals(&ReplacementMemberChanges::default(), &dependencies);
    assert!(
        !refusals.is_empty(),
        "unresolved support metadata must not approve replacement"
    );
}

const BIOS: &[u8] = b"bios rom bytes";
const PARENT: &[u8] = b"parent program";
const CLONE: &[u8] = b"clone program";

fn rom(name: &str, bytes: &[u8], merge: Option<&str>) -> DatRomEntry {
    let hashes = hash_bytes(bytes, "", "");
    DatRomEntry {
        name: name.into(),
        size_bytes: Some(bytes.len() as u64),
        sha1: hashes.sha1.clone(),
        crc32: hashes.crc32.clone(),
        merge: merge.map(str::to_owned),
        ..Default::default()
    }
}

fn machine(
    name: &str,
    clone_of: Option<&str>,
    rom_of: Option<&str>,
    roms: Vec<DatRomEntry>,
) -> DatGameEntry {
    DatGameEntry {
        name: name.into(),
        clone_of: clone_of.map(str::to_owned),
        rom_of: rom_of.map(str::to_owned),
        roms,
        ..Default::default()
    }
}

/// A BIOS, a parent that borrows a BIOS ROM, a clone, and a clone-free game.
fn catalogue() -> ParsedDat {
    let mut bios = machine("bios", None, None, vec![rom("sys.rom", BIOS, None)]);
    bios.is_bios = Some("yes".into());
    let games = vec![
        bios,
        machine(
            "parent",
            None,
            Some("bios"),
            vec![
                rom("sys.rom", BIOS, Some("sys.rom")),
                rom("p.rom", PARENT, None),
            ],
        ),
        machine(
            "clone",
            Some("parent"),
            Some("parent"),
            vec![
                rom("sys.rom", BIOS, Some("sys.rom")),
                rom("p.rom", PARENT, Some("p.rom")),
                rom("c.rom", CLONE, None),
            ],
        ),
        machine("solo", None, None, vec![rom("s.rom", PARENT, None)]),
        machine(
            "two",
            None,
            None,
            vec![rom("t.rom", b"second parent", None)],
        ),
        machine(
            "twoc",
            Some("two"),
            Some("two"),
            vec![rom("tc.rom", b"second clone", None)],
        ),
        machine(
            "three",
            None,
            None,
            vec![rom("h.rom", b"third parent", None)],
        ),
        machine(
            "threec",
            Some("three"),
            Some("three"),
            vec![rom("hc.rom", b"third clone", None)],
        ),
    ];
    ParsedDat {
        source: DatSource {
            format: DatFormat::Logiqx,
            ecosystem: DatEcosystem::MAMEArcade,
            file_path: "fixture".into(),
            name: None,
            description: None,
            version: Some("test".into()),
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: games.len(),
            rom_count: games.iter().map(|g| g.roms.len()).sum(),
            parse_warnings: vec![],
            packing_policy: DatPackingPolicy::Standard,
        },
        games,
    }
}

struct Lab {
    dir: tempfile::TempDir,
    dat: ParsedDat,
}

impl Lab {
    /// A MAME folder holding only a non-merged clone set, which carries the
    /// BIOS, parent and clone ROMs. The merged parent set does not exist yet.
    fn new() -> Self {
        let lab = Self {
            dir: tempfile::tempdir().unwrap(),
            dat: catalogue(),
        };
        std::fs::create_dir_all(lab.root()).unwrap();
        lab.zip(
            "clone.zip",
            &[("sys.rom", BIOS), ("p.rom", PARENT), ("c.rom", CLONE)],
        );
        lab
    }
    fn root(&self) -> PathBuf {
        self.dir.path().join("mame")
    }
    fn journal(&self) -> PathBuf {
        self.dir.path().join("journal")
    }
    fn zip(&self, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = self.root().join(name);
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (entry, bytes) in entries {
            writer
                .start_file(
                    *entry,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        path
    }
    fn inputs(&self, set: &str) -> MameInputs {
        MameInputs {
            root: self.root(),
            dat: self.dir.path().join("mame.dat"),
            reference: CatalogueRef::local("fixture"),
            trusted: true,
            set: set.into(),
        }
    }
    /// The same plan the production preview builds, minus the database
    /// (packed ZIP discovery supplies all evidence here).
    fn family(
        &self,
        set: &str,
    ) -> Result<(MameMergedReconstructionPlan, Option<ArcadeJoinEvidence>), String> {
        let joins = discover_packed_zip_sources(&self.root(), &self.dat, set, "digest")?;
        let plan =
            build_merged_reconstruction_plan(&self.root(), &self.dat, &joins, set, "digest")?;
        let target = target_evidence(&joins, &plan);
        Ok((plan, target))
    }
    fn plan(&self, set: &str) -> Result<MameMergedReconstructionPlan, String> {
        self.family(set).map(|(plan, _)| plan)
    }
    fn preview(&self, set: &str) -> MamePreview {
        let (plan, target) = self.family(set).unwrap();
        MamePreview::from_plan(
            self.inputs(set),
            plan,
            reconstruction_dependencies(&self.dat, set).unwrap(),
            target.as_ref(),
            &self.dat,
        )
    }
    fn publish(&self, preview: &MamePreview) -> PublishResult {
        let staging = staging_root(&self.root());
        run_publish(
            preview,
            self.family(&preview.inputs.set),
            &MameContext {
                staging_root: &staging,
                journal_dir: &self.journal(),
            },
        )
    }
    fn destination(&self) -> PathBuf {
        self.root().join("parent.zip")
    }
    fn history(&self) -> Vec<RenameTransaction> {
        load_history(&self.journal()).0
    }
}

fn read_back(path: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        out.insert(entry.name().to_string(), bytes);
    }
    out
}

fn published(result: PublishResult) -> RenameTransaction {
    match result.outcome {
        PublishOutcome::Published { transaction, .. } => *transaction,
        other => panic!("expected a verified publication, got {other:?}"),
    }
}

// --- Correct merged reconstruction, BIOS inheritance, packed staging -------

#[test]
fn publish_builds_the_merged_set_from_packed_members_without_bios_roms() {
    let lab = Lab::new();
    let clone_before = std::fs::read(lab.root().join("clone.zip")).unwrap();
    let preview = lab.preview("clone");
    assert_eq!(preview.destination, DestinationStatus::Absent);
    assert!(preview.gate().is_ready(), "{:#?}", preview.plan);
    // Dependencies: the BIOS is reported and kept out of the output.
    assert_eq!(preview.dependencies.external_sets[0].name, "bios");
    assert_eq!(preview.dependencies.stored_separately().count(), 1);

    let transaction = published(lab.publish(&preview));
    assert!(publication_succeeded(&transaction));
    let output = read_back(&lab.destination());
    assert_eq!(
        output.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["c.rom", "p.rom"],
        "the merged output holds the family's own ROMs only"
    );
    assert_eq!(output["c.rom"], CLONE);
    assert_eq!(output["p.rom"], PARENT);
    assert_eq!(
        std::fs::read(lab.root().join("clone.zip")).unwrap(),
        clone_before,
        "the donor archive is an input only"
    );
    let history = lab.history();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history_status(&history[0]),
        "Verified publication complete; undo is available."
    );
    assert!(undo_actionable(&history[0]));
}

#[test]
fn undo_removes_the_published_copy_and_repeat_undo_is_refused() {
    let lab = Lab::new();
    let transaction = published(lab.publish(&lab.preview("clone")));
    match run_undo(&transaction.transaction_id, &lab.journal()) {
        UndoOutcome::RolledBack { transaction } => assert!(rollback_verified(&transaction)),
        other => panic!("expected a verified undo, got {other:?}"),
    }
    assert!(!lab.destination().exists());
    assert!(lab.root().join("clone.zip").exists());
    assert_eq!(
        history_status(&lab.history()[0]),
        "Rolled back; the generated destination is removed."
    );
    assert!(matches!(
        run_undo(&transaction.transaction_id, &lab.journal()),
        UndoOutcome::Refused(_)
    ));
    assert!(matches!(
        run_undo("mame-not-a-transaction", &lab.journal()),
        UndoOutcome::Refused(message) if message.contains("no longer in the recovery journal")
    ));
}

#[test]
fn a_second_preview_after_publishing_reports_nothing_to_publish() {
    let lab = Lab::new();
    published(lab.publish(&lab.preview("clone")));
    let again = lab.preview("parent");
    assert_eq!(again.destination, DestinationStatus::AlreadyPublished);
    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    panel.root = Some(lab.root());
    panel.dat = Some(lab.inputs("parent").dat);
    panel.set = "parent".into();
    panel.preview = Some(again);
    assert!(!panel.can_publish());
}

// --- Missing, duplicate, wrong hash, collisions ---------------------------

#[test]
fn a_missing_member_blocks_publish_and_nothing_is_written() {
    let lab = Lab::new();
    lab.zip("clone.zip", &[("p.rom", PARENT)]);
    let preview = lab.preview("clone");
    assert!(matches!(
        preview.gate(),
        PublishGate::Blocked(reasons) if reasons.contains(&BlockReason::MissingMembers(1))
    ));
    assert!(matches!(
        lab.publish(&preview).outcome,
        PublishOutcome::Refused(MameRefusal::PlanBlocked(_))
    ));
    assert!(!lab.destination().exists());
    assert!(!lab.journal().exists());
}

#[test]
fn wrong_bytes_are_never_used_as_the_member() {
    let lab = Lab::new();
    lab.zip(
        "clone.zip",
        &[("p.rom", PARENT), ("c.rom", b"corrupted clone program")],
    );
    let preview = lab.preview("clone");
    // The corrupted file has no catalogue identity, so the member is missing.
    assert_eq!(preview.plan.missing_members, vec!["c.rom".to_string()]);
    assert!(!preview.gate().is_ready());
}

#[test]
fn two_possible_sources_for_one_member_block_publish() {
    let lab = Lab::new();
    let mut dat = catalogue();
    dat.games.push(machine(
        "clone2",
        Some("parent"),
        Some("parent"),
        vec![rom("p.rom", PARENT, Some("p.rom"))],
    ));
    let lab = Lab { dat, ..lab };
    lab.zip("clone2.zip", &[("p.rom", PARENT)]);
    let preview = lab.preview("clone");
    assert_eq!(preview.plan.duplicate_candidates, vec!["p.rom".to_string()]);
    assert!(matches!(
        lab.publish(&preview).outcome,
        PublishOutcome::Refused(MameRefusal::PlanBlocked(_))
    ));
    assert!(!lab.destination().exists());
}

#[test]
fn an_existing_foreign_destination_is_never_replaced() {
    let lab = Lab::new();
    // An unreadable archive named like the parent refuses the whole preview.
    std::fs::write(lab.destination(), b"not a zip, owned by the person").unwrap();
    let error = lab.plan("clone").unwrap_err();
    assert!(error.contains("refused"), "{error}");
    assert_eq!(
        std::fs::read(lab.destination()).unwrap(),
        b"not a zip, owned by the person"
    );

    // A readable archive that is not this set is a collision, never replaced.
    let foreign = lab.zip("parent.zip", &[("readme.txt", b"someone else's file")]);
    let foreign_bytes = std::fs::read(&foreign).unwrap();
    let preview = lab.preview("clone");
    assert!(matches!(
        preview.destination,
        DestinationStatus::Occupied(_)
    ));
    assert!(matches!(
        lab.publish(&preview).outcome,
        PublishOutcome::Refused(MameRefusal::DestinationCollision(_))
    ));
    assert_eq!(std::fs::read(lab.destination()).unwrap(), foreign_bytes);
    assert!(!lab.journal().exists());
}

// --- Stale previews ------------------------------------------------------

#[test]
fn a_changed_source_after_preview_is_refused_as_stale() {
    let lab = Lab::new();
    let preview = lab.preview("clone");
    lab.zip(
        "clone.zip",
        &[("p.rom", PARENT), ("c.rom", CLONE), ("extra.bin", b"x")],
    );
    let result = lab.publish(&preview);
    assert!(
        matches!(
            result.outcome,
            PublishOutcome::Refused(MameRefusal::PlanStale(_))
        ),
        "{:?}",
        result.outcome
    );
    assert!(result.refreshed_plan.is_some());
    assert!(!lab.destination().exists());
    assert!(lab.history().is_empty());
}

#[test]
fn a_destination_created_after_preview_is_refused() {
    let lab = Lab::new();
    let preview = lab.preview("clone");
    std::fs::write(lab.destination(), b"appeared later").unwrap();
    let result = lab.publish(&preview);
    assert!(matches!(result.outcome, PublishOutcome::Refused(_)));
    assert_eq!(std::fs::read(lab.destination()).unwrap(), b"appeared later");
}

#[test]
fn changing_the_inputs_makes_the_preview_stale_in_the_panel() {
    let lab = Lab::new();
    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    panel.root = Some(lab.root());
    panel.dat = Some(lab.inputs("clone").dat);
    panel.set = "clone".into();
    panel.preview = Some(lab.preview("clone"));
    assert!(panel.can_publish());
    panel.set = "parent".into();
    assert!(!panel.preview_is_current());
    assert!(!panel.can_publish());
    panel.confirmation = publish_confirmation_phrase(1);
    assert!(
        !panel.confirm_publish(),
        "a stale preview cannot be published"
    );
}

// --- Interrupted Apply, failed Undo, recovery ---------------------------

/// Rewrites a journal as a crash between the rename and the final journal
/// update would leave it.
fn mark_interrupted(lab: &Lab, transaction: &RenameTransaction) -> RenameTransaction {
    let mut interrupted = transaction.clone();
    interrupted.state = TransactionState::Applying;
    for entry in &mut interrupted.entries {
        entry.state = EntryState::Applying;
        entry.applied_at_unix = None;
    }
    archivefs_core::dat::rename_apply::write_journal(&lab.journal(), &interrupted).unwrap();
    interrupted
}

#[test]
fn an_interrupted_apply_needs_recovery_and_blocks_further_publication() {
    let lab = Lab::new();
    let transaction = published(lab.publish(&lab.preview("clone")));
    let interrupted = mark_interrupted(&lab, &transaction);
    assert!(!publication_succeeded(&interrupted));
    assert_eq!(
        history_status(&lab.history()[0]),
        "Recovery is required before this transaction can be considered complete."
    );
    assert!(undo_actionable(&lab.history()[0]));

    // A new publication of another family is refused until recovery.
    std::fs::rename(lab.destination(), lab.dir.path().join("moved.zip")).unwrap();
    let result = lab.publish(&lab.preview("clone"));
    assert!(
        matches!(
            result.outcome,
            PublishOutcome::Refused(MameRefusal::RecoveryRequired(_))
        ),
        "{:?}",
        result.outcome
    );
    assert!(!lab.destination().exists());
    std::fs::rename(lab.dir.path().join("moved.zip"), lab.destination()).unwrap();

    // Undo reconciles the in-flight entry; success only when the copy is gone.
    match run_undo(&transaction.transaction_id, &lab.journal()) {
        UndoOutcome::RolledBack { .. } => assert!(!lab.destination().exists()),
        UndoOutcome::Incomplete { .. } | UndoOutcome::Failed(_) => {
            assert!(!history_status(&lab.history()[0]).starts_with("Rolled back;"))
        }
        other => panic!("unexpected outcome {other:?}"),
    }
}

#[test]
fn undo_refuses_to_destroy_a_changed_published_copy_and_never_reports_success() {
    let lab = Lab::new();
    let transaction = published(lab.publish(&lab.preview("clone")));
    std::fs::write(lab.destination(), b"the person edited this").unwrap();
    for _ in 0..2 {
        match run_undo(&transaction.transaction_id, &lab.journal()) {
            UndoOutcome::RolledBack { .. } | UndoOutcome::ClosedWithoutPublication { .. } => {
                panic!("a changed output must not be reported as undone")
            }
            UndoOutcome::Incomplete { message, .. } => {
                assert!(message.contains("did not complete"), "{message}")
            }
            UndoOutcome::Refused(_) | UndoOutcome::Failed(_) => {}
        }
        assert_eq!(
            std::fs::read(lab.destination()).unwrap(),
            b"the person edited this"
        );
    }
    let status = history_status(&lab.history()[0]);
    assert!(!status.starts_with("Rolled back;"), "{status}");
}

#[test]
fn an_inner_apply_failure_is_never_presented_as_published() {
    let lab = Lab::new();
    let mut transaction = published(lab.publish(&lab.preview("clone")));
    transaction.entries[0].state = EntryState::ApplyFailed;
    assert!(!publication_succeeded(&transaction));
    assert!(history_status(&transaction).contains("do not confirm a publication"));
    transaction.state = TransactionState::ApplyFailed;
    assert!(!undo_actionable(&transaction));
    assert!(history_status(&transaction).starts_with("Publication did not complete"));

    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    panel.finish_publish(PublishResult {
        outcome: PublishOutcome::Failed {
            reason: "transaction x ended as \"Apply failed\"".into(),
            transaction: Some(Box::new(transaction)),
        },
        refreshed_plan: None,
    });
    let notice = panel.notice.unwrap();
    assert_eq!(notice.tone, Tone::Danger);
    assert!(
        notice
            .lines
            .iter()
            .any(|l| l.contains("not reported as published"))
    );
}

#[cfg(unix)]
#[test]
fn a_real_inner_apply_failure_is_reported_as_failed_with_its_record() {
    use std::os::unix::fs::PermissionsExt;
    let lab = Lab::new();
    let preview = lab.preview("clone");
    // Staging stays writable; only the final rename into the folder fails,
    // which the executor reports inside an `Ok` outcome.
    std::fs::create_dir_all(staging_root(&lab.root())).unwrap();
    std::fs::set_permissions(lab.root(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = lab.publish(&preview);
    std::fs::set_permissions(lab.root(), std::fs::Permissions::from_mode(0o755)).unwrap();
    match result.outcome {
        PublishOutcome::Failed {
            reason,
            transaction: Some(transaction),
        } => {
            assert!(reason.contains("Apply failed"), "{reason}");
            assert!(!publication_succeeded(&transaction));
            assert!(!undo_actionable(&transaction));
        }
        other => panic!("an inner ApplyFailed must be a failure, got {other:?}"),
    }
    assert!(!lab.destination().exists());
    let history = lab.history();
    assert_eq!(history.len(), 1);
    assert!(history_status(&history[0]).starts_with("Publication did not complete"));
}

#[test]
fn a_partial_undo_and_its_retry_never_report_a_rollback() {
    let lab = Lab::new();
    lab.zip(
        "twoc.zip",
        &[("t.rom", b"second parent"), ("tc.rom", b"second clone")],
    );
    let first = published(lab.publish(&lab.preview("clone")));
    let second = published(lab.publish(&lab.preview("twoc")));
    // One transaction with two outputs, as earlier journals can hold.
    let mut combined = first.clone();
    combined.entries.extend(second.entries.clone());
    archivefs_core::dat::rename_apply::remove_journal(&lab.journal(), &second.transaction_id)
        .unwrap();
    archivefs_core::dat::rename_apply::write_journal(&lab.journal(), &combined).unwrap();
    let edited = lab.destination();
    let untouched = lab.root().join("two.zip");
    std::fs::write(&edited, b"changed by the person").unwrap();

    for attempt in 0..2 {
        match run_undo(&combined.transaction_id, &lab.journal()) {
            UndoOutcome::Incomplete {
                message,
                transaction,
            } => {
                assert!(message.contains("did not complete"), "{attempt}: {message}");
                assert!(!rollback_verified(&transaction) || !transaction.is_fully_rolled_back());
            }
            UndoOutcome::Refused(_) if attempt == 1 => {}
            other => panic!("{attempt}: a partial undo reported {other:?}"),
        }
        assert!(!untouched.exists(), "the unchanged output was undone");
        assert_eq!(std::fs::read(&edited).unwrap(), b"changed by the person");
        let status = history_status(&lab.history()[0]);
        assert!(!status.starts_with("Rolled back;"), "{status}");
    }
}

// --- Replacing an existing split parent ZIP ------------------------------

/// A split collection: parent.zip holds the parent's own ROM, clone.zip the
/// clone's. The merged parent.zip must replace the split one.
fn split_lab() -> Lab {
    let lab = Lab::new();
    lab.zip("parent.zip", &[("p.rom", PARENT)]);
    lab.zip("clone.zip", &[("c.rom", CLONE)]);
    lab
}

fn replaced(lab: &Lab) -> (RenameTransaction, Vec<u8>) {
    let original = std::fs::read(lab.destination()).unwrap();
    let preview = lab.preview("clone");
    let transaction = published(lab.publish(&preview));
    (transaction, original)
}

#[test]
fn a_split_parent_is_previewed_as_a_reviewable_replacement() {
    let lab = split_lab();
    let preview = lab.preview("clone");
    match &preview.destination {
        DestinationStatus::Replaceable { changes, .. } => {
            assert_eq!(changes.kept, vec!["p.rom".to_string()]);
            assert_eq!(changes.added, vec!["c.rom".to_string()]);
            assert!(changes.every_member_accounted_for());
            assert!(changes.provided_elsewhere.is_empty());
        }
        other => panic!("expected a reviewable replacement, got {other:?}"),
    }
    assert!(preview.gate().is_ready(), "{:?}", preview.gate());
    // The raw family plan still counts the existing archive as a collision.
    assert!(!publish_gate(&preview.plan).is_ready());

    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    panel.root = Some(lab.root());
    panel.dat = Some(lab.inputs("clone").dat);
    panel.set = "clone".into();
    panel.preview = Some(preview);
    assert!(panel.can_publish());
    assert_eq!(panel.required_phrase().unwrap(), "REPLACE parent.zip");
    panel.confirmation = publish_confirmation_phrase(1);
    assert!(
        !panel.confirm_publish(),
        "the create phrase never authorises a replacement"
    );
    panel.publish_pending = true;
    let text = render(&mut panel);
    for expected in [
        "Replace the existing parent ZIP",
        "Kept (already correct): p.rom",
        "Added: c.rom",
        "never edited or deleted",
        "Confirm: replace",
        "REPLACE parent.zip",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
    assert!(!lab.journal().exists(), "previewing wrote nothing");
}

#[test]
fn a_persisted_join_is_never_overridden_by_evidence_read_from_the_zip() {
    use archivefs_core::dat::mame_arcade_join::ArcadeJoinClass;
    let lab = split_lab();
    let discovered = discover_packed_zip_sources(&lab.root(), &lab.dat, "clone", "digest").unwrap();
    // The database holds an ambiguous join for the same parent.zip.
    let mut ambiguous = discovered
        .iter()
        .find(|(path, _)| path == &lab.destination())
        .cloned()
        .unwrap();
    ambiguous.1.class = ArcadeJoinClass::Ambiguous;
    let joins = combine_joins(vec![ambiguous], discovered.clone());
    assert_eq!(
        joins.len(),
        discovered.len(),
        "the same archive is never counted twice"
    );
    let plan =
        build_merged_reconstruction_plan(&lab.root(), &lab.dat, &joins, "clone", "digest").unwrap();
    let target = target_evidence(&joins, &plan).unwrap();
    assert_eq!(target.class, ArcadeJoinClass::Ambiguous);
    let preview = MamePreview::from_plan(
        lab.inputs("clone"),
        plan,
        reconstruction_dependencies(&lab.dat, "clone").unwrap(),
        Some(&target),
        &lab.dat,
    );
    assert!(
        matches!(preview.destination, DestinationStatus::Occupied(_)),
        "{:?}",
        preview.destination
    );
    assert!(!preview.gate().is_ready());
}

#[test]
fn one_matching_rom_beside_an_unknown_member_is_never_replaceable() {
    let lab = Lab::new();
    lab.zip(
        "parent.zip",
        &[("p.rom", PARENT), ("notes.txt", b"the person's own notes")],
    );
    lab.zip("clone.zip", &[("c.rom", CLONE)]);
    let before = std::fs::read(lab.destination()).unwrap();
    let preview = lab.preview("clone");
    match &preview.destination {
        DestinationStatus::ReplacementRefused { changes, reasons } => {
            assert_eq!(changes.kept, vec!["p.rom".to_string()]);
            assert_eq!(changes.unidentified, vec!["notes.txt".to_string()]);
            assert!(reasons[0].contains("not identified"), "{reasons:?}");
        }
        other => panic!("one matching ROM must not authorise replacement: {other:?}"),
    }
    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    panel.root = Some(lab.root());
    panel.dat = Some(lab.inputs("clone").dat);
    panel.set = "clone".into();
    panel.preview = Some(preview.clone());
    assert!(!panel.can_publish());
    let text = render(&mut panel);
    assert!(text.contains("will not be replaced"), "{text}");
    assert!(
        text.contains("Not identified by the catalogue: notes.txt"),
        "{text}"
    );
    assert!(!text.contains("Review replacement"), "{text}");
    assert!(matches!(
        lab.publish(&preview).outcome,
        PublishOutcome::Refused(MameRefusal::DestinationCollision(_))
    ));
    assert_eq!(std::fs::read(lab.destination()).unwrap(), before);
    assert!(!lab.journal().exists());
}

#[test]
fn wrong_content_under_a_required_name_is_never_replaceable() {
    let lab = Lab::new();
    lab.zip(
        "parent.zip",
        &[("p.rom", PARENT), ("c.rom", b"not the clone program")],
    );
    lab.zip("clone.zip", &[("c.rom", CLONE)]);
    let preview = lab.preview("clone");
    match &preview.destination {
        DestinationStatus::ReplacementRefused { changes, .. } => {
            assert_eq!(changes.changed, vec!["c.rom".to_string()]);
        }
        other => panic!("unestablished content must refuse replacement: {other:?}"),
    }
}

#[test]
fn a_bios_rom_in_a_non_merged_parent_is_accounted_for_and_undo_restores_exact_bytes() {
    let lab = Lab::new();
    // The BIOS set holds its own verified copy of sys.rom.
    lab.zip("bios.zip", &[("sys.rom", BIOS)]);
    lab.zip("parent.zip", &[("sys.rom", BIOS), ("p.rom", PARENT)]);
    lab.zip("clone.zip", &[("c.rom", CLONE)]);
    let preview = lab.preview("clone");
    match &preview.destination {
        DestinationStatus::Replaceable { changes, .. } => {
            assert_eq!(
                changes.provided_elsewhere,
                vec![("sys.rom".to_string(), "bios".to_string())]
            );
            assert!(changes.every_member_accounted_for());
        }
        other => panic!("expected a reviewable replacement, got {other:?}"),
    }
    let (transaction, original) = replaced(&lab);
    assert_eq!(
        read_back(&lab.destination())
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["c.rom", "p.rom"]
    );
    assert!(matches!(
        run_undo(&transaction.transaction_id, &lab.journal()),
        UndoOutcome::RolledBack { .. }
    ));
    assert_eq!(std::fs::read(lab.destination()).unwrap(), original);
}

#[test]
fn a_bios_rom_with_no_verified_copy_elsewhere_refuses_replacement() {
    for bios_zip in [None, Some(&[("other.rom", b"x".as_slice())][..])] {
        let lab = Lab::new();
        if let Some(entries) = bios_zip {
            lab.zip("bios.zip", entries);
        }
        lab.zip("parent.zip", &[("sys.rom", BIOS), ("p.rom", PARENT)]);
        lab.zip("clone.zip", &[("c.rom", CLONE)]);
        let preview = lab.preview("clone");
        match &preview.destination {
            DestinationStatus::ReplacementRefused { changes, reasons } => {
                assert_eq!(
                    changes.support_unverified,
                    vec![("sys.rom".to_string(), "bios".to_string())]
                );
                assert!(
                    reasons.iter().any(|r| r.contains("no verified copy")),
                    "{reasons:?}"
                );
            }
            other => panic!("the only live BIOS copy must not be dropped: {other:?}"),
        }
    }
}

#[test]
fn a_failed_publication_stays_truthful_after_a_restart() {
    use std::os::unix::fs::PermissionsExt;
    let lab = Lab::new();
    let preview = lab.preview("clone");
    std::fs::create_dir_all(staging_root(&lab.root())).unwrap();
    std::fs::set_permissions(lab.root(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = lab.publish(&preview);
    std::fs::set_permissions(lab.root(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(result.outcome, PublishOutcome::Failed { .. }));

    // A fresh panel, as after restarting EmuWiz, reads the journal itself.
    let mut panel = MameReconstructionPanel::with_executor(
        MameEnvironment {
            database_path: Some(lab.dir.path().join("evidence.db")),
            journal_dir: Some(lab.journal()),
            catalogue_inventory: None,
        },
        Arc::new(execute),
    );
    render(&mut panel);
    wait_for(&mut panel);
    assert_eq!(panel.history.len(), 1);
    let text = render(&mut panel);
    assert!(text.contains("Publication did not complete"), "{text}");
    assert!(!text.contains("Verified publication complete"), "{text}");
    assert!(!text.contains("Undo published MAME output"), "{text}");
    assert!(!text.contains("Undo this transaction"), "{text}");
}

#[test]
fn replacement_preserves_the_exact_original_and_undo_puts_it_back() {
    let lab = split_lab();
    let clone_before = std::fs::read(lab.root().join("clone.zip")).unwrap();
    let (transaction, original) = replaced(&lab);
    assert!(is_replacement(&transaction));
    let output = read_back(&lab.destination());
    assert_eq!(
        output.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["c.rom", "p.rom"]
    );
    let preserved = transaction.entries[0].source_path.clone();
    assert!(preserved.starts_with(staging_root(&lab.root())));
    assert_eq!(std::fs::read(&preserved).unwrap(), original);
    assert_eq!(
        std::fs::read(lab.root().join("clone.zip")).unwrap(),
        clone_before
    );
    assert!(history_status(&lab.history()[0]).starts_with("Verified replacement complete"));

    match run_undo(&transaction.transaction_id, &lab.journal()) {
        UndoOutcome::RolledBack { transaction } => assert!(rollback_verified(&transaction)),
        other => panic!("expected a verified undo, got {other:?}"),
    }
    assert_eq!(std::fs::read(lab.destination()).unwrap(), original);
    assert!(history_status(&lab.history()[0]).contains("original archive was restored"));
    assert!(matches!(
        run_undo(&transaction.transaction_id, &lab.journal()),
        UndoOutcome::Refused(_)
    ));
    assert_eq!(std::fs::read(lab.destination()).unwrap(), original);
}

#[test]
fn a_destination_changed_after_review_is_never_replaced() {
    let lab = split_lab();
    let preview = lab.preview("clone");
    // Same members, different file: a new inode and modification time.
    std::fs::remove_file(lab.destination()).unwrap();
    lab.zip("parent.zip", &[("p.rom", PARENT)]);
    let current = std::fs::read(lab.destination()).unwrap();
    let result = lab.publish(&preview);
    assert!(
        matches!(
            result.outcome,
            PublishOutcome::Refused(MameRefusal::PlanStale(_) | MameRefusal::DestinationChanged(_))
        ),
        "{:?}",
        result.outcome
    );
    assert_eq!(std::fs::read(lab.destination()).unwrap(), current);
    assert!(lab.history().is_empty());
}

#[test]
fn a_staging_failure_leaves_the_existing_archive_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let lab = split_lab();
    let original = std::fs::read(lab.destination()).unwrap();
    let preview = lab.preview("clone");
    let staging = staging_root(&lab.root());
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = lab.publish(&preview);
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();
    match result.outcome {
        PublishOutcome::Refused(refusal) => {
            assert!(refusal.next_step().starts_with("Nothing was published"))
        }
        other => panic!("a staging failure must refuse, got {other:?}"),
    }
    assert_eq!(std::fs::read(lab.destination()).unwrap(), original);
    assert!(lab.history().is_empty());
}

#[test]
fn an_interrupted_replacement_never_loses_the_original() {
    let lab = split_lab();
    let (transaction, original) = replaced(&lab);
    let preserved = transaction.entries[0].source_path.clone();
    mark_interrupted(&lab, &transaction);
    assert_eq!(
        history_status(&lab.history()[0]),
        "Recovery is required before this transaction can be considered complete."
    );
    let outcome = run_undo(&transaction.transaction_id, &lab.journal());
    let at_destination = std::fs::read(lab.destination()).ok();
    let at_preserved = std::fs::read(&preserved).ok();
    assert!(
        at_destination.as_deref() == Some(original.as_slice())
            || at_preserved.as_deref() == Some(original.as_slice()),
        "the original must survive: {outcome:?}"
    );
    if let UndoOutcome::RolledBack { .. } = outcome {
        assert_eq!(at_destination.unwrap(), original);
    }
}

#[test]
fn undo_of_an_edited_replacement_keeps_both_the_edit_and_the_original() {
    let lab = split_lab();
    let (transaction, original) = replaced(&lab);
    let preserved = transaction.entries[0].source_path.clone();
    std::fs::write(lab.destination(), b"edited after replacement").unwrap();
    for _ in 0..2 {
        assert!(!matches!(
            run_undo(&transaction.transaction_id, &lab.journal()),
            UndoOutcome::RolledBack { .. } | UndoOutcome::ClosedWithoutPublication { .. }
        ));
        assert_eq!(
            std::fs::read(lab.destination()).unwrap(),
            b"edited after replacement"
        );
        assert_eq!(std::fs::read(&preserved).unwrap(), original);
    }
    assert!(!history_status(&lab.history()[0]).starts_with("Rolled back;"));
}

#[test]
fn a_journal_that_cannot_be_reserved_is_refused_not_failed() {
    use std::os::unix::fs::PermissionsExt;
    let lab = Lab::new();
    let preview = lab.preview("clone");
    std::fs::create_dir_all(lab.journal()).unwrap();
    std::fs::set_permissions(lab.journal(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = lab.publish(&preview);
    std::fs::set_permissions(lab.journal(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(
            result.outcome,
            PublishOutcome::Refused(MameRefusal::JournalUnavailable(_))
        ),
        "{:?}",
        result.outcome
    );
    assert!(!lab.destination().exists());
}

#[test]
fn a_refused_stale_publish_clears_the_preview() {
    let lab = Lab::new();
    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    panel.preview = Some(lab.preview("clone"));
    panel.finish_publish(PublishResult {
        outcome: PublishOutcome::Refused(MameRefusal::PlanStale("changed".into())),
        refreshed_plan: Some(lab.plan("clone").unwrap()),
    });
    assert!(panel.preview.is_none());
    assert!(!panel.can_publish());
}

// --- Receipts that contradict themselves (ported from organisation.rs) -----

#[test]
fn an_undo_receipt_contradicted_by_its_entries_is_never_a_rollback() {
    let lab = Lab::new();
    let applied = published(lab.publish(&lab.preview("clone")));
    for entry_state in [
        EntryState::Applying,
        EntryState::Applied,
        EntryState::RollingBack,
        EntryState::RollbackFailed,
    ] {
        // The outer result and the transaction state both claim success.
        let mut claimed = applied.clone();
        claimed.state = TransactionState::RolledBack;
        claimed.entries[0].state = entry_state;
        match classify_rollback(RollbackOutcome {
            result: RollbackResult::FullyRolledBack,
            transaction: claimed.clone(),
        }) {
            UndoOutcome::Incomplete { message, .. } => {
                assert!(message.contains("did not complete"), "{entry_state:?}");
                assert!(
                    message.contains("entries do not confirm"),
                    "{entry_state:?}: {message}"
                );
            }
            other => panic!("{entry_state:?}: reported {other:?}"),
        }
        let status = history_status(&claimed);
        assert!(!status.starts_with("Rolled back;"), "{entry_state:?}");
        assert!(status.contains("its entries do not confirm it"), "{status}");
    }
    assert!(lab.destination().is_file());
}

#[test]
fn a_history_row_never_claims_what_its_entries_contradict() {
    let lab = Lab::new();
    let applied = published(lab.publish(&lab.preview("clone")));
    for entry_state in [
        EntryState::Skipped,
        EntryState::ApplyFailed,
        EntryState::Planned,
    ] {
        let mut mixed = applied.clone();
        mixed.entries.push(applied.entries[0].clone());
        mixed.entries[1].state = entry_state;
        assert!(!publication_succeeded(&mixed));
        let status = history_status(&mixed);
        assert!(
            status.contains("its entries do not confirm a publication"),
            "{entry_state:?}: {status}"
        );
    }
    let mut rolled_back = applied.clone();
    rolled_back.state = TransactionState::RolledBack;
    rolled_back.entries[0].state = EntryState::RolledBack;
    assert!(rollback_verified(&rolled_back));
    assert!(history_status(&rolled_back).starts_with("Rolled back;"));
    let mut nothing = rolled_back.clone();
    nothing.entries[0].state = EntryState::ApplyFailed;
    assert!(!rollback_verified(&nothing));
    assert!(history_status(&nothing).contains("published nothing"));
    let mut empty = rolled_back;
    empty.entries.clear();
    assert!(!rollback_verified(&empty));
    assert!(!history_status(&empty).starts_with("Rolled back;"));
}

// --- Ported from organisation.rs: retries and restarts ------------------

fn journal_bytes(lab: &Lab, transaction_id: &str) -> Vec<u8> {
    std::fs::read(
        archivefs_core::dat::rename_apply::journal_path(&lab.journal(), transaction_id).unwrap(),
    )
    .unwrap()
}

fn restarted_panel(lab: &Lab) -> (MameReconstructionPanel, String) {
    let mut panel = MameReconstructionPanel::with_executor(
        MameEnvironment {
            database_path: Some(lab.dir.path().join("evidence.db")),
            journal_dir: Some(lab.journal()),
            catalogue_inventory: None,
        },
        Arc::new(execute),
    );
    render(&mut panel);
    wait_for(&mut panel);
    let text = render(&mut panel);
    (panel, text)
}

fn assert_no_undo_offered(text: &str) {
    assert!(!text.contains("Undo published MAME output"), "{text}");
    assert!(
        !text.contains("Undo this transaction's remaining applied outputs"),
        "{text}"
    );
    assert!(!text.contains("Undo replacement"), "{text}");
}

#[test]
fn needs_review_only_outputs_offer_a_retry_that_succeeds_once_the_blocker_is_gone() {
    let lab = Lab::new();
    lab.zip(
        "twoc.zip",
        &[("t.rom", b"second parent"), ("tc.rom", b"second clone")],
    );
    let first = published(lab.publish(&lab.preview("clone")));
    let second = published(lab.publish(&lab.preview("twoc")));
    let mut combined = first.clone();
    combined.entries.extend(second.entries.clone());
    archivefs_core::dat::rename_apply::remove_journal(&lab.journal(), &second.transaction_id)
        .unwrap();
    archivefs_core::dat::rename_apply::write_journal(&lab.journal(), &combined).unwrap();
    let alpha = lab.destination();
    let beta = lab.root().join("two.zip");
    // A different file now occupies the original path of the later output.
    let obstruction = combined.entries[1].source_path.clone();
    std::fs::write(&obstruction, b"someone else's file").unwrap();
    assert!(matches!(
        run_undo(&combined.transaction_id, &lab.journal()),
        UndoOutcome::Incomplete { .. }
    ));
    let states = |t: &RenameTransaction| t.entries.iter().map(|e| e.state).collect::<Vec<_>>();
    // Only a needs-review output remains: nothing is Applied.
    assert_eq!(
        states(&lab.history()[0]),
        vec![EntryState::RolledBack, EntryState::RollbackFailed]
    );
    assert!(undo_actionable(&lab.history()[0]));
    let (_, text) = restarted_panel(&lab);
    assert!(
        text.contains("Retry undo of the outputs awaiting review"),
        "{text}"
    );
    assert!(!text.contains("Rolled back;"), "{text}");

    // Still blocked: the retry is offered, but fresh checks refuse it and the
    // unrelated file is never touched.
    assert!(matches!(
        run_undo(&combined.transaction_id, &lab.journal()),
        UndoOutcome::Incomplete { .. }
    ));
    assert_eq!(std::fs::read(&obstruction).unwrap(), b"someone else's file");
    assert!(beta.exists());

    std::fs::remove_file(&obstruction).unwrap();
    assert!(matches!(
        run_undo(&combined.transaction_id, &lab.journal()),
        UndoOutcome::RolledBack { .. }
    ));
    assert!(!alpha.exists() && !beta.exists());
    assert!(rollback_verified(&lab.history()[0]));
    assert!(!undo_actionable(&lab.history()[0]), "fully undone");
    let (_, text) = restarted_panel(&lab);
    assert_no_undo_offered(&text);
    assert!(!text.contains("Retry undo of the outputs awaiting review"));
}

#[test]
fn ineligible_needs_review_entries_are_not_offered_a_retry() {
    let lab = Lab::new();
    let mut transaction = published(lab.publish(&lab.preview("clone")));
    transaction.state = TransactionState::RollbackFailed;
    let identity = transaction.entries[0].identity.clone();
    for operation in [
        TransactionOperation::CreateCopy {
            expected_source: PathBuf::from("/x"),
            destination_root: PathBuf::from("/y"),
        },
        TransactionOperation::ReplaceExisting {
            original_identity: identity.clone(),
            destination_root: PathBuf::from("/y"),
        },
    ] {
        transaction.entries[0].state = EntryState::RollbackFailed;
        transaction.entries[0].operation = operation;
        assert!(!undo_actionable(&transaction));
    }
    transaction.entries[0].state = EntryState::RollbackFailed;
    transaction.entries[0].operation = TransactionOperation::RenameMove;
    assert!(undo_actionable(&transaction));
    transaction.entries[0].state = EntryState::RolledBack;
    assert!(!undo_actionable(&transaction));
}

#[test]
fn a_mixed_undo_continues_past_a_refused_output_and_a_retry_refuses_it_again() {
    let lab = Lab::new();
    lab.zip(
        "twoc.zip",
        &[("t.rom", b"second parent"), ("tc.rom", b"second clone")],
    );
    lab.zip(
        "threec.zip",
        &[("h.rom", b"third parent"), ("hc.rom", b"third clone")],
    );
    let first = published(lab.publish(&lab.preview("clone")));
    let second = published(lab.publish(&lab.preview("twoc")));
    let third = published(lab.publish(&lab.preview("threec")));
    let mut combined = first.clone();
    for other in [&second, &third] {
        combined.entries.extend(other.entries.clone());
        archivefs_core::dat::rename_apply::remove_journal(&lab.journal(), &other.transaction_id)
            .unwrap();
    }
    archivefs_core::dat::rename_apply::write_journal(&lab.journal(), &combined).unwrap();
    let alpha = lab.destination();
    let beta = lab.root().join("two.zip");
    let gamma = lab.root().join("three.zip");
    // Reverse order: gamma undone, beta refused, alpha undone around it.
    std::fs::write(&beta, b"changed by the user").unwrap();
    assert!(matches!(
        run_undo(&combined.transaction_id, &lab.journal()),
        UndoOutcome::Incomplete { .. }
    ));
    let states = |t: &RenameTransaction| t.entries.iter().map(|e| e.state).collect::<Vec<_>>();
    assert_eq!(
        states(&lab.history()[0]),
        vec![
            EntryState::RolledBack,
            EntryState::RollbackFailed,
            EntryState::RolledBack
        ]
    );
    assert!(!gamma.exists() && !alpha.exists());

    // The refused output stays needs-review; it is never offered as an undo of
    // a "published" transaction and is never reported as rolled back.
    let (_, text) = restarted_panel(&lab);
    assert!(!text.contains("Undo published MAME output"), "{text}");
    assert!(!text.contains("Rolled back;"), "{text}");

    match run_undo(&combined.transaction_id, &lab.journal()) {
        UndoOutcome::Incomplete { message, .. } => {
            assert!(message.contains("did not complete"), "{message}")
        }
        other => panic!("a retry over a refused output reported {other:?}"),
    }
    assert!(!alpha.exists());
    assert_eq!(std::fs::read(&beta).unwrap(), b"changed by the user");
    let journaled = &lab.history()[0];
    assert_eq!(journaled.state, TransactionState::RollbackFailed);
    assert_eq!(
        states(journaled),
        vec![
            EntryState::RolledBack,
            EntryState::RollbackFailed,
            EntryState::RolledBack
        ]
    );
    let (_, text) = restarted_panel(&lab);
    assert!(!text.contains("Rolled back;"), "{text}");
    assert_no_undo_offered(&text);
}

#[test]
fn persisted_failure_journals_stay_truthful_after_a_restart() {
    let lab = Lab::new();
    let transaction = published(lab.publish(&lab.preview("clone")));
    let id = transaction.transaction_id.clone();
    std::fs::write(lab.destination(), b"changed by the user").unwrap();
    assert!(matches!(
        run_undo(&id, &lab.journal()),
        UndoOutcome::Incomplete { .. }
    ));

    // "Restart": a fresh panel rebuilds the history from the journal alone.
    let (panel, text) = restarted_panel(&lab);
    let reason = panel.history[0].entries[0].failure_reason.clone().unwrap();
    assert!(text.contains("Undo did not complete"), "{text}");
    assert!(text.contains(&reason), "{text}");
    assert!(!text.contains("Rolled back;"), "{text}");
    assert_no_undo_offered(&text);
    // The refused output awaits review: a retry is offered, but it re-validates
    // first, so a still-changed file is refused again and left untouched.
    assert!(
        text.contains("Retry undo of the outputs awaiting review"),
        "{text}"
    );
    assert!(matches!(
        run_undo(&id, &lab.journal()),
        UndoOutcome::Incomplete { .. }
    ));
    assert_eq!(
        std::fs::read(lab.destination()).unwrap(),
        b"changed by the user"
    );
    assert_eq!(
        lab.history()[0].entries[0].state,
        EntryState::RollbackFailed
    );

    // A journal rewritten by the old behaviour (transaction "Rolled back",
    // entry still "Rollback failed") is shown as unverified, and an undo
    // neither accepts nor rewrites it.
    let mut legacy = panel.history[0].clone();
    legacy.state = TransactionState::RolledBack;
    archivefs_core::dat::rename_apply::write_journal(&lab.journal(), &legacy).unwrap();
    let before = journal_bytes(&lab, &id);
    let (_, text) = restarted_panel(&lab);
    assert!(!text.contains("Rolled back;"), "{text}");
    assert!(text.contains("its entries do not confirm it"), "{text}");
    assert!(text.contains(&reason), "{text}");
    assert_no_undo_offered(&text);
    assert!(!matches!(
        run_undo(&id, &lab.journal()),
        UndoOutcome::RolledBack { .. } | UndoOutcome::ClosedWithoutPublication { .. }
    ));
    assert_eq!(before, journal_bytes(&lab, &id));
    assert_eq!(
        std::fs::read(lab.destination()).unwrap(),
        b"changed by the user"
    );
}

// --- Refusal mapping --------------------------------------------------------

#[test]
fn backend_refusals_keep_their_meaning() {
    for (message, expected) in [
        (
            "stale reconstruction source; review again; no staging cleanup was performed; inspect Recovery",
            "SourceChanged",
        ),
        (
            "stale reconstruction destination; review again",
            "DestinationChanged",
        ),
        (
            "unrecorded or changed leftover /m/.emuwiz-mame-staging/mame-1.zip.staged was left untouched",
            "RecoveryRequired",
        ),
        (
            "transaction mame-1 is Applying and requires Recovery before another publication",
            "RecoveryRequired",
        ),
        (
            "existing target needs checksum-backed ownership evidence",
            "DestinationCollision",
        ),
        ("reconstruction preflight refused 1 path(s): x", "Preflight"),
        ("staged SHA-1/CRC/size mismatch for a", "VerificationFailed"),
        // Real wrapped messages: the staging suffix names a .zip.staged path.
        (
            "could not exclusively reserve MAME journal: Permission denied (os error 13); no staging cleanup was performed; inspect Recovery and attempt /m/.emuwiz-mame-staging/mame-1.zip.staged",
            "JournalUnavailable",
        ),
        (
            "another MAME publication is in progress in /j; retry after it finishes; nothing was staged or moved",
            "Busy",
        ),
        (
            "stale reconstruction target; review again; no staging cleanup was performed; inspect Recovery and attempt /m/.emuwiz-mame-staging/mame-2.zip.staged",
            "DestinationChanged",
        ),
        ("something new", "Other"),
    ] {
        let refusal = MameRefusal::from_backend(message);
        assert!(
            format!("{refusal:?}").starts_with(expected),
            "{message} -> {refusal:?}"
        );
        assert!(refusal.next_step().starts_with("Nothing was published"));
    }
}

// --- Clone-free games ---------------------------------------------------

#[test]
fn a_clone_free_game_is_explained_not_reported_as_broken_identity() {
    let lab = Lab::new();
    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    panel.finish(MameJobReport {
        result: MameResult::Preview(Ok(PreviewOutcome::NotNeeded(Box::new(StandaloneGame {
            inputs: lab.inputs("solo"),
            parent: "solo".into(),
            dependencies: reconstruction_dependencies(&lab.dat, "solo").unwrap(),
        })))),
        history: None,
    });
    let notice = panel.notice.clone().unwrap();
    assert_eq!(notice.tone, Tone::Info);
    assert_eq!(
        notice.headline,
        "No merged parent/clone reconstruction is needed for this game"
    );
    assert!(!notice.lines.join(" ").contains("incomplete"));
    assert!(panel.preview.is_none());
    assert!(!panel.can_publish());
}

// --- Background jobs and GUI responsiveness -------------------------------

fn panel_with(lab: &Lab, executor: MameExecutor) -> MameReconstructionPanel {
    let mut panel = MameReconstructionPanel::with_executor(
        MameEnvironment {
            database_path: Some(lab.dir.path().join("evidence.db")),
            journal_dir: Some(lab.journal()),
            catalogue_inventory: None,
        },
        executor,
    );
    panel.catalogue = Some(CatalogueRef::local("fixture"));
    panel.catalogue_trusted = true;
    // History is loaded by the tests that need it.
    panel.history_requested = true;
    panel
}

fn render(panel: &mut MameReconstructionPanel) -> String {
    fn text(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| text(s, out)),
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 4000.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| panel.show(ui));
            });
        },
    );
    let mut rendered = String::new();
    output
        .shapes
        .iter()
        .for_each(|s| text(&s.shape, &mut rendered));
    rendered
}

fn wait_for(panel: &mut MameReconstructionPanel) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !panel.poll() {
        assert!(Instant::now() < deadline, "background job never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_long_publish_runs_off_the_gui_thread_and_locks_other_actions() {
    let lab = Lab::new();
    let (release, gate) = mpsc::channel::<()>();
    let gate = Arc::new(Mutex::new(gate));
    let executor: MameExecutor = Arc::new(move |request, _, phase| {
        assert!(matches!(request, MameRequest::Publish { .. }));
        phase("Staging and verifying the new set, then publishing it");
        gate.lock().unwrap().recv().unwrap();
        MameJobReport {
            result: MameResult::Publish(Box::new(PublishResult {
                outcome: PublishOutcome::AlreadyPublished {
                    destination: PathBuf::from("/x/parent.zip"),
                },
                refreshed_plan: None,
            })),
            history: None,
        }
    });
    let mut panel = panel_with(&lab, executor);
    panel.root = Some(lab.root());
    panel.dat = Some(lab.inputs("clone").dat);
    panel.set = "clone".into();
    panel.preview = Some(lab.preview("clone"));
    panel.publish_pending = true;
    panel.confirmation = publish_confirmation_phrase(1);

    let started = Instant::now();
    assert!(panel.confirm_publish());
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "starting a publish must not wait for it"
    );
    assert_eq!(panel.running_kind(), Some(MameJobKind::Publish));
    // Frames keep rendering while the worker is blocked.
    for _ in 0..3 {
        let frame = Instant::now();
        let text = render(&mut panel);
        assert!(frame.elapsed() < Duration::from_secs(2));
        assert!(text.contains("Publishing the reconstructed set"), "{text}");
        assert!(text.contains("cannot be cancelled once started"));
    }
    assert!(!panel.can_publish());
    assert!(!panel.confirm_publish(), "a second publish cannot start");
    assert!(!panel.start(MameRequest::LoadHistory));

    release.send(()).unwrap();
    wait_for(&mut panel);
    assert!(!panel.is_busy());
    assert_eq!(
        panel.notice.as_ref().unwrap().headline,
        "Nothing to publish"
    );
}

#[test]
fn a_worker_that_dies_mid_publish_reports_an_unknown_outcome() {
    let lab = Lab::new();
    let executor: MameExecutor = Arc::new(|_, _, _| panic!("simulated worker crash"));
    let mut panel = panel_with(&lab, executor);
    assert!(panel.start(MameRequest::Undo {
        transaction_id: "mame-x".into()
    }));
    wait_for(&mut panel);
    let notice = panel.notice.unwrap();
    assert_eq!(notice.tone, Tone::Danger);
    assert!(notice.lines[0].contains("outcome is unknown"));
}

#[test]
fn the_production_worker_publishes_and_undoes_through_the_panel() {
    // The production executor needs the pinned MAME catalogue, so this
    // journey drives the same worker bodies through the panel's jobs.
    let lab = Arc::new(Lab::new());
    let worker_lab = Arc::clone(&lab);
    let executor: MameExecutor = Arc::new(move |request, environment, _| {
        let lab = &worker_lab;
        let history = || Some(environment.journal_dir().map(|dir| load_history(&dir)));
        match request {
            MameRequest::Publish { preview } => MameJobReport {
                result: MameResult::Publish(Box::new(lab.publish(&preview))),
                history: history(),
            },
            MameRequest::Undo { transaction_id } => MameJobReport {
                result: MameResult::Undo(run_undo(
                    &transaction_id,
                    &environment.journal_dir().unwrap(),
                )),
                history: history(),
            },
            MameRequest::LoadHistory => MameJobReport {
                result: MameResult::History,
                history: history(),
            },
            other => panic!("unexpected {other:?}"),
        }
    });
    let mut panel = panel_with(&lab, executor);
    panel.root = Some(lab.root());
    panel.dat = Some(lab.inputs("clone").dat);
    panel.set = "clone".into();
    panel.preview = Some(lab.preview("clone"));

    let text = render(&mut panel);
    for expected in [
        "2. Dependencies",
        "bios (BIOS)",
        "not part of this rebuild",
        "It is not self-contained",
        "Publish reconstructed merged output",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
    assert!(!text.contains('→') && !text.contains('✓'));

    panel.publish_pending = true;
    panel.confirmation = "PUBLISH MAME".into();
    assert!(
        !panel.confirm_publish(),
        "the typed confirmation is required"
    );
    panel.confirmation = publish_confirmation_phrase(1);
    assert!(panel.confirm_publish());
    wait_for(&mut panel);
    let notice = panel.notice.clone().unwrap();
    assert_eq!(notice.tone, Tone::Success, "{notice:?}");
    assert!(
        notice
            .lines
            .iter()
            .any(|l| l.contains("does not prove any machine runs"))
    );
    assert!(
        panel.preview.is_none(),
        "a used preview cannot be published twice"
    );
    assert_eq!(panel.history.len(), 1);
    let id = panel.history[0].transaction_id.clone();

    panel.undo_pending = Some(id.clone());
    panel.confirmation = "UNDO MAME clone".into();
    assert!(!panel.confirm_undo(&id));
    panel.confirmation = undo_confirmation_phrase("parent");
    assert!(panel.confirm_undo(&id));
    wait_for(&mut panel);
    assert_eq!(
        panel.notice.as_ref().unwrap().headline,
        "MAME reconstruction was rolled back safely"
    );
    assert!(!lab.destination().exists());
    assert!(!undo_actionable(&panel.history[0]));
    let text = render(&mut panel);
    assert!(text.contains("Rolled back; the generated destination is removed."));
    assert!(!text.contains("Undo published MAME output"));
}

fn versioned_xml(version: &str, clone_bytes: &[u8]) -> String {
    let entry = |name: &str, bytes: &[u8], merge: &str| {
        let h = hash_bytes(bytes, "", "");
        format!(
            "<rom name=\"{name}\" size=\"{}\" sha1=\"{}\" crc=\"{}\" {merge}/>",
            bytes.len(),
            h.sha1.unwrap(),
            h.crc32.unwrap()
        )
    };
    format!(
        "<mame build=\"{version}\"><machine name=\"bios\" isbios=\"yes\">{}</machine><machine name=\"parent\" romof=\"bios\">{}{}</machine><machine name=\"clone\" cloneof=\"parent\" romof=\"parent\">{}{}{}</machine></mame>",
        entry("sys.rom", BIOS, ""),
        entry("sys.rom", BIOS, "merge=\"sys.rom\""),
        entry("p.rom", PARENT, ""),
        entry("sys.rom", BIOS, "merge=\"sys.rom\""),
        entry("p.rom", PARENT, "merge=\"p.rom\""),
        entry("c.rom", clone_bytes, "")
    )
}

#[test]
fn explicitly_selected_versioned_mame_dat_is_not_restricted_to_0174() {
    for version in ["0.250", "0.260"] {
        let lab = Lab::new();
        std::fs::write(lab.inputs("clone").dat, versioned_xml(version, CLONE)).unwrap();
        let report = execute(
            MameRequest::RefreshEvidence {
                inputs: lab.inputs("clone"),
                only_family: true,
            },
            &MameEnvironment {
                database_path: Some(lab.dir.path().join("evidence.db")),
                journal_dir: Some(lab.journal()),
                catalogue_inventory: Some(catalogue_inventory(&lab)),
            },
            &mut |_| {},
        );
        assert!(
            matches!(report.result, MameResult::Evidence(Ok(_))),
            "{version}: {:?}",
            report.result
        );
    }
}

fn catalogue_inventory(lab: &Lab) -> crate::dat_catalogue_picker::CatalogueInventorySnapshot {
    use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
    let mut registry = DatSourceRegistry::new();
    registry
        .add(DatSourceEntry::new(
            "fixture".into(),
            "Synthetic versioned MAME".into(),
            lab.inputs("clone").dat,
            DatSourceKind::File,
        ))
        .unwrap();
    crate::dat_catalogue_picker::CatalogueInventorySnapshot {
        local_registry: registry,
        managed_sources: Default::default(),
        managed_root: lab.dir.path().join("managed"),
    }
}

fn version_environment(lab: &Lab) -> MameEnvironment {
    let path = lab.inputs("clone").dat;
    if !path.exists() {
        std::fs::write(path, versioned_xml("0.250", CLONE)).unwrap();
    }
    let database_path = lab.dir.path().join("version-evidence.db");
    drop(Database::open_or_create(&database_path).unwrap());
    MameEnvironment {
        database_path: Some(database_path),
        journal_dir: Some(lab.journal()),
        catalogue_inventory: Some(catalogue_inventory(lab)),
    }
}
fn version_preview(lab: &Lab, environment: &MameEnvironment) -> MamePreview {
    match execute(
        MameRequest::Preview {
            inputs: lab.inputs("clone"),
        },
        environment,
        &mut |_| {},
    )
    .result
    {
        MameResult::Preview(Ok(PreviewOutcome::Ready(preview))) => *preview,
        other => panic!("expected real versioned preview: {other:?}"),
    }
}

#[test]
fn selected_versions_use_their_own_hashes_clones_dependencies_and_replacement_gate() {
    let lab = Lab::new();
    let environment = version_environment(&lab);
    let path = lab.inputs("clone").dat;
    std::fs::write(&path, versioned_xml("0.250", CLONE)).unwrap();
    let first = version_preview(&lab, &environment);
    assert!(first.gate().is_ready(), "{:?}", first.gate());
    assert_eq!(first.plan.dat_version, "0.250");
    assert_eq!(first.plan.clones, vec!["clone"]);
    assert_eq!(
        first.catalogue.as_ref().unwrap().sha256,
        first.plan.dat_sha256
    );
    assert!(!first.dependencies.external_sets.is_empty());
    // An existing parent lacking the clone member may be reviewed for replacement
    // only using the selected version's hashes and existing every-member rule.
    lab.zip("parent.zip", &[("p.rom", PARENT)]);
    lab.zip("clone.zip", &[("c.rom", CLONE)]);
    let replacement = version_preview(&lab, &environment);
    assert!(replacement.is_replacement());
    std::fs::write(&path, versioned_xml("0.260", b"different clone revision")).unwrap();
    let second = version_preview(&lab, &environment);
    assert_eq!(second.plan.dat_version, "0.260");
    assert!(!second.gate().is_ready());
    assert_ne!(first.plan.dat_sha256, second.plan.dat_sha256);
    assert!(!second.plan.missing_members.is_empty());
    assert!(!lab.journal().exists());
    // A missing clone parent in this version is refused, never borrowed from 0.250.
    std::fs::write(
        &path,
        versioned_xml("0.260", CLONE).replace("cloneof=\"parent\"", "cloneof=\"missing-parent\""),
    )
    .unwrap();
    assert!(matches!(
        execute(
            MameRequest::Preview {
                inputs: lab.inputs("clone")
            },
            &environment,
            &mut |_| {}
        )
        .result,
        MameResult::Preview(Err(_))
    ));
}

#[test]
fn publish_revalidates_catalogue_bytes_identity_reference_and_availability_before_staging() {
    for change in ["bytes", "inode", "missing", "disabled", "reference"] {
        let lab = Lab::new();
        let mut environment = version_environment(&lab);
        let path = lab.inputs("clone").dat;
        let xml = versioned_xml("0.250", CLONE);
        std::fs::write(&path, &xml).unwrap();
        let mut preview = version_preview(&lab, &environment);
        let before_zip = std::fs::read(lab.root().join("clone.zip")).unwrap();
        match change {
            "bytes" => {
                std::fs::write(
                    &path,
                    format!("{xml}<!--changed bytes, same version and layout-->"),
                )
                .unwrap();
            }
            "inode" => {
                std::fs::rename(&path, lab.dir.path().join("old.dat")).unwrap();
                std::fs::write(&path, &xml).unwrap();
            }
            "missing" => {
                std::fs::remove_file(&path).unwrap();
            }
            "disabled" => {
                use archivefs_core::dat::sources::{
                    DatSourceEntry, DatSourceKind, DatSourceRegistry,
                };
                let mut registry = DatSourceRegistry::new();
                let mut source = DatSourceEntry::new(
                    "fixture".into(),
                    "Fixture".into(),
                    path,
                    DatSourceKind::File,
                );
                source.enabled = false;
                registry.add(source).unwrap();
                environment
                    .catalogue_inventory
                    .as_mut()
                    .unwrap()
                    .local_registry = registry;
            }
            "reference" => {
                preview.inputs.reference = CatalogueRef::local("not-the-reviewed-reference");
            }
            _ => unreachable!(),
        }
        let result = execute(
            MameRequest::Publish {
                preview: Box::new(preview),
            },
            &environment,
            &mut |_| {},
        )
        .result;
        assert!(
            matches!(result, MameResult::Publish(ref result) if matches!(result.outcome, PublishOutcome::Refused(_))),
            "{change}: {result:?}"
        );
        assert!(!lab.destination().exists());
        assert!(!staging_root(&lab.root()).exists());
        assert!(!lab.journal().exists());
        assert_eq!(
            std::fs::read(lab.root().join("clone.zip")).unwrap(),
            before_zip
        );
    }
}

#[test]
fn untrusted_unversioned_and_non_arcade_catalogues_do_not_enable_reconstruction() {
    let lab = Lab::new();
    let environment = version_environment(&lab);
    let path = lab.inputs("clone").dat;
    std::fs::write(&path, versioned_xml("0.250", CLONE)).unwrap();
    let mut untrusted = lab.inputs("clone");
    untrusted.trusted = false;
    assert!(catalogue::load_selected(&untrusted, &environment).is_err());
    for xml in [
        versioned_xml("", CLONE),
        versioned_xml("not a version", CLONE),
        "<softwarelist name=\"x\"><software name=\"x\"/></softwarelist>".into(),
    ] {
        std::fs::write(&path, xml).unwrap();
        assert!(catalogue::load_selected(&lab.inputs("clone"), &environment).is_err());
    }
    assert!(!lab.journal().exists());
    let panel = MameReconstructionPanel::default();
    assert!(!panel.can_publish());
    assert!(panel.inputs().is_none());
}

#[test]
fn versioned_catalogue_worker_publishes_and_undoes_without_reinterpreting_history() {
    let lab = Lab::new();
    let environment = version_environment(&lab);
    let path = lab.inputs("clone").dat;
    std::fs::write(&path, versioned_xml("0.250", CLONE)).unwrap();
    let preview = version_preview(&lab, &environment);
    let digest = preview.plan.dat_sha256.clone();
    let result = execute(
        MameRequest::Publish {
            preview: Box::new(preview),
        },
        &environment,
        &mut |_| {},
    )
    .result;
    let transaction = match result {
        MameResult::Publish(result) => published(*result),
        other => panic!("{other:?}"),
    };
    let persisted = lab.history();
    assert_eq!(persisted[0], transaction);
    assert_eq!(
        transaction
            .unknown
            .get(
                archivefs_core::dat::mame_merged_reconstruction::MAME_RECONSTRUCTION_PROVENANCE_KEY
            )
            .and_then(|value| value.get("dat_version"))
            .and_then(serde_json::Value::as_str),
        Some("0.250")
    );
    assert!(
        serde_json::to_string(&transaction)
            .unwrap()
            .contains(&digest)
    );
    std::fs::write(&path, versioned_xml("0.260", b"new revision")).unwrap();
    assert_eq!(
        lab.history(),
        persisted,
        "selection changes never rewrite past receipts"
    );
    let undo = execute(
        MameRequest::Undo {
            transaction_id: transaction.transaction_id.clone(),
        },
        &environment,
        &mut |_| {},
    )
    .result;
    assert!(matches!(
        undo,
        MameResult::Undo(UndoOutcome::RolledBack { .. })
    ));
    assert!(!lab.destination().exists());
}

#[test]
fn changed_catalogue_cannot_be_reported_as_already_published() {
    let lab = Lab::new();
    let environment = version_environment(&lab);
    let first = version_preview(&lab, &environment);
    let result = execute(
        MameRequest::Publish {
            preview: Box::new(first),
        },
        &environment,
        &mut |_| {},
    )
    .result;
    assert!(
        matches!(result, MameResult::Publish(ref result) if matches!(result.outcome, PublishOutcome::Published { .. })),
        "{result:?}"
    );
    let again = version_preview(&lab, &environment);
    assert_eq!(again.destination, DestinationStatus::AlreadyPublished);
    let records = lab.history();
    let path = lab.inputs("clone").dat;
    std::fs::write(
        path,
        format!("{}<!--metadata change-->", versioned_xml("0.250", CLONE)),
    )
    .unwrap();
    let result = execute(
        MameRequest::Publish {
            preview: Box::new(again),
        },
        &environment,
        &mut |_| {},
    )
    .result;
    assert!(
        matches!(result, MameResult::Publish(ref result) if matches!(result.outcome, PublishOutcome::Refused(_))),
        "stale catalogue was reported as current: {result:?}"
    );
    assert_eq!(lab.history(), records);
}

#[test]
fn catalogue_change_at_the_last_worker_boundary_refuses_before_staging() {
    let lab = Lab::new();
    let environment = version_environment(&lab);
    let preview = version_preview(&lab, &environment);
    let path = lab.inputs("clone").dat;
    let mut changed = false;
    let result = execute(
        MameRequest::Publish {
            preview: Box::new(preview),
        },
        &environment,
        &mut |phase| {
            if phase.starts_with("Staging and verifying") {
                std::fs::write(
                    &path,
                    format!("{}<!--late change-->", versioned_xml("0.250", CLONE)),
                )
                .unwrap();
                changed = true;
            }
        },
    )
    .result;
    assert!(changed);
    assert!(
        matches!(result, MameResult::Publish(ref result) if matches!(result.outcome, PublishOutcome::Refused(_))),
        "{result:?}"
    );
    assert!(!staging_root(&lab.root()).exists());
    assert!(!lab.journal().exists());
    assert!(!lab.destination().exists());
}

#[test]
fn two_installed_versions_are_explicit_choices_and_selection_invalidates_preview() {
    use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind};
    let lab = Lab::new();
    let mut environment = version_environment(&lab);
    let second_path = lab.dir.path().join("mame-0260.dat");
    std::fs::write(
        &second_path,
        versioned_xml("0.260", b"different clone revision"),
    )
    .unwrap();
    environment
        .catalogue_inventory
        .as_mut()
        .unwrap()
        .local_registry
        .add(DatSourceEntry::new(
            "version260".into(),
            "MAME 0.260 (synthetic)".into(),
            second_path.clone(),
            DatSourceKind::File,
        ))
        .unwrap();
    let first = version_preview(&lab, &environment);
    let mut second_inputs = lab.inputs("clone");
    second_inputs.dat = second_path;
    second_inputs.reference = CatalogueRef::local("version260");
    let selected = catalogue::load_selected(&second_inputs, &environment).unwrap();
    assert_eq!(selected.dat.version, "0.260");
    assert_ne!(
        first.catalogue.as_ref().unwrap().reference,
        selected.binding.reference
    );
    assert_ne!(first.plan.dat_sha256, selected.binding.sha256);
    let second = match execute(
        MameRequest::Preview {
            inputs: second_inputs.clone(),
        },
        &environment,
        &mut |_| {},
    )
    .result
    {
        MameResult::Preview(Ok(PreviewOutcome::Ready(preview))) => preview,
        other => panic!("{other:?}"),
    };
    assert_eq!(second.plan.dat_version, "0.260");
    assert!(!second.gate().is_ready());
    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    panel.root = Some(lab.root());
    panel.dat = Some(lab.inputs("clone").dat);
    panel.set = "clone".into();
    panel.preview = Some(first);
    assert!(panel.can_publish());
    panel.catalogue = Some(second_inputs.reference);
    panel.dat = Some(second_inputs.dat);
    assert!(!panel.can_publish());
    panel.catalogue_trusted = false;
    assert!(!panel.can_publish());
}

#[test]
fn unsupported_legacy_root_refuses_and_supported_0174_keeps_its_declared_version() {
    let lab = Lab::new();
    let environment = version_environment(&lab);
    let xml = versioned_xml("0.174", CLONE)
        .replacen(
            "<mame build=\"0.174\">",
            "<datafile><header><name>MAME Arcade</name><description>MAME Arcade 0.174</description><version>0.174</version></header>",
            1,
        )
        .replace("</mame>", "</datafile>");
    std::fs::write(lab.inputs("clone").dat, xml).unwrap();
    let refusal = catalogue::load_selected(&lab.inputs("clone"), &environment)
        .err()
        .expect("unsupported datafile root must not discard machine semantics");
    assert!(
        refusal.contains("Expected a MAME document root"),
        "{refusal}"
    );
    std::fs::write(lab.inputs("clone").dat, versioned_xml("0.174", CLONE)).unwrap();
    let selected = catalogue::load_selected(&lab.inputs("clone"), &environment).unwrap();
    assert_eq!(selected.dat.version, "0.174");
    assert_eq!(selected.dat.parsed.source.version.as_deref(), Some("0.174"));
    assert!(
        selected
            .dat
            .parsed
            .games
            .iter()
            .find(|g| g.name == "bios")
            .unwrap()
            .is_bios
            .as_deref()
            == Some("yes")
    );
}

#[test]
fn mame_flow_exposes_a_playing_library_handoff() {
    let lab = Lab::new();
    let mut panel = panel_with(&lab, Arc::new(|_, _, _| unreachable!()));
    let text = render(&mut panel);
    assert!(text.contains("MAME Playing Library"), "{text}");
}

mod playing_checks;
