use super::*;
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};

fn identity(platform: &str, key: &str) -> CanonicalIdentityStatus {
    CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: platform.into(),
        game_key: key.into(),
    })
}

fn binding() -> Result<ScummVmNativeLaunchBinding, String> {
    Ok(ScummVmNativeLaunchBinding {
        executable: "/usr/games/scummvm".into(),
    })
}

#[test]
fn verified_folder_uses_id_and_path_as_separate_arguments() {
    let folder = std::path::Path::new("/games/Name with spaces/é");
    let plan = build_scummvm_command_plan(
        &identity("ScummVM", "scumm:monkey"),
        Some("scumm:monkey"),
        folder,
        &binding(),
    );
    let command = plan.command.unwrap();
    assert_eq!(command.arguments[0], "-p");
    assert_eq!(command.arguments[1], "/games/Name with spaces/é");
    assert_eq!(command.arguments[2], "scumm:monkey");
    assert!(plan.blockers.is_empty());
}

#[test]
fn folder_name_does_not_authorize_or_change_game_id() {
    let plan = build_scummvm_command_plan(
        &identity("ScummVM", "sci:demo"),
        Some("sci:demo"),
        std::path::Path::new("/games/misleading-name"),
        &binding(),
    );
    assert_eq!(plan.command.unwrap().selection.game_id, "sci:demo");
}

#[test]
fn wrong_missing_malformed_and_conflicting_identity_refuse() {
    for (status, id) in [
        (CanonicalIdentityStatus::Unknown, Some("scumm:foo")),
        (CanonicalIdentityStatus::Conflicting, Some("scumm:foo")),
        (identity("PSP", "ULUS-1"), Some("scumm:foo")),
        (identity("ScummVM", "scumm:foo"), None),
        (identity("ScummVM", "scumm:foo"), Some("not-qualified")),
    ] {
        assert!(
            build_scummvm_command_plan(
                &status,
                id,
                std::path::Path::new("/games/game"),
                &binding(),
            )
            .command
            .is_none()
        );
    }
}

#[test]
fn missing_binding_refuses() {
    let plan = build_scummvm_command_plan(
        &identity("ScummVM", "scumm:foo"),
        Some("scumm:foo"),
        std::path::Path::new("/games/game"),
        &Err("native ScummVM executable is unavailable".into()),
    );
    assert!(plan.command.is_none());
}
