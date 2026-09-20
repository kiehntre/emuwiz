//! Role-aware arcade ingestion.
//!
//! MAME/FBNeo collections commonly contain extracted set directories whose
//! members deliberately have chip labels rather than game-media extensions.
//! This module treats a directory as one logical set only when the configured
//! source is an arcade source and the directory has bounded, regular-file
//! evidence of an extracted set.  It never turns a filename into a verified
//! game identity and never writes to the source.

use std::path::{Path, PathBuf};

use crate::database::SourceRole;

const MAX_SET_MEMBERS: usize = 16_384;
const MAX_SET_DIRECTORIES: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
pub enum SupportMaterialKind {
    BiosFirmware,
    SamplePack,
    EmulatorSupport,
}

impl SupportMaterialKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::BiosFirmware => "BIOS/firmware",
            Self::SamplePack => "sample pack",
            Self::EmulatorSupport => "emulator support asset",
        }
    }
}

/// Counts produced by the bounded arcade/support pass.  These counters are
/// intentionally separate from generic unsupported-extension counts: a BIOS
/// or sample is known non-game content, not a user action that needs repair.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArcadeIngestionDiagnostics {
    pub raw_files_considered: usize,
    pub playable_games_created: usize,
    pub support_assets_excluded: usize,
    pub bios_firmware_excluded: usize,
    pub sample_packs_excluded: usize,
    pub emulator_support_excluded: usize,
    pub logical_sets_aggregated: usize,
    pub logical_set_members: usize,
    pub unresolved_items: usize,
}

impl ArcadeIngestionDiagnostics {
    pub fn merge(&mut self, other: &Self) {
        self.raw_files_considered += other.raw_files_considered;
        self.playable_games_created += other.playable_games_created;
        self.support_assets_excluded += other.support_assets_excluded;
        self.bios_firmware_excluded += other.bios_firmware_excluded;
        self.sample_packs_excluded += other.sample_packs_excluded;
        self.emulator_support_excluded += other.emulator_support_excluded;
        self.logical_sets_aggregated += other.logical_sets_aggregated;
        self.logical_set_members += other.logical_set_members;
        self.unresolved_items += other.unresolved_items;
    }

    pub fn note_support(&mut self, kind: SupportMaterialKind) {
        self.support_assets_excluded += 1;
        match kind {
            SupportMaterialKind::BiosFirmware => self.bios_firmware_excluded += 1,
            SupportMaterialKind::SamplePack => self.sample_packs_excluded += 1,
            SupportMaterialKind::EmulatorSupport => self.emulator_support_excluded += 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArcadeSetDirectory {
    pub path: PathBuf,
    pub set_name: String,
    pub members: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArcadeSetDiscovery {
    pub sets: Vec<ArcadeSetDirectory>,
    pub diagnostics: ArcadeIngestionDiagnostics,
}

/// Classifies known support paths only when the source context makes them
/// arcade/emulator material.  A folder called `samples` in an ordinary
/// computer-games source is not hidden by this function.
pub fn support_material_kind(
    path: &Path,
    source_root: &Path,
    role: SourceRole,
) -> Option<SupportMaterialKind> {
    let components: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let arcade_context = matches!(role, SourceRole::ArcadeRomset)
        || path_context_contains(path, source_root, &["arcade", "mame", "fbneo"]);
    if components.iter().any(|part| {
        matches!(
            part.as_str(),
            "bios" | "firmware" | "bios_firmware" | "romsets-support"
        )
    }) || matches!(role, SourceRole::BiosFirmware)
    {
        return Some(SupportMaterialKind::BiosFirmware);
    }
    if !arcade_context {
        return None;
    }
    if components
        .iter()
        .any(|part| matches!(part.as_str(), "sample" | "samples"))
    {
        return Some(SupportMaterialKind::SamplePack);
    }
    if components.iter().any(|part| {
        matches!(
            part.as_str(),
            "nvram" | "cheats" | "artwork" | "hash" | "plugins" | "plugin"
        )
    }) {
        return Some(SupportMaterialKind::EmulatorSupport);
    }
    None
}

fn path_context_contains(path: &Path, source_root: &Path, names: &[&str]) -> bool {
    path.strip_prefix(source_root)
        .ok()
        .into_iter()
        .flat_map(Path::components)
        .chain(source_root.components())
        .any(|component| {
            let value = component.as_os_str().to_string_lossy().to_ascii_lowercase();
            names.iter().any(|name| value == *name)
        })
}

/// Finds immediate extracted set directories below an explicitly configured
/// arcade root.  A set is accepted only if it has a safe identifier-shaped
/// directory name, at least two regular members, no nested directories, and
/// no symlinks.  The source role and this structural evidence are the
/// platform evidence; the set name remains a candidate identity until MAME/
/// DAT evidence resolves it.
pub fn discover_extracted_sets(root: &Path) -> std::io::Result<ArcadeSetDiscovery> {
    let mut result = ArcadeSetDiscovery::default();
    let mut directories = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            directories.push(path);
        } else if metadata.is_file() {
            result.diagnostics.raw_files_considered += 1;
        }
    }
    directories.sort();
    for path in directories.into_iter().take(MAX_SET_DIRECTORIES) {
        if support_material_kind(&path, root, SourceRole::ArcadeRomset).is_some() {
            continue;
        }
        let Some(set_name) = safe_set_name(&path) else {
            result.diagnostics.unresolved_items += 1;
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&path) else {
            result.diagnostics.unresolved_items += 1;
            continue;
        };
        let mut members = Vec::new();
        let mut malformed = false;
        for entry in entries.take(MAX_SET_MEMBERS + 1) {
            let Ok(entry) = entry else {
                malformed = true;
                continue;
            };
            let member = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&member) else {
                malformed = true;
                continue;
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                malformed = true;
                continue;
            }
            members.push(member);
        }
        members.sort();
        if malformed || members.len() < 2 || members.len() > MAX_SET_MEMBERS {
            result.diagnostics.unresolved_items += 1;
            continue;
        }
        result.diagnostics.raw_files_considered += members.len();
        result.diagnostics.logical_sets_aggregated += 1;
        result.diagnostics.logical_set_members += members.len();
        result.diagnostics.playable_games_created += 1;
        result.sets.push(ArcadeSetDirectory {
            path,
            set_name,
            members,
        });
    }
    Ok(result)
}

fn safe_set_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.is_empty() || name.len() > 40 {
        return None;
    }
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Some(name)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn support_paths_are_contextual_and_classified() {
        let root = Path::new("/library/games");
        assert_eq!(
            support_material_kind(
                Path::new("/library/games/bios/fbneo/samples/donpachi.zip"),
                root,
                SourceRole::Games
            ),
            Some(SupportMaterialKind::SamplePack)
        );
        assert_eq!(
            support_material_kind(
                Path::new("/library/games/arcade/samples/donpachi.zip"),
                Path::new("/library/games/arcade"),
                SourceRole::ArcadeRomset
            ),
            Some(SupportMaterialKind::SamplePack)
        );
        assert_eq!(
            support_material_kind(
                Path::new("/library/games/home/samples.zip"),
                root,
                SourceRole::Games
            ),
            None
        );
    }

    #[test]
    fn extracted_set_is_one_logical_game_and_members_are_preserved() {
        let dir = tempdir().unwrap();
        let set = dir.path().join("pacman");
        fs::create_dir(&set).unwrap();
        fs::write(set.join("pacman.6e"), b"one").unwrap();
        fs::write(set.join("pacman.6f"), b"two").unwrap();
        let report = discover_extracted_sets(dir.path()).unwrap();
        assert_eq!(report.sets.len(), 1);
        assert_eq!(report.sets[0].set_name, "pacman");
        assert_eq!(report.sets[0].members.len(), 2);
        assert_eq!(report.diagnostics.logical_sets_aggregated, 1);
    }

    #[test]
    fn ambiguous_directory_fails_closed() {
        let dir = tempdir().unwrap();
        let set = dir.path().join("not a set");
        fs::create_dir(&set).unwrap();
        fs::write(set.join("a.bin"), b"one").unwrap();
        fs::write(set.join("b.bin"), b"two").unwrap();
        let report = discover_extracted_sets(dir.path()).unwrap();
        assert!(report.sets.is_empty());
        assert_eq!(report.diagnostics.unresolved_items, 1);
    }
}
