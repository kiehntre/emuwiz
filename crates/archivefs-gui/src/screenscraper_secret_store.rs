//! Optional ScreenScraper credentials in the desktop Secret Service.
//! No credential is passed in arguments, written to a file, or included in errors.

use std::io::Write;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

const ATTRS: [&str; 4] = ["application", "EmuWiz", "provider", "ScreenScraper"];

#[derive(Clone, Serialize, Deserialize)]
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
}

/// Uses the system's Secret Service through `secret-tool` when installed.
/// A missing or failing service never causes a plaintext fallback.
pub(super) struct SecretStore {
    program: PathBuf,
    requires_session_bus: bool,
}

impl SecretStore {
    pub fn system() -> Self {
        Self {
            program: PathBuf::from("/usr/bin/secret-tool"),
            requires_session_bus: true,
        }
    }

    #[cfg(test)]
    pub(super) fn new(program: &Path) -> Self {
        Self {
            program: program.to_owned(),
            requires_session_bus: false,
        }
    }

    pub fn available(&self) -> bool {
        self.program.is_file()
            && (!self.requires_session_bus
                || std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some())
    }

    pub fn load(&self) -> Result<Option<StoredCredentials>, StoreError> {
        if !self.available() {
            return Err(StoreError::Unavailable);
        }
        let output = Command::new(&self.program)
            .arg("lookup")
            .args(ATTRS)
            .stderr(Stdio::null())
            .output()
            .map_err(|_| StoreError::Unavailable)?;
        if !output.status.success() {
            // Secret Service lookup uses exit 1 for both missing entries and
            // service failures. A failed lookup does not authorize persistence.
            return Ok(None);
        }
        if output.stdout.len() > 20_000 {
            return Err(StoreError::InvalidRecord);
        }
        let record: StoredCredentials =
            serde_json::from_slice(&output.stdout).map_err(|_| StoreError::InvalidRecord)?;
        Ok(Some(record))
    }

    pub fn save(&self, record: &StoredCredentials) -> Result<(), StoreError> {
        if !self.available() {
            return Err(StoreError::Unavailable);
        }
        let payload = serde_json::to_vec(record).map_err(|_| StoreError::Failed)?;
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
        Ok(())
    }

    pub fn forget(&self) -> Result<(), StoreError> {
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
        if status.success() {
            Ok(())
        } else {
            Err(StoreError::Failed)
        }
    }
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
                "#!/bin/sh\ncase \"$1\" in\n store) cat > '{}' ;;\n lookup) cat '{}' ;;\n clear) rm -f '{}' ;;\nesac\n",
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
}
