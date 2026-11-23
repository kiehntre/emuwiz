//! Bulk rename eligibility: a RomM title is a hint and never renames a file in
//! bulk, at preview or at Apply (a stale preview included); a verified
//! catalogue name does.

use super::hint_gate::{
    UNVERIFIED_HINT_REASON, UNVERIFIED_NAME_REASON, block_unverified_names, check_live_names,
    refuse_unverified_names, refuse_unverified_names_with, unverified_name_label,
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
        disc_set: None,
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

/// Owner decision 2026-11-22: link modes are gated too. Reorganising
/// existing links and building a linked library never take a RomM name; a
/// link that keeps its file's own name, and a verified name, are unaffected.
#[test]
#[allow(clippy::single_element_loop)] // one link mode today; more join the list
fn every_link_mode_refuses_romm_names_and_keeps_verified_and_unrenamed_links() {
    // Reorganise existing links needs real links (its own name is the
    // target's): see existing_links_are_judged_by_their_target_game_file.
    for mode in [OrganisationMode::BuildLinkedLibrary] {
        let (mut plan, choices) = mixed_preview(mode);
        assert_eq!(block_unverified_names(&mut plan, &choices), 1, "{mode:?}");
        assert_eq!(
            status_of(&plan, HINT_SRC),
            OrganisationStatus::Blocked,
            "{mode:?}"
        );
        let reason = plan
            .entries
            .iter()
            .find(|e| e.source_path == Path::new(HINT_SRC))
            .and_then(|e| e.reason.clone())
            .unwrap();
        assert_eq!(
            reason,
            super::hint_gate::UNVERIFIED_LINK_HINT_REASON,
            "{mode:?}"
        );
        assert!(!reason.contains("rename"), "{reason}");
        assert_eq!(
            status_of(&plan, VERIFIED_SRC),
            OrganisationStatus::Suggested
        );
        assert_eq!(status_of(&plan, KEEP_SRC), OrganisationStatus::Suggested);

        // Apply, stale preview: the RomM row is refused in this mode too.
        let (stale, _) = mixed_preview(mode);
        let error = refuse_unverified_names_with(&stale, &all_approved(&stale), &|p| {
            if p == Path::new(VERIFIED_SRC) {
                verified(p, "Verified One (USA).nes")
            } else {
                romm_only(p, "Hint One (USA)")
            }
        })
        .unwrap_err();
        assert!(error.contains("h_one.nes"), "{mode:?}: {error}");
        assert!(error.contains("files and links"), "{error}");
    }
}

#[test]
fn a_new_name_from_neither_source_gets_its_own_reason_not_the_romm_one() {
    // Rename/Move: a new name RomM did not give is "not verified", not a hint.
    let (mut plan, mut choices) = mixed_preview(OrganisationMode::MoveRealFile);
    choices.insert(
        PathBuf::from(HINT_SRC),
        choose_name(V::None, None, Path::new(HINT_SRC)),
    );
    block_unverified_names(&mut plan, &choices);
    let row = plan
        .entries
        .iter()
        .find(|e| e.source_path == Path::new(HINT_SRC))
        .unwrap();
    assert_eq!(row.reason.as_deref(), Some(UNVERIFIED_NAME_REASON));
    assert_eq!(
        unverified_name_label(row),
        Some("Proposed name (not verified, not applied)")
    );
}

/// Link modes: when the file's own name (the fallback) had to be changed to
/// be a safe link name, the row says so; it is not called a RomM hint and is
/// not counted as one, even when RomM has a title for the game.
#[test]
fn a_link_whose_own_name_had_to_change_is_not_called_a_romm_hint() {
    for mode in [
        OrganisationMode::OrganiseSymlinkOnly,
        OrganisationMode::BuildLinkedLibrary,
    ] {
        let source = Path::new("/roms/nes/ h_one.nes");
        let mut plan = plan(mode, vec![entry("/roms/nes/ h_one.nes", "h_one.nes", mode)]);
        let mut choices = BTreeMap::new();
        choices.insert(source.to_path_buf(), romm_only(source, "Hint One (USA)"));
        assert_eq!(block_unverified_names(&mut plan, &choices), 1);
        let row = &plan.entries[0];
        assert_eq!(
            row.reason.as_deref(),
            Some(super::hint_gate::UNSAFE_LINK_FALLBACK_REASON),
            "{mode:?}"
        );
        assert_eq!(
            unverified_name_label(row),
            Some("Own file name needs changing (no link made)")
        );
        assert_eq!(name_counts(&plan, &choices).romm_fallbacks, 0);
    }
}

fn page_texts(state: &mut RomOrganisationPageState) -> Vec<String> {
    let ctx = egui::Context::default();
    let output = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                show_rom_organisation_page_with_busy(ui, state, false, false);
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
    let mut texts = Vec::new();
    output
        .shapes
        .iter()
        .for_each(|c| collect(&c.shape, &mut texts));
    texts
}

fn blocked_hint_page(mode: OrganisationMode) -> RomOrganisationPageState {
    let mut state = RomOrganisationPageState::default();
    state.set_mode(mode);
    let (mut plan, choices) = mixed_preview(mode);
    plan.entries
        .retain(|e| e.source_path == Path::new(HINT_SRC));
    block_unverified_names(&mut plan, &choices);
    state.plan = Some(plan);
    state.name_choices = choices;
    state
}

#[test]
fn the_existing_link_page_says_the_hint_is_not_applied_and_no_action_runs() {
    let mut state = blocked_hint_page(OrganisationMode::OrganiseSymlinkOnly);
    let texts = page_texts(&mut state);
    let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
    assert!(has("RomM suggests (hint only, not applied)"), "{texts:?}");
    assert!(
        has("Action: none (the new name is not verified)"),
        "{texts:?}"
    );
    assert!(!has("Will become:"), "{texts:?}");
    assert!(!has("Action: Move existing link"), "{texts:?}");
    assert!(has("not used to name files or links in bulk"), "{texts:?}");
    assert!(state.approved.is_empty());
}

#[test]
fn the_linked_library_page_says_no_link_is_created_for_a_hint() {
    let mut state = blocked_hint_page(OrganisationMode::BuildLinkedLibrary);
    let texts = page_texts(&mut state);
    let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
    assert!(
        has("RomM suggests (hint only, not applied), no link is created"),
        "{texts:?}"
    );
    assert!(!has("Destination link:"), "{texts:?}");
    assert!(state.approved.is_empty());
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
    let mut state = RomOrganisationPageState {
        mode: OrganisationMode::RenameInPlace,
        ..Default::default()
    };
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

/// The check both Apply paths run refuses a stale preview holding an approved
/// RomM row before any database is read (this path does not exist).
#[test]
fn the_shared_apply_check_refuses_a_stale_romm_row_first() {
    let (plan, _) = mixed_preview(OrganisationMode::RenameInPlace);
    let names = NameResolver::for_test(None, None);
    let error = check_live_names(
        &plan,
        &all_approved(&plan),
        Path::new("/nonexistent/emuwiz-hint-gate-test.sqlite3"),
        &names,
    )
    .unwrap_err();
    assert!(error.contains("verified catalogue names only"), "{error}");
}

/// Both Apply paths go through the shared check, and neither keeps a
/// revalidation of its own that could run without it.
#[test]
fn standard_apply_and_advanced_protection_both_run_the_shared_check() {
    let page = include_str!("rom_organisation_page.rs");
    let calls = |needle: &str| page.matches(needle).count();
    assert_eq!(
        calls("hint_gate::check_live_names(plan, &self.approved, database.path(), &names)"),
        1,
        "Standard Apply"
    );
    assert_eq!(
        calls("hint_gate::check_live_names(plan, approved, &database, &names)?"),
        1,
        "Advanced Protection"
    );
    assert_eq!(calls("revalidate_organisation_plan("), 0);
    assert_eq!(
        calls("block_unverified_names(&mut plan, &name_choices)"),
        1,
        "preview"
    );
}

/// Owner policy 2026-11-22 (linked libraries): a link takes a verified
/// catalogue name; any other link keeps the file's own name, so no game is
/// left out and no RomM title is used. Rename and Move keep the chosen name
/// (and the row is then blocked unless verified).
#[test]
fn link_modes_fall_back_to_the_file_name_and_file_modes_keep_the_choice() {
    use super::hint_gate::bulk_name;
    let src = Path::new("/roms/nes/h_one.nes");
    let dat = verified(src, "Verified One (USA).nes");
    let romm = romm_only(src, "Hint One (USA)");
    let none = choose_name(V::None, None, src);
    for mode in [
        OrganisationMode::OrganiseSymlinkOnly,
        OrganisationMode::BuildLinkedLibrary,
    ] {
        assert_eq!(
            bulk_name(mode, &dat).as_deref(),
            Some("Verified One (USA).nes")
        );
        assert_eq!(
            bulk_name(mode, &romm),
            None,
            "{mode:?}: RomM title not used"
        );
        assert_eq!(bulk_name(mode, &none), None, "{mode:?}");
    }
    for mode in [
        OrganisationMode::RenameInPlace,
        OrganisationMode::MoveRealFile,
    ] {
        assert_eq!(bulk_name(mode, &romm), romm.name, "{mode:?}");
    }
}

#[test]
fn link_rows_say_where_the_link_name_comes_from() {
    use super::hint_gate::link_name_line;
    let src = Path::new("/roms/nes/h_one.nes");
    assert_eq!(
        link_name_line(Some(&verified(src, "Verified One (USA).nes"))),
        "Link name: verified catalogue name (synthetic-nes)"
    );
    assert_eq!(
        link_name_line(Some(&romm_only(src, "Hint One (USA)"))),
        "Link name: the game file's own name (RomM suggests 'Hint One (USA)', not used: not verified)"
    );
    assert_eq!(
        link_name_line(Some(&choose_name(V::None, None, src))),
        "Link name: the game file's own name (no verified catalogue name)"
    );
    assert_eq!(
        link_name_line(None),
        "Link name: the game file's own name (no verified catalogue name)"
    );
}

/// A verified-status change between preview and Apply is reported as a name
/// change, not as a platform change, in either direction.
#[test]
fn a_name_that_changed_since_the_preview_is_refused_with_that_reason() {
    use super::hint_gate::refuse_changed_names_with;
    // Linked library: previewed under its own name (RomM only), verified now.
    let mode = OrganisationMode::BuildLinkedLibrary;
    let linked = plan(mode, vec![entry(HINT_SRC, "h_one.nes", mode)]);
    let approved = all_approved(&linked);
    let error = refuse_changed_names_with(&linked, &approved, &|p| {
        verified(p, "Hint One Verified (USA).nes")
    })
    .unwrap_err();
    assert!(
        error.contains("The name for /roms/nes/h_one.nes changed"),
        "{error}"
    );
    assert!(error.contains("'h_one.nes'") && error.contains("'Hint One Verified (USA).nes'"));
    assert!(!error.contains("platform"), "{error}");
    // Unchanged: RomM only, still the own name.
    assert_eq!(
        refuse_changed_names_with(&linked, &approved, &|p| romm_only(p, "Hint One (USA)")),
        Ok(())
    );
    // Rename: previewed verified, the catalogue check is gone now.
    let mode = OrganisationMode::RenameInPlace;
    let renamed = plan(
        mode,
        vec![entry(VERIFIED_SRC, "Verified One (USA).nes", mode)],
    );
    let error = refuse_changed_names_with(&renamed, &all_approved(&renamed), &|p| {
        choose_name(V::None, None, p)
    })
    .unwrap_err();
    assert!(error.contains("changed since the preview"), "{error}");
}

/// Reorganise existing links (owner policy 2026-11-22): a link's own name is
/// its target game file's name, so an old RomM-titled link is a new name (it
/// is blocked when a plan still carries it, and refused at Apply), a link
/// renamed to its game file's name is not, and a link whose target cannot be
/// checked fails closed.
#[test]
fn existing_links_are_judged_by_their_target_game_file() {
    use super::hint_gate::{UNVERIFIED_LINK_HINT_REASON, own_name};
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let games = root.join("games");
    let links = root.join("links");
    std::fs::create_dir_all(&games).unwrap();
    std::fs::create_dir_all(&links).unwrap();
    for game in ["v_one.nes", "h_one.nes"] {
        std::fs::write(games.join(game), game).unwrap();
    }
    let verified_link = links.join("Alpha RomM Guess (USA).nes");
    let romm_link = links.join("Hint One (USA).nes");
    let broken_link = links.join("Broken.nes");
    std::os::unix::fs::symlink(games.join("v_one.nes"), &verified_link).unwrap();
    std::os::unix::fs::symlink(games.join("h_one.nes"), &romm_link).unwrap();
    std::os::unix::fs::symlink(games.join("gone.nes"), &broken_link).unwrap();
    let mode = OrganisationMode::OrganiseSymlinkOnly;
    assert_eq!(own_name(mode, &romm_link).as_deref(), Some("h_one.nes"));
    assert_eq!(own_name(mode, &broken_link), None);
    let row = |link: &Path, name: &str| {
        let mut row = entry(&link.to_string_lossy(), name, mode);
        row.destination_path = root.join("roms/nes").join(name);
        row
    };
    let mut choices = BTreeMap::new();
    choices.insert(
        verified_link.clone(),
        verified(&verified_link, "Verified One (USA).nes"),
    );
    choices.insert(romm_link.clone(), romm_only(&romm_link, "Hint One (USA)"));
    choices.insert(broken_link.clone(), romm_only(&broken_link, "Broken"));

    // A plan that still carries the old RomM-titled link name.
    let mut stale = plan(
        mode,
        vec![
            row(&verified_link, "Verified One (USA).nes"),
            row(&romm_link, "Hint One (USA).nes"),
            row(&broken_link, "Broken.nes"),
        ],
    );
    let approved = all_approved(&stale);
    let error =
        refuse_unverified_names_with(&stale, &approved, &|p| choices.get(p).cloned().unwrap())
            .unwrap_err();
    assert!(error.contains("Hint One (USA).nes"), "{error}");
    assert_eq!(block_unverified_names(&mut stale, &choices), 2);
    let find = |plan: &OrganisationPlan, link: &Path| {
        plan.entries
            .iter()
            .find(|e| e.source_path == link)
            .unwrap()
            .clone()
    };
    assert_eq!(
        find(&stale, &verified_link).status,
        OrganisationStatus::Suggested
    );
    let romm_row = find(&stale, &romm_link);
    assert_eq!(romm_row.status, OrganisationStatus::Blocked);
    assert_eq!(
        romm_row.reason.as_deref(),
        Some(UNVERIFIED_LINK_HINT_REASON)
    );
    assert_eq!(
        find(&stale, &broken_link).status,
        OrganisationStatus::Blocked
    );

    // The fallback the planner now gives: the game file's own name.
    let fallback = plan(mode, vec![row(&romm_link, "h_one.nes")]);
    assert_eq!(
        refuse_unverified_names_with(&fallback, &all_approved(&fallback), &|p| {
            choices.get(p).cloned().unwrap()
        }),
        Ok(())
    );
    let mut fallback = fallback;
    assert_eq!(block_unverified_names(&mut fallback, &choices), 0);
    assert_eq!(
        super::hint_gate::existing_link_line(&romm_link),
        format!("Links to game file: {}", games.join("h_one.nes").display())
    );
    assert!(super::hint_gate::existing_link_line(&broken_link).contains("broken"));
}
