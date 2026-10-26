//! Synthetic evidence only: no filesystem, process or PCSX2 is touched.
use super::*;
use crate::launch::installation::LaunchInstallation;
use crate::patch_manager::pcsx2_local::{
    Pcsx2Executable, Pcsx2PatchCategory, Pcsx2PatchDirectory, Pcsx2PatchDirectoryState,
    Pcsx2ProfileScope,
};
use crate::patch_manager::pcsx2_pnach::{ManagedPnachCheat, PnachPatchLine};

const SERIAL: &str = "SLUS-20312";
const CRC: &str = "A1B2C3D4";

fn identity(
    state: Pcsx2IdentityState,
    serial: Option<&str>,
    crc: Option<&str>,
) -> Pcsx2GameIdentity {
    Pcsx2GameIdentity {
        archive_path: PathBuf::from("/synthetic/game.iso"),
        title: "Synthetic".into(),
        region: None,
        serial: serial.map(str::to_owned),
        executable_crc: crc.map(str::to_owned),
        state,
        evidence: Vec::new(),
        plain_failure_reason: (state != Pcsx2IdentityState::Verified)
            .then(|| "identity not verified".to_string()),
    }
}

fn verified() -> Pcsx2GameIdentity {
    identity(Pcsx2IdentityState::Verified, Some(SERIAL), Some(CRC))
}

fn profile(kind: Pcsx2InstallationType) -> Pcsx2Profile {
    Pcsx2Profile {
        profile_id: "synthetic".into(),
        installation_type: kind,
        scope: if kind == Pcsx2InstallationType::Portable {
            Pcsx2ProfileScope::Portable
        } else {
            Pcsx2ProfileScope::User
        },
        configuration_path: PathBuf::from("/synthetic/config/PCSX2"),
        provenance: "synthetic",
        eligible: true,
        blockers: Vec::new(),
        patch_directories: vec![Pcsx2PatchDirectory {
            path: PathBuf::from("/synthetic/config/PCSX2/cheats"),
            category: Pcsx2PatchCategory::Cheats,
            state: Pcsx2PatchDirectoryState::Available,
            warning: None,
            identity: None,
        }],
        configuration_identity: None,
        executable_candidates: Vec::new(),
    }
}

fn binding(
    mode: Pcsx2UserDirectoryMode,
    installation: LaunchInstallation,
) -> Pcsx2NativeLaunchBinding {
    Pcsx2NativeLaunchBinding {
        executable: PathBuf::from("/synthetic/bin/pcsx2-qt"),
        user_directory_mode: mode,
        installation,
    }
}

fn material(serial: Option<&str>, crc: &str, lines: usize) -> LocalPcsx2Discovery {
    LocalPcsx2Discovery {
        source_path: PathBuf::from("/synthetic/cheats.pnach"),
        source_sha256: "0".repeat(64),
        detected_serial: serial.map(str::to_owned),
        detected_crc: crc.to_string(),
        cheat: ManagedPnachCheat {
            id: "local-0000000000000000".into(),
            name: "Infinite health".into(),
            description: None,
            patch_lines: (0..lines)
                .map(|_| PnachPatchLine::parse("patch=1,EE,00100000,word,12345678").unwrap())
                .collect(),
        },
    }
}

fn routing_known() -> Pcsx2FolderRouting {
    Pcsx2FolderRouting::from_ini_text(
        "[Folders]\nMemoryCards = /real/memcards\nSavestates = /real/sstates\nBios = /real/bios\n\
         Snapshots = /real/snaps\nLogs = /real/logs\nCache = /real/cache\nTextures = /real/tex\n\
         InputProfiles = /real/input\nVideos = /real/videos\n",
    )
}

struct Case {
    identity: Pcsx2GameIdentity,
    profile: Pcsx2Profile,
    binding: Result<Pcsx2NativeLaunchBinding, Pcsx2LaunchBlocker>,
    material: Option<Result<LocalPcsx2Discovery, LocalPcsx2FileError>>,
    routing: Pcsx2FolderRouting,
}

impl Case {
    fn native() -> Self {
        Self {
            identity: verified(),
            profile: profile(Pcsx2InstallationType::Native),
            binding: Ok(binding(
                Pcsx2UserDirectoryMode::DefaultNative,
                LaunchInstallation::Native,
            )),
            material: Some(Ok(material(Some(SERIAL), CRC, 2))),
            routing: routing_known(),
        }
    }

    fn plan(&self) -> Pcsx2TempCheatReadiness {
        let mut profile = self.profile.clone();
        if profile.executable_candidates.is_empty()
            && let Ok(binding) = &self.binding
        {
            profile.executable_candidates.push(Pcsx2Executable {
                path: binding.executable.clone(),
                installation_type: profile.installation_type,
                launch: binding.installation.clone(),
            });
        }
        plan_pcsx2_temporary_cheat_launch(&Pcsx2TempCheatInput {
            identity: &self.identity,
            profile: &profile,
            binding: self.binding.as_ref(),
            cheat_material: self.material.as_ref().map(Result::as_ref),
            folder_routing: &self.routing,
        })
    }
}

fn kinds(readiness: &Pcsx2TempCheatReadiness) -> Vec<Pcsx2TempCheatBlockerKind> {
    readiness.blockers.iter().map(|item| item.kind).collect()
}

#[test]
fn native_with_clean_evidence_still_requires_runtime_proof() {
    let readiness = Case::native().plan();
    assert!(readiness.blockers.is_empty());
    assert_eq!(
        readiness.verdict,
        Pcsx2TempCheatVerdict::RequiresRuntimeProof
    );
    assert_eq!(readiness.runtime_proof_needed.len(), 5);
    assert!(
        readiness
            .staging_requirements
            .contains(&Pcsx2StagingRequirement::ExplicitDataPathArgument)
    );
    assert!(readiness.staging_requirements.contains(
        &Pcsx2StagingRequirement::StageCheatFileAndEnableList {
            file_name: format!("{SERIAL}_{CRC}.pnach"),
        }
    ));
}

#[test]
fn clean_evidence_never_exceeds_requires_runtime_proof() {
    // The input has no way to carry proof, so no verdict above
    // RequiresRuntimeProof is representable.
    let readiness = Case::native().plan();
    assert_eq!(
        readiness.verdict,
        Pcsx2TempCheatVerdict::RequiresRuntimeProof
    );
    assert_eq!(readiness.runtime_proof_needed.len(), 5);
}

#[test]
fn launchable_appimage_without_portable_marker_is_plannable_not_supported() {
    let mut case = Case::native();
    case.profile = profile(Pcsx2InstallationType::Native);
    case.binding = Ok(binding(
        Pcsx2UserDirectoryMode::DefaultNative,
        LaunchInstallation::AppImage {
            extract_and_run: false,
        },
    ));
    let readiness = case.plan();
    assert_eq!(
        readiness.verdict,
        Pcsx2TempCheatVerdict::RequiresRuntimeProof
    );
}

#[test]
fn flatpak_needs_a_sandbox_grant_and_its_own_proof() {
    let mut case = Case::native();
    case.profile = profile(Pcsx2InstallationType::FlatpakUser);
    case.binding = Ok(binding(
        Pcsx2UserDirectoryMode::FlatpakSandbox,
        LaunchInstallation::Flatpak {
            app_id: "net.pcsx2.PCSX2".into(),
        },
    ));
    let readiness = case.plan();
    assert_eq!(
        readiness.verdict,
        Pcsx2TempCheatVerdict::RequiresRuntimeProof
    );
    assert!(
        readiness
            .runtime_proof_needed
            .contains(&Pcsx2RuntimeProofItem::SandboxSawStagedRoot)
    );
    assert!(readiness.staging_requirements.contains(
        &Pcsx2StagingRequirement::FlatpakWorkspaceGrant {
            app_id: "net.pcsx2.PCSX2".into()
        }
    ));
    assert!(
        readiness
            .runtime_proof_needed
            .contains(&Pcsx2RuntimeProofItem::SandboxSawRealUserFolders)
    );
}

#[test]
fn portable_profile_is_blocked_even_when_it_can_launch() {
    let mut case = Case::native();
    case.profile = profile(Pcsx2InstallationType::Portable);
    case.binding = Ok(binding(
        Pcsx2UserDirectoryMode::PortableBesideExecutable,
        LaunchInstallation::AppImage {
            extract_and_run: false,
        },
    ));
    let readiness = case.plan();
    assert_eq!(readiness.verdict, Pcsx2TempCheatVerdict::Blocked);
    assert!(kinds(&readiness).contains(&Pcsx2TempCheatBlockerKind::PortableCannotBeRedirected));
    assert!(readiness.staging_requirements.is_empty());
}

#[test]
fn portable_marker_beside_a_native_executable_is_blocked() {
    let mut case = Case::native();
    case.binding = Err(Pcsx2LaunchBlocker {
        kind: Pcsx2LaunchBlockerKind::PortableMarkerConflict,
        detail: "portable.ini beside pcsx2-qt".into(),
    });
    let readiness = case.plan();
    assert_eq!(
        kinds(&readiness),
        vec![Pcsx2TempCheatBlockerKind::PortableCannotBeRedirected]
    );
}

#[test]
fn binding_blockers_map_to_distinct_reasons() {
    let cases = [
        (
            Pcsx2LaunchBlockerKind::UnsupportedInstallationType,
            Pcsx2TempCheatBlockerKind::UnsupportedInstallation,
        ),
        (
            Pcsx2LaunchBlockerKind::AmbiguousExecutable,
            Pcsx2TempCheatBlockerKind::LaunchBindingUnavailable,
        ),
        (
            Pcsx2LaunchBlockerKind::ExecutableMissing,
            Pcsx2TempCheatBlockerKind::LaunchBindingUnavailable,
        ),
        (
            Pcsx2LaunchBlockerKind::DefaultResolutionMismatch,
            Pcsx2TempCheatBlockerKind::ConfigurationUnknown,
        ),
        (
            Pcsx2LaunchBlockerKind::ProfileRootMismatch,
            Pcsx2TempCheatBlockerKind::ConfigurationUnknown,
        ),
    ];
    for (launch_kind, expected) in cases {
        let mut case = Case::native();
        case.binding = Err(Pcsx2LaunchBlocker {
            kind: launch_kind,
            detail: "synthetic".into(),
        });
        assert_eq!(kinds(&case.plan()), vec![expected], "{launch_kind:?}");
    }
}

#[test]
fn explicit_data_path_installation_is_not_silently_replaced() {
    let mut case = Case::native();
    case.binding = Ok(binding(
        Pcsx2UserDirectoryMode::ExplicitDataPath(PathBuf::from("/user/chosen")),
        LaunchInstallation::Native,
    ));
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::UnsupportedInstallation]
    );
}

#[test]
fn identity_states_are_never_guessed() {
    for state in [
        Pcsx2IdentityState::MissingCrc,
        Pcsx2IdentityState::Deferred,
        Pcsx2IdentityState::Ambiguous,
        Pcsx2IdentityState::Unsupported,
    ] {
        let mut case = Case::native();
        case.identity = identity(state, Some(SERIAL), None);
        let readiness = case.plan();
        assert_eq!(
            readiness.verdict,
            Pcsx2TempCheatVerdict::Blocked,
            "{state:?}"
        );
        assert!(kinds(&readiness).contains(&Pcsx2TempCheatBlockerKind::IdentityNotVerified));
    }
}

#[test]
fn verified_crc_without_serial_is_blocked_because_launch_needs_a_serial() {
    let mut case = Case::native();
    case.identity = identity(Pcsx2IdentityState::Verified, None, Some(CRC));
    case.material = Some(Ok(material(None, CRC, 1)));
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::SerialMissing]
    );
}

#[test]
fn cheat_material_for_another_game_is_a_conflict() {
    let mut case = Case::native();
    case.material = Some(Ok(material(Some(SERIAL), "DEADBEEF", 1)));
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::IdentityConflict]
    );
    case.material = Some(Ok(material(Some("SLES-50001"), CRC, 1)));
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::IdentityConflict]
    );
    case.material = Some(Err(LocalPcsx2FileError::IdentityConflict {
        detail: "targets another CRC".into(),
    }));
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::IdentityConflict]
    );
}

#[test]
fn missing_and_incompatible_cheat_material_are_distinct() {
    let mut case = Case::native();
    case.material = None;
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::CheatMaterialMissing]
    );
    case.material = Some(Err(LocalPcsx2FileError::NotFound {
        path: PathBuf::from("/synthetic/gone.pnach"),
        detail: "no such file".into(),
    }));
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::CheatMaterialMissing]
    );
    case.material = Some(Err(LocalPcsx2FileError::UnsupportedExtension {
        path: PathBuf::from("/synthetic/cheats.txt"),
    }));
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::CheatMaterialIncompatible]
    );
    case.material = Some(Ok(material(Some(SERIAL), CRC, 0)));
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::CheatMaterialIncompatible]
    );
}

#[test]
fn unknown_configuration_and_cheats_folder_are_blocked() {
    let mut case = Case::native();
    case.routing = Pcsx2FolderRouting::unknown("PCSX2.ini was not readable");
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::FolderRoutingUnknown]
    );

    let mut case = Case::native();
    case.profile.patch_directories[0].state = Pcsx2PatchDirectoryState::UnsafePath;
    assert_eq!(
        kinds(&case.plan()),
        vec![Pcsx2TempCheatBlockerKind::CheatsFolderUnavailable]
    );

    let mut case = Case::native();
    case.profile.eligible = false;
    assert!(kinds(&case.plan()).contains(&Pcsx2TempCheatBlockerKind::ConfigurationUnknown));
}

#[test]
fn relative_and_parent_folder_entries_are_unsafe_routing() {
    let mut case = Case::native();
    case.routing = Pcsx2FolderRouting::from_ini_text(
        "[Folders]\nBios = ../../../../run/user/1000/doc/bios\nMemoryCards = memcards\n\
         Savestates = /real/sstates\nCheats = ../shared/cheats\n",
    );
    let readiness = case.plan();
    assert_eq!(
        kinds(&readiness),
        vec![Pcsx2TempCheatBlockerKind::FolderRoutingUnsafe]
    );
    let reason = &readiness.blockers[0].reason;
    assert!(reason.contains("Bios"), "{reason}");
    assert!(reason.contains("MemoryCards"), "{reason}");
    assert!(reason.contains("Cheats"), "{reason}");
    assert!(!reason.contains("Savestates"), "{reason}");
}

#[test]
fn unspecified_user_folders_become_pin_requirements_not_blockers() {
    let mut case = Case::native();
    case.routing = Pcsx2FolderRouting::from_ini_text("[UI]\nTheme = dark\n");
    let readiness = case.plan();
    assert!(readiness.blockers.is_empty());
    for key in PCSX2_USER_DATA_FOLDER_KEYS {
        if *key == "GameSettings" {
            assert!(
                readiness
                    .staging_requirements
                    .contains(&Pcsx2StagingRequirement::CarryPerGameSettings)
            );
            assert!(!readiness.staging_requirements.contains(
                &Pcsx2StagingRequirement::PinUserFolderToRealDefault {
                    key: key.to_string()
                }
            ));
            continue;
        }
        assert!(
            readiness.staging_requirements.contains(
                &Pcsx2StagingRequirement::PinUserFolderToRealDefault {
                    key: (*key).to_string()
                }
            ),
            "{key}"
        );
    }
}

#[test]
fn redirected_launch_must_preserve_patch_and_additional_settings_folders() {
    // These v2.8.2 routing keys were omitted by the first readiness planner.
    // A relative Patches path must not silently switch to an empty workspace.
    for key in [
        "Patches",
        "Covers",
        "GameSettings",
        "UserResources",
        "DebuggerLayouts",
        "DebuggerSettings",
    ] {
        let mut case = Case::native();
        case.routing = Pcsx2FolderRouting::from_ini_text(&format!("[Folders]\n{key} = custom\n"));
        let readiness = case.plan();
        assert_eq!(
            kinds(&readiness),
            vec![Pcsx2TempCheatBlockerKind::FolderRoutingUnsafe]
        );
        assert!(readiness.blockers[0].reason.contains(key));

        case.routing =
            Pcsx2FolderRouting::from_ini_text(&format!("[Folders]\n{key} = /real/custom\n"));
        assert_eq!(
            case.plan().verdict,
            Pcsx2TempCheatVerdict::RequiresRuntimeProof
        );

        case.routing =
            Pcsx2FolderRouting::from_ini_text(&format!("[Folders]\n{key} = ../escape\n"));
        assert_eq!(
            kinds(&case.plan()),
            vec![Pcsx2TempCheatBlockerKind::FolderRoutingUnsafe]
        );
    }
}

#[test]
fn ini_classification_edges() {
    assert!(matches!(
        Pcsx2FolderRouting::from_ini_text("[Folders\nBios = /x"),
        Pcsx2FolderRouting::Unknown { .. }
    ));
    assert!(matches!(
        Pcsx2FolderRouting::from_ini_text("[Folders]\nBios = /x\0y"),
        Pcsx2FolderRouting::Unknown { .. }
    ));
    let oversized = "a".repeat(PCSX2_MAX_CONFIG_BYTES as usize + 1);
    assert!(matches!(
        Pcsx2FolderRouting::from_ini_text(&oversized),
        Pcsx2FolderRouting::Unknown { .. }
    ));
    // Keys outside [Folders] are ignored.
    let Pcsx2FolderRouting::Known { user_data, .. } =
        Pcsx2FolderRouting::from_ini_text("[Other]\nBios = relative\n[Folders]\n")
    else {
        panic!("expected known routing");
    };
    assert_eq!(user_data["Bios"], Pcsx2FolderEntry::Unspecified);
}

#[test]
fn planning_is_deterministic_and_does_not_mutate_inputs() {
    let case = Case::native();
    let before_profile = case.profile.clone();
    assert_eq!(case.plan(), case.plan());
    assert_eq!(case.profile, before_profile);
}

fn assert_blocked_with(case: &Case, kind: Pcsx2TempCheatBlockerKind) {
    let readiness = case.plan();
    assert_eq!(readiness.verdict, Pcsx2TempCheatVerdict::Blocked);
    assert!(kinds(&readiness).contains(&kind), "{:?}", kinds(&readiness));
    assert!(readiness.staging_requirements.is_empty());
}

#[test]
fn flatpak_profile_with_native_binding_is_refused() {
    let mut case = Case::native();
    case.profile = profile(Pcsx2InstallationType::FlatpakUser);
    assert_blocked_with(&case, Pcsx2TempCheatBlockerKind::LaunchBindingMismatch);
}

#[test]
fn native_profile_with_flatpak_binding_is_refused() {
    let mut case = Case::native();
    case.binding = Ok(binding(
        Pcsx2UserDirectoryMode::FlatpakSandbox,
        LaunchInstallation::Flatpak {
            app_id: "net.pcsx2.PCSX2".into(),
        },
    ));
    assert_blocked_with(&case, Pcsx2TempCheatBlockerKind::LaunchBindingMismatch);
}

#[test]
fn binding_executable_not_discovered_by_the_profile_is_refused() {
    let mut case = Case::native();
    case.profile.executable_candidates.push(Pcsx2Executable {
        path: PathBuf::from("/synthetic/bin/other-pcsx2"),
        installation_type: Pcsx2InstallationType::Native,
        launch: LaunchInstallation::Native,
    });
    assert_blocked_with(&case, Pcsx2TempCheatBlockerKind::LaunchBindingMismatch);
}

#[test]
fn absolute_cheats_folder_is_refused() {
    let mut case = Case::native();
    case.routing = Pcsx2FolderRouting::from_ini_text(
        "[Folders]\nCheats = /somewhere/else\nMemoryCards = /m\nSavestates = /s\nBios = /b\n\
         Snapshots = /sn\nLogs = /l\nCache = /c\nTextures = /t\nInputProfiles = /i\nVideos = /v\n",
    );
    assert_blocked_with(&case, Pcsx2TempCheatBlockerKind::FolderRoutingUnsafe);
}

#[test]
fn verified_state_without_crc_or_with_empty_serial_is_refused() {
    let mut case = Case::native();
    case.identity = identity(Pcsx2IdentityState::Verified, Some(SERIAL), None);
    case.material = Some(Ok(material(Some(SERIAL), "DEADBEEF", 1)));
    assert_blocked_with(&case, Pcsx2TempCheatBlockerKind::IdentityNotVerified);

    let mut case = Case::native();
    case.identity = identity(Pcsx2IdentityState::Verified, Some(""), Some(CRC));
    assert_blocked_with(&case, Pcsx2TempCheatBlockerKind::SerialMissing);
    case.identity = identity(Pcsx2IdentityState::Verified, Some("  "), Some(CRC));
    assert_blocked_with(&case, Pcsx2TempCheatBlockerKind::SerialMissing);
}

#[test]
fn duplicate_or_oddly_cased_folder_configuration_is_unknown_not_guessed() {
    for text in [
        "[Folders]\nBios=/a\n[Other]\nX=1\n[Folders]\nBios=/b\n",
        "[Folders]\nBios=/a\nBios=/b\n",
        "[Folders]\nBios=/a\nbios=/b\n",
        "[Folders]\nbios=/a\n",
        "[folders]\nBios=/a\n",
    ] {
        assert!(
            matches!(
                Pcsx2FolderRouting::from_ini_text(text),
                Pcsx2FolderRouting::Unknown { .. }
            ),
            "{text:?}"
        );
    }
    let mut case = Case::native();
    case.routing = Pcsx2FolderRouting::from_ini_text("[Folders]\nBios=/a\nBios=/b\n");
    assert_blocked_with(&case, Pcsx2TempCheatBlockerKind::FolderRoutingUnknown);
}
