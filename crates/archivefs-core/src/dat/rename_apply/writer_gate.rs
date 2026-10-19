//! Mixed-version safety gate for journal-writing work.
//!
//! A build that seals its journals must not create, rewrite, seal or undo any
//! journal while an older EmuWiz that knows nothing about seals is running as
//! the same user: that build could rewrite recovery state behind the seals.
//!
//! Every sealing-capable process registers itself in `sealed-writers/` in the
//! data directory (pid plus kernel start time, so a reused pid never counts).
//! Before any journal write or Undo, the running processes of this user are
//! inspected (read-only, `/proc`). An EmuWiz executable that is not a
//! registered sealing writer is an older writer and closes the gate.
//!
//! Test binaries point [`PROC_ROOT_ENV`] at an empty private directory before
//! `main`, so tests never inspect real processes; gate tests supply a fake
//! process table there.

use std::path::{Path, PathBuf};

/// Alternate process-table root (default `/proc`). Set by test isolation.
pub const PROC_ROOT_ENV: &str = "EMUWIZ_WRITER_GATE_PROC_ROOT";
/// Folder (in the data directory) holding sealing-writer registrations.
pub const SEALED_WRITERS_DIR: &str = "sealed-writers";
/// Prefix of the refusal while an older writer is running.
pub const OLDER_WRITER_RUNNING: &str = "an older EmuWiz is running";

/// Executable names of EmuWiz builds that can write journals.
const EMUWIZ_EXECUTABLES: &[&str] = &[
    "emuwiz",
    "emuwiz-gui",
    "emuwiz-v2",
    "archivefs-gui",
    "archivefs",
    "emuwiz-cli",
    "archivefs-cli",
    "emuwiz-r2",
];

/// The protocol a registration declares. Only builds that seal their journals
/// write it.
const PROTOCOL: &str = "sealed-journal-v1";

/// One running process that may write journals without seals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OlderWriter {
    pub pid: u32,
    pub executable: PathBuf,
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static PROC_ROOT_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Test support: a fake process table for the current thread only.
#[cfg(any(test, feature = "test-support"))]
pub fn set_process_root_for_this_thread(root: Option<PathBuf>) {
    PROC_ROOT_OVERRIDE.with(|cell| *cell.borrow_mut() = root);
}

/// Whether this thread uses a test process table (results are then never
/// cached across threads).
pub fn process_root_overridden() -> bool {
    #[cfg(any(test, feature = "test-support"))]
    {
        PROC_ROOT_OVERRIDE.with(|cell| cell.borrow().is_some())
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        false
    }
}

fn proc_root() -> PathBuf {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(root) = PROC_ROOT_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return root;
    }
    std::env::var_os(PROC_ROOT_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/proc"))
}

fn registrations_dir() -> Result<PathBuf, String> {
    // A test's fake process table carries its own registry, so parallel tests
    // never see or sweep each other's records.
    #[cfg(any(test, feature = "test-support"))]
    if let Some(root) = PROC_ROOT_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return Ok(root.join(".sealed-writers"));
    }
    crate::app_dirs::data_path(SEALED_WRITERS_DIR)
        .map_err(|error| format!("cannot resolve the sealed-writer registry: {error}"))
}

/// Kernel start time of a process (field 22 of `stat`), which distinguishes a
/// reused pid from the process that registered.
fn start_time(root: &Path, pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    // The command name is parenthesised and may contain spaces.
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(19).map(str::to_owned)
}

#[cfg(unix)]
fn same_user(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let own = unsafe { libc::geteuid() };
    std::fs::metadata(path).is_ok_and(|metadata| metadata.uid() == own)
}

#[cfg(not(unix))]
fn same_user(_path: &Path) -> bool {
    true
}

/// The device and inode of a process's running executable.
fn executable_identity(root: &Path, pid: u32) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(root.join(pid.to_string()).join("exe")).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

fn registration_text(started: &str, (dev, ino): (u64, u64)) -> String {
    format!("protocol={PROTOCOL}\nstart={started}\nexe_dev={dev}\nexe_ino={ino}\n")
}

/// Registers this process as a sealing writer, atomically (a private file
/// renamed into place). Idempotent.
fn register_self() -> Result<(), String> {
    let pid = std::process::id();
    let real = Path::new("/proc");
    let (Some(started), Some(identity)) = (start_time(real, pid), executable_identity(real, pid))
    else {
        return Err("cannot identify this process for writer registration".into());
    };
    let text = registration_text(&started, identity);
    let dir = registrations_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create the sealed-writer registry: {error}"))?;
    let path = dir.join(pid.to_string());
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    static ATTEMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let attempt = ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let staging = dir.join(format!(".{pid}.{attempt}.staging"));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&staging, &path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("cannot register this sealing writer: {error}"));
    }
    Ok(())
}

/// Held by a running sealing-capable program; removes its registration when
/// the program ends normally. A crashed program's record is recognised as
/// stale (another start time or no such process) and removed later.
pub struct Registration {
    path: PathBuf,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Registers this program at startup, before any Apply or Undo is admitted,
/// once it has confirmed it can run the sealed-journal protocol (its seal
/// store opens and is prepared).
pub fn register_at_startup() -> Result<Registration, String> {
    super::seal::self_check()?;
    register_self()?;
    Ok(Registration {
        path: registrations_dir()?.join(std::process::id().to_string()),
    })
}

/// A registration is honoured only while it declares the sealed protocol and
/// matches the live process: same kernel start time and the same running
/// executable. A marker without those facts, a reused pid or a different
/// program is not a sealing writer. (A hostile process of the same user can
/// still forge one; it could equally edit the journals and seals themselves.)
fn registered(root: &Path, pid: u32) -> bool {
    let Ok(dir) = registrations_dir() else {
        return false;
    };
    let path = dir.join(pid.to_string());
    let plain =
        std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file());
    let (Some(started), Some(identity)) = (start_time(root, pid), executable_identity(root, pid))
    else {
        return false;
    };
    let honoured = plain
        && same_user(&path)
        && std::fs::read_to_string(&path).ok().as_deref()
            == Some(registration_text(&started, identity).as_str());
    if !honoured && plain {
        // Stale (crashed or reused pid) or not a real registration.
        let _ = std::fs::remove_file(&path);
    }
    honoured
}

/// Removes registrations whose process no longer exists.
fn sweep_stale(root: &Path) {
    let Ok(dir) = registrations_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != std::process::id() && !root.join(pid.to_string()).exists() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Running EmuWiz processes of this user that are not registered sealing
/// writers. Read-only.
pub fn older_writers() -> Vec<OlderWriter> {
    let root = proc_root();
    let own = std::process::id();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own || !same_user(&entry.path()) {
            continue;
        }
        let Ok(executable) = std::fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        let name = executable
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .trim_end_matches(" (deleted)");
        if !EMUWIZ_EXECUTABLES.contains(&name) {
            continue;
        }
        if registered(&root, pid) {
            continue;
        }
        found.push(OlderWriter { pid, executable });
    }
    found.sort_by_key(|writer| writer.pid);
    found
}

/// Opens the gate for journal work, or explains which older EmuWiz must be
/// closed first. Registers this process as a sealing writer when open.
pub fn check() -> Result<(), String> {
    static OPEN_UNTIL: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let now = std::time::Instant::now();
    let cacheable = !process_root_overridden();
    if cacheable
        && OPEN_UNTIL
            .lock()
            .ok()
            .and_then(|guard| *guard)
            .is_some_and(|until| now < until)
    {
        return Ok(());
    }
    sweep_stale(&proc_root());
    let older = older_writers();
    if older.is_empty() {
        register_self()?;
        // An open gate is re-inspected at most every two seconds; a closed
        // one is never cached.
        if cacheable && let Ok(mut guard) = OPEN_UNTIL.lock() {
            *guard = Some(now + std::time::Duration::from_secs(2));
        }
        return Ok(());
    }
    let list = older
        .iter()
        .map(|writer| format!("{} (PID {})", writer.executable.display(), writer.pid))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "{OLDER_WRITER_RUNNING}: {list}. It does not seal its Undo records and could rewrite \
         them, so organising, renaming and Undo are disabled here until it is closed. Nothing was \
         changed"
    ))
}

/// Whether an error is the older-writer refusal.
pub fn is_older_writer_refusal(error: &str) -> bool {
    error.contains(OLDER_WRITER_RUNNING)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fake process table with one process of the given executable name.
    pub(crate) fn fake_table(name: &str, pid: u32, started: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        add_process(root.path(), name, pid, started);
        root
    }

    /// Adds a process whose executable is a real file inside the table.
    pub(crate) fn add_process(root: &Path, name: &str, pid: u32, started: &str) {
        let process = root.join(pid.to_string());
        std::fs::create_dir_all(&process).unwrap();
        let binary = root.join("bin").join(format!("{pid}")).join(name);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"binary").unwrap();
        std::os::unix::fs::symlink(&binary, process.join("exe")).unwrap();
        std::fs::write(
            process.join("stat"),
            format!("{pid} ({name}) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {started} 0 0"),
        )
        .unwrap();
    }

    /// Writes the registration a sealing-capable instance would write.
    fn register(root: &Path, pid: u32) {
        let text = registration_text(
            &start_time(root, pid).unwrap(),
            executable_identity(root, pid).unwrap(),
        );
        let registry = registrations_dir().unwrap();
        std::fs::create_dir_all(&registry).unwrap();
        std::fs::write(registry.join(pid.to_string()), text).unwrap();
    }

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            set_process_root_for_this_thread(None);
        }
    }

    #[test]
    fn an_unregistered_older_emuwiz_closes_the_gate() {
        for (index, name) in EMUWIZ_EXECUTABLES.iter().enumerate() {
            let pid = 4_000_001 + index as u32 * 10;
            let table = fake_table(name, pid, "777");
            set_process_root_for_this_thread(Some(table.path().to_path_buf()));
            let _reset = Reset;
            assert_eq!(older_writers().len(), 1, "{name}");
            let refusal = check().unwrap_err();
            assert!(is_older_writer_refusal(&refusal), "{refusal}");
            assert!(refusal.contains(&format!("PID {pid}")), "{refusal}");
        }
    }

    #[test]
    fn two_new_instances_and_gui_with_cli_do_not_block_each_other() {
        for (first, second) in [("emuwiz", "emuwiz-gui"), ("emuwiz", "emuwiz-cli")] {
            let table = tempfile::tempdir().unwrap();
            add_process(table.path(), first, 4_100_001, "11");
            add_process(table.path(), second, 4_100_002, "12");
            set_process_root_for_this_thread(Some(table.path().to_path_buf()));
            let _reset = Reset;
            register(table.path(), 4_100_001);
            register(table.path(), 4_100_002);
            assert!(older_writers().is_empty(), "{first} + {second}");
            assert!(check().is_ok());
        }
    }

    #[test]
    fn an_old_instance_beside_a_new_one_still_closes_the_gate() {
        let table = tempfile::tempdir().unwrap();
        add_process(table.path(), "emuwiz", 4_200_001, "21");
        add_process(table.path(), "emuwiz", 4_200_002, "22");
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        register(table.path(), 4_200_001);
        let older = older_writers();
        assert_eq!(
            older.iter().map(|w| w.pid).collect::<Vec<_>>(),
            vec![4_200_002]
        );
        assert!(is_older_writer_refusal(&check().unwrap_err()));
    }

    #[test]
    fn stale_reused_and_forged_registrations_are_not_honoured_and_are_cleaned() {
        let table = tempfile::tempdir().unwrap();
        add_process(table.path(), "emuwiz", 4_300_001, "31");
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        let registry = registrations_dir().unwrap();
        std::fs::create_dir_all(&registry).unwrap();
        let record = registry.join("4300001");
        // A bare marker (the earlier format) is not a registration.
        std::fs::write(&record, "31").unwrap();
        assert_eq!(older_writers().len(), 1);
        assert!(!record.exists(), "a non-registration is removed");
        // A crashed process's record reused by another start time.
        register(table.path(), 4_300_001);
        std::fs::write(
            table.path().join("4300001").join("stat"),
            "4300001 (emuwiz) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 99 0 0",
        )
        .unwrap();
        assert_eq!(older_writers().len(), 1);
        assert!(!record.exists(), "a stale record is removed");
        // A record for a process that no longer exists is swept.
        std::fs::write(registry.join("4300999"), "protocol=sealed-journal-v1").unwrap();
        assert!(check().is_err());
        assert!(!registry.join("4300999").exists());
    }

    #[test]
    fn startup_registration_is_atomic_declares_the_protocol_and_is_released() {
        let empty = tempfile::tempdir().unwrap();
        set_process_root_for_this_thread(Some(empty.path().to_path_buf()));
        let _reset = Reset;
        let path = registrations_dir()
            .unwrap()
            .join(std::process::id().to_string());
        {
            let _registration = register_at_startup().unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.starts_with("protocol=sealed-journal-v1\n"), "{text}");
            assert!(text.contains("exe_ino="), "{text}");
        }
        assert!(!path.exists(), "released when the program ends");
    }
}
