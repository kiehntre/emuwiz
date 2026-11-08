//! Isolated, disposable Hatari session.
//!
//! Hatari merges `--configfile` into whatever it already loaded from the
//! user's own `~/.config/hatari`, and it will insert drive B, attach hard
//! disks and write back to the selected image according to that inherited
//! configuration. This module removes every one of those paths:
//!
//! * a private session directory holds a fresh `HOME` and `XDG_*` tree, so no
//!   user configuration, storage path or save is ever read;
//! * the selected disk is copied (bounded, no symlinks, hash-verified) into
//!   the session as a read-only file, so the original media is never opened
//!   for writing;
//! * the command line enables only drive A, switches drive B off, turns floppy
//!   write protection on, and passes machine and TOS explicitly.
//!
//! Saves: with a saves root (the normal Play path) each game gets its own
//! persistent working disk under EmuWiz's data directory, seeded once from
//! the verified original. Hatari writes high scores and save games to that
//! working disk; the previous state is kept as `*.previous` before every
//! launch, and a second launch of the same game is refused while one is
//! running. The original media is never opened for writing. Without a saves
//! root the copy is read-only and discarded with the session.
//!
//! Hardware preferences (CPU, clock, RAM, monitor, blitter, FPU, DSP, MMU)
//! are read from the user's own `hatari.cfg` through a strict allow-list and
//! passed as validated flags; no storage, floppy or path setting is read.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::launch::hatari_command::HatariCommand;
use crate::patch_manager::HatariMachineModel;

/// Floppy images are small; anything larger is refused.
pub const MAX_DISK_BYTES: u64 = 32 * 1024 * 1024;

/// Sessions older than this are removed at the next launch.
pub const STALE_SESSION_AGE: Duration = Duration::from_secs(24 * 60 * 60);

const SESSION_PREFIX: &str = "session-";

/// Records the Hatari process id so pruning never removes a live session.
pub const SESSION_PID_FILE: &str = "hatari.pid";

/// Hatari reads this system-wide file before any user configuration, and no
/// command-line flag switches off the hard-disk, GEMDOS, ACSI or IDE settings
/// it may carry. Isolation cannot be guaranteed while it exists, so launch is
/// refused (fail closed) rather than risk inherited storage.
pub const SYSTEM_CONFIG: &str = "/etc/hatari.cfg";

/// A prepared, private Hatari invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HatariSession {
    pub root: PathBuf,
    pub disk_copy: PathBuf,
    pub working_directory: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    pub arguments: Vec<OsString>,
    /// The persistent per-game save directory, when saves are kept.
    pub save_dir: Option<PathBuf>,
}

/// How a session is prepared beyond the always-on isolation.
#[derive(Debug, Clone, Default)]
pub struct SessionOptions<'a> {
    /// Full-file SHA-256 (hex) the selected media must still have.
    pub expected_sha256: Option<&'a str>,
    /// Root of persistent per-game saves; `None` = disposable, read-only.
    pub saves_root: Option<&'a Path>,
    /// Validated hardware flags (see [`hardware_arguments`]).
    pub hardware: Vec<OsString>,
}

/// Bounded size for reading the user's `hatari.cfg`.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Reads only the allow-listed hardware keys from a Hatari configuration
/// file (no symlinks, bounded). Unreadable or odd files yield no flags, so
/// Hatari's defaults for the authorised machine apply.
pub fn read_hardware_arguments(config_path: &Path) -> Vec<OsString> {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(config_path)
    else {
        return Vec::new();
    };
    if !file.metadata().is_ok_and(|m| m.file_type().is_file()) {
        return Vec::new();
    }
    let mut text = String::new();
    if file
        .take(MAX_CONFIG_BYTES)
        .read_to_string(&mut text)
        .is_err()
    {
        return Vec::new();
    }
    hardware_arguments(&text)
}

/// Translates allow-listed `hatari.cfg` hardware keys into Hatari flags.
/// Every value is validated against the documented range; anything else is
/// skipped. Storage, floppy, ROM and path keys are never consulted.
pub fn hardware_arguments(config_text: &str) -> Vec<OsString> {
    let mut section = String::new();
    let mut values: Vec<(String, String)> = Vec::new();
    for line in config_text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
        } else if let Some((key, value)) = line.split_once('=') {
            values.push((
                format!("{section}.{}", key.trim()),
                value.trim().to_string(),
            ));
        }
    }
    let get = |key: &str| {
        values
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let boolean = |key: &str| match get(key).map(str::to_ascii_lowercase).as_deref() {
        Some("true" | "1" | "yes") => Some("true"),
        Some("false" | "0" | "no") => Some("false"),
        _ => None,
    };
    let int = |key: &str| get(key).and_then(|v| v.parse::<u32>().ok());
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |flag: &str, value: String| {
        args.push(flag.into());
        args.push(value.into());
    };
    if let Some(level @ (0..=4 | 6)) = int("System.nCpuLevel") {
        push("--cpulevel", level.to_string());
    }
    if let Some(clock @ (8 | 16 | 32)) = int("System.nCpuFreq") {
        push("--cpuclock", clock.to_string());
    }
    for (key, flag) in [
        ("System.bCompatibleCpu", "--compatible"),
        ("System.bCycleExactCpu", "--cpu-exact"),
        ("System.bAddressSpace24", "--addr24"),
        ("System.bMMU", "--mmu"),
        ("System.bBlitter", "--blitter"),
    ] {
        if let Some(value) = boolean(key) {
            push(flag, value.to_string());
        }
    }
    if let Some(fpu) = int("System.nFPUType").and_then(|v| match v {
        0 => Some("none"),
        68881 => Some("68881"),
        68882 => Some("68882"),
        68040 => Some("internal"),
        _ => None,
    }) {
        push("--fpu", fpu.to_string());
    }
    if let Some(dsp) = int("System.nDSPType").and_then(|v| match v {
        0 => Some("none"),
        1 => Some("dummy"),
        2 => Some("emu"),
        _ => None,
    }) {
        push("--dsp", dsp.to_string());
    }
    // ST RAM: KiB (current Hatari) or MiB (older `nMemorySize`).
    if let Some(kib) = int("Memory.STRamSize_KB").filter(|k| (256..=14 * 1024).contains(k)) {
        push("--memsize", kib.to_string());
    } else if let Some(mib) = int("Memory.nMemorySize").filter(|m| *m <= 14) {
        push("--memsize", mib.to_string());
    }
    let tt_mib = int("Memory.TTRamSize_KB")
        .map(|kib| kib / 1024)
        .or_else(|| int("Memory.nTTRamSize"));
    if let Some(mib) = tt_mib.filter(|m| *m <= 1024 && m % 4 == 0) {
        push("--ttram", mib.to_string());
    }
    if let Some(monitor) = int("Screen.nMonitorType").and_then(|v| match v {
        0 => Some("mono"),
        1 => Some("rgb"),
        2 => Some("vga"),
        3 => Some("tv"),
        _ => None,
    }) {
        push("--monitor", monitor.to_string());
    }
    args
}

/// The `--machine` argument value for a model.
pub fn machine_argument(model: HatariMachineModel) -> Option<&'static str> {
    match model {
        HatariMachineModel::St => Some("st"),
        HatariMachineModel::Ste => Some("ste"),
        HatariMachineModel::Tt => Some("tt"),
        HatariMachineModel::Falcon => Some("falcon"),
        HatariMachineModel::Unknown => None,
    }
}

/// The complete, inherited-config-free argument list. `disk` is the image
/// Hatari will open (the session copy at spawn time, the selected media in a
/// read-only preview).
pub fn isolated_arguments(
    machine: HatariMachineModel,
    tos_path: &Path,
    disk: &Path,
) -> Result<Vec<OsString>, String> {
    session_arguments(machine, tos_path, disk, &[], true)
}

fn session_arguments(
    machine: HatariMachineModel,
    tos_path: &Path,
    disk: &Path,
    hardware: &[OsString],
    protect_floppy: bool,
) -> Result<Vec<OsString>, String> {
    let machine = machine_argument(machine).ok_or("Hatari machine model is unknown")?;
    let mut args: Vec<OsString> = vec!["--machine".into(), machine.into()];
    // After `--machine`, so the user's preferences refine its defaults.
    args.extend(hardware.iter().cloned());
    args.extend([
        "--tos".into(),
        tos_path.as_os_str().to_os_string(),
        "--drive-a".into(),
        "on".into(),
        "--drive-b".into(),
        "off".into(),
        "--protect-floppy".into(),
        if protect_floppy { "on" } else { "off" }.into(),
        "--confirm-quit".into(),
        "false".into(),
        "--disk-a".into(),
        disk.as_os_str().to_os_string(),
    ]);
    Ok(args)
}

fn read_source(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("cannot open the selected disk: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("the selected disk is not a regular file".into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_DISK_BYTES {
        return Err("the selected disk image is empty or larger than 32 MiB".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_DISK_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != metadata.len() {
        return Err("the selected disk changed while it was being copied".into());
    }
    Ok(bytes)
}

fn private_dir(path: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))
}

fn copy_file_name(source: &Path) -> String {
    // Lossy, not `to_str`: a non-UTF-8 name must keep its real extension,
    // which Hatari uses to pick the image format.
    let name: String = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "disk.st".into())
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("disk-{name}")
}

/// Builds the private session for `command` under `sessions_root`. Never
/// writes to, renames or locks the selected media.
pub fn prepare_session(
    command: &HatariCommand,
    sessions_root: &Path,
) -> Result<HatariSession, String> {
    prepare_session_expecting(command, sessions_root, None)
}

/// As [`prepare_session`], but the copied bytes must also hash to
/// `expected_sha256` (lower-case hex): the exact media that was inspected
/// and authorised, so a file swapped in after preflight is never booted.
pub fn prepare_session_expecting(
    command: &HatariCommand,
    sessions_root: &Path,
    expected_sha256: Option<&str>,
) -> Result<HatariSession, String> {
    prepare_session_with(
        command,
        sessions_root,
        &SessionOptions {
            expected_sha256,
            ..SessionOptions::default()
        },
    )
}

/// Full form: hash binding, persistent saves and hardware preferences.
pub fn prepare_session_with(
    command: &HatariCommand,
    sessions_root: &Path,
    options: &SessionOptions<'_>,
) -> Result<HatariSession, String> {
    let expected_sha256 = options.expected_sha256;
    if Path::new(SYSTEM_CONFIG).exists() {
        return Err(format!(
            "{SYSTEM_CONFIG} exists and could attach hard disks to every Hatari session; \
             EmuWiz will not launch Hatari until it is removed or renamed"
        ));
    }
    prune_stale_sessions(sessions_root, STALE_SESSION_AGE);
    let source = &command.selection.content_path;
    let bytes = read_source(source)?;
    let digest = Sha256::digest(&bytes);
    if let Some(expected) = expected_sha256 {
        let actual: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(
                "the selected disk changed after it was checked; nothing was launched".into(),
            );
        }
    }

    private_dir(sessions_root)?;
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let root = sessions_root.join(format!("{SESSION_PREFIX}{}-{unique}", std::process::id()));
    private_dir(&root)?;
    let built = build_session(command, source, &bytes, &digest, &root, options);
    if built.is_err() {
        let _ = make_tree_removable(&root);
        let _ = std::fs::remove_dir_all(&root);
    }
    built
}

fn build_session(
    command: &HatariCommand,
    source: &Path,
    bytes: &[u8],
    digest: &[u8],
    root: &Path,
    options: &SessionOptions<'_>,
) -> Result<HatariSession, String> {
    let root = root.to_path_buf();
    let home = root.join("home");
    let config = root.join("xdg-config");
    let data = root.join("xdg-data");
    let cache = root.join("xdg-cache");
    let state = root.join("xdg-state");
    let media = root.join("media");
    for dir in [&home, &config, &data, &cache, &state, &media] {
        private_dir(dir)?;
    }

    let (disk_copy, save_dir) = match options.saves_root {
        Some(saves_root) => {
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            let save_dir = saves_root.join(hex);
            let disk = persistent_disk(&save_dir, source, bytes, digest)?;
            (disk, Some(save_dir))
        }
        None => (disposable_copy(&media, source, bytes, digest)?, None),
    };
    let protect_floppy = save_dir.is_none();
    let mut environment: Vec<(OsString, OsString)> = vec![
        ("HOME".into(), home.clone().into_os_string()),
        ("XDG_CONFIG_HOME".into(), config.into_os_string()),
        ("XDG_DATA_HOME".into(), data.into_os_string()),
        ("XDG_CACHE_HOME".into(), cache.into_os_string()),
        ("XDG_STATE_HOME".into(), state.into_os_string()),
    ];
    // A changed HOME must not break X authorisation.
    if std::env::var_os("XAUTHORITY").is_none()
        && let Some(original_home) = std::env::var_os("HOME")
    {
        let cookie = PathBuf::from(original_home).join(".Xauthority");
        if cookie.is_file() {
            environment.push(("XAUTHORITY".into(), cookie.into_os_string()));
        }
    }

    let arguments = session_arguments(
        command.selection.machine_model,
        &command.selection.tos_path,
        &disk_copy,
        &options.hardware,
        protect_floppy,
    )?;
    Ok(HatariSession {
        root,
        disk_copy,
        working_directory: home,
        environment,
        arguments,
        save_dir,
    })
}

/// The game's persistent working disk: created once from the verified
/// original bytes, otherwise reused (with the last state backed up first).
/// An existing working disk is never replaced, so saves are never lost.
fn persistent_disk(
    save_dir: &Path,
    source: &Path,
    bytes: &[u8],
    digest: &[u8],
) -> Result<PathBuf, String> {
    private_dir(save_dir)?;
    if session_is_live(save_dir) {
        return Err("this game is already running in Hatari; close it before playing again".into());
    }
    let working = save_dir.join(copy_file_name(source));
    match std::fs::symlink_metadata(&working) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || metadata.len() > MAX_DISK_BYTES {
                return Err(format!(
                    "the saved disk {} is not a regular disk image; it was left untouched",
                    working.display()
                ));
            }
            backup_previous(save_dir)?;
        }
        Err(_) => {
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&working)
                .map_err(|e| format!("cannot create the saved disk: {e}"))?;
            out.write_all(bytes).map_err(|e| e.to_string())?;
            out.sync_all().map_err(|e| e.to_string())?;
            drop(out);
            let written = std::fs::read(&working).map_err(|e| e.to_string())?;
            if Sha256::digest(&written).as_slice() != digest {
                let _ = std::fs::remove_file(&working);
                return Err(
                    "the saved disk did not match the original; nothing was launched".into(),
                );
            }
        }
    }
    Ok(working)
}

/// Copies every working file (the disk and any Hatari write overlay) to
/// `<name>.previous` atomically, so one bad session can always be undone.
fn backup_previous(save_dir: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(save_dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("disk-") || name.ends_with(".previous") || name.ends_with(".tmp") {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let temp = save_dir.join(format!("{name}.previous.tmp"));
        let _ = std::fs::remove_file(&temp);
        std::fs::copy(entry.path(), &temp)
            .and_then(|_| std::fs::rename(&temp, save_dir.join(format!("{name}.previous"))))
            .map_err(|e| format!("cannot back up the saved disk: {e}"))?;
    }
    Ok(())
}

fn disposable_copy(
    media: &Path,
    source: &Path,
    bytes: &[u8],
    digest: &[u8],
) -> Result<PathBuf, String> {
    let disk_copy = media.join(copy_file_name(source));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(&disk_copy)
        .map_err(|e| format!("cannot create the disk copy: {e}"))?;
    out.write_all(bytes).map_err(|e| e.to_string())?;
    out.sync_all().map_err(|e| e.to_string())?;
    drop(out);
    let written = std::fs::read(&disk_copy).map_err(|e| e.to_string())?;
    if Sha256::digest(&written).as_slice() != digest {
        return Err("the disk copy did not match the original; nothing was launched".into());
    }
    Ok(disk_copy)
}

/// Removes session directories (only those this module created, never
/// symlinks) whose modification time is older than `max_age`.
pub fn prune_stale_sessions(sessions_root: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(sessions_root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(SESSION_PREFIX) {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.file_type().is_dir() || session_is_live(&entry.path()) {
            continue;
        }
        let old = metadata
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if old {
            // The disk copy is read-only; make the tree writable to remove it.
            let _ = make_tree_removable(&entry.path());
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// True while the Hatari recorded for this session is still running.
fn session_is_live(session: &Path) -> bool {
    std::fs::read_to_string(session.join(SESSION_PID_FILE))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists())
}

/// Records the spawned Hatari's pid in its session (best effort).
pub fn record_session_pid(session: &HatariSession, pid: u32) {
    let _ = std::fs::write(session.root.join(SESSION_PID_FILE), pid.to_string());
    if let Some(save_dir) = &session.save_dir {
        let _ = std::fs::write(save_dir.join(SESSION_PID_FILE), pid.to_string());
    }
}

fn make_tree_removable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        for entry in std::fs::read_dir(path)?.flatten() {
            let _ = make_tree_removable(&entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
