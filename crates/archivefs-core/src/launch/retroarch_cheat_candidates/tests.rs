use super::*;
use crate::game_identity::{
    IdentityConfidence, IdentityEvidence, IdentityKind, IdentityProvenance, IdentityStatus,
};
use crate::patch_manager::{
    CheatPackAssociation, CheatPackCatalogueGame, CheatPackIdentityRequirement, CheatPackLimits,
    UserCheatLibraryGame, preview_cheat_pack,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use tempfile::{TempDir, tempdir};

fn fact(kind: IdentityKind, value: &str) -> IdentityEvidence {
    IdentityEvidence {
        kind,
        status: IdentityStatus::Verified,
        value: Some(value.into()),
        confidence: IdentityConfidence::StructuredMetadata,
        provenance: IdentityProvenance {
            archive_path: PathBuf::from("/unopened/game.rom"),
            member_path: None,
            member_index: None,
            method: "fixture evidence".into(),
        },
        diagnostic: String::new(),
    }
}

fn game() -> CheatPackCatalogueGame {
    CheatPackCatalogueGame {
        game: UserCheatLibraryGame {
            game_id: "g".into(),
            title: "Example".into(),
            platform: Some("NES".into()),
            ..Default::default()
        },
        facts: vec![fact(IdentityKind::LooseRomSha256, &"a".repeat(64))],
        revision: None,
    }
}

fn exact() -> CheatPackAssociation {
    CheatPackAssociation {
        title: Some("Example".into()),
        platform: Some("NES".into()),
        identities: vec![CheatPackIdentityRequirement {
            kind: IdentityKind::LooseRomSha256,
            value: "a".repeat(64),
        }],
        ..Default::default()
    }
}

fn weak() -> CheatPackAssociation {
    CheatPackAssociation {
        title: Some("Example".into()),
        platform: Some("NES".into()),
        ..Default::default()
    }
}

fn write(root: &Path, name: &str, text: &str) {
    fs::write(root.join(name), text).unwrap();
}

fn cht(desc: &str, code: &str) -> String {
    format!(
        "cheats = 1\ncheat0_desc = \"{desc}\"\ncheat0_code = \"{code}\"\ncheat0_enable = false\n"
    )
}

fn preview(root: &TempDir, association: CheatPackAssociation) -> CheatPackPreview {
    let associations: BTreeMap<PathBuf, CheatPackAssociation> = fs::read_dir(root.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| (PathBuf::from(e.file_name()), association.clone()))
        .collect();
    preview_cheat_pack(
        root.path(),
        &[game()],
        &associations,
        &BTreeSet::new(),
        &CheatPackLimits::default(),
    )
    .unwrap()
}

fn key(preview: &CheatPackPreview) -> String {
    preview
        .games
        .first()
        .map(|g| g.key.clone())
        .unwrap_or_default()
}

#[test]
fn a_clean_exact_cht_becomes_a_candidate_with_its_source_fingerprint() {
    let root = tempdir().unwrap();
    write(
        root.path(),
        "example.cht",
        &cht("Infinite Lives", "8000AA00"),
    );
    let p = preview(&root, exact());
    let found = retroarch_launch_candidates(&p, &key(&p));
    assert!(found.excluded.is_empty(), "{:?}", found.excluded);
    assert_eq!(found.candidates.len(), 1);
    let candidate = &found.candidates[0];
    assert_eq!(candidate.title, "Infinite Lives");
    assert_eq!(candidate.variants.len(), 1);
    let variant = &candidate.variants[0];
    assert_eq!(variant.format, CheatLaunchFormat::RetroArchCht);
    assert!(variant.entry.as_ref().unwrap().code.is_some());
    let source = &variant.source;
    assert_eq!(
        source.source_path.as_deref(),
        Some(root.path().join("example.cht").as_path())
    );
    let hash = source.source_sha256.as_deref().unwrap();
    assert_eq!(hash.len(), 64);
    assert_eq!(source.entry_index, Some(0));
    // The existing applicability engine decides how much review is needed; a
    // candidate is only ever one that is not a hard refusal.
    assert_ne!(
        applicability_verdict(&variant.applicability),
        ApplicabilityVerdict::Blocked
    );
}

#[test]
fn a_weak_match_is_kept_but_requires_review() {
    let root = tempdir().unwrap();
    write(
        root.path(),
        "example.cht",
        &cht("Infinite Lives", "8000AA00"),
    );
    let p = preview(&root, weak());
    let found = retroarch_launch_candidates(&p, &key(&p));
    assert_eq!(found.candidates.len(), 1, "{:?}", found.excluded);
    assert_eq!(
        applicability_verdict(&found.candidates[0].variants[0].applicability),
        ApplicabilityVerdict::ReviewRequired
    );
}

#[test]
fn two_implementations_of_the_same_cheat_force_an_explicit_choice() {
    let root = tempdir().unwrap();
    write(root.path(), "a.cht", &cht("Infinite Lives", "8000AA00"));
    write(root.path(), "b.cht", &cht("Infinite Lives", "8000BB00"));
    let p = preview(&root, exact());
    let found = retroarch_launch_candidates(&p, &key(&p));
    let candidate = found
        .candidates
        .iter()
        .find(|c| c.variants.len() > 1 || c.unresolved_conflict)
        .unwrap_or_else(|| panic!("no multi-implementation candidate: {found:?}"));
    // The planner requires a variant choice whenever there are several.
    assert!(candidate.variants.len() > 1 || candidate.unresolved_conflict);
    let ids: BTreeSet<_> = candidate
        .variants
        .iter()
        .map(|v| v.variant_id.clone())
        .collect();
    assert_eq!(
        ids.len(),
        candidate.variants.len(),
        "variant ids must be distinct"
    );
}

#[test]
fn malformed_entries_never_become_candidates() {
    let root = tempdir().unwrap();
    write(
        root.path(),
        "broken.cht",
        "cheats = 1\ncheat0_desc = \"No Code\"\ncheat0_enable = false\n",
    );
    let p = preview(&root, exact());
    let found = retroarch_launch_candidates(&p, &key(&p));
    assert!(found.candidates.is_empty(), "{:?}", found.candidates);
}

#[test]
fn other_emulators_cheat_formats_are_not_launched() {
    let root = tempdir().unwrap();
    write(
        root.path(),
        "example.pnach",
        "gametitle=Example\npatch=1,EE,00100000,word,00000001\n",
    );
    let p = preview(&root, exact());
    let found = retroarch_launch_candidates(&p, &key(&p));
    assert!(found.candidates.is_empty());
    assert!(
        found
            .excluded
            .iter()
            .all(|e| e.reason == CandidateExclusion::NotARetroArchCht)
    );
}

#[test]
fn a_hard_refusal_or_missing_fingerprint_excludes_with_a_reason() {
    let root = tempdir().unwrap();
    write(
        root.path(),
        "example.cht",
        &cht("Infinite Lives", "8000AA00"),
    );
    let mut p = preview(&root, exact());
    let game_key = key(&p);

    let mut blocked = p.clone();
    blocked.observations[0].assessment.as_mut().unwrap().state =
        CheatApplicabilityState::DifferentGame;
    let found = retroarch_launch_candidates(&blocked, &game_key);
    assert!(found.candidates.is_empty());
    assert_eq!(
        found.excluded[0].reason,
        CandidateExclusion::NotApplicable(CheatApplicabilityState::DifferentGame)
    );
    assert!(
        found.excluded[0]
            .reason
            .explanation()
            .contains("not applicable")
    );

    p.files[0].provenance = None;
    let found = retroarch_launch_candidates(&p, &game_key);
    assert!(found.candidates.is_empty());
    assert_eq!(
        found.excluded[0].reason,
        CandidateExclusion::NoSourceFingerprint
    );
}

#[test]
fn another_games_cheats_are_not_offered() {
    let root = tempdir().unwrap();
    write(
        root.path(),
        "example.cht",
        &cht("Infinite Lives", "8000AA00"),
    );
    let p = preview(&root, exact());
    let found = retroarch_launch_candidates(&p, "some-other-game");
    assert!(found.candidates.is_empty() && found.excluded.is_empty());
}

#[test]
fn source_paths_are_absolute_and_a_path_that_escapes_the_root_is_refused() {
    let root = tempdir().unwrap();
    write(
        root.path(),
        "example.cht",
        &cht("Infinite Lives", "8000AA00"),
    );
    let p = preview(&root, exact());
    let game_key = key(&p);
    let found = retroarch_launch_candidates(&p, &game_key);
    let path = found.candidates[0].variants[0]
        .source
        .source_path
        .clone()
        .unwrap();
    assert!(path.is_absolute());
    assert!(path.starts_with(root.path()));
    let mut escaping = p.clone();
    escaping.files[0].provenance.as_mut().unwrap().original_path =
        PathBuf::from("/somewhere/else/outside.cht");
    let found = retroarch_launch_candidates(&escaping, &game_key);
    assert!(found.candidates.is_empty());
    assert_eq!(found.excluded[0].reason, CandidateExclusion::Malformed);
    escaping.files[0].provenance.as_mut().unwrap().original_path =
        root.path().join("../outside.cht");
    assert!(
        retroarch_launch_candidates(&escaping, &game_key)
            .candidates
            .is_empty()
    );
}

#[test]
fn a_single_chosen_file_is_bound_by_its_own_real_path() {
    let root = tempdir().unwrap();
    write(root.path(), "pick.cht", &cht("Infinite Lives", "8000AA00"));
    let file = root.path().join("pick.cht");
    let associations: BTreeMap<PathBuf, CheatPackAssociation> =
        [(PathBuf::from("pick.cht"), weak())].into();
    let p = preview_cheat_pack(
        &file,
        &[game()],
        &associations,
        &BTreeSet::new(),
        &CheatPackLimits::default(),
    )
    .unwrap();
    let found = retroarch_launch_candidates(&p, &key(&p));
    assert_eq!(found.candidates.len(), 1, "{:?}", found.excluded);
    let source = &found.candidates[0].variants[0].source;
    assert_eq!(source.source_path.as_deref(), Some(file.as_path()));
    assert_eq!(source.source_sha256.as_deref().map(str::len), Some(64));
}
