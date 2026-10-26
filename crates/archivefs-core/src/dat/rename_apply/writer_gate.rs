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
//! registered sealing writer closes the gate unless verified to use an isolated
//! default GUI profile. Uncertain profile or process evidence always blocks.
//!
//! Test binaries point `PROC_ROOT_ENV` at an empty private directory before
//! `main`, so tests never inspect real processes; gate tests supply a fake
//! process table there. Both hooks exist only in test builds: a production
//! build always inspects the real `/proc`, and a process table it cannot read
//! closes the gate rather than opening it.

use std::path::{Path, PathBuf};

mod profile;

/// Alternate process-table root (default `/proc`). Set by test isolation and
/// compiled only into test builds: an environment variable must never be able
/// to redirect or blind the production inspection.
#[cfg(any(test, feature = "test-support"))]
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
    {
        if let Some(root) = PROC_ROOT_OVERRIDE.with(|cell| cell.borrow().clone()) {
            return root;
        }
        if let Some(root) = std::env::var_os(PROC_ROOT_ENV) {
            return PathBuf::from(root);
        }
    }
    PathBuf::from("/proc")
}

/// The numbered entries of the process table. An unreadable table is an
/// inspection failure, never "no other EmuWiz is running".
fn process_entries(root: &Path) -> Result<std::fs::ReadDir, String> {
    std::fs::read_dir(root).map_err(|error| {
        format!(
            "cannot inspect running processes ({}: {error}), so organising, renaming and Undo \
             are disabled. Nothing was changed",
            root.display()
        )
    })
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

/// Folder (in the data directory) of builds that understand the `Finalising`
/// and `Finalised` journal states. Separate from `sealed-writers/` on purpose:
/// older sealing builds neither read nor write it, so adding it changes nothing
/// about how they and this build share journals today.
pub const FINALISE_WRITERS_DIR: &str = "finalise-capable-writers";
const FINALISE_PROTOCOL: &str = "finalised-state-v1";
/// Prefix of the refusal while a build that cannot read the Finalise states runs.
pub const FINALISE_INCOMPATIBLE_WRITER: &str =
    "a build that cannot read finalised journals is running";

fn finalise_registrations_dir() -> Result<PathBuf, String> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(root) = PROC_ROOT_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return Ok(root.join(".finalise-writers"));
    }
    crate::app_dirs::data_path(FINALISE_WRITERS_DIR)
        .map_err(|error| format!("cannot resolve the finalise-capable registry: {error}"))
}

fn finalise_text(started: &str, (dev, ino): (u64, u64)) -> String {
    format!("protocol={FINALISE_PROTOCOL}\nstart={started}\nexe_dev={dev}\nexe_ino={ino}\n")
}

/// Registers this process as able to read and write the Finalise states.
fn register_finalise_self() -> Result<(), String> {
    let pid = std::process::id();
    let real = Path::new("/proc");
    let (Some(started), Some(identity)) = (start_time(real, pid), executable_identity(real, pid))
    else {
        return Err("cannot identify this process for finalise registration".into());
    };
    let text = finalise_text(&started, identity);
    let dir = finalise_registrations_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create the finalise-capable registry: {error}"))?;
    let path = dir.join(pid.to_string());
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    let staging = dir.join(format!(".{pid}.staging"));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&staging)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&staging, &path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staging);
        return Err(format!(
            "cannot register this finalise-capable writer: {error}"
        ));
    }
    Ok(())
}

fn finalise_capable(root: &Path, pid: u32) -> bool {
    let (Some(started), Some(identity)) = (start_time(root, pid), executable_identity(root, pid))
    else {
        return false;
    };
    let text = finalise_text(&started, identity);
    let in_data_dir = finalise_registrations_dir().is_ok_and(|dir| {
        let path = dir.join(pid.to_string());
        std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file())
            && same_user(&path)
            && std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str())
    });
    in_data_dir || runtime_record_matches(FINALISE_WRITERS_DIR, pid, &text)
}

// --------------------------------------------- per-user runtime registry

/// The per-user runtime registry, beside the data-directory ones. Read-only
/// invocations register only here, so they leave nothing in the data
/// directory, yet sealing writers still recognise them. Its location comes
/// from the kernel uid, never the environment; it is used only when every
/// level is a real directory of this user with no group or other access, and
/// EmuWiz never creates the base itself. Records are the same verified text as
/// in the data directory.
const RUNTIME_REGISTRY_NAME: &str = "emuwiz";

fn private_directory(dir: &Path) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let own = unsafe { libc::geteuid() };
    std::fs::symlink_metadata(dir).is_ok_and(|metadata| {
        metadata.file_type().is_dir()
            && metadata.uid() == own
            && metadata.permissions().mode() & 0o077 == 0
    })
}

/// `base/emuwiz/<kind>`, created owner-only when `create`; `None` unless every
/// level (including the existing `base`) is private to this user.
fn runtime_registry_in(base: &Path, kind: &str, create: bool) -> Option<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    if !private_directory(base) {
        return None;
    }
    let mut dir = base.to_path_buf();
    for level in [RUNTIME_REGISTRY_NAME, kind] {
        dir.push(level);
        if create {
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return None,
            }
        }
        if !private_directory(&dir) {
            return None;
        }
    }
    Some(dir)
}

fn runtime_base() -> PathBuf {
    #[cfg(any(test, feature = "test-support"))]
    {
        let fake = PROC_ROOT_OVERRIDE
            .with(|cell| cell.borrow().clone())
            .or_else(|| std::env::var_os(PROC_ROOT_ENV).map(PathBuf::from));
        if let Some(root) = fake {
            // A fake process table carries its own private runtime base.
            use std::os::unix::fs::DirBuilderExt;
            let base = root.join(".runtime");
            let _ = std::fs::DirBuilder::new().mode(0o700).create(&base);
            return base;
        }
    }
    PathBuf::from(format!("/run/user/{}", unsafe { libc::geteuid() }))
}

/// Whether `pid` holds a matching record of `kind` in the runtime registry;
/// a plain record that does not match (stale or forged) is removed.
fn runtime_record_matches(kind: &str, pid: u32, expected: &str) -> bool {
    let Some(dir) = runtime_registry_in(&runtime_base(), kind, false) else {
        return false;
    };
    let path = dir.join(pid.to_string());
    let plain = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
    let honoured = plain
        && same_user(&path)
        && std::fs::read_to_string(&path).ok().as_deref() == Some(expected);
    if plain && !honoured {
        let _ = std::fs::remove_file(&path);
    }
    honoured
}

/// Writes one record atomically (a private file renamed into place).
fn write_record(dir: &Path, pid: u32, text: &str) -> Result<(), String> {
    let path = dir.join(pid.to_string());
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text) {
        return Ok(());
    }
    let staging = dir.join(format!(".{pid}.reader.staging"));
    // Named for this pid, so a leftover is a crashed earlier process's own;
    // clear it rather than refusing every later process with this pid.
    let _ = std::fs::remove_file(&staging);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&staging, &path))
        .map_err(|error| {
            let _ = std::fs::remove_file(&staging);
            format!("cannot register this read-only EmuWiz: {error}")
        })
}

/// Registers a read-only invocation (no journal work is expected) in the
/// per-user runtime registry only: no seal store and nothing in the data
/// directory. Fails when that registry is unavailable; the caller then runs
/// unregistered (a concurrent writer fails closed) and never falls back to the
/// data directory. Any journal work this process does still passes [`check`],
/// which registers it in the data directory first.
pub fn register_reader_at_startup() -> Result<Registration, String> {
    let pid = std::process::id();
    let real = Path::new("/proc");
    let (Some(started), Some(identity)) = (start_time(real, pid), executable_identity(real, pid))
    else {
        return Err("cannot identify this process for registration".into());
    };
    let base = runtime_base();
    let (Some(sealed), Some(finalise)) = (
        runtime_registry_in(&base, SEALED_WRITERS_DIR, true),
        runtime_registry_in(&base, FINALISE_WRITERS_DIR, true),
    ) else {
        return Err("no private per-user runtime directory".into());
    };
    write_record(&sealed, pid, &registration_text(&started, identity))?;
    write_record(&finalise, pid, &finalise_text(&started, identity))?;
    Ok(Registration {
        path: sealed.join(pid.to_string()),
    })
}

/// Inspect candidate identity without interpreting an unreadable live process
/// as absent. ENOENT on `exe` alone also occurs for zombies and is not proof
/// that the process directory has disappeared.
fn candidate_executable(process: &Path) -> Result<Option<PathBuf>, String> {
    use std::os::unix::fs::MetadataExt;
    let inspection_error = |error| {
        format!(
            "cannot inspect running process {}: {error}; journal work is disabled. Nothing was changed",
            process.display()
        )
    };
    let metadata = match std::fs::metadata(process) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(inspection_error(error)),
    };
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Ok(None);
    }
    match std::fs::read_link(process.join("exe")) {
        Ok(executable) => Ok(Some(executable)),
        Err(error) => {
            if std::fs::metadata(process).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
                return Ok(None);
            }
            let zombie = std::fs::read_to_string(process.join("stat"))
                .ok()
                .and_then(|stat| {
                    stat.rsplit_once(')')
                        .map(|(_, rest)| rest.split_whitespace().next() == Some("Z"))
                })
                .unwrap_or(false);
            if zombie {
                Ok(None)
            } else {
                Err(inspection_error(error))
            }
        }
    }
}

/// Running EmuWiz processes of this user (other than this one) that cannot
/// read the Finalise states: any process that is not a verified
/// finalise-capable registrant, sealing or not. Read-only, for display: an
/// unreadable process table lists none here, but the gate itself refuses.
pub fn finalise_incompatible_writers() -> Vec<OlderWriter> {
    try_finalise_incompatible_writers().unwrap_or_default()
}

fn try_finalise_incompatible_writers() -> Result<Vec<OlderWriter>, String> {
    let root = proc_root();
    let own = std::process::id();
    let entries = process_entries(&root)?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect running processes: {error}; journal work is disabled. Nothing was changed"))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own {
            continue;
        }
        let Some(executable) = candidate_executable(&entry.path())? else {
            continue;
        };
        let name = executable
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .trim_end_matches(" (deleted)");
        if EMUWIZ_EXECUTABLES.contains(&name)
            && !finalise_capable(&root, pid)
            && !profile::verified_isolated(&root, pid)
        {
            found.push(OlderWriter { pid, executable });
        }
    }
    found.sort_by_key(|writer| writer.pid);
    Ok(found)
}

/// The gate for any journal move into, within or out of the Finalise states.
/// Stricter than [`check`]: besides the sealing requirement, every other
/// running EmuWiz of this user must be a registered build that understands
/// `Finalising`/`Finalised`. A build that does not would fail closed on such a
/// journal (it cannot parse it), but finalising never starts while one runs, so
/// no older process can be mid-operation on a journal that is being finalised.
pub fn check_finalise() -> Result<(), String> {
    check()?;
    register_finalise_self()?;
    let incompatible = try_finalise_incompatible_writers()?;
    if incompatible.is_empty() {
        return Ok(());
    }
    let list = incompatible
        .iter()
        .map(|writer| format!("{} (PID {})", writer.executable.display(), writer.pid))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "{FINALISE_INCOMPATIBLE_WRITER}: {list}. Close it before finalising a replacement. Nothing was changed"
    ))
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
        let pid = std::process::id().to_string();
        let _ = std::fs::remove_file(&self.path);
        if let Ok(dir) = finalise_registrations_dir() {
            let _ = std::fs::remove_file(dir.join(&pid));
        }
        if let Some(dir) = runtime_registry_in(&runtime_base(), FINALISE_WRITERS_DIR, false) {
            let _ = std::fs::remove_file(dir.join(&pid));
        }
    }
}

/// Registers this program at startup, before any Apply or Undo is admitted,
/// once it has confirmed it can run the sealed-journal protocol (its seal
/// store opens and is prepared).
pub fn register_at_startup() -> Result<Registration, String> {
    super::seal::self_check()?;
    register_self()?;
    register_finalise_self()?;
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
        || runtime_record_matches(
            SEALED_WRITERS_DIR,
            pid,
            &registration_text(&started, identity),
        )
}

/// Removes registrations whose process no longer exists.
fn sweep_stale(root: &Path) {
    for dir in [registrations_dir(), finalise_registrations_dir()]
        .into_iter()
        .flatten()
    {
        sweep_dir(root, &dir);
    }
    let base = runtime_base();
    for kind in [SEALED_WRITERS_DIR, FINALISE_WRITERS_DIR] {
        if let Some(dir) = runtime_registry_in(&base, kind, false) {
            sweep_dir(root, &dir);
        }
    }
}

fn sweep_dir(root: &Path, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
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
/// writers. Read-only, for display: an
/// unreadable process table lists none here, but the gate itself refuses.
pub fn older_writers() -> Vec<OlderWriter> {
    try_older_writers()
        .map(|(writers, _)| writers)
        .unwrap_or_default()
}

fn try_older_writers() -> Result<(Vec<OlderWriter>, bool), String> {
    let root = proc_root();
    let own = std::process::id();
    let entries = process_entries(&root)?;
    let mut found = Vec::new();
    let mut excluded_profile = false;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect running processes: {error}; journal work is disabled. Nothing was changed"))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own {
            continue;
        }
        let Some(executable) = candidate_executable(&entry.path())? else {
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
        if profile::verified_isolated(&root, pid) {
            excluded_profile = true;
            continue;
        }
        found.push(OlderWriter { pid, executable });
    }
    found.sort_by_key(|writer| writer.pid);
    Ok((found, excluded_profile))
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
    // Inspect first: with an unreadable table every peer would look gone, and
    // sweeping would delete live registrations.
    let root = proc_root();
    process_entries(&root)?;
    sweep_stale(&root);
    let (older, excluded_profile) = try_older_writers()?;
    if older.is_empty() {
        register_self()?;
        // An open gate is re-inspected at most every two seconds; a closed
        // one is never cached.
        // Profile evidence is live filesystem evidence and must be rechecked
        // on every gate invocation, rather than receiving the no-peer cache.
        if cacheable
            && !excluded_profile
            && let Ok(mut guard) = OPEN_UNTIL.lock()
        {
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

    #[test]
    fn genuinely_isolated_profiles_do_not_block_journal_work() {
        let table = fake_table("emuwiz-gui", 4_800_001, "81");
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        let peer = tempfile::tempdir().unwrap();
        for tail in [".config/emuwiz", ".local/share/emuwiz"] {
            std::fs::create_dir_all(peer.path().join(tail)).unwrap();
        }
        let ours = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ours.path().join(".local/share/emuwiz")).unwrap();
        std::fs::create_dir_all(ours.path().join(".config/emuwiz")).unwrap();
        let process = table.path().join("4800001");
        std::fs::write(
            process.join("environ"),
            format!("HOME={}\0", peer.path().display()),
        )
        .unwrap();
        std::fs::write(process.join("cmdline"), b"emuwiz-gui\0").unwrap();
        std::fs::create_dir(process.join("fd")).unwrap();
        std::os::unix::fs::symlink(peer.path(), process.join("cwd")).unwrap();
        std::fs::create_dir(process.join("ns")).unwrap();
        std::os::unix::fs::symlink(
            std::fs::read_link("/proc/self/ns/mnt").unwrap(),
            process.join("ns/mnt"),
        )
        .unwrap();
        std::os::unix::fs::symlink("/", process.join("root")).unwrap();
        let own = std::process::id();
        add_process(table.path(), "emuwiz-gui", own, "82");
        std::fs::write(
            table.path().join(own.to_string()).join("environ"),
            format!("HOME={}\0", ours.path().display()),
        )
        .unwrap();
        std::fs::write(
            table.path().join(own.to_string()).join("cmdline"),
            b"emuwiz-gui\0",
        )
        .unwrap();
        assert!(
            older_writers().is_empty(),
            "isolated profile must not close the gate"
        );
        assert!(finalise_incompatible_writers().is_empty());
        assert!(
            try_older_writers().unwrap().1,
            "profile exception must disable caching"
        );
        assert!(check().is_ok());
        assert!(check_finalise().is_ok());
        // The next check must re-evaluate newly shared journal aliases.
        std::os::unix::fs::symlink(
            ours.path().join(".local/share/emuwiz"),
            peer.path().join(".local/share/emuwiz/shared-journals"),
        )
        .unwrap();
        assert!(check().is_err());
    }

    #[test]
    fn unreadable_live_executable_is_unknown_but_confirmed_exit_and_zombie_are_safe() {
        let table = fake_table("emuwiz-gui", 4_800_003, "84");
        let process = table.path().join("4800003");
        std::fs::remove_file(process.join("exe")).unwrap();
        assert!(candidate_executable(&process).is_err());
        std::fs::write(process.join("exe"), b"not a readable executable link").unwrap();
        assert!(candidate_executable(&process).is_err());
        std::fs::write(process.join("stat"), "4800003 (emuwiz-gui) Z 1 1").unwrap();
        assert!(candidate_executable(&process).unwrap().is_none());
        std::fs::remove_dir_all(&process).unwrap();
        assert!(candidate_executable(&process).unwrap().is_none());
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

    /// Writes the runtime-registry records a read-only instance would write.
    fn register_reader(root: &Path, pid: u32) {
        let (started, identity) = (
            start_time(root, pid).unwrap(),
            executable_identity(root, pid).unwrap(),
        );
        let base = runtime_base();
        let sealed = runtime_registry_in(&base, SEALED_WRITERS_DIR, true).unwrap();
        let finalise = runtime_registry_in(&base, FINALISE_WRITERS_DIR, true).unwrap();
        write_record(&sealed, pid, &registration_text(&started, identity)).unwrap();
        write_record(&finalise, pid, &finalise_text(&started, identity)).unwrap();
    }

    #[test]
    fn a_reader_registered_only_at_runtime_keeps_both_gates_open() {
        let table = fake_table("emuwiz-cli", 4_600_001, "61");
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        // Unregistered: an older writer, as before.
        assert_eq!(older_writers().len(), 1);
        assert!(is_older_writer_refusal(&check().unwrap_err()));
        register_reader(table.path(), 4_600_001);
        assert!(older_writers().is_empty());
        check().unwrap();
        check_finalise().unwrap();
        // Nothing of the reader's went into the data-directory registries.
        let data_record = registrations_dir().unwrap().join("4600001");
        assert!(!data_record.exists());
        assert!(
            !finalise_registrations_dir()
                .unwrap()
                .join("4600001")
                .exists()
        );
    }

    #[test]
    fn stale_or_forged_runtime_records_are_not_honoured_and_are_cleaned() {
        let table = fake_table("emuwiz", 4_700_001, "71");
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        let sealed = runtime_registry_in(&runtime_base(), SEALED_WRITERS_DIR, true).unwrap();
        let identity = executable_identity(table.path(), 4_700_001).unwrap();
        for forged in [
            registration_text("70", identity), // pid reused: other start
            registration_text("71", (identity.0, identity.1 + 1)), // other executable
            "protocol=unsealed\n".to_string(),
        ] {
            std::fs::write(sealed.join("4700001"), forged).unwrap();
            assert_eq!(older_writers().len(), 1);
            assert!(
                !sealed.join("4700001").exists(),
                "a forged record is removed"
            );
        }
        // A valid record reached through a symlink is not a registration.
        let real = table.path().join("elsewhere");
        std::fs::write(
            &real,
            registration_text(&start_time(table.path(), 4_700_001).unwrap(), identity),
        )
        .unwrap();
        std::os::unix::fs::symlink(&real, sealed.join("4700001")).unwrap();
        assert_eq!(older_writers().len(), 1);
    }

    #[test]
    fn an_unsafe_runtime_base_is_never_used() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = tempfile::tempdir().unwrap();
        let open = scratch.path().join("group-writable");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o770)).unwrap();
        let private = scratch.path().join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = scratch.path().join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        let missing = scratch.path().join("missing");
        for unsafe_base in [&open, &link, &missing] {
            assert!(runtime_registry_in(unsafe_base, SEALED_WRITERS_DIR, true).is_none());
        }
        assert!(!missing.exists(), "the base is never created");
        assert!(runtime_registry_in(&private, SEALED_WRITERS_DIR, true).is_some());
        // A level made group-writable afterwards is refused too.
        std::fs::set_permissions(
            private.join(RUNTIME_REGISTRY_NAME),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        assert!(runtime_registry_in(&private, SEALED_WRITERS_DIR, false).is_none());
    }

    #[test]
    fn a_reader_registration_writes_only_the_runtime_registry_and_is_released() {
        let table = tempfile::tempdir().unwrap();
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        let pid = std::process::id().to_string();
        let registration = register_reader_at_startup().unwrap();
        let sealed = runtime_registry_in(&runtime_base(), SEALED_WRITERS_DIR, false).unwrap();
        let finalise = runtime_registry_in(&runtime_base(), FINALISE_WRITERS_DIR, false).unwrap();
        assert!(sealed.join(&pid).is_file() && finalise.join(&pid).is_file());
        assert!(!registrations_dir().unwrap().join(&pid).exists());
        assert!(!finalise_registrations_dir().unwrap().join(&pid).exists());
        drop(registration);
        assert!(!sealed.join(&pid).exists() && !finalise.join(&pid).exists());
    }

    #[test]
    fn an_unreadable_process_table_closes_the_gate_instead_of_opening_it() {
        let scratch = tempfile::tempdir().unwrap();
        let not_a_directory = scratch.path().join("proc-is-a-file");
        std::fs::write(&not_a_directory, b"").unwrap();
        for root in [scratch.path().join("no-such-proc"), not_a_directory] {
            set_process_root_for_this_thread(Some(root.clone()));
            let _reset = Reset;
            for refusal in [check().unwrap_err(), check_finalise().unwrap_err()] {
                assert!(
                    refusal.contains("cannot inspect running processes"),
                    "{refusal}"
                );
                assert!(refusal.contains("Nothing was changed"), "{refusal}");
            }
            // Display lists stay empty, but they never gate anything.
            assert!(older_writers().is_empty());
        }
        // A readable, empty table is the normal open gate.
        let empty = tempfile::tempdir().unwrap();
        set_process_root_for_this_thread(Some(empty.path().to_path_buf()));
        let _reset = Reset;
        check().unwrap();
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
    /// Writes the registration of a build that also reads the Finalise states.
    fn register_finalise(root: &Path, pid: u32) {
        let text = finalise_text(
            &start_time(root, pid).unwrap(),
            executable_identity(root, pid).unwrap(),
        );
        let registry = finalise_registrations_dir().unwrap();
        std::fs::create_dir_all(&registry).unwrap();
        std::fs::write(registry.join(pid.to_string()), text).unwrap();
    }

    #[test]
    fn a_sealing_build_that_cannot_read_finalised_journals_blocks_only_finalising() {
        let table = tempfile::tempdir().unwrap();
        add_process(table.path(), "emuwiz", 4_400_001, "41");
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        register(table.path(), 4_400_001);
        // Ordinary journal work still shares with it, exactly as before.
        assert!(older_writers().is_empty());
        assert!(check().is_ok());
        // Finalising does not.
        let refusal = check_finalise().unwrap_err();
        assert!(refusal.contains(FINALISE_INCOMPATIBLE_WRITER), "{refusal}");
        assert!(refusal.contains("PID 4400001"), "{refusal}");
        assert!(!is_older_writer_refusal(&refusal));
        // Once that process is a finalise-capable build, it no longer blocks.
        register_finalise(table.path(), 4_400_001);
        assert!(check_finalise().is_ok());
    }

    #[test]
    fn an_unsealed_older_emuwiz_blocks_finalising_through_the_ordinary_gate() {
        let table = tempfile::tempdir().unwrap();
        add_process(table.path(), "archivefs-gui", 4_500_001, "51");
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        assert!(is_older_writer_refusal(&check_finalise().unwrap_err()));
    }

    #[test]
    fn stale_reused_and_forged_finalise_registrations_are_not_honoured() {
        let table = tempfile::tempdir().unwrap();
        add_process(table.path(), "emuwiz", 4_600_001, "61");
        set_process_root_for_this_thread(Some(table.path().to_path_buf()));
        let _reset = Reset;
        register(table.path(), 4_600_001);
        let registry = finalise_registrations_dir().unwrap();
        std::fs::create_dir_all(&registry).unwrap();
        // A bare marker and a wrong protocol are not registrations.
        for forged in ["61", "protocol=sealed-journal-v1\nstart=61\n"] {
            std::fs::write(registry.join("4600001"), forged).unwrap();
            assert!(check_finalise().is_err(), "{forged}");
        }
        // A genuine record for a pid reused by another start time.
        register_finalise(table.path(), 4_600_001);
        assert!(check_finalise().is_ok());
        std::fs::write(
            table.path().join("4600001").join("stat"),
            "4600001 (emuwiz) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 99 0 0",
        )
        .unwrap();
        assert!(check_finalise().is_err());
    }

    #[test]
    fn startup_registers_finalise_capability_and_releases_it() {
        let empty = tempfile::tempdir().unwrap();
        set_process_root_for_this_thread(Some(empty.path().to_path_buf()));
        let _reset = Reset;
        let path = finalise_registrations_dir()
            .unwrap()
            .join(std::process::id().to_string());
        {
            let _registration = register_at_startup().unwrap();
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.starts_with("protocol=finalised-state-v1\n"), "{text}");
        }
        assert!(!path.exists());
    }
}
