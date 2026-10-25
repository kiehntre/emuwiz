//! Cheat sets already on this computer: classification, preview through the
//! ordinary chooser, and the refusals. Synthetic catalogue folders only; no
//! RetroArch, cache download or user file is involved.

use super::retroarch_cheat_sets::{
    NO_CATALOGUE, SetMatch, core_mechanism_note, discover, find_in_catalogue, preview,
};
use super::retroarch_cheats::unavailable_reason;
use archivefs_core::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
use archivefs_core::launch::RetroArchLaunchRequest;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

const PLATFORM_DIR: &str = "Sega - Mega Drive - Genesis";
const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const CHT: &str = "cheats = 2\ncheat0_desc = \"Infinite Lives\"\ncheat0_code = \"8000AA00\"\ncheat0_enable = true\ncheat1_desc = \"Max Rings\"\ncheat1_code = \"8000BB00\"\ncheat1_enable = false\n";

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "emuwiz-ra-cheat-sets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request(content: &Path) -> RetroArchLaunchRequest {
    RetroArchLaunchRequest {
        selected_content_path: content.to_path_buf(),
        expected_platform_id: "MegaDrive".into(),
        expected_game_key: SHA.into(),
        profile: ProfileRef {
            profile_kind: ProfileKind::Native,
            scope: ProfileScope::User,
        },
        core_stem: "genesis_plus_gx".into(),
        expected_appimage_executable: None,
        expected_executable_binding: None,
        expected_configuration_file: None,
    }
}

fn catalogue(root: &Root) -> PathBuf {
    root.write(&format!("catalogue/{PLATFORM_DIR}/Sonic (USA).cht"), CHT);
    root.write(
        &format!("catalogue/{PLATFORM_DIR}/Streets of Rage (USA).cht"),
        CHT,
    );
    root.write(
        "catalogue/Sega - Master System - Mark III/Sonic (USA).cht",
        CHT,
    );
    root.0.join("catalogue")
}

#[test]
fn a_title_only_match_is_possible_never_exact_and_needs_review() {
    let root = Root::new();
    let catalogue = catalogue(&root);
    let content = root.write("roms/Sonic (USA).md", "rom");
    let found = find_in_catalogue(&request(&content), &catalogue, false);
    let sonic = found
        .sets
        .iter()
        .find(|set| set.relative_path == format!("{PLATFORM_DIR}/Sonic (USA).cht"))
        .expect("the same-named set on this platform is listed");
    assert_eq!(sonic.matched, SetMatch::Uncertain);
    assert!(found.sets.iter().all(|set| set.matched != SetMatch::Exact));

    let mut choice = preview(&request(&content), &found, sonic).expect("previewed");
    assert_eq!(choice.rows.len(), 2);
    assert_eq!(choice.ticked_count(), 0, "nothing is ticked for the player");
    let lives = choice
        .rows
        .iter()
        .position(|row| row.candidate.title == "Infinite Lives")
        .unwrap();
    assert!(
        choice.rows[lives].on_by_default(),
        "the file's own enabled state is shown"
    );
    choice.rows[lives].ticked = true;
    let refused = choice.selections().unwrap_err();
    assert!(
        refused.contains("I reviewed this cheat for this game"),
        "{refused}"
    );
    choice.rows[lives].acknowledged = true;
    let selections = choice.selections().expect("user-approved selection");
    assert_eq!(selections.len(), 1);
    assert!(selections[0].review_acknowledged);
}

#[test]
fn a_set_for_another_platform_is_incompatible_and_cannot_be_previewed() {
    let root = Root::new();
    let catalogue = catalogue(&root);
    let content = root.write("roms/Sonic (USA).md", "rom");
    let found = find_in_catalogue(&request(&content), &catalogue, false);
    let other = found
        .sets
        .iter()
        .find(|set| set.relative_path.starts_with("Sega - Master System"))
        .expect("listed for transparency");
    assert_eq!(other.matched, SetMatch::Incompatible);
    assert!(preview(&request(&content), &found, other).is_err());
    assert_eq!(
        found.sets.last().map(|set| set.matched),
        Some(SetMatch::Incompatible),
        "incompatible sets are listed last"
    );
}

#[test]
fn an_approval_belongs_to_one_verified_game() {
    let root = Root::new();
    let catalogue = catalogue(&root);
    let content = root.write("roms/Sonic (USA).md", "rom");
    let found = find_in_catalogue(&request(&content), &catalogue, false);
    let set = found
        .sets
        .iter()
        .find(|set| set.matched == SetMatch::Uncertain)
        .unwrap();
    let choice = preview(&request(&content), &found, set).unwrap();
    assert!(choice.matches(&request(&content)));
    // Same file path, but it now verifies as another game.
    let mut other_game = request(&content);
    other_game.expected_game_key = "f".repeat(64);
    assert!(!choice.matches(&other_game));
    let mut other_core = request(&content);
    other_core.core_stem = "picodrive".into();
    assert!(!choice.matches(&other_core));
}

#[test]
fn a_set_that_changed_after_listing_is_not_opened() {
    let root = Root::new();
    let catalogue = catalogue(&root);
    let content = root.write("roms/Sonic (USA).md", "rom");
    let found = find_in_catalogue(&request(&content), &catalogue, false);
    let set = found
        .sets
        .iter()
        .find(|set| set.relative_path == format!("{PLATFORM_DIR}/Sonic (USA).cht"))
        .unwrap()
        .clone();
    root.write(
        &format!("catalogue/{PLATFORM_DIR}/Sonic (USA).cht"),
        "cheats = 1\ncheat0_desc = \"Swapped\"\ncheat0_code = \"FFFF0000\"\ncheat0_enable = false\n",
    );
    let error = preview(&request(&content), &found, &set).unwrap_err();
    assert!(error.contains("changed or disappeared"), "{error}");
}

#[test]
fn no_retrieved_catalogue_is_reported_without_creating_anything() {
    let root = Root::new();
    let cache = root.0.join("cheat-cache");
    let content = root.write("roms/Sonic (USA).md", "rom");
    let error = discover(&request(&content), &cache).unwrap_err();
    assert_eq!(error, NO_CATALOGUE);
    assert!(!cache.exists(), "the lookup never creates the cache");
}

#[test]
fn unsupported_cores_and_profiles_are_explained_not_claimed() {
    let content = PathBuf::from("/synthetic/Sonic (USA).md");
    let mut flatpak = request(&content);
    flatpak.profile.profile_kind = ProfileKind::Flatpak;
    assert!(unavailable_reason(&flatpak, false).is_some());
    assert!(unavailable_reason(&request(&content), true).is_some());
    let note = core_mechanism_note("genesis_plus_gx");
    assert!(note.contains("genesis_plus_gx"));
    assert!(note.contains("cannot tell in advance"));
}

fn render_chooser(state: &mut super::RetroArchLaunchState, request: &RetroArchLaunchRequest) {
    let ctx = eframe::egui::Context::default();
    let _ = ctx.run(eframe::egui::RawInput::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            super::retroarch_cheats::show_chooser(ui, state, request, false);
        });
    });
}

#[test]
fn an_approval_is_dropped_on_reselection_and_never_comes_back() {
    let root = Root::new();
    let catalogue = catalogue(&root);
    let content = root.write("roms/Sonic (USA).md", "rom");
    let game = request(&content);
    let found = find_in_catalogue(&game, &catalogue, false);
    let set = found
        .sets
        .iter()
        .find(|set| set.relative_path == format!("{PLATFORM_DIR}/Sonic (USA).cht"))
        .unwrap();
    let mut choice = preview(&game, &found, set).unwrap();
    for row in &mut choice.rows {
        row.ticked = true;
        row.acknowledged = true;
    }
    let mut other_game = game.clone();
    other_game.expected_game_key = "f".repeat(64);
    let mut other_profile = game.clone();
    other_profile.profile.profile_kind = ProfileKind::AppImage;
    let mut other_core = game.clone();
    other_core.core_stem = "picodrive".into();
    let mut other_content = game.clone();
    other_content.selected_content_path = root.write("roms/Sonic 2 (USA).md", "rom2");
    for changed in [other_game, other_profile, other_core, other_content] {
        let mut state = super::RetroArchLaunchState {
            cheats: Some(choice.clone()),
            ..Default::default()
        };
        render_chooser(&mut state, &game);
        assert!(
            state.cheats.is_some(),
            "the approval stays for its own game"
        );
        render_chooser(&mut state, &changed);
        assert!(
            state.cheats.is_none(),
            "another selection drops the approval"
        );
        render_chooser(&mut state, &game);
        assert!(
            state.cheats.is_none(),
            "returning to the game needs a fresh review"
        );
    }
}

#[test]
fn a_default_enabled_cheat_is_never_selected_without_tick_and_review() {
    let root = Root::new();
    let catalogue = catalogue(&root);
    let content = root.write("roms/Sonic (USA).md", "rom");
    let found = find_in_catalogue(&request(&content), &catalogue, false);
    let set = found
        .sets
        .iter()
        .find(|set| set.relative_path == format!("{PLATFORM_DIR}/Sonic (USA).cht"))
        .unwrap();
    let mut choice = preview(&request(&content), &found, set).unwrap();
    let lives = choice
        .rows
        .iter()
        .position(|row| row.on_by_default())
        .expect("the fixture enables one cheat by default");
    assert!(!choice.rows[lives].ticked);
    assert!(!choice.rows[lives].acknowledged);
    assert!(
        choice.selections().is_err(),
        "nothing is selected by default"
    );
    choice.rows[lives].acknowledged = true;
    assert!(
        choice.selections().is_err(),
        "a review alone does not tick the cheat"
    );
    choice.rows[lives].ticked = true;
    assert_eq!(choice.selections().unwrap().len(), 1);
}

fn ranked(
    classification: archivefs_core::patch_manager::CheatCandidateClassification,
    evidence: &[archivefs_core::patch_manager::CheatCandidateEvidenceKind],
) -> archivefs_core::patch_manager::CheatCandidate {
    archivefs_core::patch_manager::CheatCandidate {
        source_evidence: Vec::new(),
        catalogue_relative_path: format!("{PLATFORM_DIR}/Sonic (USA).cht"),
        display_name: "Sonic (USA)".into(),
        platform: Some("MegaDrive".into()),
        region: Some("USA".into()),
        revision: None,
        classification,
        confidence_score: 900,
        evidence: evidence
            .iter()
            .map(
                |kind| archivefs_core::patch_manager::CheatCandidateEvidence {
                    kind: *kind,
                    detail: String::new(),
                },
            )
            .collect(),
        cheat_count: 2,
        source_file_hash: None,
        auto_selectable: true,
        manually_selectable: true,
    }
}

#[test]
fn only_fingerprint_evidence_makes_a_set_exact() {
    use super::retroarch_cheat_sets::classify;
    use archivefs_core::patch_manager::CheatCandidateClassification as C;
    use archivefs_core::patch_manager::CheatCandidateEvidenceKind as E;
    // The ranker calls title + platform + region "verified exact"; names are
    // not a game binding, so it stays a possible match.
    let by_name = ranked(
        C::VerifiedExact,
        &[E::ExactNormalizedTitle, E::PlatformMatch, E::RegionMatch],
    );
    assert_eq!(classify(&by_name).0, SetMatch::Uncertain);
    assert_eq!(
        classify(&ranked(C::Strong, &[E::FilenameSimilarity])).0,
        SetMatch::Uncertain
    );
    assert_eq!(
        classify(&ranked(C::VerifiedExact, &[E::ExactContentHash])).0,
        SetMatch::Exact
    );
    assert_eq!(
        classify(&ranked(C::Ambiguous, &[E::ExactSerial])).0,
        SetMatch::Uncertain,
        "a tie is never exact"
    );
    assert_eq!(
        classify(&ranked(
            C::VerifiedExact,
            &[E::ExactContentHash, E::PlatformMismatch]
        ))
        .0,
        SetMatch::Uncertain
    );
}
