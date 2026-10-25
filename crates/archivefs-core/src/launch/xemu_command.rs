//! Read-only native xemu command planning.
//!
//! # xemu's real launch contract (verified against official documentation)
//!
//! xemu's own documented command-line arguments
//! (`https://xemu.app/docs/cli/`) include `-dvd_path <iso>` - the flag this
//! module uses to select the game disc image for one run. Confirmed via
//! xemu's own issue tracker (`xemu-project/xemu#1715`) that `-dvd_path` is a
//! **one-time runtime override**: it does not write back to `xemu.toml`, so
//! passing it never mutates the user's permanent configuration. This module
//! therefore never reads, writes, or copies any xemu configuration file -
//! the user's existing MCPX/flash BIOS/EEPROM/HDD paths already in their
//! `xemu.toml` are left completely alone and simply apply as normal; only
//! the disc image is overridden, for this one launch, via the flag.
//!
//! No CLI flag is invented here: `-dvd_path` is the only argument this
//! module ever emits, and it is exactly the flag xemu's own documentation
//! names for exactly this purpose.
//!
//! # xemu cannot boot a loose XBE directly
//!
//! A prior investigation established that xemu has no mechanism to boot a
//! standalone `.xbe` file - it always boots through the full Xbox hardware
//! chain (MCPX bootloader -> dashboard -> DVD drive), which requires a real
//! disc-shaped image, not a bare executable. That finding is not revisited
//! or regressed here: this module accepts only a verified original-Xbox
//! **disc image** (`.iso`/`.xiso`, produced by
//! [`crate::game_identity`]'s XDVDFS disc-image identity path) as runnable
//! content. A direct loose `.xbe` or a ZIP containing one is genuine,
//! verified identity but is refused here as unsupported content - exactly
//! like every other native launch slice in this crate refuses a format its
//! own identity layer can prove but its target emulator cannot run.
//!
//! # Firmware/readiness: reused, never duplicated
//!
//! xemu requires four system files to boot at all - MCPX boot ROM, flash
//! BIOS, EEPROM, and an HDD image. This module reads their already-modeled
//! [`crate::patch_manager::XemuHealth`]/[`crate::patch_manager::XemuSystemFileState`]
//! and projects each one through the existing, unchanged
//! [`crate::launch::readiness::xemu_firmware_readiness`] - the exact same
//! projection every other xemu-aware code path already uses. No new BIOS/
//! firmware state, enum, or verification is invented here.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::launch::planning::{
    CanonicalIdentityStatus, LaunchCandidate, LaunchContainerKind, LaunchContentKind, LaunchTarget,
};
use crate::launch::readiness::{
    FirmwareReadiness, LaunchBlocker, LaunchBlockerKind, LaunchReadiness, xemu_firmware_readiness,
};
use crate::patch_manager::{XemuHealth, XemuLaunchBlocker, XemuNativeLaunchBinding};

/// The only platform this native launch slice supports.
pub const XEMU_SUPPORTED_PLATFORM_ID: &str = "Xbox";

/// The only direct content extensions this slice supports (lowercase, no
/// dot) - a loose `.xbe`, a ZIP-contained `.xbe`, and any other archive/
/// mount-input format are all refused. See the module documentation for why
/// a disc image is required at all.
const XEMU_SUPPORTED_EXTENSIONS: &[&str] = &["iso", "xiso"];

/// The exact xemu CLI flag used to select the disc image for one run - see
/// the module documentation for why this, and only this, flag is used.
const XEMU_DVD_PATH_FLAG: &str = "-dvd_path";

/// The executable invocation data for an xemu launch that has passed every
/// fail-closed check. This is data only: no type in this module implements
/// process spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuCommand {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub selection: XemuCommandSelection,
}

/// The facts that produced the command's argv - profile, platform, verified
/// XBE title ID, and the disc-image content path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuCommandSelection {
    pub profile_id: String,
    pub platform_id: String,
    pub verified_xbox_title_id: String,
    pub content_path: PathBuf,
}

/// A successful command, or the structured reasons a command was withheld.
/// `command` is `None` whenever `blockers` is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XemuCommandPlan {
    pub command: Option<XemuCommand>,
    pub blockers: Vec<LaunchBlocker>,
}

impl XemuCommandPlan {
    fn blocked(blockers: Vec<LaunchBlocker>) -> Self {
        debug_assert!(!blockers.is_empty());
        Self {
            command: None,
            blockers,
        }
    }
}

fn blocker(kind: LaunchBlockerKind, detail: impl Into<String>) -> LaunchBlocker {
    LaunchBlocker::new(kind, detail)
}

pub(crate) fn direct_xbox_disc_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            XEMU_SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

/// Every firmware/system-file blocker `health` currently has, in a fixed,
/// deterministic order. Reuses [`xemu_firmware_readiness`] unchanged for
/// each of the four required files - never a new BIOS check. Only
/// [`FirmwareReadiness::Missing`] blocks, exactly like the shared
/// [`crate::launch::planning`] firmware gate for every other adapter;
/// `PresentUnverified`/`Unknown` are never treated as a proven failure.
fn firmware_blockers(health: &XemuHealth) -> Vec<LaunchBlocker> {
    let checks: [(&str, crate::patch_manager::XemuSystemFileState); 4] = [
        ("MCPX boot ROM", health.mcpx),
        ("flash BIOS", health.flash_bios),
        ("EEPROM", health.eeprom),
        ("HDD image", health.hdd),
    ];
    checks
        .into_iter()
        .filter(|(_, state)| xemu_firmware_readiness(*state) == FirmwareReadiness::Missing)
        .map(|(name, _)| {
            blocker(
                LaunchBlockerKind::RequiredFirmwareMissing,
                format!("{name} is missing"),
            )
        })
        .collect()
}

/// Builds a safe xemu argv plan from only an already-authorized launch
/// candidate, an already-computed launch binding result, the verified XBE
/// title ID the caller freshly re-confirmed, and xemu's own already-
/// inspected system-file health.
///
/// `binding` is a `Result` rather than a bare [`XemuNativeLaunchBinding`] so
/// a caller's fresh [`crate::patch_manager::resolve_xemu_native_launch_binding`]
/// failure (missing/unsafe executable, unsupported installation type, etc.)
/// flows straight into this plan's blockers instead of forcing the caller
/// to invent a placeholder success value.
pub fn build_xemu_command_plan(
    identity: &CanonicalIdentityStatus,
    verified_xbox_title_id: Option<&str>,
    candidate: &LaunchCandidate,
    binding: &Result<XemuNativeLaunchBinding, XemuLaunchBlocker>,
    health: &XemuHealth,
) -> XemuCommandPlan {
    let mut blockers = Vec::new();

    let resolved = match identity {
        CanonicalIdentityStatus::Resolved(resolved) => Some(resolved),
        CanonicalIdentityStatus::Unknown => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityUnresolved,
                "canonical game identity could not be resolved",
            ));
            None
        }
        CanonicalIdentityStatus::Conflicting => {
            blockers.push(blocker(
                LaunchBlockerKind::IdentityConflict,
                "canonical game identity evidence conflicts and was not resolved to one answer",
            ));
            None
        }
    };
    if let Some(resolved) = resolved
        && resolved.platform_id != XEMU_SUPPORTED_PLATFORM_ID
    {
        blockers.push(blocker(
            LaunchBlockerKind::XemuPlatformMismatch,
            format!(
                "resolved identity targets {}, but only {XEMU_SUPPORTED_PLATFORM_ID} is \
                 supported by this native xemu launch slice",
                resolved.platform_id
            ),
        ));
    }
    if verified_xbox_title_id.is_none() {
        blockers.push(blocker(
            LaunchBlockerKind::XemuTitleIdMissing,
            "no verified original-Xbox XBE title ID is available for this content",
        ));
    }

    let LaunchTarget::Standalone {
        adapter_id,
        profile_id,
        ..
    } = &candidate.target
    else {
        blockers.push(blocker(
            LaunchBlockerKind::XemuCandidateRequired,
            "the supplied launch candidate does not target a standalone adapter",
        ));
        return XemuCommandPlan::blocked(blockers);
    };
    if *adapter_id != "xemu" {
        blockers.push(blocker(
            LaunchBlockerKind::XemuCandidateRequired,
            format!("the supplied launch candidate targets adapter `{adapter_id}`, not `xemu`"),
        ));
    }

    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        blockers.extend(candidate.blockers.iter().cloned());
        if candidate.blockers.is_empty() {
            blockers.push(blocker(
                LaunchBlockerKind::CandidateBlocked,
                "the supplied xemu launch candidate is marked blocked",
            ));
        }
    }

    if candidate.content.requires_mount {
        blockers.push(blocker(
            LaunchBlockerKind::ContentNotResolved,
            "content requires a mount that has not been performed, so no command can be produced",
        ));
    }
    let content_path = match (
        &candidate.content.resolved_path,
        candidate.content.container,
    ) {
        (Some(path), Some(LaunchContainerKind::PlainFile))
            if !candidate.content.requires_mount
                && candidate.content.kind == Some(LaunchContentKind::OpticalDisc)
                && direct_xbox_disc_extension(path) =>
        {
            Some(path.clone())
        }
        (Some(_), _) => {
            blockers.push(blocker(
                LaunchBlockerKind::XemuContentFormatUnsupported,
                "only a direct, non-archived .iso/.xiso original-Xbox disc image is supported - \
                 a loose .xbe (xemu cannot boot one directly) and a ZIP-contained .xbe are both \
                 refused",
            ));
            None
        }
        (None, _) => {
            blockers.push(blocker(
                LaunchBlockerKind::ContentNotResolved,
                "no resolved runnable Xbox disc image path is available",
            ));
            None
        }
    };

    let binding = match binding {
        Ok(binding) => Some(binding),
        Err(error) => {
            blockers.push(blocker(
                LaunchBlockerKind::XemuBindingUnavailable,
                format!("{:?}: {}", error.kind, error.detail),
            ));
            None
        }
    };

    blockers.extend(firmware_blockers(health));

    if !blockers.is_empty() {
        return XemuCommandPlan::blocked(blockers);
    }

    let resolved = resolved.expect("identity is Resolved when no blockers exist");
    let verified_xbox_title_id = verified_xbox_title_id
        .expect("a verified Xbox title ID is required when no blockers exist");
    let content_path =
        content_path.expect("a resolved content path is required when no blockers exist");
    let binding = binding.expect("a launch binding is required when no blockers exist");

    let arguments = vec![
        OsString::from(XEMU_DVD_PATH_FLAG),
        content_path.clone().into_os_string(),
    ];

    XemuCommandPlan {
        command: Some(XemuCommand {
            executable: binding.executable.clone(),
            arguments,
            working_directory: None,
            selection: XemuCommandSelection {
                profile_id: profile_id.clone(),
                platform_id: resolved.platform_id.clone(),
                verified_xbox_title_id: verified_xbox_title_id.to_string(),
                content_path,
            },
        }),
        blockers: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
