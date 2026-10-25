//! Input binding only. Trainer rendering, writes, receipts and recovery stay in core.
use std::path::{Path, PathBuf};
use std::result::Result;

use archivefs_core::patch_manager::{
    PreviewAdapter, ScummVmTrainerApplyOptions, ScummVmTrainerIdentity, ScummVmTrainerPreview,
    ScummVmTrainerRequest, ScummVmTrainerSelection, SharedApplyJournal, SharedApplyResult,
    SharedRollbackConfirmation, SharedRollbackOptions, SharedRollbackPreview, SharedRollbackResult,
    apply_scummvm_trainer_preview, build_scummvm_trainer_preview, default_shared_backup_root,
    default_shared_history_root, discover_shared_apply_history, execute_shared_rollback,
    generate_shared_operation_id, preview_shared_rollback, scummvm_trainer_options,
};
use archivefs_core::scummvm_detection::{
    detect_scummvm_directory_with_executable, resolve_scummvm_executable,
};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub(super) struct Roots {
    pub profiles: PathBuf,
    pub history: PathBuf,
    pub backups: PathBuf,
}

impl Roots {
    pub fn managed() -> Result<Self, String> {
        let profiles = archivefs_core::app_dirs::data_path("scummvm-trainer-options")
            .map_err(|e| e.to_string())?;
        // A test that installed its own temporary machine keeps its receipts
        // out of every other test's History, as the other cheat routes do.
        // Thread-local: resolve on the caller's thread, not inside a worker.
        #[cfg(test)]
        if let Some((history, backups)) =
            crate::cheats_mods::duckstation_environment::shared_history_roots()
        {
            return Ok(Self {
                profiles,
                history,
                backups,
            });
        }
        Ok(Self {
            profiles,
            history: default_shared_history_root().map_err(|e| e.detail)?,
            backups: default_shared_backup_root().map_err(|e| e.detail)?,
        })
    }

    // Both identity and exact folder distinguish installations. No title or raw
    // path reaches a filename; moving a game intentionally needs a new preview.
    pub fn profile(&self, folder: &Path, id: &str) -> PathBuf {
        let mut hash = Sha256::new();
        hash.update(id.as_bytes());
        hash.update([0]);
        hash.update(folder.as_os_str().as_encoded_bytes());
        let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        self.profiles.join(format!("{digest}.ini"))
    }
}

pub(super) struct Prepared {
    pub preview: ScummVmTrainerPreview,
    pub identity: ScummVmTrainerIdentity,
    pub executable: PathBuf,
    pub roots: Roots,
    // Keep staged, hashed output alive through confirmation, then drop it.
    _staging: tempfile::TempDir,
    source: SourceBinding,
}

/// Section the owned profile writes and a launch reads back.
pub(super) const TARGET_NAME: &str = "emuwiz-trainer";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceBinding {
    canonical: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn source_binding(folder: &Path) -> Result<SourceBinding, String> {
    let metadata = std::fs::symlink_metadata(folder).map_err(|e| e.to_string())?;
    let canonical = folder.canonicalize().map_err(|e| e.to_string())?;
    if !metadata.file_type().is_dir() || canonical != folder {
        return Err(
            "Select the real, absolute game directory; symlinked or aliased paths are refused."
                .into(),
        );
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(SourceBinding {
        canonical,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}

pub(super) fn prepare(
    folder: &Path,
    expected_id: Option<&str>,
    selections: Vec<ScummVmTrainerSelection>,
) -> Result<Prepared, String> {
    let executable = resolve_scummvm_executable().ok_or(
        "Native ScummVM is unavailable. Install it before checking trainer compatibility.",
    )?;
    prepare_with(
        folder,
        expected_id,
        selections,
        executable,
        Roots::managed()?,
    )
}

pub(super) fn prepare_with(
    folder: &Path,
    expected_id: Option<&str>,
    selections: Vec<ScummVmTrainerSelection>,
    executable: PathBuf,
    roots: Roots,
) -> Result<Prepared, String> {
    let source = source_binding(folder)?;
    if roots.profiles.starts_with(folder) {
        return Err("The owned profile directory must be outside the game directory.".into());
    }
    let detected =
        detect_scummvm_directory_with_executable(folder, &executable).map_err(|e| e.to_string())?;
    let id = format!("{}:{}", detected.engine_id, detected.game_id);
    if expected_id.is_some_and(|expected| expected != id) || source_binding(folder)? != source {
        return Err(
            "The detected game or source directory changed. Check compatibility again.".into(),
        );
    }
    // The engine owns compatibility, declarations and their current values.
    scummvm_trainer_options(&id, b"").map_err(|e| e.to_string())?;
    let identity = ScummVmTrainerIdentity {
        game_id: id.clone(),
        game_folder: folder.to_path_buf(),
        target_name: TARGET_NAME.into(),
    };
    let staging = tempfile::tempdir().map_err(|e| e.to_string())?;
    let preview = build_scummvm_trainer_preview(&ScummVmTrainerRequest {
        identity: identity.clone(),
        configuration_path: roots.profile(folder, &id),
        staging_root: staging.path().to_path_buf(),
        selections,
        // The shared plan records the exact existing/missing destination and
        // digest, including an existing empty file, and rechecks under lock.
        expected_configuration_sha256: None,
    })
    .map_err(|e| e.to_string())?;
    Ok(Prepared {
        preview,
        identity,
        executable,
        roots,
        _staging: staging,
        source,
    })
}

pub(super) fn apply(prepared: Prepared) -> Result<SharedApplyResult, String> {
    let folder = &prepared.identity.game_folder;
    if source_binding(folder)? != prepared.source {
        return Err("The source directory changed since Preview. Nothing was applied.".into());
    }
    let detected = detect_scummvm_directory_with_executable(folder, &prepared.executable)
        .map_err(|e| e.to_string())?;
    if format!("{}:{}", detected.engine_id, detected.game_id) != prepared.identity.game_id
        || source_binding(folder)? != prepared.source
    {
        return Err("ScummVM no longer detects the previewed game. Nothing was applied.".into());
    }
    apply_scummvm_trainer_preview(
        &prepared.preview,
        &ScummVmTrainerApplyOptions {
            general_approved: true,
            replacement_approved: true,
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: now(),
            history_root: prepared.roots.history,
            backup_root: prepared.roots.backups,
        },
    )
    .map_err(|e| e.to_string())
}

pub(super) struct Receipt {
    pub journal_path: PathBuf,
    pub journal: SharedApplyJournal,
}

// History is authoritative after restart. Never let a mislabeled receipt
// broaden Undo from our one owned config to another destination or adapter.
fn owned_receipt(roots: &Roots, journal: &SharedApplyJournal) -> bool {
    let Ok(folder) = journal.context.selected_archive.to_path_buf() else {
        return false;
    };
    let id = &journal.context.verified_game_identity;
    let expected = roots.profile(&folder, id);
    journal.context.adapter == PreviewAdapter::ScummVmTrainer
        && journal.context.profile_id == "scummvm-trainer"
        && journal.context.source_mode == "scummvm-game-options"
        && folder.is_absolute()
        && scummvm_trainer_options(id, b"").is_ok()
        && journal.destination_root.to_path_buf().ok().as_ref() == Some(&roots.profiles)
        && journal.entries.len() == 1
        && journal.entries.iter().all(|entry| {
            let plan = &entry.plan_entry;
            plan.adapter == PreviewAdapter::ScummVmTrainer
                && plan.selected_archive == journal.context.selected_archive
                && plan.verified_game_identity == *id
                && plan.destination_root == journal.destination_root
                && plan.destination_relative_path.to_path_buf().ok().as_deref()
                    == expected.file_name().map(Path::new)
        })
}

pub(super) fn history(roots: &Roots) -> (Vec<Receipt>, Vec<String>) {
    let report = discover_shared_apply_history(&roots.history);
    let mut problems: Vec<String> = report
        .warnings
        .iter()
        .map(|w| format!("{}: {}", w.path.display, w.failure.detail))
        .collect();
    if !report.complete {
        problems.push("History is incomplete; some receipts could not be read.".into());
    }
    let mut receipts = Vec::new();
    for (path, journal) in report.journals {
        if journal.context.adapter != PreviewAdapter::ScummVmTrainer {
            continue;
        }
        if !owned_receipt(roots, &journal) {
            problems.push(format!(
                "{}: trainer receipt has an unexpected identity or destination; Undo refused.",
                path.display
            ));
        } else if let Ok(journal_path) = path.to_path_buf() {
            receipts.push(Receipt {
                journal_path,
                journal,
            });
        }
    }
    receipts.sort_by_key(|r| std::cmp::Reverse(r.journal.timestamp_unix_seconds));
    (receipts, problems)
}

pub(super) fn undo_preview(
    receipt: &Receipt,
    roots: &Roots,
) -> Result<SharedRollbackPreview, String> {
    let (fresh, _) = history(roots);
    if !owned_receipt(roots, &receipt.journal)
        || !fresh
            .iter()
            .any(|r| r.journal_path == receipt.journal_path && r.journal == receipt.journal)
    {
        return Err("This receipt is outside the owned trainer profile. Undo refused.".into());
    }
    // Core reloads the authoritative journal and checks installed/backup hashes.
    let preview = preview_shared_rollback(&receipt.journal_path, &roots.profiles, &roots.backups);
    let folder = receipt
        .journal
        .context
        .selected_archive
        .to_path_buf()
        .map_err(|e| format!("{e:?}"))?;
    let expected = roots.profile(&folder, &receipt.journal.context.verified_game_identity);
    if preview.original_operation_id != receipt.journal.operation_id
        || preview.entries.iter().any(|entry| {
            entry
                .destination
                .as_ref()
                .and_then(|p| p.to_path_buf().ok())
                .as_ref()
                != Some(&expected)
        })
    {
        return Err("The journal changed or names an unexpected destination. Undo refused.".into());
    }
    Ok(preview)
}

pub(super) fn undo(preview: SharedRollbackPreview, roots: Roots) -> SharedRollbackResult {
    execute_shared_rollback(
        &preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: now(),
            history_root: roots.history,
            backup_root: roots.backups,
        },
    )
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
