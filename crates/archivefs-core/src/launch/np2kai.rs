//! NP2kai (PC-98) readiness preview.
//!
//! Classification: PREVIEW / READINESS ONLY. NP2kai is not installed here and
//! its launch contract is not established by anything that could be read:
//!
//! * the audit (`docs/research/PC98_NP2KAI_NATIVE_ADAPTER_AUDIT.md`) found no
//!   stable config-selection flag, no write-protect switch and no machine
//!   selector, and says a launch "must refuse" if the config argument cannot be
//!   proven for the selected binary;
//! * upstream places the SDL2 frontend's config under `~/.config/np2kai` and the
//!   X11 frontend's under `~/.config/xnp2kai`, but where CMOS/NVRAM/SRAM and BIOS
//!   state land, and whether HOME isolation redirects them, is unproven;
//! * the positional D88 routing of the installed frontend is unproven.
//!
//! The earlier prototype's weaker D88 check and invented seed syntax
//! (`machine=PC-9801`) are discarded. Media is validated with main's own D88
//! parser (`disk_format`), which checks the header, track table and sector
//! records. HDI/NHD (HDD-class images) stay refused. No `prepare`, `spawn` or
//! argv exists here.

use std::ffi::OsStr;
use std::path::PathBuf;

use super::native_support::{
    ExecutableDiscovery, NativeAdapterClassification, NativePreview, PreviewError, PreviewRequest,
    plan_preview,
};
use super::planning::CanonicalIdentityStatus;
use super::safe_launch_sandbox::{MediaKind, SourceProvenance};
use crate::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};
use crate::identity_source::model::LocalEvidenceStrength;
use crate::safe_read::TrustedRoots;

pub const ADAPTER_ID: &str = "np2kai";
pub const PLATFORM_ID: &str = "PC-98";
pub const CLASSIFICATION: NativeAdapterClassification =
    NativeAdapterClassification::PreviewReadinessOnly;
pub const UNPROVEN: &[&str] = &[
    "the config-selection argument and positional D88 routing of the installed NP2kai frontend are not established",
    "config, CMOS/NVRAM/SRAM and BIOS state live under ~/.config/np2kai or ~/.config/xnp2kai; redirecting them into a sandbox is unproven",
    "NP2kai has no documented write-protect switch for D88, so a launch needs proven scratch media AND a proven scratch state directory",
];

/// One explicit PC-9801-compatible profile; PC-9821 is never inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineProfile {
    Pc9801,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFormat {
    D88,
    /// HDD-class: no overlay/copy-on-write primitive is audited.
    Hdi,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub executable: PathBuf,
    /// `None` until a person chooses; never defaulted.
    pub machine: Option<MachineProfile>,
    pub disposable_session_acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Media {
    pub path: PathBuf,
    pub format: MediaFormat,
    pub identity: CanonicalIdentityStatus,
    pub platform_evidence: LocalEvidenceStrength,
    pub verified_source_sha256: [u8; 32],
}

pub fn discover_executables(explicit: &[PathBuf], path_env: Option<&OsStr>) -> ExecutableDiscovery {
    // Reviewed frontend names only; there is no bare "np2kai" release binary.
    super::native_support::discover_executables(
        explicit,
        path_env,
        &["np2kai", "xnp21kai", "sdlnp21kai"],
    )
}

fn validate(media: &Media, _source: &SourceProvenance) -> Result<(), PreviewError> {
    // main's validator: header, track table and sector records must agree.
    let evidence = inspect_disk_format(
        &media.path,
        &TrustedRoots::none(),
        DiskFormatContext {
            folder_platform: None,
        },
        None,
    );
    if evidence.format != Some(DiskFormat::D88Container) {
        return Err(PreviewError::UnsupportedMedia(
            "D88 structure is invalid, truncated, or not a D88 floppy container",
        ));
    }
    Ok(())
}

pub fn preview(profile: &Profile, media: &Media) -> Result<NativePreview, PreviewError> {
    match media.format {
        MediaFormat::D88 => {}
        MediaFormat::Hdi => {
            return Err(PreviewError::UnsupportedMedia(
                "HDI/HDD media is refused: no read-only mode or audited overlay exists for it",
            ));
        }
        MediaFormat::Unknown => {
            return Err(PreviewError::UnsupportedMedia(
                "NP2kai preview supports D88 floppy images only",
            ));
        }
    }
    if profile.machine.is_none() {
        return Err(PreviewError::MachineRequired);
    }
    plan_preview(
        PreviewRequest {
            adapter: ADAPTER_ID,
            platform_id: PLATFORM_ID,
            executable: &profile.executable,
            media: &media.path,
            kind: MediaKind::Floppy,
            suffix: "d88",
            identity: Some(&media.identity),
            platform_evidence: media.platform_evidence,
            verified_source_sha256: media.verified_source_sha256,
            disposable_session_acknowledged: profile.disposable_session_acknowledged,
            unproven: UNPROVEN,
        },
        |source| validate(media, source),
    )
}

#[cfg(test)]
mod tests;
