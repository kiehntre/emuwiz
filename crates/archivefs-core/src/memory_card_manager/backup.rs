//! Verified backup of a memory-card file.
//!
//! A backup is a separate copy in an EmuWiz-owned folder with a small JSON
//! record beside it. It is kept only if the copy reads back with the same
//! SHA-256 as the card, and the card still has that SHA-256 after the copy
//! (so a card an emulator was writing during the copy is never kept as a
//! "backup"). The card itself is only ever read.

use super::{
    CardError, CardFormat, io_error, read_card_bytes, report_bytes, sha256_hex, write_new_file,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The record written beside each backup (`<backup>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardBackup {
    pub format_version: u32,
    pub source: PathBuf,
    pub backup_file: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_unix_seconds: u64,
    /// What the card was when it was backed up, for display.
    pub card_kind: String,
    pub save_names: Vec<String>,
}

impl CardBackup {
    pub fn record_file(&self) -> PathBuf {
        record_path(&self.backup_file)
    }
}

fn record_path(backup_file: &Path) -> PathBuf {
    let mut name = backup_file.as_os_str().to_os_string();
    name.push(".json");
    PathBuf::from(name)
}

/// `<data dir>/memory-card-backups`: kept apart from per-game snapshots.
pub fn default_backup_root() -> crate::Result<PathBuf> {
    Ok(crate::app_dirs::data_dir()?.join("memory-card-backups"))
}

/// Copies `card` into `backup_root` and verifies the copy. Read-only for the
/// card; never overwrites an existing backup.
pub fn backup_card(card: &Path, backup_root: &Path) -> Result<CardBackup, CardError> {
    let bytes = read_card_bytes(card)?;
    let sha256 = sha256_hex(&bytes);
    let report = report_bytes(card, &bytes);
    if report.format == CardFormat::NotACard {
        return Err(CardError::NotACard(report.notes.join("; ")));
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(backup_root)
        .map_err(|error| io_error("cannot create", backup_root, error))?;
    let created_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let file_name = card
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "card".into());
    let backup_file = backup_root.join(format!(
        "{created_unix_seconds}-{}-{file_name}",
        &sha256[..12]
    ));
    write_new_file(&backup_file, &bytes)?;
    let verified = (|| {
        let copy = read_card_bytes(&backup_file)?;
        if sha256_hex(&copy) != sha256 {
            return Err(CardError::VerificationFailed(
                "the backup does not read back identical to the card".into(),
            ));
        }
        let again = read_card_bytes(card)?;
        if sha256_hex(&again) != sha256 {
            return Err(CardError::Refused(
                "the card changed while it was being copied (is its emulator running?). \
                 No backup was kept; try again when the emulator is closed"
                    .into(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = verified {
        let _ = fs::remove_file(&backup_file);
        return Err(error);
    }
    let backup = CardBackup {
        format_version: 1,
        source: card.to_path_buf(),
        backup_file: backup_file.clone(),
        size_bytes: bytes.len() as u64,
        sha256,
        created_unix_seconds,
        card_kind: report.format.label().to_string(),
        save_names: report.saves.iter().map(|save| save.name.clone()).collect(),
    };
    let record = serde_json::to_vec_pretty(&backup)
        .map_err(|error| CardError::Io(format!("cannot write the backup record: {error}")))?;
    if let Err(error) = write_new_file(&backup.record_file(), &record) {
        let _ = fs::remove_file(&backup_file);
        return Err(error);
    }
    Ok(backup)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// The card still has exactly the backed-up content.
    Unchanged,
    /// The card has been written since the backup (normal after playing).
    ChangedSinceBackup,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupCheck {
    /// The backup file still has the size and SHA-256 its record states.
    pub backup_intact: bool,
    pub source: SourceState,
    pub detail: String,
}

pub fn load_backup(record_file: &Path) -> Result<CardBackup, CardError> {
    let metadata = fs::symlink_metadata(record_file)
        .map_err(|error| io_error("cannot read", record_file, error))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(CardError::NotAFile(record_file.to_path_buf()));
    }
    let bytes =
        fs::read(record_file).map_err(|error| io_error("cannot read", record_file, error))?;
    let mut backup: CardBackup = serde_json::from_slice(&bytes)
        .map_err(|error| CardError::Io(format!("not a backup record: {error}")))?;
    if backup.format_version != 1 {
        return Err(CardError::Unsupported(format!(
            "backup record version {}",
            backup.format_version
        )));
    }
    // The copy is the file beside the record, wherever the folder now is.
    if let (Some(folder), Some(name)) = (record_file.parent(), backup.backup_file.file_name()) {
        backup.backup_file = folder.join(name);
    }
    Ok(backup)
}

/// Re-reads a backup and compares it with its record and with the card.
pub fn verify_backup(record_file: &Path) -> Result<BackupCheck, CardError> {
    let backup = load_backup(record_file)?;
    let backup_intact = read_card_bytes(&backup.backup_file).is_ok_and(|copy| {
        copy.len() as u64 == backup.size_bytes && sha256_hex(&copy) == backup.sha256
    });
    let source = match read_card_bytes(&backup.source) {
        Ok(card) if sha256_hex(&card) == backup.sha256 => SourceState::Unchanged,
        Ok(_) => SourceState::ChangedSinceBackup,
        Err(_) => SourceState::Missing,
    };
    let detail = match (backup_intact, source) {
        (false, _) => "The backup file is missing or no longer matches its record.".to_string(),
        (true, SourceState::Unchanged) => {
            "The backup is intact and the card still has exactly this content.".to_string()
        }
        (true, SourceState::ChangedSinceBackup) => {
            "The backup is intact. The card has been written since it was made.".to_string()
        }
        (true, SourceState::Missing) => {
            "The backup is intact. The card it was made from is no longer at its path.".to_string()
        }
    };
    Ok(BackupCheck {
        backup_intact,
        source,
        detail,
    })
}

/// Every backup recorded in `backup_root`, newest first.
pub fn list_backups(backup_root: &Path) -> Vec<CardBackup> {
    let mut backups: Vec<CardBackup> = fs::read_dir(backup_root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|path| load_backup(&path).ok())
        .collect();
    backups.sort_by(|left, right| {
        right
            .created_unix_seconds
            .cmp(&left.created_unix_seconds)
            .then_with(|| left.backup_file.cmp(&right.backup_file))
    });
    backups
}
