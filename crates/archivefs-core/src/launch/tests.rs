//! Fail-closed and projection-correctness tests for Unified Launch
//! Planning, Phase 1.
//!
//! Every input here is hand-constructed; nothing touches the filesystem,
//! network, or a process - matching [`build_launch_plan`]'s own purity
//! contract.

use std::path::PathBuf;

use super::planning::*;
use super::platform_map::retroarch_platform_candidate;
use super::readiness::*;
use crate::emulator_environment::retroarch::{
    ConfigFileFinding, ConfigReadOutcome, CoreFinding, CoreInfoFinding, DirectoryProbeFinding,
    Evidence, ProfileKind, ProfileScope, RetroArchEnvironmentReport, RetroArchPlaylistInventory,
    RetroArchProfile,
};
use crate::emulator_environment::{EncodedPath, FsProbe};

fn resolved_content() -> LaunchContentRef {
    LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::Chd),
        resolved_path: Some(PathBuf::from("/library/psx/Game.chd")),
        requires_mount: false,
        provenance: "already-resolved library path".to_string(),
    }
}

fn unresolved_mount_content() -> LaunchContentRef {
    LaunchContentRef {
        kind: Some(LaunchContentKind::OpticalDisc),
        container: Some(LaunchContainerKind::Archive),
        resolved_path: None,
        requires_mount: true,
        provenance: "content is inside an archive that has not been mounted".to_string(),
    }
}

fn resolved(platform_id: &str) -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: platform_id.to_string(),
        game_key: "TEST-00001".to_string(),
    })
}

fn empty_retroarch_environment() -> RetroArchEnvironmentReport {
    RetroArchEnvironmentReport {
        format_version: 1,
        profiles: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn core_finding(stem: &str, system_name: Option<&str>, database: Option<&str>) -> CoreFinding {
    CoreFinding {
        file_name: EncodedPath::from_path(&PathBuf::from(format!("{stem}_libretro.so"))),
        full_path: EncodedPath::from_path(&PathBuf::from(format!(
            "/home/user/.config/retroarch/cores/{stem}_libretro.so"
        ))),
        core_stem: stem.to_string(),
        info: CoreInfoFinding::Found {
            display_name: None,
            display_version: None,
            system_name: system_name.map(str::to_string),
            supported_extensions: Vec::new(),
            core_name: Some(stem.to_string()),
            manufacturer: None,
            categories: None,
            database: database.map(str::to_string),
            firmware: Vec::new(),
        },
    }
}

fn retroarch_environment_with_cores(cores: Vec<CoreFinding>) -> RetroArchEnvironmentReport {
    let config_dir = EncodedPath::from_path(&PathBuf::from("/home/user/.config/retroarch"));
    RetroArchEnvironmentReport {
        format_version: 1,
        profiles: vec![RetroArchProfile {
            profile_kind: ProfileKind::Native,
            scope: ProfileScope::User,
            evidence: Evidence {
                executables: Vec::new(),
                flatpak_metadata_found: false,
                config_directory_found: true,
                config_file_found: false,
            },
            config_directory: DirectoryProbeFinding {
                path: config_dir.clone(),
                probe: FsProbe::PresentDirectory,
            },
            config_file: ConfigFileFinding {
                path: EncodedPath::from_path(&PathBuf::from(
                    "/home/user/.config/retroarch/retroarch.cfg",
                )),
                probe: FsProbe::Missing,
                read: ConfigReadOutcome::NotAttempted,
            },
            paths: Vec::new(),
            cores,
            playlists: RetroArchPlaylistInventory {
                directory: None,
                playlists: Vec::new(),
                diagnostics: Vec::new(),
                complete: true,
            },
            app_images: Vec::new(),
            diagnostics: Vec::new(),
        }],
        diagnostics: Vec::new(),
    }
}

fn eligible_standalone(
    adapter_id: &'static str,
    firmware: FirmwareReadiness,
) -> StandaloneProfileInput {
    StandaloneProfileInput {
        adapter_id,
        profile_id: format!("{adapter_id}-native"),
        profile_path: Some(PathBuf::from(format!("/home/user/.config/{adapter_id}"))),
        eligible: true,
        firmware,
    }
}

// ---------------------------------------------------------------------
// Identity fail-closed
// ---------------------------------------------------------------------

#[test]
fn unknown_identity_produces_no_runnable_candidate() {
    let plan = build_launch_plan(
        &CanonicalIdentityStatus::Unknown,
        &resolved_content(),
        &[eligible_standalone(
            "duckstation",
            FirmwareReadiness::NotRequired,
        )],
        &empty_retroarch_environment(),
        &[],
    );
    assert!(plan.platform_id.is_none());
    assert!(plan.candidates.is_empty());
    assert_eq!(plan.summary.ready, 0);
    assert_eq!(plan.summary.ready_with_warnings, 0);
}

#[test]
fn conflicting_identity_produces_no_runnable_candidate() {
    let plan = build_launch_plan(
        &CanonicalIdentityStatus::Conflicting,
        &resolved_content(),
        &[eligible_standalone(
            "duckstation",
            FirmwareReadiness::NotRequired,
        )],
        &empty_retroarch_environment(),
        &[],
    );
    assert!(plan.platform_id.is_none());
    assert!(plan.candidates.is_empty());
    assert_eq!(plan.summary.ready, 0);
    assert_eq!(plan.summary.ready_with_warnings, 0);
}

#[test]
fn identity_blocker_kinds_carry_the_right_reason() {
    assert_eq!(
        LaunchBlocker::new(LaunchBlockerKind::IdentityUnresolved, "x").kind,
        LaunchBlockerKind::IdentityUnresolved
    );
    assert_eq!(
        LaunchBlocker::new(LaunchBlockerKind::IdentityConflict, "x").kind,
        LaunchBlockerKind::IdentityConflict
    );
}

// ---------------------------------------------------------------------
// Filename/extension-only evidence never authorizes a target - proven at
// the platform_map level (candidate generation reads only systemname/
// database) and confirmed end-to-end for every named shared-extension
// family.
// ---------------------------------------------------------------------

#[test]
fn psx_ps2_saturn_dreamcast_shared_extensions_do_not_decide_platform() {
    // A core whose .info never resolves must never become a candidate for
    // any of these platforms, no matter what extension the content has.
    let core = core_finding("some_core", Some("Totally Unbranded Core"), None);
    assert_eq!(retroarch_platform_candidate(&core.info), None);
    for platform_id in ["PSX", "PS2", "Saturn", "Dreamcast"] {
        let plan = build_launch_plan(
            &resolved(platform_id),
            &resolved_content(),
            &[],
            &retroarch_environment_with_cores(vec![core_finding(
                "some_core",
                Some("Totally Unbranded Core"),
                None,
            )]),
            &[],
        );
        assert!(
            plan.candidates
                .iter()
                .all(|candidate| !matches!(candidate.target, LaunchTarget::RetroArchCore { .. })),
            "{platform_id} must not get a RetroArch candidate from unresolved .info metadata"
        );
    }
}

#[test]
fn gamecube_and_wii_shared_formats_do_not_decide_platform() {
    // A GameCube-resolving core must never appear as a Wii candidate, and
    // vice versa, even though both platforms share .iso/.rvz/.gcz content.
    let gamecube_plan = build_launch_plan(
        &resolved("GameCube"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core_finding("dolphin", Some("Wii"), None)]),
        &[],
    );
    assert!(
        gamecube_plan
            .candidates
            .iter()
            .all(|c| !matches!(c.target, LaunchTarget::RetroArchCore { .. })),
        "a core resolving to Wii must never become a GameCube candidate"
    );
}

#[test]
fn xbox_and_xbox360_shared_formats_do_not_decide_platform() {
    let xbox_plan = build_launch_plan(
        &resolved("Xbox"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core_finding("xenia", Some("Xbox360"), None)]),
        &[],
    );
    assert!(
        xbox_plan
            .candidates
            .iter()
            .all(|c| !matches!(c.target, LaunchTarget::RetroArchCore { .. })),
        "a core resolving to Xbox360 must never become an Xbox candidate"
    );
}

// ---------------------------------------------------------------------
// Firmware readiness -> blocking behaviour
// ---------------------------------------------------------------------

#[test]
fn missing_required_firmware_blocks() {
    let plan = build_launch_plan(
        &resolved("PS2"),
        &resolved_content(),
        &[eligible_standalone("pcsx2", FirmwareReadiness::Missing)],
        &empty_retroarch_environment(),
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Blocked);
    assert!(
        plan.candidates[0]
            .blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::RequiredFirmwareMissing)
    );
    assert_eq!(plan.summary.blocked, 1);
}

#[test]
fn present_unverified_firmware_is_ready_with_warnings() {
    let plan = build_launch_plan(
        &resolved("PS2"),
        &resolved_content(),
        &[eligible_standalone(
            "pcsx2",
            FirmwareReadiness::PresentUnverified,
        )],
        &empty_retroarch_environment(),
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(
        plan.candidates[0].readiness,
        LaunchReadiness::ReadyWithWarnings
    );
    assert!(
        plan.candidates[0]
            .warnings
            .iter()
            .any(|w| w.kind == LaunchWarningKind::FirmwarePresentUnverified)
    );
    assert_eq!(plan.summary.ready_with_warnings, 1);
}

#[test]
fn ppsspp_never_requires_firmware_in_a_plan() {
    let plan = build_launch_plan(
        &resolved("PSP"),
        &resolved_content(),
        &[eligible_standalone("ppsspp", ppsspp_firmware_readiness())],
        &empty_retroarch_environment(),
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(plan.candidates[0].firmware, FirmwareReadiness::NotRequired);
    assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
}

#[test]
fn hatari_verified_tos_is_ready() {
    use crate::patch_manager::HatariTosHealth;
    let plan = build_launch_plan(
        &resolved("AtariST"),
        &resolved_content(),
        &[eligible_standalone(
            "hatari",
            hatari_firmware_readiness(HatariTosHealth::Verified),
        )],
        &empty_retroarch_environment(),
        &[],
    );
    assert_eq!(plan.candidates[0].firmware, FirmwareReadiness::Verified);
    assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
}

// ---------------------------------------------------------------------
// Installation / content resolution
// ---------------------------------------------------------------------

#[test]
fn absent_installation_is_a_blocker() {
    let plan = build_launch_plan(
        &resolved("PS3"),
        &resolved_content(),
        &[], // nothing discovered
        &empty_retroarch_environment(),
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Blocked);
    assert!(
        plan.candidates[0]
            .blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::NoInstallationCandidate)
    );
}

#[test]
fn unresolved_content_blocks_and_carries_requires_mount() {
    let plan = build_launch_plan(
        &resolved("PS2"),
        &unresolved_mount_content(),
        &[eligible_standalone("pcsx2", FirmwareReadiness::NotRequired)],
        &empty_retroarch_environment(),
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    let candidate = &plan.candidates[0];
    assert_eq!(candidate.readiness, LaunchReadiness::Blocked);
    assert!(
        candidate
            .blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::ContentNotResolved)
    );
    assert!(candidate.content.requires_mount);
    assert!(!candidate.content.has_runnable_path());
}

// ---------------------------------------------------------------------
// RetroArch candidate generation
// ---------------------------------------------------------------------

#[test]
fn alias_resolvable_systemname_becomes_a_candidate() {
    let plan = build_launch_plan(
        &resolved("SNES"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core_finding(
            "snes9x",
            Some("Nintendo - SNES / SFC"),
            None,
        )]),
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    assert!(matches!(
        plan.candidates[0].target,
        LaunchTarget::RetroArchCore { .. }
    ));
    assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
}

#[test]
fn reviewed_classic_core_hints_prefer_one_of_two_matching_cores() {
    for (platform_id, preferred, alternative, system_name, database) in [
        (
            "NES",
            "nestopia",
            "fceumm",
            Some("Nintendo - Nintendo Entertainment System"),
            None,
        ),
        (
            "SNES",
            "snes9x",
            "bsnes",
            Some("Nintendo - SNES / SFC"),
            None,
        ),
        (
            "Game Boy Color",
            "gambatte",
            "sameboy",
            Some("Game Boy/Game Boy Color"),
            Some("Nintendo - Game Boy|Nintendo - Game Boy Color"),
        ),
        (
            "Game Boy Advance",
            "mgba",
            "vba_next",
            Some("Game Boy/Game Boy Color/Game Boy Advance"),
            Some("Nintendo - Game Boy|Nintendo - Game Boy Color|Nintendo - Game Boy Advance"),
        ),
        (
            "MegaDrive",
            "genesis_plus_gx",
            "picodrive",
            Some("Sega 8/16-bit (Various)"),
            Some("Sega - Game Gear|Sega - Master System - Mark III|Sega - Mega Drive - Genesis"),
        ),
        (
            "N64",
            "mupen64plus_next",
            "parallel_n64",
            Some("Nintendo - Nintendo 64"),
            None,
        ),
        (
            "Atari2600",
            "stella",
            "virtualjaguar",
            Some("Atari - 2600"),
            None,
        ),
        (
            "Atari5200",
            "a5200",
            "stella",
            Some("Atari - 5200"),
            None,
        ),
        (
            "Atari Lynx",
            "handy",
            "mednafen_lynx",
            Some("Atari - Lynx"),
            None,
        ),
    ] {
        let plan = build_launch_plan(
            &resolved(platform_id),
            &resolved_content(),
            &[],
            &retroarch_environment_with_cores(vec![
                core_finding(preferred, system_name, database),
                core_finding(alternative, system_name, database),
            ]),
            &[],
        );
        let preferred_candidate = plan
            .candidates
            .iter()
            .find(|candidate| {
                matches!(&candidate.target, LaunchTarget::RetroArchCore { core_stem, .. } if core_stem == preferred)
            })
            .expect("preferred core should be a candidate");
        assert_eq!(
            preferred_candidate.preference,
            CandidatePreference::SoleEligible,
            "{platform_id}"
        );
        assert_eq!(preferred_candidate.readiness, LaunchReadiness::Ready);
        let alternative_candidate = plan
            .candidates
            .iter()
            .find(|candidate| {
                matches!(&candidate.target, LaunchTarget::RetroArchCore { core_stem, .. } if core_stem == alternative)
            })
            .expect("alternative core should remain visible");
        assert_eq!(alternative_candidate.readiness, LaunchReadiness::Ready);
        assert_eq!(plan.summary.ready, 2);
    }
}

#[test]
fn two_matching_cores_without_a_reviewed_hint_remain_ambiguous() {
    let plan = build_launch_plan(
        &resolved("NES"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![
            core_finding("fceumm", Some("Nintendo - Nintendo Entertainment System"), None),
            core_finding("mesen", Some("Nintendo - Nintendo Entertainment System"), None),
        ]),
        &[],
    );
    let retroarch_candidates: Vec<_> = plan
        .candidates
        .iter()
        .filter(|candidate| matches!(candidate.target, LaunchTarget::RetroArchCore { .. }))
        .collect();
    assert_eq!(retroarch_candidates.len(), 2);
    assert!(retroarch_candidates.iter().all(|candidate| candidate
        .blockers
        .iter()
        .any(|blocker| blocker.kind == LaunchBlockerKind::AmbiguousCore)));
}

#[test]
fn reviewed_core_hint_never_manufactures_identity_or_cross_selects_platform() {
    let unknown = build_launch_plan(
        &CanonicalIdentityStatus::Unknown,
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core_finding(
            "nestopia",
            Some("Nintendo - Nintendo Entertainment System"),
            None,
        )]),
        &[],
    );
    assert!(unknown.candidates.is_empty());

    let wrong_platform = build_launch_plan(
        &resolved("SNES"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core_finding(
            "nestopia",
            Some("Nintendo - Nintendo Entertainment System"),
            None,
        )]),
        &[],
    );
    assert!(wrong_platform.candidates.iter().all(|candidate| !matches!(
        candidate.target,
        LaunchTarget::RetroArchCore { .. }
    )));
}

#[test]
fn genesis_plus_gx_can_plan_a_sega_cd_candidate_from_reviewed_metadata() {
    let plan = build_launch_plan(
        &resolved("Sega CD"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core_finding(
            "genesis_plus_gx",
            Some("Sega - MS/GG/MD/CD"),
            Some(
                "Sega - Game Gear|Sega - Master System - Mark III|Sega - Mega-CD - Sega CD|Sega - Mega Drive - Genesis",
            ),
        )]),
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    assert!(matches!(
        plan.candidates[0].target,
        LaunchTarget::RetroArchCore { .. }
    ));
    assert_eq!(plan.candidates[0].readiness, LaunchReadiness::Ready);
}

#[test]
fn alias_resolvable_database_becomes_a_candidate_when_systemname_absent() {
    let plan = build_launch_plan(
        &resolved("NES"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core_finding(
            "nestopia",
            None,
            Some("Nintendo - Nintendo Entertainment System"),
        )]),
        &[],
    );
    assert_eq!(plan.candidates.len(), 1);
    assert!(matches!(
        plan.candidates[0].target,
        LaunchTarget::RetroArchCore { .. }
    ));
}

#[test]
fn non_resolving_info_metadata_stays_unknown_no_candidate() {
    let plan = build_launch_plan(
        &resolved("NES"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core_finding(
            "mystery_core",
            Some("Not A Real Platform Name"),
            None,
        )]),
        &[],
    );
    assert!(
        plan.candidates
            .iter()
            .all(|c| !matches!(c.target, LaunchTarget::RetroArchCore { .. }))
    );
}

#[test]
fn corename_alone_never_creates_a_retroarch_candidate() {
    // core_finding never sets corename as authority; confirm end-to-end
    // that a core with a real-sounding corename but no resolvable
    // systemname/database produces zero candidates.
    let core = CoreFinding {
        file_name: EncodedPath::from_path(&PathBuf::from("nestopia_libretro.so")),
        full_path: EncodedPath::from_path(&PathBuf::from(
            "/home/user/.config/retroarch/cores/nestopia_libretro.so",
        )),
        core_stem: "nestopia".to_string(),
        info: CoreInfoFinding::Found {
            display_name: None,
            display_version: None,
            system_name: None,
            supported_extensions: Vec::new(),
            core_name: Some("Nestopia".to_string()),
            manufacturer: Some("Nintendo".to_string()),
            categories: None,
            database: None,
            firmware: Vec::new(),
        },
    };
    let plan = build_launch_plan(
        &resolved("NES"),
        &resolved_content(),
        &[],
        &retroarch_environment_with_cores(vec![core]),
        &[],
    );
    assert!(
        plan.candidates
            .iter()
            .all(|c| !matches!(c.target, LaunchTarget::RetroArchCore { .. }))
    );
}

// ---------------------------------------------------------------------
// Identity boundary: platform mapping never rewrites canonical identity
// ---------------------------------------------------------------------

#[test]
fn platform_mapping_does_not_alter_canonical_identity() {
    let identity = resolved("PSX");
    let plan = build_launch_plan(
        &identity,
        &resolved_content(),
        &[eligible_standalone(
            "duckstation",
            FirmwareReadiness::NotRequired,
        )],
        &empty_retroarch_environment(),
        &[],
    );
    // The identity value passed in is untouched by the call (moved by
    // reference only), and the plan's own platform/game key are exactly
    // what was supplied - never widened, narrowed, or replaced by a
    // RetroArch/adapter-derived guess.
    assert_eq!(identity, resolved("PSX"));
    assert_eq!(plan.platform_id.as_deref(), Some("PSX"));
    assert_eq!(plan.game_key.as_deref(), Some("TEST-00001"));
}
