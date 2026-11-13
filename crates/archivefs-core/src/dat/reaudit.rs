//! Bounded, resumable re-check of saved DAT evidence (issue #279).
//!
//! Saved rows that are not current evidence (see
//! [`crate::database::SavedDatEvidenceStatus`]) are only ever a list of what
//! to check again. Nothing here copies, back-fills or promotes a saved row:
//! every result that is written comes from reading the game's content now,
//! against the catalogue's bytes now, through the ordinary verification
//! engine ([`super::library_verification`]).
//!
//! Work is cut into bounded sets. Each set is hashed and saved in one
//! database transaction, so the saved rows themselves are the checkpoint:
//! after a stop, a crash or a restart, a new plan lists exactly the rows
//! that still lack current evidence. No "in progress" marker is ever read
//! as a result.
use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::catalogue_selection::ResolvedCatalogue;
use super::configured_catalogues::ConfiguredCatalogue;
use super::library_verification::{
    VerificationFindingKind, VerificationTotals, preview_rows_verification,
};
use super::sources::DatSourceKind;
use super::sources::audit_run::DatAuditProgress;
use crate::Database;
use crate::database::SavedDatEvidenceStatus;
use crate::safe_read::TrustedRoots;

/// One saved row that needs checking again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReauditItem {
    pub archive_id: i64,
    pub path: PathBuf,
    /// The size the library last recorded; used only to bound a set.
    pub size_bytes: Option<u64>,
    /// The saved verdict column. A hint about what to expect, never a result.
    pub saved_verdict: String,
    pub status: SavedDatEvidenceStatus,
}

/// Rows that can be checked together: one catalogue, one system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReauditGroup {
    pub source_id: String,
    pub catalogue_name: String,
    pub platform: String,
    pub items: Vec<ReauditItem>,
}

/// Why a saved row cannot be checked again right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReauditBlock {
    /// The catalogue is not configured, is a folder of several files, or has
    /// no content digest: no check against it could record proof.
    CatalogueUnavailable,
    /// The game has no known system, so no catalogue can be chosen for it.
    SystemUnknown,
    /// The file is missing or its storage is not connected.
    FileUnavailable,
    /// The saved result does not apply to this game.
    DoesNotApply,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReauditBlocked {
    pub archive_id: i64,
    pub source_id: String,
    pub path: PathBuf,
    pub saved_verdict: String,
    pub status: SavedDatEvidenceStatus,
    pub block: ReauditBlock,
}

/// What a re-check would do. Built from saved rows and file metadata only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReauditPlan {
    pub groups: Vec<ReauditGroup>,
    pub blocked: Vec<ReauditBlocked>,
    /// Saved rows that already are current evidence; never re-read.
    pub current: usize,
    pub saved_rows: usize,
}

impl ReauditPlan {
    pub fn games(&self) -> usize {
        self.groups.iter().map(|group| group.items.len()).sum()
    }

    /// Recorded sizes of the games to read. Unknown sizes count as zero.
    pub fn bytes(&self) -> u64 {
        self.groups
            .iter()
            .flat_map(|group| &group.items)
            .filter_map(|item| item.size_bytes)
            .sum()
    }

    /// Games to check for each reason they are not current.
    pub fn by_status(&self) -> BTreeMap<SavedDatEvidenceStatus, usize> {
        let mut counts = BTreeMap::new();
        for item in self.groups.iter().flat_map(|group| &group.items) {
            *counts.entry(item.status).or_insert(0) += 1;
        }
        counts
    }

    /// Keeps at most `limit` games, in plan order.
    pub fn truncated(mut self, limit: usize) -> Self {
        let mut left = limit;
        for group in &mut self.groups {
            group.items.truncate(left);
            left -= group.items.len();
        }
        self.groups.retain(|group| !group.items.is_empty());
        self
    }
}

/// Lists the saved rows that are not current evidence and groups the ones a
/// check can cover. Opens the library read-only; reads no game content.
pub fn plan_reaudit(
    database_path: &Path,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
) -> Result<ReauditPlan, String> {
    let db = Database::open_read_only(database_path).map_err(|e| e.to_string())?;
    let rows = db
        .saved_dat_evidence_status(catalogues)
        .map_err(|e| e.to_string())?;
    let mut plan = ReauditPlan {
        saved_rows: rows.len(),
        ..Default::default()
    };
    let mut groups: BTreeMap<(String, String), Vec<ReauditItem>> = BTreeMap::new();
    for row in rows {
        use SavedDatEvidenceStatus as S;
        let catalogue = catalogues.get(&row.dat_source_id);
        let block = match row.status {
            S::Current => {
                plan.current += 1;
                continue;
            }
            S::DoesNotApply => Some(ReauditBlock::DoesNotApply),
            S::FileUnavailable => Some(ReauditBlock::FileUnavailable),
            S::CatalogueUnavailable => Some(ReauditBlock::CatalogueUnavailable),
            S::MissingProof | S::FileChanged | S::CatalogueChanged => None,
        };
        let block = block.or_else(|| {
            catalogue
                .is_none_or(|c| c.kind != DatSourceKind::File || c.sha256.is_none())
                .then_some(ReauditBlock::CatalogueUnavailable)
        });
        let platform = row
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias);
        let block = block.or(platform.is_none().then_some(ReauditBlock::SystemUnknown));
        // Disconnected storage or a deleted file: nothing can be read now, and
        // the saved row is left exactly as it is.
        let block = block.or_else(|| {
            std::fs::symlink_metadata(&row.path)
                .is_err()
                .then_some(ReauditBlock::FileUnavailable)
        });
        if let Some(block) = block {
            plan.blocked.push(ReauditBlocked {
                archive_id: row.archive_id,
                source_id: row.dat_source_id,
                path: row.path,
                saved_verdict: row.saved_verdict,
                status: row.status,
                block,
            });
            continue;
        }
        groups
            .entry((row.dat_source_id, platform.unwrap_or_default().to_string()))
            .or_default()
            .push(ReauditItem {
                archive_id: row.archive_id,
                path: row.path,
                size_bytes: row.size_bytes,
                saved_verdict: row.saved_verdict,
                status: row.status,
            });
    }
    plan.groups = groups
        .into_iter()
        .map(|((source_id, platform), items)| ReauditGroup {
            catalogue_name: catalogues
                .get(&source_id)
                .map(|c| c.display_name.clone())
                .unwrap_or_default(),
            source_id,
            platform,
            items,
        })
        .collect();
    Ok(plan)
}

/// The configured catalogue stores, loaded once for a plan and its run.
pub struct ReauditCatalogues {
    local: super::sources::DatSourceRegistry,
    managed: super::managed_sources::ManagedDatSources,
    managed_root: PathBuf,
}

impl ReauditCatalogues {
    /// Loads the person's own DAT sources and the managed catalogues from
    /// their usual places. Reads configuration only.
    pub fn load_default() -> Result<Self, String> {
        let config = super::sources::default_dat_sources_config_path()
            .and_then(|path| super::sources::load_dat_sources_config_from(&path))
            .map_err(|e| e.to_string())?;
        let (local, _) = super::sources::DatSourceRegistry::from_config(&config);
        Ok(Self {
            local,
            managed: super::managed_sources::load_managed_dat_sources_default()
                .map_err(|e| e.to_string())?,
            managed_root: super::updates::managed_dat_root().map_err(|e| e.to_string())?,
        })
    }

    fn inputs(&self) -> super::catalogue_selection::CatalogueInventoryInputs<'_> {
        super::catalogue_selection::CatalogueInventoryInputs {
            local_registry: &self.local,
            managed_sources: &self.managed,
            managed_root: &self.managed_root,
            limits: super::limits::DatLimits::default(),
        }
    }

    /// Every usable catalogue with its current content digest.
    pub fn configured(&self) -> HashMap<String, ConfiguredCatalogue> {
        super::configured_catalogues::configured_catalogues(self.inputs())
    }

    /// The catalogue saved rows name by `source_id`, as it is right now.
    pub fn resolve(&self, source_id: &str) -> Result<ResolvedCatalogue, String> {
        super::configured_catalogues::resolve_configured_catalogue(source_id, self.inputs())
    }
}

/// How much one set may hold. A set is the unit of work that is lost when a
/// run stops, and the unit that one changed file can send back for a retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReauditLimits {
    pub max_games_per_set: usize,
    /// By recorded size. One game larger than this still forms its own set.
    pub max_bytes_per_set: u64,
}

impl Default for ReauditLimits {
    fn default() -> Self {
        Self {
            max_games_per_set: 250,
            max_bytes_per_set: 8 * 1024 * 1024 * 1024,
        }
    }
}

/// A game the run looked at and saved nothing for, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReauditNote {
    pub archive_id: i64,
    pub source_id: String,
    pub path: PathBuf,
    pub reason: String,
}

/// A saved match that the content no longer supports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReauditContradiction {
    pub archive_id: i64,
    pub source_id: String,
    pub path: PathBuf,
    pub saved_verdict: String,
    pub now: VerificationFindingKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReauditStop {
    /// Stopped on request. Sets saved before the stop stay saved.
    Cancelled,
    /// A set could not be saved twice running for a reason that is not one
    /// game's fault (the library or catalogue is changing, or the database
    /// is busy). Nothing from that set was saved.
    Interrupted(String),
}

#[derive(Clone, Debug, Default)]
pub struct ReauditReport {
    pub planned: usize,
    pub verified: usize,
    pub no_match: usize,
    pub ambiguous: usize,
    /// Rows written (inserted or replaced) across all saved sets.
    pub saved: usize,
    pub sets_saved: usize,
    pub not_checked: Vec<ReauditNote>,
    pub contradictions: Vec<ReauditContradiction>,
    /// Planned games the run never reached because it stopped.
    pub not_reached: usize,
    pub stopped: Option<ReauditStop>,
    /// The one disc-order refresh after the last saved set.
    pub disc_order: Option<Result<crate::DatDiscOrderOutcome, String>>,
}

#[derive(Clone, Debug)]
pub enum ReauditProgress {
    SetStarted {
        source_id: String,
        platform: String,
        games: usize,
        /// Games in sets already finished, saved or not.
        done: usize,
        planned: usize,
    },
    Audit(DatAuditProgress),
    SetSaved {
        verified: usize,
        no_match: usize,
        ambiguous: usize,
        not_checked: usize,
    },
    SetNotSaved {
        reason: String,
        retrying: bool,
    },
}

/// Held for a whole run so two re-checks of one library never interleave.
struct RunLock {
    _file: File,
}

impl RunLock {
    fn acquire(database_path: &Path) -> Result<Self, String> {
        let mut name = database_path
            .file_name()
            .ok_or("The library database path has no file name.")?
            .to_os_string();
        name.push(".recheck.lock");
        let path = database_path.with_file_name(name);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|e| format!("Could not open the re-check lock {}: {e}", path.display()))?;
        // SAFETY: `file` is an open descriptor owned by this function.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(
                "Another re-check of this library is already running. Nothing was checked.".into(),
            );
        }
        Ok(Self { _file: file })
    }
}

fn sets(items: &[ReauditItem], limits: ReauditLimits) -> Vec<&[ReauditItem]> {
    let max_games = limits.max_games_per_set.max(1);
    let mut out = Vec::new();
    let (mut start, mut bytes) = (0usize, 0u64);
    for (index, item) in items.iter().enumerate() {
        let size = item.size_bytes.unwrap_or(0);
        if index > start
            && (index - start >= max_games || bytes.saturating_add(size) > limits.max_bytes_per_set)
        {
            out.push(&items[start..index]);
            start = index;
            bytes = 0;
        }
        bytes = bytes.saturating_add(size);
    }
    if start < items.len() {
        out.push(&items[start..]);
    }
    out
}

struct SetOutcome {
    totals: Option<VerificationTotals>,
    /// Rows dropped before the saved attempt, with the reason.
    dropped: Vec<(i64, PathBuf, String)>,
    catalogue_sha256: Option<String>,
    failure: Option<String>,
}

/// Runs one set to a single transaction. A file that changes while it is
/// being read refuses the whole save; that file is dropped (and reported)
/// and the rest is tried again. Any other refusal is retried once as is.
#[allow(clippy::too_many_arguments)]
fn run_set(
    database_path: &Path,
    group: &ReauditGroup,
    set: &[ReauditItem],
    resolve: &dyn Fn(&str) -> Result<ResolvedCatalogue, String>,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    progress: &dyn Fn(ReauditProgress),
) -> SetOutcome {
    let mut rows: BTreeMap<i64, PathBuf> = set
        .iter()
        .map(|item| (item.archive_id, item.path.clone()))
        .collect();
    let mut outcome = SetOutcome {
        totals: None,
        dropped: Vec::new(),
        catalogue_sha256: None,
        failure: None,
    };
    let mut plain_retry_used = false;
    loop {
        if rows.is_empty() {
            return outcome;
        }
        if cancel.load(Ordering::Relaxed) {
            outcome.failure = Some("Cancelled. No verification results saved.".into());
            return outcome;
        }
        let attempt = resolve(&group.source_id).and_then(|resolved| {
            preview_rows_verification(
                resolved,
                database_path.to_path_buf(),
                group.platform.clone(),
                rows.clone(),
            )
        });
        let preview = match attempt {
            Ok(preview) => preview,
            Err(reason) => {
                outcome.failure = Some(reason);
                return outcome;
            }
        };
        outcome.catalogue_sha256 = Some(preview.sha256.clone());
        let watch = preview.input_watch();
        match preview.execute(trusted, cancel, &|p| progress(ReauditProgress::Audit(p))) {
            Ok(totals) => {
                outcome.totals = Some(totals);
                return outcome;
            }
            Err(reason) => {
                if cancel.load(Ordering::Relaxed) {
                    outcome.failure = Some(reason);
                    return outcome;
                }
                let changed = watch.changed();
                let mut dropped_now = 0;
                rows.retain(|id, path| {
                    // A changed companion (a disc track, say) belongs to the
                    // row in the same folder that names it; without a cheap
                    // way to tell which, every row beside it is dropped.
                    let hit = changed.iter().any(|c| c == path)
                        || changed.iter().any(|c| {
                            !set.iter().any(|i| &i.path == c) && c.parent() == path.parent()
                        });
                    if hit {
                        dropped_now += 1;
                        outcome.dropped.push((
                            *id,
                            path.clone(),
                            "The file changed while it was being checked".into(),
                        ));
                    }
                    !hit
                });
                let retrying = dropped_now > 0 || !plain_retry_used;
                if dropped_now == 0 {
                    plain_retry_used = true;
                }
                progress(ReauditProgress::SetNotSaved {
                    reason: reason.clone(),
                    retrying,
                });
                if !retrying {
                    outcome.failure = Some(reason);
                    return outcome;
                }
            }
        }
    }
}

/// Checks the planned rows again, one bounded set per transaction.
///
/// Only rows in `plan` are read or written; the plan is never widened. The
/// caller must have shown the plan and been told to go ahead: this writes
/// to the library database. Game files are only ever read.
///
/// Returns `Err` only when nothing was started. Once running, every way a
/// set can fail is reported in the [`ReauditReport`], and every saved set
/// stays saved.
pub fn run_reaudit(
    database_path: &Path,
    plan: &ReauditPlan,
    resolve: &dyn Fn(&str) -> Result<ResolvedCatalogue, String>,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    limits: ReauditLimits,
    progress: &dyn Fn(ReauditProgress),
) -> Result<ReauditReport, String> {
    let _lock = RunLock::acquire(database_path)?;
    let planned = plan.games();
    let mut report = ReauditReport {
        planned,
        ..Default::default()
    };
    let mut done = 0usize;
    'groups: for group in &plan.groups {
        for set in sets(&group.items, limits) {
            if cancel.load(Ordering::Relaxed) {
                report.stopped = Some(ReauditStop::Cancelled);
                break 'groups;
            }
            progress(ReauditProgress::SetStarted {
                source_id: group.source_id.clone(),
                platform: group.platform.clone(),
                games: set.len(),
                done,
                planned,
            });
            let outcome = run_set(
                database_path,
                group,
                set,
                resolve,
                trusted,
                cancel,
                progress,
            );
            let by_path: HashMap<&Path, &ReauditItem> =
                set.iter().map(|item| (item.path.as_path(), item)).collect();
            let mut notes: Vec<(i64, PathBuf, String)> = outcome.dropped;
            if let Some(totals) = &outcome.totals {
                report.verified += totals.verified;
                report.no_match += totals.no_match;
                report.ambiguous += totals.ambiguous;
                report.saved += totals.saved;
                report.sets_saved += 1;
                for finding in &totals.findings {
                    if let Some(item) = by_path.get(finding.path.as_path())
                        && item.saved_verdict == "verified_single_match"
                        && finding.kind != VerificationFindingKind::Verified
                    {
                        report.contradictions.push(ReauditContradiction {
                            archive_id: item.archive_id,
                            source_id: group.source_id.clone(),
                            path: item.path.clone(),
                            saved_verdict: item.saved_verdict.clone(),
                            now: finding.kind,
                        });
                    }
                }
                for unchecked in &totals.unchecked {
                    if let Some(item) = by_path.get(unchecked.path.as_path()) {
                        notes.push((item.archive_id, item.path.clone(), unchecked.reason.clone()));
                    }
                }
                progress(ReauditProgress::SetSaved {
                    verified: totals.verified,
                    no_match: totals.no_match,
                    ambiguous: totals.ambiguous,
                    not_checked: notes.len(),
                });
            }
            let stop = match (&outcome.totals, outcome.failure) {
                (Some(_), _) | (None, None) => None,
                (None, Some(_)) if cancel.load(Ordering::Relaxed) => Some(ReauditStop::Cancelled),
                (None, Some(reason)) => Some(ReauditStop::Interrupted(reason)),
            };
            if stop.is_none() {
                done += set.len();
            }
            // The notes explain, after a restart, why a row is still not
            // current. Failing to write one never undoes a saved set.
            if !notes.is_empty()
                && let Ok(mut db) = Database::open_or_create(database_path)
            {
                let _ = db.record_dat_recheck_attempts(
                    &group.source_id,
                    outcome.catalogue_sha256.as_deref(),
                    &notes,
                );
            }
            report
                .not_checked
                .extend(
                    notes
                        .into_iter()
                        .map(|(archive_id, path, reason)| ReauditNote {
                            archive_id,
                            source_id: group.source_id.clone(),
                            path,
                            reason,
                        }),
                );
            if let Some(stop) = stop {
                // A catalogue that cannot be used stops only its own rows.
                if matches!(&stop, ReauditStop::Interrupted(_))
                    && outcome.catalogue_sha256.is_none()
                {
                    let ReauditStop::Interrupted(reason) = stop else {
                        unreachable!()
                    };
                    report.not_checked.extend(
                        group
                            .items
                            .iter()
                            .skip_while(|item| item.archive_id != set[0].archive_id)
                            .map(|item| ReauditNote {
                                archive_id: item.archive_id,
                                source_id: group.source_id.clone(),
                                path: item.path.clone(),
                                reason: reason.clone(),
                            }),
                    );
                    continue 'groups;
                }
                report.stopped = Some(stop);
                break 'groups;
            }
        }
    }
    report.not_reached = planned
        .saturating_sub(report.verified + report.no_match + report.ambiguous)
        .saturating_sub(report.not_checked.len());
    if report.sets_saved > 0 {
        report.disc_order = Some(
            Database::open_or_create(database_path)
                .and_then(|mut db| db.refresh_dat_disc_order(0))
                .map_err(|e| e.to_string()),
        );
    }
    Ok(report)
}
