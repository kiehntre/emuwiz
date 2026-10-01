//! Reviewed whole-component Saturn patches, preserving a complete CUE tree.
//! IPS/BPS/UPS/PPF3 decoding is exclusively the canonical standalone engine.
//! No SSP, CHD, logical-track rewriting, filesystem rebuilding or launching.
//! Preparation returns the same durable tree receipt used by the DCP adapter;
//! publication/recovery/undo are patch_output_recovery::tree operations.
use crate::optical_patch_tree::{Content, Contents, file_content, refuse};
use crate::patch_output_recovery::tree::{self, PreparedTreePatch, TreePatchPlan};
use crate::saturn_disc_manifest::{
    SaturnDescriptorType, SaturnDiscManifest, SaturnManifestStatus, SaturnTrackMode,
    SaturnTrackType, inspect_saturn_disc, verify_saturn_manifest,
};
use crate::saturn_patch_readiness::SaturnPatchTarget;
use crate::standalone_patch::{
    PatchInspectionState, StandalonePatchApplyPlan, StandalonePatchFormat,
    build_standalone_patch_apply_plan, inspect_standalone_patch, prepare_standalone_patch_output,
};
use std::cell::RefCell;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_PATCH_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_COMPONENT_BYTES: u64 = 512 * 1024 * 1024;
/// Exact caller-reviewed relationship, not a readiness label. In particular,
/// an IPS patch has no embedded source identity; the caller must review it.
#[derive(Clone, Debug)]
pub struct SaturnPatchBinding {
    pub manifest: SaturnDiscManifest,
    pub target: SaturnPatchTarget,
    pub patch_sha256: String,
    pub track_number: u32,
    pub disc_ordinal: u32,
    pub patch_disc_ordinal: u32,
}
#[derive(Clone, Debug)]
pub struct SaturnPatchPlan {
    tree: TreePatchPlan,
    source: PathBuf,
    patch: PathBuf,
    patch_content: Content,
    patch_plan: StandalonePatchApplyPlan,
    original: Contents,
    manifest: SaturnDiscManifest,
    target: PathBuf,
    cue: PathBuf,
    allowance: u64,
}
// Admission only, not a second CUE parser: the canonical parser below owns
// paths, tracks and timestamps. Refuse directives/FILE types it drops rather
// than allowing an incomplete projection to authorize a component write.
fn admit_cue(path: &Path) -> io::Result<()> {
    let mut text = String::new();
    fs::File::open(path)?
        .take(crate::ingestion::cue_bin::MAX_CUE_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > crate::ingestion::cue_bin::MAX_CUE_BYTES {
        return Err(refuse("CUE size bound"));
    }
    for raw in text.lines() {
        let line = raw.trim_matches([' ', '\t', '\r']);
        if line.is_empty() {
            continue;
        }
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return Err(refuse("CUE control character"));
        }
        let mut fields = line.split_ascii_whitespace();
        let directive = fields
            .next()
            .ok_or_else(|| refuse("missing CUE directive"))?;
        let rest = line[directive.len()..].trim_matches([' ', '\t']);
        let supported = match directive.to_ascii_uppercase().as_str() {
            "FILE" => rest
                .strip_prefix('"')
                .and_then(|s| s.split_once('"'))
                .is_some_and(|(name, kind)| {
                    !name.is_empty()
                        && !name.contains('\\')
                        && kind.trim().eq_ignore_ascii_case("BINARY")
                }),
            "TRACK" => {
                fields
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .is_some_and(|n| (1..=99).contains(&n))
                    && fields.next().is_some_and(|v| {
                        matches!(
                            v.to_ascii_uppercase().as_str(),
                            "MODE1/2048" | "MODE1/2352" | "MODE2/2352" | "AUDIO"
                        )
                    })
                    && fields.next().is_none()
            }
            "INDEX" => {
                fields
                    .next()
                    .and_then(|v| v.parse::<u8>().ok())
                    .is_some_and(|n| n <= 1)
                    && fields.next().is_some()
                    && fields.next().is_none()
            }
            "PREGAP" | "POSTGAP" => fields.next().is_some() && fields.next().is_none(),
            "REM" => fields.next().is_some_and(|v| {
                v.eq_ignore_ascii_case("COMMENT") || v.eq_ignore_ascii_case("GENRE")
            }),
            "TITLE" => {
                rest.len() >= 2
                    && rest.starts_with('"')
                    && rest.ends_with('"')
                    && !rest[1..rest.len() - 1].contains('"')
            }
            _ => false,
        };
        if !supported {
            return Err(refuse(
                "CUE directive is outside the verified component-patch layout",
            ));
        }
    }
    Ok(())
}
pub fn review_saturn_patch(
    patch: &Path,
    destination: &Path,
    binding: &SaturnPatchBinding,
) -> io::Result<SaturnPatchPlan> {
    if binding.disc_ordinal == 0 || binding.disc_ordinal != binding.patch_disc_ordinal {
        return Err(refuse("wrong reviewed disc ordinal"));
    }
    let manifest = &binding.manifest;
    if manifest.descriptor_type != SaturnDescriptorType::Cue
        || !manifest
            .source_descriptor
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cue"))
        || !matches!(
            manifest.status,
            SaturnManifestStatus::Complete | SaturnManifestStatus::CompleteWithWarnings
        )
        || manifest.system_id.is_none()
    {
        return Err(refuse(
            "complete native Saturn CUE identity required; CHD/lone BIN refused",
        ));
    }
    let device = &manifest
        .system_id
        .as_ref()
        .ok_or_else(|| refuse("missing System ID"))?
        .fact
        .device_info;
    let native_ordinal = device
        .strip_prefix("CD-")
        .and_then(|s| s.split_once('/'))
        .and_then(|(disc, _)| disc.parse::<u32>().ok());
    if native_ordinal != Some(binding.disc_ordinal) {
        return Err(refuse(
            "native Saturn disc ordinal does not match reviewed patch",
        ));
    }
    if patch
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ssp"))
    {
        return Err(refuse("SSP unsupported"));
    }
    let source = manifest
        .source_descriptor
        .parent()
        .ok_or_else(|| refuse("CUE parent missing"))?
        .to_owned();
    let original = Contents::read(&source, MAX_SOURCE_BYTES)?;
    if patch.starts_with(&source) {
        return Err(refuse("patch must be outside source tree"));
    }
    let patch_content = file_content(patch, MAX_PATCH_BYTES)?;
    if patch_content.sha256 != binding.patch_sha256 {
        return Err(refuse("reviewed patch identity mismatch"));
    }
    let allowance = original
        .size()?
        .checked_add(patch_content.size)
        .ok_or_else(|| refuse("input size overflow"))?
        .max(1);
    let tree = TreePatchPlan::review_with_max_total_bytes(
        &[source.clone(), patch.to_owned()],
        destination,
        allowance,
    )?;
    admit_cue(&manifest.source_descriptor)?;
    if manifest
        .tracks
        .iter()
        .enumerate()
        .any(|(index, track)| track.number as usize != index + 1)
    {
        return Err(refuse("CUE tracks must be consecutively ordered from 01"));
    }
    if !verify_saturn_manifest(manifest).issues.is_empty()
        || inspect_saturn_disc(&manifest.source_descriptor).map_err(refuse)? != *manifest
    {
        return Err(refuse("reviewed CUE/component manifest changed"));
    }
    let (component, sha256) = match &binding.target {
        SaturnPatchTarget::ComponentBin { component, sha256 } => (component, sha256),
        _ => return Err(refuse("explicit whole ComponentBin target required")),
    };
    let target = component.strip_prefix(&source).map_err(refuse)?.to_owned();
    let content = original.file(&target)?;
    if content.sha256 != *sha256
        || content.size > MAX_COMPONENT_BYTES
        || !manifest
            .components
            .iter()
            .any(|c| c.path == *component && c.sha256 == *sha256)
        || !manifest.tracks.iter().any(|t| {
            t.number == binding.track_number
                && t.source_file == *component
                && t.track_type == SaturnTrackType::Data
        })
        || manifest.tracks.iter().any(|t| {
            t.source_file == *component
                && (t.track_type == SaturnTrackType::Audio || t.number != binding.track_number)
        })
    {
        return Err(refuse(
            "wrong component identity/track or ambiguous audio/multi-track component",
        ));
    }
    let inspection = inspect_standalone_patch(patch).map_err(refuse)?;
    if inspection.state != PatchInspectionState::Valid
        || !matches!(
            inspection.format,
            StandalonePatchFormat::Ips
                | StandalonePatchFormat::Bps
                | StandalonePatchFormat::Ups
                | StandalonePatchFormat::Ppf
        )
    {
        return Err(refuse(
            "unsupported or malformed Saturn patch; IPS/BPS/UPS/PPF3 only",
        ));
    }
    if inspection.patch_sha256 != binding.patch_sha256 {
        return Err(refuse("patch changed during review"));
    }
    // Canonical source size/checksum and format constraints; no patch execution.
    let patch_plan = build_standalone_patch_apply_plan(
        &inspection,
        component,
        &destination.join(&target),
        destination,
    )
    .map_err(refuse)?;
    original.verify(&source, MAX_SOURCE_BYTES)?;
    let cue = manifest
        .source_descriptor
        .strip_prefix(&source)
        .map_err(refuse)?
        .to_owned();
    let plan = SaturnPatchPlan {
        tree,
        source,
        patch: patch.to_owned(),
        patch_content,
        patch_plan,
        original,
        manifest: manifest.clone(),
        target,
        cue,
        allowance,
    };
    plan.verify_raw_headers(&plan.source)?;
    Ok(plan)
}
impl SaturnPatchPlan {
    pub fn max_total_bytes(&self) -> u64 {
        self.allowance
    }
    fn produce(&self, staging: &Path) -> io::Result<Content> {
        self.original.verify(&self.source, MAX_SOURCE_BYTES)?;
        if file_content(&self.patch, MAX_PATCH_BYTES)? != self.patch_content {
            return Err(refuse("patch changed after review"));
        }
        self.original.copy(&self.source, staging)?;
        // Keep the source SHA and patch identity reviewed before staging. Only
        // relocate the base; never accept the copied bytes as a new baseline.
        let mut plan = self.patch_plan.clone();
        plan.reviewed.base_path = staging.join(&self.target);
        let output = prepare_standalone_patch_output(&plan).map_err(refuse)?;
        if output.bytes.len() as u64 != self.original.file(&self.target)?.size {
            return Err(refuse("patched component length changes optical layout"));
        }
        let content = Content::bytes(&output.bytes);
        fs::write(staging.join(&self.target), output.bytes)?;
        Ok(content)
    }
    fn verify(&self, staging: &Path, patched: &Content) -> io::Result<()> {
        let mut expected = self.original.clone();
        expected
            .0
            .insert(self.target.clone(), Some(patched.clone()));
        expected.verify(staging, self.allowance)?;
        self.verify_raw_headers(staging)?;
        let actual = inspect_saturn_disc(&staging.join(&self.cue)).map_err(refuse)?;
        if actual.descriptor_sha256 != self.manifest.descriptor_sha256
            || actual.system_id != self.manifest.system_id
            || actual.status != self.manifest.status
            || actual.components.len() != self.manifest.components.len()
            || actual.tracks.len() != self.manifest.tracks.len()
        {
            return Err(refuse(
                "Saturn CUE/native identity/layout verification failed",
            ));
        }
        for (before, after) in self.manifest.components.iter().zip(&actual.components) {
            let rel = before.path.strip_prefix(&self.source).map_err(refuse)?;
            if after.path != staging.join(rel)
                || before.size_bytes != after.size_bytes
                || (rel != self.target && before.sha256 != after.sha256)
            {
                return Err(refuse("Saturn component mapping/size/content changed"));
            }
        }
        for (before, after) in self.manifest.tracks.iter().zip(&actual.tracks) {
            let mut expected = before.clone();
            let rel = before
                .source_file
                .strip_prefix(&self.source)
                .map_err(refuse)?;
            expected.source_file = staging.join(rel);
            if rel == self.target {
                expected.data_logical_sha256 = after.data_logical_sha256.clone();
            }
            if expected != *after {
                return Err(refuse(
                    "Saturn track order/mapping/gap/audio/layout changed",
                ));
            }
        }
        Ok(())
    }
    fn verify_raw_headers(&self, staging: &Path) -> io::Result<()> {
        use crate::raw_cd_sector::{
            MODE1_USER_DATA_OFFSET, MODE2_FORM1_USER_DATA_OFFSET, RAW_SECTOR_BYTES,
            RawCdSectorMode, detect_sector_mode,
        };
        let track = self
            .manifest
            .tracks
            .iter()
            .find(|t| t.source_file == self.source.join(&self.target))
            .ok_or_else(|| refuse("missing target track"))?;
        let (header_bytes, mode) = match track.mode {
            SaturnTrackMode::Mode1_2048 => return Ok(()),
            SaturnTrackMode::Mode1_2352 => (MODE1_USER_DATA_OFFSET, RawCdSectorMode::Mode1Raw),
            SaturnTrackMode::Mode2_2352 => {
                (MODE2_FORM1_USER_DATA_OFFSET, RawCdSectorMode::Mode2Raw)
            }
            _ => return Err(refuse("unsupported target sector mode")),
        };
        let mut source = fs::File::open(self.source.join(&self.target))?;
        let mut output = fs::File::open(staging.join(&self.target))?;
        let mut before = [0; RAW_SECTOR_BYTES];
        let mut after = [0; RAW_SECTOR_BYTES];
        for sector in 0..self.original.file(&self.target)?.size / RAW_SECTOR_BYTES as u64 {
            source.read_exact(&mut before)?;
            output.read_exact(&mut after)?;
            if before[..header_bytes] != after[..header_bytes] {
                return Err(refuse("raw sector sync/address/mode/XA subheader changed"));
            }
            if sector >= track.index_01_frame && detect_sector_mode(&before) != Some(mode) {
                return Err(refuse("raw sector mode disagrees with reviewed CUE"));
            }
        }
        Ok(())
    }
    pub fn prepare(&self) -> io::Result<PreparedTreePatch> {
        let patched = RefCell::new(None);
        tree::prepare(
            &self.tree,
            |staging| {
                *patched.borrow_mut() = Some(self.produce(staging)?);
                Ok(())
            },
            |staging| {
                self.verify(
                    staging,
                    patched
                        .borrow()
                        .as_ref()
                        .ok_or_else(|| refuse("missing patch output proof"))?,
                )
            },
        )
    }
}
#[cfg(test)]
#[path = "saturn_patch_apply_tests.rs"]
pub(crate) mod tests;
