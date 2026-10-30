//! EmuWiz-owned ScreenScraper dotenv file. Contents are plaintext to this
//! account and root; the operator must approve local and backup exposure.
//! Every operation retains a no-follow directory descriptor and rejects unsafe
//! ownership, permissions, links, or repository locations.

use std::ffi::CString;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use crate::screenscraper_secret_store::{StoreError, StoredCredentials};

const FILE_NAME: &str = "screenscraper.env";
const HEADER: &str = "# EmuWiz ScreenScraper credentials v1; data only, do not source\n";
const KEYS: [&str; 4] = [
    "EMUWIZ_SCREENSCRAPER_DEVELOPER_ID",
    "EMUWIZ_SCREENSCRAPER_DEVELOPER_PASSWORD",
    "EMUWIZ_SCREENSCRAPER_USER_ID",
    "EMUWIZ_SCREENSCRAPER_USER_PASSWORD",
];
const MAX_BYTES: usize = 20_000;
const TEMP_PREFIX: &str = ".screenscraper-env-";

/// Best-effort, side-effect-free detection of dotfiles management that could
/// copy a plaintext credentials directory into a repository or sync target.
/// Only metadata and directory listings are read; no repository tool runs and
/// no file content is read.
#[derive(Clone, Default)]
pub(super) struct DotfilesGuard {
    git_environment: bool,
    home: Option<PathBuf>,
    yadm_repositories: Vec<PathBuf>,
    chezmoi_source: Option<PathBuf>,
}

impl DotfilesGuard {
    pub fn from_process() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg_data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        Self::from_vars(
            home.as_deref(),
            xdg_data.as_deref(),
            std::env::var_os("GIT_DIR").as_deref(),
            std::env::var_os("GIT_WORK_TREE").as_deref(),
        )
    }

    pub fn from_vars(
        home: Option<&Path>,
        xdg_data_home: Option<&Path>,
        git_dir: Option<&OsStr>,
        git_work_tree: Option<&OsStr>,
    ) -> Self {
        let set = |value: Option<&OsStr>| value.is_some_and(|value| !value.is_empty());
        let home = home
            .filter(|home| home.is_absolute())
            .map(Path::to_path_buf);
        let xdg_data = xdg_data_home
            .filter(|path| path.is_absolute())
            .map(Path::to_path_buf);
        let default_data = home.as_ref().map(|home| home.join(".local/share"));
        let mut yadm_repositories = Vec::new();
        for data in [xdg_data.clone(), default_data.clone()]
            .into_iter()
            .flatten()
        {
            yadm_repositories.push(data.join("yadm/repo.git"));
        }
        if let Some(home) = &home {
            yadm_repositories.push(home.join(".yadm/repo.git"));
        }
        Self {
            git_environment: set(git_dir) || set(git_work_tree),
            chezmoi_source: xdg_data.or(default_data).map(|data| data.join("chezmoi")),
            home,
            yadm_repositories,
        }
    }

    fn check(&self, directory: &Path) -> Result<(), StoreError> {
        if self.git_environment {
            return Err(StoreError::Dotfiles);
        }
        let Some(home) = &self.home else {
            return Ok(());
        };
        let Ok(relative) = directory.strip_prefix(home) else {
            return Ok(());
        };
        if self
            .yadm_repositories
            .iter()
            .any(|repo| repo.symlink_metadata().is_ok())
        {
            return Err(StoreError::Dotfiles);
        }
        if home_has_bare_repository(home) {
            return Err(StoreError::Dotfiles);
        }
        if let Some(source) = &self.chezmoi_source
            && chezmoi_manages(source, relative)
        {
            return Err(StoreError::Dotfiles);
        }
        Ok(())
    }
}

fn home_has_bare_repository(home: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(home) else {
        return false;
    };
    entries.flatten().take(10_000).any(|entry| {
        entry.file_name().as_bytes().starts_with(b".")
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && {
                let path = entry.path();
                path.join("HEAD").is_file()
                    && path.join("objects").is_dir()
                    && path.join("refs").is_dir()
            }
    })
}

fn chezmoi_target_name(name: &OsStr) -> Option<String> {
    let mut name = name.to_str()?;
    const ATTRIBUTES: [&str; 8] = [
        "exact_",
        "private_",
        "readonly_",
        "empty_",
        "executable_",
        "symlink_",
        "external_",
        "remove_",
    ];
    while let Some(rest) = ATTRIBUTES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
    {
        name = rest;
    }
    Some(match name.strip_prefix("dot_") {
        Some(rest) => format!(".{rest}"),
        None => name.to_owned(),
    })
}

/// True when the chezmoi source state has an entry for the config root (the
/// first two components below HOME) of `relative`, or all of it if shorter.
fn chezmoi_manages(source: &Path, relative: &Path) -> bool {
    if source.symlink_metadata().is_err() {
        return false;
    }
    let components: Vec<_> = relative.components().collect();
    let needed = components.len().min(2);
    let mut current = source.to_path_buf();
    for (index, component) in components.iter().enumerate().take(needed) {
        let Component::Normal(wanted) = component else {
            return false;
        };
        let Ok(entries) = std::fs::read_dir(&current) else {
            return false;
        };
        let found = entries
            .flatten()
            .take(10_000)
            .find(|entry| chezmoi_target_name(&entry.file_name()).as_deref() == wanted.to_str());
        let Some(found) = found else {
            return false;
        };
        if index + 1 == needed {
            return true;
        }
        current = found.path();
    }
    false
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum Fault {
    None,
    BeforePublish,
    AfterPublish,
}

pub(super) struct RestrictedFileStore {
    directory: PathBuf,
    create_trailing_components: usize,
    guard: DotfilesGuard,
}

impl RestrictedFileStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            create_trailing_components: 1,
            guard: Self::process_guard(),
        }
    }

    /// `<config-root>/secrets`: the config root (if missing) and `secrets`
    /// are created; an existing config root keeps its permissions.
    pub fn managed_default(directory: PathBuf) -> Self {
        Self {
            directory,
            create_trailing_components: 2,
            guard: Self::process_guard(),
        }
    }

    // Unit tests build stores under temporary directories and must not depend
    // on the developer's real dotfiles setup; guard tests set it explicitly.
    fn process_guard() -> DotfilesGuard {
        if cfg!(test) {
            DotfilesGuard::default()
        } else {
            DotfilesGuard::from_process()
        }
    }

    #[cfg(test)]
    fn with_guard(mut self, guard: DotfilesGuard) -> Self {
        self.guard = guard;
        self
    }

    fn open(&self, create: usize, enforce_dotfiles: bool) -> Result<File, StoreError> {
        if enforce_dotfiles {
            self.guard.check(&self.directory)?;
        }
        open_private_directory(&self.directory, create, enforce_dotfiles)
    }

    /// Number of crash-left temporary files (never reads their content).
    pub fn leftover_temporary_files(&self) -> usize {
        self.open(0, false)
            .ok()
            .and_then(|directory| scan_temporary_files(&directory, false).ok())
            .unwrap_or(0)
    }

    pub fn load(&self) -> Result<Option<StoredCredentials>, StoreError> {
        let directory = match self.open(0, true) {
            Ok(directory) => directory,
            Err(StoreError::Unavailable) => return Ok(None),
            Err(error) => return Err(error),
        };
        lock_directory(&directory, libc::LOCK_SH)?;
        read_record(&directory)
    }

    pub fn save(&self, record: &StoredCredentials) -> Result<(), StoreError> {
        self.save_inner(record, Fault::None)
    }

    #[cfg(test)]
    pub fn save_failing_before_publish(
        &self,
        record: &StoredCredentials,
    ) -> Result<(), StoreError> {
        self.save_inner(record, Fault::BeforePublish)
    }

    #[cfg(test)]
    pub fn save_failing_after_publish(&self, record: &StoredCredentials) -> Result<(), StoreError> {
        self.save_inner(record, Fault::AfterPublish)
    }

    fn save_inner(&self, record: &StoredCredentials, fault: Fault) -> Result<(), StoreError> {
        let directory = self.open(self.create_trailing_components, true)?;
        lock_directory(&directory, libc::LOCK_EX)?;
        // Writers hold this lock, so any existing temporary file is stale
        // plaintext from a crashed save; remove it before creating ours.
        scan_temporary_files(&directory, true)?;
        let previous = open_record(&directory)?;
        let previous_value = previous.as_ref().map(decode_file).transpose()?;
        let previous_identity = previous
            .as_ref()
            .map(|file| file.metadata().map(|m| (m.dev(), m.ino())))
            .transpose()
            .map_err(|_| StoreError::Failed)?;
        let payload = encode_record(record)?;
        if payload.len() > MAX_BYTES {
            return Err(StoreError::TooLarge);
        }
        let pinned_directory = pinned_path(&directory);
        let mut temporary = tempfile::Builder::new()
            .prefix(".screenscraper-env-")
            .tempfile_in(&pinned_directory)
            .map_err(|_| StoreError::Failed)?;
        check_private_file(temporary.as_file())?;
        temporary
            .write_all(&payload)
            .map_err(|_| StoreError::Failed)?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| StoreError::Failed)?;
        if fault == Fault::BeforePublish {
            return Err(StoreError::Failed);
        }
        let current = open_record(&directory)?;
        let current_identity = current
            .as_ref()
            .map(|file| file.metadata().map(|m| (m.dev(), m.ino())))
            .transpose()
            .map_err(|_| StoreError::Failed)?;
        let current_value = current.as_ref().map(decode_file).transpose()?;
        if current_identity != previous_identity || current_value != previous_value {
            return Err(StoreError::Failed);
        }
        if previous_identity.is_some() {
            temporary
                .persist(pinned_directory.join(FILE_NAME))
                .map_err(|_| StoreError::Failed)?;
        } else {
            temporary
                .persist_noclobber(pinned_directory.join(FILE_NAME))
                .map_err(|_| StoreError::Failed)?;
        }
        // The rename above published the record. From here a failure means
        // "written but unverified", never "not saved".
        if fault == Fault::AfterPublish || directory.sync_all().is_err() {
            return Err(StoreError::PublishedUnverified);
        }
        match read_record(&directory) {
            Ok(Some(saved)) if saved == *record => Ok(()),
            _ => Err(StoreError::PublishedUnverified),
        }
    }

    pub fn forget(&self) -> Result<(), StoreError> {
        // Removal is never blocked by the dotfiles guard.
        let directory = match self.open(0, false) {
            Ok(directory) => directory,
            Err(StoreError::Unavailable) => return Ok(()),
            Err(error) => return Err(error),
        };
        lock_directory(&directory, libc::LOCK_EX)?;
        let record_result = remove_record(&directory);
        let remaining = scan_temporary_files(&directory, true);
        if remaining.is_ok() {
            let _ = directory.sync_all();
        }
        record_result?;
        match remaining {
            Ok(0) => Ok(()),
            _ => Err(StoreError::Residual),
        }
    }
}

fn remove_record(directory: &File) -> Result<(), StoreError> {
    let Some(file) = open_record(directory)? else {
        return Ok(());
    };
    require_emuwiz_file(&file)?;
    let name = CString::new(FILE_NAME).expect("fixed file name");
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(StoreError::Failed);
    }
    directory.sync_all().map_err(|_| StoreError::Failed)?;
    if open_record(directory)?.is_some() {
        return Err(StoreError::Failed);
    }
    Ok(())
}

/// Lists `.screenscraper-env-*` entries without following links or reading
/// content. With `remove`, deletes those that are regular, owner-only (0600),
/// single-link files owned by this user. Returns how many remain.
fn scan_temporary_files(directory: &File, remove: bool) -> Result<usize, StoreError> {
    let entries = std::fs::read_dir(pinned_path(directory)).map_err(|_| StoreError::Failed)?;
    let mut remaining = 0;
    for entry in entries {
        let entry = entry.map_err(|_| StoreError::Failed)?;
        let file_name = entry.file_name();
        if !file_name.as_bytes().starts_with(TEMP_PREFIX.as_bytes()) {
            continue;
        }
        let name = CString::new(file_name.as_bytes()).map_err(|_| StoreError::Failed)?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let found = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if found != 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
                continue;
            }
            remaining += 1;
            continue;
        }
        let safe = stat.st_mode & libc::S_IFMT == libc::S_IFREG
            && stat.st_uid == unsafe { libc::geteuid() }
            && stat.st_mode & 0o7777 == 0o600
            && stat.st_nlink == 1;
        if remove && safe && unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } == 0
        {
            continue;
        }
        remaining += 1;
    }
    Ok(remaining)
}

fn lock_directory(directory: &File, operation: libc::c_int) -> Result<(), StoreError> {
    if unsafe { libc::flock(directory.as_raw_fd(), operation) } != 0 {
        return Err(StoreError::Failed);
    }
    Ok(())
}

fn open_private_directory(
    path: &Path,
    create_trailing_components: usize,
    reject_git_worktrees: bool,
) -> Result<File, StoreError> {
    if !path.is_absolute() {
        return Err(StoreError::Insecure);
    }
    let mut directory = File::open("/").map_err(|_| StoreError::Failed)?;
    let mut count = 0;
    let total_components = path
        .components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count();
    for component in path.components() {
        let Component::Normal(name) = component else {
            if component == Component::RootDir {
                continue;
            }
            return Err(StoreError::Insecure);
        };
        count += 1;
        let name = CString::new(name.as_bytes()).map_err(|_| StoreError::Insecure)?;
        let mut fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0
            && create_trailing_components > 0
            && count > total_components.saturating_sub(create_trailing_components)
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT)
        {
            let parent_metadata = directory.metadata().map_err(|_| StoreError::Failed)?;
            if parent_metadata.uid() != unsafe { libc::geteuid() } {
                return Err(StoreError::Insecure);
            }
            let created = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
            if created != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
            {
                return Err(StoreError::Failed);
            }
            fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if created == 0 {
                directory.sync_all().map_err(|_| StoreError::Failed)?;
            }
        }
        if fd < 0 {
            return Err(match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::ENOENT) => StoreError::Unavailable,
                Some(libc::ELOOP | libc::ENOTDIR) => StoreError::Insecure,
                _ => StoreError::Failed,
            });
        }
        directory = unsafe { File::from_raw_fd(fd) };
        if reject_git_worktrees && is_git_worktree_directory(&directory)? {
            return Err(StoreError::Dotfiles);
        }
    }
    if count == 0 {
        return Err(StoreError::Insecure);
    }
    let metadata = directory.metadata().map_err(|_| StoreError::Failed)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(StoreError::Insecure);
    }
    Ok(directory)
}

fn is_git_worktree_directory(directory: &File) -> Result<bool, StoreError> {
    let marker = pinned_path(directory).join(".git");
    let metadata = match marker.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(StoreError::Failed),
    };
    if metadata.file_type().is_symlink() || metadata.is_file() {
        return Ok(true);
    }
    if !metadata.is_dir() {
        return Err(StoreError::Insecure);
    }
    Ok(marker.join("HEAD").is_file() && marker.join("objects").is_dir())
}

fn open_record(directory: &File) -> Result<Option<File>, StoreError> {
    let name = CString::new(FILE_NAME).expect("fixed file name");
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ENOENT) => Ok(None),
            Some(libc::ELOOP) => Err(StoreError::Insecure),
            _ => Err(StoreError::Failed),
        };
    }
    let file = unsafe { File::from_raw_fd(fd) };
    check_private_file(&file)?;
    Ok(Some(file))
}

fn check_private_file(file: &File) -> Result<(), StoreError> {
    check_private_file_for_uid(file, unsafe { libc::geteuid() })
}

fn check_private_file_for_uid(file: &File, expected_uid: u32) -> Result<(), StoreError> {
    let metadata = file.metadata().map_err(|_| StoreError::Failed)?;
    if !metadata.is_file()
        || metadata.uid() != expected_uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > MAX_BYTES as u64
    {
        return Err(StoreError::Insecure);
    }
    Ok(())
}

fn require_emuwiz_file(file: &File) -> Result<(), StoreError> {
    decode_file(file).map(|_| ())
}

fn encode_record(record: &StoredCredentials) -> Result<Vec<u8>, StoreError> {
    let values = [
        &record.developer_id,
        &record.developer_password,
        &record.user_id,
        &record.user_password,
    ];
    let mut text = String::from(HEADER);
    for (key, value) in KEYS.into_iter().zip(values) {
        text.push_str(key);
        text.push('=');
        let quoted = serde_json::to_string(value).map_err(|_| StoreError::Failed)?;
        text.push_str(
            &quoted
                .replace('$', "\\u0024")
                .replace('`', "\\u0060")
                .replace('!', "\\u0021"),
        );
        text.push('\n');
    }
    Ok(text.into_bytes())
}

fn decode_record(bytes: &[u8]) -> Result<StoredCredentials, StoreError> {
    let text = std::str::from_utf8(bytes).map_err(|_| StoreError::InvalidRecord)?;
    let body = text.strip_prefix(HEADER).ok_or(StoreError::InvalidRecord)?;
    let mut lines = body.split('\n');
    let mut values = Vec::with_capacity(KEYS.len());
    for key in KEYS {
        let value = lines
            .next()
            .and_then(|line| line.strip_prefix(key))
            .and_then(|line| line.strip_prefix('='))
            .ok_or(StoreError::InvalidRecord)?;
        values.push(serde_json::from_str::<String>(value).map_err(|_| StoreError::InvalidRecord)?);
    }
    if lines.next() != Some("") || lines.next().is_some() {
        return Err(StoreError::InvalidRecord);
    }
    Ok(StoredCredentials {
        developer_id: values.remove(0),
        developer_password: values.remove(0),
        user_id: values.remove(0),
        user_password: values.remove(0),
    })
}

fn read_record(directory: &File) -> Result<Option<StoredCredentials>, StoreError> {
    let Some(file) = open_record(directory)? else {
        return Ok(None);
    };
    decode_file(&file).map(Some)
}

fn decode_file(file: &File) -> Result<StoredCredentials, StoreError> {
    let mut reader = file.try_clone().map_err(|_| StoreError::Failed)?;
    let before = reader.metadata().map_err(|_| StoreError::Failed)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut reader)
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| StoreError::Failed)?;
    let after = reader.metadata().map_err(|_| StoreError::Failed)?;
    if bytes.len() > MAX_BYTES
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || bytes.len() as u64 != before.len()
    {
        return Err(StoreError::Failed);
    }
    decode_record(&bytes)
}

fn pinned_path(directory: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn fixture() -> (tempfile::TempDir, PathBuf, RestrictedFileStore) {
        // Test HOME may be inside the checkout; the store must reject Git worktrees.
        let root = tempfile::Builder::new()
            .prefix("emuwiz-screenscraper-file-test-")
            .tempdir()
            .unwrap();
        let directory = root.path().join("secrets");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let store = RestrictedFileStore::new(directory.clone());
        (root, directory, store)
    }

    fn absent_fixture() -> (tempfile::TempDir, PathBuf, RestrictedFileStore) {
        let root = tempfile::Builder::new()
            .prefix("emuwiz-screenscraper-env-test-")
            .tempdir()
            .unwrap();
        let directory = root.path().join(".config/emuwiz");
        let store = RestrictedFileStore::managed_default(directory.clone());
        (root, directory, store)
    }

    fn record(dev_password: &str, user_password: &str) -> StoredCredentials {
        StoredCredentials {
            developer_id: "synthetic-dev-id".into(),
            developer_password: dev_password.into(),
            user_id: "synthetic-user-id".into(),
            user_password: user_password.into(),
        }
    }

    #[test]
    fn restart_update_and_forget_are_private_and_durable() {
        let (_root, directory, store) = fixture();
        let first = record("synthetic-dev-password", "synthetic-user-password");
        store.save(&first).unwrap();
        let path = directory.join(FILE_NAME);
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(
            RestrictedFileStore::new(directory.clone()).load().unwrap(),
            Some(first)
        );

        let updated = record("updated-dev-password", "updated-user-password");
        store.save(&updated).unwrap();
        let restarted = RestrictedFileStore::new(directory.clone());
        assert_eq!(restarted.load().unwrap(), Some(updated.clone()));
        assert_ne!(updated.developer_password, updated.user_password);
        assert!(!format!("{updated:?}").contains("updated-user-password"));
        restarted.forget().unwrap();
        assert_eq!(RestrictedFileStore::new(directory).load().unwrap(), None);
    }

    #[test]
    fn process_restart_recovers_updates_and_removal() {
        let (_root, directory, _store) = absent_fixture();
        for operation in [
            "save",
            "load",
            "update",
            "load_updated",
            "forget",
            "load_empty",
        ] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("screenscraper_restricted_file_store::tests::process_restart_child")
                .env("EMUWIZ_FILE_TEST_DIR", &directory)
                .env("EMUWIZ_FILE_TEST_OPERATION", operation)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "synthetic child failed at {operation}"
            );
        }
    }

    #[test]
    fn first_save_creates_private_secrets_directory_without_touching_config_root() {
        let root = tempfile::Builder::new()
            .prefix("emuwiz-screenscraper-env-test-")
            .tempdir()
            .unwrap();
        let config = root.path().join(".config");
        let emuwiz = config.join("emuwiz");
        fs::create_dir_all(&emuwiz).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&emuwiz, fs::Permissions::from_mode(0o755)).unwrap();
        let directory = emuwiz.join("secrets");
        let store = RestrictedFileStore::managed_default(directory.clone());
        assert_eq!(store.load().unwrap(), None);
        assert!(!directory.exists());
        let original = record("synthetic-dev-password", "synthetic-user-password");
        store.save(&original).unwrap();
        assert_eq!(fs::metadata(&config).unwrap().mode() & 0o7777, 0o755);
        assert_eq!(fs::metadata(&emuwiz).unwrap().mode() & 0o7777, 0o755);
        let directory_metadata = fs::metadata(&directory).unwrap();
        let file_metadata = fs::metadata(directory.join(FILE_NAME)).unwrap();
        assert_eq!(directory_metadata.mode() & 0o7777, 0o700);
        assert_eq!(file_metadata.mode() & 0o7777, 0o600);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        assert_eq!(
            RestrictedFileStore::new(directory).load().unwrap(),
            Some(original)
        );
        // A missing config root is created private as well.
        let fresh = root.path().join("fresh/emuwiz/secrets");
        fs::create_dir(root.path().join("fresh")).unwrap();
        RestrictedFileStore::managed_default(fresh.clone())
            .save(&record("a", "b"))
            .unwrap();
        assert_eq!(fs::metadata(&fresh).unwrap().mode() & 0o7777, 0o700);
    }

    #[test]
    fn published_record_with_failed_follow_up_is_reported_as_unverified_not_missing() {
        let (_root, directory, store) = fixture();
        let saved = record("dev", "user");
        assert_eq!(
            store.save_failing_after_publish(&saved),
            Err(StoreError::PublishedUnverified)
        );
        assert_eq!(store.load().unwrap(), Some(saved));
    }

    fn write_temp(directory: &Path, name: &str, mode: u32) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, b"synthetic-plaintext-not-read").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[test]
    fn forget_and_save_remove_safe_crash_left_temporary_files() {
        let (_root, directory, store) = fixture();
        let stale = write_temp(&directory, ".screenscraper-env-crashed1", 0o600);
        assert_eq!(store.leftover_temporary_files(), 1);
        // Loading reports but never deletes or reads the file.
        assert_eq!(store.load().unwrap(), None);
        assert!(stale.exists());
        store.forget().unwrap();
        assert!(!stale.exists());
        assert_eq!(store.leftover_temporary_files(), 0);

        let stale = write_temp(&directory, ".screenscraper-env-crashed2", 0o600);
        store.save(&record("dev", "user")).unwrap();
        assert!(!stale.exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let again = write_temp(&directory, ".screenscraper-env-crashed3", 0o600);
        store.forget().unwrap();
        assert!(!again.exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
    }

    #[test]
    fn unsafe_temporary_files_are_never_followed_and_forget_reports_residue() {
        let (root, directory, store) = fixture();
        let outside = root.path().join("outside");
        fs::write(&outside, b"synthetic-outside").unwrap();
        symlink(&outside, directory.join(".screenscraper-env-link")).unwrap();
        let loose = write_temp(&directory, ".screenscraper-env-loose", 0o644);
        let linked = write_temp(&directory, ".screenscraper-env-linked", 0o600);
        fs::hard_link(&linked, root.path().join("second-name")).unwrap();
        assert_eq!(store.forget(), Err(StoreError::Residual));
        assert!(loose.exists() && linked.exists());
        assert!(
            directory
                .join(".screenscraper-env-link")
                .symlink_metadata()
                .is_ok()
        );
        assert_eq!(fs::read(&outside).unwrap(), b"synthetic-outside");
        assert_eq!(store.leftover_temporary_files(), 3);
    }

    fn guard_home() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::Builder::new()
            .prefix("emuwiz-screenscraper-guard-test-")
            .tempdir()
            .unwrap();
        let directory = root.path().join(".config/emuwiz/secrets");
        fs::create_dir_all(&directory).unwrap();
        for path in [
            &root.path().join(".config"),
            &root.path().join(".config/emuwiz"),
            &directory,
        ] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        (root, directory)
    }

    fn guarded(home: &Path, directory: &Path) -> RestrictedFileStore {
        RestrictedFileStore::new(directory.to_path_buf()).with_guard(DotfilesGuard::from_vars(
            Some(home),
            None,
            None,
            None,
        ))
    }

    #[test]
    fn dotfiles_management_refuses_plaintext_but_allows_forget() {
        let (root, directory) = guard_home();
        let home = root.path();
        let data = record("dev", "user");
        guarded(home, &directory).save(&data).unwrap();

        // GIT_DIR / GIT_WORK_TREE in the environment.
        for (git_dir, work_tree) in [(Some("/x"), None), (None, Some("/x"))] {
            let guard = DotfilesGuard::from_vars(
                Some(home),
                None,
                git_dir.map(OsStr::new),
                work_tree.map(OsStr::new),
            );
            let store = RestrictedFileStore::new(directory.clone()).with_guard(guard);
            assert_eq!(store.load(), Err(StoreError::Dotfiles));
            assert_eq!(store.save(&data), Err(StoreError::Dotfiles));
        }
        let empty = DotfilesGuard::from_vars(Some(home), None, Some(OsStr::new("")), None);
        assert!(
            RestrictedFileStore::new(directory.clone())
                .with_guard(empty)
                .load()
                .is_ok()
        );

        // yadm bare repository.
        let yadm = home.join(".local/share/yadm/repo.git");
        fs::create_dir_all(&yadm).unwrap();
        let store = guarded(home, &directory);
        assert_eq!(store.load(), Err(StoreError::Dotfiles));
        assert_eq!(store.save(&data), Err(StoreError::Dotfiles));
        fs::remove_dir_all(home.join(".local")).unwrap();
        assert!(store.load().is_ok());

        // Arbitrary bare repository under HOME (git --git-dir=$HOME/.cfg).
        let bare = home.join(".cfg");
        fs::create_dir_all(bare.join("objects")).unwrap();
        fs::create_dir_all(bare.join("refs")).unwrap();
        fs::write(bare.join("HEAD"), b"").unwrap();
        assert_eq!(store.load(), Err(StoreError::Dotfiles));
        fs::remove_dir_all(&bare).unwrap();

        // chezmoi source state managing the EmuWiz config directory.
        let source = home.join(".local/share/chezmoi/private_dot_config/exact_private_emuwiz");
        fs::create_dir_all(&source).unwrap();
        assert_eq!(store.load(), Err(StoreError::Dotfiles));
        fs::remove_dir_all(home.join(".local")).unwrap();
        let unrelated = home.join(".local/share/chezmoi/dot_config/other");
        fs::create_dir_all(unrelated).unwrap();
        assert!(store.load().is_ok());
        fs::remove_dir_all(home.join(".local")).unwrap();

        // Git work tree in any ancestor, including HOME itself.
        fs::create_dir_all(home.join(".git/objects")).unwrap();
        fs::write(home.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        assert_eq!(store.load(), Err(StoreError::Dotfiles));
        assert_eq!(store.save(&data), Err(StoreError::Dotfiles));
        // Removing existing credentials stays possible.
        store.forget().unwrap();
        assert!(!directory.join(FILE_NAME).exists());
    }

    #[test]
    fn unrelated_env_is_never_imported_overwritten_or_removed() {
        let (_root, directory, store) = fixture();
        let path = directory.join(FILE_NAME);
        let unrelated = b"SCREENSCRAPER_USER=other-app-value\n";
        fs::write(&path, unrelated).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.load(), Err(StoreError::InvalidRecord));
        assert_eq!(
            store.save(&record("dev", "user")),
            Err(StoreError::InvalidRecord)
        );
        assert_eq!(store.forget(), Err(StoreError::InvalidRecord));
        assert!(fs::read(path).unwrap() == unrelated);
    }

    #[test]
    fn quoted_special_values_round_trip_without_shell_evaluation() {
        let (_root, directory, store) = fixture();
        let original = StoredCredentials {
            developer_id: "developer id with spaces".into(),
            developer_password: "synthetic \"quoted\" $HOME $(command) `command` \\ newline\nnext"
                .into(),
            user_id: "synthetic-user-id".into(),
            user_password: "synthetic # = ' \\ \t 🔒".into(),
        };
        store.save(&original).unwrap();
        let saved = fs::read(directory.join(FILE_NAME)).unwrap();
        assert!(!saved.contains(&b'$'));
        assert!(!saved.contains(&b'`'));
        assert_eq!(
            RestrictedFileStore::new(directory.clone()).load().unwrap(),
            Some(original)
        );
        let path = directory.join(FILE_NAME);
        let mut malformed = String::from(HEADER);
        malformed.push_str("EMUWIZ_SCREENSCRAPER_DEVELOPER_ID=\"valid\"\n");
        malformed.push_str("EMUWIZ_SCREENSCRAPER_DEVELOPER_PASSWORD=$(command)\n");
        fs::write(&path, malformed).unwrap();
        assert_eq!(store.load(), Err(StoreError::InvalidRecord));
        assert_eq!(
            store.save(&record("new-dev", "new-user")),
            Err(StoreError::InvalidRecord)
        );
        assert_eq!(store.forget(), Err(StoreError::InvalidRecord));
        assert!(path.exists());
    }

    #[test]
    fn process_restart_child() {
        let Some(directory) = std::env::var_os("EMUWIZ_FILE_TEST_DIR") else {
            return;
        };
        let operation = std::env::var("EMUWIZ_FILE_TEST_OPERATION").unwrap();
        let store = RestrictedFileStore::managed_default(PathBuf::from(directory));
        let initial = record("synthetic-dev-password", "synthetic-user-password");
        let updated = record("synthetic-dev-password", "updated-user-password");
        match operation.as_str() {
            "save" => store.save(&initial).unwrap(),
            "load" => assert_eq!(store.load().unwrap(), Some(initial)),
            "update" => store.save(&updated).unwrap(),
            "load_updated" => assert_eq!(store.load().unwrap(), Some(updated)),
            "forget" => store.forget().unwrap(),
            "load_empty" => assert_eq!(store.load().unwrap(), None),
            _ => panic!("unknown synthetic test operation"),
        }
    }

    #[test]
    fn insecure_permissions_and_ownership_are_refused() {
        let (_root, directory, store) = fixture();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            store.save(&record("dev", "user")),
            Err(StoreError::Insecure)
        );
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        store.save(&record("dev", "user")).unwrap();
        let path = directory.join(FILE_NAME);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(store.load(), Err(StoreError::Insecure));
        assert_eq!(
            store.save(&record("new-dev", "new-user")),
            Err(StoreError::Insecure)
        );
        assert_eq!(store.forget(), Err(StoreError::Insecure));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let file = File::open(&path).unwrap();
        assert_eq!(
            check_private_file_for_uid(&file, unsafe { libc::geteuid() } + 1),
            Err(StoreError::Insecure)
        );
    }

    #[test]
    fn linked_paths_and_repository_locations_are_refused() {
        let (root, directory, store) = fixture();
        let linked_directory = root.path().join("linked-secrets");
        symlink(&directory, &linked_directory).unwrap();
        assert_eq!(
            RestrictedFileStore::new(linked_directory).load(),
            Err(StoreError::Insecure)
        );
        let linked_store = RestrictedFileStore::new(root.path().join("linked-secrets"));
        assert_eq!(
            linked_store.save(&record("dev", "user")),
            Err(StoreError::Insecure)
        );

        let target = root.path().join("target");
        fs::write(&target, b"synthetic-outside-value").unwrap();
        symlink(&target, directory.join(FILE_NAME)).unwrap();
        assert_eq!(store.load(), Err(StoreError::Insecure));
        assert_eq!(store.forget(), Err(StoreError::Insecure));
        assert_eq!(
            store.save(&record("dev", "user")),
            Err(StoreError::Insecure)
        );
        assert!(fs::read(&target).unwrap() == b"synthetic-outside-value");
        fs::remove_file(directory.join(FILE_NAME)).unwrap();

        fs::write(root.path().join(".git"), b"").unwrap();
        assert_eq!(
            store.save(&record("dev", "user")),
            Err(StoreError::Dotfiles)
        );
    }

    #[test]
    fn failed_publish_keeps_the_previous_record_and_removes_temporary_file() {
        let (_root, directory, store) = fixture();
        let first = record("original-dev", "original-user");
        store.save(&first).unwrap();
        assert_eq!(
            store.save_failing_before_publish(&record("new-dev", "new-user")),
            Err(StoreError::Failed)
        );
        assert_eq!(store.load().unwrap(), Some(first));
        assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
    }

    #[test]
    fn save_waits_for_directory_lock_before_publishing() {
        let (_root, directory, store) = fixture();
        let pinned = open_private_directory(&directory, 0, true).unwrap();
        lock_directory(&pinned, libc::LOCK_EX).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender.send(store.save(&record("dev", "user"))).unwrap();
        });
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err()
        );
        assert!(!directory.join(FILE_NAME).exists());
        drop(pinned);
        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            Ok(())
        );
        worker.join().unwrap();
    }

    #[test]
    fn missing_or_relative_directory_never_falls_back() {
        let root = tempfile::Builder::new()
            .prefix("emuwiz-screenscraper-file-test-")
            .tempdir()
            .unwrap();
        assert_eq!(
            RestrictedFileStore::new(root.path().join("missing")).load(),
            Ok(None)
        );
        assert_eq!(
            RestrictedFileStore::new(PathBuf::from("relative")).load(),
            Err(StoreError::Insecure)
        );
        let absent_home = root.path().join("missing-home/.config/emuwiz");
        assert_eq!(
            RestrictedFileStore::managed_default(absent_home.clone()).save(&record("dev", "user")),
            Err(StoreError::Unavailable)
        );
        assert!(!root.path().join("missing-home").exists());
    }
}
