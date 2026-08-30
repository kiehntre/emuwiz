use super::*;
use crate::dat::dependency::{DependencyState, SetDependencyReport};
use crate::dat::set::{SetIdentity, SetResolution, SetState};
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};

fn identity(key: &str) -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Arcade".to_string(),
        game_key: key.to_string(),
    })
}

fn resolution(name: &str, state: SetState) -> SetResolution {
    SetResolution {
        identity: SetIdentity {
            source_id: "mame".to_string(),
            game_name: name.to_string(),
        },
        archive_path: "/library/set.zip".into(),
        state,
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
    }
}

#[test]
fn complete_set_plans_one_native_set_name_argument() {
    let plan = build_mame_command_plan(
        &identity("pacman"),
        &[resolution("pacman", SetState::Complete)],
        Some(std::path::Path::new("/usr/bin/mame")),
        true,
    );
    let command = plan.command.expect("complete MAME set should plan");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.arguments, vec![OsString::from("pacman")]);
}

#[test]
fn set_and_dependency_verdicts_block_without_fallback() {
    let incomplete = build_mame_command_plan(
        &identity("pacman"),
        &[resolution("pacman", SetState::Incomplete)],
        Some(std::path::Path::new("/usr/bin/mame")),
        true,
    );
    assert!(incomplete.command.is_none());
    assert!(
        incomplete
            .blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::MameSetIncomplete)
    );

    let unavailable = build_mame_command_plan(
        &identity("pacman"),
        &[],
        Some(std::path::Path::new("/usr/bin/mame")),
        true,
    );
    assert!(
        unavailable
            .blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::MameSetVerdictUnavailable)
    );
}

#[test]
fn identity_mismatch_and_unconfigured_search_path_block() {
    let plan = build_mame_command_plan(
        &identity("other"),
        &[resolution("pacman", SetState::Complete)],
        Some(std::path::Path::new("/usr/bin/mame")),
        false,
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::MameSetIdentityUnavailable)
    );
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::MameSearchPathUnconfigured)
    );
}

#[test]
fn no_identity_or_executable_never_becomes_ready() {
    let plan = build_mame_command_plan(&CanonicalIdentityStatus::Unknown, &[], None, true);
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::IdentityUnresolved)
    );
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::MameEmulatorUnavailable)
    );
}
