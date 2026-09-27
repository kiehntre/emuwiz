//! Safe ScummVM engine-option trainers.
//!
//! This is intentionally narrower than a generic ScummVM settings editor. It
//! exposes only documented Hypno engine options whose descriptions explicitly
//! describe gameplay cheats. Graphics, audio, debug, and enhancement settings
//! remain outside the Cheats/Trainers surface.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::Digest;

use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport, SharedPreviewRequest,
    build_shared_preview,
};
use super::shared_transaction::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyResult, SharedTransactionPlan,
    build_shared_transaction_plan, execute_shared_apply,
};

pub const SCUMMVM_TRAINER_MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub const SCUMMVM_TRAINER_MAX_OPTIONS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScummVmTrainerOptionKind {
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScummVmTrainerValue {
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmTrainerOption {
    pub key: String,
    pub label: String,
    pub description: String,
    pub engine_id: String,
    pub game_id: String,
    pub kind: ScummVmTrainerOptionKind,
    pub current_value: Option<ScummVmTrainerValue>,
    pub default_value: ScummVmTrainerValue,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmTrainerSelection {
    pub key: String,
    pub value: Option<ScummVmTrainerValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScummVmTrainerConflictKind {
    UnsupportedGame,
    UnknownOption,
    DuplicateOption,
    InvalidValue,
    IdentityMismatch,
    StaleConfiguration,
    UnsafeTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmTrainerConflict {
    pub kind: ScummVmTrainerConflictKind,
    pub key: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmTrainerIdentity {
    /// The verified `engine:game` ID returned by ScummVM detection.
    pub game_id: String,
    pub game_folder: PathBuf,
    pub target_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScummVmTrainerRequest {
    pub identity: ScummVmTrainerIdentity,
    /// An EmuWiz-owned per-game ScummVM configuration file. Global ScummVM
    /// configuration paths are not accepted by this adapter.
    pub configuration_path: PathBuf,
    pub staging_root: PathBuf,
    pub selections: Vec<ScummVmTrainerSelection>,
    pub expected_configuration_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScummVmTrainerReadiness {
    Ready,
    UnsupportedGame,
    IdentityUnverified,
    StaleConfiguration,
    Conflict,
}

#[derive(Debug)]
pub struct ScummVmTrainerPreview {
    pub options: Vec<ScummVmTrainerOption>,
    pub conflicts: Vec<ScummVmTrainerConflict>,
    pub readiness: ScummVmTrainerReadiness,
    pub target_name: String,
    pub configuration_path: PathBuf,
    pub report: SharedPreviewReport,
    pub transaction_plan: SharedTransactionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScummVmTrainerError {
    Io(String),
    InvalidIdentity,
    UnsafePath,
    UnsupportedGame(String),
    InvalidTarget(String),
    ConfigurationChanged {
        expected: String,
        actual: Option<String>,
    },
    TooLarge,
    Conflict(Vec<ScummVmTrainerConflict>),
    Shared(String),
}

impl fmt::Display for ScummVmTrainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ScummVmTrainerError {}

#[derive(Debug, Clone)]
struct IniDocument {
    lines: Vec<String>,
    sections: BTreeMap<String, Vec<(usize, String, String)>>,
}

/// Returns the documented gameplay trainer options for the exact supported
/// ScummVM engine/game identity. We do not infer options from arbitrary keys.
pub fn scummvm_trainer_options(
    verified_game_id: &str,
    configuration: &[u8],
) -> Result<Vec<ScummVmTrainerOption>, ScummVmTrainerError> {
    let (engine_id, game_id) = split_game_id(verified_game_id)?;
    if engine_id != "hypno" {
        return Err(ScummVmTrainerError::UnsupportedGame(
            "only documented Hypno gameplay trainer options are implemented".into(),
        ));
    }
    let document = parse_ini(configuration)?;
    let section = document.sections.get(game_id).or_else(|| {
        document.sections.values().find(|entries| {
            entries.iter().any(|(_, key, value)| {
                key == "gameid" && (value == verified_game_id || value == game_id)
            })
        })
    });
    let value = |key: &str| {
        section.and_then(|entries| {
            entries
                .iter()
                .rev()
                .find(|(_, candidate, _)| candidate == key)
                .map(|(_, _, value)| parse_bool(value))
                .flatten()
        })
    };
    let declarations = [
        (
            "cheats",
            "Enable original cheats",
            "Allows the game's cheat commands and menu.",
        ),
        (
            "infiniteHealth",
            "Infinite health",
            "Player health will never decrease.",
        ),
        (
            "infiniteAmmo",
            "Infinite ammo",
            "Player ammo will never decrease.",
        ),
        (
            "unlockAllLevels",
            "Unlock all levels",
            "Makes all levels available to play.",
        ),
    ];
    Ok(declarations
        .into_iter()
        .map(|(key, label, description)| ScummVmTrainerOption {
            key: key.into(),
            label: label.into(),
            description: description.into(),
            engine_id: engine_id.into(),
            game_id: game_id.into(),
            kind: ScummVmTrainerOptionKind::Boolean,
            current_value: value(key).map(ScummVmTrainerValue::Boolean),
            default_value: ScummVmTrainerValue::Boolean(false),
            provenance: "ScummVM official Hypno engine Game Options documentation".into(),
        })
        .collect())
}

pub fn build_scummvm_trainer_preview(
    request: &ScummVmTrainerRequest,
) -> Result<ScummVmTrainerPreview, ScummVmTrainerError> {
    let (engine_id, _game_id) = split_game_id(&request.identity.game_id)?;
    if engine_id != "hypno" {
        return Err(ScummVmTrainerError::UnsupportedGame(
            "this ScummVM identity has no implemented trainer projection".into(),
        ));
    }
    ensure_safe_game_folder(&request.identity.game_folder)?;
    ensure_config_scope(&request.configuration_path)?;
    ensure_target_name(&request.identity.target_name)?;
    let configuration = read_configuration(&request.configuration_path)?;
    let actual_configuration = (!configuration.is_empty()).then(|| sha256_hex(&configuration));
    if let Some(expected) = &request.expected_configuration_sha256
        && actual_configuration.as_deref() != Some(expected.as_str())
    {
        return Err(ScummVmTrainerError::ConfigurationChanged {
            expected: expected.clone(),
            actual: actual_configuration,
        });
    }
    let document = parse_ini(&configuration)?;
    if let Some(entries) = document.sections.get(&request.identity.target_name)
        && let Some((_, _, configured_id)) = entries.iter().find(|(_, key, _)| key == "gameid")
        && configured_id != &request.identity.game_id
    {
        return Err(ScummVmTrainerError::Conflict(vec![
            ScummVmTrainerConflict {
                kind: ScummVmTrainerConflictKind::IdentityMismatch,
                key: Some("gameid".into()),
                detail: "the existing EmuWiz target is bound to a different ScummVM game ID".into(),
            },
        ]));
    }
    let options = scummvm_trainer_options(&request.identity.game_id, &configuration)?;
    if options.len() > SCUMMVM_TRAINER_MAX_OPTIONS {
        return Err(ScummVmTrainerError::UnsupportedGame(
            "ScummVM trainer declaration limit exceeded".into(),
        ));
    }
    let conflicts = validate_selections(&options, &request.selections);
    if !conflicts.is_empty() {
        return Err(ScummVmTrainerError::Conflict(conflicts));
    }
    let output = render_scummvm_config(&configuration, &request.identity, &request.selections)?;
    fs::create_dir_all(&request.staging_root).map_err(io_error)?;
    let staged = request.staging_root.join("scummvm-trainer.ini");
    fs::write(&staged, &output).map_err(io_error)?;
    let config_root = request
        .configuration_path
        .parent()
        .ok_or(ScummVmTrainerError::UnsafePath)?;
    let file_name = request
        .configuration_path
        .file_name()
        .ok_or(ScummVmTrainerError::UnsafePath)?;
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::ScummVmTrainer,
        selected_archive: request.identity.game_folder.clone(),
        platform: Some("ScummVM".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::ScummVmGameId,
            state: PreviewIdentityState::Verified,
            value: Some(request.identity.game_id.clone()),
            archive_path: request.identity.game_folder.clone(),
            revision: None,
        },
        destination_root: config_root.to_path_buf(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::ScummVmTrainer,
            source_path: staged,
            expected_source_digest: None,
            destination_relative_paths: vec![PathBuf::from(file_name)],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| ScummVmTrainerError::Shared(format!("{error:?}")))?;
    let transaction_plan = build_shared_transaction_plan(
        &report,
        "scummvm-trainer",
        "scummvm-game-options",
        &request.staging_root,
    )
    .map_err(|error| ScummVmTrainerError::Shared(format!("{error:?}")))?;
    Ok(ScummVmTrainerPreview {
        options,
        conflicts: Vec::new(),
        readiness: ScummVmTrainerReadiness::Ready,
        target_name: request.identity.target_name.clone(),
        configuration_path: request.configuration_path.clone(),
        report,
        transaction_plan,
    })
}

#[derive(Debug, Clone)]
pub struct ScummVmTrainerApplyOptions {
    pub general_approved: bool,
    pub replacement_approved: bool,
    pub operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

pub fn apply_scummvm_trainer_preview(
    preview: &ScummVmTrainerPreview,
    options: &ScummVmTrainerApplyOptions,
) -> Result<SharedApplyResult, ScummVmTrainerError> {
    if preview.readiness != ScummVmTrainerReadiness::Ready {
        return Err(ScummVmTrainerError::Conflict(vec![]));
    }
    let plan = &preview.transaction_plan;
    Ok(execute_shared_apply(
        plan,
        &SharedApplyOptions {
            dry_run: false,
            confirmation: Some(SharedApplyConfirmation {
                plan_id: plan.plan_id.clone(),
                general_approved: options.general_approved,
                replacement_approved: options.replacement_approved,
            }),
            operation_id: options.operation_id.clone(),
            timestamp_unix_seconds: options.timestamp_unix_seconds,
            current_context: plan.context.clone(),
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
    ))
}

fn validate_selections(
    options: &[ScummVmTrainerOption],
    selections: &[ScummVmTrainerSelection],
) -> Vec<ScummVmTrainerConflict> {
    let known: BTreeSet<&str> = options.iter().map(|option| option.key.as_str()).collect();
    let mut seen = BTreeSet::new();
    let mut conflicts = Vec::new();
    for selection in selections {
        if !known.contains(selection.key.as_str()) {
            conflicts.push(ScummVmTrainerConflict {
                kind: ScummVmTrainerConflictKind::UnknownOption,
                key: Some(selection.key.clone()),
                detail: "the selected ScummVM game does not document this trainer option".into(),
            });
        }
        if !seen.insert(selection.key.clone()) {
            conflicts.push(ScummVmTrainerConflict {
                kind: ScummVmTrainerConflictKind::DuplicateOption,
                key: Some(selection.key.clone()),
                detail: "the same trainer option was selected more than once".into(),
            });
        }
        if !matches!(
            selection.value,
            None | Some(ScummVmTrainerValue::Boolean(_))
        ) {
            conflicts.push(ScummVmTrainerConflict {
                kind: ScummVmTrainerConflictKind::InvalidValue,
                key: Some(selection.key.clone()),
                detail: "ScummVM trainer options accept only boolean values".into(),
            });
        }
    }
    conflicts
}

fn render_scummvm_config(
    configuration: &[u8],
    identity: &ScummVmTrainerIdentity,
    selections: &[ScummVmTrainerSelection],
) -> Result<Vec<u8>, ScummVmTrainerError> {
    let mut document = parse_ini(configuration)?;
    let section = identity.target_name.clone();
    let mut rendered = BTreeMap::new();
    rendered.insert("gameid".to_string(), identity.game_id.clone());
    rendered.insert(
        "engineid".to_string(),
        identity.game_id.split(':').next().unwrap().into(),
    );
    rendered.insert(
        "path".to_string(),
        identity.game_folder.display().to_string(),
    );
    for selection in selections {
        if let Some(ScummVmTrainerValue::Boolean(value)) = selection.value {
            rendered.insert(selection.key.clone(), value.to_string());
        } else {
            rendered.remove(&selection.key);
        }
    }
    if let Some(entries) = document.sections.get(&section).cloned() {
        let selected: BTreeSet<String> = selections
            .iter()
            .map(|selection| selection.key.clone())
            .collect();
        for (_, key, _) in entries {
            if let Some(value) = rendered.get(&key) {
                if let Some(index) = document.sections[&section]
                    .iter()
                    .find(|(_, candidate, _)| candidate == &key)
                    .map(|(index, _, _)| *index)
                {
                    document.lines[index] = format!("{key}={value}");
                }
            } else if selected.contains(&key)
                && let Some(index) = document.sections[&section]
                    .iter()
                    .find(|(_, candidate, _)| candidate == &key)
                    .map(|(index, _, _)| *index)
            {
                document.lines[index].clear();
            }
        }
        for (key, value) in &rendered {
            if !document.sections[&section]
                .iter()
                .any(|(_, candidate, _)| candidate == key)
            {
                document.lines.push(format!("{key}={value}"));
            }
        }
    } else {
        if !document.lines.is_empty() && !document.lines.last().unwrap().is_empty() {
            document.lines.push(String::new());
        }
        document.lines.push(format!("[{section}]"));
        for (key, value) in rendered {
            document.lines.push(format!("{key}={value}"));
        }
    }
    Ok(document.lines.join("\n").into_bytes())
}

fn split_game_id(value: &str) -> Result<(&str, &str), ScummVmTrainerError> {
    let (engine, game) = value
        .split_once(':')
        .filter(|(engine, game)| !engine.is_empty() && !game.is_empty())
        .ok_or(ScummVmTrainerError::InvalidIdentity)?;
    Ok((engine, game))
}

fn parse_ini(configuration: &[u8]) -> Result<IniDocument, ScummVmTrainerError> {
    if configuration.len() > SCUMMVM_TRAINER_MAX_CONFIG_BYTES {
        return Err(ScummVmTrainerError::TooLarge);
    }
    let text = std::str::from_utf8(configuration)
        .map_err(|error| ScummVmTrainerError::Io(error.to_string()))?;
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut sections: BTreeMap<String, Vec<(usize, String, String)>> = BTreeMap::new();
    let mut current = String::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current = trimmed[1..trimmed.len() - 1].to_string();
            sections.entry(current.clone()).or_default();
        } else if let Some((key, value)) = trimmed.split_once('=')
            && !current.is_empty()
            && !key.trim().is_empty()
        {
            sections.entry(current.clone()).or_default().push((
                index,
                key.trim().to_string(),
                value.trim().to_string(),
            ));
        }
    }
    Ok(IniDocument { lines, sections })
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn ensure_safe_game_folder(path: &Path) -> Result<(), ScummVmTrainerError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(ScummVmTrainerError::UnsafePath);
    }
    Ok(())
}

fn ensure_config_scope(path: &Path) -> Result<(), ScummVmTrainerError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
        || path.file_name().is_none()
    {
        return Err(ScummVmTrainerError::UnsafePath);
    }
    Ok(())
}

fn ensure_target_name(value: &str) -> Result<(), ScummVmTrainerError> {
    if value.is_empty()
        || value.len() > 96
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(ScummVmTrainerError::InvalidTarget(value.into()));
    }
    Ok(())
}

fn read_configuration(path: &Path) -> Result<Vec<u8>, ScummVmTrainerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ScummVmTrainerError::UnsafePath)
        }
        Ok(metadata) if metadata.len() as usize > SCUMMVM_TRAINER_MAX_CONFIG_BYTES => {
            Err(ScummVmTrainerError::TooLarge)
        }
        Ok(_) => fs::read(path).map_err(io_error),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(io_error(error)),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn io_error(error: std::io::Error) -> ScummVmTrainerError {
    ScummVmTrainerError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_manager::shared_transaction::{
        SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions,
        execute_shared_rollback, preview_shared_rollback,
    };
    use tempfile::tempdir;

    fn request(root: &Path, config: &Path) -> ScummVmTrainerRequest {
        ScummVmTrainerRequest {
            identity: ScummVmTrainerIdentity {
                game_id: "hypno:demo".into(),
                game_folder: root.join("game"),
                target_name: "emuwiz-hypno-demo".into(),
            },
            configuration_path: config.to_path_buf(),
            staging_root: root.join("staging"),
            selections: vec![ScummVmTrainerSelection {
                key: "infiniteHealth".into(),
                value: Some(ScummVmTrainerValue::Boolean(true)),
            }],
            expected_configuration_sha256: None,
        }
    }

    #[test]
    fn projects_only_documented_hypno_gameplay_options() {
        let options =
            scummvm_trainer_options("hypno:demo", b"[demo]\ninfiniteHealth=false\n").unwrap();
        assert_eq!(options.len(), 4);
        assert_eq!(
            options[1].current_value,
            Some(ScummVmTrainerValue::Boolean(false))
        );
        assert!(scummvm_trainer_options("scumm:monkey", b"").is_err());
    }

    #[test]
    fn rendering_is_deterministic_and_preserves_unrelated_config() {
        let root = tempdir().unwrap();
        let identity = ScummVmTrainerIdentity {
            game_id: "hypno:demo".into(),
            game_folder: root.path().join("game"),
            target_name: "emuwiz-hypno-demo".into(),
        };
        let selections = vec![ScummVmTrainerSelection {
            key: "infiniteAmmo".into(),
            value: Some(ScummVmTrainerValue::Boolean(true)),
        }];
        let source = b"[scummvm]\nfullscreen=true\n[other]\npath=/keep\n";
        let first = render_scummvm_config(source, &identity, &selections).unwrap();
        let second = render_scummvm_config(source, &identity, &selections).unwrap();
        assert_eq!(first, second);
        let text = String::from_utf8(first).unwrap();
        assert!(text.contains("fullscreen=true"));
        assert!(text.contains("infiniteAmmo=true"));
        assert!(text.contains("gameid=hypno:demo"));
    }

    #[test]
    fn identity_and_free_form_values_are_refused() {
        let root = tempdir().unwrap();
        let config = root.path().join("game.ini");
        fs::write(&config, b"[emuwiz-hypno-demo]\ngameid=hypno:demo\n").unwrap();
        let mut bad = request(root.path(), &config);
        bad.identity.game_id = "scumm:demo".into();
        assert!(matches!(
            build_scummvm_trainer_preview(&bad),
            Err(ScummVmTrainerError::UnsupportedGame(_))
        ));
        let mut raw = request(root.path(), &config);
        raw.selections[0].value = None;
        raw.selections.push(ScummVmTrainerSelection {
            key: "arbitrary_command".into(),
            value: Some(ScummVmTrainerValue::Boolean(true)),
        });
        assert!(matches!(
            build_scummvm_trainer_preview(&raw),
            Err(ScummVmTrainerError::Conflict(_))
        ));
    }

    #[test]
    fn transactional_apply_and_rollback_preserve_game_folder() {
        let root = tempdir().unwrap();
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();
        let config = root.path().join("game.ini");
        fs::write(&config, b"[existing]\nkeep=true\n").unwrap();
        let preview = build_scummvm_trainer_preview(&request(root.path(), &config)).unwrap();
        let history_dir = tempdir().unwrap();
        let backup_dir = tempdir().unwrap();
        let history = history_dir.path().to_path_buf();
        let backup = backup_dir.path().to_path_buf();
        let result = apply_scummvm_trainer_preview(
            &preview,
            &ScummVmTrainerApplyOptions {
                general_approved: true,
                replacement_approved: true,
                operation_id: "op".into(),
                timestamp_unix_seconds: 1,
                history_root: history.clone(),
                backup_root: backup.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            result.journal.status,
            SharedApplyStatus::Success,
            "journal: {:#?}; failure: {:#?}",
            result.journal,
            result.journal_failure
        );
        assert!(
            String::from_utf8(fs::read(&config).unwrap())
                .unwrap()
                .contains("infiniteHealth=true")
        );
        assert!(!game.join("modified").exists());
        let journal = result.journal_path.unwrap();
        let rollback = preview_shared_rollback(&journal, root.path(), &backup);
        assert!(rollback.available);
        let rollback_result = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "rollback".into(),
                timestamp_unix_seconds: 2,
                history_root: history,
                backup_root: backup,
            },
        );
        assert_eq!(rollback_result.status, SharedApplyStatus::Success);
        assert_eq!(fs::read(&config).unwrap(), b"[existing]\nkeep=true\n");
    }
}
