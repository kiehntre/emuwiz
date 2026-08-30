use super::*;
use crate::dat::dependency::{DependencyState, SetDependencyReport};
use crate::dat::set::{SetIdentity, SetResolution, SetState};
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};

fn identity(key: &str) -> CanonicalIdentityStatus {
    identity_for_platform("Arcade", key)
}

fn identity_for_platform(platform_id: &str, key: &str) -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: platform_id.to_string(),
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

#[test]
fn complete_neogeo_set_plans_through_native_mame() {
    let plan = build_mame_command_plan(
        &identity_for_platform("NeoGeo", "mslug3"),
        &[resolution("mslug3", SetState::Complete)],
        Some(std::path::Path::new("/usr/bin/mame")),
        true,
    );
    let command = plan.command.expect("complete NeoGeo set should plan");
    assert!(plan.blockers.is_empty());
    assert_eq!(command.arguments, vec![OsString::from("mslug3")]);
}

#[test]
fn neo_geo_cd_is_not_a_native_mame_platform() {
    let plan = build_mame_command_plan(
        &identity_for_platform("Neo Geo CD", "lastblad"),
        &[resolution("lastblad", SetState::Complete)],
        Some(std::path::Path::new("/usr/bin/mame")),
        true,
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::MamePlatformMismatch)
    );
}

#[test]
fn unrelated_platform_is_not_a_native_mame_platform() {
    let plan = build_mame_command_plan(
        &identity_for_platform("SNES", "mario"),
        &[resolution("mario", SetState::Complete)],
        Some(std::path::Path::new("/usr/bin/mame")),
        true,
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::MamePlatformMismatch)
    );
}

#[test]
fn neogeo_dependency_blocker_remains_authoritative() {
    let mut set = resolution("kof98", SetState::Complete);
    set.dependencies.state = DependencyState::Missing;
    let plan = build_mame_command_plan(
        &identity_for_platform("NeoGeo", "kof98"),
        &[set],
        Some(std::path::Path::new("/usr/bin/mame")),
        true,
    );
    assert!(plan.command.is_none());
    assert!(
        plan.blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::MameDependencyBlocked)
    );
}

#[test]
fn neogeo_argument_comes_from_dat_set_shortname_not_archive_filename() {
    let mut set = resolution("real_set", SetState::Complete);
    set.archive_path = "/library/renamed-display-title.zip".into();
    let plan = build_mame_command_plan(
        &identity_for_platform("NeoGeo", "real_set"),
        &[set],
        Some(std::path::Path::new("/usr/bin/mame")),
        true,
    );
    let command = plan.command.expect("verified set should plan");
    assert_eq!(command.arguments, vec![OsString::from("real_set")]);
}
