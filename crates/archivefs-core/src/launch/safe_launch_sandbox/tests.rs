use super::*;
use std::fs as host_fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::Instant;
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    manager: SandboxManager,
    source: PathBuf,
}

fn declaration() -> LaunchMediaSafetyDeclaration {
    LaunchMediaSafetyDeclaration {
        safety: LaunchMediaSafety::ScratchCopyWithIsolatedConfig,
        config_isolation: ConfigIsolation::XdgEnvironment,
        persistent_state: PersistentStatePolicy::DisposableSession,
    }
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.d88");
        host_fs::write(&source, b"entirely synthetic disk bytes").unwrap();
        let (manager, _) = SandboxManager::open(temp.path()).unwrap();
        Self {
            temp,
            manager,
            source,
        }
    }
    fn member(&self) -> MediaMember {
        MediaMember {
            source: self.source.clone(),
            role: MediaRole::PrimaryMedia,
            kind: MediaKind::Floppy,
            suffix: "d88".into(),
        }
    }
    fn plan(&self) -> SandboxPlan {
        plan_sandbox(declaration(), &[self.member()], ProfileSeed::Empty).unwrap()
    }
    fn executable(&self, body: &str) -> PathBuf {
        // Test-only emulator stand-in; production never builds shell commands.
        let path = self.temp.path().join("fake-emulator");
        host_fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        host_fs::set_permissions(&path, host_fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn command(&self, prepared: &PreparedSandbox, body: &str) -> PreparedProcessCommand {
        let mut arguments: Vec<OsString> = prepared
            .mappings()
            .iter()
            .filter(|m| m.role != MediaRole::Config)
            .map(|m| m.scratch_path.clone().into_os_string())
            .collect();
        arguments.extend(prepared.config_arguments());
        PreparedProcessCommand {
            executable: self.executable(body),
            arguments,
            working_directory: None,
        }
    }
    fn transactions(&self) -> Vec<OsString> {
        fs::entries(&self.manager.inner.directory, 128)
            .unwrap()
            .into_iter()
            .filter(|n| n.to_str().is_some_and(valid_id))
            .collect()
    }
}

fn wait(process: &mut WatchedProcess) -> &ProcessExitReport {
    let deadline = Instant::now() + Duration::from_secs(10);
    while process.poll().is_none() {
        assert!(Instant::now() < deadline, "test process timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
    process.poll().unwrap()
}

#[test]
fn source_unchanged_scratch_physically_distinct_and_mode_private() {
    let f = Fixture::new();
    let plan = f.plan();
    let before = capture(&f.source, MAX_MEMBER_BYTES).unwrap();
    let prepared = f.manager.prepare(&plan).unwrap();
    let mapping = &prepared.mappings()[0];
    assert_eq!(mapping.original, before);
    assert_ne!(mapping.scratch_path, f.source);
    assert_ne!(
        mapping.scratch_identity.inode,
        before.original_identity.inode
    );
    assert_eq!(host_fs::metadata(&mapping.scratch_path).unwrap().nlink(), 1);
    assert_eq!(
        host_fs::metadata(prepared.workspace_path()).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(
        host_fs::metadata(&mapping.scratch_path).unwrap().mode() & 0o777,
        0o600
    );
    host_fs::write(&mapping.scratch_path, b"emulated writes").unwrap();
    assert_eq!(capture(&f.source, MAX_MEMBER_BYTES).unwrap(), before);
    let path = prepared.workspace_path().to_owned();
    prepared.cleanup().unwrap();
    assert!(!path.exists());
    assert_eq!(capture(&f.source, MAX_MEMBER_BYTES).unwrap(), before);
}

#[test]
fn real_watched_spawn_attaches_only_scratch_and_cleans_on_exit() {
    let f = Fixture::new();
    let plan = f.plan();
    let prepared = f.manager.prepare(&plan).unwrap();
    let workspace = prepared.workspace_path().to_owned();
    let source = capture(&f.source, MAX_MEMBER_BYTES).unwrap();
    let command = f.command(&prepared, "printf changed > \"$1\"\nprintf config > \"$XDG_CONFIG_HOME/new-config\"\nprintf '%s\\n%s\\n%s' \"$1\" \"$HOME\" \"$PWD\" >&2");
    let expected = command.arguments[0].clone();
    let mut process = prepared.spawn(command).unwrap();
    let report = wait(&mut process.process);
    assert!(report.status.as_ref().unwrap().success());
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(stderr.contains(expected.to_str().unwrap()));
    assert!(stderr.contains(workspace.to_str().unwrap()));
    assert!(!stderr.contains(f.source.to_str().unwrap()));
    assert_eq!(process.cleanup_outcome(), CleanupOutcome::Removed);
    assert!(!workspace.exists());
    assert_eq!(capture(&f.source, MAX_MEMBER_BYTES).unwrap(), source);
    assert_eq!(process.original_plan.sources().next(), Some(&source));
}

#[test]
fn modification_between_planning_preparation_and_spawn_fails_closed() {
    let f = Fixture::new();
    let plan = f.plan();
    host_fs::write(&f.source, b"edited source").unwrap();
    assert!(f.manager.prepare(&plan).is_err());
    assert!(f.transactions().is_empty());
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let command = f.command(&prepared, "exit 0");
    host_fs::write(&f.source, b"second source edit").unwrap();
    assert!(prepared.spawn(command).is_err());
    assert!(f.transactions().is_empty());
}

#[test]
fn same_size_change_with_restored_mtime_still_fails_hash_validation() {
    let f = Fixture::new();
    let plan = f.plan();
    let file = File::options().write(true).open(&f.source).unwrap();
    let metadata = file.metadata().unwrap();
    host_fs::write(&f.source, vec![b'x'; metadata.len() as usize]).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert!(matches!(
        f.manager.prepare(&plan),
        Err(SafeLaunchSandboxError::ScratchVerificationFailed {
            reason: VerificationFailureReason::SourceContentDrifted,
            ..
        })
    ));
}

#[test]
fn insufficient_total_space_refuses_before_copying_any_member() {
    let f = Fixture::new();
    let second = f.temp.path().join("two.d88");
    host_fs::write(&second, [9; 128]).unwrap();
    let mut member = f.member();
    member.source = second;
    member.role = MediaRole::SecondaryMedia;
    let plan = plan_sandbox(declaration(), &[f.member(), member], ProfileSeed::Empty).unwrap();
    let required = plan.total_bytes() + 1024 * 1024;
    assert!(
        matches!(f.manager.prepare_with_capacity(&plan, |_| Some(required - 1)), Err(SafeLaunchSandboxError::InsufficientTemporarySpace { required_bytes, .. }) if required_bytes == required)
    );
    assert!(f.transactions().is_empty());
    assert!(f.manager.prepare_with_capacity(&plan, |_| None).is_err());
    assert!(f.transactions().is_empty());
}

#[test]
fn source_symlink_parent_symlink_and_path_traversal_refused() {
    let f = Fixture::new();
    let linked = f.temp.path().join("link.d88");
    symlink(&f.source, &linked).unwrap();
    let parent = f.temp.path().join("linked-parent");
    symlink(f.temp.path(), &parent).unwrap();
    for path in [
        linked,
        parent.join("source.d88"),
        f.temp.path().join("a/../source.d88"),
        PathBuf::from("relative.d88"),
    ] {
        let mut member = f.member();
        member.source = path;
        assert!(plan_sandbox(declaration(), &[member], ProfileSeed::Empty).is_err());
    }
}

#[test]
fn root_symlink_or_insecure_existing_root_refused_without_chmod() {
    let temp = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    symlink(other.path(), temp.path().join("emuwiz")).unwrap();
    assert!(SandboxManager::open(temp.path()).is_err());
    assert!(!other.path().join("launch").exists());
    let temp = tempfile::tempdir().unwrap();
    host_fs::create_dir(temp.path().join("emuwiz")).unwrap();
    host_fs::set_permissions(
        temp.path().join("emuwiz"),
        host_fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert!(SandboxManager::open(temp.path()).is_err());
    assert_eq!(
        host_fs::metadata(temp.path().join("emuwiz"))
            .unwrap()
            .mode()
            & 0o777,
        0o777
    );
}

#[test]
fn complete_multifile_set_or_no_launch_and_no_filename_inheritance() {
    let f = Fixture::new();
    let second = f
        .temp
        .path()
        .join("../../looking-but-safe-title.d88".replace('/', "_"));
    host_fs::write(&second, b"disk two").unwrap();
    let mut member = f.member();
    member.role = MediaRole::SecondaryMedia;
    member.source = second.clone();
    let plan = plan_sandbox(declaration(), &[f.member(), member], ProfileSeed::Empty).unwrap();
    let prepared = f.manager.prepare(&plan).unwrap();
    assert_eq!(prepared.mappings().len(), 2);
    assert_eq!(
        host_fs::read(&prepared.mappings()[1].scratch_path).unwrap(),
        b"disk two"
    );
    assert_eq!(
        prepared.mappings()[1].scratch_path.file_name().unwrap(),
        "secondary-01.d88"
    );
    let mut command = f.command(&prepared, "exit 0");
    command.arguments.pop();
    assert!(prepared.spawn(command).is_err());
    assert!(f.transactions().is_empty());
    host_fs::remove_file(&second).unwrap();
    assert!(f.manager.prepare(&plan).is_err());
    assert!(f.transactions().is_empty());
}

#[test]
fn second_member_changed_after_capacity_check_cleans_partial_copy() {
    let f = Fixture::new();
    let second = f.temp.path().join("second.d88");
    host_fs::write(&second, b"second").unwrap();
    let mut member = f.member();
    member.role = MediaRole::SecondaryMedia;
    member.source = second.clone();
    let plan = plan_sandbox(declaration(), &[f.member(), member], ProfileSeed::Empty).unwrap();
    let result = f.manager.prepare_with_capacity(&plan, |_| {
        host_fs::remove_file(&second).unwrap();
        Some(u64::MAX)
    });
    assert!(result.is_err());
    assert!(f.transactions().is_empty());
    assert_eq!(
        host_fs::read(&f.source).unwrap(),
        b"entirely synthetic disk bytes"
    );
}

#[test]
fn cleanup_refuses_unowned_markers_replaced_roots_and_live_workspaces() {
    let f = Fixture::new();
    let unowned = f.manager.root().join("1-0123456789abcdef");
    host_fs::create_dir(&unowned).unwrap();
    host_fs::write(unowned.join(MARKER), b"").unwrap();
    host_fs::write(unowned.join("keep"), b"foreign").unwrap();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let path = prepared.workspace_path().to_owned();
    assert!(f.manager.startup_cleanup().removed.is_empty());
    assert!(path.exists() && unowned.join("keep").exists());
    host_fs::remove_file(path.join(MARKER)).unwrap();
    assert!(matches!(
        prepared.cleanup(),
        Err(SafeLaunchSandboxError::CleanupFailed { .. })
    ));
    assert!(path.exists() && unowned.join("keep").exists());
}

#[test]
fn startup_cleans_expired_owned_only_and_never_traverses_emulator_symlinks() {
    let f = Fixture::new();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let mut prepared = prepared;
    let mut workspace = prepared.workspace.take().unwrap();
    let path = workspace.path.clone();
    workspace
        .phase(WorkspacePhase::Failed {
            expires: now().saturating_sub(1),
        })
        .unwrap();
    symlink(f.temp.path(), path.join("cache/external-link")).unwrap();
    drop(workspace);
    // A parallel test's fork can inherit the CLOEXEC lease until exec. A
    // sweep MUST skip it while held, then clean it after that transient owner
    // releases it. Do not weaken the production ownership check for a test.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let report = f.manager.startup_cleanup();
        if report.removed == vec![path.clone()] {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "stale cleanup refused: {:?}",
            report.skipped
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(f.source.exists());
}

#[test]
fn marker_copied_to_unowned_root_does_not_grant_cleanup_authority() {
    let f = Fixture::new();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let fake = f.manager.root().join("1-0123456789abcdef");
    host_fs::create_dir(&fake).unwrap();
    host_fs::set_permissions(&fake, host_fs::Permissions::from_mode(0o700)).unwrap();
    host_fs::write(
        fake.join(MARKER),
        host_fs::read(prepared.workspace_path().join(MARKER)).unwrap(),
    )
    .unwrap();
    host_fs::write(fake.join(LEASE), []).unwrap();
    for name in [MARKER, LEASE] {
        host_fs::set_permissions(fake.join(name), host_fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(f.manager.startup_cleanup().removed.is_empty());
    assert!(fake.exists());
}

#[test]
fn config_seed_is_copied_isolated_and_provenance_preserved() {
    let f = Fixture::new();
    let seed = f.temp.path().join("reviewed-profile.cfg");
    host_fs::write(&seed, b"reviewed=yes").unwrap();
    let original = capture(&seed, MAX_CONFIG_BYTES).unwrap();
    let mut decl = declaration();
    decl.config_isolation = ConfigIsolation::Combined { flag: "-config" };
    let plan = plan_sandbox(
        decl,
        &[f.member()],
        ProfileSeed::KnownProfile {
            source: seed.clone(),
            scratch_relative: "test.cfg".into(),
        },
    )
    .unwrap();
    let prepared = f.manager.prepare(&plan).unwrap();
    assert_eq!(prepared.mappings()[1].original, original);
    assert_eq!(prepared.config_arguments()[0], "-config");
    let command = f.command(
        &prepared,
        "printf mutated > \"$3\"\nprintf isolated > \"$HOME/state.txt\"",
    );
    let mut process = prepared.spawn(command).unwrap();
    assert!(
        wait(&mut process.process)
            .status
            .as_ref()
            .unwrap()
            .success()
    );
    assert_eq!(capture(&seed, MAX_CONFIG_BYTES).unwrap(), original);
    assert_eq!(process.cleanup_outcome(), CleanupOutcome::Removed);
    assert!(!f.temp.path().join("state.txt").exists());
}

#[test]
fn config_escape_missing_flag_and_replaced_config_directory_refuse() {
    let f = Fixture::new();
    assert!(
        plan_sandbox(
            declaration(),
            &[f.member()],
            ProfileSeed::KnownProfile {
                source: f.source.clone(),
                scratch_relative: "../outside.cfg".into()
            }
        )
        .is_err()
    );
    let mut decl = declaration();
    decl.config_isolation = ConfigIsolation::Combined { flag: "-config" };
    let prepared = f
        .manager
        .prepare(&plan_sandbox(decl, &[f.member()], ProfileSeed::Empty).unwrap())
        .unwrap();
    let mut command = f.command(&prepared, "exit 0");
    command.arguments.truncate(1);
    assert!(prepared.spawn(command).is_err());
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let config = prepared.workspace_path().join("config");
    host_fs::remove_dir(&config).unwrap();
    symlink(f.temp.path(), &config).unwrap();
    assert!(prepared.revalidate().is_err());
    prepared.cleanup().unwrap();
    assert!(f.source.exists());
}

#[test]
fn scratch_hardlink_and_source_substitution_refused() {
    let f = Fixture::new();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let dest = prepared.mappings()[0].scratch_path.clone();
    host_fs::remove_file(&dest).unwrap();
    host_fs::hard_link(&f.source, &dest).unwrap();
    assert!(prepared.revalidate().is_err());
    prepared.cleanup().unwrap();
    assert_eq!(host_fs::metadata(&f.source).unwrap().nlink(), 1);
}

#[test]
fn oversized_hdd_optical_reference_media_and_undeclared_saves_refused() {
    let f = Fixture::new();
    for kind in [
        MediaKind::HardDisk,
        MediaKind::Optical,
        MediaKind::ReferenceManifest,
    ] {
        let mut member = f.member();
        member.kind = kind;
        assert!(plan_sandbox(declaration(), &[member], ProfileSeed::Empty).is_err());
    }
    for safety in [
        LaunchMediaSafety::UnsafeUnsupported,
        LaunchMediaSafety::DirectReadOnly,
    ] {
        let mut decl = declaration();
        decl.safety = safety;
        assert!(plan_sandbox(decl, &[f.member()], ProfileSeed::Empty).is_err());
    }
    let mut decl = declaration();
    decl.persistent_state = PersistentStatePolicy::NotYetHandled;
    assert!(plan_sandbox(decl, &[f.member()], ProfileSeed::Empty).is_err());
    let file = File::options().write(true).open(&f.source).unwrap();
    file.set_len(MAX_MEMBER_BYTES + 1).unwrap();
    assert!(plan_sandbox(declaration(), &[f.member()], ProfileSeed::Empty).is_err());
    assert!(f.transactions().is_empty());
}

#[test]
fn failed_spawn_removes_workspace_never_falls_back_to_source() {
    let f = Fixture::new();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let mut command = f.command(&prepared, "exit 0");
    command.executable = f.temp.path().join("does-not-exist");
    assert!(matches!(
        prepared.spawn(command),
        Err(SafeLaunchSandboxError::SpawnFailed(_))
    ));
    assert!(f.transactions().is_empty());
    assert_eq!(
        host_fs::read(&f.source).unwrap(),
        b"entirely synthetic disk bytes"
    );
}

#[test]
fn failed_process_retention_is_bounded_and_exposed() {
    let f = Fixture::new();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let path = prepared.workspace_path().to_owned();
    let command = f.command(&prepared, "exit 7");
    let mut process = prepared.spawn(command).unwrap();
    assert_eq!(
        wait(&mut process.process).status.as_ref().unwrap().code(),
        Some(7)
    );
    let CleanupOutcome::Retained {
        path: observed,
        expires_unix,
    } = process.cleanup_outcome()
    else {
        panic!("failure not retained");
    };
    assert_eq!(observed, path);
    assert!(expires_unix >= now() && expires_unix <= now() + FAILED_RETENTION.as_secs());
    assert!(f.manager.startup_cleanup().removed.is_empty());
    // TTL expiration/removal is tested without waiting in the startup test.
}

#[test]
fn transaction_ids_are_bounded_nonsemantic_and_unique() {
    let mut ids = std::collections::BTreeSet::new();
    for _ in 0..128 {
        let id = transaction_id().unwrap();
        assert!(id.len() <= 37 && valid_id(&id));
        assert!(ids.insert(id));
    }
    for id in [
        "../bad",
        "",
        "/tmp",
        "1-0123456789abcdef/child",
        "1-0123456789abcdeg",
    ] {
        assert!(!valid_id(id));
    }
}

#[test]
fn failed_retention_worker_actually_removes_owned_workspace_at_expiry() {
    use std::os::unix::process::ExitStatusExt;
    let f = Fixture::new();
    let mut prepared = f.manager.prepare(&f.plan()).unwrap();
    let path = prepared.workspace_path().to_owned();
    let workspace = prepared.workspace.take().unwrap();
    let outcome = Arc::new(Mutex::new(CleanupOutcome::Running));
    finish_workspace_with_retention(
        workspace,
        &ProcessExitReport {
            status: Ok(std::process::ExitStatus::from_raw(7 << 8)),
            stderr: vec![],
        },
        Arc::clone(&outcome),
        Duration::from_millis(20),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while *outcome.lock().unwrap() != CleanupOutcome::Removed {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!path.exists());
    assert!(f.source.exists());
}

#[test]
fn selection_drop_does_not_remove_running_child_workspace() {
    let f = Fixture::new();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let path = prepared.workspace_path().to_owned();
    let command = f.command(&prepared, "sleep 0.2\nprintf changed > \"$1\"");
    let process = prepared.spawn(command).unwrap();
    drop(process); // simulates UI selection changing while child is running
    assert!(path.exists());
    assert!(f.manager.startup_cleanup().removed.is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    while path.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        host_fs::read(&f.source).unwrap(),
        b"entirely synthetic disk bytes"
    );
}

#[test]
fn startup_never_deletes_possible_live_child_or_unknown_spawn_intent() {
    let f = Fixture::new();
    for phase in [
        WorkspacePhase::Running(std::process::id()),
        WorkspacePhase::SpawnIntent,
    ] {
        // See `prepare_tolerating_transient_busy`'s doc comment: this test
        // is about startup cleanup's live-child/unknown-intent policy, not
        // about lock contention, so it should not fail on the documented-
        // transient `PreparationBusy` signal either.
        let mut prepared = prepare_tolerating_transient_busy(&f.manager, &f.plan()).unwrap();
        let mut workspace = prepared.workspace.take().unwrap();
        let path = workspace.path.clone();
        workspace.marker.created = 0;
        workspace.phase(phase).unwrap();
        drop(workspace); // parent gone, no parent lease
        assert!(f.manager.startup_cleanup().removed.is_empty());
        assert!(path.exists());
    }
}

/// A fixed, always-sufficient capacity oracle for [`SandboxManager::prepare_with_capacity`] -
/// see its use below.
fn ample_capacity(_: &Path) -> Option<u64> {
    Some(16 * 1024 * 1024 * 1024)
}

/// `prepare_with_capacity`'s own comment documents `PreparationBusy` as an
/// expected-transient condition - "tolerate that transient lease, but never
/// wait indefinitely for a genuine owner" - bounded by a 100ms internal
/// retry window per call. Under this crate's real, heavily parallel
/// `cargo test -p archivefs-core --lib` run (thousands of test threads
/// contending for the host's CPUs), a caller's own thread can occasionally
/// be descheduled long enough that a single 100ms window elapses without
/// ever being a genuine second holder - the same class of transient busy
/// signal a real launch caller is expected to retry, not treat as fatal.
/// `genuine_preparation_lock_contention_refuses_without_copying` already
/// covers the real, deliberate-contention refusal path directly; a test
/// whose actual claim is about the workspace-count admission bound (not
/// about lock contention) should not fail on this unrelated, documented-
/// transient signal. Retrying here does not hide any other error variant -
/// only `PreparationBusy` is retried, and only up to a bounded deadline.
fn prepare_tolerating_transient_busy(
    manager: &SandboxManager,
    plan: &SandboxPlan,
) -> Result<PreparedSandbox, SafeLaunchSandboxError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match manager.prepare_with_capacity(plan, ample_capacity) {
            Err(SafeLaunchSandboxError::PreparationBusy { .. }) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

#[test]
fn transaction_admission_bound_and_duplicate_media_refuse() {
    let f = Fixture::new();
    let mut duplicate = f.member();
    duplicate.role = MediaRole::SecondaryMedia;
    assert!(plan_sandbox(declaration(), &[f.member(), duplicate], ProfileSeed::Empty).is_err());
    let plan = f.plan();
    // This test proves the MAX_WORKSPACES admission bound specifically, not
    // "prepare fails for some reason or other" - so it must not depend on
    // `prepare`'s real disk-capacity check, which reads the actual host
    // filesystem's free space (`assess_storage`/`statvfs`). Under a large
    // parallel `cargo test` run, other tests concurrently writing to the
    // same shared filesystem transiently shrink that real free-space
    // figure, which could make this loop's (tiny) synthetic plan look
    // capacity-refused well before the 17th call - an environmental flake,
    // not a bound violation. `prepare_with_capacity` with a fixed, always-
    // sufficient oracle isolates the admission-bound assertion from host
    // free space entirely, exactly like `insufficient_total_space_refuses_before_copying_any_member`
    // above already does for the opposite (capacity-refusal) case.
    //
    // Similarly, `prepare_tolerating_transient_busy` absorbs the documented-
    // transient `PreparationBusy` signal (see its doc comment) so this test
    // proves the admission bound itself, not scheduler timing.
    let mut prepared = Vec::new();
    for _ in 0..MAX_WORKSPACES {
        prepared.push(prepare_tolerating_transient_busy(&f.manager, &plan).unwrap());
    }
    assert!(matches!(
        prepare_tolerating_transient_busy(&f.manager, &plan),
        Err(SafeLaunchSandboxError::UnsafeMediaPolicy { .. })
    ));
    assert_eq!(f.transactions().len(), MAX_WORKSPACES);
    drop(prepared);
    assert!(f.transactions().is_empty());
}

#[test]
fn genuine_preparation_lock_contention_refuses_without_copying() {
    let f = Fixture::new();
    let lock = fs::child(
        &f.manager.inner.directory,
        OsStr::new(".prepare-lock"),
        libc::O_RDWR | libc::O_CREAT,
    )
    .unwrap();
    fs::lock(&lock).unwrap();
    let start = Instant::now();
    assert!(matches!(
        f.manager.prepare(&f.plan()),
        Err(SafeLaunchSandboxError::PreparationBusy { .. })
    ));
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(f.transactions().is_empty());
}

#[test]
fn workspace_replaced_before_cleanup_refuses_unowned_replacement() {
    let f = Fixture::new();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let path = prepared.workspace_path().to_owned();
    let moved = f.temp.path().join("original-owned-workspace");
    host_fs::rename(&path, &moved).unwrap();
    host_fs::create_dir(&path).unwrap();
    host_fs::set_permissions(&path, host_fs::Permissions::from_mode(0o700)).unwrap();
    host_fs::write(path.join("foreign"), b"untouchable").unwrap();
    assert!(prepared.cleanup().is_err());
    assert!(moved.exists() && path.join("foreign").exists());
}

#[test]
fn cleanup_cannot_cross_mountpoints_or_follow_symlinks() {
    let root = File::open("/").unwrap();
    let error = fs::cleanup_directory(&root, OsStr::new("proc")).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EXDEV));
    let f = Fixture::new();
    symlink(f.temp.path(), f.manager.root().join("foreign-link")).unwrap();
    let error =
        fs::cleanup_directory(&f.manager.inner.directory, OsStr::new("foreign-link")).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ELOOP));
}

#[test]
fn ordinary_emulator_created_directories_are_cleaned_without_chmod() {
    let f = Fixture::new();
    let prepared = f.manager.prepare(&f.plan()).unwrap();
    let directory = prepared.workspace_path().join("cache/emulator-generated");
    host_fs::create_dir(&directory).unwrap();
    host_fs::set_permissions(&directory, host_fs::Permissions::from_mode(0o755)).unwrap();
    host_fs::write(directory.join("file"), b"disposable").unwrap();
    prepared.cleanup().unwrap();
    assert!(f.source.exists());
}

#[test]
fn non_opted_in_watched_spawn_retains_argv_environment_and_cwd_behavior() {
    let f = Fixture::new();
    let executable = f.executable("printf '%s\\n%s\\n%s' \"$1\" \"$HOME\" \"$PWD\" >&2");
    let original_home = std::env::var_os("HOME").unwrap_or_default();
    let command = PreparedProcessCommand {
        executable,
        arguments: vec![f.source.clone().into_os_string()],
        working_directory: Some(f.temp.path().to_owned()),
    };
    let mut process = super::super::process_spawn::spawn_watched_process(&command).unwrap();
    let report = wait(&mut process);
    assert!(report.status.as_ref().unwrap().success());
    let expected = format!(
        "{}\n{}\n{}",
        f.source.display(),
        original_home.to_string_lossy(),
        f.temp.path().display()
    );
    assert_eq!(String::from_utf8_lossy(&report.stderr), expected);
    assert!(f.transactions().is_empty());
    assert_eq!(std::env::var_os("HOME").unwrap_or_default(), original_home);
}

fn referenced_plan(f: &Fixture, seed_text: &str) -> (SandboxPlan, PathBuf) {
    let rom = f.temp.path().join("firmware.rom");
    host_fs::write(&rom, b"entirely synthetic firmware").unwrap();
    let seed = f.temp.path().join("seed.cfg");
    host_fs::write(&seed, seed_text).unwrap();
    let plan = plan_sandbox(
        declaration(),
        &[
            f.member(),
            MediaMember {
                source: rom.clone(),
                role: MediaRole::ConfigReferenced,
                kind: MediaKind::Firmware,
                suffix: "rom".into(),
            },
        ],
        ProfileSeed::KnownProfile {
            source: seed,
            scratch_relative: "profile.cfg".into(),
        },
    )
    .unwrap();
    (plan, rom)
}

#[test]
fn config_referenced_firmware_reaches_the_emulator_only_through_the_owned_config() {
    let f = Fixture::new();
    let (plan, rom) = referenced_plan(
        &f,
        "rom = 'media/secondary-01'\nrom_file = media/secondary-01.rom\n",
    );
    let before = capture(&rom, MAX_MEMBER_BYTES).unwrap();
    let prepared = f.manager.prepare(&plan).unwrap();
    // Only the primary medium is an argv token; the ROM copy is read through
    // the workspace-relative path the config names, in the child's cwd.
    let command = PreparedProcessCommand {
        executable: f.executable("test -f media/secondary-01.rom && printf ok >&2"),
        arguments: vec![prepared.mappings()[0].scratch_path.clone().into_os_string()],
        working_directory: None,
    };
    let mut process = prepared.spawn(command).unwrap();
    let report = wait(&mut process.process);
    assert!(report.status.as_ref().unwrap().success());
    assert_eq!(String::from_utf8_lossy(&report.stderr), "ok");
    assert_eq!(process.cleanup_outcome(), CleanupOutcome::Removed);
    assert_eq!(capture(&rom, MAX_MEMBER_BYTES).unwrap(), before);
}

#[test]
fn config_referenced_members_need_a_seed_that_names_them_and_never_appear_in_argv() {
    let f = Fixture::new();
    // No seed at all: refused at planning.
    let rom = f.temp.path().join("firmware.rom");
    host_fs::write(&rom, b"entirely synthetic firmware").unwrap();
    assert!(
        plan_sandbox(
            declaration(),
            &[
                f.member(),
                MediaMember {
                    source: rom,
                    role: MediaRole::ConfigReferenced,
                    kind: MediaKind::Firmware,
                    suffix: "rom".into(),
                },
            ],
            ProfileSeed::Empty,
        )
        .is_err()
    );
    // A seed that does not name the member's scratch copy: refused at spawn,
    // before any process starts, and the workspace is cleaned up.
    let (plan, _rom) = referenced_plan(&f, "rom = 'somewhere/else'\n");
    let prepared = f.manager.prepare(&plan).unwrap();
    let workspace = prepared.workspace_path().to_owned();
    let command = PreparedProcessCommand {
        executable: f.executable("exit 0"),
        arguments: vec![prepared.mappings()[0].scratch_path.clone().into_os_string()],
        working_directory: None,
    };
    assert!(prepared.spawn(command).is_err());
    assert!(!workspace.exists());
    // Naming it in the config AND passing it in argv is refused too: one
    // attachment route per member.
    let (plan, _rom) = referenced_plan(&f, "rom = 'media/secondary-01'\n");
    let prepared = f.manager.prepare(&plan).unwrap();
    let command = PreparedProcessCommand {
        executable: f.executable("exit 0"),
        arguments: prepared
            .mappings()
            .iter()
            .filter(|m| m.role != MediaRole::Config)
            .map(|m| m.scratch_path.clone().into_os_string())
            .collect(),
        working_directory: None,
    };
    assert!(prepared.spawn(command).is_err());
}

#[test]
fn a_config_that_names_an_original_path_is_refused_before_spawn() {
    let f = Fixture::new();
    let (plan, rom) = referenced_plan(
        &f,
        &format!(
            "rom = 'media/secondary-01'\nwritable = {}\n",
            f.source.display()
        ),
    );
    let _ = rom;
    let prepared = f.manager.prepare(&plan).unwrap();
    let command = PreparedProcessCommand {
        executable: f.executable("exit 0"),
        arguments: vec![prepared.mappings()[0].scratch_path.clone().into_os_string()],
        working_directory: None,
    };
    assert!(prepared.spawn(command).is_err());
}
