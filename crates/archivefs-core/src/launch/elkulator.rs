//! Acorn Electron readiness preview for Elkulator; never an execution adapter.
//!
//! The audited Allegro 4 and Allegro 5 builds use different config/resource
//! roots. No installed binary's containment or firmware provisioning contract
//! has been verified. SSD/DSD can be written; there is no read-only CLI switch.
//! See `docs/research/ACORN_ELECTRON_NATIVE_ADAPTER.md` for pinned upstream
//! evidence. Reuse the private, non-executable `NativePreview` until those
//! contracts are proven. DFS/UEF structure never resolves BBC versus Electron.

use std::ffi::OsStr;
use std::path::PathBuf;

use super::native_support::{
    ExecutableDiscovery, NativeAdapterClassification, NativePreview, PreviewError, PreviewRequest,
    plan_preview,
};
use super::planning::CanonicalIdentityStatus;
use super::safe_launch_sandbox::{MediaKind, SourceProvenance};
use crate::identity_source::model::LocalEvidenceStrength;

pub mod inspection;

pub const ADAPTER_ID: &str = "elkulator";
pub const PLATFORM_ID: &str = "Acorn Electron";
pub const CLASSIFICATION: NativeAdapterClassification =
    NativeAdapterClassification::PreviewReadinessOnly;
pub const UNPROVEN: &[&str] = &[
    "config/state containment is not verified for the selected executable: Allegro 4 uses the executable directory, Allegro 5 uses a relative root; HOME/XDG does not select elk.cfg",
    "SSD/DSD are opened rb+ and can be rewritten; no read-only CLI switch exists, so any future disk launch must use a verified disposable copy with no write-back",
    "firmware/resource provisioning and the installed build's CLI/config contract remain unverified; no emulator or ROMs are executed or supplied by this preview",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElkulatorMediaFormat {
    Ssd,
    Dsd,
    Uef,
    Csw,
    Wav,
    Rom,
    Snapshot,
    Adf,
    Fdi,
    Unknown,
}

impl ElkulatorMediaFormat {
    fn plan_details(self) -> Result<(MediaKind, &'static str), PreviewError> {
        match self {
            Self::Ssd => Ok((MediaKind::Floppy, "ssd")),
            Self::Dsd => Ok((MediaKind::Floppy, "dsd")),
            Self::Uef => Ok((MediaKind::Tape, "uef")),
            _ => Err(PreviewError::UnsupportedMedia(
                "Elkulator preview accepts only structurally verified SSD, DSD and UEF; other media are not audited here",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElkulatorProfile {
    /// Explicit selection from discovery; discovery never selects a winner.
    pub executable: PathBuf,
    pub disposable_session_acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElkulatorMedia {
    pub path: PathBuf,
    /// Selects structural validation, never platform identity.
    pub format: ElkulatorMediaFormat,
    /// Already resolved upstream. Choosing this adapter cannot resolve it.
    pub identity: CanonicalIdentityStatus,
    pub platform_evidence: LocalEvidenceStrength,
    pub verified_source_sha256: [u8; 32],
}

pub fn discover_executables(explicit: &[PathBuf], path_env: Option<&OsStr>) -> ExecutableDiscovery {
    super::native_support::discover_executables(explicit, path_env, &[ADAPTER_ID])
}

fn validate(media: &ElkulatorMedia, source: &SourceProvenance) -> Result<(), PreviewError> {
    inspection::inspect_structure(media.format, source)
        .map(|_| ())
        .map_err(|_| {
            PreviewError::UnsupportedMedia(
                "media failed the existing bounded DFS or UEF structural validation",
            )
        })
}

/// Hash-bound validation only. Requires verified, resolved Electron identity;
/// unknown/conflicting identity and BBC identity stay refused even for valid
/// family media or a person-selected executable. No argv/prepare/spawn exists.
pub fn preview_elkulator(
    profile: &ElkulatorProfile,
    media: &ElkulatorMedia,
) -> Result<NativePreview, PreviewError> {
    let (kind, suffix) = media.format.plan_details()?;
    plan_preview(
        PreviewRequest {
            adapter: ADAPTER_ID,
            platform_id: PLATFORM_ID,
            executable: &profile.executable,
            media: &media.path,
            kind,
            suffix,
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
#[path = "elkulator/tests.rs"]
mod tests;
