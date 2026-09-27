use std::fs;

use super::*;
use crate::patch_manager::cheat_route::{
    CheatRouteBasis, CheatRouteTarget, cheat_apply_support, native_cheat_format,
};

fn route(target: CheatRouteTarget) -> CheatRoute {
    CheatRoute {
        platform_id: "PS2".to_string(),
        apply_support: cheat_apply_support(&target),
        native_format: native_cheat_format(&target),
        target,
        basis: CheatRouteBasis::ExplicitSelection,
        alternatives: Vec::new(),
    }
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

struct Fixture {
    _dir: tempfile::TempDir,
    cheats: PathBuf,
    installed: PathBuf,
    digest: String,
}

fn installed_pnach() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let cheats = dir.path().join("cheats");
    fs::create_dir_all(&cheats).unwrap();
    let installed = cheats.join("SLUS-20312_F460F374.pnach");
    let bytes = b"patch=1,EE,00100000,word,00000001\n";
    fs::write(&installed, bytes).unwrap();
    Fixture {
        digest: sha(bytes),
        _dir: dir,
        cheats,
        installed,
    }
}

fn pcsx2_input(fixture: &Fixture, enabled: Option<bool>) -> CheatLoadabilityInput {
    CheatLoadabilityInput {
        route: route(CheatRouteTarget::standalone("pcsx2")),
        installed_path: Some(fixture.installed.clone()),
        file_check: verify_installed_cheat_file(&fixture.installed, Some(&fixture.digest)),
        expected_path: Some(CheatLoadPathConvention {
            directory: fixture.cheats.clone(),
            file_name: None,
            required_suffix: Some(".pnach".to_string()),
            convention: "pcsx2_profile_cheats_directory",
        }),
        enablement: Some(CheatEnablementEvidence {
            enabled,
            setting: "[EmuCore] EnableCheats",
        }),
        process: EmulatorProcessObservation::NotRunning,
        identity: CheatIdentityStrength::Exact,
    }
}

#[test]
fn installed_bytes_are_verified_against_the_recorded_digest() {
    let fixture = installed_pnach();
    assert_eq!(
        verify_installed_cheat_file(&fixture.installed, Some(&fixture.digest)),
        CheatInstalledFileCheck::Verified {
            sha256: fixture.digest.clone()
        }
    );
    // Prefixed / upper-case digests compare equal.
    assert!(
        verify_installed_cheat_file(
            &fixture.installed,
            Some(&format!("sha256:{}", fixture.digest.to_uppercase()))
        )
        .verified()
    );
    fs::write(&fixture.installed, b"tampered").unwrap();
    assert!(matches!(
        verify_installed_cheat_file(&fixture.installed, Some(&fixture.digest)),
        CheatInstalledFileCheck::DigestMismatch { .. }
    ));
    fs::remove_file(&fixture.installed).unwrap();
    assert_eq!(
        verify_installed_cheat_file(&fixture.installed, Some(&fixture.digest)),
        CheatInstalledFileCheck::Missing
    );
}

#[cfg(unix)]
#[test]
fn symlinked_destination_is_not_treated_as_installed() {
    let fixture = installed_pnach();
    let link = fixture.cheats.join("link.pnach");
    std::os::unix::fs::symlink(&fixture.installed, &link).unwrap();
    assert_eq!(
        verify_installed_cheat_file(&link, Some(&fixture.digest)),
        CheatInstalledFileCheck::NotARegularFile
    );
}

#[test]
fn correct_destination_with_cheats_enabled_is_verified_by_config() {
    let fixture = installed_pnach();
    let report = assess_cheat_loadability(&pcsx2_input(&fixture, Some(true)));
    assert!(report.file_installed);
    assert_eq!(
        report.state,
        CheatLoadabilityState::LoadableVerifiedByConfig
    );
    assert_eq!(report.headline(), "Installed and ready.");
    assert_eq!(report.restart, CheatRestartRequirement::RestartGame);
    assert!(report.restart_note().is_some());
    assert!(
        report
            .evidence
            .contains(&CheatLoadabilityEvidence::CheatsEnabledInConfig {
                setting: "[EmuCore] EnableCheats"
            })
    );
    assert!(!report.headline().to_lowercase().contains("executed"));
}

#[test]
fn correct_destination_with_cheats_disabled_is_not_success() {
    let fixture = installed_pnach();
    let report = assess_cheat_loadability(&pcsx2_input(&fixture, Some(false)));
    assert!(report.file_installed);
    assert_eq!(report.state, CheatLoadabilityState::EmulatorCheatsDisabled);
    assert_eq!(report.state.will_load(), Some(false));
    assert_eq!(
        report.headline(),
        "Installed, but cheats are disabled in the selected emulator."
    );
    assert!(report.restart_note().is_none());
}

#[test]
fn unreadable_cheat_setting_is_only_expected_not_verified() {
    let fixture = installed_pnach();
    let report = assess_cheat_loadability(&pcsx2_input(&fixture, None));
    assert_eq!(report.state, CheatLoadabilityState::LoadableExpected);
    assert!(
        report
            .issues
            .contains(&CheatLoadabilityIssue::CheatSettingUnreadable {
                setting: "[EmuCore] EnableCheats"
            })
    );
}

#[test]
fn running_emulator_requires_restart() {
    let fixture = installed_pnach();
    let mut input = pcsx2_input(&fixture, Some(true));
    input.process = EmulatorProcessObservation::Running;
    let report = assess_cheat_loadability(&input);
    assert_eq!(report.state, CheatLoadabilityState::RestartRequired);
    assert_eq!(
        report.headline(),
        "Installed. Restart the emulator to load this cheat."
    );
}

#[test]
fn wrong_destination_is_path_not_observed() {
    let fixture = installed_pnach();
    let mut input = pcsx2_input(&fixture, Some(true));
    input.expected_path.as_mut().unwrap().directory = fixture.cheats.join("elsewhere");
    let report = assess_cheat_loadability(&input);
    assert!(report.file_installed);
    assert_eq!(report.state, CheatLoadabilityState::PathNotObserved);
    assert_eq!(
        report.headline(),
        "Installed, but the selected emulator is not currently configured to load it."
    );
    assert!(matches!(
        report.issues[0],
        CheatLoadabilityIssue::DestinationOutsideEmulatorDirectory { .. }
    ));
}

#[test]
fn wrong_suffix_is_path_not_observed() {
    let fixture = installed_pnach();
    let mut input = pcsx2_input(&fixture, Some(true));
    input.expected_path.as_mut().unwrap().required_suffix = Some(".ini".to_string());
    assert_eq!(
        assess_cheat_loadability(&input).state,
        CheatLoadabilityState::PathNotObserved
    );
}

#[test]
fn unverified_file_never_reports_loadable() {
    let fixture = installed_pnach();
    let mut input = pcsx2_input(&fixture, Some(true));
    input.file_check = CheatInstalledFileCheck::Missing;
    let report = assess_cheat_loadability(&input);
    assert!(!report.file_installed);
    assert_eq!(report.state, CheatLoadabilityState::PathNotObserved);
    assert_eq!(
        report.headline(),
        "The cheat file could not be verified after installing."
    );
}

#[test]
fn retroarch_core_is_recorded_in_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let core_dir = dir.path().join("cheats/mednafen_psx_hw");
    fs::create_dir_all(&core_dir).unwrap();
    let installed = core_dir.join("Game (USA).cht");
    fs::write(&installed, b"cheats = 0\n").unwrap();
    let input = CheatLoadabilityInput {
        route: route(CheatRouteTarget::retroarch(Some("mednafen_psx_hw"))),
        installed_path: Some(installed.clone()),
        file_check: verify_installed_cheat_file(&installed, Some(&sha(b"cheats = 0\n"))),
        expected_path: Some(CheatLoadPathConvention {
            directory: core_dir.clone(),
            file_name: Some("Game (USA).cht".to_string()),
            required_suffix: None,
            convention: "retroarch_per_core_game_specific_cheat",
        }),
        enablement: Some(CheatEnablementEvidence {
            enabled: Some(true),
            setting: "apply_cheats_after_load",
        }),
        process: EmulatorProcessObservation::NotObserved,
        identity: CheatIdentityStrength::Exact,
    };
    let report = assess_cheat_loadability(&input);
    assert_eq!(report.retroarch_core.as_deref(), Some("mednafen_psx_hw"));
    assert!(
        report
            .evidence
            .contains(&CheatLoadabilityEvidence::RetroArchCore {
                core: "mednafen_psx_hw".to_string()
            })
    );
    assert_eq!(
        report.state,
        CheatLoadabilityState::LoadableVerifiedByConfig
    );
    assert_eq!(report.restart, CheatRestartRequirement::ReloadContent);
}

#[test]
fn retroarch_platform_folder_install_needs_manual_load() {
    let dir = tempfile::tempdir().unwrap();
    let platform_dir = dir.path().join("cheats/Sony - PlayStation");
    fs::create_dir_all(&platform_dir).unwrap();
    let installed = platform_dir.join("Game (USA).cht");
    fs::write(&installed, b"cheats = 0\n").unwrap();
    let input = CheatLoadabilityInput {
        route: route(CheatRouteTarget::retroarch(Some("mednafen_psx_hw"))),
        installed_path: Some(installed.clone()),
        file_check: verify_installed_cheat_file(&installed, Some(&sha(b"cheats = 0\n"))),
        expected_path: Some(CheatLoadPathConvention {
            directory: dir.path().join("cheats/mednafen_psx_hw"),
            file_name: Some("Game (USA).cht".to_string()),
            required_suffix: None,
            convention: "retroarch_per_core_game_specific_cheat",
        }),
        enablement: None,
        process: EmulatorProcessObservation::NotObserved,
        identity: CheatIdentityStrength::TitleOnly,
    };
    let report = assess_cheat_loadability(&input);
    assert_eq!(report.state, CheatLoadabilityState::PathNotObserved);
    assert!(
        report
            .issues
            .contains(&CheatLoadabilityIssue::RetroArchManualLoadRequired)
    );
    assert!(
        report
            .issues
            .contains(&CheatLoadabilityIssue::GameRevisionNotVerified)
    );
}

#[test]
fn retroarch_without_core_is_ambiguous() {
    let fixture = installed_pnach();
    let mut input = pcsx2_input(&fixture, Some(true));
    input.route = route(CheatRouteTarget::retroarch(None));
    let report = assess_cheat_loadability(&input);
    assert_eq!(report.state, CheatLoadabilityState::AmbiguousProfile);
    assert_eq!(report.state.will_load(), None);
    assert_eq!(
        report.headline(),
        "EmuWiz cannot verify which RetroArch core this install belongs to."
    );
}

#[test]
fn inventory_only_emulator_is_unsupported() {
    let fixture = installed_pnach();
    let mut input = pcsx2_input(&fixture, Some(true));
    input.route = route(CheatRouteTarget::standalone("duckstation"));
    let report = assess_cheat_loadability(&input);
    assert_eq!(
        report.state,
        CheatLoadabilityState::UnsupportedBySelectedEmulator
    );
}

#[test]
fn title_only_identity_keeps_revision_warning_even_when_ready() {
    let fixture = installed_pnach();
    let mut input = pcsx2_input(&fixture, Some(true));
    input.identity = CheatIdentityStrength::TitleOnly;
    let report = assess_cheat_loadability(&input);
    assert_eq!(
        report.state,
        CheatLoadabilityState::LoadableVerifiedByConfig
    );
    assert!(
        report
            .issues
            .contains(&CheatLoadabilityIssue::GameRevisionNotVerified)
    );
    // Exact identity is never downgraded to a revision warning.
    let exact = assess_cheat_loadability(&pcsx2_input(&fixture, Some(true)));
    assert!(
        !exact
            .issues
            .contains(&CheatLoadabilityIssue::GameRevisionNotVerified)
    );
}

#[test]
fn config_bool_parser_handles_sections_quotes_and_comments() {
    let text = "[Core]\nEnableCheats = False\n[EmuCore]\nEnableCheats = true # comment\n";
    assert_eq!(
        parse_config_bool(text, Some("EmuCore"), "EnableCheats"),
        Some(true)
    );
    assert_eq!(
        parse_config_bool(text, Some("Core"), "enablecheats"),
        Some(false)
    );
    assert_eq!(
        parse_config_bool(text, Some("Missing"), "EnableCheats"),
        None
    );
    assert_eq!(
        parse_config_bool(
            "apply_cheats_after_load = \"true\"\n",
            None,
            "apply_cheats_after_load"
        ),
        Some(true)
    );
    assert_eq!(parse_config_bool("x = maybe\n", None, "x"), None);
}

#[test]
fn process_observation_reads_comm_names() {
    let proc_root = tempfile::tempdir().unwrap();
    fs::create_dir_all(proc_root.path().join("123")).unwrap();
    fs::write(proc_root.path().join("123/comm"), "pcsx2-qt\n").unwrap();
    fs::create_dir_all(proc_root.path().join("self")).unwrap();
    assert_eq!(
        observe_emulator_process_in(proc_root.path(), &CheatRouteTarget::standalone("pcsx2")),
        EmulatorProcessObservation::Running
    );
    assert_eq!(
        observe_emulator_process_in(proc_root.path(), &CheatRouteTarget::standalone("dolphin")),
        EmulatorProcessObservation::NotRunning
    );
    assert_eq!(
        observe_emulator_process_in(
            &proc_root.path().join("absent"),
            &CheatRouteTarget::standalone("pcsx2")
        ),
        EmulatorProcessObservation::NotObserved
    );
}
