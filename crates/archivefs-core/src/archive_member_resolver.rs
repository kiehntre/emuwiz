//! Safe, read-only selection of a runnable member from an inspected archive.
//!
//! This module deliberately consumes the existing [`InspectorReport`] rather
//! than opening or parsing an archive again.  It answers only “which stored
//! members are plausible content for this already-known platform?”; it does
//! not identify the game and it never chooses between multiple candidates.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::inspector::{InspectorEntryClassification, InspectorEntryKind, InspectorReport};
use crate::platform::platform_by_id;

/// Maximum number of candidates retained for one preparation decision.
pub const MAX_PREPARE_CANDIDATES: usize = 256;
/// Maximum stored member path length accepted by the preparation flow.
pub const MAX_PREPARE_MEMBER_PATH_BYTES: usize = 4096;
/// Maximum number of path components accepted in a stored member path.
pub const MAX_PREPARE_MEMBER_COMPONENTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedMemberCandidate {
    pub member_name: String,
    pub size_bytes: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveMemberResolution {
    One(PreparedMemberCandidate),
    Multiple(Vec<PreparedMemberCandidate>),
    None(String),
}

/// Resolves only regular, likely-content members compatible with `platform`.
/// An incomplete inspection is refused, as is an unknown platform: both
/// cases would require guessing.
pub fn resolve_prepared_members(
    report: &InspectorReport,
    platform: Option<&str>,
) -> ArchiveMemberResolution {
    if report.truncated {
        return ArchiveMemberResolution::None(
            "The archive listing was too large to inspect completely.".to_string(),
        );
    }
    let Some(platform_id) = platform else {
        return ArchiveMemberResolution::None(
            "This game has no confirmed platform, so EmuWiz cannot choose a safe member yet."
                .to_string(),
        );
    };
    let Some(platform) = platform_by_id(platform_id) else {
        return ArchiveMemberResolution::None(
            "This game's platform is not known well enough to choose a safe member.".to_string(),
        );
    };

    let mut candidates = Vec::new();
    for entry in &report.entries {
        if entry.kind != InspectorEntryKind::File
            || entry.classification != InspectorEntryClassification::LikelyContent
            || !safe_member_name(&entry.name)
        {
            continue;
        }
        let Some(extension) = Path::new(&entry.name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        if !platform.accepts_extension(&extension) {
            continue;
        }
        if candidates.len() == MAX_PREPARE_CANDIDATES {
            return ArchiveMemberResolution::None(
                "The archive contains too many possible game files to choose safely.".to_string(),
            );
        }
        candidates.push(PreparedMemberCandidate {
            member_name: entry.name.clone(),
            size_bytes: entry.uncompressed_size,
            reason: format!("{} media accepted for {}", extension, platform.display_name),
        });
    }

    match candidates.len() {
        0 => ArchiveMemberResolution::None(
            "The archive was inspected, but it contains no playable file matching this game's platform. Check its contents or choose a different archive."
                .to_string(),
        ),
        1 => ArchiveMemberResolution::One(candidates.remove(0)),
        _ => ArchiveMemberResolution::Multiple(candidates),
    }
}

/// Joins a selected archive member to a mounted root without following a
/// symlink.  Every component is checked with `symlink_metadata`; no recursive
/// scan or path canonicalisation is involved.
pub fn prepared_member_path(mount_root: &Path, member_name: &str) -> Result<PathBuf, String> {
    if !safe_member_name(member_name) {
        return Err("The selected archive member has an unsafe path.".to_string());
    }
    let root_metadata = fs::symlink_metadata(mount_root)
        .map_err(|error| format!("prepared archive mount is unavailable: {error}"))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("prepared archive mount is not a real directory.".to_string());
    }

    let components = Path::new(member_name).components().collect::<Vec<_>>();
    let mut current = mount_root.to_path_buf();
    for (index, component) in components.into_iter().enumerate() {
        let Component::Normal(component) = component else {
            return Err("The selected archive member has an unsafe path.".to_string());
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("selected archive member is unavailable: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err("The selected archive member uses a symlink and was refused.".to_string());
        }
        if index + 1 < Path::new(member_name).components().count() && !metadata.is_dir() {
            return Err("The selected archive member path is not a directory.".to_string());
        }
    }
    let metadata = fs::symlink_metadata(&current)
        .map_err(|error| format!("selected archive member is unavailable: {error}"))?;
    if !metadata.is_file() {
        return Err("The selected archive member is not a regular file.".to_string());
    }
    Ok(current)
}

fn safe_member_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_PREPARE_MEMBER_PATH_BYTES
        || name.contains('\\')
        || Path::new(name).is_absolute()
        || Path::new(name).components().count() > MAX_PREPARE_MEMBER_COMPONENTS
    {
        return false;
    }
    Path::new(name)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspector::{InspectorEntry, InspectorEntryKind};

    fn report(names: &[(&str, InspectorEntryClassification)]) -> InspectorReport {
        InspectorReport {
            entries: names
                .iter()
                .map(|(name, classification)| InspectorEntry {
                    name: (*name).to_string(),
                    kind: InspectorEntryKind::File,
                    uncompressed_size: 12,
                    compressed_size: Some(4),
                    compression_method: Some("deflate".to_string()),
                    classification: *classification,
                })
                .collect(),
            truncated: false,
            total_entries_in_archive: names.len(),
        }
    }

    #[test]
    fn one_compatible_member_is_the_only_auto_resolution() {
        let result = resolve_prepared_members(
            &report(&[("game.nes", InspectorEntryClassification::LikelyContent)]),
            Some("NES"),
        );
        assert!(
            matches!(result, ArchiveMemberResolution::One(candidate) if candidate.member_name == "game.nes")
        );
    }

    #[test]
    fn multiple_members_require_a_choice() {
        let result = resolve_prepared_members(
            &report(&[
                ("disc1.iso", InspectorEntryClassification::LikelyContent),
                ("disc2.iso", InspectorEntryClassification::LikelyContent),
            ]),
            Some("PS2"),
        );
        assert!(
            matches!(result, ArchiveMemberResolution::Multiple(candidates) if candidates.len() == 2)
        );
    }

    #[test]
    fn ancillary_and_traversal_members_are_not_candidates() {
        let result = resolve_prepared_members(
            &report(&[
                ("manual.txt", InspectorEntryClassification::Documentation),
                ("../game.nes", InspectorEntryClassification::LikelyContent),
                ("game.nes", InspectorEntryClassification::LikelyContent),
            ]),
            Some("NES"),
        );
        assert!(
            matches!(result, ArchiveMemberResolution::One(candidate) if candidate.member_name == "game.nes")
        );
    }

    #[test]
    fn incomplete_or_unknown_input_fails_closed() {
        let mut incomplete = report(&[("game.nes", InspectorEntryClassification::LikelyContent)]);
        incomplete.truncated = true;
        assert!(matches!(
            resolve_prepared_members(&incomplete, Some("NES")),
            ArchiveMemberResolution::None(_)
        ));
        assert!(matches!(
            resolve_prepared_members(&report(&[]), None),
            ArchiveMemberResolution::None(_)
        ));
    }

    #[test]
    fn mounted_member_path_rejects_traversal_and_symlinks() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let root = temporary.path().join("mounted");
        std::fs::create_dir(&root).expect("mount root");
        std::fs::write(root.join("game.nes"), b"game").expect("game member");
        assert!(prepared_member_path(&root, "../game.nes").is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temporary.path(), root.join("escape")).expect("symlink");
            assert!(prepared_member_path(&root, "escape/outside.nes").is_err());
        }
    }
}
