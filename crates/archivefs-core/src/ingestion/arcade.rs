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
    /// Read errors or enumeration bounds mean this is not a complete walk.
    pub scan_errors_total: usize,
    /// The walk was restricted to requested set names, so it never opened the
    /// other sets. Such a result proves nothing about what is absent and must
    /// not feed presence, coverage or Missing decisions.
    pub scoped: bool,
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
    if !arcade_context {
        return if matches!(role, SourceRole::BiosFirmware)
            || components.iter().any(|part| {
                matches!(
                    part.as_str(),
                    "bios" | "firmware" | "bios_firmware" | "romsets-support"
                )
            }) {
            Some(SupportMaterialKind::BiosFirmware)
        } else {
            None
        };
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
    if matches!(role, SourceRole::BiosFirmware)
        || components.iter().any(|part| {
            matches!(
                part.as_str(),
                "bios" | "firmware" | "bios_firmware" | "romsets-support"
            )
        })
    {
        return Some(SupportMaterialKind::BiosFirmware);
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
    discover_extracted_sets_excluding(root, &[])
}

/// Like [`discover_extracted_sets`], but opens only the sets whose normalised
/// name is in `requested_names`. The root listing, the directory bound and the
/// per-set rules are unchanged, so each returned set is exactly the one the full
/// walk returns for that name, in the same order. The result is marked
/// [`ArcadeSetDiscovery::scoped`]: it is for reading evidence about a known
/// family, never for deciding what is absent.
pub fn discover_extracted_sets_scoped(
    root: &Path,
    requested_names: &std::collections::BTreeSet<String>,
) -> std::io::Result<ArcadeSetDiscovery> {
    discover_extracted_sets_inner(root, &[], Some(requested_names))
}

pub(crate) fn discover_extracted_sets_excluding(
    root: &Path,
    excluded: &[std::path::PathBuf],
) -> std::io::Result<ArcadeSetDiscovery> {
    discover_extracted_sets_inner(root, excluded, None)
}

fn discover_extracted_sets_inner(
    root: &Path,
    excluded: &[std::path::PathBuf],
    requested_names: Option<&std::collections::BTreeSet<String>>,
) -> std::io::Result<ArcadeSetDiscovery> {
    let mut result = ArcadeSetDiscovery {
        scoped: requested_names.is_some(),
        ..Default::default()
    };
    let mut directories = Vec::new();
    let mut direct_members = Vec::new();
    let mut root_malformed = false;
    for entry in std::fs::read_dir(root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                result.scan_errors_total += 1;
                continue;
            }
        };
        let path = entry.path();
        if excluded.iter().any(|owned| path.starts_with(owned)) {
            continue;
        }
        // Neither form follows symlinks. A scoped walk takes the type from the
        // directory listing instead of stat-ing every entry of a large folder.
        let file_type = if requested_names.is_some() {
            entry.file_type()
        } else {
            std::fs::symlink_metadata(&path).map(|metadata| metadata.file_type())
        };
        let file_type = match file_type {
            Ok(file_type) => file_type,
            Err(_) => {
                result.scan_errors_total += 1;
                continue;
            }
        };
        if file_type.is_symlink() {
            root_malformed = true;
            continue;
        }
        if file_type.is_dir() {
            directories.push(path);
        } else if file_type.is_file() {
            result.diagnostics.raw_files_considered += 1;
            direct_members.push(path);
        } else {
            root_malformed = true;
        }
    }
    // A separately configured leaf below an Arcade namespace owns its own
    // extracted set. Never aggregate across a configured descendant boundary.
    let below_arcade = root.ancestors().skip(1).any(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("arcade"))
    });
    if below_arcade
        && directories.is_empty()
        && !root_malformed
        && result.scan_errors_total == 0
        && (2..=MAX_SET_MEMBERS).contains(&direct_members.len())
        && !excluded.iter().any(|p| p.starts_with(root))
        && direct_members.iter().all(|p| {
            !p.extension().is_some_and(|e| {
                matches!(
                    e.to_string_lossy().to_ascii_lowercase().as_str(),
                    "zip" | "7z" | "rar" | "cue" | "iso"
                )
            })
        })
        && let Some(set_name) = safe_set_name(root)
        && requested_names.is_none_or(|names| names.contains(&set_name))
    {
        direct_members.sort();
        result.diagnostics.logical_sets_aggregated += 1;
        result.diagnostics.logical_set_members += direct_members.len();
        result.diagnostics.playable_games_created += 1;
        result.sets.push(ArcadeSetDirectory {
            path: root.to_path_buf(),
            set_name,
            members: direct_members,
        });
    }
    directories.sort();
    if directories.len() > MAX_SET_DIRECTORIES {
        result.scan_errors_total += 1;
    }
    for path in directories.into_iter().take(MAX_SET_DIRECTORIES) {
        // A logical set cannot absorb a delegated descendant either.
        if excluded.iter().any(|owned| owned.starts_with(&path)) {
            continue;
        }
        if support_material_kind(&path, root, SourceRole::ArcadeRomset).is_some() {
            continue;
        }
        let Some(set_name) = safe_set_name(&path) else {
            result.diagnostics.unresolved_items += 1;
            continue;
        };
        if requested_names.is_some_and(|names| !names.contains(&set_name)) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&path) else {
            result.scan_errors_total += 1;
            result.diagnostics.unresolved_items += 1;
            continue;
        };
        let mut members = Vec::new();
        let mut malformed = false;
        for entry in entries.take(MAX_SET_MEMBERS + 1) {
            let Ok(entry) = entry else {
                result.scan_errors_total += 1;
                malformed = true;
                continue;
            };
            let member = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&member) else {
                result.scan_errors_total += 1;
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
        if members.len() > MAX_SET_MEMBERS {
            result.scan_errors_total += 1;
        }
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
    use std::collections::BTreeSet;
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

    fn make_set(root: &Path, name: &str) {
        let set = root.join(name);
        fs::create_dir(&set).unwrap();
        fs::write(set.join("one.bin"), b"one").unwrap();
        fs::write(set.join("two.bin"), b"two").unwrap();
    }

    fn names(report: &ArcadeSetDiscovery) -> Vec<&str> {
        report.sets.iter().map(|s| s.set_name.as_str()).collect()
    }

    #[test]
    fn scoped_discovery_is_the_full_walk_restricted_to_the_requested_names() {
        let dir = tempdir().unwrap();
        for name in ["alpha", "beta", "gamma", "delta"] {
            make_set(dir.path(), name);
        }
        fs::write(dir.path().join("loose.zip"), b"zip").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("alpha"), dir.path().join("link")).unwrap();

        let full = discover_extracted_sets(dir.path()).unwrap();
        assert!(!full.scoped);
        for subset in [
            vec!["beta", "alpha"],
            vec!["delta"],
            vec!["nothing"],
            vec!["alpha", "beta", "gamma", "delta"],
        ] {
            let wanted: BTreeSet<String> = subset.iter().map(|s| s.to_string()).collect();
            let scoped = discover_extracted_sets_scoped(dir.path(), &wanted).unwrap();
            assert!(scoped.scoped);
            let expected: Vec<_> = full
                .sets
                .iter()
                .filter(|set| wanted.contains(&set.set_name))
                .cloned()
                .collect();
            assert_eq!(scoped.sets, expected, "{subset:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn scoped_discovery_never_opens_an_unrequested_set() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        make_set(dir.path(), "wanted");
        make_set(dir.path(), "locked");
        let locked = dir.path().join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Root can read anything; the property is only observable without that.
        let can_still_read = fs::read_dir(&locked).is_ok();

        let requested = BTreeSet::from(["wanted".to_string()]);
        let scoped = discover_extracted_sets_scoped(dir.path(), &requested).unwrap();
        let full = discover_extracted_sets(dir.path()).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(names(&scoped), vec!["wanted"]);
        assert_eq!(scoped.scan_errors_total, 0);
        if !can_still_read {
            assert_eq!(full.scan_errors_total, 1, "the full walk does open it");
        }
    }

    #[test]
    fn scoped_discovery_keeps_the_root_as_a_set_rule() {
        let dir = tempdir().unwrap();
        let set = dir.path().join("arcade").join("wanted");
        fs::create_dir_all(&set).unwrap();
        fs::write(set.join("one.bin"), b"one").unwrap();
        fs::write(set.join("two.bin"), b"two").unwrap();

        let full = discover_extracted_sets(&set).unwrap();
        assert_eq!(names(&full), vec!["wanted"]);
        let yes = BTreeSet::from(["wanted".to_string()]);
        assert_eq!(
            discover_extracted_sets_scoped(&set, &yes).unwrap().sets,
            full.sets
        );
        let no = BTreeSet::from(["other".to_string()]);
        assert!(
            discover_extracted_sets_scoped(&set, &no)
                .unwrap()
                .sets
                .is_empty()
        );
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
