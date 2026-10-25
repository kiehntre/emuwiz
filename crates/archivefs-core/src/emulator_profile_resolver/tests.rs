//! Resolver tests. Every fixture lives in a temporary directory.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use crate::emulator_inventory::InventoryEmulator;
use crate::patch_manager::duckstation_local::DuckStationProfileDiscoveryRoots;
use crate::patch_manager::ppsspp_local::PpssppProfileDiscoveryRoots;
use crate::patch_manager::{
    DuckStationDiscTopology, DuckStationNativeCheat, DuckStationNativeOperation,
    DuckStationNativeRequest, plan_duckstation_native,
};

struct Machine {
    dir: TempDir,
    home: PathBuf,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn executable(path: &Path) {
    write(path, "#!/bin/sh\n");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

const COMPLETE: &str = "[Main]\nSetupWizardIncomplete = false\n";

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        Self { dir, home }
    }

    fn app(&self, folder: &str) -> PathBuf {
        self.home.join("Applications").join(folder)
    }

    /// A portable AppImage install: marker, initialised settings, play history,
    /// per-game settings. BIOS search directory points somewhere that exists
    /// unless `bios` says otherwise.
    fn portable(&self, folder: &str, bios: &str) -> PathBuf {
        let root = self.app(folder);
        executable(&root.join("DuckStation.AppImage"));
        write(&root.join("portable.txt"), "");
        write(
            &root.join("settings.ini"),
            &format!(
                "{COMPLETE}\n[BIOS]\nSearchDirectory = {bios}\n\n[Folders]\nCheats = cheats\nGameSettings = gamesettings\n"
            ),
        );
        write(
            &root.join("playtime.dat"),
            "SCUS-94601   341   1783636844\n",
        );
        write(&root.join("gamesettings/SCUS-94601.ini"), "[Cheats]\n");
        fs::create_dir_all(root.join("cheats")).unwrap();
        fs::create_dir_all(root.join("bios")).unwrap();
        write(&root.join("bios/scph1001.bin"), "bios");
        root
    }

    /// An AppImage with no marker beside it, so it uses the per-user default
    /// profile, which is initialised but never set up or used.
    fn native(&self) -> (PathBuf, PathBuf) {
        let exe = self.app("emulators").join("DuckStation.AppImage");
        executable(&exe);
        let root = self.home.join(".local/share/duckstation");
        write(
            &root.join("settings.ini"),
            "[Main]\nSomething = 1\n\n[BIOS]\nSearchDirectory = bios\n",
        );
        fs::create_dir_all(root.join("bios")).unwrap();
        (exe, root)
    }

    fn launcher_for(&self, exe: &Path) {
        write(
            &self
                .home
                .join(".local/share/applications/duckstation.desktop"),
            &format!(
                "[Desktop Entry]\nName=DuckStation\nExec=\"{}\" %f\n",
                exe.display()
            ),
        );
    }

    fn roots(&self) -> DuckStationProfileDiscoveryRoots {
        DuckStationProfileDiscoveryRoots {
            home: self.home.clone(),
            xdg_config_home: self.home.join(".config"),
            xdg_data_home: self.home.join(".local/share"),
            xdg_config_home_explicit: false,
            explicit_configuration_roots: Vec::new(),
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            path_override: Some(Vec::new()),
        }
    }

    fn adapter(&self) -> DuckStationAdapter {
        self.adapter_with(self.roots())
    }

    fn adapter_with(&self, roots: DuckStationProfileDiscoveryRoots) -> DuckStationAdapter {
        DuckStationAdapter::new(roots, vec![self.home.join(".local/share/applications")])
    }
}

/// The real machine's shape: portable A (healthy, dead BIOS path), unused
/// native B, stale Flatpak data, an old override pointing at B.
struct RealStyle {
    machine: Machine,
    a_exe: PathBuf,
    a_root: PathBuf,
    b_exe: PathBuf,
    b_root: PathBuf,
}

fn real_style() -> RealStyle {
    let machine = Machine::new();
    let a_root = machine.portable("DuckStation", "../../../../does/not/exist/psx");
    let a_exe = a_root.join("DuckStation.AppImage");
    let (b_exe, b_root) = machine.native();
    machine.launcher_for(&a_exe);
    fs::create_dir_all(
        machine
            .home
            .join(".var/app/org.duckstation.DuckStation/config/duckstation"),
    )
    .unwrap();
    RealStyle {
        machine,
        a_exe,
        a_root,
        b_exe,
        b_root,
    }
}

fn resolved(outcome: &ResolutionOutcome) -> &Resolution {
    outcome
        .resolved()
        .unwrap_or_else(|| panic!("expected Resolved, got {outcome:?}"))
}

fn standing<'a>(resolution: &'a Resolution, exe: &Path) -> &'a Standing {
    &resolution
        .assessments
        .iter()
        .find(|item| item.identity.executable.as_deref() == Some(exe))
        .unwrap_or_else(|| panic!("no assessment for {}", exe.display()))
        .standing
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u32)> {
    fn walk(path: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, u32)>) {
        for entry in fs::read_dir(path).unwrap().flatten() {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                out.insert(path.clone(), (Vec::new(), metadata.permissions().mode()));
                walk(&path, out);
            } else {
                out.insert(
                    path.clone(),
                    (
                        fs::read(&path).unwrap_or_default(),
                        metadata.permissions().mode(),
                    ),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

// ---------------------------------------------------------------------------
// AUTO
// ---------------------------------------------------------------------------

#[test]
fn one_healthy_auto_candidate_is_resolved() {
    let machine = Machine::new();
    let root = machine.portable("DuckStation", "bios");
    let outcome = resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto);
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.profile_root, root);
    assert_eq!(
        resolution.profile.executable,
        root.join("DuckStation.AppImage")
    );
    assert_eq!(
        resolution.profile.reason,
        SelectionReason::OnlyViableCandidate
    );
    assert_eq!(resolution.profile.mode, SelectionModeKind::Auto);
    assert_eq!(resolution.profile.main_config, root.join("settings.ini"));
    assert_eq!(resolution.profile.layout, ProfileLayout::Portable);
    assert_eq!(resolution.profile.readiness.setup, SetupState::Complete);
    assert!(resolution.profile.warnings.is_empty());
}

#[test]
fn an_initialised_profile_beats_an_uninitialised_one() {
    let fixture = real_style();
    let outcome =
        resolve_emulator_profile(&fixture.machine.adapter(), &EmulatorSelectionMode::Auto);
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.executable, fixture.a_exe);
    assert_eq!(
        resolution.profile.reason,
        SelectionReason::StrongestEvidence(RankFactor::SetupState)
    );
    assert_eq!(
        standing(resolution, &fixture.b_exe),
        &Standing::Alternative {
            lost_on: RankFactor::SetupState
        }
    );
}

#[test]
fn use_history_decides_between_two_initialised_installs() {
    let machine = Machine::new();
    let active = machine.portable("DuckStation", "bios");
    let idle = machine.portable("DuckStation-B", "bios");
    fs::remove_file(idle.join("playtime.dat")).unwrap();
    let outcome = resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto);
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.profile_root, active);
    assert_eq!(
        resolution.profile.reason,
        SelectionReason::StrongestEvidence(RankFactor::UseHistory)
    );
    assert_eq!(
        standing(resolution, &idle.join("DuckStation.AppImage")),
        &Standing::Alternative {
            lost_on: RankFactor::UseHistory
        }
    );
}

#[test]
fn a_portable_marker_pairs_the_executable_with_its_own_profile() {
    let fixture = real_style();
    let candidates = fixture.machine.adapter().discover();
    let pair = |exe: &Path| {
        candidates
            .iter()
            .find(|candidate| candidate.identity.executable.as_deref() == Some(exe))
            .map(|candidate| (candidate.identity.profile_root.clone(), candidate.layout))
            .unwrap()
    };
    assert_eq!(
        pair(&fixture.a_exe),
        (fixture.a_root.clone(), ProfileLayout::Portable)
    );
    assert_eq!(
        pair(&fixture.b_exe),
        (fixture.b_root.clone(), ProfileLayout::DefaultUser)
    );
    // No candidate ever crosses one installation with another's profile.
    for candidate in &candidates {
        if let Some(exe) = &candidate.identity.executable {
            let own = exe.parent().unwrap() == candidate.identity.profile_root;
            let default = candidate.identity.profile_root == fixture.b_root;
            assert!(own || default, "{} crossed", candidate.identity);
        }
    }
}

#[test]
fn stale_flatpak_data_is_ignored() {
    let fixture = real_style();
    let outcome =
        resolve_emulator_profile(&fixture.machine.adapter(), &EmulatorSelectionMode::Auto);
    let resolution = resolved(&outcome);
    let stale = resolution
        .assessments
        .iter()
        .find(|item| item.layout == ProfileLayout::FlatpakSandbox)
        .expect("stale flatpak data is reported");
    assert!(matches!(&stale.standing, Standing::Unusable(reasons)
        if reasons.contains(&UnusableReason::FlatpakNotInstalled)));
    assert_ne!(resolution.profile.layout, ProfileLayout::FlatpakSandbox);
}

#[test]
fn a_stale_override_does_not_win_auto() {
    let fixture = real_style();
    let mut roots = fixture.machine.roots();
    roots.explicit_executables = vec![fixture.b_exe.clone()];
    let outcome = resolve_emulator_profile(
        &fixture.machine.adapter_with(roots),
        &EmulatorSelectionMode::Auto,
    );
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.executable, fixture.a_exe);
    // It is evidence, though: B carries it.
    let b = resolution
        .assessments
        .iter()
        .find(|item| item.identity.executable.as_deref() == Some(fixture.b_exe.as_path()))
        .unwrap();
    assert!(
        b.evidence
            .iter()
            .any(|item| item.kind == EvidenceKind::UserOverrideNamesIt)
    );
}

#[test]
fn real_style_fixture_selects_portable_and_explains_why() {
    let fixture = real_style();
    let outcome =
        resolve_emulator_profile(&fixture.machine.adapter(), &EmulatorSelectionMode::Auto);
    let resolution = resolved(&outcome);
    let profile = &resolution.profile;
    assert_eq!(profile.executable, fixture.a_exe);
    assert_eq!(profile.profile_root, fixture.a_root);
    let kinds: Vec<EvidenceKind> = profile.evidence.iter().map(|item| item.kind).collect();
    for expected in [
        EvidenceKind::PortableMarker,
        EvidenceKind::SetupComplete,
        EvidenceKind::UseHistory,
        EvidenceKind::GameSpecificConfig,
        EvidenceKind::DesktopLauncher,
    ] {
        assert!(kinds.contains(&expected), "missing {expected:?}: {kinds:?}");
    }
    // The BIOS problem is reported, not fixed, and does not stop selection.
    assert!(profile.warnings.iter().any(|warning| matches!(
        warning,
        ResolutionWarning::Configuration(ConfigurationWarning::BiosDirectoryMissing { .. })
    )));
    // WHY NOT the others: B outranked on setup state, flatpak data unusable.
    assert!(matches!(
        standing(resolution, &fixture.b_exe),
        Standing::Alternative { .. }
    ));
    assert!(
        resolution
            .assessments
            .iter()
            .any(|item| matches!(item.standing, Standing::Unusable(_)))
    );
}

// ---------------------------------------------------------------------------
// PREFERRED
// ---------------------------------------------------------------------------

fn identity_of(exe: &Path, root: &Path) -> CandidateIdentity {
    CandidateIdentity {
        emulator: InventoryEmulator::DuckStation,
        executable: Some(exe.to_path_buf()),
        profile_root: root.to_path_buf(),
    }
}

#[test]
fn a_valid_preferred_candidate_wins_even_over_stronger_evidence() {
    let fixture = real_style();
    let preferred = identity_of(&fixture.b_exe, &fixture.b_root);
    let outcome = resolve_emulator_profile(
        &fixture.machine.adapter(),
        &EmulatorSelectionMode::Preferred(preferred),
    );
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.executable, fixture.b_exe);
    assert_eq!(
        resolution.profile.reason,
        SelectionReason::PreferredAndUsable
    );
    assert_eq!(
        resolution.profile.preference,
        PreferenceState::PreferredHonoured
    );
    assert!(matches!(
        standing(resolution, &fixture.a_exe),
        Standing::NotPreferred { .. }
    ));
}

#[test]
fn a_broken_preferred_candidate_falls_back_with_a_warning() {
    let fixture = real_style();
    fs::remove_file(&fixture.b_exe).unwrap();
    let preferred = identity_of(&fixture.b_exe, &fixture.b_root);
    let outcome = resolve_emulator_profile(
        &fixture.machine.adapter(),
        &EmulatorSelectionMode::Preferred(preferred.clone()),
    );
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.executable, fixture.a_exe);
    assert_eq!(resolution.profile.mode, SelectionModeKind::Preferred);
    assert!(resolution.profile.warnings.iter().any(|warning| matches!(
        warning,
        ResolutionWarning::PreferredProfileBypassed { preferred: bypassed, .. } if *bypassed == preferred
    )));
    assert!(matches!(
        resolution.profile.preference,
        PreferenceState::PreferredBypassed(_)
    ));
}

#[test]
fn a_preferred_candidate_with_a_changed_pairing_is_bypassed_not_trusted() {
    let fixture = real_style();
    // B's executable is remembered with A's profile: that is not a real pair.
    let stale = identity_of(&fixture.b_exe, &fixture.a_root);
    let outcome = resolve_emulator_profile(
        &fixture.machine.adapter(),
        &EmulatorSelectionMode::Preferred(stale),
    );
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.executable, fixture.a_exe);
    assert!(
        resolution
            .profile
            .warnings
            .iter()
            .any(|warning| matches!(warning, ResolutionWarning::PreferredProfileBypassed { .. }))
    );
}

// ---------------------------------------------------------------------------
// FORCED
// ---------------------------------------------------------------------------

#[test]
fn a_viable_forced_candidate_is_used_exactly() {
    let fixture = real_style();
    let forced = identity_of(&fixture.b_exe, &fixture.b_root);
    let outcome = resolve_emulator_profile(
        &fixture.machine.adapter(),
        &EmulatorSelectionMode::Forced(forced),
    );
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.executable, fixture.b_exe);
    assert_eq!(resolution.profile.profile_root, fixture.b_root);
    assert_eq!(resolution.profile.reason, SelectionReason::ForcedByUser);
    assert_eq!(resolution.profile.preference, PreferenceState::Forced);
    // Reported, never repaired: the uninitialised profile still says so.
    assert_eq!(resolution.profile.readiness.setup, SetupState::NotRecorded);
}

#[test]
fn a_broken_forced_candidate_refuses_rather_than_switching() {
    let fixture = real_style();
    fs::remove_file(&fixture.b_exe).unwrap();
    let forced = identity_of(&fixture.b_exe, &fixture.b_root);
    let outcome = resolve_emulator_profile(
        &fixture.machine.adapter(),
        &EmulatorSelectionMode::Forced(forced.clone()),
    );
    match outcome {
        ResolutionOutcome::Refused(ForcedRefusal::ForcedProfileUnavailable { pinned, reasons }) => {
            assert_eq!(pinned, forced);
            assert!(reasons.contains(&UnusableReason::ExecutableMissing));
        }
        other => panic!("expected a forced refusal, got {other:?}"),
    }
}

#[test]
fn a_forced_cross_pairing_is_invalid() {
    let fixture = real_style();
    let crossed = identity_of(&fixture.b_exe, &fixture.a_root);
    let outcome = resolve_emulator_profile(
        &fixture.machine.adapter(),
        &EmulatorSelectionMode::Forced(crossed),
    );
    assert!(matches!(
        outcome,
        ResolutionOutcome::Refused(ForcedRefusal::ForcedProfileInvalid { reasons, .. })
            if reasons.contains(&UnusableReason::IncompatiblePairing)
    ));
}

#[test]
fn a_forced_candidate_with_a_missing_profile_is_unavailable() {
    let machine = Machine::new();
    let exe = machine.app("emulators").join("DuckStation.AppImage");
    executable(&exe);
    let root = machine.home.join(".local/share/duckstation");
    let outcome = resolve_emulator_profile(
        &machine.adapter(),
        &EmulatorSelectionMode::Forced(identity_of(&exe, &root)),
    );
    assert!(matches!(
        outcome,
        ResolutionOutcome::Refused(ForcedRefusal::ForcedProfileUnavailable { .. })
    ));
    assert!(!root.exists(), "the resolver must not create the profile");
}

// ---------------------------------------------------------------------------
// Ambiguity and absence
// ---------------------------------------------------------------------------

#[test]
fn equal_healthy_candidates_are_ambiguous() {
    let machine = Machine::new();
    machine.portable("DuckStation", "bios");
    machine.portable("DuckStation-B", "bios");
    let outcome = resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto);
    match outcome {
        ResolutionOutcome::Ambiguous {
            choices,
            assessments,
        } => {
            assert_eq!(choices.len(), 2);
            assert!(
                choices
                    .iter()
                    .all(|choice| choice.standing == Standing::Tied)
            );
            assert_eq!(assessments.len(), 2);
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

#[test]
fn two_executables_of_one_profile_are_not_a_question() {
    let machine = Machine::new();
    let root = machine.portable("DuckStation", "bios");
    executable(&root.join("duckstation-qt.AppImage"));
    let outcome = resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto);
    let resolution = resolved(&outcome);
    assert_eq!(resolution.profile.profile_root, root);
    assert_eq!(
        resolution.profile.reason,
        SelectionReason::EquivalentExecutableOfSameProfile
    );
    assert!(
        resolution
            .profile
            .warnings
            .iter()
            .any(|warning| matches!(warning, ResolutionWarning::EquivalentExecutables { .. }))
    );
}

#[test]
fn nothing_installed_is_no_installation_and_an_executable_without_a_profile_is_not_usable() {
    let machine = Machine::new();
    assert!(matches!(
        resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto),
        ResolutionOutcome::NoInstallation { .. }
    ));
    executable(&machine.app("emulators").join("DuckStation.AppImage"));
    assert!(matches!(
        resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto),
        ResolutionOutcome::NoUsableProfile { .. }
    ));
}

// ---------------------------------------------------------------------------
// DuckStation paths and warnings
// ---------------------------------------------------------------------------

#[test]
fn settings_and_folders_resolve_with_folder_overrides() {
    let machine = Machine::new();
    let root = machine.portable("DuckStation", "bios");
    write(
        &root.join("settings.ini"),
        &format!(
            "{COMPLETE}\n[Folders]\nCheats = my-cheats\nGameSettings = /opt/not/there/gs\n\n[BIOS]\nSearchDirectory = bios\n"
        ),
    );
    fs::create_dir_all(root.join("my-cheats")).unwrap();
    let outcome = resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto);
    let profile = &resolved(&outcome).profile;
    let cheats = &profile.folders[&ProfileFolder::Cheats];
    assert_eq!(cheats.path, root.join("my-cheats"));
    assert!(cheats.custom && cheats.exists);
    let game_settings = &profile.folders[&ProfileFolder::GameSettings];
    assert_eq!(game_settings.path, PathBuf::from("/opt/not/there/gs"));
    assert!(game_settings.custom && !game_settings.exists);
    assert!(profile.warnings.iter().any(|warning| matches!(
        warning,
        ResolutionWarning::Configuration(ConfigurationWarning::CustomFolderMissing {
            folder: ProfileFolder::GameSettings,
            ..
        })
    )));
    match &profile.details {
        Some(EmulatorDetails::DuckStation(details)) => {
            let folders = details.folders.as_ref().unwrap();
            assert_eq!(folders.cheats, root.join("my-cheats"));
        }
        other => panic!("expected DuckStation details, got {other:?}"),
    }
}

#[test]
fn a_relative_bios_directory_resolves_against_the_profile_and_a_missing_one_is_only_a_warning() {
    let machine = Machine::new();
    let root = machine.portable("DuckStation", "nowhere/bios");
    let outcome = resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto);
    let profile = &resolved(&outcome).profile;
    let bios = &profile.folders[&ProfileFolder::BiosSearch];
    assert_eq!(bios.path, root.join("nowhere/bios"));
    assert!(!bios.exists);
    assert!(profile.warnings.iter().any(|warning| matches!(
        warning,
        ResolutionWarning::Configuration(ConfigurationWarning::BiosDirectoryMissing { .. })
    )));
    assert_eq!(profile.readiness.setup, SetupState::Complete);
}

#[test]
fn an_incomplete_setup_is_reported_not_repaired() {
    let machine = Machine::new();
    let root = machine.portable("DuckStation", "bios");
    write(
        &root.join("settings.ini"),
        "[Main]\nSetupWizardIncomplete = true\n",
    );
    let before = snapshot(&machine.home);
    let outcome = resolve_emulator_profile(&machine.adapter(), &EmulatorSelectionMode::Auto);
    let profile = &resolved(&outcome).profile;
    assert_eq!(profile.readiness.setup, SetupState::Incomplete);
    assert!(profile.warnings.iter().any(|warning| matches!(
        warning,
        ResolutionWarning::Configuration(ConfigurationWarning::SetupWizardIncomplete)
    )));
    assert_eq!(snapshot(&machine.home), before);
}

#[test]
fn resolving_in_every_mode_writes_nothing() {
    let fixture = real_style();
    let before = snapshot(fixture.machine.dir.path());
    let adapter = fixture.machine.adapter();
    let identities = [
        identity_of(&fixture.a_exe, &fixture.a_root),
        identity_of(&fixture.b_exe, &fixture.b_root),
        identity_of(&fixture.b_exe, &fixture.a_root),
        identity_of(
            &fixture.machine.home.join("missing.AppImage"),
            &fixture.machine.home.join("missing-root"),
        ),
    ];
    let _ = resolve_emulator_profile(&adapter, &EmulatorSelectionMode::Auto);
    for identity in identities {
        let _ = resolve_emulator_profile(
            &adapter,
            &EmulatorSelectionMode::Preferred(identity.clone()),
        );
        let _ = resolve_emulator_profile(&adapter, &EmulatorSelectionMode::Forced(identity));
    }
    assert_eq!(snapshot(fixture.machine.dir.path()), before);
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

#[test]
fn refresh_notices_that_the_installation_changed() {
    let fixture = real_style();
    let adapter = fixture.machine.adapter();
    let first = resolve_emulator_profile(&adapter, &EmulatorSelectionMode::Auto);
    let (same, changed) = refresh_emulator_profile(&adapter, &EmulatorSelectionMode::Auto, &first);
    assert!(!changed);
    assert_eq!(same, first);

    // The AppImage is removed and the marker disappears: A is gone.
    fs::remove_file(&fixture.a_exe).unwrap();
    fs::remove_file(fixture.a_root.join("portable.txt")).unwrap();
    let (second, changed) =
        refresh_emulator_profile(&adapter, &EmulatorSelectionMode::Auto, &first);
    assert!(changed);
    assert_eq!(resolved(&second).profile.executable, fixture.b_exe);

    // The user initialises B's profile; the answer is refreshed again.
    write(
        &fixture.b_root.join("settings.ini"),
        &format!("{COMPLETE}\n[BIOS]\nSearchDirectory = bios\n"),
    );
    let (third, changed) =
        refresh_emulator_profile(&adapter, &EmulatorSelectionMode::Auto, &second);
    assert!(changed);
    assert_eq!(
        resolved(&third).profile.readiness.setup,
        SetupState::Complete
    );
}

#[test]
fn a_remembered_pair_goes_stale_when_a_marker_changes_the_profile() {
    let fixture = real_style();
    let pinned = identity_of(&fixture.a_exe, &fixture.a_root);
    let adapter = fixture.machine.adapter();
    let mode = EmulatorSelectionMode::Forced(pinned);
    assert!(
        resolve_emulator_profile(&adapter, &mode)
            .resolved()
            .is_some()
    );
    // Without the marker and settings.ini beside it, A's executable would use
    // the default profile, not the directory it sits in.
    fs::remove_file(fixture.a_root.join("portable.txt")).unwrap();
    fs::remove_file(fixture.a_root.join("settings.ini")).unwrap();
    assert!(matches!(
        resolve_emulator_profile(&adapter, &mode),
        ResolutionOutcome::Refused(ForcedRefusal::ForcedProfileInvalid { reasons, .. })
            if reasons.contains(&UnusableReason::IncompatiblePairing)
    ));
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

#[test]
fn the_selection_mode_round_trips_and_other_emulators_are_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("emulator_selection.toml");
    assert_eq!(
        load_selection_from(&path, InventoryEmulator::DuckStation).unwrap(),
        EmulatorSelectionMode::Auto
    );
    let duck = EmulatorSelectionMode::Forced(identity_of(
        Path::new("/home/u/Applications/DuckStation/DuckStation.AppImage"),
        Path::new("/home/u/Applications/DuckStation"),
    ));
    let psp = EmulatorSelectionMode::Preferred(CandidateIdentity {
        emulator: InventoryEmulator::Ppsspp,
        executable: None,
        profile_root: PathBuf::from("/home/u/.config/ppsspp"),
    });
    remember_selection_to(&path, InventoryEmulator::DuckStation, &duck).unwrap();
    remember_selection_to(&path, InventoryEmulator::Ppsspp, &psp).unwrap();
    assert_eq!(
        load_selection_from(&path, InventoryEmulator::DuckStation).unwrap(),
        duck
    );
    assert_eq!(
        load_selection_from(&path, InventoryEmulator::Ppsspp).unwrap(),
        psp
    );
    clear_selection_at(&path, InventoryEmulator::DuckStation).unwrap();
    assert_eq!(
        load_selection_from(&path, InventoryEmulator::DuckStation).unwrap(),
        EmulatorSelectionMode::Auto
    );
    assert_eq!(
        load_selection_from(&path, InventoryEmulator::Ppsspp).unwrap(),
        psp
    );
}

// ---------------------------------------------------------------------------
// Cheat planner consumer
// ---------------------------------------------------------------------------

#[test]
fn the_duckstation_cheat_planner_consumes_the_resolved_portable_profile() {
    let fixture = real_style();
    let outcome =
        resolve_emulator_profile(&fixture.machine.adapter(), &EmulatorSelectionMode::Auto);
    let profile = &resolved(&outcome).profile;
    let game = fixture.machine.home.join("game.cue");
    write(&game, "FILE \"game.bin\" BINARY\n");
    let request = DuckStationNativeRequest::for_resolved_profile(
        profile,
        game,
        vec!["SCUS-94601".to_string()],
        DuckStationDiscTopology::SingleDisc,
        DuckStationNativeOperation::Add {
            cheat: DuckStationNativeCheat {
                name: "Infinite Health".into(),
                metadata: BTreeMap::new(),
                comments: Vec::new(),
                code_lines: vec!["80010000 0001".into()],
            },
            enable: true,
        },
        true,
    )
    .unwrap();
    let plan = plan_duckstation_native(&request).unwrap();
    assert_eq!(
        plan.preview.cht_path,
        fixture.a_root.join("cheats/SCUS-94601.cht")
    );
    assert!(!plan.preview.cht_path.starts_with(&fixture.b_root));
    assert!(!plan.preview.cht_path.exists(), "planning writes nothing");
}

#[test]
fn a_non_duckstation_profile_cannot_drive_the_duckstation_planner() {
    let machine = Machine::new();
    let ppsspp = ppsspp_machine(&machine);
    let outcome = resolve_emulator_profile(&ppsspp, &EmulatorSelectionMode::Auto);
    let profile = &resolved(&outcome).profile;
    assert!(
        DuckStationNativeRequest::for_resolved_profile(
            profile,
            PathBuf::from("/x"),
            vec![],
            DuckStationDiscTopology::SingleDisc,
            DuckStationNativeOperation::Remove { name: "x".into() },
            false,
        )
        .is_err()
    );
}

// ---------------------------------------------------------------------------
// Second emulator
// ---------------------------------------------------------------------------

fn ppsspp_machine(machine: &Machine) -> PpssppAdapter {
    let exe = machine.home.join(".local/bin/ppsspp");
    executable(&exe);
    let root = machine.home.join(".config/ppsspp");
    write(&root.join("PSP/SYSTEM/ppsspp.ini"), "[General]\n");
    write(&root.join("PSP/SAVEDATA/ULUS10041/DATA.BIN"), "x");
    let roots = PpssppProfileDiscoveryRoots {
        home: machine.home.clone(),
        xdg_config_home: machine.home.join(".config"),
        xdg_data_home: machine.home.join(".local/share"),
        explicit_configuration_roots: Vec::new(),
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
        path_override: Some(vec![machine.home.join(".local/bin")]),
    };
    PpssppAdapter::new(roots, vec![machine.home.join(".local/share/applications")])
}

#[test]
fn the_ppsspp_adapter_resolves_through_the_same_generic_resolver() {
    let machine = Machine::new();
    let adapter = ppsspp_machine(&machine);
    // A second, never-used profile with no executable of its own.
    write(
        &machine
            .home
            .join(".local/share/ppsspp/PSP/SYSTEM/ppsspp.ini"),
        "[General]\n",
    );
    let outcome = resolve_emulator_profile(&adapter, &EmulatorSelectionMode::Auto);
    let resolution = resolved(&outcome);
    let profile = &resolution.profile;
    assert_eq!(profile.emulator, InventoryEmulator::Ppsspp);
    assert_eq!(profile.profile_root, machine.home.join(".config/ppsspp"));
    assert_eq!(profile.executable, machine.home.join(".local/bin/ppsspp"));
    assert_eq!(profile.layout, ProfileLayout::DefaultUser);
    assert_eq!(
        profile.main_config,
        machine.home.join(".config/ppsspp/PSP/SYSTEM/ppsspp.ini")
    );
    assert!(profile.folders.contains_key(&ProfileFolder::SaveData));
    assert!(
        profile
            .evidence
            .iter()
            .any(|item| item.kind == EvidenceKind::UseHistory)
    );
    // The same two paths are two usable candidates; the used one wins.
    assert_eq!(
        profile.reason,
        SelectionReason::StrongestEvidence(RankFactor::UseHistory)
    );
    // And FORCED works the same way for it.
    let forced = resolve_emulator_profile(
        &adapter,
        &EmulatorSelectionMode::Forced(profile.identity.clone()),
    );
    assert_eq!(
        resolved(&forced).profile.reason,
        SelectionReason::ForcedByUser
    );
}

#[test]
fn the_ppsspp_adapter_refuses_a_forced_pair_its_own_rule_does_not_allow() {
    let machine = Machine::new();
    let adapter = ppsspp_machine(&machine);
    let stranger = machine.home.join("Applications/other/ppsspp");
    executable(&stranger);
    let outcome = resolve_emulator_profile(
        &adapter,
        &EmulatorSelectionMode::Forced(CandidateIdentity {
            emulator: InventoryEmulator::Ppsspp,
            executable: Some(stranger),
            profile_root: machine.home.join(".config/ppsspp"),
        }),
    );
    assert!(matches!(
        outcome,
        ResolutionOutcome::Refused(ForcedRefusal::ForcedProfileInvalid { .. })
    ));
}

// ---------------------------------------------------------------------------
// Real-machine, read-only acceptance (ignored by default)
// ---------------------------------------------------------------------------

/// Run with `cargo test -p archivefs-core --lib real_machine -- --ignored --nocapture`.
/// Reads the developer's actual DuckStation installs; writes nothing.
#[test]
#[ignore = "inspects the real machine; read-only"]
fn real_machine_duckstation_auto_resolution_report() {
    let adapter = DuckStationAdapter::from_environment().unwrap();
    let outcome = resolve_emulator_profile(&adapter, &EmulatorSelectionMode::Auto);
    println!("{outcome:#?}");
    let resolution = resolved(&outcome);
    println!("EXECUTABLE {}", resolution.profile.executable.display());
    println!("PROFILE    {}", resolution.profile.profile_root.display());
    println!("REASON     {:?}", resolution.profile.reason);
    for item in &resolution.profile.evidence {
        println!(
            "  evidence {:?} {:?}: {}",
            item.kind, item.effect, item.detail
        );
    }
    for warning in &resolution.profile.warnings {
        println!("  warning  {warning:?}");
    }
    for item in &resolution.assessments {
        println!("  {} -> {:?}", item.identity, item.standing);
    }
}
