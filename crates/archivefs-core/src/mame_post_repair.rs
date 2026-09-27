//! Bounded, non-mutating verification after an exploded-directory MAME repair.
//!
//! Filesystem repair and MAME verification are deliberately separate facts:
//! an exact destination hash proves the copy, while MAME's `-verifyroms`
//! proves only what the selected MAME executable reports for one set.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha1::{Digest as Sha1Digest, Sha1};

pub const MAME_POST_REPAIR_SCHEMA_VERSION: u32 = 1;
pub const MAME_VERIFY_OUTPUT_LIMIT: usize = 64 * 1024;
pub const MAME_VERIFY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameSetIdentityReceipt {
    pub schema_version: u32,
    pub catalogue_sha256: String,
    pub catalogue_version: Option<String>,
    pub machine_shortname: String,
    pub parent_shortname: Option<String>,
    pub arcade_root: PathBuf,
    pub evidence_generation: String,
    pub dependency_topology_digest: String,
    pub captured_at_unix: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameRepairDestinationHash {
    pub path: PathBuf,
    pub expected_sha1: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameTargetedVerificationState {
    Passed,
    MissingDependencies,
    BadDump,
    NoDump,
    NonZeroExit,
    Timeout,
    UnexpectedOutput,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameRepairVerificationState {
    VerifiedComplete,
    VerifiedBestAvailable,
    StillIncomplete,
    NeedsRedump,
    VerificationFailed,
    VerificationUnavailable,
    StaleEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum MameRepairVerificationIssue {
    CatalogueChanged,
    CatalogueVersionChanged,
    SetShortnameChanged,
    ArcadeRootChanged,
    PlanChanged,
    TransactionChanged,
    DestinationChanged(PathBuf),
    DestinationMissing(PathBuf),
    DestinationUnsafe(PathBuf),
    MameUnavailable,
    MissingDependency(String),
    BadDumpDependency(String),
    NoDumpDependency(String),
    Timeout,
    NonZeroExit(i32),
    UnexpectedOutput,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameTargetedVerification {
    pub state: MameTargetedVerificationState,
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    pub bounded_output: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MameRepairVerification {
    pub schema_version: u32,
    pub identity: MameSetIdentityReceipt,
    pub transaction_id: String,
    pub repair_plan_digest: String,
    pub destination_hashes_verified: Vec<MameRepairDestinationHash>,
    pub targeted_mame: Option<MameTargetedVerification>,
    pub remaining_missing_dependencies: Vec<String>,
    pub remaining_bad_dependencies: Vec<String>,
    pub bad_dump_present: bool,
    pub no_dump_present: bool,
    pub state: MameRepairVerificationState,
    pub issues: Vec<MameRepairVerificationIssue>,
}

pub struct MamePostRepairVerificationRequest {
    pub identity: MameSetIdentityReceipt,
    pub transaction_id: String,
    pub repair_plan_digest: String,
    pub expected_destinations: Vec<MameRepairDestinationHash>,
    pub current_catalogue_sha256: String,
    pub current_catalogue_version: Option<String>,
    pub current_machine_shortname: String,
    pub current_arcade_root: PathBuf,
    pub executable: Option<PathBuf>,
    pub timeout: Duration,
    pub missing_dependencies: Vec<String>,
    pub bad_dump_dependencies: Vec<String>,
    pub no_dump_present: bool,
}

impl MameSetIdentityReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalogue_sha256: impl Into<String>,
        catalogue_version: Option<String>,
        machine_shortname: impl Into<String>,
        parent_shortname: Option<String>,
        arcade_root: PathBuf,
        evidence_generation: impl Into<String>,
        dependency_topology_digest: impl Into<String>,
        captured_at_unix: u64,
    ) -> Self {
        Self {
            schema_version: MAME_POST_REPAIR_SCHEMA_VERSION,
            catalogue_sha256: catalogue_sha256.into(),
            catalogue_version,
            machine_shortname: machine_shortname.into(),
            parent_shortname,
            arcade_root,
            evidence_generation: evidence_generation.into(),
            dependency_topology_digest: dependency_topology_digest.into(),
            captured_at_unix,
        }
    }
}

pub fn verify_mame_repair(request: &MamePostRepairVerificationRequest) -> MameRepairVerification {
    let mut result = base_result(request);
    if request.identity.catalogue_sha256 != request.current_catalogue_sha256 {
        result
            .issues
            .push(MameRepairVerificationIssue::CatalogueChanged);
    }
    if request.identity.catalogue_version != request.current_catalogue_version {
        result
            .issues
            .push(MameRepairVerificationIssue::CatalogueVersionChanged);
    }
    if request.identity.machine_shortname != request.current_machine_shortname {
        result
            .issues
            .push(MameRepairVerificationIssue::SetShortnameChanged);
    }
    if request.identity.arcade_root != request.current_arcade_root {
        result
            .issues
            .push(MameRepairVerificationIssue::ArcadeRootChanged);
    }
    if !result.issues.is_empty() {
        result.state = MameRepairVerificationState::StaleEvidence;
        return result;
    }

    for expected in &request.expected_destinations {
        match verify_destination(&request.identity.arcade_root, expected) {
            Ok(()) => result.destination_hashes_verified.push(expected.clone()),
            Err(issue) => result.issues.push(issue),
        }
    }
    if !result.issues.is_empty() {
        result.state = MameRepairVerificationState::StaleEvidence;
        return result;
    }

    if !request.missing_dependencies.is_empty() {
        result.remaining_missing_dependencies = request.missing_dependencies.clone();
    }
    if !request.bad_dump_dependencies.is_empty() {
        result.remaining_bad_dependencies = request.bad_dump_dependencies.clone();
    }
    result.bad_dump_present = !result.remaining_bad_dependencies.is_empty();
    result.no_dump_present = request.no_dump_present;

    let Some(executable) = request.executable.as_deref() else {
        result.targeted_mame = Some(unavailable_targeted(request));
        result.state = MameRepairVerificationState::VerificationUnavailable;
        result
            .issues
            .push(MameRepairVerificationIssue::MameUnavailable);
        return result;
    };
    let targeted = run_targeted_mame(executable, &request.identity, request.timeout);
    result.targeted_mame = Some(targeted.clone());
    result.state = final_state(&targeted, &result);
    append_targeted_issues(&mut result, &targeted);
    result
}

fn base_result(request: &MamePostRepairVerificationRequest) -> MameRepairVerification {
    MameRepairVerification {
        schema_version: MAME_POST_REPAIR_SCHEMA_VERSION,
        identity: request.identity.clone(),
        transaction_id: request.transaction_id.clone(),
        repair_plan_digest: request.repair_plan_digest.clone(),
        destination_hashes_verified: Vec::new(),
        targeted_mame: None,
        remaining_missing_dependencies: Vec::new(),
        remaining_bad_dependencies: Vec::new(),
        bad_dump_present: false,
        no_dump_present: request.no_dump_present,
        state: MameRepairVerificationState::VerificationFailed,
        issues: Vec::new(),
    }
}

fn verify_destination(
    arcade_root: &Path,
    expected: &MameRepairDestinationHash,
) -> Result<(), MameRepairVerificationIssue> {
    let root = fs::canonicalize(arcade_root)
        .map_err(|_| MameRepairVerificationIssue::DestinationUnsafe(arcade_root.to_path_buf()))?;
    let metadata = fs::symlink_metadata(&expected.path)
        .map_err(|_| MameRepairVerificationIssue::DestinationMissing(expected.path.clone()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(MameRepairVerificationIssue::DestinationUnsafe(
            expected.path.clone(),
        ));
    }
    let parent = expected
        .path
        .parent()
        .ok_or_else(|| MameRepairVerificationIssue::DestinationUnsafe(expected.path.clone()))?;
    if !fs::canonicalize(parent)
        .map(|path| path.starts_with(&root))
        .unwrap_or(false)
    {
        return Err(MameRepairVerificationIssue::DestinationUnsafe(
            expected.path.clone(),
        ));
    }
    let mut file = fs::File::open(&expected.path)
        .map_err(|_| MameRepairVerificationIssue::DestinationMissing(expected.path.clone()))?;
    let mut hasher = Sha1::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| MameRepairVerificationIssue::DestinationChanged(expected.path.clone()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual = hex(&hasher.finalize());
    if actual != expected.expected_sha1.to_ascii_lowercase() {
        return Err(MameRepairVerificationIssue::DestinationChanged(
            expected.path.clone(),
        ));
    }
    Ok(())
}

fn run_targeted_mame(
    executable: &Path,
    identity: &MameSetIdentityReceipt,
    timeout: Duration,
) -> MameTargetedVerification {
    let args = vec![
        OsString::from("-noreadconfig"),
        OsString::from("-nowriteconfig"),
        OsString::from("-rompath"),
        identity.arcade_root.as_os_str().to_os_string(),
        OsString::from("-verifyroms"),
        OsString::from(&identity.machine_shortname),
    ];
    let argv = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let mut child = match Command::new(executable)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return MameTargetedVerification {
                state: MameTargetedVerificationState::Unavailable,
                argv,
                exit_code: None,
                bounded_output: String::new(),
            };
        }
    };
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let stdout_thread = thread::spawn(|| bounded_read(stdout));
    let stderr_thread = thread::spawn(|| bounded_read(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => break None,
        }
    };
    let mut output = stdout_thread.join().unwrap_or_default();
    output.extend(stderr_thread.join().unwrap_or_default());
    let output = String::from_utf8_lossy(&output)
        .chars()
        .take(MAME_VERIFY_OUTPUT_LIMIT)
        .collect();
    let Some(status) = status else {
        return MameTargetedVerification {
            state: MameTargetedVerificationState::Timeout,
            argv,
            exit_code: None,
            bounded_output: output,
        };
    };
    let lower = output.to_ascii_lowercase();
    let set = identity.machine_shortname.to_ascii_lowercase();
    let good = lower.contains(&format!("romset {set} is good"))
        || lower.contains(&format!("romset {set} is ok"));
    let state = if status.success() && good {
        MameTargetedVerificationState::Passed
    } else if lower.contains("no dump") || lower.contains("nodump") {
        MameTargetedVerificationState::NoDump
    } else if lower.contains("bad dump") || lower.contains("baddump") {
        MameTargetedVerificationState::BadDump
    } else if lower.contains(" is missing") || lower.contains("not found") {
        MameTargetedVerificationState::MissingDependencies
    } else if !status.success() {
        MameTargetedVerificationState::NonZeroExit
    } else {
        MameTargetedVerificationState::UnexpectedOutput
    };
    MameTargetedVerification {
        state,
        argv,
        exit_code: status.code(),
        bounded_output: output,
    }
}

fn bounded_read(reader: impl Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    reader
        .take((MAME_VERIFY_OUTPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .ok();
    bytes.truncate(MAME_VERIFY_OUTPUT_LIMIT);
    bytes
}

fn unavailable_targeted(request: &MamePostRepairVerificationRequest) -> MameTargetedVerification {
    MameTargetedVerification {
        state: MameTargetedVerificationState::Unavailable,
        argv: vec![
            "mame executable unavailable".into(),
            request.identity.machine_shortname.clone(),
        ],
        exit_code: None,
        bounded_output: String::new(),
    }
}

fn final_state(
    targeted: &MameTargetedVerification,
    result: &MameRepairVerification,
) -> MameRepairVerificationState {
    if targeted.state == MameTargetedVerificationState::NoDump || result.no_dump_present {
        return MameRepairVerificationState::VerifiedBestAvailable;
    }
    if targeted.state == MameTargetedVerificationState::BadDump || result.bad_dump_present {
        return MameRepairVerificationState::NeedsRedump;
    }
    if targeted.state == MameTargetedVerificationState::MissingDependencies
        || !result.remaining_missing_dependencies.is_empty()
    {
        return MameRepairVerificationState::StillIncomplete;
    }
    if targeted.state == MameTargetedVerificationState::Passed {
        return MameRepairVerificationState::VerifiedComplete;
    }
    MameRepairVerificationState::VerificationFailed
}

fn append_targeted_issues(
    result: &mut MameRepairVerification,
    targeted: &MameTargetedVerification,
) {
    match targeted.state {
        MameTargetedVerificationState::MissingDependencies => {
            result
                .issues
                .push(MameRepairVerificationIssue::MissingDependency(
                    targeted.bounded_output.clone(),
                ))
        }
        MameTargetedVerificationState::BadDump => {
            result
                .issues
                .push(MameRepairVerificationIssue::BadDumpDependency(
                    targeted.bounded_output.clone(),
                ))
        }
        MameTargetedVerificationState::NoDump => {
            result
                .issues
                .push(MameRepairVerificationIssue::NoDumpDependency(
                    targeted.bounded_output.clone(),
                ))
        }
        MameTargetedVerificationState::Timeout => {
            result.issues.push(MameRepairVerificationIssue::Timeout)
        }
        MameTargetedVerificationState::NonZeroExit => result.issues.push(
            MameRepairVerificationIssue::NonZeroExit(targeted.exit_code.unwrap_or(-1)),
        ),
        MameTargetedVerificationState::UnexpectedOutput => result
            .issues
            .push(MameRepairVerificationIssue::UnexpectedOutput),
        MameTargetedVerificationState::Passed | MameTargetedVerificationState::Unavailable => {}
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn request(root: &Path, executable: Option<PathBuf>) -> MamePostRepairVerificationRequest {
        let file = root.join("pacman").join("rom.bin");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"verified").unwrap();
        let mut hash = Sha1::new();
        hash.update(b"verified");
        MamePostRepairVerificationRequest {
            identity: MameSetIdentityReceipt::new(
                "catalogue",
                Some("0.280".into()),
                "pacman",
                Some("puckman".into()),
                root.to_path_buf(),
                "generation-1",
                "topology-1",
                1,
            ),
            transaction_id: "tx-1".into(),
            repair_plan_digest: "plan-1".into(),
            expected_destinations: vec![MameRepairDestinationHash {
                path: file,
                expected_sha1: hex(&hash.finalize()),
            }],
            current_catalogue_sha256: "catalogue".into(),
            current_catalogue_version: Some("0.280".into()),
            current_machine_shortname: "pacman".into(),
            current_arcade_root: root.to_path_buf(),
            executable,
            timeout: MAME_VERIFY_TIMEOUT,
            missing_dependencies: Vec::new(),
            bad_dump_dependencies: Vec::new(),
            no_dump_present: false,
        }
    }

    #[test]
    fn unavailable_mame_keeps_destination_verification() {
        let dir = tempdir().unwrap();
        let result = verify_mame_repair(&request(dir.path(), None));
        assert_eq!(result.destination_hashes_verified.len(), 1);
        assert_eq!(
            result.state,
            MameRepairVerificationState::VerificationUnavailable
        );
    }

    #[test]
    fn fake_mame_receives_only_one_explicit_set_and_passes() {
        let dir = tempdir().unwrap();
        let executable = dir.path().join("mame");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'romset pacman is good\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let result = verify_mame_repair(&request(dir.path(), Some(executable)));
        assert_eq!(result.state, MameRepairVerificationState::VerifiedComplete);
        let argv = &result.targeted_mame.unwrap().argv;
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["-verifyroms", "pacman"])
        );
    }

    #[test]
    fn missing_output_is_incomplete_and_wrong_set_is_not_a_pass() {
        let dir = tempdir().unwrap();
        let executable = dir.path().join("mame");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'romset pacman is missing\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let result = verify_mame_repair(&request(dir.path(), Some(executable)));
        assert_eq!(result.state, MameRepairVerificationState::StillIncomplete);

        let executable = dir.path().join("wrong-mame");
        fs::write(&executable, "#!/bin/sh\nprintf 'romset other is good\\n'\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let result = verify_mame_repair(&request(dir.path(), Some(executable)));
        assert_eq!(
            result.state,
            MameRepairVerificationState::VerificationFailed
        );
    }

    #[test]
    fn nonzero_exit_and_timeout_are_not_verified() {
        let dir = tempdir().unwrap();
        let executable = dir.path().join("nonzero-mame");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'romset pacman is good\\n'\nexit 2\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let result = verify_mame_repair(&request(dir.path(), Some(executable)));
        assert_eq!(
            result.targeted_mame.unwrap().state,
            MameTargetedVerificationState::NonZeroExit
        );

        let executable = dir.path().join("timeout-mame");
        fs::write(&executable, "#!/bin/sh\nsleep 1\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let mut request = request(dir.path(), Some(executable));
        request.timeout = Duration::from_millis(1);
        let result = verify_mame_repair(&request);
        assert_eq!(
            result.targeted_mame.unwrap().state,
            MameTargetedVerificationState::Timeout
        );
    }

    #[test]
    fn stale_catalogue_refuses_spawn() {
        let dir = tempdir().unwrap();
        let result = verify_mame_repair(&MamePostRepairVerificationRequest {
            current_catalogue_sha256: "changed".into(),
            ..request(dir.path(), None)
        });
        assert_eq!(result.state, MameRepairVerificationState::StaleEvidence);
        assert!(result.targeted_mame.is_none());
    }

    #[test]
    fn bad_and_no_dump_remain_distinct() {
        let dir = tempdir().unwrap();
        let mut bad = request(dir.path(), None);
        bad.bad_dump_dependencies.push("bad.bin".into());
        let bad_result = verify_mame_repair(&bad);
        assert!(bad_result.bad_dump_present);
        assert_eq!(
            bad_result.state,
            MameRepairVerificationState::VerificationUnavailable
        );
        let mut nodump = request(dir.path(), None);
        nodump.no_dump_present = true;
        let nodump_result = verify_mame_repair(&nodump);
        assert!(nodump_result.no_dump_present);
        assert_eq!(
            nodump_result.state,
            MameRepairVerificationState::VerificationUnavailable
        );
    }
}
