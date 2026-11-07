//! The one rule for locating an external helper executable.
//!
//! Several subsystems hand work to a program they did not ship: 7-Zip for
//! RAR/LHA members, `unrar`, emulator binaries for version probes and
//! launches. Each of them used to search `PATH` on its own, and each search
//! accepted whatever `PATH` contained - including an empty entry, `.` or a
//! relative directory. Those entries resolve against the *current working
//! directory*, so a file called `7zz` dropped next to where the process
//! happened to be started would be the program that ran (Night Watch D8-01,
//! S05-01).
//!
//! This module replaces those searches with one policy:
//!
//! 1. Only **absolute** `PATH` directories are searched. Empty, `.` and
//!    relative entries are skipped; they are never joined with the working
//!    directory.
//! 2. At most [`MAX_PATH_DIRECTORIES`] entries are considered, in order.
//! 3. A bare candidate name (`7zz`) is searched; an absolute candidate
//!    (`/usr/lib/7zip/7z`) is checked in place; any other shape - a relative
//!    path with a directory part such as `tools/7zz` - is refused outright,
//!    because it could only ever mean "relative to wherever we are".
//! 4. The resolved file must be a regular file (after following symlinks:
//!    `/usr/bin/unrar -> /etc/alternatives/unrar` is the normal shape of a
//!    packaged tool) with at least one execute bit. A directory, FIFO,
//!    socket or a plain data file is not an executable.
//! 5. What comes back is always an absolute path, so the caller spawns the
//!    exact file that was checked rather than letting `execvp` repeat the
//!    search under different rules.
//!
//! Nothing here runs a program, reads a file's contents or writes anything.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Longest `PATH` this module is willing to walk. A `PATH` longer than this is
/// not an installation anyone maintains by hand; the tail is ignored rather
/// than searched.
pub const MAX_PATH_DIRECTORIES: usize = 64;

/// The absolute directories of `path_env`, in order, with every empty, `.`,
/// `..` or otherwise relative entry removed. `None` (no `PATH` at all) yields
/// an empty list.
pub fn absolute_path_directories(path_env: Option<&OsStr>) -> Vec<PathBuf> {
    let Some(path_env) = path_env else {
        return Vec::new();
    };
    std::env::split_paths(path_env)
        .filter(|directory| is_searchable_directory_entry(directory))
        .take(MAX_PATH_DIRECTORIES)
        .collect()
}

/// The same as [`absolute_path_directories`] for the process's own `PATH`.
pub fn process_path_directories() -> Vec<PathBuf> {
    absolute_path_directories(std::env::var_os("PATH").as_deref())
}

/// Whether one `PATH` entry may be searched: it must be absolute and contain
/// no `.` or `..` component. (`/usr/../etc` is absolute but still describes
/// a place by walking, and no packaged tool lives there.)
fn is_searchable_directory_entry(directory: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = directory.as_os_str().as_bytes();
        // Checked on the raw bytes: `Path::components` silently drops `.`
        // segments, which would let `/usr/./bin` through. An empty segment
        // (`/opt/7zip/`, `/usr//bin`) names the same directory and is
        // harmless, so it is allowed.
        bytes.first() == Some(&b'/')
            && bytes
                .split(|byte| *byte == b'/')
                .all(|segment| segment != b"." && segment != b"..")
    }
    #[cfg(not(unix))]
    {
        directory.is_absolute()
            && directory.components().all(|component| {
                matches!(
                    component,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
    }
}

/// Whether `path` names a regular file with an execute bit, following
/// symlinks. Anything unreadable, missing, or of another type is `false`.
pub fn is_regular_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Resolves `candidate` against `path_directories` (already filtered by
/// [`absolute_path_directories`]).
///
/// - An absolute candidate is accepted only if it is itself a regular
///   executable; `PATH` is not consulted.
/// - A bare file name is looked up in each directory in turn and the first
///   regular executable wins.
/// - Anything else (a relative path with a directory part, an empty name,
///   `.` or `..`) is refused with `None`.
pub fn resolve_executable_in(candidate: &Path, path_directories: &[PathBuf]) -> Option<PathBuf> {
    if candidate.as_os_str().is_empty() {
        return None;
    }
    if candidate.is_absolute() {
        return is_regular_executable(candidate).then(|| candidate.to_path_buf());
    }
    let mut components = candidate.components();
    let Some(std::path::Component::Normal(name)) = components.next() else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }
    path_directories
        .iter()
        .map(|directory| directory.join(name))
        .find(|path| is_regular_executable(path))
}

/// [`resolve_executable_in`] against the process's own `PATH`.
pub fn resolve_executable(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_absolute() {
        return resolve_executable_in(candidate, &[]);
    }
    resolve_executable_in(candidate, &process_path_directories())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn executable_stub(directory: &Path, name: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn relative_empty_and_dot_path_entries_are_never_searched() {
        let directories = absolute_path_directories(Some(&OsString::from(
            ":.:rel:tools/bin:/usr/bin::./x:..:/opt/tools",
        )));
        assert_eq!(
            directories,
            vec![PathBuf::from("/usr/bin"), PathBuf::from("/opt/tools")]
        );
        assert!(absolute_path_directories(None).is_empty());
        assert!(absolute_path_directories(Some(&OsString::from(".:rel:"))).is_empty());
    }

    #[test]
    fn absolute_entries_with_dot_components_are_not_searched() {
        let directories =
            absolute_path_directories(Some(&OsString::from("/usr/../etc:/usr/./bin:/usr/bin")));
        assert_eq!(directories, vec![PathBuf::from("/usr/bin")]);
        // A trailing or doubled slash names the same directory: still searched.
        let temp = tempfile::tempdir().unwrap();
        let stub = executable_stub(temp.path(), "7zz");
        for shape in [
            format!("{}/", temp.path().display()),
            format!("{}//", temp.path().display()),
            temp.path().display().to_string().replacen('/', "//", 1),
        ] {
            let directories = absolute_path_directories(Some(&OsString::from(&shape)));
            assert_eq!(directories.len(), 1, "{shape}");
            assert_eq!(
                resolve_executable_in(Path::new("7zz"), &directories)
                    .map(|p| std::fs::canonicalize(p).unwrap()),
                Some(std::fs::canonicalize(&stub).unwrap()),
                "{shape}"
            );
        }
    }

    #[test]
    fn path_walk_is_capped() {
        let long = std::iter::repeat_n("/nonexistent", MAX_PATH_DIRECTORIES + 10)
            .collect::<Vec<_>>()
            .join(":");
        assert_eq!(
            absolute_path_directories(Some(&OsString::from(long))).len(),
            MAX_PATH_DIRECTORIES
        );
    }

    #[test]
    fn a_bare_name_resolves_only_through_an_absolute_directory() {
        let temp = tempfile::tempdir().unwrap();
        let stub = executable_stub(temp.path(), "7zz");
        let found = resolve_executable_in(Path::new("7zz"), &[temp.path().to_path_buf()]);
        assert_eq!(found, Some(stub));
        assert_eq!(resolve_executable_in(Path::new("7zz"), &[]), None);
    }

    #[test]
    fn relative_candidates_with_a_directory_part_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("tools")).unwrap();
        executable_stub(&temp.path().join("tools"), "7zz");
        let directories = vec![temp.path().to_path_buf()];
        for candidate in ["tools/7zz", "./7zz", "../7zz", ".", "..", ""] {
            assert_eq!(
                resolve_executable_in(Path::new(candidate), &directories),
                None,
                "{candidate:?} must not resolve"
            );
        }
    }

    #[test]
    fn an_absolute_candidate_is_checked_in_place_and_must_be_executable() {
        let temp = tempfile::tempdir().unwrap();
        let stub = executable_stub(temp.path(), "7z");
        assert_eq!(resolve_executable_in(&stub, &[]), Some(stub.clone()));
        let data = temp.path().join("data");
        std::fs::write(&data, b"not a program").unwrap();
        assert_eq!(resolve_executable_in(&data, &[]), None);
        assert_eq!(resolve_executable_in(temp.path(), &[]), None);
        assert_eq!(
            resolve_executable_in(&temp.path().join("missing"), &[]),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_non_executable_or_special_file_named_like_the_tool_is_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        std::fs::write(first.join("7zz"), b"data without an execute bit").unwrap();
        let fifo = first.join("unrar");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let real = executable_stub(&second, "7zz");
        let directories = vec![first.clone(), second.clone()];
        assert_eq!(
            resolve_executable_in(Path::new("7zz"), &directories),
            Some(real)
        );
        assert_eq!(
            resolve_executable_in(Path::new("unrar"), &directories),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_packaged_tool_still_resolves() {
        let temp = tempfile::tempdir().unwrap();
        let real = executable_stub(temp.path(), "unrar-real");
        let link = temp.path().join("unrar");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            resolve_executable_in(Path::new("unrar"), &[temp.path().to_path_buf()]),
            Some(link)
        );
    }
}
