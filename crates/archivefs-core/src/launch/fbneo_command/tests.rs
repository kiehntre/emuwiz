use super::*;
use crate::dat::dependency::{DependencyState, SetDependencyReport};
use crate::dat::model::DatEcosystem;
use crate::dat::set::{SetIdentity, SetResolution, SetState};
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};

fn identity(key: &str) -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Arcade".into(),
        game_key: key.into(),
    })
}

fn evidence(name: &str) -> FbneoSetEvidence {
    FbneoSetEvidence {
        driver_name: name.into(),
        resolution: SetResolution {
            identity: SetIdentity {
                source_id: "fbneo".into(),
                game_name: name.into(),
            },
            archive_path: "/library/renamed archive.zip".into(),
            state: SetState::Complete,
            members_required: Vec::new(),
            members_verified: Vec::new(),
            members_bad: Vec::new(),
            members_optional: Vec::new(),
            members_borrowed: Vec::new(),
            disks_required: Vec::new(),
            disks_verified: Vec::new(),
            disks_parent_required: Vec::new(),
            dependencies: SetDependencyReport {
                state: DependencyState::NotApplicable,
                requirements: Vec::new(),
            },
        },
        identity_evidence: FbneoIdentityEvidence::VerifiedDat {
            source_id: "fbneo".into(),
            ecosystem: DatEcosystem::FBNeo,
        },
    }
}

#[test]
fn trusted_fbneo_dat_identity_plans_native_driver_argument() {
    let plan = build_fbneo_command_plan(
        &identity("sf2"),
        &evidence("sf2"),
        Some(std::path::Path::new("/opt/FinalBurn Neo/fbneo")),
    );
    let command = plan.command.expect("trusted FBNeo evidence should plan");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.arguments, vec![OsString::from("sf2")]);
    assert_eq!(
        command.selected_content,
        std::path::PathBuf::from("/library/renamed archive.zip")
    );
}

#[test]
fn mame_only_identity_does_not_authorize_fbneo() {
    let mut set = evidence("pacman");
    set.identity_evidence = FbneoIdentityEvidence::MameOnly {
        source_id: "mame-listxml".into(),
    };
    let plan = build_fbneo_command_plan(
        &identity("pacman"),
        &set,
        Some(std::path::Path::new("/usr/bin/fbneo")),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| { blocker.kind == LaunchBlockerKind::FbneoCompatibilityUnavailable })
    );
}

#[test]
fn incomplete_set_and_unknown_dependency_do_not_claim_ready() {
    let mut set = evidence("galaga");
    set.resolution.state = SetState::Incomplete;
    set.resolution.dependencies.state = DependencyState::EvidenceUnavailable;
    let plan = build_fbneo_command_plan(
        &identity("galaga"),
        &set,
        Some(std::path::Path::new("/usr/bin/fbneo")),
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::FbneoSetIncomplete)
    );
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::FbneoDependencyBlocked)
    );
}

#[test]
fn parent_or_clone_name_is_not_substituted() {
    let plan = build_fbneo_command_plan(
        &identity("sf2ce"),
        &evidence("sf2ce"),
        Some(std::path::Path::new("/usr/bin/fbneo")),
    );
    assert_eq!(
        plan.command.unwrap().arguments,
        vec![OsString::from("sf2ce")]
    );
}

#[test]
fn unrelated_platform_and_missing_executable_are_blocked() {
    let other = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "SNES".into(),
        game_key: "mario".into(),
    });
    let plan = build_fbneo_command_plan(&other, &evidence("mario"), None);
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::FbneoPlatformMismatch)
    );
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::FbneoEmulatorUnavailable)
    );
}
