//! Synthetic Unix fixtures. Hooks inject scheduling and syscall failures only
//! in the unit-test build; exclusive creation still uses the real allocator.

use super::{Event, write};
use std::cell::{Cell, RefCell};
use std::fs;
use std::io::{self, BufRead, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

type Hook = Box<dyn FnMut(Event, &Path) -> io::Result<()>>;
thread_local! {
    static HOOK: RefCell<Option<Hook>> = RefCell::new(None);
    static FORCE_NAME: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn configure_builder(builder: &mut tempfile::Builder<'_, '_>) {
    if FORCE_NAME.get() {
        builder
            .prefix(".archivefs-config-write-forced")
            .rand_bytes(0);
    }
}

pub(super) fn event(event: Event, path: &Path) -> io::Result<()> {
    HOOK.with_borrow_mut(|hook| match hook {
        Some(hook) => hook(event, path),
        None => Ok(()),
    })
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        HOOK.with_borrow_mut(|hook| *hook = None);
        FORCE_NAME.set(false);
    }
}

fn install_hook(hook: impl FnMut(Event, &Path) -> io::Result<()> + 'static) -> Reset {
    HOOK.with_borrow_mut(|slot| {
        assert!(slot.is_none());
        *slot = Some(Box::new(hook));
    });
    Reset
}

fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("emuwiz-owned-stage-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    dev: u64,
    ino: u64,
    links: u64,
    mode: u32,
    bytes: Option<Vec<u8>>,
    link: Option<PathBuf>,
}

fn snapshot(path: &Path) -> Snapshot {
    let meta = fs::symlink_metadata(path).unwrap();
    Snapshot {
        dev: meta.dev(),
        ino: meta.ino(),
        links: meta.nlink(),
        mode: meta.mode(),
        bytes: meta.is_file().then(|| fs::read(path).unwrap()),
        link: meta
            .file_type()
            .is_symlink()
            .then(|| fs::read_link(path).unwrap()),
    }
}

fn stages(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".archivefs-config-write-")
        })
        .collect()
}

fn collision(kind: &str) {
    let root = fixture();
    let target = root.path().join("destination");
    let foreign = root.path().join(".archivefs-config-write-forced.tmp");
    let unrelated = root.path().join("unrelated");
    fs::write(&target, "old destination").unwrap();
    fs::write(&unrelated, "unrelated bytes").unwrap();
    match kind {
        "regular" => fs::write(&foreign, "foreign bytes").unwrap(),
        "symlink" => symlink(&unrelated, &foreign).unwrap(),
        "hardlink" => fs::hard_link(&unrelated, &foreign).unwrap(),
        "directory" => fs::create_dir(&foreign).unwrap(),
        _ => unreachable!(),
    }
    let before = (snapshot(&target), snapshot(&foreign), snapshot(&unrelated));
    let _reset = Reset;
    FORCE_NAME.set(true);
    let error = write(&target, "replacement").unwrap_err();
    assert!(
        matches!(error, crate::ArchiveFsError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists)
    );
    assert_eq!(
        before,
        (snapshot(&target), snapshot(&foreign), snapshot(&unrelated))
    );
}

#[test]
fn exclusive_creation_preserves_regular_collision() {
    collision("regular");
}
#[test]
fn exclusive_creation_preserves_symlink_collision() {
    collision("symlink");
}
#[test]
fn exclusive_creation_preserves_hardlink_collision() {
    collision("hardlink");
}
#[test]
fn exclusive_creation_preserves_directory_collision() {
    collision("directory");
}

fn substitution(kind: &'static str, event_to_swap: Event) {
    let root = fixture();
    let target = root.path().join("destination");
    let unrelated = root.path().join("unrelated");
    let retained = root.path().join("retained-owned-stage");
    fs::write(&target, "old destination").unwrap();
    fs::write(&unrelated, "unrelated bytes").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let before_target = snapshot(&target);
    let expected = std::rc::Rc::new(RefCell::new(None));
    let captured = expected.clone();
    let retained_hook = retained.clone();
    let unrelated_hook = unrelated.clone();
    let mut replaced = false;
    let _hook = install_hook(move |event, stage| {
        if event == event_to_swap && !replaced {
            replaced = true;
            fs::rename(stage, &retained_hook)?;
            match kind {
                "regular" => fs::write(stage, "foreign replacement")?,
                "symlink" => symlink(&unrelated_hook, stage)?,
                "hardlink" => fs::hard_link(&unrelated_hook, stage)?,
                "directory" => fs::create_dir(stage)?,
                _ => unreachable!(),
            }
            *captured.borrow_mut() =
                Some((stage.to_owned(), snapshot(stage), snapshot(&unrelated_hook)));
        }
        Ok(())
    });
    let error = write(&target, "new complete contents")
        .unwrap_err()
        .to_string();
    assert!(error.contains("cleanup refused or failed"), "{error}");
    assert_eq!(snapshot(&target), before_target);
    let expected = expected.borrow();
    let (stage, expected_stage, expected_unrelated) = expected.as_ref().unwrap();
    assert_eq!(&snapshot(stage), expected_stage);
    assert_eq!(&snapshot(&unrelated), expected_unrelated);
    // A swap just before fchmod/write must not redirect either to the name.
    assert_eq!(
        fs::read_to_string(&retained).unwrap(),
        "new complete contents"
    );
    assert_eq!(fs::metadata(&retained).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn substituted_regular_before_publish_is_retained() {
    substitution("regular", Event::BeforePublish);
}
#[test]
fn substituted_symlink_before_publish_is_retained() {
    substitution("symlink", Event::BeforePublish);
}
#[test]
fn substituted_hardlink_before_publish_is_retained() {
    substitution("hardlink", Event::BeforePublish);
}
#[test]
fn substituted_directory_before_publish_is_retained() {
    substitution("directory", Event::BeforePublish);
}
#[test]
fn writes_and_permissions_use_held_handle_after_name_swap() {
    substitution("symlink", Event::BeforePermissions);
}

#[test]
fn added_hardlink_refuses_publication_and_uncertain_cleanup() {
    let root = fixture();
    let target = root.path().join("destination");
    let alias = root.path().join("stage-alias");
    fs::write(&target, "old").unwrap();
    let original = snapshot(&target);
    let hook_alias = alias.clone();
    let _hook = install_hook(move |event, stage| {
        if event == Event::BeforePublish {
            fs::hard_link(stage, &hook_alias)?;
        }
        Ok(())
    });
    assert!(write(&target, "new").is_err());
    assert_eq!(snapshot(&target), original);
    let stage = stages(root.path()).pop().unwrap();
    assert_eq!(snapshot(&stage), snapshot(&alias));
    assert_eq!(fs::metadata(stage).unwrap().nlink(), 2);
}

#[test]
fn cleanup_refuses_a_foreign_entry_substituted_after_write_failure() {
    let root = fixture();
    let target = root.path().join("destination");
    let retained = root.path().join("owned-retained");
    fs::write(&target, "old").unwrap();
    let original = snapshot(&target);
    let _hook = install_hook(move |event, stage| {
        if event == Event::BeforeSync {
            return Err(io::Error::other("injected sync failure"));
        }
        if event == Event::BeforeCleanup {
            fs::rename(stage, &retained)?;
            fs::write(stage, "foreign cleanup sentinel")?;
        }
        Ok(())
    });
    let error = write(&target, "new").unwrap_err().to_string();
    assert!(error.contains("injected sync failure") && error.contains("cleanup refused"));
    assert_eq!(snapshot(&target), original);
    assert_eq!(
        fs::read_to_string(stages(root.path()).pop().unwrap()).unwrap(),
        "foreign cleanup sentinel"
    );
}

fn injected_failure(event_to_fail: Event) {
    let root = fixture();
    let target = root.path().join("destination");
    fs::write(&target, "old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let original = snapshot(&target);
    let _hook = install_hook(move |event, _| {
        if event == event_to_fail {
            Err(io::Error::from_raw_os_error(libc::EACCES))
        } else {
            Ok(())
        }
    });
    assert!(write(&target, "new").is_err());
    assert_eq!(snapshot(&target), original);
    assert!(stages(root.path()).is_empty());
}

#[test]
fn initial_permission_failure_refuses_and_cleans_owned_stage() {
    injected_failure(Event::BeforePermissions);
}
#[test]
fn sync_failure_refuses_and_cleans_owned_stage() {
    injected_failure(Event::BeforeSync);
}

#[test]
fn final_permission_failure_refuses_and_cleans_owned_stage() {
    let root = fixture();
    let target = root.path().join("destination");
    fs::write(&target, "old").unwrap();
    let original = snapshot(&target);
    let permissions_calls = std::rc::Rc::new(Cell::new(0));
    let hook_calls = permissions_calls.clone();
    let _hook = install_hook(move |event, _| {
        if event == Event::BeforePermissions {
            hook_calls.set(hook_calls.get() + 1);
            if hook_calls.get() == 2 {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }
        }
        Ok(())
    });
    assert!(write(&target, "new").is_err());
    assert_eq!(permissions_calls.get(), 2);
    assert_eq!(snapshot(&target), original);
    assert!(stages(root.path()).is_empty());
}

#[test]
fn destination_symlink_refuses_without_staging() {
    let root = fixture();
    let target = root.path().join("destination");
    let unrelated = root.path().join("unrelated");
    fs::write(&unrelated, "unrelated").unwrap();
    symlink(&unrelated, &target).unwrap();
    let before = (snapshot(&target), snapshot(&unrelated));
    assert!(write(&target, "new").is_err());
    assert_eq!(before, (snapshot(&target), snapshot(&unrelated)));
    assert!(stages(root.path()).is_empty());
}

#[test]
fn restricted_destination_stage_is_restricted_from_creation() {
    let root = fixture();
    let target = root.path().join("destination");
    fs::write(&target, "old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let _hook = install_hook(|event, stage| {
        if event == Event::Created {
            assert_eq!(fs::metadata(stage)?.mode() & 0o777, 0o600);
        }
        Ok(())
    });
    write(&target, "new").unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    assert_eq!(fs::metadata(target).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn destination_becoming_symlink_during_file_sync_is_refused() {
    let root = fixture();
    let target = root.path().join("destination");
    let unrelated = root.path().join("unrelated");
    let retained_old = root.path().join("retained-old-destination");
    fs::write(&target, "old destination").unwrap();
    fs::write(&unrelated, "unrelated bytes").unwrap();
    let target_hook = target.clone();
    let unrelated_hook = unrelated.clone();
    let retained_hook = retained_old.clone();
    let _hook = install_hook(move |event, _| {
        if event == Event::BeforeSync {
            fs::rename(&target_hook, &retained_hook)?;
            symlink(&unrelated_hook, &target_hook)?;
        }
        Ok(())
    });
    let result = write(&target, "new complete contents");
    println!(
        "DESTINATION_SYNC_RACE result={result:?}, destination={:?}, unrelated={:?}",
        snapshot(&target),
        snapshot(&unrelated)
    );
    assert!(
        result.is_err(),
        "a destination symlink introduced during sync must be refused"
    );
    assert!(
        fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(target).unwrap(), unrelated);
    assert_eq!(fs::read_to_string(&unrelated).unwrap(), "unrelated bytes");
    assert_eq!(fs::read_to_string(retained_old).unwrap(), "old destination");
    assert!(stages(root.path()).is_empty());
}

#[test]
fn legitimate_hardlinked_destination_replacement_preserves_other_alias() {
    let root = fixture();
    let target = root.path().join("destination");
    let alias = root.path().join("old-alias");
    fs::write(&target, "old").unwrap();
    fs::hard_link(&target, &alias).unwrap();
    let old_inode = fs::metadata(&alias).unwrap().ino();
    write(&target, "new").unwrap();
    assert_eq!(fs::read_to_string(&alias).unwrap(), "old");
    assert_eq!(fs::metadata(&alias).unwrap().ino(), old_inode);
    assert_eq!(fs::metadata(&alias).unwrap().nlink(), 1);
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    assert_ne!(fs::metadata(&target).unwrap().ino(), old_inode);
    assert!(stages(root.path()).is_empty());
}

#[test]
fn panic_does_not_run_a_pathname_cleanup_destructor() {
    let root = fixture();
    let target = root.path().join("destination");
    fs::write(&target, "old").unwrap();
    let before = snapshot(&target);
    let _hook = install_hook(|event, _| {
        if event == Event::BeforeSync {
            panic!("synthetic interrupted operation");
        }
        Ok(())
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write(&target, "new")));
    assert!(result.is_err());
    assert_eq!(snapshot(&target), before);
    assert_eq!(
        fs::read_to_string(stages(root.path()).pop().unwrap()).unwrap(),
        "new"
    );
}

#[test]
fn real_rename_and_cleanup_permission_failure_retains_owned_stage() {
    assert_ne!(unsafe { libc::geteuid() }, 0);
    let root = fixture();
    let target = root.path().join("destination");
    fs::write(&target, "old").unwrap();
    let before = snapshot(&target);
    let parent = root.path().to_owned();
    let _hook = install_hook(move |event, _| {
        if event == Event::BeforePublish {
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o500))?;
        }
        Ok(())
    });
    let result = write(&target, "new");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let error = result.unwrap_err().to_string();
    assert!(error.contains("cleanup refused or failed"), "{error}");
    assert_eq!(snapshot(&target), before);
    assert_eq!(
        fs::read_to_string(stages(root.path()).pop().unwrap()).unwrap(),
        "new"
    );
}

#[test]
fn unavailable_parent_and_denied_creation_leave_foreign_objects() {
    assert_ne!(unsafe { libc::geteuid() }, 0);
    let root = fixture();
    let foreign = root.path().join("foreign");
    fs::write(&foreign, "foreign").unwrap();
    let before = snapshot(&foreign);
    assert!(write(&foreign.join("not-a-directory"), "new").is_err());
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = write(&root.path().join("destination"), "new");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(snapshot(&foreign), before);
    assert!(stages(root.path()).is_empty());
}

fn child_command(case: &str, root: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "atomic_text::tests::fixture_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("EMUWIZ_ATOMIC_UNIT_CASE", case)
        .env("EMUWIZ_ATOMIC_UNIT_ROOT", root);
    command
}

struct RunningChild(Child);
impl Drop for RunningChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn ready_child(case: &str, root: &Path, body: &str) -> (RunningChild, PathBuf) {
    let mut command = child_command(case, root);
    command
        .env("EMUWIZ_ATOMIC_UNIT_BODY", body)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = RunningChild(command.spawn().unwrap());
    let stdout = child.0.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Some(offset) = line.find("OWNED_STAGE_READY ") {
                let _ = send.send(PathBuf::from(&line[offset + "OWNED_STAGE_READY ".len()..]));
            }
        }
    });
    let stage = receive
        .recv_timeout(Duration::from_secs(20))
        .expect("child did not reach publication gate");
    (child, stage)
}

fn wait_success(child: &mut RunningChild) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "child exited {status}");
            return;
        }
        assert!(std::time::Instant::now() < deadline, "child did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn process_death_before_publication_retains_stage_and_old_destination() {
    let root = fixture();
    let target = root.path().join("destination");
    fs::write(&target, "old").unwrap();
    let before = snapshot(&target);
    let (mut child, stage) = ready_child("gate", root.path(), "complete staged body");
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    assert_eq!(snapshot(&target), before);
    assert_eq!(fs::read_to_string(stage).unwrap(), "complete staged body");
}

#[test]
fn cooperating_processes_have_distinct_stages_and_complete_last_writer_output() {
    let root = fixture();
    let target = root.path().join("destination");
    fs::write(&target, "old").unwrap();
    let (mut left, left_stage) = ready_child("gate", root.path(), "left complete body");
    let (mut right, right_stage) = ready_child("gate", root.path(), "right complete body");
    assert_ne!(left_stage, right_stage);
    assert_ne!(
        fs::metadata(&left_stage).unwrap().ino(),
        fs::metadata(&right_stage).unwrap().ino()
    );
    left.0
        .stdin
        .take()
        .unwrap()
        .write_all(b"publish\n")
        .unwrap();
    right
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(b"publish\n")
        .unwrap();
    wait_success(&mut left);
    wait_success(&mut right);
    let final_body = fs::read_to_string(target).unwrap();
    assert!(final_body == "left complete body" || final_body == "right complete body");
    assert!(stages(root.path()).is_empty());
}

#[test]
fn new_file_umask_and_existing_restricted_mode_are_preserved_in_child_processes() {
    for mask in [0o000, 0o002, 0o022, 0o027, 0o077] {
        let root = fixture();
        let mut command = child_command("mode", root.path());
        command.env("EMUWIZ_ATOMIC_UNIT_MASK", mask.to_string());
        // Only the fresh child changes its umask, before Rust test threads run.
        unsafe {
            command.pre_exec(move || {
                libc::umask(mask);
                Ok(())
            });
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn real_short_write_failure_preserves_destination_and_cleans_owned_stage() {
    let root = fixture();
    let output = child_command("short-write", root.path()).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn fixture_child() {
    let Ok(case) = std::env::var("EMUWIZ_ATOMIC_UNIT_CASE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("EMUWIZ_ATOMIC_UNIT_ROOT").unwrap());
    let target = root.join("destination");
    match case.as_str() {
        "gate" => {
            let _hook = install_hook(|event, stage| {
                if event == Event::BeforePublish {
                    println!("OWNED_STAGE_READY {}", stage.display());
                    io::stdout().flush()?;
                    let mut line = String::new();
                    io::stdin().read_line(&mut line)?;
                    assert_eq!(line, "publish\n");
                }
                Ok(())
            });
            write(&target, &std::env::var("EMUWIZ_ATOMIC_UNIT_BODY").unwrap()).unwrap();
        }
        "mode" => {
            let mask: u32 = std::env::var("EMUWIZ_ATOMIC_UNIT_MASK")
                .unwrap()
                .parse()
                .unwrap();
            #[cfg(target_os = "linux")]
            let before_status = fs::read_to_string("/proc/self/status").unwrap();
            write(&target, "new file").unwrap();
            assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o666 & !mask);
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
            write(&target, "replacement").unwrap();
            assert_eq!(fs::metadata(&target).unwrap().mode() & 0o777, 0o600);
            #[cfg(target_os = "linux")]
            assert_eq!(
                before_status
                    .lines()
                    .find(|line| line.starts_with("Umask:")),
                fs::read_to_string("/proc/self/status")
                    .unwrap()
                    .lines()
                    .find(|line| line.starts_with("Umask:"))
            );
        }
        #[cfg(target_os = "linux")]
        "short-write" => {
            fs::write(&target, "old destination").unwrap();
            let before = snapshot(&target);
            let observed_size = std::rc::Rc::new(Cell::new(None));
            let observed_hook = observed_size.clone();
            let _hook = install_hook(move |event, stage| {
                if event == Event::BeforeCleanup {
                    observed_hook.set(Some(fs::metadata(stage)?.len()));
                }
                Ok(())
            });
            let limit = libc::rlimit {
                rlim_cur: 4,
                rlim_max: 4,
            };
            unsafe {
                assert_ne!(libc::signal(libc::SIGXFSZ, libc::SIG_IGN), libc::SIG_ERR);
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
            }
            let error = write(&target, &"x".repeat(64)).unwrap_err();
            assert!(
                matches!(error, crate::ArchiveFsError::Io { source, .. } if source.raw_os_error() == Some(libc::EFBIG))
            );
            assert_eq!(observed_size.get(), Some(4));
            assert_eq!(snapshot(&target), before);
            assert!(stages(&root).is_empty());
        }
        _ => panic!("unknown synthetic child case"),
    }
}
