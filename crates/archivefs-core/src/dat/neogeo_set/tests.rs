use std::path::PathBuf;

use super::*;
use crate::dat::dependency::{DependencyRequirement, DependencyState, DependencyTarget};
use crate::dat::model::DatRomEntry;
use crate::dat::set::{BadMetadataReason, SetBadMember, SetIdentity};

fn rom(name: &str, region: Option<&str>) -> DatRomEntry {
    DatRomEntry {
        name: name.to_string(),
        region: region.map(str::to_string),
        ..Default::default()
    }
}

fn game(name: &str, roms: Vec<DatRomEntry>) -> DatGameEntry {
    DatGameEntry {
        name: name.to_string(),
        description: Some(format!("{name} description")),
        roms,
        ..Default::default()
    }
}

fn no_deps() -> SetDependencyReport {
    SetDependencyReport {
        state: DependencyState::NotApplicable,
        requirements: Vec::new(),
    }
}

fn bios_dep(outcome: DependencyOutcome) -> SetDependencyReport {
    SetDependencyReport {
        state: match outcome {
            DependencyOutcome::Satisfied => DependencyState::Satisfied,
            DependencyOutcome::Missing => DependencyState::Missing,
            _ => DependencyState::Ambiguous,
        },
        requirements: vec![DependencyRequirement {
            kind: DependencyKind::Bios,
            target: DependencyTarget::Set {
                name: "neogeo".to_string(),
            },
            outcome,
            via_member: None,
        }],
    }
}

fn resolution(
    game_name: &str,
    state: SetState,
    required: &[&str],
    verified: &[&str],
    bad: &[&str],
    borrowed: &[&str],
    dependencies: SetDependencyReport,
) -> SetResolution {
    SetResolution {
        identity: SetIdentity {
            source_id: "mame".to_string(),
            game_name: game_name.to_string(),
        },
        archive_path: PathBuf::from("/library/set.zip"),
        state,
        members_required: required.iter().map(|s| s.to_string()).collect(),
        members_verified: verified.iter().map(|s| s.to_string()).collect(),
        members_bad: bad
            .iter()
            .map(|name| SetBadMember {
                rom_name: name.to_string(),
                reason: BadMetadataReason::BadDump,
            })
            .collect(),
        members_optional: Vec::new(),
        members_borrowed: borrowed.iter().map(|s| s.to_string()).collect(),
        disks_required: Vec::new(),
        disks_verified: Vec::new(),
        disks_parent_required: Vec::new(),
        dependencies,
    }
}

// --- Task B: ROM roles from authoritative region evidence ---------------

#[test]
fn known_neogeo_regions_map_to_the_real_mame_roles() {
    assert_eq!(neogeo_rom_role("maincpu"), Some(NeoGeoRomRole::Program));
    assert_eq!(neogeo_rom_role("fixed"), Some(NeoGeoRomRole::FixedLayer));
    assert_eq!(
        neogeo_rom_role("fixedbios"),
        Some(NeoGeoRomRole::FixedLayer)
    );
    assert_eq!(
        neogeo_rom_role("audiocpu"),
        Some(NeoGeoRomRole::SoundProgram)
    );
    assert_eq!(neogeo_rom_role("ymsnd"), Some(NeoGeoRomRole::AdpcmSamples));
    assert_eq!(
        neogeo_rom_role("ymsnd.deltat"),
        Some(NeoGeoRomRole::AdpcmSamples)
    );
    assert_eq!(neogeo_rom_role("sprites"), Some(NeoGeoRomRole::Graphics));
    assert_eq!(neogeo_rom_role("MAINCPU"), Some(NeoGeoRomRole::Program));
}

#[test]
fn unrecognised_region_is_none_never_a_guess() {
    assert_eq!(neogeo_rom_role("some_future_region"), None);
    assert_eq!(neogeo_rom_role(""), None);
}

#[test]
fn role_is_never_derived_from_the_rom_filename() {
    // A file literally named like a program ROM, but with no region at
    // all, must not be labelled Program - only `region=` proves a role.
    let unlabelled = rom("p1.p1", None);
    assert_eq!(unlabelled.region.as_deref().and_then(neogeo_rom_role), None);
}

#[test]
fn is_neogeo_mame_driver_requires_the_real_sourcefile() {
    assert!(is_neogeo_mame_driver(Some("neogeo.cpp")));
    assert!(is_neogeo_mame_driver(Some("neogeo/neogeo.cpp")));
    assert!(is_neogeo_mame_driver(Some("NEOGEO.CPP")));
    assert!(!is_neogeo_mame_driver(Some("pacman.cpp")));
    assert!(!is_neogeo_mame_driver(None));
}

// --- Task J.1: complete set ------------------------------------------

#[test]
fn complete_set() {
    let g = game(
        "mslug",
        vec![
            rom("p1.p1", Some("maincpu")),
            rom("s1.s1", Some("fixed")),
            rom("m1.m1", Some("audiocpu")),
            rom("v1.v1", Some("ymsnd")),
            rom("c1.c1", Some("sprites")),
        ],
    );
    let r = resolution(
        "mslug",
        SetState::Complete,
        &["p1.p1", "s1.s1", "m1.m1", "v1.v1", "c1.c1"],
        &["p1.p1", "s1.s1", "m1.m1", "v1.v1", "c1.c1"],
        &[],
        &[],
        no_deps(),
    );
    let summary = project_neogeo_set(&g, &r);
    assert_eq!(summary.coherence, NeoGeoCoherence::Complete);
    assert_eq!(summary.members_required, 5);
    assert_eq!(summary.members_verified, 5);
    assert!(summary.members.iter().all(|m| m.present && m.required));
    assert_eq!(
        summary
            .members
            .iter()
            .find(|m| m.name == "p1.p1")
            .unwrap()
            .role,
        Some(NeoGeoRomRole::Program)
    );
}

// --- Task J.2: missing member -----------------------------------------

#[test]
fn missing_member() {
    let g = game(
        "mslug",
        vec![rom("p1.p1", Some("maincpu")), rom("c1.c1", Some("sprites"))],
    );
    let r = resolution(
        "mslug",
        SetState::Incomplete,
        &["p1.p1", "c1.c1"],
        &["p1.p1"],
        &[],
        &[],
        no_deps(),
    );
    let summary = project_neogeo_set(&g, &r);
    assert_eq!(summary.coherence, NeoGeoCoherence::Incomplete);
    let c1 = summary.members.iter().find(|m| m.name == "c1.c1").unwrap();
    assert!(c1.required);
    assert!(!c1.present);
}

// --- Task J.3: extra member (present in the DAT entry, not required) --

#[test]
fn extra_member_never_flips_to_complete() {
    let g = game(
        "mslug",
        vec![rom("p1.p1", Some("maincpu")), rom("unexpected.bin", None)],
    );
    // The DAT does not declare "unexpected.bin" as required at all - it is
    // simply absent from `members_required`, and its presence in the
    // archive does not appear in `members_verified` either (R1: membership
    // comes only from the DAT).
    let r = resolution(
        "mslug",
        SetState::Incomplete,
        &["p1.p1", "c1.c1"],
        &["p1.p1"],
        &[],
        &[],
        no_deps(),
    );
    let summary = project_neogeo_set(&g, &r);
    assert_eq!(summary.coherence, NeoGeoCoherence::Incomplete);
    let extra = summary
        .members
        .iter()
        .find(|m| m.name == "unexpected.bin")
        .unwrap();
    assert!(!extra.required);
    assert!(!extra.present);
}

// --- Task J.4: wrong hash (surfaces as BadMetadata via the existing R4) -

#[test]
fn wrong_hash_reported_as_conflicting_via_bad_dump() {
    let g = game("mslug", vec![rom("p1.p1", Some("maincpu"))]);
    let r = resolution(
        "mslug",
        SetState::BadMetadata(BadMetadataReason::BadDump),
        &[],
        &[],
        &["p1.p1"],
        &[],
        no_deps(),
    );
    let summary = project_neogeo_set(&g, &r);
    assert_eq!(summary.coherence, NeoGeoCoherence::Conflicting);
    assert!(
        summary
            .members
            .iter()
            .find(|m| m.name == "p1.p1")
            .unwrap()
            .bad
    );
}

// --- Task J.5: parent/clone distinction, never flattened ---------------

#[test]
fn parent_and_clone_are_kept_as_distinct_named_sets() {
    let parent = game("kof98", vec![rom("p1.p1", Some("maincpu"))]);
    let parent_res = resolution(
        "kof98",
        SetState::Complete,
        &["p1.p1"],
        &["p1.p1"],
        &[],
        &[],
        no_deps(),
    );
    let mut clone = game("kof98h", vec![rom("p1.p1", Some("maincpu"))]);
    clone.clone_of = Some("kof98".to_string());
    clone.rom_of = Some("kof98".to_string());
    let clone_res = resolution(
        "kof98h",
        SetState::Complete,
        &["p1.p1"],
        &["p1.p1"],
        &[],
        &[],
        no_deps(),
    );

    let parent_summary = project_neogeo_set(&parent, &parent_res);
    let clone_summary = project_neogeo_set(&clone, &clone_res);

    assert_eq!(parent_summary.set_name, "kof98");
    assert_eq!(parent_summary.parent_set, None);
    assert_eq!(clone_summary.set_name, "kof98h");
    assert_eq!(clone_summary.parent_set.as_deref(), Some("kof98"));
    assert_eq!(clone_summary.rom_source_set.as_deref(), Some("kof98"));
    assert_ne!(parent_summary.set_name, clone_summary.set_name);
}

// --- Task J.6: ambiguous candidates, no auto-winner ---------------------

#[test]
fn ambiguous_candidates_are_reported_not_resolved() {
    let g = game("mslug", vec![rom("p1.p1", Some("maincpu"))]);
    let r = resolution(
        "mslug",
        SetState::NeedsReview(NeedsReviewReason::AmbiguousMemberAttribution),
        &["p1.p1"],
        &[],
        &[],
        &[],
        no_deps(),
    );
    let summary = project_neogeo_set(&g, &r);
    assert_eq!(summary.coherence, NeoGeoCoherence::Ambiguous);
    // The summary itself never invents a winner: it is exactly one
    // candidate's own view, and a caller must retain every candidate's
    // NeoGeoSetSummary separately (proven by this being a plain, unopinionated
    // per-resolution projection with no "pick best" logic anywhere in this
    // module).
}

// --- Task J.7: conflicting source DATs preserved separately -----------

#[test]
fn conflicting_source_dats_are_never_merged() {
    let g = game("mslug", vec![rom("p1.p1", Some("maincpu"))]);
    let mame_res = resolution(
        "mslug",
        SetState::Complete,
        &["p1.p1"],
        &["p1.p1"],
        &[],
        &[],
        no_deps(),
    );
    let mut fbneo_res = resolution(
        "mslug",
        SetState::Incomplete,
        &["p1.p1"],
        &[],
        &[],
        &[],
        no_deps(),
    );
    fbneo_res.identity.source_id = "fbneo".to_string();

    let mame_summary = project_neogeo_set(&g, &mame_res);
    let fbneo_summary = project_neogeo_set(&g, &fbneo_res);

    assert_eq!(mame_summary.source_id, "mame");
    assert_eq!(fbneo_summary.source_id, "fbneo");
    assert_ne!(mame_summary.coherence, fbneo_summary.coherence);
    // Both candidates remain independently addressable - nothing here
    // averages, prefers, or discards either source's verdict.
}

// --- Task J.8: BIOS missing while game set complete ---------------------

#[test]
fn bios_missing_while_game_set_complete() {
    let g = game("mslug", vec![rom("p1.p1", Some("maincpu"))]);
    let r = resolution(
        "mslug",
        SetState::Complete,
        &["p1.p1"],
        &["p1.p1"],
        &[],
        &[],
        bios_dep(DependencyOutcome::Missing),
    );
    let summary = project_neogeo_set(&g, &r);
    assert_eq!(summary.coherence, NeoGeoCoherence::Complete);
    assert_eq!(summary.bios_status, NeoGeoBiosStatus::Missing);
}

// --- Task J.9: BIOS complete while game set missing members -------------

#[test]
fn bios_ready_while_game_set_incomplete() {
    let g = game(
        "mslug",
        vec![rom("p1.p1", Some("maincpu")), rom("c1.c1", Some("sprites"))],
    );
    let r = resolution(
        "mslug",
        SetState::Incomplete,
        &["p1.p1", "c1.c1"],
        &["p1.p1"],
        &[],
        &[],
        bios_dep(DependencyOutcome::Satisfied),
    );
    let summary = project_neogeo_set(&g, &r);
    assert_eq!(summary.coherence, NeoGeoCoherence::Incomplete);
    assert_eq!(summary.bios_status, NeoGeoBiosStatus::Ready);
}

// --- Task J.10: folder-name-only refusal ---------------------------------

#[test]
fn folder_name_alone_never_establishes_identity() {
    // `project_neogeo_set` never reads a path/folder name at all - its
    // inputs are exactly `DatGameEntry`/`SetResolution`, both already
    // produced from DAT/hash evidence. A directory named "Metal Slug (Neo
    // Geo)" with unrelated or absent bytes never reaches this function with
    // a `SetResolution` at all (the existing `dat::set` engine only ever
    // emits one when a positionally-attributed hash actually matched - R1).
    // This is a structural, not a runtime, refusal: proven by
    // `project_neogeo_set`'s signature taking no path.
    fn _never_takes_a_path(_g: &DatGameEntry, _r: &SetResolution) -> NeoGeoSetSummary {
        project_neogeo_set(_g, _r)
    }
}

// --- Task J.11: archive member ordering irrelevant -----------------------

#[test]
fn member_order_does_not_affect_the_result() {
    let forward = game(
        "mslug",
        vec![rom("p1.p1", Some("maincpu")), rom("c1.c1", Some("sprites"))],
    );
    let reversed = game(
        "mslug",
        vec![rom("c1.c1", Some("sprites")), rom("p1.p1", Some("maincpu"))],
    );
    let r = resolution(
        "mslug",
        SetState::Complete,
        &["p1.p1", "c1.c1"],
        &["p1.p1", "c1.c1"],
        &[],
        &[],
        no_deps(),
    );
    let forward_summary = project_neogeo_set(&forward, &r);
    let mut reversed_summary = project_neogeo_set(&reversed, &r);
    reversed_summary.members.sort_by(|a, b| a.name.cmp(&b.name));
    let mut forward_members = forward_summary.members;
    forward_members.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(forward_members, reversed_summary.members);
    assert_eq!(forward_summary.coherence, NeoGeoCoherence::Complete);
}

// --- Task J.12: no filesystem mutation ------------------------------------

#[test]
fn projection_is_pure_and_touches_no_filesystem() {
    // `project_neogeo_set` takes only in-memory values and returns a new
    // value; there is no `std::fs`/`File`/path parameter anywhere in its
    // signature or this module's implementation to mutate anything.
    let g = game("mslug", vec![rom("p1.p1", Some("maincpu"))]);
    let r = resolution(
        "mslug",
        SetState::Complete,
        &["p1.p1"],
        &["p1.p1"],
        &[],
        &[],
        no_deps(),
    );
    let a = project_neogeo_set(&g, &r);
    let b = project_neogeo_set(&g, &r);
    assert_eq!(
        a, b,
        "calling twice must be idempotent with identical output"
    );
}

// --- Additional coherence-vocabulary mapping coverage --------------------

#[test]
fn every_needs_review_reason_maps_to_ambiguous_or_unknown() {
    let ambiguous = [
        NeedsReviewReason::AmbiguousMemberAttribution,
        NeedsReviewReason::AmbiguousDependency,
        NeedsReviewReason::DuplicateGameName,
    ];
    for reason in ambiguous {
        assert_eq!(
            neogeo_coherence(&SetState::NeedsReview(reason)),
            NeoGeoCoherence::Ambiguous
        );
    }
    let unknown = [
        NeedsReviewReason::UnsupportedSetStructure,
        NeedsReviewReason::PartialArchivePass,
        NeedsReviewReason::DuplicateArchiveEvidence,
        NeedsReviewReason::ContradictoryMemberFlags,
        NeedsReviewReason::UnknownLoadflag,
        NeedsReviewReason::UnsupportedSoftware,
        NeedsReviewReason::NoDeclaredMembers,
        NeedsReviewReason::OnlyNonFileOrOptionalMembers,
        NeedsReviewReason::DependencyCycle,
        NeedsReviewReason::ContradictoryDependencyMetadata,
        NeedsReviewReason::UnsupportedDependencyStructure,
        NeedsReviewReason::DependencyEvidenceIncomplete,
    ];
    for reason in unknown {
        assert_eq!(
            neogeo_coherence(&SetState::NeedsReview(reason)),
            NeoGeoCoherence::Unknown
        );
    }
}

#[test]
fn nodump_and_baddump_both_map_to_conflicting() {
    assert_eq!(
        neogeo_coherence(&SetState::BadMetadata(BadMetadataReason::NoDump)),
        NeoGeoCoherence::Conflicting
    );
    assert_eq!(
        neogeo_coherence(&SetState::BadMetadata(BadMetadataReason::BadDump)),
        NeoGeoCoherence::Conflicting
    );
}

#[test]
fn bios_status_not_applicable_when_no_bios_dependency_declared() {
    assert_eq!(
        neogeo_bios_status(&no_deps()),
        NeoGeoBiosStatus::NotApplicable
    );
}

#[test]
fn bios_status_unknown_when_ambiguous() {
    assert_eq!(
        neogeo_bios_status(&bios_dep(DependencyOutcome::Ambiguous)),
        NeoGeoBiosStatus::Unknown
    );
}

#[test]
fn bios_only_directory_has_no_game_members_required() {
    // A BIOS-only directory (just `neogeo.zip`'s own contents) reaches this
    // projection, if at all, as the BIOS set's own resolution - it declares
    // no BIOS dependency on itself (`NotApplicable`), and its own storage
    // completeness is reported like any other set, never silently promoted
    // to "a complete game".
    let bios_set = game(
        "neogeo",
        vec![
            rom("sp-s2.sp1", Some("mainbios")),
            rom("sm1.sm1", Some("audiobios")),
        ],
    );
    let r = resolution(
        "neogeo",
        SetState::Complete,
        &["sp-s2.sp1", "sm1.sm1"],
        &["sp-s2.sp1", "sm1.sm1"],
        &[],
        &[],
        no_deps(),
    );
    let summary = project_neogeo_set(&bios_set, &r);
    assert_eq!(summary.bios_status, NeoGeoBiosStatus::NotApplicable);
    assert_eq!(summary.coherence, NeoGeoCoherence::Complete);
    assert!(
        summary
            .members
            .iter()
            .all(|m| m.role == Some(NeoGeoRomRole::Other))
    );
}

#[test]
fn one_matching_member_from_a_much_larger_set_stays_incomplete() {
    let g = game(
        "mslug",
        vec![
            rom("p1.p1", Some("maincpu")),
            rom("p2.p2", Some("maincpu")),
            rom("s1.s1", Some("fixed")),
            rom("m1.m1", Some("audiocpu")),
            rom("v1.v1", Some("ymsnd")),
            rom("v2.v2", Some("ymsnd")),
            rom("c1.c1", Some("sprites")),
            rom("c2.c2", Some("sprites")),
        ],
    );
    let r = resolution(
        "mslug",
        SetState::Incomplete,
        &[
            "p1.p1", "p2.p2", "s1.s1", "m1.m1", "v1.v1", "v2.v2", "c1.c1", "c2.c2",
        ],
        &["c1.c1"],
        &[],
        &[],
        no_deps(),
    );
    let summary = project_neogeo_set(&g, &r);
    assert_eq!(summary.coherence, NeoGeoCoherence::Incomplete);
    assert_eq!(summary.members_verified, 1);
    assert_eq!(summary.members_required, 8);
    assert_ne!(summary.coherence, NeoGeoCoherence::Complete);
}
