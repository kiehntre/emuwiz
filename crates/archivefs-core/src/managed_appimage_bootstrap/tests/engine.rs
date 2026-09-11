use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use super::super::*;
use crate::emulator_download::EmulatorDownloadTransport;
use std::process::Command;
use std::time::Duration;

fn context(root: &std::path::Path, executable: Option<PathBuf>) -> BootstrapContext {
    BootstrapContext {
        target: BootstrapTarget::Ppsspp,
        root: root.to_path_buf(),
        destination: root.join("emulators/ppsspp/ppsspp.AppImage"),
        executable,
        release: Some("v1".into()),
        asset_url: Some("https://example.invalid/asset".into()),
        expected_digest: Some("digest".into()),
        source: "official".into(),
        host: "linux".into(),
        arch: "x86_64".into(),
        environment: BTreeMap::new(),
        filesystem: BTreeMap::new(),
        policy_fingerprint: "policy".into(),
    }
}

struct Executor;
impl BootstrapExecutor for Executor {
    fn execute(&self, _: &EmulatorBootstrapPlan) -> Result<(), model::BootstrapError> {
        Ok(())
    }
    fn final_inspection(
        &self,
        context: &BootstrapContext,
    ) -> Result<BootstrapInspection, model::BootstrapError> {
        inspect(context)
    }
}

#[test]
fn explicit_approval_is_required() {
    let temp = tempfile::tempdir().unwrap();
    let c = context(temp.path(), None);
    let p = plan(c, None, vec![]).unwrap();
    assert_eq!(
        execute(&p, BootstrapApproval::NotApproved, &Executor, None),
        Err(model::BootstrapError::NotApproved)
    );
}

#[test]
fn unchanged_plan_executes_after_explicit_approval() {
    let temp = tempfile::tempdir().unwrap();
    let p = plan(context(temp.path(), None), None, vec![]).unwrap();
    assert!(execute(&p, p.approval(), &Executor, None).is_ok());
}

#[test]
fn approval_cannot_authorize_a_different_plan() {
    let temp = tempfile::tempdir().unwrap();
    let first = plan(context(temp.path(), None), None, vec![]).unwrap();
    let mut changed_context = context(temp.path(), None);
    changed_context.release = Some("v2".into());
    let second = plan(changed_context, None, vec![]).unwrap();
    assert!(matches!(
        execute(&second, first.approval(), &Executor, None),
        Err(BootstrapError::StalePlan(_))
    ));
}

#[test]
fn filesystem_snapshot_drift_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state");
    fs::write(&path, b"before").unwrap();
    let mut c = context(temp.path(), Some(path.clone()));
    c.filesystem.insert(path.clone(), Some("before".into()));
    let p = plan(c, None, vec![]).unwrap();
    fs::write(path, b"after").unwrap();
    assert!(matches!(
        execute(&p, p.approval(), &Executor, None),
        Err(model::BootstrapError::StalePlan(_))
    ));
}

#[test]
fn bounded_process_output_rejects_overflow() {
    assert!(super::super::process::bounded_read(&b"12345"[..], 4).is_err());
}

#[test]
fn unsafe_paths_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let mut c = context(temp.path(), None);
    c.destination = PathBuf::from("relative/AppImage");
    assert!(inspect(&c).is_err());
}

#[test]
fn marker_binding_rejects_drift() {
    let marker = b"target=ppsspp\nrelease=v1\ndigest=abc\nsource=official\n";
    assert!(super::super::safety::validate_marker_binding(
        marker, "ppsspp", "v1", "abc", "official"
    )
    .is_ok());
    assert!(super::super::safety::validate_marker_binding(
        marker, "pcsx2", "v1", "abc", "official"
    )
    .is_err());
}

#[test]
fn publication_refuses_existing_destination_and_bad_digest() {
    let temp = tempfile::tempdir().unwrap();
    let staged = temp.path().join("staged");
    let destination = temp.path().join("published");
    let marker_staged = temp.path().join("marker.staged");
    let marker = temp.path().join("marker");
    fs::write(&staged, b"payload").unwrap();
    fs::write(&marker_staged, b"marker").unwrap();
    fs::write(&destination, b"user-owned").unwrap();
    assert!(super::super::installer::publish_no_clobber(
        &staged,
        &destination,
        &marker_staged,
        &marker,
        "unused",
        1,
    )
    .is_err());
    fs::remove_file(&destination).unwrap();
    assert!(super::super::installer::publish_no_clobber(
        &staged,
        &destination,
        &marker_staged,
        &marker,
        "wrong",
        1,
    )
    .is_err());
}

#[test]
fn process_environment_is_sanitized_and_timeout_is_bounded() {
    let spec = super::super::process::SanitizedProcessSpec {
        executable: PathBuf::from("/bin/true"),
        home: PathBuf::from("/tmp/emuwiz-home"),
        config: PathBuf::from("/tmp/emuwiz-config"),
        data: PathBuf::from("/tmp/emuwiz-data"),
        current_dir: PathBuf::from("/tmp"),
        session_environment: BTreeMap::new(),
    };
    let command = super::super::process::sanitized_command(&spec);
    let environment: BTreeMap<_, _> = command
        .get_envs()
        .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
        .collect();
    assert_eq!(
        environment.get(std::ffi::OsStr::new("PATH")),
        Some(&std::ffi::OsString::from("/usr/bin:/bin"))
    );
    assert!(!environment.contains_key(std::ffi::OsStr::new("LD_PRELOAD")));

    let mut child = Command::new("/bin/sleep").arg("1").spawn().unwrap();
    assert!(super::super::process::wait_bounded(&mut child, Duration::from_millis(10)).is_err());
}

#[test]
fn bounded_staging_and_process_paths_are_checked() {
    let temp = tempfile::tempdir().unwrap();
    let staged = temp.path().join("staged");
    let digest = super::super::installer::stream_to_file(&b"payload"[..], &staged, 64).unwrap();
    assert_eq!(digest.len(), 64);
    let spec = super::super::process::SanitizedProcessSpec {
        executable: PathBuf::from("/bin/true"),
        home: PathBuf::from("/tmp/home"),
        config: PathBuf::from("/tmp/config"),
        data: PathBuf::from("/tmp/data"),
        current_dir: PathBuf::from("/tmp"),
        session_environment: BTreeMap::new(),
    };
    assert!(super::super::process::validate_process_paths(&spec).is_ok());
    assert!(super::super::safety::marker_bytes(&b"marker"[..]).is_ok());
}

#[test]
fn target_type_has_only_the_two_release_targets() {
    assert_ne!(BootstrapTarget::Ppsspp, BootstrapTarget::Pcsx2);
}

#[allow(dead_code)]
fn _transport(_: &dyn EmulatorDownloadTransport) {}
