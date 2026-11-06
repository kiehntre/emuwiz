use super::*;
use std::os::unix::fs::PermissionsExt;

/// A stand-in for MAME (never the real program): answers `-listxml` with the
/// given machine body and `-verifyroms` with the given text.
fn fake(dir: &Path, name: &str, machine_inner: &str, mach_attrs: &str, verify: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n*-listxml*) cat <<'XML'\n<mame><machine name=\"gam\" {mach_attrs}><driver status=\"good\" emulation=\"good\"/>{machine_inner}</machine></mame>\nXML\n;;\n*-verifyroms*) cat <<'V'\n{verify}\nV\n;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn set_dir(root: &Path) -> PathBuf {
    let dir = root.join("arcade").join("gam");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn plan_with_mame() -> LaunchPlan {
    use archivefs_core::launch::{CandidatePreference, FirmwareReadiness, LaunchCandidate};
    LaunchPlan {
        platform_id: Some("Arcade".into()),
        game_key: Some("gam".into()),
        summary: Default::default(),
        candidates: vec![LaunchCandidate {
            target: LaunchTarget::Standalone {
                adapter_id: "mame",
                profile_id: "mame".into(),
                profile_path: None,
            },
            content: archivefs_core::launch::LaunchContentRef {
                kind: None,
                container: None,
                resolved_path: None,
                requires_mount: false,
                provenance: String::new(),
            },
            firmware: FirmwareReadiness::NotRequired,
            blockers: Vec::new(),
            warnings: Vec::new(),
            readiness: LaunchReadiness::Ready,
            preference: CandidatePreference::Undetermined,
        }],
        media_topology: None,
    }
}

#[test]
fn working_complete_set_folder_is_ready_with_a_complete_resolution() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(root.path(), "m1", "", "", "romset gam is good");
    let probe = probe_with(&exe, &set_dir(root.path()), &[]).unwrap();
    assert_eq!(probe.verdict.readiness, ArcadeReadiness::Ready);
    assert_eq!(probe.resolution.state, SetState::Complete);
    assert_eq!(probe.set, "gam");
}

#[test]
fn imperfect_set_gets_a_named_warning_on_the_mame_candidate() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(
        root.path(),
        "m2",
        "<feature type=\"sound\" status=\"imperfect\"/>",
        "",
        "romset gam is good",
    );
    let probe = probe_with(&exe, &set_dir(root.path()), &[]).unwrap();
    let mut plan = plan_with_mame();
    apply_warnings(&mut plan, &probe);
    assert_eq!(
        plan.candidates[0].readiness,
        LaunchReadiness::ReadyWithWarnings
    );
    assert!(plan.candidates[0].warnings[0].detail.contains("sound"));
    assert_eq!(plan.summary.ready_with_warnings, 1);
}

#[test]
fn blocked_set_yields_an_exact_blocker_and_an_incomplete_resolution() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(
        root.path(),
        "m3",
        "",
        "",
        "romset gam is bad\ngam         : x.bin (10 bytes) - NOT FOUND (neogeo)",
    );
    let probe = probe_with(&exe, &set_dir(root.path()), &[]).unwrap();
    assert_eq!(probe.resolution.state, SetState::Incomplete);
    let blocker = blocker_for(&probe.verdict);
    assert_eq!(blocker.kind, LaunchBlockerKind::MameDependencyBlocked);
    assert!(blocker.detail.contains("neogeo"));
}

#[test]
fn fruit_and_video_with_the_same_mame_data_get_the_same_readiness() {
    let root = tempfile::tempdir().unwrap();
    let video = fake(
        root.path(),
        "v",
        "<control type=\"joy\"/>",
        "",
        "romset gam is good",
    );
    let fruit = fake(
        root.path(),
        "f",
        "<control type=\"gambling\"/>",
        "ismechanical=\"yes\"",
        "romset gam is good",
    );
    let dir = set_dir(root.path());
    let v = probe_with(&video, &dir, &[]).unwrap();
    let f = probe_with(&fruit, &dir, &[]).unwrap();
    assert_eq!(v.resolution.state, f.resolution.state);
    assert_ne!(f.verdict.readiness, ArcadeReadiness::Blocked);
    assert!(
        f.verdict
            .warnings
            .iter()
            .any(|w| w.contains("gambling/fruit"))
    );
}

#[test]
fn files_and_unsafe_names_are_not_routed_here() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(root.path(), "m", "", "", "romset gam is good");
    let file = root.path().join("gam.zip");
    std::fs::write(&file, b"z").unwrap();
    assert!(probe_with(&exe, &file, &[]).is_none());
    let odd = root.path().join("arcade").join("a b");
    std::fs::create_dir_all(&odd).unwrap();
    assert!(probe_with(&exe, &odd, &[]).is_none());
}

#[test]
fn results_are_cached_in_memory_per_key() {
    let root = tempfile::tempdir().unwrap();
    let exe = fake(root.path(), "m", "", "", "romset gam is good");
    let dir = set_dir(root.path());
    let a = probe_with(&exe, &dir, &[]).unwrap();
    let b = probe_with(&exe, &dir, &[]).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
}
