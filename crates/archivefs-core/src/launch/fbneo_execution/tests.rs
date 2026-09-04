use super::*;
use crate::dat::dependency::{DependencyState, SetDependencyReport};
use crate::dat::model::DatEcosystem;
use crate::dat::set::{SetIdentity, SetResolution, SetState};
use crate::launch::fbneo_command::FbneoIdentityEvidence;
use crate::launch::planning::{CanonicalIdentityStatus, ResolvedIdentity};
use std::os::unix::fs::PermissionsExt;

fn request(executable: PathBuf) -> FbneoLaunchRequest {
    let selected_content = executable.with_extension("zip");
    std::fs::write(&selected_content, b"verified").unwrap();
    let expected =
        CapturedFileIdentity::capture(&std::fs::symlink_metadata(&selected_content).unwrap());
    FbneoLaunchRequest {
        identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "Arcade".into(),
            game_key: "sf2".into(),
        }),
        set: FbneoSetEvidence {
            driver_name: "sf2".into(),
            resolution: SetResolution {
                identity: SetIdentity {
                    source_id: "fbneo".into(),
                    game_name: "sf2".into(),
                },
                archive_path: selected_content.clone(),
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
        },
        expected_executable: executable,
        selected_content,
        expected_content_identity: Some(expected),
    }
}

#[test]
fn preflight_and_spawn_preserve_driver_and_paths_with_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("FinalBurn Neo");
    std::fs::write(
        &executable,
        b"#!/bin/sh\nprintf '%s' \"$1\" > \"$0.argv\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let request = request(executable.clone());
    let command = preflight_fbneo_launch(&request).unwrap();
    assert_eq!(command.arguments, vec![std::ffi::OsString::from("sf2")]);
    let mut process = spawn_fbneo(&command).unwrap();
    while process.poll().is_none() {
        std::thread::yield_now();
    }
    assert_eq!(
        std::fs::read_to_string(executable.with_extension("argv")).unwrap(),
        "sf2"
    );
}

#[test]
fn replaced_content_at_the_same_path_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("fbneo");
    std::fs::write(&executable, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let request = request(executable);
    std::fs::write(&request.selected_content, b"replacement with another size").unwrap();
    let error = preflight_fbneo_launch(&request).unwrap_err();
    assert!(
        error
            .blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::FbneoContentUnavailable)
    );
}

#[test]
fn selected_content_path_drift_and_symlinks_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("fbneo");
    std::fs::write(&executable, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut request = request(executable);
    request.selected_content = dir.path().join("other.zip");
    std::fs::write(&request.selected_content, b"other").unwrap();
    let error = preflight_fbneo_launch(&request).unwrap_err();
    assert!(
        error
            .blockers
            .iter()
            .any(|b| b.kind == LaunchBlockerKind::FbneoContentUnavailable)
    );
}
