//! Which old catalogue rows are confidently represented by a current row.
//!
//! A library that was reorganised leaves rows at their old paths flagged
//! missing next to rows for the same files at their new paths. This module
//! derives, from rows already in the catalogue and from EmuWiz's own applied
//! rename journals, which missing rows are *superseded* by exactly one current
//! row. It is pure: it reads no file, writes nothing and deletes nothing, so the
//! historical rows stay in the database as provenance and callers decide how
//! to present them.
//!
//! Only strong evidence supersedes a row (first match wins):
//!
//! 1. an applied rename journal entry from the old path to a current row;
//! 2. an old raw arcade member whose parent folder is a current arcade set row;
//! 3. the same file name and the same size as exactly one current row;
//! 4. a path that differs only by letter case from exactly one current row of
//!    the same size;
//! 5. no current row with the same file name, but exactly one with the same
//!    extension, the same size and the same name once case and punctuation are
//!    ignored.
//!
//! A row from a configured source is only compared inside its own source folder.
//! A row from a source that is no longer configured (an old library location) is
//! compared with every current row, and then both platforms must be known and equal.
//! Journal links excepted, two known platforms must agree, and anything ambiguous - several candidates,
//! the same title alone, a different size - is left unresolved so it keeps
//! appearing, and counting, as an ordinary row.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::PersistedArchive;
use crate::dat::rename_apply::journal::list_journals;
use crate::dat::rename_apply::model::{EntryState, TransactionOperation};

const MAX_JOURNAL_HOPS: usize = 8;
const ARCADE_SET_KIND: &str = "arcade_set_directory";

/// Why a row is considered superseded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SupersessionEvidence {
    /// EmuWiz's own applied rename journal moved the file to the current path.
    JournalRename,
    /// A raw member of an arcade set that is now catalogued as one set.
    ArcadeSetMember,
    /// Same file name and size as exactly one current row.
    SameNameAndSize,
    /// Same path apart from letter case, same size, one current row.
    CaseOnlyDifference,
    /// Same extension and size, same name ignoring case and punctuation, and no
    /// current row with the exact file name.
    NormalisedNameAndSize,
}

impl SupersessionEvidence {
    /// Plain-language reason for Advanced details.
    pub fn describe(self) -> &'static str {
        match self {
            Self::JournalRename => "EmuWiz's own rename history records the move",
            Self::ArcadeSetMember => "this file is now part of an arcade game set",
            Self::SameNameAndSize => "the same file name and size exist at the new location",
            Self::CaseOnlyDifference => "the same file exists with different capital letters",
            Self::NormalisedNameAndSize => {
                "the same size and a matching name exist at the new location"
            }
        }
    }
}

/// An old row and the one current row that replaces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Supersession {
    pub old_id: i64,
    pub current_id: i64,
    pub evidence: SupersessionEvidence,
}

/// The `(old path, new path)` of every applied move recorded by the rename
/// journals in `dir`. Unreadable journals are skipped here (they are reported
/// by the History pages), and nothing is modified.
pub fn applied_renames_from_journals(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let (transactions, _problems) = list_journals(dir);
    transactions
        .into_iter()
        .flat_map(|transaction| transaction.entries)
        .filter(|entry| {
            entry.state == EntryState::Applied
                && matches!(entry.operation, TransactionOperation::RenameMove)
        })
        .map(|entry| (entry.source_path, entry.destination_path))
        .collect()
}

fn is_missing(archive: &PersistedArchive) -> bool {
    archive.last_verified_missing_at.is_some()
}

fn file_name(archive: &PersistedArchive) -> &OsStr {
    archive
        .relative_path
        .file_name()
        .unwrap_or_else(|| archive.relative_path.as_os_str())
}

fn extension_lower(archive: &PersistedArchive) -> String {
    archive
        .relative_path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn normalised_stem(archive: &PersistedArchive) -> String {
    let name = file_name(archive).to_string_lossy().to_lowercase();
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) => stem,
        None => name.as_str(),
    };
    stem.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Two known platforms must agree; an unknown one does not conflict.
fn platforms_compatible(old: &PersistedArchive, current: &PersistedArchive) -> bool {
    let canonical = |platform: &Option<String>| {
        platform
            .as_deref()
            .map(str::trim)
            .filter(|platform| !platform.is_empty())
            .map(|platform| {
                crate::canonical_platform_for_alias(platform)
                    .unwrap_or(platform)
                    .to_lowercase()
            })
    };
    match (canonical(&old.platform), canonical(&current.platform)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// Both platforms known and equal (used when crossing source folders).
fn platforms_known_equal(old: &PersistedArchive, current: &PersistedArchive) -> bool {
    let canonical = |platform: &Option<String>| {
        platform
            .as_deref()
            .map(str::trim)
            .filter(|platform| !platform.is_empty())
            .map(|platform| {
                crate::canonical_platform_for_alias(platform)
                    .unwrap_or(platform)
                    .to_lowercase()
            })
    };
    matches!((canonical(&old.platform), canonical(&current.platform)), (Some(a), Some(b)) if a == b)
}

fn single<T: Copy>(candidates: &[T]) -> Option<T> {
    (candidates.len() == 1).then(|| candidates[0])
}

/// Derives, deterministically and without side effects, which old rows are
/// superseded by exactly one current row. A row is *old* when it is flagged
/// missing or belongs to a source that is no longer configured
/// (`configured_sources`; `None` treats every source as configured); a row is
/// *current* when it is neither. `renames` are applied moves from
/// [`applied_renames_from_journals`] (pass an empty slice for none).
pub fn derive_supersessions(
    archives: &[PersistedArchive],
    renames: &[(PathBuf, PathBuf)],
    configured_sources: Option<&HashSet<i64>>,
) -> BTreeMap<i64, Supersession> {
    let configured = |archive: &PersistedArchive| {
        configured_sources.is_none_or(|sources| sources.contains(&archive.source_folder_id))
    };
    let is_old = |archive: &PersistedArchive| is_missing(archive) || !configured(archive);
    let current: Vec<&PersistedArchive> = archives.iter().filter(|a| !is_old(a)).collect();
    let by_absolute: HashMap<&Path, &PersistedArchive> = current
        .iter()
        .map(|archive| (archive.absolute_path.as_path(), *archive))
        .collect();
    let mut by_name: HashMap<(i64, &OsStr), Vec<&PersistedArchive>> = HashMap::new();
    let mut by_lower_path: HashMap<(i64, String), Vec<&PersistedArchive>> = HashMap::new();
    let mut by_norm: HashMap<(i64, String), Vec<&PersistedArchive>> = HashMap::new();
    let mut name_any: HashMap<&OsStr, Vec<&PersistedArchive>> = HashMap::new();
    let mut norm_any: HashMap<String, Vec<&PersistedArchive>> = HashMap::new();
    let mut arcade_sets: HashMap<(i64, &Path), &PersistedArchive> = HashMap::new();
    for archive in &current {
        by_name
            .entry((archive.source_folder_id, file_name(archive)))
            .or_default()
            .push(archive);
        by_lower_path
            .entry((
                archive.source_folder_id,
                archive.relative_path.to_string_lossy().to_lowercase(),
            ))
            .or_default()
            .push(archive);
        by_norm
            .entry((archive.source_folder_id, normalised_stem(archive)))
            .or_default()
            .push(archive);
        name_any
            .entry(file_name(archive))
            .or_default()
            .push(archive);
        norm_any
            .entry(normalised_stem(archive))
            .or_default()
            .push(archive);
        if archive.archive_kind == ARCADE_SET_KIND {
            arcade_sets.insert(
                (archive.source_folder_id, archive.relative_path.as_path()),
                archive,
            );
        }
    }

    // Journal links, followed through multi-step renames. A source with two
    // different final destinations is ambiguous and not used.
    let mut moves: HashMap<&Path, Vec<&Path>> = HashMap::new();
    for (from, to) in renames {
        let entry = moves.entry(from.as_path()).or_default();
        if !entry.contains(&to.as_path()) {
            entry.push(to.as_path());
        }
    }
    let journal_target = |start: &Path| -> Option<&PersistedArchive> {
        let mut at = start;
        for _ in 0..MAX_JOURNAL_HOPS {
            at = single(moves.get(at)?)?;
            if let Some(current) = by_absolute.get(at) {
                return Some(*current);
            }
        }
        None
    };

    let mut result = BTreeMap::new();
    for old in archives.iter().filter(|a| is_old(a)) {
        let source = old.source_folder_id;
        // A row from a source that is no longer configured is compared with
        // every current row; any other row only with its own source.
        let cross = !configured(old);
        let compatible = |c: &PersistedArchive| {
            if cross {
                platforms_known_equal(old, c)
            } else {
                platforms_compatible(old, c)
            }
        };
        let exact_name: Vec<&PersistedArchive> = if cross {
            name_any.get(file_name(old)).cloned().unwrap_or_default()
        } else {
            by_name
                .get(&(source, file_name(old)))
                .cloned()
                .unwrap_or_default()
        };
        let found = journal_target(&old.absolute_path)
            .filter(|current| current.id != old.id && compatible(current))
            .map(|current| (current, SupersessionEvidence::JournalRename))
            .or_else(|| {
                if cross || old.archive_kind == ARCADE_SET_KIND {
                    return None;
                }
                let parent = old.relative_path.parent()?;
                arcade_sets
                    .get(&(source, parent))
                    .map(|set| (*set, SupersessionEvidence::ArcadeSetMember))
            })
            .or_else(|| {
                let size = old.size_bytes?;
                let same_size: Vec<&PersistedArchive> = exact_name
                    .iter()
                    .copied()
                    .filter(|c| c.size_bytes == Some(size) && compatible(c))
                    .collect();
                single(&same_size).map(|c| (c, SupersessionEvidence::SameNameAndSize))
            })
            .or_else(|| {
                if cross {
                    return None;
                }
                let size = old.size_bytes?;
                let key = (source, old.relative_path.to_string_lossy().to_lowercase());
                let same_path: Vec<&PersistedArchive> = by_lower_path
                    .get(&key)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .copied()
                    .filter(|c| c.size_bytes == Some(size) && compatible(c))
                    .collect();
                single(&same_path).map(|c| (c, SupersessionEvidence::CaseOnlyDifference))
            })
            .or_else(|| {
                let size = old.size_bytes?;
                // Any current row with the exact name (whatever its size) makes
                // the match ambiguous rather than "strong".
                if !exact_name.is_empty() {
                    return None;
                }
                let norm = normalised_stem(old);
                if norm.is_empty() {
                    return None;
                }
                let ext = extension_lower(old);
                let pool: &[&PersistedArchive] = if cross {
                    norm_any.get(&norm).map(Vec::as_slice).unwrap_or(&[])
                } else {
                    by_norm
                        .get(&(source, norm))
                        .map(Vec::as_slice)
                        .unwrap_or(&[])
                };
                let similar: Vec<&PersistedArchive> = pool
                    .iter()
                    .copied()
                    .filter(|c| {
                        c.size_bytes == Some(size) && extension_lower(c) == ext && compatible(c)
                    })
                    .collect();
                single(&similar).map(|c| (c, SupersessionEvidence::NormalisedNameAndSize))
            });
        if let Some((current, evidence)) = found
            && current.id != old.id
        {
            result.insert(
                old.id,
                Supersession {
                    old_id: old.id,
                    current_id: current.id,
                    evidence,
                },
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        id: i64,
        rel: &str,
        size: u64,
        missing: bool,
        platform: Option<&str>,
    ) -> PersistedArchive {
        PersistedArchive {
            id,
            source_folder_id: 1,
            relative_path: rel.into(),
            absolute_path: Path::new("/games").join(rel),
            archive_kind: "zip".into(),
            display_name: rel.into(),
            normalized_name: rel.to_lowercase(),
            size_bytes: Some(size),
            modified_time_unix_seconds: Some(1),
            platform: platform.map(str::to_string),
            platform_source: None,
            last_known_health: "Pending".into(),
            last_seen_at: "x".into(),
            last_verified_missing_at: missing.then(|| "2026-01-01T00:00:00Z".into()),
            identity_report: None,
        }
    }

    fn derive(rows: &[PersistedArchive]) -> BTreeMap<i64, Supersession> {
        derive_supersessions(rows, &[], None)
    }

    #[test]
    fn a_unique_same_name_and_size_move_is_superseded() {
        let rows = [
            row(1, "gameboy/Tetris.gb", 32768, true, Some("Game Boy")),
            row(2, "gb/Tetris.gb", 32768, false, Some("Game Boy")),
        ];
        let result = derive(&rows);
        assert_eq!(result[&1].current_id, 2);
        assert_eq!(result[&1].evidence, SupersessionEvidence::SameNameAndSize);
        assert!(
            !result.contains_key(&2),
            "a current row is never superseded"
        );
    }

    #[test]
    fn an_ambiguous_duplicate_name_does_not_supersede() {
        let rows = [
            row(1, "old/Tetris.gb", 32768, true, None),
            row(2, "gb/Tetris.gb", 32768, false, None),
            row(3, "backup/Tetris.gb", 32768, false, None),
        ];
        assert!(derive(&rows).is_empty());
    }

    #[test]
    fn a_different_size_or_the_same_title_alone_does_not_supersede() {
        let rows = [
            row(1, "old/Tetris.gb", 32768, true, None),
            row(2, "gb/Tetris.gb", 40000, false, None),
            row(3, "old/Mario Land (USA).gb", 65536, true, None),
            row(4, "gb/Mario Land (Europe).gb", 65536, false, None),
            row(5, "old/Zelda.gb", 1, true, None),
            row(6, "gb/Zelda Remake.gb", 1, false, None),
        ];
        assert!(derive(&rows).is_empty());
    }

    #[test]
    fn two_known_platforms_must_agree() {
        let rows = [
            row(1, "old/Game.zip", 10, true, Some("SNES")),
            row(2, "new/Game.zip", 10, false, Some("NES")),
        ];
        assert!(derive(&rows).is_empty());
        let rows = [
            row(1, "old/Game.zip", 10, true, None),
            row(2, "new/Game.zip", 10, false, Some("NES")),
        ];
        assert_eq!(
            derive(&rows).len(),
            1,
            "an unknown platform does not conflict"
        );
    }

    #[test]
    fn rows_in_other_sources_are_not_compared() {
        let mut other = row(2, "gb/Tetris.gb", 32768, false, None);
        other.source_folder_id = 2;
        let rows = [row(1, "gameboy/Tetris.gb", 32768, true, None), other];
        assert!(derive(&rows).is_empty());
    }

    #[test]
    fn a_case_only_difference_is_superseded_when_unique_and_equal() {
        let rows = [
            row(
                1,
                "snes/RoboCop versus The Terminator (USA).zip",
                139289,
                true,
                None,
            ),
            row(
                2,
                "snes/RoboCop Versus The Terminator (USA).zip",
                139289,
                false,
                None,
            ),
        ];
        let result = derive(&rows);
        assert_eq!(
            result[&1].evidence,
            SupersessionEvidence::CaseOnlyDifference
        );
        let different_size = [
            row(1, "snes/Robocop.zip", 1, true, None),
            row(2, "snes/RoboCop.zip", 2, false, None),
        ];
        assert!(derive(&different_size).is_empty());
    }

    #[test]
    fn a_normalised_name_needs_the_same_size_extension_and_no_exact_name() {
        let rows = [
            row(
                1,
                "pcengine/Pachio Kun - Juuban Shoubu (Japan).zip",
                349042,
                true,
                None,
            ),
            row(
                2,
                "tg16/Pachio-kun - Juuban Shoubu (Japan).zip",
                349042,
                false,
                None,
            ),
        ];
        assert_eq!(
            derive(&rows)[&1].evidence,
            SupersessionEvidence::NormalisedNameAndSize
        );
        let other_extension = [
            row(1, "a/Game One.zip", 5, true, None),
            row(2, "b/Game-One.7z", 5, false, None),
        ];
        assert!(derive(&other_extension).is_empty());
        // An exact-name row of another size makes it ambiguous, not strong.
        let exact_name_elsewhere = [
            row(1, "a/Game One.zip", 5, true, None),
            row(2, "b/Game-One.zip", 5, false, None),
            row(3, "c/Game One.zip", 9, false, None),
        ];
        assert!(derive(&exact_name_elsewhere).is_empty());
        // Two similar candidates are ambiguous.
        let two = [
            row(1, "a/Game One.zip", 5, true, None),
            row(2, "b/Game-One.zip", 5, false, None),
            row(3, "c/Game_One.zip", 5, false, None),
        ];
        assert!(derive(&two).is_empty());
    }

    #[test]
    fn an_old_raw_arcade_member_is_replaced_by_its_set() {
        let mut set = row(2, "arcade/blackbdb", 0, false, None);
        set.archive_kind = ARCADE_SET_KIND.into();
        let mut member = row(
            1,
            "arcade/blackbdb/blackbeard  ru_04b.img",
            65142784,
            true,
            None,
        );
        member.archive_kind = "direct_game_image".into();
        let present_member = row(3, "arcade/blackbdb/other.img", 1, false, None);
        let result = derive(&[member, set, present_member]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[&1].current_id, 2);
        assert_eq!(result[&1].evidence, SupersessionEvidence::ArcadeSetMember);
    }

    #[test]
    fn a_journalled_rename_links_old_to_new_even_with_a_different_name() {
        let rows = [
            row(
                1,
                "atari2600/A-Team, The (USA) (Proto).zip",
                100,
                true,
                None,
            ),
            row(
                2,
                "atari2600/A-Team, The (1984)(Atari).zip",
                100,
                false,
                None,
            ),
        ];
        let renames = [(
            PathBuf::from("/games/atari2600/A-Team, The (USA) (Proto).zip"),
            PathBuf::from("/games/atari2600/A-Team, The (1984)(Atari).zip"),
        )];
        let result = derive_supersessions(&rows, &renames, None);
        assert_eq!(result[&1].evidence, SupersessionEvidence::JournalRename);
        // Multi-step renames are followed.
        let steps = [
            (PathBuf::from("/games/a.zip"), PathBuf::from("/games/b.zip")),
            (PathBuf::from("/games/b.zip"), PathBuf::from("/games/c.zip")),
        ];
        let rows = [
            row(1, "a.zip", 1, true, None),
            row(2, "c.zip", 1, false, None),
        ];
        assert_eq!(derive_supersessions(&rows, &steps, None)[&1].current_id, 2);
        // A source with two different destinations is ambiguous.
        let forked = [
            (PathBuf::from("/games/a.zip"), PathBuf::from("/games/c.zip")),
            (PathBuf::from("/games/a.zip"), PathBuf::from("/games/d.zip")),
        ];
        let rows = [
            row(1, "a.zip", 1, true, None),
            row(2, "c.zip", 2, false, None),
            row(3, "d.zip", 3, false, None),
        ];
        assert!(derive_supersessions(&rows, &forked, None).is_empty());
        // A rename whose destination is not a current row proves nothing.
        let rows = [row(1, "a.zip", 1, true, None)];
        assert!(derive_supersessions(&rows, &steps, None).is_empty());
    }

    #[test]
    fn a_row_from_a_source_that_is_no_longer_configured_needs_a_unique_current_match() {
        let mut old = row(1, "roms/snes/Game.sfc", 10, false, Some("SNES"));
        old.source_folder_id = 9;
        let mut current = row(2, "snes/Game.sfc", 10, false, Some("SNES"));
        current.source_folder_id = 1;
        let configured: HashSet<i64> = HashSet::from([1]);
        let rows = [old.clone(), current.clone()];
        let result = derive_supersessions(&rows, &[], Some(&configured));
        assert_eq!(result[&1].current_id, 2);
        assert_eq!(result[&1].evidence, SupersessionEvidence::SameNameAndSize);
        assert!(!result.contains_key(&2), "a configured row is current");
        // The same two rows are unrelated when both sources are configured.
        let both: HashSet<i64> = HashSet::from([1, 9]);
        assert!(derive_supersessions(&rows, &[], Some(&both)).is_empty());
        // Cross-source matches need both platforms known and equal.
        let mut unknown = old.clone();
        unknown.platform = None;
        assert!(
            derive_supersessions(&[unknown, current.clone()], &[], Some(&configured)).is_empty()
        );
        let mut other = old.clone();
        other.platform = Some("NES".into());
        assert!(derive_supersessions(&[other, current.clone()], &[], Some(&configured)).is_empty());
        // Two current copies make it ambiguous; an old row is never a target.
        let mut twin = current.clone();
        twin.id = 3;
        twin.relative_path = "elsewhere/Game.sfc".into();
        twin.absolute_path = "/games/elsewhere/Game.sfc".into();
        assert!(
            derive_supersessions(
                &[old.clone(), current.clone(), twin],
                &[],
                Some(&configured)
            )
            .is_empty()
        );
        let mut second_old = old.clone();
        second_old.id = 4;
        second_old.relative_path = "more/Game.sfc".into();
        let both_old = derive_supersessions(&[old, second_old, current], &[], Some(&configured));
        assert_eq!(
            both_old.len(),
            2,
            "several old rows may share one current row"
        );
    }

    #[test]
    fn derivation_is_deterministic_and_changes_nothing() {
        let rows = [
            row(1, "old/Tetris.gb", 32768, true, None),
            row(2, "gb/Tetris.gb", 32768, false, None),
        ];
        let before = format!("{rows:?}");
        assert_eq!(derive(&rows), derive(&rows));
        assert_eq!(before, format!("{rows:?}"));
    }

    #[test]
    fn only_applied_move_entries_in_journals_count() {
        use crate::dat::rename_apply::journal::write_journal;
        use crate::dat::rename_apply::model::RenameTransaction;
        let dir = tempfile::tempdir().unwrap();
        let entry = |from: &str, to: &str, state: &str| -> serde_json::Value {
            serde_json::json!({
                "source_path": from, "destination_path": to,
                "original_basename": "x", "proposed_basename": "y",
                "identity": {"size_bytes": 1, "modified_unix": 1, "kind": "regular_file"},
                "state": state
            })
        };
        let transaction: RenameTransaction = serde_json::from_value(serde_json::json!({
            "transaction_id": "1-0", "plan_generation": 0, "created_at_unix": 1,
            "source_scan_root": "/games", "state": "applied",
            "entries": [
                entry("/games/a.zip", "/games/b.zip", "applied"),
                entry("/games/c.zip", "/games/d.zip", "rolled_back"),
                entry("/games/e.zip", "/games/f.zip", "planned"),
            ]
        }))
        .unwrap();
        write_journal(dir.path(), &transaction).unwrap();
        let renames = applied_renames_from_journals(dir.path());
        assert_eq!(
            renames,
            vec![(PathBuf::from("/games/a.zip"), PathBuf::from("/games/b.zip"))]
        );
        assert!(applied_renames_from_journals(&dir.path().join("missing")).is_empty());
    }
}
