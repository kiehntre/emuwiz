//! "Library files check": explains which files EmuWiz has never catalogued,
//! which catalogue entries point at missing files, and which files probably
//! moved. Everything shown comes from the read-only core reconciliation; this
//! module only translates it into plain language and pages it.
//!
//! It never imports, relinks, renames, moves or deletes anything. The only
//! thing a person can start here is an explicit, budgeted, cancellable checksum
//! calculation, which reads files and writes nothing.
use super::{
    App, Route, Section,
    activity::JobProgress,
    backend::{Command, Payload},
    browse_play::thousands,
    library::{Library, SharedLibrary},
};
use crate::ui::theme;
use archivefs_core::catalogue_reconciliation::{
    AmbiguityReason, CompanionBasis, FileState, MoveProof, RowState, StrongHash, SupportCategory,
    SupportContradiction, UnprovableReason, WeakBasis,
    db::{LibraryReconciliation, ReconciliationOutcome, reconcile_library},
    hashing::hash_files,
    support_category,
    support_check::{
        CatalogueToCheck, SupportCheckOutcome, SupportContentCheck, check_support_file_content,
    },
    walk::{RootWalkState, default_mount_of},
};
use eframe::egui;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Rows drawn per page: thousands of entries are reachable but never rendered at once.
pub(super) const PAGE_SIZE: usize = 50;
/// The most file content one "confirm" run will read.
pub(super) const HASH_BYTE_BUDGET: u64 = 4 * 1024 * 1024 * 1024;
const HASH_BATCH: usize = 8;

pub(super) type Hashes = BTreeMap<PathBuf, Vec<StrongHash>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Tab {
    NotCatalogued,
    Missing,
    StrongMatches,
    PossibleMoves,
    NeedsReview,
    DiscFiles,
    UnrecognisedFiles,
    SupportFiles,
    CannotCheck,
}

impl Tab {
    pub(super) const ALL: [Tab; 9] = [
        Tab::NotCatalogued,
        Tab::Missing,
        Tab::StrongMatches,
        Tab::PossibleMoves,
        Tab::NeedsReview,
        Tab::DiscFiles,
        Tab::UnrecognisedFiles,
        Tab::SupportFiles,
        Tab::CannotCheck,
    ];
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::NotCatalogued => "Game/media files not yet catalogued",
            Self::Missing => "Missing catalogue files",
            Self::StrongMatches => "Likely moved or renamed",
            Self::PossibleMoves => "Possible moves to review",
            Self::NeedsReview => "Ambiguous or conflicting",
            Self::DiscFiles => "Referenced disc files",
            Self::UnrecognisedFiles => "Unrecognised files",
            Self::SupportFiles => "Support files set aside",
            Self::CannotCheck => "Couldn't be checked",
        }
    }
    pub(super) fn meaning(self) -> &'static str {
        match self {
            Self::NotCatalogued => {
                "These game or media files are in your game folders, but EmuWiz has never catalogued them. EmuWiz does not guess what system they are for, and nothing is added automatically. Artwork, manuals and other support files are not listed here."
            }
            Self::Missing => {
                "These catalogue entries point to a file that is no longer there, and nothing else in your library clearly matches them. Nothing is removed."
            }
            Self::StrongMatches => {
                "Each of these catalogue entries has exactly one file whose stored checksum matches. Review the match; nothing has been changed."
            }
            Self::PossibleMoves => {
                "A file with a similar name and size exists, but EmuWiz cannot prove it is the same file."
            }
            Self::NeedsReview => {
                "More than one answer fits, or the stored information disagrees with itself. EmuWiz will not pick for you."
            }
            Self::DiscFiles => {
                "These files are already part of a catalogued CUE/GDI disc game. They do not need separate catalogue entries."
            }
            Self::UnrecognisedFiles => {
                "EmuWiz does not recognise these file types. They are not treated as game files, and they are not assumed to be artwork or manuals either: some may be a game format EmuWiz doesn't support yet. Nothing is changed."
            }
            Self::SupportFiles => {
                "Artwork, manuals and other support files EmuWiz set aside, each with its reason. They are not counted as games, not offered as moved files and never checksummed for this check. This list is for review only: nothing here can be moved, renamed, deleted or imported."
            }
            Self::CannotCheck => {
                "EmuWiz can't tell whether these files are really missing, because their storage couldn't be fully checked."
            }
        }
    }
    /// Informational tabs are not problems.
    pub(super) fn is_problem(self) -> bool {
        !matches!(self, Self::DiscFiles | Self::SupportFiles)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Item {
    pub title: String,
    pub path: String,
    pub root: Option<String>,
    pub size: Option<u64>,
    /// Plain-language explanation, first line first.
    pub lines: Vec<String>,
    /// For a likely move: where the matching file is now.
    pub new_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Summary {
    pub not_catalogued: usize,
    pub missing: usize,
    pub strong: usize,
    pub possible: usize,
    pub needs_review: usize,
    pub disc_files: usize,
    pub contended_disc_files: usize,
    pub cannot_check: usize,
    /// Artwork, manuals and other support files set aside: listed for review
    /// in their own tab, never a problem.
    pub support_files: usize,
    /// Support-extension files that catalogue evidence contradicts.
    pub support_review: usize,
    pub unrecognised: usize,
}

#[derive(Clone, Debug, Default)]
pub(super) struct View {
    pub summary: Summary,
    pub items: BTreeMap<Tab, Vec<Item>>,
    /// Why part of the library could not be fully examined, if so.
    pub coverage: Vec<String>,
    /// Files whose checksum would settle a "possible move".
    pub hash_requests: Vec<PathBuf>,
    pub roots: Vec<PathBuf>,
    pub files_examined: usize,
}

impl View {
    pub(super) fn complete(&self) -> bool {
        self.coverage.is_empty()
    }
    pub(super) fn problem_count(&self) -> usize {
        Tab::ALL
            .iter()
            .filter(|tab| tab.is_problem())
            .map(|tab| self.items.get(tab).map_or(0, Vec::len))
            .sum()
    }
}

pub(super) fn row_lines(state: &RowState) -> Vec<String> {
    match state {
        RowState::CataloguedPresent => vec!["This file is where EmuWiz expects it.".into()],
        RowState::CatalogueFileMissing => vec![
            "This catalogue entry points to a file that is no longer there, and nothing else in your library clearly matches it.".into(),
        ],
        RowState::StrongMoveCandidate { .. } => vec![
            "This appears to be the same file at a new location. Its stored checksum matches exactly.".into(),
            "Nothing has been changed. EmuWiz does not relink files on its own.".into(),
        ],
        RowState::AmbiguousMoveCandidates { reason, candidates_total, .. } => vec![
            match reason {
                AmbiguityReason::ManyFilesOneRow => format!(
                    "More than one file could match this library entry ({candidates_total} found)."
                ),
                AmbiguityReason::ManyRowsOneFile => {
                    "More than one library entry matches this same file, so EmuWiz will not pick one.".into()
                }
            },
        ],
        RowState::PossiblyMoved { basis, proof, candidates_total, .. } => {
            let mut lines = vec![match basis {
                WeakBasis::NameAndSize => format!(
                    "A file with a similar name and size exists, but EmuWiz cannot prove it is the same file ({candidates_total} possible)."
                ),
                WeakBasis::SizeOnly => format!(
                    "{candidates_total} file(s) have the same size, but EmuWiz cannot prove any is the same file."
                ),
            }];
            lines.push(match proof {
                MoveProof::NeedsHashing => {
                    "EmuWiz needs a checksum before it can prove this is the same file.".into()
                }
                MoveProof::NoPersistedHash => {
                    "EmuWiz has no stored checksum for this entry, so it cannot be proven either way.".into()
                }
            });
            lines
        }
        RowState::Conflict { .. } => vec![
            "The stored information about this entry disagrees with itself, so EmuWiz will not guess.".into(),
        ],
        RowState::Unknown { reason } => vec![match reason {
            UnprovableReason::StorageUnavailable => {
                "EmuWiz cannot check this file while its storage location is unavailable.".to_string()
            }
            UnprovableReason::SourceNeedsReview => {
                "This library location's storage has changed since it was last reviewed, so EmuWiz is not treating its files as missing.".to_string()
            }
            UnprovableReason::NestedBoundaryUnproven => {
                "This file sits behind a drive or mount EmuWiz can't confirm is the same one as before.".to_string()
            }
            UnprovableReason::Inaccessible => {
                "EmuWiz couldn't read this location (permissions or a read error).".to_string()
            }
        }],
    }
}

pub(super) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Pure projection of the core result into plain-language lists.
pub(super) fn build_view(rec: &LibraryReconciliation, library: &Library) -> View {
    let title_of = |id: i64| {
        library
            .game(id)
            .map_or_else(|| format!("entry {id}"), |game| game.title.clone())
    };
    let root_of = |source: i64| rec.root_paths.get(&source).map(|p| p.display().to_string());
    let mut view = View {
        hash_requests: rec.report.hash_requests.clone(),
        roots: rec.root_paths.values().cloned().collect(),
        files_examined: rec.report.counts.files,
        ..View::default()
    };
    for root in &rec.roots {
        let name = rec.root_paths.get(&root.source_id).map_or_else(
            || "A library location".to_string(),
            |p| p.display().to_string(),
        );
        let note = match root.state {
            RootWalkState::Complete if !root.truncated => None,
            RootWalkState::Complete | RootWalkState::Partial => Some(if root.truncated {
                format!(
                    "{name}: only part of it could be examined (it is very large or deeply nested)."
                )
            } else if !root.nested_boundaries.is_empty() {
                format!("{name}: contains other drives or mounts that were not entered.")
            } else {
                format!("{name}: some folders could not be read.")
            }),
            RootWalkState::Unavailable => Some(format!("{name}: could not be reached.")),
            RootWalkState::Cancelled => Some(format!("{name}: the check was stopped early.")),
        };
        view.coverage.extend(note);
    }
    for row in &rec.report.rows {
        let tab = match &row.state {
            RowState::CataloguedPresent => continue,
            RowState::CatalogueFileMissing => Tab::Missing,
            RowState::StrongMoveCandidate { .. } => Tab::StrongMatches,
            RowState::PossiblyMoved { .. } => Tab::PossibleMoves,
            RowState::AmbiguousMoveCandidates { .. } | RowState::Conflict { .. } => {
                Tab::NeedsReview
            }
            RowState::Unknown { .. } => Tab::CannotCheck,
        };
        let mut lines = row_lines(&row.state);
        if let Some(platform) = &row.platform {
            lines.push(format!(
                "System on record: {}",
                archivefs_core::platform::display_name_for(platform)
            ));
        }
        let new_path = match &row.state {
            RowState::StrongMoveCandidate { to } => Some(to.display().to_string()),
            _ => None,
        };
        let listed = match &row.state {
            RowState::PossiblyMoved {
                candidates,
                candidates_total,
                ..
            }
            | RowState::AmbiguousMoveCandidates {
                candidates,
                candidates_total,
                ..
            } => {
                for candidate in candidates {
                    lines.push(format!("Could be: {}", candidate.display()));
                }
                (*candidates_total > candidates.len()).then(|| {
                    format!(
                        "…and {} more not listed",
                        candidates_total - candidates.len()
                    )
                })
            }
            _ => None,
        };
        lines.extend(listed);
        view.items.entry(tab).or_default().push(Item {
            title: title_of(row.archive_id),
            path: row.path.display().to_string(),
            root: None,
            size: None,
            lines,
            new_path,
        });
    }
    for file in &rec.report.files {
        let (tab, lines) = match &file.state {
            FileState::Catalogued { .. } => continue,
            // Listed read-only and paged: a library can hold tens of
            // thousands of artwork files.
            FileState::KnownSupportFile => (Tab::SupportFiles, support_lines(&file.path)),
            FileState::SupportNeedsReview { evidence } => (
                Tab::NeedsReview,
                support_review_lines(&file.path, evidence, &title_of),
            ),
            FileState::UnrecognisedFile => (
                Tab::UnrecognisedFiles,
                vec!["EmuWiz doesn't recognise this file type. It has not been treated as a game, and it has not been assumed to be artwork or a manual.".to_string()],
            ),
            FileState::Uncatalogued => (
                Tab::NotCatalogued,
                vec!["EmuWiz has never catalogued this file. No system has been assumed from its folder.".to_string()],
            ),
            FileState::StrongMoveTarget { archive_id } => (
                Tab::NotCatalogued,
                vec![format!("May be the new location of \"{}\" (see Likely moved or renamed).", title_of(*archive_id))],
            ),
            FileState::ContendedMoveTarget { .. } => (
                Tab::NotCatalogued,
                vec!["Several catalogue entries match this file, so EmuWiz will not pick one.".to_string()],
            ),
            FileState::ReferencedCompanion { parent_archive_id, basis } => (
                Tab::DiscFiles,
                vec![format!(
                    "Part of the disc game \"{}\" (named by its {}). It does not need its own entry.",
                    title_of(*parent_archive_id),
                    match basis {
                        CompanionBasis::CueFileReference => "CUE sheet",
                        CompanionBasis::GdiTrackReference => "GDI descriptor",
                    }
                )],
            ),
            FileState::ContendedCompanion { parent_archive_ids } => (
                Tab::NeedsReview,
                vec![
                    "This file is referenced by more than one catalogued disc descriptor.".to_string(),
                    format!(
                        "Referenced by: {}. EmuWiz will not guess which one owns it.",
                        parent_archive_ids.iter().map(|id| format!("\"{}\"", title_of(*id))).collect::<Vec<_>>().join(", ")
                    ),
                ],
            ),
        };
        view.items.entry(tab).or_default().push(Item {
            title: file_name(&file.path),
            path: file.path.display().to_string(),
            root: root_of(file.source_id),
            size: Some(file.size),
            lines,
            new_path: None,
        });
    }
    for items in view.items.values_mut() {
        items.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.title.cmp(&b.title)));
    }
    let len = |tab| view.items.get(&tab).map_or(0, Vec::len);
    view.summary = Summary {
        not_catalogued: len(Tab::NotCatalogued),
        missing: len(Tab::Missing),
        strong: len(Tab::StrongMatches),
        possible: len(Tab::PossibleMoves),
        needs_review: len(Tab::NeedsReview),
        disc_files: len(Tab::DiscFiles),
        contended_disc_files: rec.report.counts.contended_companions,
        cannot_check: len(Tab::CannotCheck),
        support_files: len(Tab::SupportFiles),
        support_review: rec.report.counts.support_review_files,
        // Core count: also includes unrecognised files that are a move target.
        unrecognised: rec.report.counts.unrecognised_files,
    };
    view
}

fn extension_label(path: &std::path::Path) -> String {
    path.extension()
        .or_else(|| path.file_name())
        .map(|ext| {
            ext.to_string_lossy()
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

/// Category and exact reason for a support file that was set aside.
pub(super) fn support_lines(path: &std::path::Path) -> Vec<String> {
    let category = match support_category(path) {
        SupportCategory::Artwork => "Artwork (box art, screenshots, fan art)",
        SupportCategory::Manual => "Manual or guide (PDF)",
        SupportCategory::TextOrMetadata => "Text or metadata (readme, NFO, config)",
        SupportCategory::Housekeeping => "Frontend or system housekeeping file",
        SupportCategory::VideoOrAudio => "Video or audio extra (trailer, theme music)",
        SupportCategory::Other => "Other support file",
    };
    vec![
        format!("Category: {category}"),
        format!(
            "Set aside because .{} is on EmuWiz's list of file types that are never game content, and no catalogue entry, disc descriptor or stored checksum says otherwise.",
            extension_label(path)
        ),
    ]
}

/// Why a support-extension file was kept for review instead of set aside.
pub(super) fn support_review_lines(
    path: &std::path::Path,
    evidence: &SupportContradiction,
    title_of: &dyn Fn(i64) -> String,
) -> Vec<String> {
    let (basis, archive_id) = match evidence {
        SupportContradiction::ChecksumMatchesCatalogueEntry { archive_id } => {
            ("its checksum matches the stored checksum of", *archive_id)
        }
        SupportContradiction::NameAndSizeMatchCatalogueEntry { archive_id } => {
            ("its name and size match the catalogue entry", *archive_id)
        }
    };
    vec![
        format!(
            "This file has a support-file type (.{}), but {basis} \"{}\". It may be game content under a misleading name.",
            extension_label(path),
            title_of(archive_id)
        ),
        "EmuWiz has not set it aside as support and has not offered it as a moved file. Review it yourself; nothing has been changed.".to_string(),
    ]
}

/// Indices of `items` whose title, path or explanation contains `filter`
/// (case-insensitive). An empty filter keeps every item.
pub(super) fn filtered(items: &[Item], filter: &str) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() {
        return (0..items.len()).collect();
    }
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.title.to_lowercase().contains(&needle)
                || item.path.to_lowercase().contains(&needle)
                || item
                    .lines
                    .iter()
                    .any(|line| line.to_lowercase().contains(&needle))
        })
        .map(|(index, _)| index)
        .collect()
}

/// The result of an explicit hashing run.
#[derive(Debug, Default)]
pub(super) struct HashRun {
    pub hashes: Hashes,
    pub requested: usize,
    pub unreadable: usize,
    pub over_budget: usize,
    pub cancelled: bool,
}

/// What the person last asked for, shown honestly (never a fake success).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HashNote {
    Done {
        hashed: usize,
        unreadable: usize,
        over_budget: usize,
    },
    Stopped {
        hashed: usize,
        requested: usize,
    },
}

pub(super) struct FilterCache {
    filter: String,
    view: Arc<View>,
    pub(super) shown: Arc<Vec<usize>>,
}

#[derive(Default)]
pub(super) struct ReconcileState {
    pub view: Option<Arc<View>>,
    pub job: Option<u64>,
    pub error: Option<String>,
    /// Checksums from explicit runs this session: the only way a file gains one.
    pub hashes: Arc<Hashes>,
    pub hash_job: Option<u64>,
    pub hash_note: Option<HashNote>,
    /// Tests only: read this catalogue instead of the real one.
    pub source_override: Option<ReconcileSource>,
    /// How many library checks / checksum runs were ever started. Opening the
    /// page must leave both at zero: every run is an explicit click.
    pub runs_started: usize,
    pub hash_runs_started: usize,
    /// The last check was stopped before it produced an answer.
    pub last_stopped: bool,
    pub tab: Option<Tab>,
    pub page: BTreeMap<Tab, usize>,
    /// Per-tab text filter over the already computed results.
    pub filter: BTreeMap<Tab, String>,
    /// Last filter result per tab, keyed by the filter text and the view it
    /// was computed from, so a large list is not re-searched every frame.
    pub filter_cache: BTreeMap<Tab, FilterCache>,
    /// The one running "Check content against DAT" job, and its file.
    pub support_job: Option<(u64, PathBuf)>,
    /// Content-check results for the current listing only: cleared whenever
    /// a new Library Files Check result arrives.
    pub support_checks: BTreeMap<PathBuf, SupportCheckRun>,
    /// Tests only: compare with these DAT files instead of installed ones.
    pub catalogue_override: Option<Vec<(String, String, PathBuf)>>,
    pub open_item: Option<(Tab, usize)>,
}

impl ReconcileState {
    pub(super) fn hash_enabled(&self) -> bool {
        self.job.is_none()
            && self.hash_job.is_none()
            && self
                .view
                .as_ref()
                .is_some_and(|v| !v.hash_requests.is_empty())
    }
}

// ---- background work --------------------------------------------------------

/// Where a check reads from. Resolved on the UI thread so tests can point it at
/// a temporary catalogue and never at the real one.
#[derive(Clone, Debug)]
pub(super) struct ReconcileSource {
    pub database: PathBuf,
    pub roots: Vec<PathBuf>,
}

fn default_source() -> Result<ReconcileSource, String> {
    Ok(ReconcileSource {
        database: archivefs_core::default_database_path().map_err(|e| e.to_string())?,
        roots: archivefs_core::Config::load_default()
            .map_err(|e| e.to_string())?
            .source_folders,
    })
}

pub(super) fn run_reconcile(
    library: &SharedLibrary,
    hashes: &Hashes,
    source: &ReconcileSource,
    cancel: &AtomicBool,
) -> Result<Payload, String> {
    if !source.database.exists() {
        return Err("EmuWiz has no game catalogue yet, so there is nothing to check.".into());
    }
    // Opened read-only: a check never writes to the catalogue.
    let database = archivefs_core::Database::open_catalogue_health_read_only(&source.database)
        .map_err(|e| e.to_string())?;
    match reconcile_library(&database, &source.roots, hashes, cancel, &default_mount_of)
        .map_err(|e| e.to_string())?
    {
        ReconciliationOutcome::Complete(done) => Ok(Payload::Reconciliation(Some(Arc::new(
            build_view(&done, library),
        )))),
        ReconciliationOutcome::Cancelled => Ok(Payload::Reconciliation(None)),
    }
}

/// Hashes `requests` in small batches so progress and cancellation are real.
pub(super) fn run_hashing(
    roots: &[PathBuf],
    requests: &[PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(JobProgress),
) -> HashRun {
    let mut run = HashRun {
        requested: requests.len(),
        ..HashRun::default()
    };
    let mut remaining = HASH_BYTE_BUDGET;
    let total = requests.len() as u64;
    for (index, batch) in requests.chunks(HASH_BATCH).enumerate() {
        progress(
            JobProgress::new("Calculating checksums", "files")
                .with_total(total)
                .at((index * HASH_BATCH) as u64),
        );
        let outcome = hash_files(roots, batch, remaining, cancel);
        remaining = remaining.saturating_sub(outcome.bytes_read);
        run.unreadable += outcome.unreadable.len();
        run.over_budget += outcome.over_budget.len();
        run.hashes.extend(outcome.hashes);
        if outcome.cancelled || cancel.load(Ordering::Relaxed) {
            run.cancelled = true;
            break;
        }
    }
    run
}

/// Which catalogues a support content check compares with.
#[derive(Clone, Debug)]
pub(super) enum SupportCatalogues {
    /// Every installed, verify-capable catalogue, resolved and re-proven.
    Installed,
    /// Tests only: `(label, provenance, DAT path)`.
    Fixture(Vec<(String, String, PathBuf)>),
}

/// A finished content check plus the catalogues that could not be loaded.
#[derive(Clone, Debug)]
pub(super) struct SupportCheckRun {
    pub check: SupportContentCheck,
    /// `(catalogue, why)`: skipped, never silently treated as "no match".
    pub skipped: Vec<(String, String)>,
}

impl SupportCheckRun {
    /// An exact checksum match (single or ambiguous) flags the file for
    /// game-media review. Nothing is reclassified.
    pub(super) fn flagged(&self) -> bool {
        matches!(
            self.check.outcome,
            SupportCheckOutcome::ExactMatch | SupportCheckOutcome::AmbiguousMatch
        )
    }
}

fn provenance_label(
    provenance: &archivefs_core::dat::catalogue_selection::CatalogueProvenance,
) -> String {
    use archivefs_core::dat::catalogue_selection::CatalogueProvenance as P;
    match provenance {
        P::UserRegistered => "registered by you".into(),
        P::NoIntroPackProjection => "No-Intro pack".into(),
        P::TosecReleasePackProjection { pack_id } => format!("TOSEC release pack {pack_id}"),
        P::EmuWizManaged { provider } => format!("EmuWiz-managed ({provider:?})"),
    }
}

/// Worker body: load and re-prove each catalogue, then check ONE file.
pub(super) fn run_support_check(
    roots: &[PathBuf],
    path: &std::path::Path,
    size: u64,
    catalogues: &SupportCatalogues,
    cancel: &AtomicBool,
) -> SupportCheckRun {
    use archivefs_core::dat::catalogue_selection::{
        CatalogueAvailability, list_installed_catalogues, resolve_catalogue,
    };
    use archivefs_core::dat::index::DatIndex;
    let mut skipped = Vec::new();
    let mut loaded: Vec<(String, String, DatIndex)> = Vec::new();
    match catalogues {
        SupportCatalogues::Fixture(list) => {
            for (label, provenance, dat) in list {
                match archivefs_core::dat::parsers::parse_dat_file(dat, Default::default()) {
                    Ok(parsed) => loaded.push((
                        label.clone(),
                        provenance.clone(),
                        DatIndex::build(&parsed.dat),
                    )),
                    Err(error) => skipped.push((label.clone(), error.to_string())),
                }
            }
        }
        SupportCatalogues::Installed => {
            match crate::dat_catalogue_picker::load_inventory_snapshot() {
                Err(error) => skipped.push(("Installed DAT catalogues".into(), error.to_string())),
                Ok(snapshot) => {
                    for summary in list_installed_catalogues(snapshot.inputs()) {
                        if !summary.enabled
                            || !summary.capabilities.verify
                            || matches!(
                                summary.availability,
                                CatalogueAvailability::AggregateFolder { .. }
                            )
                        {
                            continue;
                        }
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        match resolve_catalogue(&summary.reference, snapshot.inputs()) {
                            Ok(resolved) => loaded.push((
                                resolved.display_name.clone(),
                                provenance_label(&resolved.provenance),
                                DatIndex::build(&resolved.parsed),
                            )),
                            Err(error) => {
                                skipped.push((summary.display_name.clone(), error.to_string()))
                            }
                        }
                    }
                }
            }
        }
    }
    let to_check: Vec<CatalogueToCheck<'_>> = loaded
        .iter()
        .map(|(label, provenance, index)| CatalogueToCheck {
            label: label.clone(),
            provenance: provenance.clone(),
            index,
        })
        .collect();
    SupportCheckRun {
        check: check_support_file_content(roots, path, size, &to_check, cancel),
        skipped,
    }
}

/// Plain-language result lines for one content check.
pub(super) fn support_check_lines(run: &SupportCheckRun) -> Vec<String> {
    use archivefs_core::dat::audit::AuditVerdict;
    let check = &run.check;
    let checked = check.checked.join(", ");
    let mut lines = vec![match check.outcome {
        SupportCheckOutcome::ExactMatch => "Exact checksum match. Flagged for game-media review: this file's content is a catalogued game, not a support file.".to_string(),
        SupportCheckOutcome::AmbiguousMatch => "Ambiguous match: this content matches more than one catalogue entry, so EmuWiz will not pick one. Flagged for game-media review.".to_string(),
        SupportCheckOutcome::NoMatchInCheckedCatalogues => format!("No match in the checked catalogues ({checked}). This does not prove the file is artwork or not a game."),
        SupportCheckOutcome::NoCatalogueToCompare => "No installed DAT catalogue could be compared, so nothing was concluded about this file.".to_string(),
        SupportCheckOutcome::Unreadable => "EmuWiz could not read this file, so its content was not checked.".to_string(),
        SupportCheckOutcome::ChangedSinceListed => "This file changed after Library Files Check listed it, so it was not checked. Run Check again first.".to_string(),
        SupportCheckOutcome::Cancelled => "The content check was stopped before it finished.".to_string(),
    }];
    for found in &check.matches {
        lines.push(match &found.verdict {
            AuditVerdict::Exact {
                game_name,
                rom_name,
                algorithm,
            } => format!(
                "Matches \"{game_name}\" ({rom_name}) in {} [{}], by {algorithm}.",
                found.catalogue, found.provenance
            ),
            AuditVerdict::ExactMultipleCandidates {
                algorithm,
                count,
                game_names,
            } => format!(
                "Matches {count} entries in {} [{}] by {algorithm}: {}.",
                found.catalogue,
                found.provenance,
                game_names.join(", ")
            ),
            other => format!("{} in {}.", other.label(), found.catalogue),
        });
    }
    for hash in &check.hashes {
        lines.push(
            format!("{:?}: {}", hash.algorithm, hash.hex)
                .replace("Sha1", "SHA-1")
                .replace("Sha256", "SHA-256"),
        );
    }
    for (catalogue, why) in &run.skipped {
        lines.push(format!(
            "Not compared: {catalogue} could not be loaded ({why})."
        ));
    }
    lines.push("Nothing was moved, renamed, imported or reclassified.".to_string());
    lines
}

// ---- app wiring ---------------------------------------------------------------

impl App {
    fn start_reconcile(&mut self) {
        if self.reconcile.job.is_some() {
            return;
        }
        self.reconcile.error = None;
        self.reconcile.last_stopped = false;
        let source = match self
            .reconcile
            .source_override
            .clone()
            .map_or_else(default_source, Ok)
        {
            Ok(source) => source,
            Err(error) => {
                self.reconcile.error = Some(error);
                return;
            }
        };
        let id = self
            .activity
            .queue("Checking library files", Route::LibraryFiles, true);
        let cancel = self.job_cancel_flag(id);
        self.reconcile.job = Some(id);
        self.reconcile.runs_started += 1;
        self.send(
            id,
            Command::ReconcileLibrary {
                library: self.library.clone(),
                hashes: self.reconcile.hashes.clone(),
                source,
                cancel,
            },
        );
    }

    fn start_reconcile_hashing(&mut self) {
        let Some(view) = self.reconcile.view.clone() else {
            return;
        };
        if !self.reconcile.hash_enabled() {
            return;
        }
        self.reconcile.hash_note = None;
        let id = self.activity.queue(
            "Calculating checksums to confirm moves",
            Route::LibraryFiles,
            true,
        );
        let cancel = self.job_cancel_flag(id);
        self.reconcile.hash_job = Some(id);
        self.reconcile.hash_runs_started += 1;
        self.send(
            id,
            Command::ReconcileHash {
                roots: view.roots.clone(),
                requests: view.hash_requests.clone(),
                cancel,
            },
        );
    }

    pub(super) fn reconcile_done(&mut self, view: Option<Arc<View>>) {
        self.reconcile.job = None;
        // A stopped check leaves the previous answer in place rather than a
        // partial one dressed up as complete.
        match view {
            Some(view) => {
                self.reconcile.view = Some(view);
                self.reconcile.page.clear();
                self.reconcile.open_item = None;
                // Results described the previous listing's files.
                self.reconcile.support_checks.clear();
            }
            None => self.reconcile.last_stopped = true,
        }
    }

    pub(super) fn reconcile_hash_done(&mut self, run: HashRun) {
        self.reconcile.hash_job = None;
        let hashed = run.hashes.len();
        if !run.hashes.is_empty() {
            let mut all = (*self.reconcile.hashes).clone();
            all.extend(run.hashes);
            self.reconcile.hashes = Arc::new(all);
        }
        if run.cancelled {
            self.reconcile.hash_note = Some(HashNote::Stopped {
                hashed,
                requested: run.requested,
            });
            return;
        }
        self.reconcile.hash_note = Some(HashNote::Done {
            hashed,
            unreadable: run.unreadable,
            over_budget: run.over_budget,
        });
        // Refresh with the new evidence. Nothing is relinked.
        self.start_reconcile();
    }

    pub(super) fn start_support_check(&mut self, path: PathBuf, size: u64) {
        let Some(view) = self.reconcile.view.clone() else {
            return;
        };
        if self.reconcile.support_job.is_some()
            || self.reconcile.job.is_some()
            || self.reconcile.hash_job.is_some()
        {
            return;
        }
        let id = self.activity.queue(
            "Checking one file's content against DAT catalogues",
            Route::LibraryFiles,
            true,
        );
        let cancel = self.job_cancel_flag(id);
        self.reconcile.support_job = Some((id, path.clone()));
        self.reconcile.support_checks.remove(&path);
        let catalogues = self
            .reconcile
            .catalogue_override
            .clone()
            .map_or(SupportCatalogues::Installed, SupportCatalogues::Fixture);
        self.send(
            id,
            Command::ReconcileSupportCheck {
                roots: view.roots.clone(),
                path,
                size,
                catalogues,
                cancel,
            },
        );
    }

    pub(super) fn reconcile_support_check_done(&mut self, id: u64, run: SupportCheckRun) {
        if self.reconcile.support_job.as_ref().map(|(job, _)| *job) != Some(id) {
            return;
        }
        self.reconcile.support_job = None;
        if !run.check.hashes.is_empty() {
            // Session evidence only, like a move-confirmation run: the next
            // explicit "Check again" can use it. Nothing is written.
            let mut all = (*self.reconcile.hashes).clone();
            all.insert(run.check.path.clone(), run.check.hashes.clone());
            self.reconcile.hashes = Arc::new(all);
        }
        self.reconcile
            .support_checks
            .insert(run.check.path.clone(), run);
    }

    pub(super) fn reconcile_failed(&mut self, id: u64, error: &str) {
        if self.reconcile.support_job.as_ref().map(|(job, _)| *job) == Some(id) {
            self.reconcile.support_job = None;
            self.reconcile.error = Some(error.to_string());
        }
        if self.reconcile.job == Some(id) {
            self.reconcile.job = None;
            self.reconcile.error = Some(error.to_string());
        }
        if self.reconcile.hash_job == Some(id) {
            self.reconcile.hash_job = None;
            self.reconcile.hash_note = None;
            self.reconcile.error = Some(error.to_string());
        }
    }

    pub(super) fn library_files_page(&mut self, ui: &mut egui::Ui) {
        // Opening this page is passive: it never walks the library, reads a file
        // or refreshes a result on its own. Every run starts from a button.
        let view = self.reconcile.view.clone();
        let busy = self.reconcile.job.is_some();
        let hashing = self.reconcile.hash_job.is_some();
        egui::ScrollArea::vertical().id_salt("v2_library_files").show(ui, |ui| {
            ui.heading("Library files check");
            ui.label("Compares the files in your game folders with EmuWiz's catalogue. Nothing is added, relinked, renamed, moved or deleted here.");
            ui.horizontal_wrapped(|ui| {
                if busy {
                    ui.spinner();
                    ui.label("Checking your library… you can keep browsing.");
                    if let Some(flag) = self.reconcile.job.map(|id| self.job_cancel_flag(id))
                        && ui.button("Stop").clicked()
                    {
                        flag.store(true, Ordering::Relaxed);
                    }
                } else if view.is_some()
                    && ui.add_enabled(!hashing, egui::Button::new("Check again")).clicked()
                {
                    self.start_reconcile();
                }
                if ui.button("Back to Problems").clicked() {
                    self.go(Route::Section(Section::Problems));
                }
            });
            if let Some(error) = &self.reconcile.error {
                ui.colored_label(theme::warning(ui), format!("The check could not finish. No files were changed. ({error})"));
            }
            let Some(view) = view else {
                if !busy {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.strong("Nothing has been checked yet");
                        ui.label("Compare EmuWiz's catalogue with the files currently on disk. Nothing will be changed.");
                        ui.label("This reads folder listings across your game folders, so on a large library it can take a while. You can stop it at any time.");
                        if self.reconcile.last_stopped {
                            ui.label("The last check was stopped before it finished, so there are no results from it.");
                        }
                        if ui
                            .add_enabled(!hashing, egui::Button::new("Check library now"))
                            .clicked()
                        {
                            self.start_reconcile();
                        }
                    });
                }
                return;
            };
            self.library_files_body(ui, &view, hashing);
        });
    }

    fn library_files_body(&mut self, ui: &mut egui::Ui, view: &Arc<View>, hashing: bool) {
        if !view.complete() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.colored_label(theme::warning(ui), "Some library locations could not be fully checked.");
                ui.label("The counts below cover only what EmuWiz could examine; they are not a complete picture of your library.");
                ui.label(egui::RichText::new("Mr Wiz · Some storage locations could not be checked.").small());
                for note in &view.coverage {
                    ui.label(note);
                }
            });
        }
        let s = &view.summary;
        let nothing = view.problem_count() == 0 && s.disc_files == 0;
        if nothing && view.complete() {
            ui.label(
                egui::RichText::new("Mr Wiz · Everything EmuWiz checked is accounted for.")
                    .strong(),
            );
        }
        ui.label(format!(
            "{} files examined.",
            thousands(view.files_examined)
        ));
        if s.support_files > 0 {
            // Informational only: support files are not errors.
            ui.label(format!(
                "{} {} set aside as artwork, manuals or other support files (see Support files set aside).",
                thousands(s.support_files),
                if s.support_files == 1 { "file" } else { "files" }
            ));
        }
        let flagged = self
            .reconcile
            .support_checks
            .values()
            .filter(|run| run.flagged())
            .count();
        if flagged > 0 {
            ui.colored_label(
                theme::warning(ui),
                format!(
                    "{flagged} support {} flagged for game-media review by a content check (see Support files set aside).",
                    if flagged == 1 { "file was" } else { "files were" }
                ),
            );
        }
        if s.support_review > 0 {
            ui.label(format!(
                "{} {} like support files but catalogue evidence disagrees; see Ambiguous or conflicting.",
                thousands(s.support_review),
                if s.support_review == 1 { "file looks" } else { "files look" }
            ));
        }
        if s.support_files > 0 || s.unrecognised > 0 {
            ui.label(
                egui::RichText::new(format!(
                    "Technical: {} support files · {} unrecognised files · {} files examined in total",
                    thousands(s.support_files),
                    thousands(s.unrecognised),
                    thousands(view.files_examined)
                ))
                .small(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            for tab in Tab::ALL {
                let count = view.items.get(&tab).map_or(0, Vec::len);
                let selected = self.reconcile.tab == Some(tab);
                if ui
                    .selectable_label(selected, format!("{} · {count}", tab.title()))
                    .clicked()
                {
                    self.reconcile.tab = Some(tab);
                    self.reconcile.open_item = None;
                }
            }
        });
        if s.contended_disc_files > 0 {
            ui.label("Some disc files are referenced by more than one catalogued disc descriptor; see Ambiguous or conflicting.");
        }
        self.hash_action(ui, view, hashing);
        let tab = self.reconcile.tab.unwrap_or(Tab::NotCatalogued);
        ui.separator();
        ui.strong(tab.title());
        ui.label(tab.meaning());
        let items = view.items.get(&tab).map(Vec::as_slice).unwrap_or(&[]);
        if items.is_empty() {
            ui.label(match tab {
                Tab::NotCatalogued => "Mr Wiz · No uncatalogued game or media files found.",
                _ => "Nothing here.",
            });
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Filter:");
            let text = self.reconcile.filter.entry(tab).or_default();
            if ui
                .add(egui::TextEdit::singleline(text).hint_text("name, folder or category"))
                .changed()
            {
                self.reconcile.page.insert(tab, 0);
            }
        });
        let text = self.reconcile.filter.get(&tab).cloned().unwrap_or_default();
        let shown = match self.reconcile.filter_cache.get(&tab) {
            Some(cache) if Arc::ptr_eq(&cache.view, view) && cache.filter == text => {
                cache.shown.clone()
            }
            _ => {
                let shown = Arc::new(filtered(items, &text));
                self.reconcile.filter_cache.insert(
                    tab,
                    FilterCache {
                        filter: text,
                        view: view.clone(),
                        shown: shown.clone(),
                    },
                );
                shown
            }
        };
        if shown.is_empty() {
            ui.label("No files match this filter.");
            return;
        }
        let pages = shown.len().div_ceil(PAGE_SIZE);
        let page = {
            let slot = self.reconcile.page.entry(tab).or_default();
            *slot = (*slot).min(pages - 1);
            *slot
        };
        let start = page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(shown.len());
        ui.horizontal(|ui| {
            ui.label(format!("Showing {}–{} of {}", start + 1, end, shown.len()));
            if ui
                .add_enabled(page > 0, egui::Button::new("Previous"))
                .clicked()
            {
                self.reconcile.page.insert(tab, page - 1);
            }
            if ui
                .add_enabled(page + 1 < pages, egui::Button::new("Next"))
                .clicked()
            {
                self.reconcile.page.insert(tab, page + 1);
            }
        });
        for &index in &shown[start..end] {
            let item = &items[index];
            ui.push_id(("recon", tab as u8, index), |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.strong(&item.title);
                    ui.label(egui::RichText::new(&item.path).small());
                    if let Some(root) = &item.root {
                        ui.label(egui::RichText::new(format!("Library location: {root}")).small());
                    }
                    if let Some(size) = item.size {
                        ui.label(egui::RichText::new(format!("Size: {}", human_bytes(size))).small());
                    }
                    for line in &item.lines {
                        ui.label(line);
                    }
                    if tab == Tab::SupportFiles {
                        let path = PathBuf::from(&item.path);
                        let running = self.reconcile.support_job.as_ref().map(|(_, p)| p);
                        if running == Some(&path) {
                            ui.label("Checking this file's content…");
                        } else if let Some(run) = self.reconcile.support_checks.get(&path) {
                            for line in support_check_lines(run) {
                                ui.label(line);
                            }
                        } else if ui
                            .add_enabled(
                                running.is_none() && !hashing && self.reconcile.job.is_none(),
                                egui::Button::new("Check content against DAT"),
                            )
                            .on_hover_text("Reads only this file, calculates its checksum and compares it with your installed DAT catalogues. Nothing is changed.")
                            .clicked()
                        {
                            self.start_support_check(path, item.size.unwrap_or(0));
                        }
                    }
                    if tab == Tab::StrongMatches {
                        let open = self.reconcile.open_item == Some((tab, index));
                        if ui.button(if open { "Hide match" } else { "Review match" }).clicked() {
                            self.reconcile.open_item = (!open).then_some((tab, index));
                        }
                        if open {
                            ui.label("Recorded location:");
                            ui.label(egui::RichText::new(&item.path).small());
                            ui.label("Matching file now at:");
                            ui.label(egui::RichText::new(item.new_path.as_deref().unwrap_or("")).small());
                            ui.label("The stored checksum matches exactly and the sizes agree. Nothing has been changed, and relinking is not available yet.");
                        }
                    }
                });
            });
        }
    }

    fn hash_action(&mut self, ui: &mut egui::Ui, view: &Arc<View>, hashing: bool) {
        let n = view.hash_requests.len();
        ui.horizontal_wrapped(|ui| {
            let label = format!("Hash these {n} files to confirm");
            if hashing {
                ui.spinner();
                ui.label("Calculating checksums… this reads the files but changes nothing.");
                if let Some(flag) = self.reconcile.hash_job.map(|id| self.job_cancel_flag(id))
                    && ui.button("Stop").clicked()
                {
                    flag.store(true, Ordering::Relaxed);
                }
            } else if ui
                .add_enabled(self.reconcile.hash_enabled(), egui::Button::new(label))
                .clicked()
            {
                self.start_reconcile_hashing();
            }
        });
        if n > 0 && !hashing {
            ui.label(format!(
                "EmuWiz needs a checksum before it can prove these are the same files. This reads up to {} of file data and only when you ask.",
                human_bytes(HASH_BYTE_BUDGET)
            ));
        }
        match &self.reconcile.hash_note {
            Some(HashNote::Done {
                hashed,
                unreadable,
                over_budget,
            }) => {
                let mut text =
                    format!("Checksums calculated for {hashed} file(s); results refreshed.");
                if *unreadable > 0 {
                    text.push_str(&format!(" {unreadable} could not be read."));
                }
                if *over_budget > 0 {
                    text.push_str(&format!(
                        " {over_budget} were left for another run (size limit)."
                    ));
                }
                ui.label(text);
            }
            Some(HashNote::Stopped { hashed, requested }) => {
                ui.colored_label(theme::warning(ui), format!("Stopped before finishing: {hashed} of {requested} file(s) were checked. The results below do not include a full confirmation."));
            }
            None => {}
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use archivefs_core::PersistedArchive;
    use archivefs_core::catalogue_reconciliation::{
        CompanionLink, FileFacts, RowFacts, RowPresence, StrongHashAlgorithm,
        reconcile_with_companions,
        walk::{RootWalk, RootWalkState},
    };

    fn sha1(n: u8) -> StrongHash {
        StrongHash::new(StrongHashAlgorithm::Sha1, &format!("{n:02x}").repeat(20)).unwrap()
    }
    fn archive(id: i64, title: &str) -> PersistedArchive {
        PersistedArchive {
            id,
            source_folder_id: 1,
            relative_path: format!("{title}.bin").into(),
            absolute_path: format!("/lib/{title}.bin").into(),
            archive_kind: "direct_game_image".into(),
            display_name: title.into(),
            normalized_name: title.into(),
            size_bytes: Some(4),
            modified_time_unix_seconds: Some(1),
            platform: Some("Atari2600".into()),
            platform_source: Some("test".into()),
            last_known_health: "pending".into(),
            last_seen_at: "now".into(),
            last_verified_missing_at: None,
            identity_report: None,
        }
    }
    fn row(id: i64, path: &str, presence: RowPresence, hashes: Vec<StrongHash>) -> RowFacts {
        RowFacts {
            archive_id: id,
            source_id: 1,
            path: path.into(),
            size: Some(4),
            platform: Some("Atari2600".into()),
            presence,
            hashes,
        }
    }
    fn file(path: &str, hashes: Vec<StrongHash>) -> FileFacts {
        FileFacts {
            path: path.into(),
            source_id: 1,
            size: 4,
            hashes,
        }
    }
    fn walk(state: RootWalkState) -> Vec<RootWalk> {
        vec![RootWalk {
            source_id: 1,
            state,
            nested_boundaries: vec![],
            inaccessible: vec![],
            truncated: false,
        }]
    }
    pub(in crate::gui_v2) fn view_of(
        rows: &[RowFacts],
        files: &[FileFacts],
        links: &[CompanionLink],
        state: RootWalkState,
        titles: &[(i64, &str)],
    ) -> View {
        let rec = LibraryReconciliation {
            report: reconcile_with_companions(rows, files, links),
            roots: walk(state),
            root_paths: BTreeMap::from([(1, PathBuf::from("/lib"))]),
        };
        let library = Library::new(titles.iter().map(|(id, t)| archive(*id, t)).collect());
        build_view(&rec, &library)
    }
    fn text(items: &[Item]) -> String {
        items
            .iter()
            .flat_map(|i| i.lines.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn all_zero_is_complete_and_has_no_problems() {
        let view = view_of(&[], &[], &[], RootWalkState::Complete, &[]);
        assert_eq!(view.summary, Summary::default());
        assert!(view.complete());
        assert_eq!(view.problem_count(), 0);
    }

    #[test]
    fn an_uncatalogued_file_is_listed_with_its_root_and_size_and_no_platform() {
        let view = view_of(
            &[],
            &[file("/lib/atari2600/stray.bin", vec![])],
            &[],
            RootWalkState::Complete,
            &[],
        );
        let item = &view.items[&Tab::NotCatalogued][0];
        assert_eq!(item.root.as_deref(), Some("/lib"));
        assert_eq!(item.size, Some(4));
        let words = text(&view.items[&Tab::NotCatalogued]);
        assert!(words.contains("never catalogued this file"));
        assert!(words.contains("No system has been assumed"));
        assert!(
            !words.contains("Atari"),
            "a folder name is never shown as a system: {words}"
        );
    }

    #[test]
    fn a_referenced_companion_is_not_uncatalogued_but_is_listed_as_a_disc_file() {
        let rows = [row(1, "/lib/Game.cue", RowPresence::Present, vec![])];
        let links = [CompanionLink {
            parent_archive_id: 1,
            basis: CompanionBasis::CueFileReference,
            files: vec!["/lib/t1.bin".into()],
        }];
        let view = view_of(
            &rows,
            &[file("/lib/t1.bin", vec![])],
            &links,
            RootWalkState::Complete,
            &[(1, "Game")],
        );
        assert_eq!(view.summary.not_catalogued, 0);
        assert_eq!(view.summary.disc_files, 1);
        assert_eq!(view.problem_count(), 0, "informational, not a problem");
        assert!(text(&view.items[&Tab::DiscFiles]).contains("Part of the disc game \"Game\""));
    }

    #[test]
    fn a_contended_companion_is_a_visible_problem() {
        let rows = [
            row(1, "/lib/A.cue", RowPresence::Present, vec![]),
            row(2, "/lib/B.cue", RowPresence::Present, vec![]),
        ];
        let links: Vec<_> = [1, 2]
            .into_iter()
            .map(|id| CompanionLink {
                parent_archive_id: id,
                basis: CompanionBasis::CueFileReference,
                files: vec!["/lib/t1.bin".into()],
            })
            .collect();
        let view = view_of(
            &rows,
            &[file("/lib/t1.bin", vec![])],
            &links,
            RootWalkState::Complete,
            &[(1, "A"), (2, "B")],
        );
        assert_eq!(view.summary.contended_disc_files, 1);
        assert_eq!(view.summary.not_catalogued, 0);
        let words = text(&view.items[&Tab::NeedsReview]);
        assert!(words.contains("referenced by more than one catalogued disc descriptor"));
        assert!(words.contains("will not guess which one owns it"));
    }

    #[test]
    fn missing_weak_strong_and_ambiguous_entries_each_get_their_own_plain_wording() {
        let mut gone = row(1, "/old/gone.bin", RowPresence::Missing, vec![sha1(1)]);
        gone.size = Some(99);
        let rows = [
            gone,
            row(2, "/old/Weak.bin", RowPresence::Missing, vec![sha1(2)]),
            row(3, "/old/Strong.bin", RowPresence::Missing, vec![sha1(3)]),
            row(4, "/old/Many.bin", RowPresence::Missing, vec![sha1(4)]),
        ];
        let mut files = vec![
            file("/lib/elsewhere/weak.BIN", vec![]),
            file("/lib/new place.bin", vec![sha1(3)]),
        ];
        files.extend((0..20).map(|i| file(&format!("/lib/many{i:02}.bin"), vec![sha1(4)])));
        let view = view_of(
            &rows,
            &files,
            &[],
            RootWalkState::Complete,
            &[(1, "Gone"), (2, "Weak"), (3, "Strong"), (4, "Many")],
        );
        assert!(text(&view.items[&Tab::Missing]).contains("no longer there"));
        let weak = text(&view.items[&Tab::PossibleMoves]);
        assert!(weak.contains("cannot prove it is the same file"));
        assert!(weak.contains("needs a checksum before it can prove this is the same file"));
        assert_eq!(
            view.hash_requests,
            vec![PathBuf::from("/lib/elsewhere/weak.BIN")]
        );
        let strong = &view.items[&Tab::StrongMatches][0];
        assert!(
            strong
                .lines
                .join(" ")
                .contains("same file at a new location. Its stored checksum matches exactly")
        );
        assert_eq!(strong.new_path.as_deref(), Some("/lib/new place.bin"));
        let ambiguous = text(&view.items[&Tab::NeedsReview]);
        assert!(ambiguous.contains("More than one file could match this library entry (20 found)"));
        assert!(
            ambiguous.contains("and 4 more not listed"),
            "true total preserved: {ambiguous}"
        );
        // A system on record is shown only for catalogue entries.
        assert!(
            view.items[&Tab::Missing][0]
                .lines
                .iter()
                .any(|l| l.contains("Atari 2600"))
        );
    }

    #[test]
    fn unavailable_storage_and_partial_coverage_are_never_presented_as_complete() {
        let rows = [row(
            1,
            "/lib/a.bin",
            RowPresence::Unprovable(UnprovableReason::StorageUnavailable),
            vec![],
        )];
        let view = view_of(&rows, &[], &[], RootWalkState::Unavailable, &[(1, "A")]);
        assert!(!view.complete());
        assert!(view.coverage[0].contains("could not be reached"));
        assert!(
            text(&view.items[&Tab::CannotCheck])
                .contains("cannot check this file while its storage location is unavailable")
        );
        assert_eq!(view.summary.missing, 0, "never reported as deleted");
        for state in [RootWalkState::Partial, RootWalkState::Cancelled] {
            assert!(!view_of(&[], &[], &[], state, &[]).complete());
        }
    }

    #[test]
    fn thousands_of_files_are_all_counted_but_pages_are_bounded() {
        let files: Vec<_> = (0..5_000)
            .map(|i| file(&format!("/lib/f{i:05}.bin"), vec![]))
            .collect();
        let view = view_of(&[], &files, &[], RootWalkState::Complete, &[]);
        assert_eq!(view.summary.not_catalogued, 5_000);
        assert_eq!(view.items[&Tab::NotCatalogued].len(), 5_000);
        assert!(PAGE_SIZE <= 100);
    }

    #[test]
    fn hashing_runs_in_batches_reports_progress_and_stops_on_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        std::fs::create_dir_all(&root).unwrap();
        let files: Vec<_> = (0..20)
            .map(|i| {
                let path = root.join(format!("f{i}.bin"));
                std::fs::write(&path, format!("data{i}")).unwrap();
                path
            })
            .collect();
        let mut ticks = Vec::new();
        let run = run_hashing(&[root.clone()], &files, &AtomicBool::new(false), &mut |p| {
            ticks.push(p.completed)
        });
        assert_eq!(run.hashes.len(), 20);
        assert!(!run.cancelled);
        assert_eq!(ticks, vec![0, 8, 16], "honest per-batch progress");
        let stopped = run_hashing(&[root], &files, &AtomicBool::new(true), &mut |_| {});
        assert!(stopped.cancelled);
        assert!(stopped.hashes.is_empty());
    }
}
