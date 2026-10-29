//! EmuWiz-owned ScreenScraper dotenv file. Contents are plaintext to this
//! account and root; the operator must approve local and backup exposure.
//! Every operation retains a no-follow directory descriptor and rejects unsafe
//! ownership, permissions, links, or repository locations.

use std::ffi::CString;
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

pub(super) struct RestrictedFileStore {
    directory: PathBuf,
    create_trailing_components: usize,
}

impl RestrictedFileStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            create_trailing_components: 1,
        }
    }

    pub fn managed_default(directory: PathBuf) -> Self {
        Self {
            directory,
            create_trailing_components: 2,
        }
    }

    pub fn load(&self) -> Result<Option<StoredCredentials>, StoreError> {
        let directory = match open_private_directory(&self.directory, 0) {
            Ok(directory) => directory,
            Err(StoreError::Unavailable) => return Ok(None),
            Err(error) => return Err(error),
        };
        lock_directory(&directory, libc::LOCK_SH)?;
        read_record(&directory)
    }

    pub fn save(&self, record: &StoredCredentials) -> Result<(), StoreError> {
        self.save_inner(record, false)
    }

    #[cfg(test)]
    pub fn save_failing_before_publish(
        &self,
        record: &StoredCredentials,
    ) -> Result<(), StoreError> {
        self.save_inner(record, true)
    }

    fn save_inner(
        &self,
        record: &StoredCredentials,
        fail_before_publish: bool,
    ) -> Result<(), StoreError> {
        let directory = open_private_directory(&self.directory, self.create_trailing_components)?;
        lock_directory(&directory, libc::LOCK_EX)?;
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
        if fail_before_publish {
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
        directory.sync_all().map_err(|_| StoreError::Failed)?;
        if read_record(&directory)? != Some(record.clone()) {
            return Err(StoreError::Failed);
        }
        Ok(())
    }

    pub fn forget(&self) -> Result<(), StoreError> {
        let directory = match open_private_directory(&self.directory, 0) {
            Ok(directory) => directory,
            Err(StoreError::Unavailable) => return Ok(()),
            Err(error) => return Err(error),
        };
        lock_directory(&directory, libc::LOCK_EX)?;
        let Some(file) = open_record(&directory)? else {
            return Ok(());
        };
        require_emuwiz_file(&file)?;
        let name = CString::new(FILE_NAME).expect("fixed file name");
        if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(StoreError::Failed);
        }
        directory.sync_all().map_err(|_| StoreError::Failed)?;
        if open_record(&directory)?.is_some() {
            return Err(StoreError::Failed);
        }
        Ok(())
    }
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
        if is_git_worktree_directory(&directory)? {
            return Err(StoreError::Insecure);
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
    fn first_save_creates_private_config_directory_and_env() {
        let (_root, directory, store) = absent_fixture();
        assert_eq!(store.load().unwrap(), None);
        assert!(!directory.exists());
        let original = record("synthetic-dev-password", "synthetic-user-password");
        store.save(&original).unwrap();
        let config_metadata = fs::metadata(directory.parent().unwrap()).unwrap();
        let directory_metadata = fs::metadata(&directory).unwrap();
        let file_metadata = fs::metadata(directory.join(FILE_NAME)).unwrap();
        assert_eq!(config_metadata.mode() & 0o7777, 0o700);
        assert_eq!(directory_metadata.mode() & 0o7777, 0o700);
        assert_eq!(directory_metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(file_metadata.mode() & 0o7777, 0o600);
        assert_eq!(file_metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        assert_eq!(
            RestrictedFileStore::new(directory).load().unwrap(),
            Some(original)
        );
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
            Err(StoreError::Insecure)
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
        let pinned = open_private_directory(&directory, 0).unwrap();
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
