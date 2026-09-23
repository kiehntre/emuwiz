//! Safe, checksum-driven normalisation of MAME sets.
//!
//! This is deliberately narrower than a general library organiser: the DAT
//! is the only source of a mutation identity.  A filename, title, or fuzzy
//! similarity can make an item visible in a preview, but can never make it
//! actionable.  ZIP rebuilds are staged and reopened before publication.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use md5::Md5;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use super::index::{DatIndex, DatRomRef};
use super::mame_arcade_join::{ArcadeJoinClass, ArcadeJoinEvidence, MemberEvidenceKind};
use super::model::ParsedDat;
use crate::dat::rename_apply::noclobber::rename_noreplace;
use crate::identity_source::hashing::Crc32;

/// The layout a collection uses.  The normaliser never silently converts one
/// layout to another; this choice is part of the reviewed plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MameCollectionMode {
    Merged,
    Split,
    NonMerged,
    #[default]
    NotSure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MameFixStatus {
    Safe,
    AlreadyCorrect,
    NeedsAttention,
    Collision,
    MissingData,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameMemberFix {
    pub current: String,
    pub correct: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameSetFix {
    pub current_path: PathBuf,
    pub correct_path: PathBuf,
    pub set_name: Option<String>,
    pub parent: Option<String>,
    pub status: MameFixStatus,
    pub members: Vec<MameMemberFix>,
    pub missing_members: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameNormalisationSummary {
    pub total_sets: usize,
    pub safe: usize,
    pub already_correct: usize,
    pub needs_attention: usize,
    pub collisions: usize,
    pub missing_data: usize,
    pub unknown: usize,
    pub outer_renames: usize,
    pub member_renames: usize,
    pub archive_rebuilds: usize,
    pub split_rebuilds: usize,
    pub moved_members: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameNormalisationPlan {
    pub root: PathBuf,
    pub mode: MameCollectionMode,
    pub dat_version: Option<String>,
    pub sets: Vec<MameSetFix>,
    #[serde(default)]
    pub split_rebuilds: Vec<MameSplitRebuild>,
    pub summary: MameNormalisationSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameSplitRebuild {
    pub parent_path: PathBuf,
    pub parent_target: PathBuf,
    pub clone_path: PathBuf,
    pub clone_target: PathBuf,
    pub parent_members: Vec<MameRebuildMember>,
    pub clone_members: Vec<MameRebuildMember>,
    pub moved_members: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MameRebuildMember {
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    pub source_member: String,
    pub correct: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedSet {
    fix: MameSetFix,
    observed_members: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MameRepairJournal {
    version: u32,
    batch_id: String,
    entries: Vec<MameRepairEntry>,
    state: MameRepairState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MameRepairEntry {
    original: PathBuf,
    published: PathBuf,
    backup: Option<PathBuf>,
    original_sha256: String,
    published_sha256: String,
    members: Vec<MameMemberFix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MameRepairState {
    Applying,
    Applied,
    Undone,
}

/// Scan one MAME folder. Only immediate ZIPs/directories are set candidates;
/// loose files are reported as Unknown and are never promoted into sets.
pub fn plan_mame_normalisation(
    root: &Path,
    dat: &ParsedDat,
    mode: MameCollectionMode,
) -> Result<MameNormalisationPlan, String> {
    if mode == MameCollectionMode::NotSure {
        return Err("choose or confirm a MAME collection layout before previewing".into());
    }
    if !root.is_dir() {
        return Err(format!(
            "MAME folder is not a directory: {}",
            root.display()
        ));
    }
    let index = DatIndex::build(dat);
    let mut planned = Vec::new();
    let mut children = fs::read_dir(root)
        .map_err(|e| format!("cannot read MAME folder: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("cannot enumerate MAME folder: {e}"))?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            planned.push(plan_zip(&path, &index, mode)?);
        } else if meta.is_dir() {
            planned.push(plan_directory(&path, &index, mode)?);
        } else if meta.is_file() {
            planned.push(PlannedSet {
                fix: MameSetFix {
                    current_path: path.clone(),
                    correct_path: path,
                    set_name: None,
                    parent: None,
                    status: MameFixStatus::Unknown,
                    members: Vec::new(),
                    missing_members: Vec::new(),
                    reason: Some("not a MAME set archive or directory".into()),
                },
                observed_members: BTreeSet::new(),
            });
        }
    }
    apply_layout_completeness(dat, mode, &mut planned);
    let split_rebuilds = if mode == MameCollectionMode::Split {
        plan_split_rebuilds(root, dat, &index, &mut planned)
    } else {
        Vec::new()
    };
    let sets = planned
        .into_iter()
        .map(|planned| planned.fix)
        .collect::<Vec<_>>();
    let mut summary = MameNormalisationSummary {
        total_sets: sets.len(),
        ..Default::default()
    };
    for set in &sets {
        match set.status {
            MameFixStatus::Safe => summary.safe += 1,
            MameFixStatus::AlreadyCorrect => summary.already_correct += 1,
            MameFixStatus::NeedsAttention => summary.needs_attention += 1,
            MameFixStatus::Collision => summary.collisions += 1,
            MameFixStatus::MissingData => summary.missing_data += 1,
            MameFixStatus::Unknown => summary.unknown += 1,
        }
        if set.current_path != set.correct_path {
            summary.outer_renames += 1;
        }
        summary.member_renames += set.members.len();
        if set
            .current_path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
            && !set.members.is_empty()
        {
            summary.archive_rebuilds += 1;
        }
    }
    summary.split_rebuilds = split_rebuilds.len();
    summary.moved_members = split_rebuilds
        .iter()
        .map(|rebuild| rebuild.moved_members)
        .sum();
    Ok(MameNormalisationPlan {
        root: root.to_path_buf(),
        mode,
        dat_version: dat.source.version.clone(),
        sets,
        split_rebuilds,
        summary,
    })
}

/// Makes a bounded, non-mutating layout suggestion. It samples at most 24
/// DAT parent/clone families and uses member placement only as a hint; the
/// user must confirm the returned mode before planning mutations.
pub fn detect_mame_collection_mode(
    root: &Path,
    dat: &ParsedDat,
) -> Result<MameCollectionMode, String> {
    if !root.is_dir() {
        return Err(format!(
            "MAME folder is not a directory: {}",
            root.display()
        ));
    }
    let mut merged = 0usize;
    let mut split = 0usize;
    let mut non_merged = 0usize;
    let mut sampled = 0usize;
    for game in dat
        .games
        .iter()
        .filter(|game| game.clone_of.is_some())
        .take(24)
    {
        let parent = game.clone_of.as_deref().unwrap_or_default();
        let clone_path = find_set_path(root, &game.name);
        let parent_path = find_set_path(root, parent);
        let Some(parent_path) = parent_path else {
            continue;
        };
        if clone_path.is_none() {
            let parent_members = member_names(&parent_path)?;
            let mut clone_names = game.roms.iter().map(|rom| rom.name.as_str());
            if clone_names.any(|name| parent_members.contains(name)) {
                merged += 1;
                sampled += 1;
            }
            continue;
        }
        let clone_path = clone_path.expect("clone path checked above");
        sampled += 1;
        let clone_members = member_names(&clone_path)?;
        let parent_names = dat
            .games
            .iter()
            .find(|candidate| candidate.name == parent)
            .map(|candidate| {
                candidate
                    .roms
                    .iter()
                    .map(|rom| rom.name.as_str())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        if !parent_names.is_empty()
            && parent_names
                .iter()
                .all(|name| clone_members.contains(*name))
        {
            non_merged += 1;
        } else {
            split += 1;
        }
    }
    if sampled == 0 {
        return Ok(MameCollectionMode::Split);
    }
    if merged > split && merged > non_merged {
        Ok(MameCollectionMode::Merged)
    } else if non_merged > split {
        Ok(MameCollectionMode::NonMerged)
    } else {
        Ok(MameCollectionMode::Split)
    }
}

fn find_set_path(root: &Path, name: &str) -> Option<PathBuf> {
    let directory = root.join(name);
    if directory.is_dir() {
        return Some(directory);
    }
    let archive = root.join(format!("{name}.zip"));
    archive.is_file().then_some(archive)
}

fn member_names(path: &Path) -> Result<BTreeSet<String>, String> {
    if path.is_dir() {
        let mut files = Vec::new();
        collect_files(path, path, &mut files)?;
        return Ok(files.into_iter().map(|(name, _)| name).collect());
    }
    let mut archive = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let member = archive.by_index(index).map_err(|e| e.to_string())?;
        if !member.is_dir() {
            names.insert(member.name().to_string());
        }
    }
    Ok(names)
}

/// Fast production preview over already trusted, SHA-bound Arcade audit
/// evidence. The audit is the proof for the set identity; un-audited objects
/// remain visible as Unknown and are never mutated by this path.
pub fn plan_mame_normalisation_from_verified_joins(
    root: &Path,
    dat: &ParsedDat,
    joins: &[(PathBuf, ArcadeJoinEvidence)],
    mode: MameCollectionMode,
) -> Result<MameNormalisationPlan, String> {
    if mode == MameCollectionMode::NotSure {
        return Err("choose or confirm a MAME collection layout before previewing".into());
    }
    if !root.is_dir() {
        return Err(format!(
            "MAME folder is not a directory: {}",
            root.display()
        ));
    }
    let mut by_path = BTreeMap::new();
    for (path, evidence) in joins {
        if path.starts_with(root) {
            by_path.insert(path.clone(), evidence);
        }
    }
    let mut planned = Vec::new();
    let mut children = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        let path = entry.path();
        let Some(evidence) = by_path.get(&path) else {
            planned.push(PlannedSet {
                fix: unresolved(
                    &path,
                    path.clone(),
                    "no current trusted MAME audit evidence",
                ),
                observed_members: BTreeSet::new(),
            });
            continue;
        };
        let Some(name) = evidence.dat_set_name.clone() else {
            planned.push(PlannedSet {
                fix: unresolved(&path, path.clone(), "trusted audit has no DAT set name"),
                observed_members: BTreeSet::new(),
            });
            continue;
        };
        let target = if path.is_dir() {
            path.with_file_name(&name)
        } else {
            path.with_file_name(format!("{name}.zip"))
        };
        let observed_members = evidence
            .members
            .iter()
            .filter(|member| {
                matches!(
                    member.kind,
                    MemberEvidenceKind::Present
                        | MemberEvidenceKind::MergedFromParent
                        | MemberEvidenceKind::ProvidedByBios
                        | MemberEvidenceKind::ProvidedByDevice
                )
            })
            .map(|member| member.name.clone())
            .collect::<BTreeSet<_>>();
        let blocked = evidence.class == ArcadeJoinClass::Ambiguous
            || evidence
                .dependencies
                .iter()
                .any(|dependency| !dependency.present);
        let collision = target != path && target.exists();
        let status = if collision {
            MameFixStatus::Collision
        } else if blocked {
            MameFixStatus::MissingData
        } else if target == path {
            MameFixStatus::AlreadyCorrect
        } else {
            MameFixStatus::Safe
        };
        planned.push(PlannedSet {
            fix: MameSetFix {
                current_path: path,
                correct_path: target,
                set_name: Some(name),
                parent: evidence.clone_of.clone(),
                status,
                members: Vec::new(),
                missing_members: Vec::new(),
                reason: if collision {
                    Some("destination already exists; no overwrite".into())
                } else if blocked {
                    Some("trusted MAME audit reports missing members or dependencies".into())
                } else {
                    None
                },
            },
            observed_members,
        });
    }
    apply_layout_completeness(dat, mode, &mut planned);
    let sets = planned
        .into_iter()
        .map(|planned| planned.fix)
        .collect::<Vec<_>>();
    let mut summary = MameNormalisationSummary {
        total_sets: sets.len(),
        ..Default::default()
    };
    for set in &sets {
        match set.status {
            MameFixStatus::Safe => summary.safe += 1,
            MameFixStatus::AlreadyCorrect => summary.already_correct += 1,
            MameFixStatus::NeedsAttention => summary.needs_attention += 1,
            MameFixStatus::Collision => summary.collisions += 1,
            MameFixStatus::MissingData => summary.missing_data += 1,
            MameFixStatus::Unknown => summary.unknown += 1,
        }
        if set.current_path != set.correct_path {
            summary.outer_renames += 1;
        }
    }
    Ok(MameNormalisationPlan {
        root: root.to_path_buf(),
        mode,
        dat_version: joins
            .first()
            .map(|(_, evidence)| evidence.dat_version.clone()),
        sets,
        split_rebuilds: Vec::new(),
        summary,
    })
}

#[derive(Debug, Clone)]
struct LocatedRom {
    archive: PathBuf,
    current: String,
    rom: DatRomRef,
    sha256: String,
}

fn plan_split_rebuilds(
    root: &Path,
    dat: &ParsedDat,
    index: &DatIndex,
    planned: &mut [PlannedSet],
) -> Vec<MameSplitRebuild> {
    let mut rebuilds = Vec::new();
    let mut used_archives = BTreeSet::new();
    let mut scanned = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return rebuilds;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_zip(&path)
            && let Ok(members) = locate_zip_members(&path, index)
        {
            scanned.push((path, members));
        }
    }
    for clone in dat.games.iter().filter(|game| game.clone_of.is_some()) {
        let Some(parent_name) = clone.clone_of.as_deref() else {
            continue;
        };
        let Some(parent_game) = dat.games.iter().find(|game| game.name == parent_name) else {
            continue;
        };
        let clone_specific = clone
            .roms
            .iter()
            .filter(|candidate| {
                !parent_game
                    .roms
                    .iter()
                    .any(|parent| rom_entries_match(candidate, parent))
            })
            .collect::<Vec<_>>();
        if clone_specific.is_empty() {
            continue;
        }
        let clone_paths = scanned
            .iter()
            .filter(|(_, members)| {
                members.iter().any(|member| {
                    clone_specific.iter().any(|candidate| {
                        member.rom.rom_name == candidate.name
                            && rom_identity_matches(candidate, &member.rom)
                    })
                })
            })
            .map(|(path, _)| path)
            .collect::<Vec<_>>();
        if clone_paths.len() != 1 {
            continue;
        }
        let clone_path = clone_paths[0].clone();
        let parent_paths = scanned
            .iter()
            .filter(|(path, members)| {
                *path != clone_path
                    && members
                        .iter()
                        .any(|member| dat_rom_for_game(parent_game, &member.rom))
            })
            .map(|(path, _)| path)
            .collect::<Vec<_>>();
        if parent_paths.len() != 1 {
            continue;
        }
        let parent_path = parent_paths[0].clone();
        if used_archives.contains(&parent_path) || used_archives.contains(&clone_path) {
            continue;
        }
        let Some(parent_index) = planned
            .iter()
            .position(|set| set.fix.current_path == parent_path)
        else {
            continue;
        };
        let Some(clone_index) = planned
            .iter()
            .position(|set| set.fix.current_path == clone_path)
        else {
            continue;
        };
        if parent_index == clone_index {
            continue;
        }
        let parent_target = root.join(format!("{parent_name}.zip"));
        let clone_target = root.join(format!("{}.zip", clone.name));
        if (parent_target.exists() && parent_target != parent_path && parent_target != clone_path)
            || (clone_target.exists() && clone_target != parent_path && clone_target != clone_path)
        {
            continue;
        }
        let Some((_, mut located)) = scanned
            .iter()
            .find(|(path, _)| *path == parent_path)
            .cloned()
        else {
            continue;
        };
        let Some((_, mut clone_located)) = scanned
            .iter()
            .find(|(path, _)| *path == clone_path)
            .cloned()
        else {
            continue;
        };
        located.append(&mut clone_located);
        if located.is_empty()
            || located.iter().any(|member| {
                !dat_rom_for_game(parent_game, &member.rom) && !dat_rom_for_game(clone, &member.rom)
            })
        {
            continue;
        }
        let Some(parent_members) =
            desired_split_members(parent_game, clone, &located, &parent_target, false)
        else {
            continue;
        };
        let Some(clone_members) =
            desired_split_members(clone, parent_game, &located, &clone_target, true)
        else {
            continue;
        };
        let moved_members = parent_members
            .iter()
            .chain(clone_members.iter())
            .filter(|member| member.source_path != member.destination_path)
            .count();
        let changed = parent_path != parent_target
            || clone_path != clone_target
            || moved_members > 0
            || !archive_matches(&parent_path, &parent_members)
            || !archive_matches(&clone_path, &clone_members);
        if !changed {
            continue;
        }
        for position in [parent_index, clone_index] {
            planned[position].fix.status = MameFixStatus::Safe;
            planned[position].fix.members.clear();
            planned[position].fix.missing_members.clear();
            planned[position].fix.reason = Some("safe Split parent/clone rebuild".into());
        }
        used_archives.insert(parent_path.clone());
        used_archives.insert(clone_path.clone());
        rebuilds.push(MameSplitRebuild {
            parent_path,
            parent_target,
            clone_path,
            clone_target,
            parent_members,
            clone_members,
            moved_members,
        });
    }
    rebuilds
}

fn locate_zip_members(path: &Path, index: &DatIndex) -> Result<Vec<LocatedRom>, String> {
    let mut archive = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut located = Vec::new();
    for position in 0..archive.len() {
        let mut member = archive.by_index(position).map_err(|e| e.to_string())?;
        if member.is_dir() {
            continue;
        }
        let current = member.name().to_string();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let digest = hashes(&bytes);
        let candidates = lookup(index, &digest);
        if candidates.is_empty()
            || candidates
                .iter()
                .map(|candidate| candidate.rom_name.as_str())
                .collect::<BTreeSet<_>>()
                .len()
                != 1
        {
            return Err(format!("member is not uniquely DAT-identified: {current}"));
        }
        located.push(LocatedRom {
            archive: path.to_path_buf(),
            current,
            rom: candidates[0].clone(),
            sha256: digest.sha256,
        });
    }
    Ok(located)
}

fn dat_rom_for_game(game: &crate::dat::model::DatGameEntry, rom: &DatRomRef) -> bool {
    game.roms
        .iter()
        .any(|candidate| candidate.name == rom.rom_name && rom_identity_matches(candidate, rom))
}

fn rom_identity_matches(candidate: &crate::dat::model::DatRomEntry, rom: &DatRomRef) -> bool {
    candidate
        .sha256
        .as_deref()
        .is_some_and(|value| rom.checksums.iter().any(|checksum| checksum.value == value))
        || candidate
            .sha1
            .as_deref()
            .is_some_and(|value| rom.checksums.iter().any(|checksum| checksum.value == value))
        || candidate
            .md5
            .as_deref()
            .is_some_and(|value| rom.checksums.iter().any(|checksum| checksum.value == value))
        || candidate
            .crc32
            .as_deref()
            .is_some_and(|value| rom.checksums.iter().any(|checksum| checksum.value == value))
}

fn desired_split_members(
    destination_game: &crate::dat::model::DatGameEntry,
    other_game: &crate::dat::model::DatGameEntry,
    located: &[LocatedRom],
    destination_path: &Path,
    exclude_shared: bool,
) -> Option<Vec<MameRebuildMember>> {
    let desired = destination_game.roms.iter().filter(|candidate| {
        !exclude_shared
            || !other_game
                .roms
                .iter()
                .any(|other| rom_entries_match(candidate, other))
    });
    let mut result = Vec::new();
    for candidate in desired {
        let matches = located
            .iter()
            .filter(|member| {
                member.rom.rom_name == candidate.name
                    && rom_identity_matches(candidate, &member.rom)
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return None;
        }
        let member = matches[0];
        result.push(MameRebuildMember {
            source_path: member.archive.clone(),
            destination_path: destination_path.to_path_buf(),
            source_member: member.current.clone(),
            correct: candidate.name.clone(),
            sha256: member.sha256.clone(),
        });
    }
    Some(result)
}

fn rom_entries_match(
    left: &crate::dat::model::DatRomEntry,
    right: &crate::dat::model::DatRomEntry,
) -> bool {
    left.sha256.is_some() && left.sha256 == right.sha256
        || left.sha1.is_some() && left.sha1 == right.sha1
        || left.md5.is_some() && left.md5 == right.md5
        || left.crc32.is_some() && left.crc32 == right.crc32
}

fn archive_matches(path: &Path, members: &[MameRebuildMember]) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    archive.len() == members.len()
        && members.iter().all(|member| {
            let Ok(mut actual) = archive.by_name(&member.correct) else {
                return false;
            };
            let mut bytes = Vec::new();
            actual.read_to_end(&mut bytes).is_ok() && hashes(&bytes).sha256 == member.sha256
        })
}

fn plan_zip(path: &Path, index: &DatIndex, mode: MameCollectionMode) -> Result<PlannedSet, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("invalid ZIP: {e}"))?;
    let mut members = Vec::new();
    let mut matches = BTreeMap::<String, Vec<(String, DatRomRef)>>::new();
    for i in 0..archive.len() {
        let mut item = archive.by_index(i).map_err(|e| e.to_string())?;
        if item.is_dir() {
            continue;
        }
        let name = item.name().to_string();
        // ZIP carries the member CRC in its central directory. Use that
        // indexed DAT evidence first; only decompress when it is missing or
        // collides. This keeps a large library preview proportional to the
        // archive index rather than reading every ROM twice.
        let crc = format!("{:08x}", item.crc32());
        let mut candidates = index.lookup_crc32(&crc).iter().collect::<Vec<_>>();
        if candidates.len() != 1 {
            let mut bytes = Vec::new();
            item.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            candidates = lookup(index, &hashes(&bytes));
        }
        for candidate in candidates {
            matches
                .entry(candidate.game_name.clone())
                .or_default()
                .push((name.clone(), candidate.clone()));
        }
    }
    let Some((game_name, parent, found)) = choose_game(&matches, mode) else {
        return Ok(PlannedSet {
            fix: unresolved(
                path,
                path.with_file_name(path.file_name().unwrap_or_default()),
                "no unique DAT checksum identity",
            ),
            observed_members: BTreeSet::new(),
        });
    };
    for (current, rom) in &found {
        if current != &rom.rom_name {
            members.push(MameMemberFix {
                current: current.clone(),
                correct: rom.rom_name.clone(),
                sha256: digest_for_member(path, current).unwrap_or_default(),
            });
        }
    }
    let observed_members = found.iter().map(|(_, rom)| rom.rom_name.clone()).collect();
    Ok(PlannedSet {
        fix: finish_set(path, &game_name, parent, members)?,
        observed_members,
    })
}

fn plan_directory(
    path: &Path,
    index: &DatIndex,
    mode: MameCollectionMode,
) -> Result<PlannedSet, String> {
    let mut files = Vec::new();
    collect_files(path, path, &mut files)?;
    let mut matches = BTreeMap::<String, Vec<(String, DatRomRef)>>::new();
    for (relative, file) in files {
        let bytes = fs::read(&file).map_err(|e| e.to_string())?;
        for candidate in lookup(index, &hashes(&bytes)) {
            matches
                .entry(candidate.game_name.clone())
                .or_default()
                .push((relative.clone(), candidate.clone()));
        }
    }
    let Some((game_name, parent, found)) = choose_game(&matches, mode) else {
        return Ok(PlannedSet {
            fix: unresolved(path, path.to_path_buf(), "no unique DAT checksum identity"),
            observed_members: BTreeSet::new(),
        });
    };
    let members = found
        .iter()
        .filter(|(current, rom)| current != &rom.rom_name)
        .map(|(current, rom)| MameMemberFix {
            current: current.clone(),
            correct: rom.rom_name.clone(),
            sha256: sha256_file(&path.join(current)).unwrap_or_default(),
        })
        .collect();
    let observed_members = found.iter().map(|(_, rom)| rom.rom_name.clone()).collect();
    Ok(PlannedSet {
        fix: finish_set(path, &game_name, parent, members)?,
        observed_members,
    })
}

fn digest_for_member(archive_path: &Path, member_name: &str) -> Result<String, String> {
    let mut archive = zip::ZipArchive::new(File::open(archive_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut member = archive.by_name(member_name).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    member.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(hashes(&bytes).sha256)
}

fn finish_set(
    path: &Path,
    game_name: &str,
    parent: Option<String>,
    members: Vec<MameMemberFix>,
) -> Result<MameSetFix, String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let correct_path = path.with_file_name(format!("{}{}", game_name, extension));
    let collision = correct_path != path && correct_path.exists();
    let status = if collision {
        MameFixStatus::Collision
    } else if members.is_empty() && correct_path == path {
        MameFixStatus::AlreadyCorrect
    } else {
        MameFixStatus::Safe
    };
    Ok(MameSetFix {
        current_path: path.to_path_buf(),
        correct_path,
        set_name: Some(game_name.to_string()),
        parent,
        status,
        members,
        missing_members: Vec::new(),
        reason: collision.then(|| "destination already exists; no overwrite".into()),
    })
}

/// Applies the three MAME storage contracts after individual files have been
/// identified by checksum. This is only a completeness projection: it never
/// moves a member between sets.
fn apply_layout_completeness(
    dat: &ParsedDat,
    mode: MameCollectionMode,
    planned: &mut [PlannedSet],
) {
    let observed = planned
        .iter()
        .filter_map(|set| {
            set.fix
                .set_name
                .as_ref()
                .map(|name| (name.clone(), set.observed_members.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    for set in planned {
        let Some(name) = set.fix.set_name.as_deref() else {
            continue;
        };
        if matches!(
            set.fix.status,
            MameFixStatus::Unknown | MameFixStatus::Collision
        ) {
            continue;
        }
        let missing = layout_missing_members(dat, name, mode, &observed);
        if !missing.is_empty() {
            set.fix.status = MameFixStatus::MissingData;
            set.fix.reason = Some(format!(
                "required DAT members are not present for the selected {mode:?} layout"
            ));
            set.fix.missing_members = missing;
        }
    }
}

fn layout_missing_members(
    dat: &ParsedDat,
    game_name: &str,
    mode: MameCollectionMode,
    observed: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let Some(game) = dat.games.iter().find(|game| game.name == game_name) else {
        return vec![game_name.to_string()];
    };
    let mut required = game
        .roms
        .iter()
        .map(|rom| rom.name.clone())
        .collect::<BTreeSet<_>>();
    let own = observed.get(game_name).cloned().unwrap_or_default();
    let parent = game
        .clone_of
        .as_deref()
        .and_then(|name| observed.get(name))
        .cloned()
        .unwrap_or_default();
    let available = match mode {
        MameCollectionMode::Merged | MameCollectionMode::Split => {
            own.union(&parent).cloned().collect()
        }
        MameCollectionMode::NonMerged => {
            if let Some(parent_name) = game.clone_of.as_deref()
                && let Some(parent_game) = dat
                    .games
                    .iter()
                    .find(|candidate| candidate.name == parent_name)
            {
                required.extend(parent_game.roms.iter().map(|rom| rom.name.clone()));
            }
            own
        }
        MameCollectionMode::NotSure => BTreeSet::new(),
    };
    required
        .into_iter()
        .filter(|name| !available.contains(name))
        .collect()
}

fn unresolved(path: &Path, destination: PathBuf, reason: &str) -> MameSetFix {
    MameSetFix {
        current_path: path.to_path_buf(),
        correct_path: destination,
        set_name: None,
        parent: None,
        status: MameFixStatus::NeedsAttention,
        members: Vec::new(),
        missing_members: Vec::new(),
        reason: Some(reason.into()),
    }
}

fn collect_files(
    root: &Path,
    current: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let mut entries = fs::read_dir(current)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            collect_files(root, &path, out)?;
        } else if meta.is_file() {
            out.push((
                path.strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/"),
                path,
            ));
        }
    }
    Ok(())
}

#[derive(Default)]
struct Hashes {
    crc: String,
    md5: String,
    sha1: String,
    sha256: String,
}

fn hashes(bytes: &[u8]) -> Hashes {
    let mut md5 = Md5::new();
    md5.update(bytes);
    let mut sha1 = Sha1::new();
    sha1.update(bytes);
    let mut sha256 = Sha256::new();
    sha256.update(bytes);
    Hashes {
        crc: Crc32::of(bytes),
        md5: encode_hex(&md5.finalize()),
        sha1: encode_hex(&sha1.finalize()),
        sha256: encode_hex(&sha256.finalize()),
    }
}

fn lookup<'a>(index: &'a DatIndex, hashes: &Hashes) -> Vec<&'a DatRomRef> {
    let mut result = Vec::new();
    for candidate in index.lookup_sha256(&hashes.sha256) {
        result.push(candidate);
    }
    if result.is_empty() {
        for candidate in index.lookup_md5(&hashes.md5) {
            result.push(candidate);
        }
    }
    if result.is_empty() {
        for candidate in index.lookup_sha1(&hashes.sha1) {
            result.push(candidate);
        }
    }
    if result.is_empty() {
        for candidate in index.lookup_crc32(&hashes.crc) {
            result.push(candidate);
        }
    }
    result
}

type GameMatches = (String, Option<String>, Vec<(String, DatRomRef)>);

fn choose_game(
    matches: &BTreeMap<String, Vec<(String, DatRomRef)>>,
    mode: MameCollectionMode,
) -> Option<GameMatches> {
    if matches.is_empty() {
        return None;
    }
    // A physical archive containing evidence from more than one logical set
    // is common in merged collections. It is not safe to infer its outer
    // identity from whichever set happens to have the most matching members
    // in that mode. Split and non-merged layouts may use the count as a
    // conservative tie-breaker because the selected set owns its members.
    let (game_name, found) = if mode == MameCollectionMode::Merged {
        if matches.len() != 1 {
            return None;
        }
        matches.iter().next()?
    } else {
        let (_, largest) = matches.iter().max_by_key(|(_, entries)| entries.len())?;
        if matches
            .values()
            .filter(|entries| entries.len() == largest.len())
            .count()
            != 1
        {
            return None;
        }
        matches.iter().max_by_key(|(_, entries)| entries.len())?
    };
    Some((
        game_name.clone(),
        found[0].1.clone_of.clone(),
        found.clone(),
    ))
}

/// Apply only `Safe` entries. ZIP rebuilds are written to a sibling temporary
/// file, reopened, and then published with no-clobber renames. A JSON journal
/// is written before the first mutation and retained until explicit undo.
pub fn apply_mame_normalisation(
    plan: &MameNormalisationPlan,
    journal_path: &Path,
) -> Result<usize, String> {
    let batch_id = format!(
        "mame-normalise-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    let mut journal = MameRepairJournal {
        version: 1,
        batch_id,
        entries: Vec::new(),
        state: MameRepairState::Applying,
    };
    let family_paths = plan
        .split_rebuilds
        .iter()
        .flat_map(|rebuild| [rebuild.parent_path.clone(), rebuild.clone_path.clone()])
        .collect::<BTreeSet<_>>();
    let mut staged_families = Vec::new();
    for rebuild in &plan.split_rebuilds {
        let parent_staged = rebuild.parent_path.with_extension("emuwiz-split.tmp.zip");
        let clone_staged = rebuild.clone_path.with_extension("emuwiz-split.tmp.zip");
        rebuild_split_zip(&parent_staged, &rebuild.parent_members)?;
        rebuild_split_zip(&clone_staged, &rebuild.clone_members)?;
        verify_rebuilt_members(&parent_staged, &rebuild.parent_members)?;
        verify_rebuilt_members(&clone_staged, &rebuild.clone_members)?;
        staged_families.push((rebuild, parent_staged, clone_staged));
    }
    for (rebuild, parent_staged, clone_staged) in &staged_families {
        let parent_backup = rebuild
            .parent_path
            .with_extension("emuwiz-normaliser.original.zip");
        let clone_backup = rebuild
            .clone_path
            .with_extension("emuwiz-normaliser.original.zip");
        if parent_backup.exists() || clone_backup.exists() {
            return Err("a previous MAME repair backup already exists".into());
        }
        let mut family_entries = Vec::new();
        for (original, published, backup, _) in [
            (
                &rebuild.parent_path,
                &rebuild.parent_target,
                &parent_backup,
                &parent_staged,
            ),
            (
                &rebuild.clone_path,
                &rebuild.clone_target,
                &clone_backup,
                &clone_staged,
            ),
        ] {
            let original_sha = sha256_file(original)?;
            journal.entries.push(MameRepairEntry {
                original: original.clone(),
                published: published.clone(),
                backup: Some(backup.clone()),
                original_sha256: original_sha,
                published_sha256: String::new(),
                members: Vec::new(),
            });
            write_journal(journal_path, &journal)?;
            family_entries.push((original, published, backup));
        }
        for (original, _, backup) in &family_entries {
            rename_noreplace(original, backup).map_err(|e| e.to_string())?;
        }
        for (index, (_, published, _)) in family_entries.iter().enumerate() {
            let staged = if index == 0 {
                parent_staged
            } else {
                clone_staged
            };
            rename_noreplace(staged, published).map_err(|e| e.to_string())?;
            let entry_index = journal.entries.len() - 2 + index;
            journal.entries[entry_index].published_sha256 = sha256_file(published)?;
            write_journal(journal_path, &journal)?;
        }
    }
    for set in plan.sets.iter().filter(|set| {
        set.status == MameFixStatus::Safe && !family_paths.contains(&set.current_path)
    }) {
        let original_sha = sha256_file(&set.current_path)?;
        let journal_index = journal.entries.len();
        journal.entries.push(MameRepairEntry {
            original: set.current_path.clone(),
            published: set.correct_path.clone(),
            backup: if is_zip(&set.current_path) && !set.members.is_empty() {
                Some(
                    set.current_path
                        .with_extension("emuwiz-normaliser.original.zip"),
                )
            } else {
                None
            },
            original_sha256: original_sha,
            published_sha256: String::new(),
            members: set.members.clone(),
        });
        write_journal(journal_path, &journal)?;
        let (published, backup) = if is_zip(&set.current_path) && !set.members.is_empty() {
            let staged = set.current_path.with_extension("emuwiz-normaliser.tmp.zip");
            rebuild_zip(&set.current_path, &staged, &set.members)?;
            verify_zip_members(&staged, &set.members)?;
            let backup = set
                .current_path
                .with_extension("emuwiz-normaliser.original.zip");
            rename_noreplace(&set.current_path, &backup).map_err(|e| e.to_string())?;
            rename_noreplace(&staged, &set.correct_path).map_err(|e| e.to_string())?;
            (set.correct_path.clone(), Some(backup))
        } else {
            if !set.members.is_empty() {
                rename_directory_members(&set.current_path, &set.members)?;
            }
            if set.current_path != set.correct_path {
                rename_noreplace(&set.current_path, &set.correct_path)
                    .map_err(|e| e.to_string())?;
            }
            (set.correct_path.clone(), None)
        };
        journal.entries[journal_index].published = published;
        journal.entries[journal_index].backup = backup;
        journal.entries[journal_index].published_sha256 =
            sha256_file(&journal.entries[journal_index].published)?;
        write_journal(journal_path, &journal)?;
    }
    journal.state = MameRepairState::Applied;
    write_journal(journal_path, &journal)?;
    Ok(journal.entries.len())
}

pub fn undo_mame_normalisation(journal_path: &Path) -> Result<usize, String> {
    let data = fs::read(journal_path).map_err(|e| e.to_string())?;
    let mut journal: MameRepairJournal =
        serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    if journal.state != MameRepairState::Applied {
        return Err("repair batch is not applied".into());
    }
    for entry in &journal.entries {
        if entry.backup.is_some() {
            let _ = fs::remove_file(&entry.published);
        }
    }
    for entry in journal.entries.iter().rev() {
        if let Some(backup) = &entry.backup {
            rename_noreplace(backup, &entry.original).map_err(|e| e.to_string())?;
        } else if entry.published != entry.original {
            rename_noreplace(&entry.published, &entry.original).map_err(|e| e.to_string())?;
        }
        if entry.backup.is_none() {
            for member in entry.members.iter().rev() {
                let source = entry.original.join(&member.correct);
                let destination = entry.original.join(&member.current);
                if source.exists() {
                    rename_noreplace(&source, &destination).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    journal.state = MameRepairState::Undone;
    write_journal(journal_path, &journal)?;
    Ok(journal.entries.len())
}

fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    Ok(encode_hex(&Sha256::digest(
        fs::read(path).map_err(|e| e.to_string())?,
    )))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn write_journal(path: &Path, journal: &MameRepairJournal) -> Result<(), String> {
    let parent = path.parent().ok_or("journal has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, journal).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

fn rebuild_zip(source: &Path, destination: &Path, fixes: &[MameMemberFix]) -> Result<(), String> {
    if destination.exists() {
        return Err("temporary archive already exists".into());
    }
    let input = File::open(source).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(input).map_err(|e| e.to_string())?;
    let output = File::create(destination).map_err(|e| e.to_string())?;
    let mut writer = zip::ZipWriter::new(output);
    let renames: BTreeMap<&str, &str> = fixes
        .iter()
        .map(|fix| (fix.current.as_str(), fix.correct.as_str()))
        .collect();
    for i in 0..archive.len() {
        let mut member = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = member.name().to_string();
        let target = renames.get(name.as_str()).copied().unwrap_or(name.as_str());
        if member.is_dir() {
            writer
                .add_directory(target, zip::write::SimpleFileOptions::default())
                .map_err(|e| e.to_string())?;
            continue;
        }
        writer
            .start_file(target, zip::write::SimpleFileOptions::default())
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut member, &mut writer).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())?;
    let check = zip::ZipArchive::new(File::open(destination).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if check.len() != archive.len() {
        return Err("rebuilt archive member count changed".into());
    }
    Ok(())
}

fn rebuild_split_zip(destination: &Path, members: &[MameRebuildMember]) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "temporary archive already exists: {}",
            destination.display()
        ));
    }
    let output = File::create(destination).map_err(|e| e.to_string())?;
    let mut writer = zip::ZipWriter::new(output);
    let mut names = BTreeSet::new();
    for member in members {
        if !names.insert(member.correct.as_str()) {
            return Err(format!("duplicate destination member: {}", member.correct));
        }
        let input = File::open(&member.source_path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(input).map_err(|e| e.to_string())?;
        let mut source = archive
            .by_name(&member.source_member)
            .map_err(|e| e.to_string())?;
        writer
            .start_file(&member.correct, zip::write::SimpleFileOptions::default())
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut source, &mut writer).map_err(|e| e.to_string())?;
    }
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn verify_rebuilt_members(path: &Path, members: &[MameRebuildMember]) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if archive.len() != members.len() {
        return Err("rebuilt Split archive member count changed".into());
    }
    for member in members {
        let mut actual = archive
            .by_name(&member.correct)
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        actual.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if hashes(&bytes).sha256 != member.sha256 {
            return Err(format!(
                "rebuilt Split member checksum changed: {}",
                member.correct
            ));
        }
    }
    Ok(())
}

fn verify_zip_members(path: &Path, fixes: &[MameMemberFix]) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    for fix in fixes {
        let mut member = archive.by_name(&fix.correct).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if hashes(&bytes).sha256 != fix.sha256 {
            return Err(format!("rebuilt member checksum changed: {}", fix.correct));
        }
    }
    Ok(())
}

fn rename_directory_members(root: &Path, fixes: &[MameMemberFix]) -> Result<(), String> {
    for fix in fixes {
        let source = root.join(&fix.current);
        let destination = root.join(&fix.correct);
        if destination.exists() {
            return Err(format!("member collision: {}", destination.display()));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        rename_noreplace(&source, &destination).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::model::{DatFormat, DatGameEntry, DatPackingPolicy, DatRomEntry, DatSource};
    use tempfile::tempdir;

    #[test]
    fn unknown_files_are_not_actionable() {
        let mut plan = MameNormalisationPlan {
            root: PathBuf::from("/tmp"),
            mode: MameCollectionMode::Split,
            dat_version: None,
            sets: vec![MameSetFix {
                current_path: PathBuf::from("x.bin"),
                correct_path: PathBuf::from("x.bin"),
                set_name: None,
                parent: None,
                status: MameFixStatus::Unknown,
                members: vec![],
                missing_members: vec![],
                reason: Some("unknown".into()),
            }],
            split_rebuilds: Vec::new(),
            summary: Default::default(),
        };
        assert_eq!(plan.sets[0].status, MameFixStatus::Unknown);
        plan.summary.total_sets = 1;
        assert_eq!(plan.summary.total_sets, 1);
    }

    #[test]
    fn disposable_zip_preview_apply_verify_undo_and_repeat() {
        let directory = tempdir().expect("fixture directory");
        let root = directory.path();
        let bad = root.join("WrongArchiveName.zip");
        let bytes = b"verified rom bytes";
        let digest = hashes(bytes);
        let mut writer =
            zip::ZipWriter::new(File::create(&bad).expect("zip").try_clone().expect("clone"));
        writer
            .start_file(
                "wrong_member_name.bin",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("member");
        writer.write_all(bytes).expect("bytes");
        writer.finish().expect("finish");
        let dat = ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: super::super::model::DatEcosystem::MAMEArcade,
                file_path: "fixture.dat".into(),
                name: Some("fixture".into()),
                description: None,
                version: Some("0.264".into()),
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: 1,
                rom_count: 1,
                parse_warnings: Vec::new(),
                packing_policy: DatPackingPolicy::Standard,
            },
            games: vec![DatGameEntry {
                name: "pacman".into(),
                roms: vec![DatRomEntry {
                    name: "good.bin".into(),
                    crc32: Some(digest.crc),
                    md5: Some(digest.md5),
                    sha1: Some(digest.sha1),
                    sha256: Some(digest.sha256),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let plan = plan_mame_normalisation(root, &dat, MameCollectionMode::Split).expect("plan");
        assert_eq!(plan.summary.safe, 1);
        assert_eq!(plan.sets[0].members[0].correct, "good.bin");
        let journal = root.join("batch.json");
        apply_mame_normalisation(&plan, &journal).expect("apply");
        let repaired = root.join("pacman.zip");
        assert!(repaired.is_file());
        let mut archive =
            zip::ZipArchive::new(File::open(&repaired).expect("repaired")).expect("valid");
        assert!(archive.by_name("good.bin").is_ok());
        undo_mame_normalisation(&journal).expect("undo");
        assert!(bad.is_file());
        assert!(!repaired.exists());
        let plan_again =
            plan_mame_normalisation(root, &dat, MameCollectionMode::Split).expect("second plan");
        apply_mame_normalisation(&plan_again, &journal).expect("second apply");
        assert!(repaired.is_file());
    }

    fn parent_clone_dat() -> ParsedDat {
        let source = DatSource {
            format: DatFormat::Logiqx,
            ecosystem: super::super::model::DatEcosystem::MAMEArcade,
            file_path: "fixture.dat".into(),
            name: Some("layout fixture".into()),
            description: None,
            version: Some("0.264".into()),
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: 2,
            rom_count: 3,
            parse_warnings: Vec::new(),
            packing_policy: DatPackingPolicy::Standard,
        };
        let rom = |name: &str| DatRomEntry {
            name: name.into(),
            ..Default::default()
        };
        ParsedDat {
            source,
            games: vec![
                DatGameEntry {
                    name: "parent".into(),
                    roms: vec![rom("shared.bin")],
                    ..Default::default()
                },
                DatGameEntry {
                    name: "clone".into(),
                    clone_of: Some("parent".into()),
                    roms: vec![rom("shared.bin"), rom("clone.bin")],
                    ..Default::default()
                },
            ],
        }
    }

    fn parent_clone_dat_with_hashes() -> ParsedDat {
        let mut dat = parent_clone_dat();
        let shared = hashes(b"shared bytes");
        let clone = hashes(b"clone bytes");
        dat.games[0].roms[0].crc32 = Some(shared.crc.clone());
        dat.games[0].roms[0].md5 = Some(shared.md5.clone());
        dat.games[0].roms[0].sha1 = Some(shared.sha1.clone());
        dat.games[0].roms[0].sha256 = Some(shared.sha256.clone());
        dat.games[1].roms[0] = dat.games[0].roms[0].clone();
        dat.games[1].roms[1].crc32 = Some(clone.crc);
        dat.games[1].roms[1].md5 = Some(clone.md5);
        dat.games[1].roms[1].sha1 = Some(clone.sha1);
        dat.games[1].roms[1].sha256 = Some(clone.sha256);
        dat
    }

    fn write_fixture_zip(path: &Path, members: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(File::create(path).expect("fixture zip"));
        for (name, bytes) in members {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("fixture member");
            writer.write_all(bytes).expect("fixture bytes");
        }
        writer.finish().expect("fixture finish");
    }

    #[test]
    fn parent_clone_completeness_matches_all_three_layouts() {
        let dat = parent_clone_dat();
        let merged = BTreeMap::from([
            (
                "parent".into(),
                BTreeSet::from(["shared.bin".into(), "clone.bin".into()]),
            ),
            ("clone".into(), BTreeSet::new()),
        ]);
        assert!(
            layout_missing_members(&dat, "clone", MameCollectionMode::Merged, &merged).is_empty()
        );

        let split = BTreeMap::from([
            ("parent".into(), BTreeSet::from(["shared.bin".into()])),
            ("clone".into(), BTreeSet::from(["clone.bin".into()])),
        ]);
        assert!(
            layout_missing_members(&dat, "clone", MameCollectionMode::Split, &split).is_empty()
        );

        let non_merged = BTreeMap::from([
            ("parent".into(), BTreeSet::from(["shared.bin".into()])),
            (
                "clone".into(),
                BTreeSet::from(["shared.bin".into(), "clone.bin".into()]),
            ),
        ]);
        assert!(
            layout_missing_members(&dat, "clone", MameCollectionMode::NonMerged, &non_merged)
                .is_empty()
        );
        let incomplete = BTreeMap::from([("clone".into(), BTreeSet::from(["clone.bin".into()]))]);
        assert_eq!(
            layout_missing_members(&dat, "clone", MameCollectionMode::NonMerged, &incomplete),
            vec!["shared.bin"]
        );
    }

    #[test]
    fn split_rebuild_moves_verified_members_and_undoes() {
        let directory = tempdir().expect("fixture directory");
        let root = directory.path();
        write_fixture_zip(
            &root.join("parent.zip"),
            &[("wrong_clone.bin", b"clone bytes")],
        );
        write_fixture_zip(
            &root.join("clone.zip"),
            &[("wrong_shared.bin", b"shared bytes")],
        );
        let before_parent = sha256_file(&root.join("parent.zip")).expect("parent hash");
        let before_clone = sha256_file(&root.join("clone.zip")).expect("clone hash");
        let dat = parent_clone_dat_with_hashes();
        let plan = plan_mame_normalisation(root, &dat, MameCollectionMode::Split).expect("plan");
        assert_eq!(plan.summary.split_rebuilds, 1);
        assert_eq!(plan.summary.moved_members, 2);
        assert_eq!(
            plan.split_rebuilds[0].parent_members[0].correct,
            "shared.bin"
        );
        assert_eq!(plan.split_rebuilds[0].clone_members[0].correct, "clone.bin");

        let journal = root.join("batch.json");
        apply_mame_normalisation(&plan, &journal).expect("apply");
        let mut parent = zip::ZipArchive::new(File::open(root.join("parent.zip")).expect("parent"))
            .expect("parent archive");
        let mut clone = zip::ZipArchive::new(File::open(root.join("clone.zip")).expect("clone"))
            .expect("clone archive");
        assert!(parent.by_name("shared.bin").is_ok());
        assert!(clone.by_name("clone.bin").is_ok());
        assert_ne!(
            sha256_file(&root.join("parent.zip")).expect("parent hash"),
            before_parent
        );
        assert_ne!(
            sha256_file(&root.join("clone.zip")).expect("clone hash"),
            before_clone
        );
        let fixed =
            plan_mame_normalisation(root, &dat, MameCollectionMode::Split).expect("fixed plan");
        assert_eq!(fixed.summary.split_rebuilds, 0);

        undo_mame_normalisation(&journal).expect("undo");
        assert_eq!(
            sha256_file(&root.join("parent.zip")).expect("parent restored"),
            before_parent
        );
        assert_eq!(
            sha256_file(&root.join("clone.zip")).expect("clone restored"),
            before_clone
        );
        let second =
            plan_mame_normalisation(root, &dat, MameCollectionMode::Split).expect("second plan");
        assert_eq!(second.summary.split_rebuilds, 1);
    }

    #[test]
    fn split_rebuild_blocks_destination_collision() {
        let directory = tempdir().expect("fixture directory");
        let root = directory.path();
        write_fixture_zip(
            &root.join("wrong-parent.zip"),
            &[("wrong_clone.bin", b"clone bytes")],
        );
        write_fixture_zip(
            &root.join("wrong-clone.zip"),
            &[("wrong_shared.bin", b"shared bytes")],
        );
        write_fixture_zip(&root.join("parent.zip"), &[("unrelated.bin", b"unrelated")]);
        let dat = parent_clone_dat_with_hashes();
        let plan = plan_mame_normalisation(root, &dat, MameCollectionMode::Split).expect("plan");
        assert_eq!(plan.summary.split_rebuilds, 0);
        assert_eq!(plan.summary.safe, 0);
    }

    #[test]
    fn layout_detector_distinguishes_merged_split_and_non_merged_samples() {
        let dat = parent_clone_dat();
        let make_sample = |files: &[(&str, &[&str])]| {
            let directory = tempdir().expect("sample directory");
            for (set, members) in files {
                let set_dir = directory.path().join(set);
                fs::create_dir_all(&set_dir).expect("set directory");
                for member in *members {
                    File::create(set_dir.join(member)).expect("member");
                }
            }
            let detected = detect_mame_collection_mode(directory.path(), &dat).expect("detect");
            (directory, detected)
        };

        let (merged_dir, merged) = make_sample(&[("parent", &["shared.bin", "clone.bin"])]);
        assert_eq!(merged, MameCollectionMode::Merged);
        drop(merged_dir);

        let (split_dir, split) =
            make_sample(&[("parent", &["shared.bin"]), ("clone", &["clone.bin"])]);
        assert_eq!(split, MameCollectionMode::Split);
        drop(split_dir);

        let (non_merged_dir, non_merged) = make_sample(&[
            ("parent", &["shared.bin"]),
            ("clone", &["shared.bin", "clone.bin"]),
        ]);
        assert_eq!(non_merged, MameCollectionMode::NonMerged);
        drop(non_merged_dir);
    }
}
