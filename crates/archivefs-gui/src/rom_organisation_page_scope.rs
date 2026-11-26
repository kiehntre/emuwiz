//! Which files a Standard Bulk Rename preview covers, and its candidates from
//! saved catalogue evidence.
//!
//! The scope is chosen for this session only. The configuration is never
//! written, and configured source folders are never changed or disabled.
//! Games and platforms come from the Library, so no folder is walked and
//! unrelated sources are never read. A folder scope is walked completely,
//! with no file limit, and never crosses into another filesystem: a mounted
//! archive (ratarmount), a FUSE or network mount, or another disk inside the
//! folder is skipped and reported. Symlinked folders are never followed.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use archivefs_core::Database;
use archivefs_core::dat::rom_organisation::{OrganisationCandidate, OrganisationMode};
use archivefs_core::job_progress::ProgressReporter;
use archivefs_core::platform::identity::{PlatformIdentityResolution, resolve_platform_identity};

use super::{NameChoice, RommTitles, choose_name, hint_gate};

/// Deepest folder level a folder walk enters. Anything deeper is reported,
/// which keeps Apply off; it is never skipped silently.
pub(crate) const MAX_FOLDER_DEPTH: usize = 32;
/// Files per evidence batch: one database query, one progress step, one
/// cancellation point.
pub(crate) const EVIDENCE_BATCH: usize = 500;

/// One Library game in a scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScopedGame {
    pub archive_id: i64,
    pub path: PathBuf,
    /// The Library already knows the file is missing.
    pub known_missing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OrganisationScope {
    /// Games picked in the Library. `hidden_by_filter` of them were not shown
    /// by the Library filter when picked. They are included only because the
    /// person chose "Rename all selected", and the scope says so.
    Games {
        games: Vec<ScopedGame>,
        hidden_by_filter: usize,
    },
    /// Whole platforms, as the Library lists them.
    Platforms {
        names: BTreeSet<String>,
        games: Vec<ScopedGame>,
    },
    /// Folders inside the configured source folders.
    Folders(Vec<PathBuf>),
}

impl OrganisationScope {
    pub(crate) fn label(&self) -> String {
        fn plural(n: usize, one: &str, many: &str) -> String {
            format!("{n} {}", if n == 1 { one } else { many })
        }
        match self {
            Self::Games {
                games,
                hidden_by_filter: 0,
            } => plural(games.len(), "selected game", "selected games"),
            Self::Games {
                games,
                hidden_by_filter,
            } => format!(
                "{} ({hidden_by_filter} hidden by the Library filter)",
                plural(games.len(), "selected game", "selected games")
            ),
            Self::Platforms { names, games } => format!(
                "{} ({})",
                names.iter().cloned().collect::<Vec<_>>().join(", "),
                plural(games.len(), "game", "games")
            ),
            Self::Folders(folders) => match folders.as_slice() {
                [one] => format!("the folder {}", one.display()),
                many => plural(many.len(), "folder", "folders"),
            },
        }
    }

    /// Changes whenever the scope does, so a preview built for another scope
    /// is discarded rather than applied.
    pub(crate) fn fingerprint(&self) -> String {
        match self {
            Self::Games {
                games,
                hidden_by_filter,
            } => {
                let mut ids: Vec<i64> = games.iter().map(|g| g.archive_id).collect();
                ids.sort_unstable();
                format!("games:{hidden_by_filter}:{ids:?}")
            }
            Self::Platforms { names, games } => {
                let mut ids: Vec<i64> = games.iter().map(|g| g.archive_id).collect();
                ids.sort_unstable();
                format!("platforms:{names:?}:{ids:?}")
            }
            Self::Folders(folders) => {
                let mut sorted = folders.clone();
                sorted.sort();
                format!("folders:{sorted:?}")
            }
        }
    }
}

/// One file in scope, with its Library id when known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScopedFile {
    pub path: PathBuf,
    pub archive_id: Option<i64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ScopeEnumeration {
    pub files: Vec<ScopedFile>,
    /// Non-empty: Apply is refused (as for the old walk), with these reasons.
    pub incomplete: Vec<String>,
    /// Platform scope: Library games whose file is missing. They are listed
    /// and never renamed.
    pub not_on_disk: Vec<PathBuf>,
    /// Folders or games on another filesystem than their source folder
    /// (mounted archives, FUSE or network mounts, other disks): skipped.
    pub other_filesystems: Vec<PathBuf>,
}

/// The device a path's own object is on (`lstat`, never following a link).
/// A test may pretend another device for a path.
fn device_of(path: &Path) -> std::io::Result<u64> {
    #[cfg(test)]
    if let Some(device) = DEVICE_OVERRIDE.with(|hook| hook.borrow().as_ref().and_then(|f| f(path)))
    {
        return Ok(device);
    }
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::symlink_metadata(path)?.dev())
}

#[cfg(test)]
type DeviceOverride = Option<Box<dyn Fn(&Path) -> Option<u64>>>;

#[cfg(test)]
thread_local! {
    pub(crate) static DEVICE_OVERRIDE: std::cell::RefCell<DeviceOverride> =
        const { std::cell::RefCell::new(None) };
}

/// The configured source folder that holds `path` (longest match), compared
/// as canonical paths, so a symlink can never widen the boundary.
fn containing_source<'a>(path: &Path, sources: &'a [(PathBuf, PathBuf)]) -> Option<&'a Path> {
    sources
        .iter()
        .filter(|(_, canonical)| path.starts_with(canonical))
        .max_by_key(|(_, canonical)| canonical.components().count())
        .map(|(_, canonical)| canonical.as_path())
}

fn canonical_sources(sources: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    sources
        .iter()
        .filter_map(|source| Some((source.clone(), std::fs::canonicalize(source).ok()?)))
        .collect()
}

/// Lists the scope's files on the worker. `None`: cancelled. Nothing was
/// changed, and the caller keeps the previous plan.
pub(crate) fn enumerate_scope(
    scope: &OrganisationScope,
    configured_sources: &[PathBuf],
    cancel: Option<&AtomicBool>,
    progress: &mut ProgressReporter<'_>,
) -> Option<ScopeEnumeration> {
    let sources = canonical_sources(configured_sources);
    match scope {
        OrganisationScope::Games { games, .. } => {
            check_listed(games, &sources, true, cancel, progress)
        }
        OrganisationScope::Platforms { games, .. } => {
            check_listed(games, &sources, false, cancel, progress)
        }
        OrganisationScope::Folders(folders) => walk_folders(folders, &sources, cancel, progress),
    }
}

/// Library games: each must still be a regular file inside a configured
/// source folder, on that folder's own filesystem. `strict` (games the person
/// picked): any that cannot be checked makes the scope incomplete. Platform
/// scope: a game the Library already marks missing is listed as not on disk;
/// any other problem makes the scope incomplete.
fn check_listed(
    games: &[ScopedGame],
    sources: &[(PathBuf, PathBuf)],
    strict: bool,
    cancel: Option<&AtomicBool>,
    progress: &mut ProgressReporter<'_>,
) -> Option<ScopeEnumeration> {
    progress.phase(
        "Checking the chosen games",
        "games",
        Some(games.len() as u64),
    );
    let mut found = ScopeEnumeration::default();
    let mut source_devices: HashMap<PathBuf, Option<u64>> = HashMap::new();
    for (position, game) in games.iter().enumerate() {
        if position % EVIDENCE_BATCH == 0 {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return None;
            }
            progress.tick(position as u64);
        }
        let metadata = match std::fs::symlink_metadata(&game.path) {
            Ok(metadata) => metadata,
            Err(_) if !strict && game.known_missing => {
                found.not_on_disk.push(game.path.clone());
                continue;
            }
            Err(error) => {
                found.incomplete.push(format!(
                    "{} could not be checked ({error})",
                    game.path.display()
                ));
                continue;
            }
        };
        if !metadata.is_file() {
            found
                .incomplete
                .push(format!("{} is not a regular file", game.path.display()));
            continue;
        }
        // The parent is canonicalised, never the file: a file that is itself
        // a link is refused above.
        let parent = game
            .path
            .parent()
            .and_then(|p| std::fs::canonicalize(p).ok());
        let Some(source) = parent
            .as_deref()
            .and_then(|parent| containing_source(parent, sources))
        else {
            found.incomplete.push(format!(
                "{} is not inside a configured source folder",
                game.path.display()
            ));
            continue;
        };
        let source_device = *source_devices
            .entry(source.to_path_buf())
            .or_insert_with(|| device_of(source).ok());
        if device_of(&game.path).ok() != source_device || source_device.is_none() {
            found.other_filesystems.push(game.path.clone());
            continue;
        }
        found.files.push(ScopedFile {
            path: game.path.clone(),
            archive_id: Some(game.archive_id),
        });
    }
    progress.tick(games.len() as u64);
    found.incomplete.sort();
    found.incomplete.dedup();
    Some(found)
}

/// A complete walk of each folder: no file limit, the same games-only
/// filtering as the source walk, symlinked folders never followed, never
/// leaving the folder's own filesystem, and every unreadable folder or entry
/// (and anything past [`MAX_FOLDER_DEPTH`]) recorded as incomplete.
fn walk_folders(
    folders: &[PathBuf],
    sources: &[(PathBuf, PathBuf)],
    cancel: Option<&AtomicBool>,
    progress: &mut ProgressReporter<'_>,
) -> Option<ScopeEnumeration> {
    progress.phase("Finding your game files", "files", None);
    let mut found = ScopeEnumeration::default();
    let mut seen = 0_u64;
    let mut roots = Vec::new();
    for folder in folders {
        let canonical = match std::fs::canonicalize(folder) {
            Ok(canonical) => canonical,
            Err(error) => {
                found.incomplete.push(format!(
                    "{} could not be opened ({error})",
                    folder.display()
                ));
                continue;
            }
        };
        let Some(source) = containing_source(&canonical, sources) else {
            found.incomplete.push(format!(
                "{} is not inside a configured source folder",
                folder.display()
            ));
            continue;
        };
        match (device_of(&canonical), device_of(source)) {
            // The folder is walked as typed: Library paths use the configured
            // form, and the canonical one only proves where it is.
            (Ok(folder_device), Ok(source_device)) if folder_device == source_device => {
                roots.push((folder.clone(), folder_device));
            }
            (Ok(_), Ok(_)) => found.other_filesystems.push(folder.clone()),
            (Err(error), _) | (_, Err(error)) => found.incomplete.push(format!(
                "{} could not be checked ({error})",
                folder.display()
            )),
        }
    }
    // Overlapping choices (a folder and one inside it) are walked once.
    roots.sort();
    roots.dedup_by(|inner, outer| inner.0.starts_with(&outer.0));
    for (root, root_device) in roots {
        let mut queue: Vec<(PathBuf, usize)> = vec![(root, 0)];
        while let Some((dir, depth)) = queue.pop() {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return None;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) => {
                    found
                        .incomplete
                        .push(format!("could not read {}: {error}", dir.display()));
                    continue;
                }
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    found
                        .incomplete
                        .push(format!("could not read an entry in {}", dir.display()));
                    continue;
                };
                let path = entry.path();
                let metadata = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        found
                            .incomplete
                            .push(format!("could not inspect {}: {error}", path.display()));
                        continue;
                    }
                };
                if super::is_supporting_file(&path) {
                    continue;
                }
                if metadata.file_type().is_symlink() {
                    // The link object is a candidate; its target is never
                    // followed (it is Blocked later: only regular files are
                    // renamed from saved evidence).
                    found.files.push(ScopedFile {
                        path,
                        archive_id: None,
                    });
                } else if metadata.is_dir() {
                    if device_of(&path).ok() != Some(root_device) {
                        found.other_filesystems.push(path);
                    } else if depth < MAX_FOLDER_DEPTH {
                        queue.push((path, depth + 1));
                    } else {
                        found.incomplete.push(format!(
                            "folder exceeds {MAX_FOLDER_DEPTH}-level scan depth: {}",
                            path.display()
                        ));
                    }
                } else if metadata.is_file() {
                    found.files.push(ScopedFile {
                        path,
                        archive_id: None,
                    });
                    seen += 1;
                    if seen.is_multiple_of(EVIDENCE_BATCH as u64) {
                        progress.tick(seen);
                    }
                }
            }
        }
    }
    found.files.sort_by(|a, b| a.path.cmp(&b.path));
    found.files.dedup_by(|a, b| a.path == b.path);
    found.incomplete.sort();
    found.incomplete.dedup();
    found.other_filesystems.sort();
    Some(found)
}

/// What an evidence-first preview plans from.
#[derive(Default)]
pub(crate) struct EvidenceCandidates {
    pub candidates: Vec<OrganisationCandidate>,
    pub choices: BTreeMap<PathBuf, NameChoice>,
    /// Each file's raw SHA-1 from its current saved check, only where that
    /// check also gave the file its verified name.
    pub raw_sha1: HashMap<PathBuf, String>,
    /// What the saved checks say about each file, for the result groups.
    pub checks: HashMap<PathBuf, SavedCheck>,
}

/// Candidates for `files` from saved evidence: in batches of
/// [`EVIDENCE_BATCH`], one query each for the verified names; only the
/// catalogues those games' saved checks name are read. No file is read.
/// `None`: cancelled.
pub(crate) fn evidence_candidates(
    database: &Database,
    files: &[ScopedFile],
    mode: OrganisationMode,
    generation: u64,
    romm: &RommTitles,
    cancel: Option<&AtomicBool>,
    progress: &mut ProgressReporter<'_>,
) -> Option<Result<EvidenceCandidates, String>> {
    use archivefs_core::VerifiedDatName as V;
    // Folder scope: the files' Library ids, read once (one pass over the
    // paths, never one query per file).
    let lookup = if files.iter().any(|file| file.archive_id.is_none()) {
        match database.archive_path_lookup() {
            Ok(lookup) => Some(lookup),
            Err(error) => return Some(Err(format!("could not read the library ({error})"))),
        }
    } else {
        None
    };
    let ids: Vec<Option<i64>> = files
        .iter()
        .map(|file| {
            file.archive_id.or_else(|| {
                lookup
                    .as_ref()
                    .and_then(|lookup| lookup.find(&file.path).ok().flatten())
            })
        })
        .collect();
    // Only the catalogues these games' saved checks name.
    let saved_sources = match database.saved_dat_result_sources() {
        Ok(sources) => sources,
        Err(error) => return Some(Err(format!("could not read the saved checks ({error})"))),
    };
    let wanted: std::collections::HashSet<String> = ids
        .iter()
        .flatten()
        .filter_map(|id| saved_sources.get(id))
        .flatten()
        .cloned()
        .collect();
    let catalogues = if wanted.is_empty() {
        Default::default()
    } else {
        archivefs_core::dat::sources::default_dat_sources_config_path()
            .map(|path| {
                archivefs_core::dat::configured_catalogues::load_configured_catalogues_among(
                    &path,
                    Some(&wanted),
                )
            })
            .unwrap_or_default()
    };
    progress.phase(
        "Checking saved catalogue evidence",
        "games",
        Some(files.len() as u64),
    );
    let mut out = EvidenceCandidates::default();
    for (batch_index, batch) in files.chunks(EVIDENCE_BATCH).enumerate() {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return None;
        }
        let start = batch_index * EVIDENCE_BATCH;
        progress.tick(start as u64);
        let batch_ids = &ids[start..start + batch.len()];
        let wanted_ids: Vec<i64> = batch_ids
            .iter()
            .flatten()
            .copied()
            .filter(|id| saved_sources.contains_key(id))
            .collect();
        let evidence = match database.verified_dat_names_for_archives(&wanted_ids, &catalogues) {
            Ok(evidence) => evidence,
            Err(error) => {
                return Some(Err(format!("could not read the saved checks ({error})")));
            }
        };
        for (file, id) in batch.iter().zip(batch_ids) {
            let resolution = match id {
                Some(id) => resolve_platform_identity(
                    generation,
                    database
                        .current_platform_identity_evidence(*id, generation)
                        .ok()
                        .unwrap_or_default(),
                ),
                None => PlatformIdentityResolution::Unknown { generation },
            };
            let found = id.and_then(|id| evidence.get(&id));
            let dat = found.map(|e| e.name.clone()).unwrap_or(V::None);
            out.checks
                .insert(file.path.clone(), SavedCheck::from_evidence(found));
            let choice = choose_name(dat, romm.name_for(&file.path), &file.path);
            // The SHA-1 counts only when that same check named the file.
            if let (Some(found), super::NameSource::VerifiedDat { .. }) = (found, &choice.source)
                && let Some(sha1) = &found.raw_sha1
            {
                out.raw_sha1.insert(file.path.clone(), sha1.clone());
            }
            out.candidates.push(OrganisationCandidate {
                source_path: file.path.clone(),
                resolution,
                canonical_name: hint_gate::bulk_name(mode, &choice),
                content_classification: None,
                original_metadata: Default::default(),
            });
            out.choices.insert(file.path.clone(), choice);
        }
    }
    progress.tick(files.len() as u64);
    Some(Ok(out))
}

/// How one preview row is presented, in the owner's five groups. Display
/// only: Apply decides from the plan and live checks, never from this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RenameGroup {
    Ready,
    AlreadyCorrect,
    NeedsVerification,
    NoCatalogueMatch,
    Blocked,
}

impl RenameGroup {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ready => "Verified and ready to rename",
            Self::AlreadyCorrect => "Already correctly named",
            Self::NeedsVerification => "Needs verification",
            Self::NoCatalogueMatch => "No reliable catalogue match",
            Self::Blocked => "Blocked by a safety or file problem",
        }
    }

    pub(crate) const ALL: [Self; 5] = [
        Self::Ready,
        Self::AlreadyCorrect,
        Self::NeedsVerification,
        Self::NoCatalogueMatch,
        Self::Blocked,
    ];
}

/// The group of one row. "Needs verification" is not a failure: the game has
/// no current check (never checked, or its check went stale), and checking it
/// again in DAT Management can make it ready.
/// The evidence-first planner holds back every ready rename that has no raw
/// SHA-1 from a current catalogue check as "Needs verification". When the
/// name is not a verified catalogue name at all (a RomM title, or a name the
/// layout made), that is the wrong reason: say what the other previews say.
/// The row stays Blocked either way; only its reason changes.
pub(crate) fn explain_unverified_names(
    plan: &mut archivefs_core::dat::rom_organisation::OrganisationPlan,
    choices: &std::collections::BTreeMap<PathBuf, NameChoice>,
) {
    use archivefs_core::dat::rom_organisation::{NEEDS_VERIFICATION_REASON, OrganisationStatus};
    for entry in &mut plan.entries {
        if entry.status != OrganisationStatus::Blocked
            || entry.reason.as_deref() != Some(NEEDS_VERIFICATION_REASON)
        {
            continue;
        }
        let reason = match choices.get(&entry.source_path).map(|choice| &choice.source) {
            Some(super::NameSource::VerifiedDat { .. }) => continue,
            Some(super::NameSource::Romm) => super::hint_gate::UNVERIFIED_HINT_REASON,
            _ => super::hint_gate::UNVERIFIED_NAME_REASON,
        };
        entry.reason = Some(reason.to_string());
    }
}

pub(crate) fn rename_group(
    entry: &archivefs_core::dat::rom_organisation::OrganisationPlanEntry,
    choice: Option<&NameChoice>,
    saved_check: SavedCheck,
) -> RenameGroup {
    use archivefs_core::dat::rom_organisation::{NEEDS_VERIFICATION_REASON, OrganisationStatus};
    let verified =
        choice.is_some_and(|c| matches!(c.source, super::NameSource::VerifiedDat { .. }));
    // Without a verified name, what the saved check says (never "correct").
    let unverified = |check| match check {
        SavedCheck::Missing | SavedCheck::NotCurrent => RenameGroup::NeedsVerification,
        SavedCheck::NoMatch | SavedCheck::Other => RenameGroup::NoCatalogueMatch,
    };
    match entry.status {
        OrganisationStatus::Suggested => RenameGroup::Ready,
        // A file keeps its name both when it already has its verified name
        // and when there is no verified name at all: only the first is
        // "already correctly named". An archive whose match is a file inside
        // it has no verified name for the archive itself.
        OrganisationStatus::AlreadyOrganised if verified => RenameGroup::AlreadyCorrect,
        OrganisationStatus::AlreadyOrganised => unverified(saved_check),
        OrganisationStatus::Conflict | OrganisationStatus::Unsupported => RenameGroup::Blocked,
        OrganisationStatus::Blocked => {
            if entry.reason.as_deref() == Some(NEEDS_VERIFICATION_REASON) {
                RenameGroup::NeedsVerification
            } else if verified {
                // A verified name was found, but a guard refused the row.
                RenameGroup::Blocked
            } else {
                match saved_check {
                    SavedCheck::NoMatch => RenameGroup::NoCatalogueMatch,
                    SavedCheck::Missing | SavedCheck::NotCurrent => RenameGroup::NeedsVerification,
                    SavedCheck::Other => RenameGroup::Blocked,
                }
            }
        }
    }
}

/// What the saved checks say about one row's game, for [`rename_group`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SavedCheck {
    /// Never checked against a catalogue.
    Missing,
    /// Checked, but the check is no longer current (file or catalogue changed).
    NotCurrent,
    /// A current check found no entry in the catalogue.
    NoMatch,
    /// Anything else (ambiguous, bad dump, archive member, …).
    Other,
}

impl SavedCheck {
    /// From the batched evidence (`None`: the game has no saved check).
    pub(crate) fn from_evidence(evidence: Option<&archivefs_core::VerifiedDatEvidence>) -> Self {
        use archivefs_core::VerifiedDatName as V;
        match evidence {
            None => Self::Missing,
            Some(e) => match (&e.name, e.has_current_check) {
                (V::None, true) => Self::NoMatch,
                (V::None | V::NotUsable(_), false) => Self::NotCurrent,
                _ => Self::Other,
            },
        }
    }
}

/// The "What to include" choices being edited, before Preview turns them
/// into a scope. Session-only.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScopePicker {
    pub platforms: BTreeSet<String>,
    pub folder_draft: String,
    pub folders: Vec<PathBuf>,
    pub folder_error: Option<String>,
}

impl ScopePicker {
    /// Adds the typed folder when it is inside a configured source folder.
    pub(crate) fn add_folder(&mut self, configured_sources: &[PathBuf]) {
        let typed = PathBuf::from(self.folder_draft.trim());
        let sources = canonical_sources(configured_sources);
        self.folder_error = match std::fs::canonicalize(&typed) {
            Err(error) => Some(format!("{} could not be opened ({error})", typed.display())),
            Ok(canonical) if containing_source(&canonical, &sources).is_none() => Some(format!(
                "{} is not inside one of your configured source folders",
                typed.display()
            )),
            Ok(_) => {
                if !self.folders.contains(&typed) {
                    self.folders.push(typed);
                }
                self.folder_draft.clear();
                None
            }
        };
    }
}

/// Apply reads above this many approved files are rate-limited.
pub(crate) const THROTTLE_ABOVE_FILES: usize = 1_000;
/// The read cap for a large Apply: about a third of the USB disk's
/// sequential rate (138 MB/s measured), so streams keep their bandwidth.
pub(crate) const THROTTLED_READ_BYTES_PER_SECOND: u64 = 40 * 1_000_000;

/// While held, this thread's reads have idle disk priority (they are served
/// when the disk is otherwise idle; mq-deadline and BFQ honour it). The
/// previous priority is restored on drop. A refusal by the kernel is
/// harmless: reads then keep their normal priority.
pub(crate) struct IdleDiskPriority {
    previous: Option<libc::c_long>,
}

const IOPRIO_WHO_PROCESS: libc::c_long = 1;
const IOPRIO_CLASS_IDLE: libc::c_long = 3;
const IOPRIO_CLASS_SHIFT: libc::c_long = 13;

impl IdleDiskPriority {
    pub(crate) fn enter() -> Self {
        // SAFETY: ioprio_get/ioprio_set take plain integers; `0` names the
        // calling thread. No memory is passed.
        let previous = unsafe { libc::syscall(libc::SYS_ioprio_get, IOPRIO_WHO_PROCESS, 0) };
        let set = unsafe {
            libc::syscall(
                libc::SYS_ioprio_set,
                IOPRIO_WHO_PROCESS,
                0,
                IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT,
            )
        };
        Self {
            previous: (previous >= 0 && set == 0).then_some(previous),
        }
    }
}

impl Drop for IdleDiskPriority {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            // SAFETY: as in `enter`.
            unsafe {
                libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, 0, previous);
            }
        }
    }
}

#[cfg(test)]
#[path = "rom_organisation_page_scope_tests.rs"]
mod tests;
