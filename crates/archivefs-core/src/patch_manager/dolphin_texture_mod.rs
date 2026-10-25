//! The first non-cheat Dolphin mod slice: installing exactly one explicitly
//! selected PNG texture file into
//! `<Dolphin profile>/Load/Textures/<verified GameID>/<original filename>.png`,
//! through the existing shared preview/transaction pipeline.
//!
//! # Scope
//!
//! Deliberately narrow. This module does not, and must not grow to:
//! - unpack ZIP/RAR/7z texture packs, import a whole directory, or read a
//!   pack manifest;
//! - install more than one file per install;
//! - edit any Dolphin configuration or a texture pack's enable/disable
//!   state (Dolphin's own `GFX.ini` `[General] HiresTextures` toggle is
//!   never touched);
//! - replace an existing, *different* file - see [`DolphinTextureModPlan::Conflict`];
//! - do anything with a standalone emulator process.
//!
//! # What this module is not
//!
//! It never writes to disk itself: [`build_dolphin_texture_mod_preview`] is
//! read-only (it reads the candidate source file's bytes once, to hash
//! them, and reads whatever [`crate::patch_manager::build_shared_preview`]
//! itself reads). Every actual write goes through the caller's own
//! [`crate::patch_manager::build_shared_transaction_plan`]/
//! [`crate::patch_manager::execute_shared_apply`]/
//! [`crate::patch_manager::execute_shared_rollback`] calls - this module
//! only prepares the exact, narrow inputs those already-reviewed functions
//! need, and interprets their preview result honestly (see the
//! [`DolphinTextureModPlan`] variants).

use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::DolphinProfile;
use super::shared_preview::{
    PreviewAdapter, PreviewIdentity, PreviewIdentityKind, PreviewIdentityState,
    PreviewMatchStrength, PreviewProposedAction, PreviewSourceItem, SharedPreviewReport,
    SharedPreviewRequest, build_shared_preview,
};
use super::shared_transaction::SHARED_MAX_SOURCE_BYTES;
use crate::game_identity::{GameIdentityReport, IdentityPlatform};

/// The `source_mode` string this feature always records on its
/// [`crate::patch_manager::SharedApplyContext`] - distinct from any other
/// adapter's own source-mode strings, so a journal alone can always say
/// which feature produced it.
pub const DOLPHIN_TEXTURE_MOD_SOURCE_MODE: &str = "dolphin_texture_mod";

/// The one destination folder name every install lives under, directly
/// beneath the resolved Dolphin mods root - Dolphin's own convention for
/// hires texture packs (`Load/Textures/<GameID>/...`).
const TEXTURES_FOLDER_NAME: &str = "Textures";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DolphinTextureModErrorKind {
    /// The identity report handed in belongs to a different archive than
    /// the one currently selected - never trusted, regardless of how
    /// recently it was computed.
    IdentityArchiveMismatch,
    /// `report.platform` is not GameCube or Wii - Dolphin texture mods
    /// (`Load/Textures/<GameID>`) are a GameCube/Wii-disc convention only.
    WrongPlatform,
    /// No `IdentityStatus::Verified` Dolphin GameID exists on the report -
    /// covers missing, candidate-only, ambiguous, and conflicting evidence
    /// alike, since [`GameIdentityReport::verified_dolphin_game_id`] itself
    /// only ever returns a value for genuinely verified evidence.
    IdentityUnverified,
    /// The verified GameID string is not exactly one safe, normal path
    /// component.
    UnsafeGameId,
    /// The selected Dolphin profile itself reports `eligible: false`.
    ProfileIneligible,
    /// The selected profile has no resolved mods directory to build a
    /// `Textures` root under.
    ProfileRootUnavailable,
    /// The resolved mods directory is not an absolute path.
    ProfileRootUnsafe,
    /// The selected source path could not be inspected at all (does not
    /// exist, or is not reachable).
    SourceNotFound,
    /// The selected source path is not a regular file (directory or
    /// special file).
    SourceNotRegularFile,
    /// The selected source path is a symlink.
    SourceSymlink,
    /// The selected source's extension is not (case-insensitively) `.png`.
    SourceNotPng,
    /// A manifest source is not within its approved expanded-pack root.
    SourceOutsideApprovedScope,
    /// A manifest source no longer matches its declared metadata.
    SourceChanged,
    /// The selected source's own filename is not exactly one safe, normal
    /// path component (or is not valid UTF-8).
    SourceUnsafeFilename,
    /// The selected source exceeds the existing shared-transaction source
    /// size cap ([`SHARED_MAX_SOURCE_BYTES`]) - this module never raises
    /// that limit for itself.
    SourceTooLarge,
    /// [`build_shared_preview`] itself refused the request or failed.
    PreviewFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTextureModError {
    pub kind: DolphinTextureModErrorKind,
    pub detail: String,
}

impl std::fmt::Display for DolphinTextureModError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for DolphinTextureModError {}

fn error(kind: DolphinTextureModErrorKind, detail: impl Into<String>) -> DolphinTextureModError {
    DolphinTextureModError {
        kind,
        detail: detail.into(),
    }
}

/// Whether `value` is exactly one safe, normal path component: non-empty,
/// no separators, no `.`/`..`, no path escape hidden inside what looks
/// like a single name. The one check every "must be a single safe
/// component" requirement in this module (a verified GameID, a source
/// filename) goes through - never a bespoke, possibly-looser check per
/// caller.
fn is_single_safe_component(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let path = Path::new(value);
    let mut components = path.components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(component)), None) if component == std::ffi::OsStr::new(value)
    )
}

/// The verified identity a texture-mod install may safely target, derived
/// from an already-loaded [`GameIdentityReport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTextureModIdentity {
    pub game_id: String,
    pub platform: IdentityPlatform,
}

/// Derives the verified Dolphin GameID (and confirms the GameCube/Wii
/// platform) a texture-mod install may safely target from `report` - the
/// already-loaded [`GameIdentityReport`] for the currently selected
/// archive.
///
/// Never derives a GameID from a filename, a Dolphin folder name, a
/// `GameSettings` filename, or any other guess - only
/// [`GameIdentityReport::verified_dolphin_game_id`], which itself only
/// returns a value when the identity layer already marked that evidence
/// [`crate::game_identity::IdentityStatus::Verified`]; candidate,
/// ambiguous, missing, or conflicting evidence can never reach this.
///
/// Fails closed when `report.archive_path` does not match
/// `expected_archive_path` - the caller's own proof that the report it is
/// holding is still the one for whichever archive is currently selected,
/// not a stale report left over from a previous selection.
pub fn verified_dolphin_texture_identity(
    report: &GameIdentityReport,
    expected_archive_path: &Path,
) -> Result<DolphinTextureModIdentity, DolphinTextureModError> {
    if report.archive_path != expected_archive_path {
        return Err(error(
            DolphinTextureModErrorKind::IdentityArchiveMismatch,
            "identity report belongs to a different archive than the one currently selected",
        ));
    }
    if !matches!(
        report.platform,
        IdentityPlatform::GameCube | IdentityPlatform::Wii
    ) {
        return Err(error(
            DolphinTextureModErrorKind::WrongPlatform,
            "Dolphin texture mods require a verified GameCube or Wii identity",
        ));
    }
    let Some(game_id) = report.verified_dolphin_game_id() else {
        return Err(error(
            DolphinTextureModErrorKind::IdentityUnverified,
            "no verified Dolphin GameID is available for this archive",
        ));
    };
    if !is_single_safe_component(game_id) {
        return Err(error(
            DolphinTextureModErrorKind::UnsafeGameId,
            "verified GameID is not a single safe path component",
        ));
    }
    Ok(DolphinTextureModIdentity {
        game_id: game_id.to_string(),
        platform: report.platform,
    })
}

fn preview_platform_label(
    platform: IdentityPlatform,
) -> Result<&'static str, DolphinTextureModError> {
    match platform {
        IdentityPlatform::GameCube => Ok("GameCube"),
        IdentityPlatform::Wii => Ok("Wii"),
        _ => Err(error(
            DolphinTextureModErrorKind::WrongPlatform,
            "internal: a non-GameCube/Wii platform reached preview construction",
        )),
    }
}

/// `<profile>/Load/Textures` for `profile` - the exact destination root a
/// texture-mod install targets. Never triggers a fresh Dolphin scan: the
/// caller already holds the retained
/// [`crate::patch_manager::DolphinProfileDiscovery`]/[`DolphinProfile`];
/// this is a small, pure derivation from it, not a rescan.
pub fn dolphin_texture_mod_destination_root(
    profile: &DolphinProfile,
) -> Result<PathBuf, DolphinTextureModError> {
    if !profile.eligible {
        return Err(error(
            DolphinTextureModErrorKind::ProfileIneligible,
            "the selected Dolphin profile is not eligible",
        ));
    }
    let Some(mods_root) = profile.resolved.destinations.mods.as_ref() else {
        return Err(error(
            DolphinTextureModErrorKind::ProfileRootUnavailable,
            "the selected Dolphin profile has no resolved mods directory",
        ));
    };
    if !mods_root.is_absolute() {
        return Err(error(
            DolphinTextureModErrorKind::ProfileRootUnsafe,
            "the resolved Dolphin mods directory is not an absolute path",
        ));
    }
    Ok(mods_root.join(TEXTURES_FOLDER_NAME))
}

/// One user-selected, already-validated PNG source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTextureModSource {
    pub path: PathBuf,
    pub file_name: String,
}

/// Validates a single, explicitly user-selected candidate texture file.
/// Never called on more than one path at a time - "the user may choose
/// exactly one file" is enforced by this function's own signature, not by
/// a separate count check.
pub fn validate_dolphin_texture_source(
    path: &Path,
) -> Result<DolphinTextureModSource, DolphinTextureModError> {
    let metadata = fs::symlink_metadata(path).map_err(|io_error| {
        error(
            DolphinTextureModErrorKind::SourceNotFound,
            format!("{}: {io_error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(error(
            DolphinTextureModErrorKind::SourceSymlink,
            "the selected file is a symlink",
        ));
    }
    if !metadata.is_file() {
        return Err(error(
            DolphinTextureModErrorKind::SourceNotRegularFile,
            "the selected path is not a regular file",
        ));
    }
    if metadata.len() > SHARED_MAX_SOURCE_BYTES {
        return Err(error(
            DolphinTextureModErrorKind::SourceTooLarge,
            format!(
                "source is {} byte(s), over the {SHARED_MAX_SOURCE_BYTES}-byte shared-transaction \
                 limit",
                metadata.len()
            ),
        ));
    }
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(error(
            DolphinTextureModErrorKind::SourceUnsafeFilename,
            "filename is missing or is not valid UTF-8",
        ));
    };
    if !is_single_safe_component(file_name) {
        return Err(error(
            DolphinTextureModErrorKind::SourceUnsafeFilename,
            "filename is not a single safe path component",
        ));
    }
    let extension_is_png = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !extension_is_png {
        return Err(error(
            DolphinTextureModErrorKind::SourceNotPng,
            "source file must have a .png extension",
        ));
    }
    Ok(DolphinTextureModSource {
        path: path.to_path_buf(),
        file_name: file_name.to_string(),
    })
}

/// Everything [`build_dolphin_texture_mod_preview`] needs, already gated by
/// the caller through [`verified_dolphin_texture_identity`],
/// [`dolphin_texture_mod_destination_root`], and
/// [`validate_dolphin_texture_source`] - this struct carries their outputs,
/// never re-derives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DolphinTextureModPreviewRequest {
    pub selected_archive: PathBuf,
    pub identity: DolphinTextureModIdentity,
    pub destination_root: PathBuf,
    pub source: DolphinTextureModSource,
}

/// How this module interprets an honest [`SharedPreviewReport`] for a
/// single-file Dolphin texture-mod install. This is a policy *wrapper*
/// around the shared preview's own, unmodified semantics - it never alters
/// `crate::patch_manager::shared_preview`'s generic replacement rules, it
/// only decides, for this one narrow feature, which of those outcomes may
/// ever lead to a transaction:
///
/// - [`PreviewProposedAction::Install`] (destination `Missing`) ->
///   [`Self::Install`] - the only variant a caller may build a
///   [`crate::patch_manager::SharedTransactionPlan`] from.
/// - [`PreviewProposedAction::Skip`] (destination `RegularFileIdentical`) ->
///   [`Self::AlreadyInstalled`] - nothing to apply.
/// - [`PreviewProposedAction::Replace`] (destination `RegularFileDifferent`)
///   -> [`Self::Conflict`] - a **hard** conflict. This module never
///   constructs a `Replace` transaction for a texture mod, regardless of
///   what generic replacement approval the shared transaction system would
///   otherwise allow; a different file already at this exact path is
///   always refused, never silently overwritten.
/// - Anything else (symlink, special file, directory, inaccessible,
///   changed-during-inspection, unavailable, or no entry at all) ->
///   [`Self::Blocked`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DolphinTextureModPlan {
    Install { report: SharedPreviewReport },
    AlreadyInstalled { report: SharedPreviewReport },
    Conflict { report: SharedPreviewReport },
    Blocked { report: SharedPreviewReport },
}

impl DolphinTextureModPlan {
    pub fn report(&self) -> &SharedPreviewReport {
        match self {
            Self::Install { report }
            | Self::AlreadyInstalled { report }
            | Self::Conflict { report }
            | Self::Blocked { report } => report,
        }
    }
}

/// Builds the honest, read-only preview for one Dolphin texture-mod
/// install, using the existing shared preview pipeline
/// ([`build_shared_preview`]) with exactly the configuration this feature
/// requires: `PreviewAdapter::Dolphin`, `PreviewIdentityKind::DolphinGameId`,
/// `PreviewIdentityState::Verified`, `PreviewMatchStrength::VerifiedExact`,
/// one [`PreviewSourceItem`] whose `destination_relative_paths` is exactly
/// `[<GameID>, <original filename>.png]` beneath `request.destination_root`.
///
/// Reads `request.source.path`'s bytes once (bounded by the same
/// [`SHARED_MAX_SOURCE_BYTES`] cap [`validate_dolphin_texture_source`]
/// already checked) to compute its digest up front and hand it to the
/// preview as `expected_source_digest` - closing the gap between whatever
/// bytes were inspected when the source was validated and whatever bytes
/// the shared preview itself hashes, rather than trusting two separate
/// reads to agree.
pub fn build_dolphin_texture_mod_preview(
    request: &DolphinTextureModPreviewRequest,
) -> Result<DolphinTextureModPlan, DolphinTextureModError> {
    let platform_label = preview_platform_label(request.identity.platform)?;

    let source_bytes = fs::read(&request.source.path).map_err(|io_error| {
        error(
            DolphinTextureModErrorKind::SourceNotFound,
            format!("{}: {io_error}", request.source.path.display()),
        )
    })?;
    if source_bytes.len() as u64 > SHARED_MAX_SOURCE_BYTES {
        return Err(error(
            DolphinTextureModErrorKind::SourceTooLarge,
            format!(
                "source is {} byte(s), over the {SHARED_MAX_SOURCE_BYTES}-byte shared-transaction \
                 limit",
                source_bytes.len()
            ),
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(&source_bytes);
    let digest = hex_digest(&hasher.finalize());

    let destination_relative =
        PathBuf::from(&request.identity.game_id).join(&request.source.file_name);

    let preview_request = SharedPreviewRequest {
        adapter: PreviewAdapter::Dolphin,
        selected_archive: request.selected_archive.clone(),
        platform: Some(platform_label.to_string()),
        identity: PreviewIdentity {
            kind: PreviewIdentityKind::DolphinGameId,
            state: PreviewIdentityState::Verified,
            value: Some(request.identity.game_id.clone()),
            archive_path: request.selected_archive.clone(),
            revision: None,
        },
        destination_root: request.destination_root.clone(),
        source_items: vec![PreviewSourceItem {
            adapter: PreviewAdapter::Dolphin,
            source_path: request.source.path.clone(),
            expected_source_digest: Some(digest),
            destination_relative_paths: vec![destination_relative],
            match_strength: PreviewMatchStrength::VerifiedExact,
        }],
    };
    let report = build_shared_preview(&preview_request).map_err(|preview_error| {
        error(
            DolphinTextureModErrorKind::PreviewFailed,
            preview_error.to_string(),
        )
    })?;
    Ok(classify_preview(report))
}

fn classify_preview(report: SharedPreviewReport) -> DolphinTextureModPlan {
    let proposed_action = report.entries.first().map(|entry| entry.proposed_action);
    match proposed_action {
        Some(PreviewProposedAction::Install) => DolphinTextureModPlan::Install { report },
        Some(PreviewProposedAction::Skip) => DolphinTextureModPlan::AlreadyInstalled { report },
        Some(PreviewProposedAction::Replace) => DolphinTextureModPlan::Conflict { report },
        Some(PreviewProposedAction::Blocked) | None => DolphinTextureModPlan::Blocked { report },
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests;
