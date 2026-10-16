//! WHDLoad trainer options over the reviewed backend contract
//! (`docs/cheats/TRAINERS_AND_POKES_BACKEND.md`, "Blue GUI Batch 8
//! contract"). This module only gathers the inputs the backend asks for and
//! calls it: discovery, preview, apply and the shared-transaction Undo all
//! stay in core. It adds no trainer behaviour of its own.
//!
//! Inputs, and only these:
//! - the verified game identity, from the workflow's own identity report;
//! - host slaves: the game itself when it is a `.slave` file, or the `.slave`
//!   files directly inside its folder (packaged LHA/HDF archives are not host
//!   files and are never passed off as one);
//! - the user's discovered FS-UAE/Amiberry profile, which `inspect_amiga_whdload_game` needs;
//! - EmuWiz's own per-game option file, resolved by core
//!   (`whdload_trainer_option_file`, the one storage contract).
//!
//! Applied options are saved to that file. No launch path passes them to
//! FS-UAE yet, so nothing here may claim a trainer is active in-game.

use std::path::{Path, PathBuf};

use archivefs_core::identity_source::whdload::{SlaveArtifact, inspect_whdload_slave_file};
use archivefs_core::patch_manager::{
    AmigaGameRequest, AmigaProfileDiscoveryRoots, SharedApplyResult, SharedRollbackConfirmation,
    SharedRollbackOptions, SharedRollbackPreview, SharedRollbackResult, TrainerOptionSelection,
    WhdloadTrainerApplyOptions, WhdloadTrainerDiscovery, WhdloadTrainerError,
    WhdloadTrainerPreview, WhdloadTrainerRequest, apply_whdload_trainer_preview,
    build_whdload_trainer_preview, default_shared_backup_root, default_shared_history_root,
    discover_amiga_profiles, discover_whdload_trainer_options, execute_shared_rollback,
    generate_shared_operation_id, inspect_amiga_whdload_game, preview_shared_rollback,
    whdload_trainer_option_file,
};
use sha2::{Digest, Sha256};

/// Upper bound on the option file read to supply current values (matches
/// the backend's configuration limit).
const MAX_OPTION_FILE_BYTES: u64 = 1024 * 1024;
/// At most this many `.slave` files are considered in a game folder.
const MAX_FOLDER_SLAVES: usize = 64;

pub(crate) const PACKAGED_NOTE: &str = "This game is a packaged archive. Trainer options need the installed WHDLoad slave (a .slave file on disk), so they are listed once the game is installed.";
pub(crate) const NO_PROFILE_NOTE: &str =
    "No FS-UAE or Amiberry setup was found, so EmuWiz cannot inspect this game's WHDLoad options.";

/// Whether the game is a packaged archive rather than an installed slave.
pub(crate) fn is_packaged(game_path: &Path) -> bool {
    game_path.is_file()
        && !game_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("slave"))
}

/// The host `.slave` files for a game: the game itself, or those directly
/// inside its folder. Symlinks are never followed.
pub(crate) fn host_slaves(game_path: &Path) -> Vec<SlaveArtifact> {
    let is_slave = |path: &Path| {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("slave"))
            && std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_file())
    };
    let paths: Vec<PathBuf> = if is_slave(game_path) {
        vec![game_path.to_path_buf()]
    } else if std::fs::symlink_metadata(game_path).is_ok_and(|meta| meta.is_dir()) {
        let mut found: Vec<PathBuf> = std::fs::read_dir(game_path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| is_slave(path))
            .take(MAX_FOLDER_SLAVES)
            .collect();
        found.sort();
        found
    } else {
        Vec::new()
    };
    paths
        .iter()
        .filter_map(|path| inspect_whdload_slave_file(path).ok())
        .collect()
}

/// The current option file bytes (empty when it does not exist yet).
pub(crate) fn read_option_file(path: &Path) -> Result<Vec<u8>, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
        Ok(meta) if !meta.file_type().is_file() => {
            Err("the option file is not a regular file".into())
        }
        Ok(meta) if meta.len() > MAX_OPTION_FILE_BYTES => {
            Err("the option file is too large".into())
        }
        Ok(_) => std::fs::read(path).map_err(|error| error.to_string()),
    }
}

/// What discovery found for the selected game, or why it could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiscoveryOutcome {
    /// The discovery, and EmuWiz's option file for it when core resolves one.
    Found(Box<WhdloadTrainerDiscovery>, Option<PathBuf>),
    /// A precondition the backend contract does not cover, in plain words.
    Unavailable(&'static str),
    Failed(String),
}

/// Runs the backend discovery for one game. Reads only; writes nothing.
pub(crate) fn discover(game_path: &Path, verified_identity: Option<String>) -> DiscoveryOutcome {
    if is_packaged(game_path) {
        return DiscoveryOutcome::Unavailable(PACKAGED_NOTE);
    }
    let Some(roots) = AmigaProfileDiscoveryRoots::from_environment() else {
        return DiscoveryOutcome::Unavailable(NO_PROFILE_NOTE);
    };
    let profiles = discover_amiga_profiles(&roots);
    let Some(profile) = profiles.profiles.first() else {
        return DiscoveryOutcome::Unavailable(NO_PROFILE_NOTE);
    };
    discover_with(profile, game_path, verified_identity, |discovery| {
        whdload_trainer_option_file(discovery).ok()
    })
}

/// Discovery with an explicit profile and option-file location (tests).
pub(crate) fn discover_with(
    profile: &archivefs_core::patch_manager::AmigaProfile,
    game_path: &Path,
    verified_identity: Option<String>,
    option_file: impl Fn(&WhdloadTrainerDiscovery) -> Option<PathBuf>,
) -> DiscoveryOutcome {
    let inspection = inspect_amiga_whdload_game(
        profile,
        &AmigaGameRequest {
            verified_amiga_identity: verified_identity.clone(),
            bare_slaves: host_slaves(game_path),
            ..Default::default()
        },
    );
    let discovery = discover_whdload_trainer_options(&inspection, b"");
    // Core resolves the option file only for a Ready, verified discovery.
    let Some(path) = option_file(&discovery) else {
        return DiscoveryOutcome::Found(Box::new(discovery), None);
    };
    match read_option_file(&path) {
        // No saved options yet: the first projection already has the values.
        Ok(bytes) if bytes.is_empty() => DiscoveryOutcome::Found(Box::new(discovery), Some(path)),
        Ok(bytes) => DiscoveryOutcome::Found(
            Box::new(discover_whdload_trainer_options(&inspection, &bytes)),
            Some(path),
        ),
        Err(error) => DiscoveryOutcome::Failed(format!(
            "EmuWiz's option file for this game could not be read: {error}"
        )),
    }
}

/// Plain words for a backend refusal, with each typed conflict as a
/// blocking reason.
pub(crate) fn refusal_text(error: &WhdloadTrainerError) -> Vec<String> {
    match error {
        WhdloadTrainerError::Conflict(conflicts) => conflicts
            .iter()
            .map(|conflict| {
                let slot = conflict
                    .custom_slot
                    .as_deref()
                    .map(|slot| format!("{slot}: "))
                    .unwrap_or_default();
                format!("{slot}{:?} — {}", conflict.kind, conflict.detail)
            })
            .collect(),
        WhdloadTrainerError::SlaveChanged { .. } => {
            vec!["The WHDLoad slave changed since it was checked. Nothing was saved.".into()]
        }
        WhdloadTrainerError::ConfigurationChanged { .. } => vec![
            "EmuWiz's option file for this game changed since the preview. Nothing was saved; preview again."
                .into(),
        ],
        other => vec![format!("Nothing was saved: {other}")],
    }
}

/// Builds the backend preview for the chosen selections. Writes only into
/// `staging_root` (a private temporary folder the caller owns).
pub(crate) fn preview(
    discovery: &WhdloadTrainerDiscovery,
    selections: &[TrainerOptionSelection],
    configuration_path: &Path,
    staging_root: &Path,
) -> Result<WhdloadTrainerPreview, WhdloadTrainerError> {
    let identity = discovery
        .identity
        .clone()
        .ok_or(WhdloadTrainerError::EmptyIdentity)?;
    let current = read_option_file(configuration_path).map_err(WhdloadTrainerError::Io)?;
    let expected_configuration_sha256 = (!current.is_empty()).then(|| {
        Sha256::digest(&current)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    });
    build_whdload_trainer_preview(&WhdloadTrainerRequest {
        identity,
        configuration_path: configuration_path.to_path_buf(),
        staging_root: staging_root.to_path_buf(),
        declarations: discovery.declarations.clone(),
        selections: selections.to_vec(),
        expected_configuration_sha256,
    })
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Applies a preview the person explicitly confirmed. The backend re-checks
/// the slave and the option file first and records the change in History.
pub(crate) fn apply(preview: &WhdloadTrainerPreview) -> Result<SharedApplyResult, String> {
    let history_root = default_shared_history_root().map_err(|error| format!("{error:?}"))?;
    let backup_root = default_shared_backup_root().map_err(|error| format!("{error:?}"))?;
    apply_with(preview, history_root, backup_root)
}

pub(crate) fn apply_with(
    preview: &WhdloadTrainerPreview,
    history_root: PathBuf,
    backup_root: PathBuf,
) -> Result<SharedApplyResult, String> {
    apply_whdload_trainer_preview(
        preview,
        &WhdloadTrainerApplyOptions {
            general_approved: true,
            // The only file that can be replaced is EmuWiz's own option file,
            // named in the confirmation the person accepted.
            replacement_approved: true,
            operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: now(),
            history_root,
            backup_root,
        },
    )
    .map_err(|error| refusal_text(&error).join(" "))
}

/// Whether the record of an apply proves Undo is safe right now.
pub(crate) fn undo_preview(
    result: &SharedApplyResult,
    configuration_path: &Path,
    backup_root: &Path,
) -> Option<SharedRollbackPreview> {
    let journal = result.journal_path.as_ref()?;
    let root = configuration_path.parent()?;
    Some(preview_shared_rollback(journal, root, backup_root))
}

pub(crate) fn undo(
    preview: &SharedRollbackPreview,
    history_root: PathBuf,
    backup_root: PathBuf,
) -> SharedRollbackResult {
    execute_shared_rollback(
        preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: generate_shared_operation_id(),
            timestamp_unix_seconds: now(),
            history_root,
            backup_root,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::patch_manager::{
        AmigaEmulatorKind, AmigaInstallationType, AmigaProfile, AmigaProfileScope,
        TrainerOptionValue, WHDLOAD_TRAINER_OPTION_FOLDER, WhdloadTrainerDiscoveryState,
        whdload_trainer_option_file_in,
    };

    /// A minimal v17 WHDLoad slave declaring `config` (same layout as the
    /// core discovery fixture).
    fn synthetic_slave(config: &str) -> Vec<u8> {
        let mut code = Vec::new();
        code.extend_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
        code.extend_from_slice(b"WHDLOADS");
        code.extend_from_slice(&17u16.to_be_bytes());
        code.extend_from_slice(&0u16.to_be_bytes());
        code.extend_from_slice(&0x80000u32.to_be_bytes());
        code.extend_from_slice(&[0u8; 8]);
        code.extend_from_slice(&0u16.to_be_bytes());
        code.extend_from_slice(&[0u8; 12]);
        code.extend_from_slice(&0u32.to_be_bytes());
        code.extend_from_slice(&0u16.to_be_bytes());
        code.extend_from_slice(&52u16.to_be_bytes());
        code.extend_from_slice(&[0, 0]);
        assert_eq!(code.len(), 52);
        code.extend_from_slice(config.as_bytes());
        code.push(0);
        while code.len() % 4 != 0 {
            code.push(0);
        }
        let words = (code.len() / 4) as u32;
        let mut out = Vec::new();
        for word in [0x3f3u32, 0, 1, 0, 0, words, 0x3e9, words] {
            out.extend_from_slice(&word.to_be_bytes());
        }
        out.extend_from_slice(&code);
        out.extend_from_slice(&0x3f2u32.to_be_bytes());
        out
    }

    fn profile(root: &Path) -> AmigaProfile {
        AmigaProfile {
            profile_id: "fs-uae-test".into(),
            emulator: AmigaEmulatorKind::FsUae,
            installation_type: AmigaInstallationType::Explicit,
            scope: AmigaProfileScope::Explicit,
            configuration_root: root.to_path_buf(),
            global_config_path: None,
            profile_paths: Vec::new(),
            executable_candidates: Vec::new(),
            eligible: true,
            warnings: Vec::new(),
        }
    }

    struct Lab {
        root: tempfile::TempDir,
        game: PathBuf,
        slave: PathBuf,
        options: PathBuf,
    }

    fn lab() -> Lab {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("games/Turrican");
        std::fs::create_dir_all(&game).unwrap();
        let slave = game.join("Turrican.slave");
        std::fs::write(
            &slave,
            synthetic_slave("C1:X:Infinite lives:0;C1:X:Infinite energy:1;C1:X:In-game keys:3;"),
        )
        .unwrap();
        let options = root.path().join("data").join(WHDLOAD_TRAINER_OPTION_FOLDER);
        Lab {
            root,
            game,
            slave,
            options,
        }
    }

    fn found(lab: &Lab, identity: Option<&str>) -> (WhdloadTrainerDiscovery, Option<PathBuf>) {
        let folder = lab.options.clone();
        match discover_with(
            &profile(lab.root.path()),
            &lab.game,
            identity.map(str::to_owned),
            move |discovery| whdload_trainer_option_file_in(&folder, discovery).ok(),
        ) {
            DiscoveryOutcome::Found(discovery, path) => (*discovery, path),
            other => panic!("{other:?}"),
        }
    }

    fn select(discovery: &WhdloadTrainerDiscovery, on: &[&str]) -> Vec<TrainerOptionSelection> {
        discovery
            .options
            .iter()
            .map(|option| TrainerOptionSelection {
                custom_slot: option.selection_key.clone(),
                value: Some(TrainerOptionValue::Boolean(
                    on.contains(&option.description.as_str()),
                )),
            })
            .collect()
    }

    fn read(path: &Path) -> String {
        String::from_utf8(std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn an_unverified_game_gets_no_option_file_and_cannot_preview() {
        let lab = lab();
        let (discovery, path) = found(&lab, None);
        assert_ne!(discovery.state, WhdloadTrainerDiscoveryState::Ready);
        assert_eq!(path, None);
        assert!(!lab.options.exists());
        let slave_before = std::fs::read(&lab.slave).unwrap();
        let staging = tempfile::tempdir().unwrap();
        let fallback = lab.options.join("x.options");
        assert!(
            preview(
                &discovery,
                &select(&discovery, &[]),
                &fallback,
                staging.path()
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&lab.slave).unwrap(), slave_before);
    }

    #[test]
    fn shared_slot_options_preview_apply_rediscover_and_undo_end_to_end() {
        let lab = lab();
        let slave_before = std::fs::read(&lab.slave).unwrap();
        let (discovery, path) = found(&lab, Some("amiga:turrican"));
        assert_eq!(
            discovery.state,
            WhdloadTrainerDiscoveryState::Ready,
            "{}",
            discovery.detail
        );
        let path = path.expect("core resolves the option file for a Ready game");
        assert_eq!(path.parent(), Some(lab.options.as_path()));
        assert!(!path.exists(), "resolving must not create the file");
        assert_eq!(discovery.options.len(), 3);
        let keys: std::collections::BTreeSet<_> = discovery
            .options
            .iter()
            .map(|option| option.selection_key.clone())
            .collect();
        assert_eq!(keys.len(), 3, "each X option on CUSTOM1 has its own key");

        // Enable lives + keys (bits 0 and 3), leave energy off.
        let history = lab.root.path().join("history");
        let backup = lab.root.path().join("backup");
        let staging = tempfile::tempdir().unwrap();
        let first = preview(
            &discovery,
            &select(&discovery, &["Infinite lives", "In-game keys"]),
            &path,
            staging.path(),
        )
        .expect("preview");
        assert!(!path.exists(), "preview writes only to staging");
        let applied = apply_with(&first, history.clone(), backup.clone()).expect("apply");
        assert!(read(&path).contains("CUSTOM1=9"), "{}", read(&path));

        // The saved values come back on the next discovery (restart / reopen).
        let (again, again_path) = found(&lab, Some("amiga:turrican"));
        assert_eq!(again_path.as_deref(), Some(path.as_path()));
        let current: Vec<_> = again
            .options
            .iter()
            .map(|option| (option.description.as_str(), option.current_value.clone()))
            .collect();
        assert_eq!(
            current,
            vec![
                ("Infinite lives", Some(TrainerOptionValue::Boolean(true))),
                ("Infinite energy", Some(TrainerOptionValue::Boolean(false))),
                ("In-game keys", Some(TrainerOptionValue::Boolean(true))),
            ]
        );

        // Turning one option off keeps the others.
        let staging = tempfile::tempdir().unwrap();
        let second = preview(
            &again,
            &select(&again, &["In-game keys"]),
            &path,
            staging.path(),
        )
        .expect("second preview");
        let second_applied = apply_with(&second, history.clone(), backup.clone()).expect("apply");
        assert!(read(&path).contains("CUSTOM1=8"), "{}", read(&path));

        // Undo the second change: back to lives + keys.
        let undo_two = undo_preview(&second_applied, &path, &backup).expect("undo preview");
        assert!(undo_two.available);
        undo(&undo_two, history.clone(), backup.clone());
        assert!(read(&path).contains("CUSTOM1=9"), "{}", read(&path));
        // Undo the first: the option file did not exist before.
        let undo_one = undo_preview(&applied, &path, &backup).expect("undo preview");
        assert!(undo_one.available);
        undo(&undo_one, history, backup);
        assert!(!path.exists());

        // Game media is never touched.
        assert_eq!(std::fs::read(&lab.slave).unwrap(), slave_before);
        assert_eq!(std::fs::read_dir(&lab.game).unwrap().count(), 1);
    }

    #[test]
    fn saving_from_the_editor_keeps_its_staging_until_the_apply_has_run() {
        // The editor owns the private staging folder the preview wrote into.
        // Saving must not drop it before the apply copies the staged file.
        let lab = lab();
        let (discovery, path) = found(&lab, Some("amiga:turrican"));
        let path = path.unwrap();
        let staging = tempfile::tempdir().unwrap();
        let staged = preview(
            &discovery,
            &select(&discovery, &["Infinite lives"]),
            &path,
            staging.path(),
        )
        .unwrap();
        let mut editor = crate::cheats_mods::WhdloadTrainerEditorState {
            staging: Some(staging),
            ..Default::default()
        };
        let history = lab.root.path().join("history");
        let backup = lab.root.path().join("backup");
        let backups = backup.clone();
        crate::cheats_mods::save_whdload_preview_with(
            &mut editor,
            &staged,
            move |preview| apply_with(preview, history, backup),
            Some(backups),
        );
        assert!(editor.refusals.is_empty(), "{:?}", editor.refusals);
        assert_eq!(
            editor.outcome.as_ref().map(|(ok, _)| *ok),
            Some(true),
            "{:?}",
            editor.outcome
        );
        assert!(
            editor.staging.is_none(),
            "staging is released after the apply"
        );
        assert!(read(&path).contains("CUSTOM1=1"), "{}", read(&path));
        assert!(editor.undo.as_ref().is_some_and(|undo| undo.available));
    }

    #[test]
    fn a_stale_preview_does_not_overwrite_an_option_file_changed_since() {
        let lab = lab();
        let (discovery, path) = found(&lab, Some("amiga:turrican"));
        let path = path.unwrap();
        let staging = tempfile::tempdir().unwrap();
        let stale = preview(
            &discovery,
            &select(&discovery, &["Infinite lives"]),
            &path,
            staging.path(),
        )
        .unwrap();
        std::fs::create_dir_all(&lab.options).unwrap();
        std::fs::write(&path, b"CUSTOM1=2\n").unwrap();
        let history = lab.root.path().join("history");
        let backup = lab.root.path().join("backup");
        assert!(apply_with(&stale, history, backup).is_err());
        assert_eq!(read(&path), "CUSTOM1=2\n");
    }

    #[test]
    fn packaged_archives_are_never_passed_off_as_host_slaves() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("Game.lha");
        std::fs::write(&package, b"not a slave").unwrap();
        assert!(is_packaged(&package));
        assert!(host_slaves(&package).is_empty());
        assert_eq!(
            discover(&package, Some("amiga:game".into())),
            DiscoveryOutcome::Unavailable(PACKAGED_NOTE)
        );
        // A folder is not a package; an invalid .slave is simply not offered.
        std::fs::write(dir.path().join("Broken.slave"), b"junk").unwrap();
        assert!(!is_packaged(dir.path()));
        assert!(host_slaves(dir.path()).is_empty());
    }

    #[test]
    fn a_missing_option_file_reads_as_empty_and_a_symlink_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.options");
        assert_eq!(read_option_file(&path).unwrap(), Vec::<u8>::new());
        #[cfg(unix)]
        {
            let target = dir.path().join("target");
            std::fs::write(&target, b"CUSTOM1=1\n").unwrap();
            let link = dir.path().join("link.options");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(read_option_file(&link).is_err());
        }
    }
}
