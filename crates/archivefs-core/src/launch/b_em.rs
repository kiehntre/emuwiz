//! b-em (BBC Micro) readiness preview.
//!
//! Classification: PREVIEW / READINESS ONLY. b-em is not installed here, and
//! the evidence that exists contradicts itself, so no launch is offered:
//!
//! * `docs/research/BBC_MICRO_NATIVE_ADAPTER_AUDIT.md` (from the upstream
//!   README) lists `-c <config>` and `-u <uef>`. The upstream `src/main.c` that
//!   was read for this work handles `-cfg`, `-disc`, `-disc1`, `-tape`, `-m<n>`
//!   and positional `.uef/.csw/.snp/<disc>` arguments, and has NO `-c` or `-u`.
//! * `main_close()` calls `config_save()` and `cmos_save()` on exit, to paths
//!   chosen by `find_cfg_dest("b-em", ".cfg")`. Nothing read shows those can be
//!   redirected into a sandbox by HOME/XDG or by a flag that is stable across
//!   versions.
//! * No read-only media switch exists, so scratch media would be mandatory, but
//!   an unproven config/CMOS destination means the user's real b-em state could
//!   be rewritten by a launch.
//!
//! The earlier prototype also invented a seed syntax (`machine=model-b`) that no
//! b-em release reads. This module validates inputs with main's own parsers and
//! binds them, but has no `prepare`, no `spawn` and no argv.

use std::ffi::OsStr;
use std::path::PathBuf;

use super::native_support::{
    ExecutableDiscovery, NativeAdapterClassification, NativePreview, PreviewError, PreviewRequest,
    plan_preview, read_prefix,
};
use super::planning::CanonicalIdentityStatus;
use super::safe_launch_sandbox::{MediaKind, SourceProvenance};
use crate::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};
use crate::identity_source::model::LocalEvidenceStrength;
use crate::safe_read::TrustedRoots;

pub const ADAPTER_ID: &str = "b-em";
pub const PLATFORM_ID: &str = "BBC Micro";
pub const CLASSIFICATION: NativeAdapterClassification =
    NativeAdapterClassification::PreviewReadinessOnly;
pub const UNPROVEN: &[&str] = &[
    "b-em's documented and source-level command lines disagree (-c/-u versus -cfg/-disc/-tape); no installed binary was available to settle it",
    "b-em saves its config and CMOS on exit to a path it chooses; redirecting that into a sandbox is unproven",
    "no read-only media switch exists, so launch would need a proven scratch config/CMOS destination",
];

/// Explicit machine choice only; never inferred from media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BEmMachine {
    ModelB,
    Master,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BEmMediaFormat {
    Ssd,
    Dsd,
    Uef,
    Csw,
    Adf,
    Snapshot,
    Unknown,
}

impl BEmMediaFormat {
    fn plan_details(self) -> Result<(MediaKind, &'static str), PreviewError> {
        match self {
            Self::Ssd => Ok((MediaKind::Floppy, "ssd")),
            Self::Dsd => Ok((MediaKind::Floppy, "dsd")),
            Self::Uef => Ok((MediaKind::Tape, "uef")),
            _ => Err(PreviewError::UnsupportedMedia(
                "b-em preview accepts only structurally verified SSD, DSD and UEF media; CSW, ADF and snapshots are refused",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BEmProfile {
    pub executable: PathBuf,
    /// `None` until a person chooses; this module never defaults one.
    pub machine: Option<BEmMachine>,
    pub disposable_session_acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BEmMedia {
    pub path: PathBuf,
    pub format: BEmMediaFormat,
    pub identity: CanonicalIdentityStatus,
    pub platform_evidence: LocalEvidenceStrength,
    pub verified_source_sha256: [u8; 32],
}

pub fn discover_executables(explicit: &[PathBuf], path_env: Option<&OsStr>) -> ExecutableDiscovery {
    super::native_support::discover_executables(
        explicit,
        path_env,
        &[ADAPTER_ID, "b-em-sdl", "bem"],
    )
}

fn validate(media: &BEmMedia, source: &SourceProvenance) -> Result<(), PreviewError> {
    let size = source.original_identity.size;
    let ok = match media.format {
        BEmMediaFormat::Uef => size > 12 && read_prefix(source, 10)?.starts_with(b"UEF File!\0"),
        BEmMediaFormat::Ssd | BEmMediaFormat::Dsd => {
            // main's structural DFS parser, not a size heuristic.
            let evidence = inspect_disk_format(
                &media.path,
                &TrustedRoots::none(),
                DiskFormatContext {
                    folder_platform: None,
                },
                None,
            );
            evidence.format == Some(DiskFormat::AcornDfsDisk)
        }
        _ => false,
    };
    if !ok {
        return Err(PreviewError::UnsupportedMedia(
            "media is not a structurally valid BBC DFS disc image or UEF tape",
        ));
    }
    Ok(())
}

/// Validates and binds the inputs. Never launchable; see the module docs.
pub fn preview_b_em(profile: &BEmProfile, media: &BEmMedia) -> Result<NativePreview, PreviewError> {
    let (kind, suffix) = media.format.plan_details()?;
    if profile.machine.is_none() {
        return Err(PreviewError::MachineRequired);
    }
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
mod tests;
