//! Conservative exception for provably separate default GUI profiles.
//!
//! Old builds may ignore newer root overrides, so inspect the union of explicit,
//! XDG and HOME roots, with both application names. Never infer isolation from
//! HOME alone. Non-default command lines remain subject to the original gate:
//! they can select journals outside the profile, including positional paths.

use super::{executable_identity, start_time};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Read;
use std::os::unix::{
    ffi::OsStringExt,
    fs::{MetadataExt, OpenOptionsExt},
};
use std::path::{Component, Path, PathBuf};

type Identity = (u64, u64);
type Environment = Vec<(OsString, OsString)>;

fn identity(path: &Path) -> Option<Identity> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.dev(), m.ino()))
}

fn process_identity(root: &Path, pid: u32) -> Option<(String, Identity)> {
    Some((start_time(root, pid)?, executable_identity(root, pid)?))
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(1_048_577).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= 1_048_576 && bytes.last() == Some(&0)).then_some(bytes)
}

fn environment(path: &Path) -> Option<Environment> {
    let bytes = read_bounded(path)?;
    let mut values = Vec::new();
    for entry in bytes[..bytes.len() - 1].split(|b| *b == 0) {
        let split = entry.iter().position(|b| *b == b'=')?;
        let key = OsString::from_vec(entry[..split].to_vec());
        if values.iter().any(|(previous, _)| previous == &key) {
            return None;
        }
        values.push((key, OsString::from_vec(entry[split + 1..].to_vec())));
    }
    Some(values)
}

fn default_gui(path: &Path) -> Option<()> {
    let bytes = read_bounded(path)?;
    // No arguments at all: flags and positional journal paths both fail closed.
    if bytes[..bytes.len() - 1].contains(&0) {
        return None;
    }
    let executable = PathBuf::from(OsString::from_vec(bytes[..bytes.len() - 1].to_vec()));
    matches!(
        executable.file_name()?.to_str()?,
        "emuwiz" | "emuwiz-gui" | "emuwiz-v2" | "archivefs" | "archivefs-gui"
    )
    .then_some(())
}

fn roots(env: &Environment) -> Option<Vec<PathBuf>> {
    let get = |key: &str| {
        env.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| PathBuf::from(v))
    };
    // HOME is required even with overrides: older builds may ignore those.
    let home = get("HOME").or_else(|| get("USERPROFILE"))?;
    let config = crate::app_dirs::config_dir_in_with_roots(
        &home,
        get("EMUWIZ_CONFIG_HOME").as_deref(),
        get("XDG_CONFIG_HOME").as_deref(),
    );
    let data = crate::app_dirs::data_dir_in_with_roots(
        &home,
        get("EMUWIZ_DATA_HOME").as_deref(),
        get("XDG_DATA_HOME").as_deref(),
    );
    // A changed HOME string and non-existent projected paths are not enough.
    // Require the active profile roots to be real directories as well.
    if !std::fs::metadata(config).ok()?.is_dir() || !std::fs::metadata(data).ok()?.is_dir() {
        return None;
    }
    let mut roots = Vec::new();
    for (explicit, xdg, default) in [
        ("EMUWIZ_CONFIG_HOME", "XDG_CONFIG_HOME", ".config"),
        ("EMUWIZ_DATA_HOME", "XDG_DATA_HOME", ".local/share"),
    ] {
        if let Some(path) = get(explicit) {
            roots.push(path);
        }
        for base in [Some(home.join(default)), get(xdg)].into_iter().flatten() {
            roots.push(base.join("emuwiz"));
            roots.push(base.join("archivefs"));
        }
    }
    roots.iter().all(|p| p.is_absolute()).then_some(roots)
}

// Missing roots are resolved through their nearest existing ancestor. Permission
// errors, dangling symlinks and ambiguous components never establish isolation.
fn resolved(path: &Path) -> Option<PathBuf> {
    if path
        .components()
        .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return None;
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => std::fs::canonicalize(path).ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Some(resolved(path.parent()?)?.join(path.file_name()?))
        }
        Err(_) => None,
    }
}

#[derive(PartialEq, Eq)]
struct State {
    paths: Vec<PathBuf>,
    identities: HashSet<Identity>,
}

fn state(roots: &[PathBuf]) -> Option<State> {
    let paths: Vec<_> = roots.iter().map(|p| resolved(p)).collect::<Option<_>>()?;
    let mut identities = HashSet::new();
    let mut pending = paths.clone();
    let mut visited_paths = HashSet::new();
    let mut visited = 0;
    while let Some(path) = pending.pop() {
        if !visited_paths.insert(path.clone()) {
            continue;
        }
        visited += 1;
        if visited > 16_384 {
            return None;
        }
        let m = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        // A nested alias can route a database or journal outside its root.
        // Hard-linked files can share data even when canonical paths differ.
        if m.file_type().is_symlink() || (m.is_file() && m.nlink() != 1) {
            return None;
        }
        if !identities.insert((m.dev(), m.ino())) {
            // Distinct bind aliases can have different child mount topology.
            // Identical inode numbers do not justify skipping the second tree.
            return None;
        }
        if m.is_dir() {
            for entry in std::fs::read_dir(&path).ok()? {
                pending.push(entry.ok()?.path());
            }
        } else if !m.is_file() {
            return None;
        }
    }
    Some(State { paths, identities })
}

fn disjoint(a: &State, b: &State) -> bool {
    a.identities.is_disjoint(&b.identities)
        && a.paths.iter().all(|a| {
            b.paths
                .iter()
                .all(|b| !a.starts_with(b) && !b.starts_with(a))
        })
}

fn isolated(root: &Path, pid: u32, ours: &Environment, after: impl FnOnce()) -> Option<()> {
    let process = root.join(pid.to_string());
    // Retain the kernel process-directory object: a replacement with the same
    // PID and a coincidentally equal start-time tick is still a different task.
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&process)
        .ok()?;
    let metadata = directory.metadata().ok()?;
    let directory_identity = (metadata.dev(), metadata.ino());
    let before = process_identity(root, pid)?;
    let namespace = std::fs::read_link(process.join("ns/mnt")).ok()?;
    let peer_root = identity(&process.join("root"))?;
    let command = read_bounded(&process.join("cmdline"))?;
    // Environment paths must mean the same thing in both processes.
    if namespace != std::fs::read_link("/proc/self/ns/mnt").ok()?
        || peer_root != identity(Path::new("/"))?
    {
        return None;
    }
    default_gui(&process.join("cmdline"))?;
    default_gui(&root.join(std::process::id().to_string()).join("cmdline"))?;
    let peer_env = environment(&process.join("environ"))?;
    let our_roots = roots(ours)?;
    let peer_roots = roots(&peer_env)?;
    let our_state = state(&our_roots)?;
    let peer_state = state(&peer_roots)?;
    if !disjoint(&our_state, &peer_state) {
        return None;
    }
    // Even a default-profile process may retain a previously selected journal.
    // An unreadable cwd/fd entry is unknown, never proof of separation.
    let mut handles = vec![process.join("cwd")];
    for entry in std::fs::read_dir(process.join("fd")).ok()? {
        handles.push(entry.ok()?.path());
    }
    for handle in handles {
        let target = std::fs::read_link(&handle).ok()?;
        if target.is_absolute() {
            let canonical = std::fs::canonicalize(&handle).ok()?;
            if our_state.paths.iter().any(|p| canonical.starts_with(p))
                || our_state.identities.contains(&identity(&handle)?)
            {
                return None;
            }
        }
    }
    after();
    // Bracket the evidence with process identity and filesystem observations.
    // Exit, exec, PID reuse or a changed root/alias invalidates the exception.
    (identity(&process)? == directory_identity
        && process_identity(root, pid)? == before
        && std::fs::read_link(process.join("ns/mnt")).ok()? == namespace
        && identity(&process.join("root"))? == peer_root
        && read_bounded(&process.join("cmdline"))? == command
        && environment(&process.join("environ"))? == peer_env
        && state(&our_roots)? == our_state
        && state(&peer_roots)? == peer_state)
        .then_some(())
}

pub(super) fn verified_isolated(root: &Path, pid: u32) -> bool {
    let ours = std::env::vars_os().collect();
    // The existing fake process table also supplies this process's environment,
    // so gate tests never share or mutate the test runner's profile roots.
    #[cfg(test)]
    let ours = if super::process_root_overridden() {
        let Some(ours) = environment(&root.join(std::process::id().to_string()).join("environ"))
        else {
            return false;
        };
        ours
    } else {
        ours
    };
    isolated(root, pid, &ours, || {}).is_some()
}

#[cfg(test)]
mod tests;
