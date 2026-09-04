use super::*;
use crate::launch::planning::{CandidatePreference, LaunchContentRef, ResolvedIdentity};
use crate::launch::readiness::FirmwareReadiness;

fn fixture() -> (HatariProfile, HatariGameInspection, LaunchCandidate) {
    let machine = crate::patch_manager::HatariMachineSettings {
        model: HatariMachineModel::St,
        ..Default::default()
    };
    let config = crate::patch_manager::HatariConfig {
        path: "/tmp/hatari.cfg".into(),
        exists: true,
        readable: true,
        malformed: false,
        machine: machine.clone(),
        floppies: Vec::new(),
        storage: Vec::new(),
        cartridge: crate::patch_manager::HatariStorage {
            mechanism: crate::patch_manager::HatariStorageMechanism::Cartridge,
            path: None,
            state: crate::patch_manager::HatariPathState::NotConfigured,
            read_only: None,
            boot_preferred: None,
            drive: None,
        },
        input: Default::default(),
        audio: Default::default(),
        video: Default::default(),
        tos_path: Some("/tmp/tos.img".into()),
        save_state_path: None,
        warnings: Vec::new(),
    };
    let inspection = HatariGameInspection {
        config,
        health: crate::patch_manager::HatariHealth {
            detected: true,
            config_readable: true,
            tos: crate::patch_manager::HatariTosRom {
                path: Some("/tmp/tos.img".into()),
                state: crate::patch_manager::HatariPathState::Present,
                health: HatariTosHealth::PresentUnverified,
                sha256: None,
                version: None,
                region: None,
            },
            machine: machine,
            warnings: Vec::new(),
        },
        selected_game: crate::patch_manager::HatariSelectedGame {
            canonical_platform: Some("AtariST".into()),
            identity: crate::patch_manager::HatariIdentityAssociation::CoreVerifiedAtari,
            verified_title: Some("Game".into()),
            per_game_profile_available: false,
            save_states: crate::patch_manager::HatariSaveStateInventory {
                configured_path: None,
                configured_state: crate::patch_manager::HatariPathState::NotConfigured,
                candidates: Vec::new(),
                complete: true,
            },
        },
    };
    let profile = HatariProfile {
        profile_id: "profile".into(),
        installation_type: crate::patch_manager::HatariInstallationType::Explicit,
        config_path: "/tmp/hatari.cfg".into(),
        provenance: "test",
        eligible: true,
        executable_candidates: vec![crate::patch_manager::HatariExecutable {
            path: "/usr/bin/hatari".into(),
            installation_type: crate::patch_manager::HatariInstallationType::Explicit,
            version: None,
        }],
    };
    let candidate = LaunchCandidate {
        target: LaunchTarget::Standalone {
            adapter_id: "hatari",
            profile_id: "profile".into(),
            profile_path: Some("/tmp/hatari.cfg".into()),
        },
        content: LaunchContentRef {
            kind: None,
            container: None,
            resolved_path: Some("/games/Game.st".into()),
            requires_mount: false,
            provenance: "verified".into(),
        },
        firmware: FirmwareReadiness::PresentUnverified,
        blockers: Vec::new(),
        warnings: Vec::new(),
        readiness: crate::launch::readiness::LaunchReadiness::ReadyWithWarnings,
        preference: CandidatePreference::SoleEligible,
    };
    (profile, inspection, candidate)
}

#[test]
fn deterministic_native_argv_keeps_selected_path_separate() {
    let (profile, inspection, candidate) = fixture();
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "AtariST".into(),
        game_key: "Game".into(),
    });
    let plan = build_hatari_command_plan(
        &identity,
        &candidate,
        &profile,
        &inspection,
        Path::new("/games/Game.st"),
        HatariMachineModel::St,
        'A',
        false,
    );
    let command = plan.command.unwrap();
    assert_eq!(
        command.arguments,
        vec![
            OsString::from("--configfile"),
            OsString::from("/tmp/hatari.cfg"),
            OsString::from("--disk-a"),
            OsString::from("/games/Game.st")
        ]
    );
}

#[test]
fn ipf_requires_explicit_backend_and_unknown_machine_fails_closed() {
    let (profile, inspection, candidate) = fixture();
    let identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "AtariST".into(),
        game_key: "Game".into(),
    });
    let plan = build_hatari_command_plan(
        &identity,
        &candidate,
        &profile,
        &inspection,
        Path::new("/games/Game.ipf"),
        HatariMachineModel::Unknown,
        'A',
        false,
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::HatariIpfBackendUnavailable)
    );
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::HatariMachineMismatch)
    );
}
