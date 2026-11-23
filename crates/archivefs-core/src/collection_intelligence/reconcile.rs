//! What became of each library record whose file is not where it was.
//!
//! Pure classification over facts gathered elsewhere: the records, the
//! current rows, EmuWiz's own applied moves, a listing of the library folders
//! and the link audit. Rules run strongest first and the first that applies
//! wins. A name on its own never ties a record to a file, and nothing below
//! the confirmed tiers claims two files hold the same bytes.

use super::inventory::{Entry, EntryKind, Inventory};
use super::model::{EvidenceTier, LinkFinding, LinkGrade, RecordFinding, RecordSet};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_JOURNAL_HOPS: usize = 8;
/// Below this size an identical byte count is too common to mean anything.
const MIN_SIZE_FOR_SIZE_MATCH: u64 = 4096;
/// A timestamp shared by more files than this is a set-wide constant.
const DISTINCTIVE_MTIME_MAX_FILES: u32 = 5;
const ARCADE_SET_KIND: &str = "arcade_set_directory";
const DIRECTORY_KINDS: [&str; 2] = [ARCADE_SET_KIND, "scummvm_directory"];
const ARCHIVE_EXTENSIONS: [&str; 5] = ["zip", "7z", "rar", "lha", "lzh"];
/// Artwork, documents and saves share names with games and are never
/// candidates for one.
const NOT_GAME_EXTENSIONS: [&str; 30] = [
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "svg", "pdf", "xml", "txt", "nfo", "json", "dat",
    "sav", "srm", "state", "db", "ini", "cfg", "log", "html", "md", "mp4", "mkv", "webm", "mp3",
    "ogg", "wav", "sfv", "torrent",
];
const NOT_GAME_FOLDERS: [&str; 9] = [
    "/assets/",
    "/media/",
    "/resources/",
    "/.romm/",
    "/saves/",
    "/states/",
    "/screenshots/",
    "/manuals/",
    "/covers/",
];

/// A record with no file at its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldRecord {
    pub set: RecordSet,
    pub archive_id: i64,
    pub path: PathBuf,
    pub archive_kind: String,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub first_seen_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub flagged_missing_at: Option<String>,
    pub platform: Option<String>,
    /// Device of the record's library folder today, when it is reachable.
    pub source_device: Option<u64>,
    /// The record's library folder.
    pub source_root: Option<PathBuf>,
    /// The library's id for that folder.
    pub source_folder_id: Option<i64>,
}

/// A library row whose file exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentRow {
    pub archive_id: i64,
    pub path: PathBuf,
    pub archive_kind: String,
    pub first_seen_at: String,
    /// The library's id for the row's library folder.
    pub source_folder_id: i64,
}

/// One library scan that finished, and what it is known to have covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRun {
    /// Seconds since the epoch.
    pub started: i64,
    pub finished: i64,
    /// The scan ran to its end.
    pub completed: bool,
    /// Per library folder id, whether the scan recorded covering it completely.
    /// `None` for scans from before the library recorded coverage.
    pub coverage: Option<HashMap<i64, bool>>,
}

impl ScanRun {
    /// True when this scan is known to have walked the whole library folder
    /// `target`. A scan that did not complete covered nothing for certain. With
    /// no coverage on record, the scan is known to have walked only the folder
    /// of the record it saw (`seen_in`).
    fn covered(&self, seen_in: Option<i64>, target: Option<i64>) -> bool {
        let Some(target) = target else {
            return false;
        };
        if !self.completed {
            return false;
        }
        match &self.coverage {
            Some(coverage) => coverage.get(&target).copied().unwrap_or(false),
            None => seen_in == Some(target),
        }
    }
}

/// One applied move from EmuWiz's own rename journals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalMove {
    pub from: PathBuf,
    pub to: PathBuf,
    pub inode: u64,
    pub device: u64,
    pub size: u64,
    /// The moved file's modification time, whole seconds.
    pub modified: i64,
    /// When the move was applied, when the journal says.
    pub applied_at: Option<i64>,
}

/// Hashes somebody else recorded for a path while the file was still there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordedHashes {
    pub source: String,
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentVerdict {
    Match,
    Differs,
    NotComparable,
}

/// Compares a file's contents today with hashes recorded for its old path.
pub trait ContentVerifier {
    fn compare(&mut self, target: &Path, recorded: &RecordedHashes) -> ContentVerdict;
}

#[derive(Debug, Clone, Default)]
pub struct ReconcileInputs {
    pub old: Vec<OldRecord>,
    pub current: Vec<CurrentRow>,
    pub moves: Vec<JournalMove>,
    pub recorded_hashes: HashMap<PathBuf, RecordedHashes>,
    /// The library's finished scans.
    pub scan_runs: Vec<ScanRun>,
}

fn lower_bytes(value: &OsStr) -> Vec<u8> {
    value.as_bytes().to_ascii_lowercase()
}

fn extension(path: &Path) -> String {
    let name = path.file_name().map(OsStr::as_bytes).unwrap_or_default();
    match name.iter().rposition(|byte| *byte == b'.') {
        Some(dot) => String::from_utf8_lossy(&name[dot + 1..]).to_ascii_lowercase(),
        None => String::new(),
    }
}

fn stem(path: &Path) -> Vec<u8> {
    let name = path.file_name().map(OsStr::as_bytes).unwrap_or_default();
    match name.iter().rposition(|byte| *byte == b'.') {
        Some(dot) => name[..dot].to_ascii_lowercase(),
        None => name.to_ascii_lowercase(),
    }
}

/// False for artwork, documents, saves and anything in an assets folder.
pub fn looks_like_game_file(path: &Path) -> bool {
    if NOT_GAME_EXTENSIONS.contains(&extension(path).as_str()) {
        return false;
    }
    let text = path.as_os_str().as_bytes();
    !NOT_GAME_FOLDERS.iter().any(|folder| {
        text.windows(folder.len())
            .any(|window| window == folder.as_bytes())
    })
}

/// `YYYY-MM-DDTHH:MM:SS…` (UTC) as seconds since the epoch.
pub(crate) fn unix_from_iso(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

struct Index<'a> {
    inventory: &'a Inventory,
    by_name_size: HashMap<(&'a OsStr, u64), Vec<&'a Entry>>,
    by_inode: HashMap<u64, Vec<&'a Entry>>,
    by_size_ext: HashMap<(u64, String), Vec<&'a Entry>>,
    by_size_mtime: HashMap<(u64, i64), Vec<&'a Entry>>,
    files_by_stem: HashMap<Vec<u8>, Vec<&'a Entry>>,
    folders_by_name: HashMap<Vec<u8>, Vec<&'a Entry>>,
    mtime_count: HashMap<i64, u32>,
}

impl<'a> Index<'a> {
    fn new(inventory: &'a Inventory) -> Self {
        let mut index = Self {
            inventory,
            by_name_size: HashMap::new(),
            by_inode: HashMap::new(),
            by_size_ext: HashMap::new(),
            by_size_mtime: HashMap::new(),
            files_by_stem: HashMap::new(),
            folders_by_name: HashMap::new(),
            mtime_count: HashMap::new(),
        };
        for entry in &inventory.entries {
            match entry.kind {
                EntryKind::File => {
                    index.by_inode.entry(entry.inode).or_default().push(entry);
                    if !looks_like_game_file(&entry.path) {
                        continue;
                    }
                    let Some(name) = entry.path.file_name() else {
                        continue;
                    };
                    index
                        .by_name_size
                        .entry((name, entry.size))
                        .or_default()
                        .push(entry);
                    index
                        .by_size_ext
                        .entry((entry.size, extension(&entry.path)))
                        .or_default()
                        .push(entry);
                    index
                        .by_size_mtime
                        .entry((entry.size, entry.mtime))
                        .or_default()
                        .push(entry);
                    index
                        .files_by_stem
                        .entry(stem(&entry.path))
                        .or_default()
                        .push(entry);
                    *index.mtime_count.entry(entry.mtime).or_default() += 1;
                }
                EntryKind::Directory => {
                    if let Some(name) = entry.path.file_name() {
                        index
                            .folders_by_name
                            .entry(lower_bytes(name))
                            .or_default()
                            .push(entry);
                    }
                }
                EntryKind::Link => {}
            }
        }
        index
    }

    fn file(&self, path: &Path) -> Option<&'a Entry> {
        self.inventory
            .get(path)
            .filter(|entry| entry.kind == EntryKind::File)
    }
}

/// The file a finding names must still be exactly what the listing saw.
fn unchanged_on_disk(entry: &Entry) -> bool {
    fs::symlink_metadata(&entry.path).is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.len() == entry.size
            && metadata.mtime() == entry.mtime
            && metadata.ino() == entry.inode
    })
}

/// True when the record's path, below its library folder, leads through a
/// symbolic link. The library scan never follows links, so whatever is found
/// that way is not what the scan recorded at that path.
fn reached_through_link(record: &OldRecord) -> bool {
    let Some(root) = &record.source_root else {
        return false;
    };
    let Ok(relative) = record.path.strip_prefix(root) else {
        return false;
    };
    match (fs::canonicalize(root), fs::canonicalize(&record.path)) {
        (Ok(real_root), Ok(real)) => real != real_root.join(relative),
        _ => false,
    }
}

struct Outcome {
    tier: EvidenceTier,
    target: Option<PathBuf>,
    evidence: Vec<String>,
}

/// Classifies every record. Findings come back ordered by set, then id.
pub fn classify_records(
    inputs: &ReconcileInputs,
    inventory: &Inventory,
    links: &[LinkFinding],
    mut verifier: Option<&mut dyn ContentVerifier>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Vec<RecordFinding> {
    let index = Index::new(inventory);
    let current_by_path: HashMap<&Path, &CurrentRow> = inputs
        .current
        .iter()
        .map(|row| (row.path.as_path(), row))
        .collect();
    let arcade_sets: HashSet<&Path> = inputs
        .current
        .iter()
        .filter(|row| row.archive_kind == ARCADE_SET_KIND)
        .map(|row| row.path.as_path())
        .collect();
    let mut moves: HashMap<&Path, Vec<&JournalMove>> = HashMap::new();
    for item in &inputs.moves {
        moves.entry(item.from.as_path()).or_default().push(item);
    }
    let mut links_by_name: HashMap<&OsStr, Vec<&LinkFinding>> = HashMap::new();
    for link in links {
        if link.grade != LinkGrade::Resolves
            && let Some(name) = link.link.file_name()
        {
            links_by_name.entry(name).or_default().push(link);
        }
    }

    let total = inputs.old.len() as u64;
    let mut findings = Vec::with_capacity(inputs.old.len());
    for (done, record) in inputs.old.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if done % 2000 == 0 {
            progress(done as u64, total);
        }
        let mut outcome = classify_one(
            record,
            &index,
            &current_by_path,
            &arcade_sets,
            &moves,
            &inputs.scan_runs,
        );

        // A finding may only name a file that is still what the listing saw.
        if outcome.tier.tied_to_file()
            && let Some(target) = &outcome.target
            && let Some(entry) = index.file(target)
            && !unchanged_on_disk(entry)
        {
            outcome = Outcome {
                tier: EvidenceTier::NoTrace,
                target: None,
                evidence: vec![format!(
                    "{} changed while it was being examined; list the folders again",
                    target.display()
                )],
            };
        }

        if outcome.tier == EvidenceTier::ConfirmedObject
            && let Some(recorded) = inputs.recorded_hashes.get(&record.path)
            && let Some(target) = &outcome.target
            && let Some(verifier) = verifier.as_deref_mut()
        {
            match verifier.compare(target, recorded) {
                ContentVerdict::Match => {
                    outcome.tier = EvidenceTier::ConfirmedContent;
                    outcome.evidence.push(format!(
                        "the file's contents today match the hashes {} recorded for the old path",
                        recorded.source
                    ));
                }
                ContentVerdict::Differs => {
                    // The file object matches but its contents do not match
                    // what was recorded for the old path. That is a conflict,
                    // not a confirmation.
                    outcome.tier = EvidenceTier::MetadataMatch;
                    outcome.evidence.insert(
                        0,
                        format!(
                            "WARNING: the file's contents today differ from the hashes {} recorded for the old path",
                            recorded.source
                        ),
                    );
                }
                ContentVerdict::NotComparable => {}
            }
        }

        // What the broken links say about records that found no file.
        if matches!(
            outcome.tier,
            EvidenceTier::SizeOnlyCandidate | EvidenceTier::NameOnlyLead | EvidenceTier::NoTrace
        ) && let Some(named) = record
            .path
            .file_name()
            .and_then(|name| links_by_name.get(name))
        {
            let mut identified: Vec<&LinkFinding> = named
                .iter()
                .copied()
                .filter(|link| link.grade == LinkGrade::TargetIdentified && link.found.is_some())
                .collect();
            identified.sort_by(|a, b| a.found.cmp(&b.found));
            identified.dedup_by(|a, b| a.found == b.found);
            match identified.as_slice() {
                [link] => {
                    let found = link.found.as_ref().expect("checked above");
                    let same_size = fs::symlink_metadata(found).is_ok_and(|metadata| {
                        metadata.is_file() && Some(metadata.len()) == record.size
                    });
                    if same_size {
                        outcome.tier = EvidenceTier::NameAndSizeViaLink;
                        outcome.target = Some(found.clone());
                        outcome.evidence.insert(
                            0,
                            format!(
                                "a broken link named like this record is at {}; a file older than that link is at the same relative path, {}, and has the record's size",
                                link.link.display(),
                                found.display()
                            ),
                        );
                    } else {
                        outcome.evidence.push(format!(
                            "a broken link named like this record leads to {}, whose size differs from the record",
                            found.display()
                        ));
                    }
                }
                [] => {
                    if outcome.tier == EvidenceTier::NoTrace
                        && let Some(link) = named
                            .iter()
                            .find(|link| link.grade == LinkGrade::Unresolved)
                    {
                        outcome.tier = EvidenceTier::BrokenLinkOnly;
                        outcome.evidence.insert(
                            0,
                            format!(
                                "only a broken link remains, at {}, pointing at {}",
                                link.link.display(),
                                link.resolved_target.display()
                            ),
                        );
                    }
                }
                several => outcome.evidence.push(format!(
                    "{} broken links named like this record lead to different files; none is singled out",
                    several.len()
                )),
            }
        }
        let target_archive_id = outcome
            .target
            .as_deref()
            .and_then(|target| current_by_path.get(target))
            .map(|row| row.archive_id);
        findings.push(RecordFinding {
            set: record.set,
            archive_id: record.archive_id,
            old_path: record.path.clone(),
            platform: record.platform.clone(),
            archive_kind: record.archive_kind.clone(),
            size_bytes: record.size,
            modified_unix_seconds: record.mtime,
            first_seen_at: record.first_seen_at.clone(),
            last_seen_at: record.last_seen_at.clone(),
            flagged_missing_at: record.flagged_missing_at.clone(),
            tier: outcome.tier,
            class: outcome.tier.class(),
            code: outcome.tier.code(),
            target: outcome.target,
            target_archive_id,
            evidence: outcome.evidence,
        });
    }
    progress(findings.len() as u64, total);
    demote_strong_records_that_coexisted(&mut findings, &inputs.scan_runs);
    findings.sort_by(|a, b| {
        (a.set, a.archive_id, &a.old_path).cmp(&(b.set, b.archive_id, &b.old_path))
    });
    findings
}

/// The time a record was in the library: from its first to its last sighting,
/// each widened to the whole scan that contains it. A sighting that cannot be
/// read leaves that end open, which can only make two records overlap.
fn library_span(finding: &RecordFinding, scan_runs: &[ScanRun]) -> (i64, i64) {
    let containing = |seen: i64| {
        scan_runs
            .iter()
            .filter(move |run| run.started <= seen && seen <= run.finished)
    };
    let first = finding.first_seen_at.as_deref().and_then(unix_from_iso);
    let last = finding.last_seen_at.as_deref().and_then(unix_from_iso);
    (
        first.map_or(i64::MIN, |seen| {
            containing(seen)
                .map(|run| run.started)
                .min()
                .unwrap_or(seen)
        }),
        last.map_or(i64::MAX, |seen| {
            containing(seen)
                .map(|run| run.finished)
                .max()
                .unwrap_or(seen)
        }),
    )
}

/// Two records are not both one file, unless they were one file seen at two
/// paths (a hard link, or a folder that was visible twice). When several
/// records are graded "same file object" for the same file and the library's
/// sightings do not show them there at different times, at most one of them
/// is the file found now, and nothing recorded says which. None of them keeps
/// the strong tier. Records that never overlapped are one file moved more
/// than once, and are left as they are.
fn demote_strong_records_that_coexisted(findings: &mut [RecordFinding], scan_runs: &[ScanRun]) {
    let mut by_file: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (index, finding) in findings.iter().enumerate() {
        if finding.tier == EvidenceTier::SameFileObject
            && let Some(target) = &finding.target
        {
            by_file.entry(target.clone()).or_default().push(index);
        }
    }
    for group in by_file.values().filter(|group| group.len() > 1) {
        let spans: Vec<(i64, i64)> = group
            .iter()
            .map(|&index| library_span(&findings[index], scan_runs))
            .collect();
        for (position, &index) in group.iter().enumerate() {
            let (start, end) = spans[position];
            let together = spans
                .iter()
                .enumerate()
                .filter(|(other, (other_start, other_end))| {
                    *other != position && start <= *other_end && *other_start <= end
                })
                .count();
            if together > 0 {
                let finding = &mut findings[index];
                finding.tier = EvidenceTier::MetadataMatch;
                finding.class = finding.tier.class();
                finding.code = finding.tier.code();
                finding.evidence.insert(
                    0,
                    format!(
                        "{together} other record(s) match this file in the same way, and the library's sightings do not show that they were there at different times; unless they were one file seen at more than one path, at most one of them is this file, and nothing recorded says which"
                    ),
                );
            }
        }
    }
}

fn classify_one(
    record: &OldRecord,
    index: &Index<'_>,
    current_by_path: &HashMap<&Path, &CurrentRow>,
    arcade_sets: &HashSet<&Path>,
    moves: &HashMap<&Path, Vec<&JournalMove>>,
    scan_runs: &[ScanRun],
) -> Outcome {
    let mut evidence = Vec::new();
    let is_directory_kind = DIRECTORY_KINDS.contains(&record.archive_kind.as_str());
    let ext = extension(&record.path);
    let name: OsString = record
        .path
        .file_name()
        .map(OsStr::to_os_string)
        .unwrap_or_default();

    // 0. Is the recorded file at the recorded path right now? Only a regular
    // file (or a folder, for a game that is a folder) counts, and only when the
    // path does not lead through a link: the library scan never follows links,
    // so a file reached through one is not the file the scan recorded there.
    if let Ok(metadata) = fs::symlink_metadata(&record.path) {
        let right_kind = if is_directory_kind {
            metadata.is_dir()
        } else {
            metadata.is_file()
        };
        let through_link = right_kind && reached_through_link(record);
        if right_kind && !through_link {
            let part_of_set = record
                .path
                .parent()
                .is_some_and(|parent| arcade_sets.contains(parent));
            evidence.push(format!(
                "on disk now: size {}, modified {}; recorded size {:?}, modified {:?}",
                metadata.len(),
                metadata.mtime(),
                record.size,
                record.mtime
            ));
            if part_of_set {
                evidence.push(
                    "its folder is now one arcade game in the library, so this file is no longer listed on its own".into(),
                );
            }
            return Outcome {
                tier: EvidenceTier::PresentAtRecordedPath,
                target: Some(record.path.clone()),
                evidence,
            };
        }
        evidence.push(if through_link {
            "the recorded place is now reached through a link, which the library scan does not follow; what is there may be a copy kept elsewhere".into()
        } else if metadata.file_type().is_symlink() {
            "a link now sits at the recorded place; it is not the file the library recorded".into()
        } else {
            "something else now sits at the recorded place".to_string()
        });
    }

    // 1. EmuWiz's own rename journal. Rename history alone proves nothing: a
    // journal entry from this path may be about another file that had the path
    // earlier or later. The entry that concerns this record is the first move
    // applied after the record was last seen, and it must have moved a file of
    // the record's size and date.
    if let Some(entries) = moves.get(record.path.as_path()) {
        let last_seen = record.last_seen_at.as_deref().and_then(unix_from_iso);
        let first = last_seen.and_then(|seen| {
            entries
                .iter()
                .copied()
                .filter(|m| m.applied_at.is_some_and(|applied| applied >= seen))
                .min_by_key(|m| (m.applied_at, m.to.as_path()))
        });
        let about_this_record =
            first.filter(|m| record.size == Some(m.size) && record.mtime == Some(m.modified));
        match about_this_record {
            None => evidence.push(
                if last_seen.is_none() {
                    "EmuWiz's rename history has a move from this path, but when this record was last seen cannot be read, so the move cannot be tied to it"
                } else if entries.iter().all(|m| m.applied_at.is_none()) {
                    "EmuWiz's rename history has a move from this path, but it does not say when the move was applied, so the move cannot be tied to this record"
                } else if first.is_none() {
                    "EmuWiz's rename history has a move from this path, applied before this record was last seen, so it moved an earlier file at this path"
                } else {
                    "EmuWiz's rename history has a move from this path after this record was last seen, but of a file with another size or date, so it was about another file"
                }
                .to_string(),
            ),
            Some(identity) => {
                let moved_at = identity.applied_at.unwrap_or(i64::MAX);
                // Later moves are followed only while they move the same inode.
                let mut at: &Path = &identity.to;
                let mut applied = moved_at;
                let mut hops = 1;
                let mut seen_paths: HashSet<&Path> = HashSet::from([record.path.as_path()]);
                while hops < MAX_JOURNAL_HOPS && seen_paths.insert(at) {
                    let next = moves.get(at).and_then(|entries| {
                        entries
                            .iter()
                            .copied()
                            .filter(|m| {
                                m.inode == identity.inode
                                    && m.size == identity.size
                                    && m.applied_at.is_some_and(|later| later >= applied)
                            })
                            .min_by_key(|m| (m.applied_at, m.to.as_path()))
                    });
                    let Some(next) = next else { break };
                    applied = next.applied_at.unwrap_or(applied);
                    at = &next.to;
                    hops += 1;
                }
                evidence.push(format!(
                    "EmuWiz's rename history records {hops} move(s) of this file (inode {}, {} bytes)",
                    identity.inode, identity.size
                ));
                // The very file the journal moved: same inode, size and date,
                // on the disk the record's library folder is on. The device
                // number recorded then is not compared: it changes across boots.
                let object = |entry: &Entry| {
                    entry.inode == identity.inode
                        && entry.size == identity.size
                        && entry.mtime == identity.modified
                        && record.source_device == Some(entry.device)
                };
                let at_destination = index.file(at);
                let found = at_destination.filter(|entry| object(entry)).or_else(|| {
                    let elsewhere: Vec<&Entry> = index
                        .by_inode
                        .get(&identity.inode)
                        .map(|entries| {
                            entries
                                .iter()
                                .copied()
                                .filter(|entry| object(entry))
                                .collect()
                        })
                        .unwrap_or_default();
                    match elsewhere.as_slice() {
                        [entry] => Some(*entry),
                        _ => None,
                    }
                });
                if let Some(entry) = found {
                    // An inode number is reused after a file is deleted. A file
                    // created after the move cannot be the file that was moved.
                    match entry.born.map(|born| born <= moved_at) {
                        Some(true) => {
                            if entry.path != at {
                                evidence.push("the same file was found under a later name".into());
                            }
                            if entry.device != identity.device {
                                evidence.push(format!(
                                    "the device number differs ({} then, {} now): the disk was renumbered between boots; inode, size, date and creation time agree",
                                    identity.device, entry.device
                                ));
                            }
                            return Outcome {
                                tier: EvidenceTier::ConfirmedObject,
                                target: Some(entry.path.clone()),
                                evidence,
                            };
                        }
                        Some(false) => evidence.push(
                            "a file with the same inode number, size and date is there, but it was created after the move, so the inode number was reused".into(),
                        ),
                        None => {
                            evidence.push(
                                "a file with the same inode number, size and date is there, but this filesystem does not record when a file was created, so a reused inode number cannot be ruled out".into(),
                            );
                            return Outcome {
                                tier: EvidenceTier::MetadataMatch,
                                target: Some(entry.path.clone()),
                                evidence,
                            };
                        }
                    }
                } else {
                    match at_destination {
                        Some(entry) if entry.size == identity.size => {
                            evidence.push(
                                "the place EmuWiz moved it to holds a file of the same size that is not the same file object".into(),
                            );
                            return Outcome {
                                tier: EvidenceTier::SizeOnlyCandidate,
                                target: Some(entry.path.clone()),
                                evidence,
                            };
                        }
                        Some(entry) => {
                            evidence.push(
                                "the place EmuWiz moved it to holds a file of a different size".into(),
                            );
                            return Outcome {
                                tier: EvidenceTier::NameOnlyLead,
                                target: Some(entry.path.clone()),
                                evidence,
                            };
                        }
                        None => evidence.push("the place EmuWiz moved it to is empty now".into()),
                    }
                }
            }
        }
    }

    // 2. One file with the same name and size.
    if !is_directory_kind && let Some(size) = record.size {
        let candidates = index
            .by_name_size
            .get(&(name.as_os_str(), size))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        match candidates {
            [] => {}
            [entry] => {
                let same_mtime = Some(entry.mtime) == record.mtime;
                let shared = index.mtime_count.get(&entry.mtime).copied().unwrap_or(0);
                evidence.push(format!(
                    "one file has this name and size; its date {} ({})",
                    if same_mtime { "is the same" } else { "differs" },
                    if shared <= DISTINCTIVE_MTIME_MAX_FILES {
                        "a distinctive date".to_string()
                    } else {
                        format!("a date shared by {shared} files")
                    }
                ));
                let last_seen = record.last_seen_at.as_deref().and_then(unix_from_iso);
                let target_row = current_by_path.get(entry.path.as_path());
                let target_first_seen =
                    target_row.and_then(|row| unix_from_iso(&row.first_seen_at));
                let older =
                    matches!((entry.born, last_seen), (Some(born), Some(last)) if born < last);
                // "It existed then, yet the library did not know it" shows a
                // rename only if the scan that last saw the old path would have
                // found this file had it been a separate file: that scan must
                // have completed and walked this file's library folder, and the
                // file's row must first appear after it. Stamps are whole
                // seconds, so when two scans share the sighting's second the
                // later one is taken, which can only make this harder to meet.
                let that_scan = last_seen.and_then(|seen| {
                    scan_runs
                        .iter()
                        .filter(|run| run.started <= seen && seen <= run.finished)
                        .max_by_key(|run| (run.started, run.finished))
                });
                let covered = that_scan.is_some_and(|run| {
                    run.covered(
                        record.source_folder_id,
                        target_row.map(|row| row.source_folder_id),
                    )
                });
                let never_together = covered
                    && matches!(
                        (target_first_seen, that_scan),
                        (Some(first), Some(run)) if first > run.finished
                    );
                let same_volume = record.source_device == Some(entry.device);
                let tier = if same_mtime && older && never_together && same_volume {
                    evidence.push(
                        "its inode was created before the old path was last seen, on the same disk; the scan that last saw the old path completed over this library folder without finding this file, and its row first appears in a later scan".into(),
                    );
                    EvidenceTier::SameFileObject
                } else if same_mtime {
                    if older && same_volume {
                        evidence.push(if covered {
                            "this file was already in the library when the old path was last seen, so the two existed side by side".into()
                        } else {
                            "its inode is older than the old path's last sighting, but the scan that saw the old path is not known to have completed over this file's library folder, so the two may have existed side by side".to_string()
                        });
                    }
                    EvidenceTier::MetadataMatch
                } else {
                    EvidenceTier::NameAndSize
                };
                return Outcome {
                    tier,
                    target: Some(entry.path.clone()),
                    evidence,
                };
            }
            several => {
                // Several files qualify, so none of them is named as "the" file.
                evidence.push(format!(
                    "{} files have this name and size: {}",
                    several.len(),
                    several
                        .iter()
                        .take(3)
                        .map(|entry| entry.path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
                return Outcome {
                    tier: EvidenceTier::NameAndSize,
                    target: None,
                    evidence,
                };
            }
        }

        // 2b. One file of identical byte size and type under another name.
        if size > MIN_SIZE_FOR_SIZE_MATCH {
            let twins: Vec<&&Entry> = index
                .by_size_ext
                .get(&(size, ext.clone()))
                .map(|entries| entries.iter().filter(|e| e.path != record.path).collect())
                .unwrap_or_default();
            if let [entry] = twins.as_slice() {
                evidence.push(format!(
                    "exactly one file has this exact size and type under another name (date {}); nothing shows it is the same game",
                    if Some(entry.mtime) == record.mtime { "equal" } else { "differs" }
                ));
                return Outcome {
                    tier: EvidenceTier::SizeOnlyCandidate,
                    target: Some(entry.path.clone()),
                    evidence,
                };
            }
        }
    }

    // 3. Something shares the base name. Reported, never accepted.
    if !is_directory_kind {
        let same_stem: Vec<&&Entry> = index
            .files_by_stem
            .get(&stem(&record.path))
            .map(|entries| entries.iter().filter(|e| e.path != record.path).collect())
            .unwrap_or_default();
        let lead = if ARCHIVE_EXTENSIONS.contains(&ext.as_str()) {
            let folders = index
                .folders_by_name
                .get(&stem(&record.path))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let archives: Vec<&&&Entry> = same_stem
                .iter()
                .filter(|e| ARCHIVE_EXTENSIONS.contains(&extension(&e.path).as_str()))
                .collect();
            archives
                .first()
                .map(|e| &e.path)
                .or_else(|| same_stem.first().map(|e| &e.path))
                .or_else(|| folders.first().map(|e| &e.path))
        } else {
            let other: Vec<&&&Entry> = same_stem
                .iter()
                .filter(|e| !ARCHIVE_EXTENSIONS.contains(&extension(&e.path).as_str()))
                .collect();
            let same_ext = other.iter().find(|e| extension(&e.path) == ext);
            same_ext
                .or(other.first())
                .map(|e| &e.path)
                .or_else(|| same_stem.first().map(|e| &e.path))
        };
        if let Some(path) = lead {
            evidence.push(
                "something shares the base name with a different size or type; a name alone is not evidence".into(),
            );
            return Outcome {
                tier: EvidenceTier::NameOnlyLead,
                target: Some(path.clone()),
                evidence,
            };
        }

        // 4. Renamed, with a size and date nothing else has.
        if let (Some(size), Some(mtime)) = (record.size, record.mtime)
            && size > MIN_SIZE_FOR_SIZE_MATCH
            && index.mtime_count.get(&mtime).copied().unwrap_or(0) <= DISTINCTIVE_MTIME_MAX_FILES
        {
            let twins: Vec<&&Entry> = index
                .by_size_mtime
                .get(&(size, mtime))
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|e| extension(&e.path) == ext && e.path != record.path)
                        .collect()
                })
                .unwrap_or_default();
            if let [entry] = twins.as_slice() {
                evidence.push(
                    "exactly one file has this size, this distinctive date and this type under another name".into(),
                );
                return Outcome {
                    tier: EvidenceTier::SizeOnlyCandidate,
                    target: Some(entry.path.clone()),
                    evidence,
                };
            }
        }
    } else if let Some(folder) = index
        .folders_by_name
        .get(&lower_bytes(&name))
        .and_then(|folders| folders.first())
    {
        // 5. A game folder: only a same-named folder is reported.
        evidence.push("a folder has the same name; its contents were not compared".into());
        return Outcome {
            tier: EvidenceTier::NameOnlyLead,
            target: Some(folder.path.clone()),
            evidence,
        };
    }

    evidence.push("no move record, and no file of this name".into());
    Outcome {
        tier: EvidenceTier::NoTrace,
        target: None,
        evidence,
    }
}
