//! Bulk rename eligibility: a RomM title is a hint and never renames a file in
//! bulk, at preview or at Apply (a stale preview included); a verified
//! catalogue name does.

use super::hint_gate::{
    UNVERIFIED_HINT_REASON, block_unverified_names, refuse_unverified_names,
    refuse_unverified_names_with,
};
use super::*;
use archivefs_core::VerifiedDatName as V;
use std::collections::BTreeMap;

fn verified(source: &Path, rom_name: &str) -> NameChoice {
    choose_name(
        V::Usable {
            rom_name: rom_name.into(),
            game_name: rom_name.into(),
            dat_source_id: "synthetic-nes".into(),
            through_archive_member: false,
        },
        Some("Some RomM Title.nes".into()),
        source,
    )
}

fn romm_only(source: &Path, title: &str) -> NameChoice {
    choose_name(V::None, Some(title.into()), source)
}

fn entry(source: &str, name: &str, mode: OrganisationMode) -> OrganisationPlanEntry {
    let source = PathBuf::from(source);
    OrganisationPlanEntry {
        source_identity: None,
        catalogue_binding: None,
        destination_path: source.with_file_name(name),
        source_path: source,
        platform: Some("NES".into()),
        platform_display_name: "Nintendo Entertainment System".into(),
        platform_source: "Manual".into(),
        slug: None,
        layout_folder: None,
        mode,
        content_classification: None,
        original_metadata: Default::default(),
        status: OrganisationStatus::Suggested,
        reason: None,
    }
}

fn plan(mode: OrganisationMode, entries: Vec<OrganisationPlanEntry>) -> OrganisationPlan {
    OrganisationPlan {
        master_root: PathBuf::from("/roms"),
        mode,
        content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
        classifier_version: "test".into(),
        generation: 7,
        entries,
    }
}

const VERIFIED_SRC: &str = "/roms/nes/v_one.nes";
const HINT_SRC: &str = "/roms/nes/h_one.nes";
const KEEP_SRC: &str = "/roms/nes/Kept (USA).nes";

/// A preview as it was built before the rule: every row Suggested.
fn mixed_preview(mode: OrganisationMode) -> (OrganisationPlan, BTreeMap<PathBuf, NameChoice>) {
    let plan = plan(
        mode,
        vec![
            entry(VERIFIED_SRC, "Verified One (USA).nes", mode),
            entry(HINT_SRC, "Hint One (USA).nes", mode),
            entry(KEEP_SRC, "Kept (USA).nes", mode),
        ],
    );
    let mut choices = BTreeMap::new();
    choices.insert(
        PathBuf::from(VERIFIED_SRC),
        verified(Path::new(VERIFIED_SRC), "Verified One (USA).nes"),
    );
    choices.insert(
        PathBuf::from(HINT_SRC),
        romm_only(Path::new(HINT_SRC), "Hint One (USA)"),
    );
    choices.insert(
        PathBuf::from(KEEP_SRC),
        choose_name(V::None, None, Path::new(KEEP_SRC)),
    );
    (plan, choices)
}

fn all_approved(plan: &OrganisationPlan) -> BTreeSet<String> {
    plan.entries
        .iter()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect()
}

fn status_of(plan: &OrganisationPlan, source: &str) -> OrganisationStatus {
    plan.entries
        .iter()
        .find(|e| e.source_path == Path::new(source))
        .unwrap()
        .status
}

#[test]
fn a_romm_hint_row_is_blocked_with_the_reason_and_keeps_its_hint_visible() {
    let (mut plan, choices) = mixed_preview(OrganisationMode::RenameInPlace);
    assert_eq!(block_unverified_names(&mut plan, &choices), 1);
    assert_eq!(
        status_of(&plan, VERIFIED_SRC),
        OrganisationStatus::Suggested
    );
    assert_eq!(status_of(&plan, KEEP_SRC), OrganisationStatus::Suggested);
    let hint = plan
        .entries
        .iter()
        .find(|e| e.source_path == Path::new(HINT_SRC))
        .unwrap();
    assert_eq!(hint.status, OrganisationStatus::Blocked);
    assert_eq!(hint.reason.as_deref(), Some(UNVERIFIED_HINT_REASON));
    // The RomM name is still shown, not erased; the hint stays counted.
    assert_eq!(
        hint.destination_path,
        Path::new("/roms/nes/Hint One (USA).nes")
    );
    let counts = name_counts(&plan, &choices);
    assert_eq!((counts.dat_changes, counts.romm_fallbacks), (1, 1));
    // Only the verified row is approved by a fresh preview.
    let approved: Vec<_> = plan.suggested().map(|e| e.source_path.clone()).collect();
    assert_eq!(
        approved,
        vec![PathBuf::from(VERIFIED_SRC), PathBuf::from(KEEP_SRC)]
    );
}

#[test]
fn a_renaming_row_without_a_recorded_name_choice_is_blocked() {
    let (mut plan, mut choices) = mixed_preview(OrganisationMode::RenameInPlace);
    choices.remove(Path::new(VERIFIED_SRC));
    assert_eq!(block_unverified_names(&mut plan, &choices), 2);
    assert_eq!(status_of(&plan, VERIFIED_SRC), OrganisationStatus::Blocked);
}

#[test]
fn a_move_that_would_rename_from_a_hint_is_blocked_too() {
    let (mut plan, choices) = mixed_preview(OrganisationMode::MoveRealFile);
    assert_eq!(block_unverified_names(&mut plan, &choices), 1);
    assert_eq!(status_of(&plan, HINT_SRC), OrganisationStatus::Blocked);
}

#[test]
fn a_linked_library_never_touches_real_files_and_is_unchanged() {
    let (mut plan, choices) = mixed_preview(OrganisationMode::BuildLinkedLibrary);
    assert_eq!(block_unverified_names(&mut plan, &choices), 0);
    assert!(
        refuse_unverified_names_with(&plan, &all_approved(&plan), &|p| { romm_only(p, "x") })
            .is_ok()
    );
}

#[test]
fn apply_refuses_a_stale_preview_that_still_approves_a_romm_hint() {
    // Built before the rule (or by any other path): nothing was blocked.
    let (plan, _) = mixed_preview(OrganisationMode::RenameInPlace);
    let approved = all_approved(&plan);
    let live = |p: &Path| {
        if p == Path::new(VERIFIED_SRC) {
            verified(p, "Verified One (USA).nes")
        } else {
            romm_only(p, "Hint One (USA)")
        }
    };
    let error = refuse_unverified_names_with(&plan, &approved, &live).unwrap_err();
    assert!(error.contains("h_one.nes"), "{error}");
    assert!(error.contains("Nothing was changed"), "{error}");
}

#[test]
fn apply_uses_the_real_resolver_and_refuses_a_romm_title() {
    use archivefs_core::identity_source::cache::{CACHE_FORMAT_VERSION, IdentityCache};
    use archivefs_core::identity_source::model::{
        ExternalIdentityRecord, ExternalVerification, IdentityProvider,
    };
    let record = ExternalIdentityRecord {
        provider: IdentityProvider::Romm,
        server_id: "t".into(),
        provider_platform_id: None,
        provider_game_id: "1".into(),
        provider_file_id: None,
        provider_path: "x/h_one.nes".into(),
        archivefs_path: Some(PathBuf::from(HINT_SRC)),
        title: Some("Hint One (USA)".into()),
        platform_candidate: None,
        provider_platform_name: None,
        regions: Vec::new(),
        revision: None,
        hashes: Vec::new(),
        file_size_bytes: None,
        metadata_provider_ids: Vec::new(),
        artwork: None,
        related_files: Vec::new(),
        sibling_game_ids: Vec::new(),
        imported_at_unix_seconds: 1,
        provider_updated_at: None,
        // Even a RomM record marked strong is a hint for bulk rename.
        verification: ExternalVerification::StrongExternal,
        conflicts: Vec::new(),
        evidence: Vec::new(),
        synopsis: None,
        genres: Vec::new(),
        players: None,
        rating: None,
        release_year: None,
        howlongtobeat: None,
    };
    let cache = IdentityCache {
        format_version: CACHE_FORMAT_VERSION,
        provider: IdentityProvider::Romm,
        server_id: "t".into(),
        server_version: None,
        source_fingerprint: "f".into(),
        imported_at_unix_seconds: 1,
        platforms: Vec::new(),
        records: vec![record],
        rejected_hashes: Vec::new(),
        unknown_platforms: Vec::new(),
        server_reported_total: Some(1),
    };
    let names = NameResolver::for_test(None, Some(&cache));
    assert_eq!(names.choose(Path::new(HINT_SRC)).source, NameSource::Romm);
    let plan = plan(
        OrganisationMode::RenameInPlace,
        vec![entry(
            HINT_SRC,
            "Hint One (USA).nes",
            OrganisationMode::RenameInPlace,
        )],
    );
    assert!(refuse_unverified_names(&plan, &all_approved(&plan), &names).is_err());
}

#[test]
fn apply_refuses_a_verified_preview_row_whose_check_went_stale() {
    let (mut plan, choices) = mixed_preview(OrganisationMode::RenameInPlace);
    block_unverified_names(&mut plan, &choices);
    let approved = all_approved(&plan);
    // The catalogue check is no longer usable: only RomM is left live.
    let error =
        refuse_unverified_names_with(&plan, &approved, &|p| romm_only(p, "Other")).unwrap_err();
    assert!(error.contains("v_one.nes"), "{error}");
}

#[test]
fn apply_accepts_verified_names_and_ignores_unapproved_or_unrenamed_rows() {
    let (plan, _) = mixed_preview(OrganisationMode::RenameInPlace);
    let approved: BTreeSet<String> = [VERIFIED_SRC, KEEP_SRC].map(String::from).into();
    let live = |p: &Path| {
        if p == Path::new(VERIFIED_SRC) {
            verified(p, "Verified One (USA).nes")
        } else {
            romm_only(p, "Hint One (USA)")
        }
    };
    // The hint row is in the plan but not approved; the kept row is not renamed.
    assert_eq!(
        refuse_unverified_names_with(&plan, &approved, &live),
        Ok(())
    );
}

#[test]
fn the_page_labels_a_hint_row_and_offers_no_apply_for_it() {
    let mut state = RomOrganisationPageState::default();
    state.mode = OrganisationMode::RenameInPlace;
    let (mut plan, choices) = mixed_preview(OrganisationMode::RenameInPlace);
    plan.entries
        .retain(|e| e.source_path == Path::new(HINT_SRC));
    block_unverified_names(&mut plan, &choices);
    state.plan = Some(plan);
    state.name_choices = choices;
    let ctx = egui::Context::default();
    let mut texts = Vec::new();
    let output = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                show_quick_rename_page_with_busy(ui, &mut state, false);
            });
        });
    });
    fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(nested) => nested.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }
    output
        .shapes
        .iter()
        .for_each(|c| collect(&c.shape, &mut texts));
    let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
    assert!(has("RomM suggests (hint only, not applied)"), "{texts:?}");
    assert!(has("hint (RomM title, not verified)"), "{texts:?}");
    assert!(has("unverified RomM hint"), "{texts:?}");
    // Nothing is selected for Apply: the hint row is not approved.
    assert!(has("0 file(s) selected"), "{texts:?}");
    assert!(has("Apply approved organisation (0)"), "{texts:?}");
    assert!(state.approved.is_empty());
}
