//! Safe handoff of a verified [`crate::media_set::MediaSet`] to an emulator.
//!
//! This module deliberately does not implement media swapping.  It either
//! gives an emulator a descriptor format it already accepts, or returns the
//! verified first medium and an honest explanation that the emulator must do
//! the later changes itself.

use crate::media_set::{
    MediaFamily, MediaProfile, MediaSet, MediaSetState, MediaSwapPlan, MediaSwapStep,
    media_swap_plan,
};
use sha2::{Digest, Sha256};
use std::fmt::{self, Write as _};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

const MANAGED_HEADER: &str = "# EmuWiz managed multi-media playlist v1\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaLaunchAdapter {
    DuckStation,
    RetroArch,
    FsUae,
    Hatari,
    Dreamcast,
    Saturn,
    GameCube,
    PcEngineCd,
    SegaCd,
    ScummVm,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaLaunchMechanism {
    M3uPlaylist,
    VerifiedStartMedia,
    SingleMedia,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaLaunchHandoff {
    pub adapter: MediaLaunchAdapter,
    pub family: MediaFamily,
    pub mechanism: MediaLaunchMechanism,
    pub launch_path: PathBuf,
    pub ordered_media: Vec<PathBuf>,
    pub descriptor: Option<PathBuf>,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaLaunchHandoffError {
    pub detail: String,
}

/// Short GUI-safe copy for an already prepared handoff.  It intentionally
/// avoids implementation terms such as topology and descriptor.
pub fn media_launch_summary(handoff: &MediaLaunchHandoff) -> Option<String> {
    if handoff.ordered_media.len() < 2 {
        return None;
    }
    let noun = match handoff.family {
        MediaFamily::Optical => "disc",
        MediaFamily::Floppy => "disk",
        MediaFamily::Tape => "tape",
    };
    let ending = match handoff.mechanism {
        MediaLaunchMechanism::M3uPlaylist => "complete set will be launched",
        MediaLaunchMechanism::Unsupported => "multi-disc launch needs adapter support",
        MediaLaunchMechanism::VerifiedStartMedia | MediaLaunchMechanism::SingleMedia => {
            "complete set available"
        }
    };
    Some(format!(
        "{}-{noun} game · {ending}",
        handoff.ordered_media.len()
    ))
}

impl fmt::Display for MediaLaunchHandoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for MediaLaunchHandoffError {}

fn refusal(detail: impl Into<String>) -> MediaLaunchHandoffError {
    MediaLaunchHandoffError {
        detail: detail.into(),
    }
}

fn plain_media(step: &MediaSwapStep) -> Result<PathBuf, MediaLaunchHandoffError> {
    step.preferred_representation
        .as_ref()
        .map(|source| source.path.clone())
        .ok_or_else(|| refusal("A verified game medium has no available representation"))
}

fn source_paths(plan: &MediaSwapPlan) -> Result<Vec<PathBuf>, MediaLaunchHandoffError> {
    if plan.ordered_media.is_empty() {
        return Err(refusal("The verified media set has no launchable media"));
    }
    let mut paths = Vec::with_capacity(plan.ordered_media.len());
    for step in &plan.ordered_media {
        if step.ordinal.is_none() {
            return Err(refusal("The disk order could not be verified"));
        }
        paths.push(plain_media(step)?);
    }
    let ordinals = plan
        .ordered_media
        .iter()
        .filter_map(|step| step.ordinal.as_ref().map(|ordinal| ordinal.number))
        .collect::<std::collections::BTreeSet<_>>();
    let known_ordinals = plan
        .ordered_media
        .iter()
        .filter(|step| step.ordinal.is_some())
        .count();
    if paths.windows(2).any(|pair| pair[0] == pair[1])
        || (known_ordinals > 1 && ordinals.len() != known_ordinals)
    {
        return Err(refusal("The media set contains a duplicate member"));
    }
    Ok(paths)
}

fn m3u_capable(adapter: &MediaLaunchAdapter, family: MediaFamily) -> bool {
    family == MediaFamily::Optical
        && matches!(
            adapter,
            MediaLaunchAdapter::DuckStation | MediaLaunchAdapter::RetroArch
        )
}

fn adapter_name(adapter: &MediaLaunchAdapter) -> &str {
    match adapter {
        MediaLaunchAdapter::DuckStation => "DuckStation",
        MediaLaunchAdapter::RetroArch => "RetroArch",
        MediaLaunchAdapter::FsUae => "FS-UAE",
        MediaLaunchAdapter::Hatari => "Hatari",
        MediaLaunchAdapter::Dreamcast => "Dreamcast emulator",
        MediaLaunchAdapter::Saturn => "Saturn emulator",
        MediaLaunchAdapter::GameCube => "GameCube emulator",
        MediaLaunchAdapter::PcEngineCd => "PC Engine CD emulator",
        MediaLaunchAdapter::SegaCd => "Sega CD emulator",
        MediaLaunchAdapter::ScummVm => "ScummVM",
        MediaLaunchAdapter::Other(name) => name,
    }
}

fn explicitly_unsupported(adapter: &MediaLaunchAdapter) -> bool {
    matches!(
        adapter,
        MediaLaunchAdapter::Dreamcast
            | MediaLaunchAdapter::Saturn
            | MediaLaunchAdapter::GameCube
            | MediaLaunchAdapter::PcEngineCd
            | MediaLaunchAdapter::SegaCd
            | MediaLaunchAdapter::ScummVm
    )
}

/// Builds the handoff from the already-resolved topology and writes only an
/// EmuWiz-owned `.m3u` when the selected adapter has a proven whole-set form.
/// The supplied profile must be the same profile used to authorize the normal
/// launch; this function does not discover emulator capabilities.
pub fn prepare_multimedia_launch(
    set: &MediaSet,
    profile: &MediaProfile,
    adapter: MediaLaunchAdapter,
    derived_root: &Path,
) -> Result<MediaLaunchHandoff, MediaLaunchHandoffError> {
    if set.state != MediaSetState::CompleteSet {
        return Err(refusal(match set.state {
            MediaSetState::IncompleteSet => match set.family {
                Some(MediaFamily::Floppy) => "One of the game disks is missing",
                Some(MediaFamily::Tape) => "One of the game tapes is missing",
                _ => "One of the game discs is missing",
            },
            MediaSetState::AmbiguousSet => "This media set has conflicting matches",
            MediaSetState::ConflictingSet => "The disk order could not be verified",
            MediaSetState::UnverifiedSet => "The saved media-set information is out of date",
            MediaSetState::UnsupportedSet => "This media set is not supported for safe launch",
            MediaSetState::CompleteSet => unreachable!(),
        }));
    }
    let plan = media_swap_plan(set, Some(profile));
    if !plan.blockers.is_empty() {
        return Err(refusal(
            plan.blockers
                .first()
                .map(|blocker| blocker.detail.clone())
                .unwrap_or_else(|| "The media set could not be verified".into()),
        ));
    }
    let paths = source_paths(&plan)?;
    let family = set
        .family
        .ok_or_else(|| refusal("The media family could not be verified"))?;
    let start = paths
        .first()
        .cloned()
        .ok_or_else(|| refusal("The verified start medium is missing"))?;

    for path in &paths {
        validate_source(path)?;
    }

    if paths.len() == 1 {
        return Ok(MediaLaunchHandoff {
            adapter,
            family,
            mechanism: MediaLaunchMechanism::SingleMedia,
            launch_path: start,
            ordered_media: paths,
            descriptor: None,
            explanation: "Single-media launch is unchanged".into(),
        });
    }

    if explicitly_unsupported(&adapter) {
        return Ok(MediaLaunchHandoff {
            explanation: format!(
                "{} has no reviewed multi-media launch adapter yet; no automatic disc composition was attempted",
                adapter_name(&adapter)
            ),
            adapter,
            family,
            mechanism: MediaLaunchMechanism::Unsupported,
            launch_path: start,
            ordered_media: paths,
            descriptor: None,
        });
    }

    if !m3u_capable(&adapter, family) {
        return Ok(MediaLaunchHandoff {
            explanation: format!(
                "{name} can only start with the first medium; later changes are handled inside the emulator",
                name = adapter_name(&adapter)
            ),
            adapter,
            family,
            mechanism: MediaLaunchMechanism::VerifiedStartMedia,
            launch_path: start,
            ordered_media: paths,
            descriptor: None,
        });
    }

    let descriptor = write_managed_m3u(derived_root, &set.identity.key.value, &plan, &paths)?;
    Ok(MediaLaunchHandoff {
        adapter,
        family,
        mechanism: MediaLaunchMechanism::M3uPlaylist,
        launch_path: descriptor.clone(),
        ordered_media: paths,
        descriptor: Some(descriptor),
        explanation: "The complete verified set will be launched".into(),
    })
}

fn validate_source(path: &Path) -> Result<(), MediaLaunchHandoffError> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(refusal("A media path is unsafe or contains path traversal"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .map_err(|_| refusal("One of the game media files is missing"))?;
        if metadata.file_type().is_symlink() {
            return Err(refusal("A game media path uses a symlink and was refused"));
        }
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| refusal("One of the game media files is missing"))?;
    if !metadata.is_file() {
        return Err(refusal("A game media path is not a regular file"));
    }
    Ok(())
}

fn derived_descriptor_path(root: &Path, identity: &str, paths: &[PathBuf]) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(identity.as_bytes());
    for path in paths {
        hash.update(path.as_os_str().to_string_lossy().as_bytes());
        if let Ok(metadata) = fs::metadata(path) {
            hash.update(metadata.len().to_le_bytes());
            if let Ok(modified) = metadata.modified()
                && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
            {
                hash.update(duration.as_nanos().to_le_bytes());
            }
        }
    }
    let digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    root.join(format!("{digest}.m3u"))
}

fn write_managed_m3u(
    root: &Path,
    identity: &str,
    plan: &MediaSwapPlan,
    paths: &[PathBuf],
) -> Result<PathBuf, MediaLaunchHandoffError> {
    validate_cache_root(root)?;
    fs::create_dir_all(root)
        .map_err(|error| refusal(format!("Cannot create launch cache: {error}")))?;
    let descriptor = derived_descriptor_path(root, identity, paths);
    let mut body = String::from(MANAGED_HEADER);
    for (step, path) in plan.ordered_media.iter().zip(paths) {
        let ordinal = step.ordinal.as_ref().map(|o| o.number).unwrap_or_default();
        let side = step.side.as_ref().map(|side| side.number).unwrap_or(0);
        write!(&mut body, "# EmuWiz: Disc {ordinal}").expect("writing to a String cannot fail");
        if side != 0 {
            write!(&mut body, " Side {side}").expect("writing to a String cannot fail");
        }
        body.push('\n');
        body.push_str(&path.to_string_lossy());
        body.push('\n');
    }

    if let Ok(existing) = fs::read_to_string(&descriptor)
        && !existing.starts_with(MANAGED_HEADER)
    {
        return Err(refusal(
            "A user file already occupies the managed playlist path",
        ));
    }
    let temp = descriptor.with_extension("m3u.tmp");
    if fs::symlink_metadata(&temp).is_ok() {
        return Err(refusal(
            "A temporary managed playlist path is already occupied",
        ));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| refusal(format!("Cannot prepare managed playlist: {error}")))?;
    file.write_all(body.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| refusal(format!("Cannot write managed playlist: {error}")))?;
    drop(file);
    fs::rename(&temp, &descriptor)
        .map_err(|error| refusal(format!("Cannot publish managed playlist: {error}")))?;
    Ok(descriptor)
}

fn validate_cache_root(root: &Path) -> Result<(), MediaLaunchHandoffError> {
    if !root.is_absolute()
        || root.as_os_str().is_empty()
        || root.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err(refusal("The managed launch-cache path is unsafe"));
    }
    let mut current = PathBuf::new();
    for component in root.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(refusal("The managed launch-cache path uses a symlink"));
        }
    }
    Ok(())
}

/// Standard media profile used by the native DuckStation adapter's M3U form.
pub fn duckstation_multimedia_profile() -> MediaProfile {
    MediaProfile {
        id: "duckstation".into(),
        platform: "PSX".into(),
        supported_formats: ["iso", "cue", "chd"]
            .into_iter()
            .map(String::from)
            .collect(),
        preferred_formats: vec!["chd".into(), "cue".into(), "iso".into()],
        readiness_hint: None,
    }
}

/// RetroArch's documented content playlist form is also the M3U form.
pub fn retroarch_multimedia_profile(core_id: impl Into<String>) -> MediaProfile {
    MediaProfile {
        id: core_id.into(),
        platform: "PSX".into(),
        supported_formats: ["iso", "cue", "chd"]
            .into_iter()
            .map(String::from)
            .collect(),
        preferred_formats: vec!["cue".into(), "chd".into(), "iso".into()],
        readiness_hint: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_set::{
        ConflictKind, EvidenceKind, ExpectedCount, IdentityKey, MediaAvailability, MediaEvidence,
        MediaFamily, MediaOrdinal, MediaRecord, MediaRole, MediaSetConflict, MediaSource,
        OrdinalUnit, SideLayout, index_media, resolve_index,
    };
    use std::fs::File;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fixture(root: &Path, family: MediaFamily, count: u16) -> MediaSet {
        let mut records = Vec::new();
        for number in 1..=count {
            let extension = match family {
                MediaFamily::Optical => "cue",
                MediaFamily::Floppy => "adf",
                MediaFamily::Tape => "tap",
            };
            let path = root.join(format!("Game {number}.{extension}"));
            File::create(&path).unwrap();
            let mut evidence = MediaEvidence::new(EvidenceKind::TrustedDat, "verified manifest");
            evidence.release = Some(IdentityKey::new("release", "game"));
            evidence.medium = Some(IdentityKey::new("medium", number.to_string()));
            evidence.ordinal = Some(MediaOrdinal {
                number,
                unit: OrdinalUnit::Medium,
            });
            evidence.expected_count = Some(ExpectedCount {
                count,
                unit: OrdinalUnit::Medium,
            });
            evidence.role = Some(MediaRole::GameMedia);
            evidence.side_layout = Some(SideLayout::WholeMedium);
            records.push(MediaRecord {
                source: MediaSource {
                    path,
                    archive_member: None,
                },
                platform: Some(
                    match family {
                        MediaFamily::Optical => "PSX",
                        MediaFamily::Floppy => "Amiga",
                        MediaFamily::Tape => "Amiga",
                    }
                    .into(),
                ),
                family: Some(family),
                format: extension.into(),
                availability: MediaAvailability::Observed,
                evidence: vec![evidence],
                warnings: Vec::new(),
            });
        }
        resolve_index(index_media(records))
            .sets
            .into_iter()
            .next()
            .unwrap()
    }

    fn temp_root(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "emuwiz-multimedia-{label}-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn verified_optical_set_generates_deterministic_duckstation_playlist() {
        let root = temp_root("duck");
        let set = fixture(&root, MediaFamily::Optical, 3);
        let cache = root.join("cache");
        let profile = duckstation_multimedia_profile();
        let first =
            prepare_multimedia_launch(&set, &profile, MediaLaunchAdapter::DuckStation, &cache)
                .unwrap();
        let second =
            prepare_multimedia_launch(&set, &profile, MediaLaunchAdapter::DuckStation, &cache)
                .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.mechanism, MediaLaunchMechanism::M3uPlaylist);
        let text = fs::read_to_string(first.descriptor.unwrap()).unwrap();
        assert!(text.contains("Disc 1"));
        assert!(text.contains("Game 3.cue"));
    }

    #[test]
    fn retroarch_uses_the_same_m3u_handoff() {
        let root = temp_root("retroarch");
        let set = fixture(&root, MediaFamily::Optical, 3);
        let handoff = prepare_multimedia_launch(
            &set,
            &retroarch_multimedia_profile("swanstation"),
            MediaLaunchAdapter::RetroArch,
            &root.join("cache"),
        )
        .unwrap();
        assert_eq!(handoff.mechanism, MediaLaunchMechanism::M3uPlaylist);
    }

    #[test]
    fn floppy_and_tape_fall_back_to_verified_start_media() {
        for family in [MediaFamily::Floppy, MediaFamily::Tape] {
            let root = temp_root("fallback");
            let set = fixture(&root, family, 2);
            let profile = MediaProfile {
                id: "fallback".into(),
                platform: "Amiga".into(),
                supported_formats: ["adf", "tap"].into_iter().map(String::from).collect(),
                preferred_formats: vec!["adf".into(), "tap".into()],
                readiness_hint: None,
            };
            let handoff = prepare_multimedia_launch(
                &set,
                &profile,
                MediaLaunchAdapter::FsUae,
                &root.join("cache"),
            )
            .unwrap();
            assert_eq!(handoff.mechanism, MediaLaunchMechanism::VerifiedStartMedia);
            assert!(handoff.descriptor.is_none());
        }
    }

    #[test]
    fn reviewed_but_unimplemented_platform_is_explicitly_unsupported() {
        let root = temp_root("unsupported");
        let set = fixture(&root, MediaFamily::Optical, 2);
        let handoff = prepare_multimedia_launch(
            &set,
            &duckstation_multimedia_profile(),
            MediaLaunchAdapter::Dreamcast,
            &root.join("cache"),
        )
        .unwrap();
        assert_eq!(handoff.mechanism, MediaLaunchMechanism::Unsupported);
        assert!(handoff.descriptor.is_none());
        assert!(handoff.explanation.contains("no reviewed"));
    }

    #[test]
    fn duplicate_disc_ordinals_are_refused_even_with_distinct_paths() {
        let root = temp_root("duplicate-ordinal");
        let mut set = fixture(&root, MediaFamily::Optical, 2);
        set.members[1].ordinal = set.members[0].ordinal.clone();
        set.members[1].representations[0].ordinal = set.members[0].ordinal.clone();
        assert!(
            prepare_multimedia_launch(
                &set,
                &duckstation_multimedia_profile(),
                MediaLaunchAdapter::DuckStation,
                &root.join("cache"),
            )
            .is_err()
        );
    }

    #[test]
    fn four_disc_set_preserves_explicit_order_in_managed_playlist() {
        let root = temp_root("four-disc");
        let set = fixture(&root, MediaFamily::Optical, 4);
        let handoff = prepare_multimedia_launch(
            &set,
            &duckstation_multimedia_profile(),
            MediaLaunchAdapter::DuckStation,
            &root.join("cache"),
        )
        .unwrap();
        let text = fs::read_to_string(handoff.descriptor.unwrap()).unwrap();
        assert_eq!(handoff.ordered_media.len(), 4);
        assert!(text.contains("Disc 4"));
        assert!(text.contains("Game 4.cue"));
    }

    #[test]
    fn conflicting_region_evidence_is_refused_before_launch() {
        let root = temp_root("region-conflict");
        let mut set = fixture(&root, MediaFamily::Optical, 2);
        set.state = MediaSetState::ConflictingSet;
        set.conflicts.push(MediaSetConflict {
            kind: ConflictKind::VariantConflict,
            detail: "region mismatch between discs".into(),
            blocking: true,
        });
        assert!(
            prepare_multimedia_launch(
                &set,
                &duckstation_multimedia_profile(),
                MediaLaunchAdapter::DuckStation,
                &root.join("cache"),
            )
            .is_err()
        );
    }

    #[test]
    fn filename_only_or_uncertain_grouping_is_not_launchable() {
        let root = temp_root("uncertain-grouping");
        let mut set = fixture(&root, MediaFamily::Optical, 2);
        set.state = MediaSetState::UnverifiedSet;
        set.conflicts.push(MediaSetConflict {
            kind: ConflictKind::UnprovenGrouping,
            detail: "relationship inferred from filenames only".into(),
            blocking: true,
        });
        assert!(
            prepare_multimedia_launch(
                &set,
                &duckstation_multimedia_profile(),
                MediaLaunchAdapter::DuckStation,
                &root.join("cache"),
            )
            .is_err()
        );
    }

    #[test]
    fn incomplete_set_is_refused_and_single_media_is_unchanged() {
        let root = temp_root("basic");
        let mut set = fixture(&root, MediaFamily::Optical, 1);
        let profile = duckstation_multimedia_profile();
        let single = prepare_multimedia_launch(
            &set,
            &profile,
            MediaLaunchAdapter::DuckStation,
            &root.join("cache"),
        )
        .unwrap();
        assert_eq!(single.mechanism, MediaLaunchMechanism::SingleMedia);
        set.state = MediaSetState::IncompleteSet;
        assert!(
            prepare_multimedia_launch(
                &set,
                &profile,
                MediaLaunchAdapter::DuckStation,
                &root.join("cache")
            )
            .is_err()
        );
    }

    #[test]
    fn symlink_media_is_refused_without_touching_originals() {
        let root = temp_root("symlink");
        let target = root.join("real.cue");
        File::create(&target).unwrap();
        let link = root.join("link.cue");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(not(unix))]
        return;
        let mut set = fixture(&root, MediaFamily::Optical, 1);
        set.members[0].representations[0].record.source.path = link;
        assert!(
            prepare_multimedia_launch(
                &set,
                &duckstation_multimedia_profile(),
                MediaLaunchAdapter::DuckStation,
                &root.join("cache")
            )
            .is_err()
        );
        assert!(target.exists());
    }
}
