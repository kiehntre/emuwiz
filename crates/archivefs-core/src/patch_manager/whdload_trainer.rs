//! Safe, documented WHDLoad trainer/custom-option support.
//!
//! This adapter treats trainer options as launch configuration, not memory
//! writes.  It never edits a slave or game media.  Only declarations projected
//! from the selected installed slave are accepted, and opaque declarations are
//! kept visible but cannot be applied.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::open_retro_cheat_providers::WhdloadCustomOption;

use super::AmigaEmulatorKind;
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewSourceItem, SharedPreviewReport, SharedPreviewRequest,
    build_shared_preview,
};
use super::shared_transaction::{
    SharedApplyConfirmation, SharedApplyOptions, SharedApplyResult, SharedTransactionPlan,
    build_shared_transaction_plan, execute_shared_apply,
};

pub const WHDLOAD_TRAINER_MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub const WHDLOAD_TRAINER_MAX_OPTIONS: usize = 64;
pub const WHDLOAD_TRAINER_MAX_CHOICES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainerOptionKind {
    Boolean,
    Numeric,
    Enum,
    Bitfield,
    Opaque,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainerOptionValue {
    Boolean(bool),
    Numeric(u32),
    Enum(String),
    Bitfield(u32),
    Opaque(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainerOptionChoice {
    pub label: String,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainerOptionSource {
    pub slave_path: PathBuf,
    pub slave_sha256: String,
    pub verified_game_identity: String,
    pub package_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainerOption {
    pub name: String,
    pub description: String,
    pub source: TrainerOptionSource,
    pub custom_slot: String,
    pub kind: TrainerOptionKind,
    pub allowed_values: Vec<TrainerOptionChoice>,
    pub current_value: Option<TrainerOptionValue>,
    pub current_raw_value: Option<u32>,
    pub default_value: Option<TrainerOptionValue>,
    pub bit_range: Option<(u8, u8)>,
    pub provenance: String,
    pub readiness: TrainerOptionReadiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainerOptionReadiness {
    Ready,
    Unsupported,
    InvalidValue,
    IdentityUnverified,
    StaleSource,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhdloadTrainerLaunchProjection {
    FsUaeArguments(Vec<String>),
    AmiberryPreviewOnly { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainerOptionSelection {
    pub custom_slot: String,
    pub value: Option<TrainerOptionValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainerOptionConflictKind {
    SameSlotDifferentValue,
    InvalidValue,
    UnsupportedOption,
    StaleSlave,
    DuplicateOption,
    OpaqueSyntax,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainerOptionConflict {
    pub kind: TrainerOptionConflictKind,
    pub custom_slot: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhdloadTrainerIdentity {
    pub slave_path: PathBuf,
    pub expected_slave_sha256: String,
    pub verified_game_identity: String,
    pub package_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhdloadTrainerRequest {
    pub identity: WhdloadTrainerIdentity,
    /// Existing EmuWiz-owned per-game WHDLoad option/tooltype layer.  Global
    /// `S:WHDLoad.prefs` and emulator-wide profiles are intentionally refused.
    pub configuration_path: PathBuf,
    pub staging_root: PathBuf,
    pub declarations: Vec<WhdloadCustomOption>,
    pub selections: Vec<TrainerOptionSelection>,
    pub expected_configuration_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhdloadTrainerReadiness {
    Ready,
    IdentityUnverified,
    StaleSlave,
    StaleConfiguration,
    Conflict,
    Unsupported,
}

#[derive(Debug)]
pub struct WhdloadTrainerPreview {
    pub slave_path: PathBuf,
    pub slave_sha256: String,
    pub configuration_path: PathBuf,
    pub configuration_sha256: Option<String>,
    pub options: Vec<TrainerOption>,
    pub conflicts: Vec<TrainerOptionConflict>,
    pub readiness: WhdloadTrainerReadiness,
    pub rendered_arguments: Vec<String>,
    pub report: SharedPreviewReport,
    pub transaction_plan: SharedTransactionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhdloadTrainerError {
    Io(String),
    EmptyIdentity,
    UnsafePath,
    SlaveChanged {
        expected: String,
        actual: String,
    },
    ConfigurationChanged {
        expected: String,
        actual: Option<String>,
    },
    TooLarge,
    InvalidDeclaration(String),
    InvalidSelection(String),
    Conflict(Vec<TrainerOptionConflict>),
    Unsupported(String),
    Shared(String),
}

impl fmt::Display for WhdloadTrainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for WhdloadTrainerError {}

#[derive(Debug, Clone)]
struct OptionFile {
    original: Vec<String>,
    values: BTreeMap<String, String>,
}

pub fn project_whdload_trainer_options(
    declarations: &[WhdloadCustomOption],
    source: &TrainerOptionSource,
    configuration: &[u8],
) -> Result<Vec<TrainerOption>, WhdloadTrainerError> {
    let file = parse_option_file(configuration)?;
    if declarations.len() > WHDLOAD_TRAINER_MAX_OPTIONS {
        return Err(WhdloadTrainerError::Unsupported(
            "slave declares too many custom options".into(),
        ));
    }
    let mut options = Vec::new();
    for declaration in declarations {
        if !valid_slot(&declaration.key) {
            continue;
        }
        let (kind, choices, bit_range, readiness) = declaration_shape(declaration);
        let current_value = file
            .values
            .get(&canonical_slot(&declaration.key))
            .and_then(|raw| parse_value(kind, raw, &choices, bit_range));
        let current_raw_value = file
            .values
            .get(&canonical_slot(&declaration.key))
            .and_then(|raw| raw.parse::<u32>().ok());
        let default_value = default_value(declaration, kind, &choices, bit_range);
        options.push(TrainerOption {
            name: declaration.key.clone(),
            description: declaration.description.clone(),
            source: source.clone(),
            custom_slot: canonical_slot(&declaration.key),
            kind,
            allowed_values: choices,
            current_value,
            current_raw_value,
            default_value,
            bit_range,
            provenance: if declaration.documented {
                if declaration.option_type == "N" {
                    "EmuWiz internal numeric compatibility representation from the selected installed WHDLoad slave".into()
                } else {
                    "official ws_config declaration from the selected installed WHDLoad slave".into()
                }
            } else {
                "opaque custom declaration from the selected installed WHDLoad slave".into()
            },
            readiness,
        });
    }
    Ok(options)
}

pub fn build_whdload_trainer_preview(
    request: &WhdloadTrainerRequest,
) -> Result<WhdloadTrainerPreview, WhdloadTrainerError> {
    if request.identity.verified_game_identity.trim().is_empty()
        || request.identity.expected_slave_sha256.trim().is_empty()
    {
        return Err(WhdloadTrainerError::EmptyIdentity);
    }
    ensure_safe_regular_file(&request.identity.slave_path)?;
    ensure_config_scope(&request.configuration_path)?;
    let slave_bytes = fs::read(&request.identity.slave_path).map_err(io_error)?;
    let actual_slave = sha256_hex(&slave_bytes);
    if actual_slave != request.identity.expected_slave_sha256.to_ascii_lowercase() {
        return Err(WhdloadTrainerError::SlaveChanged {
            expected: request.identity.expected_slave_sha256.clone(),
            actual: actual_slave,
        });
    }
    let configuration = read_configuration(&request.configuration_path)?;
    let actual_configuration = configuration.as_ref().map(|bytes| sha256_hex(bytes));
    if let Some(expected) = &request.expected_configuration_sha256 {
        if actual_configuration.as_deref() != Some(expected.as_str()) {
            return Err(WhdloadTrainerError::ConfigurationChanged {
                expected: expected.clone(),
                actual: actual_configuration,
            });
        }
    }
    let source = TrainerOptionSource {
        slave_path: request.identity.slave_path.clone(),
        slave_sha256: actual_slave.clone(),
        verified_game_identity: request.identity.verified_game_identity.clone(),
        package_version: request.identity.package_version.clone(),
    };
    let options = project_whdload_trainer_options(
        &request.declarations,
        &source,
        configuration.as_deref().unwrap_or_default(),
    )?;
    let conflicts = validate_selections(&options, &request.selections);
    if !conflicts.is_empty() {
        return Err(WhdloadTrainerError::Conflict(conflicts));
    }
    let rendered_arguments = render_whdload_arguments(&options, &request.selections)?;
    let output = render_option_file(
        configuration.as_deref().unwrap_or_default(),
        &options,
        &request.selections,
    )?;
    fs::create_dir_all(&request.staging_root).map_err(io_error)?;
    let staged = request.staging_root.join("whdload-options.conf");
    fs::write(&staged, &output).map_err(io_error)?;
    let config_root = request
        .configuration_path
        .parent()
        .ok_or(WhdloadTrainerError::UnsafePath)?;
    let file_name = request
        .configuration_path
        .file_name()
        .ok_or(WhdloadTrainerError::UnsafePath)?;
    let report = build_shared_preview(&SharedPreviewRequest {
        adapter: PreviewAdapter::AmigaWhdloadTrainer,
        selected_archive: request.identity.slave_path.clone(),
        platform: Some("Amiga".into()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::WhdloadSlave,
            state: PreviewIdentityState::Verified,
            value: Some(request.identity.verified_game_identity.clone()),
            archive_path: request.identity.slave_path.clone(),
            revision: None,
        },
        destination_root: config_root.to_path_buf(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::AmigaWhdloadTrainer,
            source_path: staged,
            expected_source_digest: None,
            destination_relative_paths: vec![PathBuf::from(file_name)],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    })
    .map_err(|error| WhdloadTrainerError::Shared(format!("{error:?}")))?;
    let transaction_plan = build_shared_transaction_plan(
        &report,
        "amiga-whdload",
        "whdload-trainer-options",
        &request.staging_root,
    )
    .map_err(|error| WhdloadTrainerError::Shared(format!("{error:?}")))?;
    Ok(WhdloadTrainerPreview {
        slave_path: request.identity.slave_path.clone(),
        slave_sha256: actual_slave,
        configuration_path: request.configuration_path.clone(),
        configuration_sha256: actual_configuration,
        options,
        conflicts: Vec::new(),
        readiness: WhdloadTrainerReadiness::Ready,
        rendered_arguments,
        report,
        transaction_plan,
    })
}

#[derive(Debug, Clone)]
pub struct WhdloadTrainerApplyOptions {
    pub general_approved: bool,
    pub replacement_approved: bool,
    pub operation_id: String,
    pub timestamp_unix_seconds: u64,
    pub history_root: PathBuf,
    pub backup_root: PathBuf,
}

pub fn apply_whdload_trainer_preview(
    preview: &WhdloadTrainerPreview,
    options: &WhdloadTrainerApplyOptions,
) -> Result<SharedApplyResult, WhdloadTrainerError> {
    if preview.readiness != WhdloadTrainerReadiness::Ready {
        return Err(WhdloadTrainerError::Unsupported(
            "WHDLoad trainer preview is not ready".into(),
        ));
    }
    ensure_safe_regular_file(&preview.slave_path)?;
    let current_slave = sha256_hex(&fs::read(&preview.slave_path).map_err(io_error)?);
    if current_slave != preview.slave_sha256 {
        return Err(WhdloadTrainerError::SlaveChanged {
            expected: preview.slave_sha256.clone(),
            actual: current_slave,
        });
    }
    let current_configuration = read_configuration(&preview.configuration_path)?;
    let current_configuration_sha256 = current_configuration
        .as_ref()
        .map(|bytes| sha256_hex(bytes));
    if current_configuration_sha256 != preview.configuration_sha256 {
        return Err(WhdloadTrainerError::ConfigurationChanged {
            expected: preview
                .configuration_sha256
                .clone()
                .unwrap_or_else(|| "<absent>".into()),
            actual: current_configuration_sha256,
        });
    }
    let plan = &preview.transaction_plan;
    let current_context = plan.context.clone();
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
            current_context,
            history_root: options.history_root.clone(),
            backup_root: options.backup_root.clone(),
        },
    ))
}

pub fn render_whdload_arguments(
    options: &[TrainerOption],
    selections: &[TrainerOptionSelection],
) -> Result<Vec<String>, WhdloadTrainerError> {
    let mut rendered = Vec::new();
    for selection in selections {
        let option = options
            .iter()
            .find(|option| option.custom_slot == canonical_slot(&selection.custom_slot))
            .ok_or_else(|| WhdloadTrainerError::InvalidSelection(selection.custom_slot.clone()))?;
        if let Some(value) = &selection.value {
            rendered.push(format!(
                "{}={}",
                option.custom_slot,
                value_for_slot(option, value, option.current_value.as_ref())?
            ));
        }
    }
    Ok(rendered)
}

pub fn project_whdload_trainer_launch_options(
    emulator: AmigaEmulatorKind,
    arguments: Vec<String>,
) -> WhdloadTrainerLaunchProjection {
    match emulator {
        AmigaEmulatorKind::FsUae => WhdloadTrainerLaunchProjection::FsUaeArguments(arguments),
        AmigaEmulatorKind::Amiberry => WhdloadTrainerLaunchProjection::AmiberryPreviewOnly {
            reason: "the current Amiberry launcher contract exposes --autoload but no proven per-game WHDLoad argument channel".into(),
        },
    }
}

fn validate_selections(
    options: &[TrainerOption],
    selections: &[TrainerOptionSelection],
) -> Vec<TrainerOptionConflict> {
    let mut conflicts = Vec::new();
    let mut seen = BTreeMap::<String, Option<TrainerOptionValue>>::new();
    for selection in selections {
        let slot = canonical_slot(&selection.custom_slot);
        let Some(option) = options.iter().find(|option| option.custom_slot == slot) else {
            conflicts.push(TrainerOptionConflict {
                kind: TrainerOptionConflictKind::UnsupportedOption,
                custom_slot: Some(slot),
                detail: "the selected slave does not declare this custom option".into(),
            });
            continue;
        };
        if option.kind == TrainerOptionKind::Opaque {
            conflicts.push(TrainerOptionConflict {
                kind: TrainerOptionConflictKind::OpaqueSyntax,
                custom_slot: Some(slot.clone()),
                detail: "the slave declaration uses an option syntax EmuWiz does not interpret"
                    .into(),
            });
        }
        if let Some(previous) = seen.insert(slot.clone(), selection.value.clone()) {
            if previous != selection.value {
                conflicts.push(TrainerOptionConflict {
                    kind: TrainerOptionConflictKind::SameSlotDifferentValue,
                    custom_slot: Some(slot.clone()),
                    detail: "two trainer selections write different values to the same CUSTOM slot"
                        .into(),
                });
            } else {
                conflicts.push(TrainerOptionConflict {
                    kind: TrainerOptionConflictKind::DuplicateOption,
                    custom_slot: Some(slot.clone()),
                    detail: "the same trainer option was selected more than once".into(),
                });
            }
        }
        if let Some(value) = &selection.value {
            if value_for_slot(option, value, option.current_value.as_ref()).is_err() {
                conflicts.push(TrainerOptionConflict {
                    kind: TrainerOptionConflictKind::InvalidValue,
                    custom_slot: Some(slot),
                    detail: "the selected value is outside the documented option choices".into(),
                });
            }
        }
    }
    conflicts
}

fn declaration_shape(
    declaration: &WhdloadCustomOption,
) -> (
    TrainerOptionKind,
    Vec<TrainerOptionChoice>,
    Option<(u8, u8)>,
    TrainerOptionReadiness,
) {
    let spec = declaration.spec.as_deref().unwrap_or_default();
    match declaration.option_type.as_str() {
        "B" => (
            TrainerOptionKind::Boolean,
            vec![
                TrainerOptionChoice {
                    label: "Off".into(),
                    value: 0,
                },
                TrainerOptionChoice {
                    label: "On".into(),
                    value: 1,
                },
            ],
            None,
            TrainerOptionReadiness::Ready,
        ),
        "X" => {
            let bit = spec.parse::<u8>().ok().filter(|bit| *bit <= 31);
            (
                TrainerOptionKind::Boolean,
                vec![
                    TrainerOptionChoice {
                        label: "Off".into(),
                        value: 0,
                    },
                    TrainerOptionChoice {
                        label: "On".into(),
                        value: 1,
                    },
                ],
                bit.map(|bit| (bit, bit)),
                bit.map_or(TrainerOptionReadiness::InvalidValue, |_| {
                    TrainerOptionReadiness::Ready
                }),
            )
        }
        "L" => {
            let choices = parse_choices(spec);
            let readiness = if choices.is_empty() {
                TrainerOptionReadiness::InvalidValue
            } else {
                TrainerOptionReadiness::Ready
            };
            (TrainerOptionKind::Enum, choices, None, readiness)
        }
        "M" => {
            let (labels, range) = spec
                .split_once(':')
                .map_or((spec, None), |(a, b)| (a, parse_bit_range(b)));
            let choices = parse_choices(labels);
            let readiness = if choices.is_empty() || range.is_none() {
                TrainerOptionReadiness::InvalidValue
            } else {
                TrainerOptionReadiness::Ready
            };
            (TrainerOptionKind::Bitfield, choices, range, readiness)
        }
        "N" => (
            TrainerOptionKind::Numeric,
            Vec::new(),
            None,
            TrainerOptionReadiness::Ready,
        ),
        _ => (
            TrainerOptionKind::Opaque,
            Vec::new(),
            None,
            TrainerOptionReadiness::Unsupported,
        ),
    }
}

fn default_value(
    declaration: &WhdloadCustomOption,
    kind: TrainerOptionKind,
    choices: &[TrainerOptionChoice],
    bit_range: Option<(u8, u8)>,
) -> Option<TrainerOptionValue> {
    let raw = declaration.default_value.as_deref()?;
    match kind {
        TrainerOptionKind::Boolean => raw
            .parse::<u32>()
            .ok()
            .map(|value| TrainerOptionValue::Boolean(value != 0)),
        TrainerOptionKind::Numeric => raw.parse::<u32>().ok().map(TrainerOptionValue::Numeric),
        TrainerOptionKind::Enum => choices
            .iter()
            .find(|choice| choice.label.eq_ignore_ascii_case(raw))
            .or_else(|| choices.first())
            .map(|choice| TrainerOptionValue::Enum(choice.label.clone())),
        TrainerOptionKind::Bitfield => bit_range.map(|_| TrainerOptionValue::Bitfield(0)),
        TrainerOptionKind::Opaque => Some(TrainerOptionValue::Opaque(raw.into())),
    }
}

fn parse_value(
    kind: TrainerOptionKind,
    raw: &str,
    choices: &[TrainerOptionChoice],
    bit_range: Option<(u8, u8)>,
) -> Option<TrainerOptionValue> {
    match kind {
        TrainerOptionKind::Boolean => raw.parse::<u32>().ok().map(|value| {
            if let Some((lo, hi)) = bit_range {
                TrainerOptionValue::Boolean(((value >> lo) & ((1u32 << (hi - lo + 1)) - 1)) != 0)
            } else {
                TrainerOptionValue::Boolean(value != 0)
            }
        }),
        TrainerOptionKind::Numeric => raw.parse::<u32>().ok().map(TrainerOptionValue::Numeric),
        TrainerOptionKind::Enum => raw.parse::<usize>().ok().and_then(|index| {
            choices
                .get(index)
                .map(|choice| TrainerOptionValue::Enum(choice.label.clone()))
        }),
        TrainerOptionKind::Bitfield => raw.parse::<u32>().ok().map(TrainerOptionValue::Bitfield),
        TrainerOptionKind::Opaque => Some(TrainerOptionValue::Opaque(raw.into())),
    }
}

fn value_for_slot(
    option: &TrainerOption,
    value: &TrainerOptionValue,
    current: Option<&TrainerOptionValue>,
) -> Result<String, WhdloadTrainerError> {
    match (&option.kind, value) {
        (TrainerOptionKind::Boolean, TrainerOptionValue::Boolean(enabled)) => {
            if let Some((bit, _)) = option.bit_range {
                let current = option.current_raw_value.unwrap_or_else(|| match current {
                    Some(TrainerOptionValue::Boolean(value)) => u32::from(*value),
                    Some(TrainerOptionValue::Bitfield(value)) => *value,
                    _ => 0,
                });
                let value = if *enabled {
                    current | (1 << bit)
                } else {
                    current & !(1 << bit)
                };
                Ok(value.to_string())
            } else {
                Ok(u32::from(*enabled).to_string())
            }
        }
        (TrainerOptionKind::Numeric, TrainerOptionValue::Numeric(value)) => Ok(value.to_string()),
        (TrainerOptionKind::Enum, TrainerOptionValue::Enum(label)) => option
            .allowed_values
            .iter()
            .find(|choice| choice.label == *label)
            .map(|choice| choice.value.to_string())
            .ok_or_else(|| WhdloadTrainerError::InvalidSelection(label.clone())),
        (TrainerOptionKind::Bitfield, TrainerOptionValue::Bitfield(value)) => {
            let Some((lo, hi)) = option.bit_range else {
                return Err(WhdloadTrainerError::InvalidSelection(
                    option.custom_slot.clone(),
                ));
            };
            let width = hi - lo + 1;
            if width < 32 && *value >= (1u32 << width) {
                return Err(WhdloadTrainerError::InvalidSelection(value.to_string()));
            }
            let width_mask = if width == 32 {
                u32::MAX
            } else {
                (1u32 << width) - 1
            };
            let current = option.current_raw_value.unwrap_or(0);
            let mask = width_mask << lo;
            Ok(((current & !mask) | ((value & width_mask) << lo)).to_string())
        }
        _ => Err(WhdloadTrainerError::Unsupported(option.custom_slot.clone())),
    }
}

fn render_option_file(
    configuration: &[u8],
    options: &[TrainerOption],
    selections: &[TrainerOptionSelection],
) -> Result<Vec<u8>, WhdloadTrainerError> {
    let file = parse_option_file(configuration)?;
    let mut replacements = BTreeMap::new();
    for selection in selections {
        let slot = canonical_slot(&selection.custom_slot);
        let option = options
            .iter()
            .find(|option| option.custom_slot == slot)
            .ok_or_else(|| WhdloadTrainerError::InvalidSelection(slot.clone()))?;
        replacements.insert(
            slot,
            selection
                .value
                .as_ref()
                .map(|value| value_for_slot(option, value, option.current_value.as_ref()))
                .transpose()?,
        );
    }
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for line in file.original {
        let Some((key, _)) = parse_assignment(&line) else {
            output.push(line);
            continue;
        };
        let key = canonical_slot(&key);
        let Some(replacement) = replacements.get(&key) else {
            output.push(line);
            continue;
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Some(value) = replacement {
            output.push(format!("{key}={value}"));
        }
    }
    for (key, value) in replacements {
        if seen.insert(key.clone()) {
            if let Some(value) = value {
                output.push(format!("{key}={value}"));
            }
        }
    }
    Ok(output.join("\n").into_bytes())
}

fn parse_option_file(bytes: &[u8]) -> Result<OptionFile, WhdloadTrainerError> {
    if bytes.len() > WHDLOAD_TRAINER_MAX_CONFIG_BYTES {
        return Err(WhdloadTrainerError::TooLarge);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| WhdloadTrainerError::InvalidSelection("configuration is not UTF-8".into()))?;
    let original = text.lines().map(str::to_string).collect::<Vec<_>>();
    let values = original
        .iter()
        .filter_map(|line| parse_assignment(line))
        .map(|(key, value)| (canonical_slot(&key), value))
        .collect();
    Ok(OptionFile { original, values })
}

fn parse_assignment(line: &str) -> Option<(String, String)> {
    let content = line.split_once(';').map_or(line, |(value, _)| value).trim();
    let (key, value) = content.split_once('=')?;
    let key = key.trim().to_ascii_uppercase();
    valid_slot(&key).then(|| (key, value.trim().to_string()))
}

fn parse_choices(spec: &str) -> Vec<TrainerOptionChoice> {
    spec.split(',')
        .take(WHDLOAD_TRAINER_MAX_CHOICES)
        .enumerate()
        .filter_map(|(value, label)| {
            let label = label.trim();
            (!label.is_empty()).then(|| TrainerOptionChoice {
                label: label.into(),
                value: value as u32,
            })
        })
        .collect()
}

fn parse_bit_range(value: &str) -> Option<(u8, u8)> {
    let (lo, hi) = value.split_once('-')?;
    let lo = lo.parse::<u8>().ok()?;
    let hi = hi.parse::<u8>().ok()?;
    (lo <= hi && hi <= 31 && hi - lo + 1 >= 2).then_some((lo, hi))
}

fn valid_slot(value: &str) -> bool {
    matches!(
        canonical_slot(value).as_str(),
        "CUSTOM1" | "CUSTOM2" | "CUSTOM3" | "CUSTOM4" | "CUSTOM5"
    )
}

fn canonical_slot(value: &str) -> String {
    let value = value.trim().to_ascii_uppercase();
    if value.starts_with("CUSTOM") {
        value
    } else if let Some(number) = value.strip_prefix('C') {
        format!("CUSTOM{number}")
    } else {
        value
    }
}

fn read_configuration(path: &Path) -> Result<Option<Vec<u8>>, WhdloadTrainerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(WhdloadTrainerError::UnsafePath)
        }
        Ok(_) => fs::read(path).map(Some).map_err(io_error),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn ensure_safe_regular_file(path: &Path) -> Result<(), WhdloadTrainerError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(WhdloadTrainerError::UnsafePath);
    }
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(WhdloadTrainerError::UnsafePath);
    }
    Ok(())
}

fn ensure_config_scope(path: &Path) -> Result<(), WhdloadTrainerError> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(WhdloadTrainerError::UnsafePath);
    }
    if let Some(parent) = path.parent() {
        if let Ok(metadata) = fs::symlink_metadata(parent) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(WhdloadTrainerError::UnsafePath);
            }
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn io_error(error: std::io::Error) -> WhdloadTrainerError {
    WhdloadTrainerError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::shared_transaction::{
        SharedRollbackConfirmation, SharedRollbackOptions, execute_shared_rollback,
        preview_shared_rollback,
    };
    use super::*;

    fn source(path: &Path) -> TrainerOptionSource {
        TrainerOptionSource {
            slave_path: path.into(),
            slave_sha256: "sha".into(),
            verified_game_identity: "amiga:test".into(),
            package_version: Some("1".into()),
        }
    }

    fn declarations() -> Vec<WhdloadCustomOption> {
        vec![
            WhdloadCustomOption {
                key: "C1".into(),
                option_type: "B".into(),
                description: "Infinite lives".into(),
                default_value: Some("0".into()),
                spec: Some("0".into()),
                documented: true,
            },
            WhdloadCustomOption {
                key: "C2".into(),
                option_type: "N".into(),
                description: "Start level".into(),
                default_value: Some("1".into()),
                spec: Some("1".into()),
                documented: true,
            },
            WhdloadCustomOption {
                key: "C3".into(),
                option_type: "L".into(),
                description: "Difficulty".into(),
                default_value: Some("Easy,Hard".into()),
                spec: Some("Easy,Hard".into()),
                documented: true,
            },
        ]
    }

    #[test]
    fn projects_boolean_numeric_and_enum_options() {
        let options = project_whdload_trainer_options(
            &declarations(),
            &source(Path::new("/games/test.slave")),
            b"CUSTOM1=1\nCUSTOM2=5\nCUSTOM3=1\n",
        )
        .unwrap();
        assert_eq!(
            options[0].current_value,
            Some(TrainerOptionValue::Boolean(true))
        );
        assert_eq!(
            options[1].current_value,
            Some(TrainerOptionValue::Numeric(5))
        );
        assert_eq!(
            options[2].current_value,
            Some(TrainerOptionValue::Enum("Hard".into()))
        );
    }

    #[test]
    fn invalid_value_and_same_slot_conflict_are_blocking() {
        let options = project_whdload_trainer_options(
            &declarations(),
            &source(Path::new("/games/test.slave")),
            b"",
        )
        .unwrap();
        let conflicts = validate_selections(
            &options,
            &[
                TrainerOptionSelection {
                    custom_slot: "C1".into(),
                    value: Some(TrainerOptionValue::Boolean(true)),
                },
                TrainerOptionSelection {
                    custom_slot: "CUSTOM1".into(),
                    value: Some(TrainerOptionValue::Boolean(false)),
                },
                TrainerOptionSelection {
                    custom_slot: "C3".into(),
                    value: Some(TrainerOptionValue::Enum("Unknown".into())),
                },
            ],
        );
        assert!(
            conflicts
                .iter()
                .any(|c| c.kind == TrainerOptionConflictKind::SameSlotDifferentValue)
        );
        assert!(
            conflicts
                .iter()
                .any(|c| c.kind == TrainerOptionConflictKind::InvalidValue)
        );
    }

    #[test]
    fn arguments_and_disable_preserve_unrelated_configuration() {
        let options = project_whdload_trainer_options(
            &declarations(),
            &source(Path::new("/games/test.slave")),
            b"PRELOAD\nCUSTOM1=0\nCUSTOM2=3\nUSER_OPTION=yes\n",
        )
        .unwrap();
        let selections = vec![
            TrainerOptionSelection {
                custom_slot: "C1".into(),
                value: Some(TrainerOptionValue::Boolean(true)),
            },
            TrainerOptionSelection {
                custom_slot: "C2".into(),
                value: None,
            },
        ];
        assert_eq!(
            render_whdload_arguments(&options, &selections).unwrap(),
            vec!["CUSTOM1=1"]
        );
        let rendered = render_option_file(
            b"PRELOAD\nCUSTOM1=0\nCUSTOM2=3\nUSER_OPTION=yes\n",
            &options,
            &selections,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(rendered).unwrap(),
            "PRELOAD\nCUSTOM1=1\nUSER_OPTION=yes"
        );
    }

    #[test]
    fn bitfield_and_opaque_declarations_are_conservative() {
        let declarations = vec![
            WhdloadCustomOption {
                key: "C1".into(),
                option_type: "M".into(),
                description: "Mode".into(),
                default_value: Some("Easy,Hard:0-1".into()),
                spec: Some("Easy,Hard:0-1".into()),
                documented: true,
            },
            WhdloadCustomOption {
                key: "C2".into(),
                option_type: "K".into(),
                description: "Unknown".into(),
                default_value: None,
                spec: None,
                documented: true,
            },
        ];
        let options = project_whdload_trainer_options(
            &declarations,
            &source(Path::new("/games/test.slave")),
            b"",
        )
        .unwrap();
        assert_eq!(options[0].kind, TrainerOptionKind::Bitfield);
        assert_eq!(options[1].readiness, TrainerOptionReadiness::Unsupported);
    }

    #[test]
    fn exact_slave_preview_apply_and_rollback_never_touch_slave() {
        let root = tempfile::tempdir().unwrap();
        let slave = root.path().join("Game.slave");
        let config = root.path().join("Game.options");
        let staging = root.path().join("staging");
        let history = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let slave_bytes = b"synthetic WHDLoad slave bytes";
        fs::write(&slave, slave_bytes).unwrap();
        fs::write(&config, b"PRELOAD\nCUSTOM1=0\nUSER_OPTION=yes\n").unwrap();
        let request = WhdloadTrainerRequest {
            identity: WhdloadTrainerIdentity {
                slave_path: slave.clone(),
                expected_slave_sha256: sha256_hex(slave_bytes),
                verified_game_identity: "amiga:test".into(),
                package_version: Some("1".into()),
            },
            configuration_path: config.clone(),
            staging_root: staging,
            declarations: declarations(),
            selections: vec![TrainerOptionSelection {
                custom_slot: "C1".into(),
                value: Some(TrainerOptionValue::Boolean(true)),
            }],
            expected_configuration_sha256: Some(sha256_hex(&fs::read(&config).unwrap())),
        };
        let preview = build_whdload_trainer_preview(&request).unwrap();
        assert_eq!(preview.rendered_arguments, vec!["CUSTOM1=1"]);
        assert_eq!(fs::read(&slave).unwrap(), slave_bytes);
        assert_eq!(
            fs::read(&config).unwrap(),
            b"PRELOAD\nCUSTOM1=0\nUSER_OPTION=yes\n"
        );
        let result = apply_whdload_trainer_preview(
            &preview,
            &WhdloadTrainerApplyOptions {
                general_approved: true,
                replacement_approved: true,
                operation_id: "whdload-test-apply".into(),
                timestamp_unix_seconds: 1,
                history_root: history.path().to_path_buf(),
                backup_root: backup.path().to_path_buf(),
            },
        )
        .unwrap();
        assert_eq!(
            result.journal.status,
            super::super::shared_transaction::SharedApplyStatus::Success,
            "{:#?}",
            result.journal
        );
        assert_eq!(
            fs::read(&config).unwrap(),
            b"PRELOAD\nCUSTOM1=1\nUSER_OPTION=yes"
        );
        let journal = result.journal_path.as_ref().unwrap();
        let rollback = preview_shared_rollback(journal, root.path(), backup.path());
        assert!(rollback.available);
        let rolled = execute_shared_rollback(
            &rollback,
            &SharedRollbackOptions {
                confirmation: SharedRollbackConfirmation {
                    preview_id: rollback.preview_id.clone(),
                    approved: true,
                },
                rollback_operation_id: "whdload-test-rollback".into(),
                timestamp_unix_seconds: 2,
                history_root: history.path().to_path_buf(),
                backup_root: backup.path().to_path_buf(),
            },
        );
        assert_eq!(
            rolled.status,
            super::super::shared_transaction::SharedApplyStatus::Success
        );
        assert_eq!(
            fs::read(&config).unwrap(),
            b"PRELOAD\nCUSTOM1=0\nUSER_OPTION=yes\n"
        );
    }

    #[test]
    fn stale_slave_and_configuration_are_refused_before_staging() {
        let root = tempfile::tempdir().unwrap();
        let slave = root.path().join("Game.slave");
        let config = root.path().join("Game.options");
        fs::write(&slave, b"slave").unwrap();
        fs::write(&config, b"CUSTOM1=0\n").unwrap();
        let mut request = WhdloadTrainerRequest {
            identity: WhdloadTrainerIdentity {
                slave_path: slave,
                expected_slave_sha256: "00".repeat(32),
                verified_game_identity: "amiga:test".into(),
                package_version: None,
            },
            configuration_path: config.clone(),
            staging_root: root.path().join("staging"),
            declarations: declarations(),
            selections: Vec::new(),
            expected_configuration_sha256: None,
        };
        assert!(matches!(
            build_whdload_trainer_preview(&request),
            Err(WhdloadTrainerError::SlaveChanged { .. })
        ));
        request.identity.expected_slave_sha256 = sha256_hex(b"slave");
        request.expected_configuration_sha256 = Some("00".repeat(32));
        assert!(matches!(
            build_whdload_trainer_preview(&request),
            Err(WhdloadTrainerError::ConfigurationChanged { .. })
        ));
    }

    #[test]
    fn launch_projection_keeps_fsuae_and_amiberry_contracts_distinct() {
        assert_eq!(
            project_whdload_trainer_launch_options(
                AmigaEmulatorKind::FsUae,
                vec!["CUSTOM1=1".into()]
            ),
            WhdloadTrainerLaunchProjection::FsUaeArguments(vec!["CUSTOM1=1".into()])
        );
        assert!(matches!(
            project_whdload_trainer_launch_options(AmigaEmulatorKind::Amiberry, vec![]),
            WhdloadTrainerLaunchProjection::AmiberryPreviewOnly { .. }
        ));
    }
}
