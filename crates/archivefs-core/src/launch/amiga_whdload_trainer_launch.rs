//! Saved WHDLoad trainer options -> FS-UAE launch plan.
//!
//! This is the launch consumer of the trainer backend: it reads the options a
//! person saved for one installed WHDLoad game, checks them against that
//! game's own slave declarations, projects them through
//! `project_whdload_trainer_launch_options`, and hands the result to
//! [`build_fsuae_whdload_command_plan_with_trainer`]. It reads only; the game,
//! its slave and the option file are never written.
//!
//! - The game identity and slave come from the trusted inspection
//!   (`discover_whdload_trainer_options`); they must match the launch input.
//! - Where the option file lives is core's storage contract
//!   (`whdload_trainer_option_file`), passed in by the caller so this module
//!   never builds that path itself.
//! - Anything malformed, conflicting, undeclared or changed is refused rather
//!   than partly applied. No saved file, or a file with nothing enabled,
//!   leaves the launch byte-identical to a launch without trainers.
//! - Packaged (LHA) games, HDF slaves, unverified games and Amiberry never
//!   receive options; their launches are unchanged.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::launch::amiga_whdload_command::{
    AmigaWHDLoadCommandPlan, WHDLoadLaunchInput, WhdloadLaunchTrainer, WhdloadPackageFormat,
    build_fsuae_whdload_command_plan, build_fsuae_whdload_command_plan_with_trainer,
};
use crate::launch::planning::CanonicalIdentityStatus;
use crate::launch::readiness::{LaunchBlocker, LaunchBlockerKind, LaunchReadiness};
use crate::patch_manager::{
    AmigaGameInspection, TrainerOption, TrainerOptionKind, TrainerOptionReadiness,
    WHDLOAD_TRAINER_MAX_CONFIG_BYTES, WhdloadTrainerDiscovery, WhdloadTrainerDiscoveryState,
    WhdloadTrainerLaunchProjection, discover_whdload_trainer_options,
    project_whdload_trainer_launch_options,
};

/// What saved trainer options mean for one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhdloadLaunchTrainerResolution {
    /// Trainer options cannot apply to this launch (packaged game, Amiberry,
    /// unverified game, no options declared, ...). Launch unchanged.
    NotApplicable { reason: String },
    /// No option file, or nothing enabled in it. Launch unchanged.
    NoSavedOptions,
    /// These options go on the FS-UAE command.
    Apply(WhdloadLaunchTrainer),
    /// Saved options exist but are malformed, stale, conflicting or not bound
    /// to this game. The launch is blocked rather than run without them.
    Refused { reason: String },
}

fn refused(reason: impl Into<String>) -> WhdloadLaunchTrainerResolution {
    WhdloadLaunchTrainerResolution::Refused {
        reason: reason.into(),
    }
}

fn not_applicable(reason: impl Into<String>) -> WhdloadLaunchTrainerResolution {
    WhdloadLaunchTrainerResolution::NotApplicable {
        reason: reason.into(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The option file's bytes; `None` when it does not exist. Symlinks,
/// non-regular files and oversize files are refused.
fn read_option_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "the trainer option file could not be read: {error}"
            ));
        }
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("the trainer option file is not a regular file".into());
    }
    let limit = WHDLOAD_TRAINER_MAX_CONFIG_BYTES as u64;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("the trainer option file could not be read: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err("the trainer option file is too large".into());
    }
    Ok(Some(bytes))
}

/// Whether a key names a CUSTOM slot in either spelling the editor accepts
/// (`CUSTOM1`, `C1`), valid or not.
fn custom_like(key: &str) -> bool {
    key.starts_with("CUSTOM")
        || key
            .strip_prefix('C')
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Strict reading of the CUSTOM values in a saved option file for launch.
/// Lines that are not CUSTOM assignments (blank, `;` comments, tooltypes the
/// editor preserves such as `PRELOAD`) are left alone and never reach the
/// command. A CUSTOM line must be `CUSTOM1`-`CUSTOM5` (or `C1`-`C5`) `=` a
/// decimal number, and each slot may appear once; anything else is damage.
fn parse_saved_values(bytes: &[u8]) -> Result<BTreeMap<String, u32>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "the trainer option file is not UTF-8 text".to_string())?;
    let mut values = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let content = line.split_once(';').map_or(line, |(value, _)| value).trim();
        let (key, value) = content.split_once('=').unwrap_or((content, ""));
        let key = key.trim().to_ascii_uppercase();
        if !custom_like(&key) {
            continue;
        }
        let malformed = || format!("line {} of the trainer option file is malformed", index + 1);
        let slot = match key.as_str() {
            "CUSTOM1" | "C1" => "CUSTOM1",
            "CUSTOM2" | "C2" => "CUSTOM2",
            "CUSTOM3" | "C3" => "CUSTOM3",
            "CUSTOM4" | "C4" => "CUSTOM4",
            "CUSTOM5" | "C5" => "CUSTOM5",
            _ => return Err(malformed()),
        };
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed());
        }
        let value = value.parse::<u32>().map_err(|_| malformed())?;
        if values.insert(slot.to_string(), value).is_some() {
            return Err(format!(
                "the trainer option file sets {slot} more than once"
            ));
        }
    }
    Ok(values)
}

fn bit_mask(option: &TrainerOption) -> Option<u32> {
    let (lo, hi) = option.bit_range?;
    let width = u32::from(hi - lo + 1);
    let low = if width >= 32 {
        u32::MAX
    } else {
        (1u32 << width) - 1
    };
    Some(low << lo)
}

/// Checks one saved slot value against the slave's declarations for that
/// slot, projected with the saved bytes.
fn check_slot(slot: &str, value: u32, options: &[&TrainerOption]) -> Result<(), String> {
    if options.is_empty() {
        return Err(format!(
            "the saved {slot} is not declared by this game's slave; review the trainer options again"
        ));
    }
    if options.iter().any(|option| {
        option.kind == TrainerOptionKind::Opaque
            || option.readiness != TrainerOptionReadiness::Ready
    }) {
        return Err(format!(
            "the slave's declaration for {slot} cannot be interpreted, so its saved value is not used"
        ));
    }
    let masks: Vec<Option<u32>> = options.iter().map(|option| bit_mask(option)).collect();
    if masks.iter().all(Option::is_some) {
        let declared = masks.iter().flatten().fold(0u32, |all, mask| all | mask);
        if value & !declared != 0 {
            return Err(format!(
                "the saved {slot} sets bits no trainer option declares"
            ));
        }
        return Ok(());
    }
    let [option] = options else {
        return Err(format!(
            "several declarations share {slot} in a way that makes its saved value ambiguous"
        ));
    };
    let valid = match option.kind {
        TrainerOptionKind::Boolean => value <= 1,
        TrainerOptionKind::Enum => option.current_value.is_some(),
        TrainerOptionKind::Numeric => true,
        TrainerOptionKind::Bitfield | TrainerOptionKind::Opaque => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "the saved {slot} value is not one this game's slave allows"
        ))
    }
}

/// Resolves the trainer options saved for the game in `input`.
///
/// `inspection` is the trusted inspection of that game (the same one the
/// trainer editor used), and `option_file` is core's storage contract,
/// normally `patch_manager::whdload_trainer_option_file`, mapped to a plain
/// error message. It is only called for a `Ready` discovery with a verified
/// identity.
pub fn resolve_saved_whdload_launch_trainer(
    input: &WHDLoadLaunchInput,
    inspection: &AmigaGameInspection,
    option_file: impl FnOnce(&WhdloadTrainerDiscovery) -> Result<PathBuf, String>,
) -> WhdloadLaunchTrainerResolution {
    let (Some(target), Some(slave)) = (input.target.as_ref(), input.slave.as_ref()) else {
        return not_applicable("no verified WHDLoad game and slave were selected");
    };
    if target.format != WhdloadPackageFormat::InstalledDirectory {
        return not_applicable(
            "this game is a packaged archive; trainer options need an installed WHDLoad slave",
        );
    }
    let CanonicalIdentityStatus::Resolved(resolved) = &input.identity else {
        return not_applicable("the game identity is not verified");
    };

    let discovery = discover_whdload_trainer_options(inspection, &[]);
    match discovery.state {
        WhdloadTrainerDiscoveryState::Ready => {}
        WhdloadTrainerDiscoveryState::SlaveChanged => {
            return refused(
                "the WHDLoad slave changed since the game was inspected; inspect it again",
            );
        }
        state => return not_applicable(state.plain_label()),
    }
    let Some(identity) = discovery.identity.as_ref() else {
        return not_applicable("the game identity is not verified");
    };
    if identity.verified_game_identity != resolved.game_key
        || identity.verified_game_identity != input.profile.verified_identity
    {
        return refused("the inspected game is not the game being launched");
    }
    if identity.slave_path != slave.artifact_path {
        return refused("the inspected slave is not the slave being launched");
    }

    let path = match option_file(&discovery) {
        Ok(path) => path,
        Err(error) => return refused(error),
    };
    let bytes = match read_option_file(&path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return WhdloadLaunchTrainerResolution::NoSavedOptions,
        Err(error) => return refused(error),
    };
    let saved = match parse_saved_values(&bytes) {
        Ok(saved) => saved,
        Err(error) => return refused(error),
    };
    // Re-read the slave with the saved values: the declarations, the hash and
    // the current values all come from one pass over the same bytes.
    let projected = discover_whdload_trainer_options(inspection, &bytes);
    if projected.state != WhdloadTrainerDiscoveryState::Ready
        || projected.slave_sha256 != discovery.slave_sha256
        || projected.slave_path != discovery.slave_path
    {
        return refused("the WHDLoad slave changed while its trainer options were being read");
    }
    for (slot, value) in &saved {
        let options: Vec<&TrainerOption> = projected
            .options
            .iter()
            .filter(|option| &option.custom_slot == slot)
            .collect();
        if let Err(error) = check_slot(slot, *value, &options) {
            return refused(error);
        }
    }
    // WHDLoad treats an absent CUSTOMn as 0, so a cleared slot adds nothing
    // and "everything off" launches exactly like no trainer at all.
    let arguments: Vec<String> = saved
        .iter()
        .filter(|(_, value)| **value != 0)
        .map(|(slot, value)| format!("{slot}={value}"))
        .collect();
    if arguments.is_empty() {
        return WhdloadLaunchTrainerResolution::NoSavedOptions;
    }
    match project_whdload_trainer_launch_options(input.profile.emulator, arguments) {
        WhdloadTrainerLaunchProjection::FsUaeArguments(arguments) => {
            WhdloadLaunchTrainerResolution::Apply(WhdloadLaunchTrainer {
                verified_game_identity: identity.verified_game_identity.clone(),
                slave_name: slave.name.clone(),
                slave_path: identity.slave_path.clone(),
                slave_sha256: identity.expected_slave_sha256.clone(),
                option_file: path,
                option_file_sha256: sha256_hex(&bytes),
                arguments,
            })
        }
        WhdloadTrainerLaunchProjection::AmiberryPreviewOnly { reason } => not_applicable(reason),
    }
}

/// The FS-UAE WHDLoad plan with the game's saved trainer options applied.
///
/// `Apply` adds them to `--x-whdload-args`; `NoSavedOptions` and
/// `NotApplicable` give exactly [`build_fsuae_whdload_command_plan`];
/// `Refused` blocks the launch with the reason. Launch the returned plan with
/// `preflight_and_launch_amiga_whdload_with_trainer`, passing the trainer from
/// the resolution, so the slave and option file are re-checked before spawn.
pub fn plan_fsuae_whdload_launch_with_saved_trainers(
    input: &WHDLoadLaunchInput,
    inspection: &AmigaGameInspection,
    option_file: impl FnOnce(&WhdloadTrainerDiscovery) -> Result<PathBuf, String>,
) -> (AmigaWHDLoadCommandPlan, WhdloadLaunchTrainerResolution) {
    let resolution = resolve_saved_whdload_launch_trainer(input, inspection, option_file);
    let plan = match &resolution {
        WhdloadLaunchTrainerResolution::Apply(trainer) => {
            build_fsuae_whdload_command_plan_with_trainer(input, Some(trainer))
        }
        WhdloadLaunchTrainerResolution::Refused { reason } => {
            let mut plan = build_fsuae_whdload_command_plan(input);
            plan.command = None;
            plan.readiness = LaunchReadiness::Blocked;
            plan.blockers.push(LaunchBlocker::new(
                LaunchBlockerKind::WhdloadTrainerOptionsRefused,
                reason.clone(),
            ));
            plan
        }
        WhdloadLaunchTrainerResolution::NoSavedOptions
        | WhdloadLaunchTrainerResolution::NotApplicable { .. } => {
            build_fsuae_whdload_command_plan(input)
        }
    };
    (plan, resolution)
}

/// The trainer a resolution carries, for the trainer-aware preflight.
impl WhdloadLaunchTrainerResolution {
    #[must_use]
    pub fn trainer(&self) -> Option<&WhdloadLaunchTrainer> {
        match self {
            Self::Apply(trainer) => Some(trainer),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
