//! Oricutron (Oric-1 / Atmos) readiness preview.
//!
//! Classification: PREVIEW / READINESS ONLY. Oricutron is not installed here,
//! and upstream's own source (`main.c`) shows why the earlier design cannot work:
//!
//! * `oricutron.cfg` is opened through `add_fileprefix()`, which on Linux is THE
//!   EXECUTABLE'S OWN DIRECTORY. Neither `HOME`/`XDG_*` nor the working directory
//!   redirects it, so a sandboxed seed would never be read;
//! * ROM paths come only from that config (`atmosrom`, `oric1rom`, `mdiscrom`),
//!   resolved with the same prefix, so a launch cannot be given a reviewed ROM
//!   profile without replacing the user's own config beside the executable;
//! * snapshots, keymaps and `sdcard/` / `usbdrive/` also live under the install
//!   directory. Only modified disks are saved to the disk image's own path.
//!
//! Safe isolation would need a complete scratch COPY of the installation
//! (executable, `roms/`, config), which is neither implemented nor proven here.
//! The prototype's ROM profile also modelled config-referenced ROMs as argv
//! members and validated its seed by length only; both are discarded (the shared
//! sandbox now models config-referenced members explicitly for adapters that can
//! use them).
//!
//! `-m <oric1|atmos>`, `-t <tape>` and `-d <disk>` are the documented options.
//! Telestrat has no variant, so it cannot be selected. This module validates and
//! binds inputs with main's Oric observers and offers no `prepare`, `spawn` or
//! argv.

use std::ffi::OsStr;
use std::path::PathBuf;

use super::native_support::{
    ExecutableDiscovery, NativeAdapterClassification, NativePreview, PreviewError, PreviewRequest,
    plan_preview, read_prefix,
};
use super::planning::CanonicalIdentityStatus;
use super::safe_launch_sandbox::{MediaKind, SourceProvenance};
use crate::identity_source::model::LocalEvidenceStrength;
use crate::oric_media::{MAX_ORIC_MEDIA_BYTES, OricMediaObservation, observe_oric_media};

pub const ADAPTER_ID: &str = "oricutron";
pub const PLATFORM_ID: &str = "Oric";
pub const CLASSIFICATION: NativeAdapterClassification =
    NativeAdapterClassification::PreviewReadinessOnly;
pub const UNPROVEN: &[&str] = &[
    "Oricutron reads oricutron.cfg from the executable's own directory and resolves ROM paths against it; HOME/XDG and the working directory do not redirect either",
    "the machine and disk-controller ROMs are chosen by that user config, so an EmuWiz ROM profile cannot be applied without replacing it",
    "isolation would need a complete scratch copy of the installation (executable, roms/, config), which is not implemented or proven",
];

/// No implicit machine; Telestrat has no variant so it cannot be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OricMachine {
    Oric1,
    Atmos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OricMediaFormat {
    Tap,
    Dsk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OricutronProfile {
    pub executable: PathBuf,
    /// `None` until a person chooses.
    pub machine: Option<OricMachine>,
    pub disposable_session_acknowledged: bool,
}

/// `observation` must be the successful result of
/// [`observe_oric_media`] for the SAME bytes; it is re-derived and compared
/// here, and `sha256` is bound to the planned source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OricutronMedia {
    pub path: PathBuf,
    pub observation: OricMediaObservation,
    pub sha256: [u8; 32],
    pub evidence: LocalEvidenceStrength,
    /// DAT/hash-led release identity, provenance only.
    pub release_identity: CanonicalIdentityStatus,
}

impl OricutronMedia {
    pub fn format(&self) -> OricMediaFormat {
        match &self.observation {
            OricMediaObservation::Tape(_) => OricMediaFormat::Tap,
            OricMediaObservation::Disk(_) => OricMediaFormat::Dsk,
        }
    }
}

pub fn discover_executables(explicit: &[PathBuf], path_env: Option<&OsStr>) -> ExecutableDiscovery {
    super::native_support::discover_executables(explicit, path_env, &[ADAPTER_ID])
}

fn validate(media: &OricutronMedia, source: &SourceProvenance) -> Result<(), PreviewError> {
    let size = source.original_identity.size as usize;
    if size > MAX_ORIC_MEDIA_BYTES {
        return Err(PreviewError::UnsupportedMedia(
            "Oric media exceeds the bounded analysis size",
        ));
    }
    let bytes = read_prefix(source, size)?;
    match observe_oric_media(&bytes) {
        Ok(observed) if observed == media.observation => Ok(()),
        _ => Err(PreviewError::UnsupportedMedia(
            "media is not the structurally verified Oric TAP or MFM_DISK that was observed",
        )),
    }
}

pub fn preview_oricutron(
    profile: &OricutronProfile,
    media: &OricutronMedia,
) -> Result<NativePreview, PreviewError> {
    if profile.machine.is_none() {
        return Err(PreviewError::MachineRequired);
    }
    let (kind, suffix) = match media.format() {
        OricMediaFormat::Tap => (MediaKind::Tape, "tap"),
        OricMediaFormat::Dsk => (MediaKind::Floppy, "dsk"),
    };
    plan_preview(
        PreviewRequest {
            adapter: ADAPTER_ID,
            platform_id: PLATFORM_ID,
            executable: &profile.executable,
            media: &media.path,
            kind,
            suffix,
            // Structural evidence is the bar for Oric (release identity is
            // DAT/hash-led and provenance only), so no platform id is required.
            identity: None,
            platform_evidence: media.evidence,
            verified_source_sha256: media.sha256,
            disposable_session_acknowledged: profile.disposable_session_acknowledged,
            unproven: UNPROVEN,
        },
        |source| validate(media, source),
    )
}

#[cfg(test)]
mod tests;
