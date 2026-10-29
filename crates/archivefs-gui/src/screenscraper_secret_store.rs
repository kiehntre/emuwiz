//! Optional ScreenScraper credentials in the user's Secret Service or an
//! EmuWiz-owned restricted file. No credential is passed in arguments
//! or included in errors.

use std::io::Write;
#[cfg(any(test, target_os = "linux"))]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[cfg(target_os = "linux")]
use crate::screenscraper_restricted_file_store::RestrictedFileStore;
use serde::{Deserialize, Serialize};

const ATTRS: [&str; 4] = ["application", "EmuWiz", "provider", "ScreenScraper"];
// `secret-tool store` reads at most 8192 bytes from stdin. Keep a margin so
// successful exit cannot conceal a truncated JSON record.
const MAX_STORED_BYTES: usize = 8191;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredCredentials {
    pub developer_id: String,
    pub developer_password: String,
    pub user_id: String,
    pub user_password: String,
}

impl std::fmt::Debug for StoredCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredCredentials(redacted)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StoreError {
    Unavailable,
    Failed,
    InvalidRecord,
    TooLarge,
    Insecure,
}

/// Uses the EmuWiz-owned file if present, or when `secret-tool` is absent.
/// Existing Secret Service users keep that backend; a failing service never
/// causes a plaintext fallback.
pub(super) struct SecretStore {
    program: PathBuf,
    requires_session_bus: bool,
    #[cfg(target_os = "linux")]
    restricted_file: Option<RestrictedFileStore>,
    #[cfg(not(target_os = "linux"))]
    unsupported_file: bool,
}

impl SecretStore {
    pub fn system() -> Self {
        #[cfg(target_os = "linux")]
        let restricted_file = selected_file_directory(Path::new("/usr/bin/secret-tool").is_file())
            .map(RestrictedFileStore::new);
        #[cfg(not(target_os = "linux"))]
        let unsupported_file = std::env::var_os("EMUWIZ_SCREENSCRAPER_SECRETS_DIR").is_some();
        Self {
            program: PathBuf::from("/usr/bin/secret-tool"),
            requires_session_bus: true,
            #[cfg(target_os = "linux")]
            restricted_file,
            #[cfg(not(target_os = "linux"))]
            unsupported_file,
        }
    }

    #[cfg(test)]
    pub(super) fn new(program: &Path) -> Self {
        Self {
            program: program.to_owned(),
            requires_session_bus: false,
            #[cfg(target_os = "linux")]
            restricted_file: None,
            #[cfg(not(target_os = "linux"))]
            unsupported_file: false,
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn restricted_file_for_test(directory: PathBuf) -> Self {
        Self {
            program: PathBuf::new(),
            requires_session_bus: false,
            restricted_file: Some(RestrictedFileStore::new(directory)),
        }
    }

    pub fn uses_restricted_file(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.restricted_file.is_some()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    pub fn available(&self) -> bool {
        #[cfg(not(target_os = "linux"))]
        if self.unsupported_file {
            return false;
        }
        if self.uses_restricted_file() {
            return true;
        }
        self.program.is_file()
            && (!self.requires_session_bus
                || std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some())
    }

    pub fn load(&self) -> Result<Option<StoredCredentials>, StoreError> {
        #[cfg(not(target_os = "linux"))]
        if self.unsupported_file {
            return Err(StoreError::Unavailable);
        }
        #[cfg(target_os = "linux")]
        if let Some(store) = &self.restricted_file {
            return store.load();
        }
        if !self.available() {
            return Err(StoreError::Unavailable);
        }
        let output = Command::new(&self.program)
            .arg("lookup")
            .args(ATTRS)
            .output()
            .map_err(|_| StoreError::Unavailable)?;
        if !output.status.success() {
            // `secret-tool` returns 1 without diagnostics for a missing item.
            // Service errors also return nonzero, but print a diagnostic.
            // Inspect only its presence; never expose the text.
            return if output.status.code() == Some(1) && output.stderr.is_empty() {
                Ok(None)
            } else {
                Err(StoreError::Failed)
            };
        }
        if output.stdout.len() > 20_000 {
            return Err(StoreError::InvalidRecord);
        }
        let record: StoredCredentials =
            serde_json::from_slice(&output.stdout).map_err(|_| StoreError::InvalidRecord)?;
        Ok(Some(record))
    }

    pub fn save(&self, record: &StoredCredentials) -> Result<(), StoreError> {
        #[cfg(not(target_os = "linux"))]
        if self.unsupported_file {
            return Err(StoreError::Unavailable);
        }
        #[cfg(target_os = "linux")]
        if let Some(store) = &self.restricted_file {
            return store.save(record);
        }
        if !self.available() {
            return Err(StoreError::Unavailable);
        }
        let payload = serde_json::to_vec(record).map_err(|_| StoreError::Failed)?;
        if payload.len() > MAX_STORED_BYTES {
            return Err(StoreError::TooLarge);
        }
        let mut child = Command::new(&self.program)
            .arg("store")
            .arg("--label=EmuWiz ScreenScraper credentials")
            .args(ATTRS)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| StoreError::Unavailable)?;
        let write_result = child
            .stdin
            .take()
            .ok_or(StoreError::Failed)?
            .write_all(&payload);
        let status = child.wait().map_err(|_| StoreError::Failed)?;
        if write_result.is_err() || !status.success() {
            return Err(StoreError::Failed);
        }
        match self.load() {
            Ok(Some(saved)) if saved == *record => Ok(()),
            _ => Err(StoreError::Failed),
        }
    }

    pub fn forget(&self) -> Result<(), StoreError> {
        #[cfg(not(target_os = "linux"))]
        if self.unsupported_file {
            return Err(StoreError::Unavailable);
        }
        #[cfg(target_os = "linux")]
        if let Some(store) = &self.restricted_file {
            return store.forget();
        }
        if !self.available() {
            return Err(StoreError::Unavailable);
        }
        let status = Command::new(&self.program)
            .arg("clear")
            .args(ATTRS)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| StoreError::Unavailable)?;
        if !status.success() {
            return Err(StoreError::Failed);
        }
        match self.load() {
            Ok(None) => Ok(()),
            _ => Err(StoreError::Failed),
        }
    }
}

#[cfg(target_os = "linux")]
fn selected_file_directory(secret_tool_available: bool) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let override_dir = std::env::var_os("EMUWIZ_SCREENSCRAPER_SECRETS_DIR").map(PathBuf::from);
    selected_file_directory_for(home.as_deref(), override_dir, secret_tool_available)
}

#[cfg(target_os = "linux")]
fn selected_file_directory_for(
    home: Option<&Path>,
    override_dir: Option<PathBuf>,
    secret_tool_available: bool,
) -> Option<PathBuf> {
    if let Some(override_dir) = override_dir {
        return Some(override_dir);
    }
    let home = home?;
    if !home.is_absolute() {
        return None;
    }
    let directory = home.join(".config/emuwiz");
    let file = directory.join("screenscraper.env");
    let existing_file_or_error = match file.symlink_metadata() {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    };
    (existing_file_or_error || !secret_tool_available).then_some(directory)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_store() -> (tempfile::TempDir, SecretStore) {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("secret-tool");
        let record = root.path().join("record");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n store) cat > '{}' ;;\n lookup) cat '{}' 2>/dev/null ;;\n clear) rm -f '{}' ;;\nesac\n",
                record.display(), record.display(), record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        (root, SecretStore::new(&script))
    }

    #[test]
    fn credentials_survive_new_store_instance_then_can_be_removed() {
        let (_root, store) = fake_store();
        let record = StoredCredentials {
            developer_id: "synthetic-dev".into(),
            developer_password: "synthetic-dev-secret".into(),
            user_id: "synthetic-user".into(),
            user_password: "synthetic-user-secret".into(),
        };
        store.save(&record).unwrap();
        let restarted = SecretStore::new(&store.program);
        let loaded = restarted.load().unwrap().unwrap();
        assert_eq!(loaded.user_id, record.user_id);
        assert_eq!(loaded.user_password, record.user_password);
        assert!(!format!("{loaded:?}").contains("synthetic-user-secret"));
        restarted.forget().unwrap();
        assert!(restarted.load().unwrap().is_none());
    }

    #[test]
    fn missing_store_fails_closed() {
        let store = SecretStore::new(Path::new("/no-such-secret-tool"));
        assert_eq!(store.load().unwrap_err(), StoreError::Unavailable);
        let record = StoredCredentials {
            developer_id: "dev".into(),
            developer_password: "synthetic".into(),
            user_id: String::new(),
            user_password: String::new(),
        };
        assert_eq!(store.save(&record).unwrap_err(), StoreError::Unavailable);
    }

    #[test]
    fn malformed_secret_record_is_rejected_without_echo() {
        let (root, store) = fake_store();
        std::fs::write(root.path().join("record"), b"synthetic-secret-invalid-json").unwrap();
        let error = store.load().unwrap_err();
        assert_eq!(error, StoreError::InvalidRecord);
        assert!(!format!("{error:?}").contains("synthetic-secret"));
    }

    #[test]
    fn oversized_record_is_refused_before_secret_tool_can_truncate_it() {
        let (_root, store) = fake_store();
        let record = StoredCredentials {
            developer_id: "d".repeat(256),
            developer_password: "x".repeat(4096),
            user_id: "u".repeat(256),
            user_password: "y".repeat(4096),
        };
        assert_eq!(store.save(&record), Err(StoreError::TooLarge));
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn service_failure_after_clear_is_not_reported_as_verified_removal() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("secret-tool");
        std::fs::write(
            &script,
            "#!/bin/sh\ncase \"$1\" in\n lookup) echo service-unavailable >&2; exit 1 ;;\n clear) exit 0 ;;\nesac\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = SecretStore::new(&script);
        assert_eq!(store.load().unwrap_err(), StoreError::Failed);
        assert_eq!(store.forget().unwrap_err(), StoreError::Failed);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn saltbox_default_selects_managed_env_without_manual_environment_variable() {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".config/emuwiz");
        assert_eq!(
            selected_file_directory_for(Some(home.path()), None, false),
            Some(directory.clone())
        );
        assert_eq!(
            selected_file_directory_for(Some(home.path()), None, true),
            None
        );
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("screenscraper.env"), b"unrelated").unwrap();
        assert_eq!(
            selected_file_directory_for(Some(home.path()), None, true),
            Some(directory)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires an explicitly configured private synthetic secrets directory"]
    fn operator_selected_file_backend_does_not_use_secret_service_fallback() {
        assert!(std::env::var_os("EMUWIZ_SCREENSCRAPER_SECRETS_DIR").is_some());
        let store = SecretStore::system();
        assert!(store.uses_restricted_file());
        let record = StoredCredentials {
            developer_id: "synthetic-dev".into(),
            developer_password: "synthetic-dev-password".into(),
            user_id: "synthetic-user".into(),
            user_password: "synthetic-user-password".into(),
        };
        store.save(&record).unwrap();
        assert_eq!(SecretStore::system().load().unwrap(), Some(record));
        SecretStore::system().forget().unwrap();
        assert_eq!(SecretStore::system().load().unwrap(), None);
    }
}
