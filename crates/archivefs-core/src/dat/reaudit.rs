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
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::catalogue_selection::{CatalogueResolveError, ResolvedCatalogue};
use super::configured_catalogues::ConfiguredCatalogue;
use super::library_identity_summary::{DatVerificationState, PersistedLibraryDatIdentity};
use super::library_verification::{
    VerificationExclusion, VerificationFindingKind, VerificationTotals, preview_rows_verification,
    preview_rows_verification_in,
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
    /// Empty for a game with no saved result.
    pub saved_verdict: String,
    /// Why the saved result is not current; `None` for a game with no saved
    /// result yet (first-time matching, [`plan_first_matching`]).
    pub status: Option<SavedDatEvidenceStatus>,
}

/// Rows that can be checked together: one catalogue, one system.
///
/// A group with a `pool` is first-time matching against every eligible
/// catalogue for one system at once (see [`plan_first_matching`]): then
/// `source_id` is [`catalogue_set_key`] for the system, never a catalogue,
/// and results are saved under each catalogue that matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReauditGroup {
    pub source_id: String,
    pub catalogue_name: String,
    pub platform: String,
    pub items: Vec<ReauditItem>,
    pub pool: Vec<PoolCatalogue>,
}

/// One catalogue of a system's catalogue set, as planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolCatalogue {
    pub source_id: String,
    pub display_name: String,
    pub sha256: String,
}

/// At most this many catalogues are matched together for one system. A
/// larger set waits (it is never cut down to some of its catalogues).
pub const MAX_CATALOGUES_PER_SYSTEM: usize = 512;

/// A set holds at most this many bytes when it is read against a catalogue
/// set: every catalogue reads the set again, so it stays small enough to be
/// served from memory after the first read.
pub const MAX_BYTES_PER_POOL_SET: u64 = 512 * 1024 * 1024;

/// The key re-check notes for a system's catalogue set are kept under.
pub fn catalogue_set_key(platform: &str) -> String {
    format!("catalogue-set:{platform}")
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
    /// The person limited this run to fewer games; this one waits for the
    /// next run.
    LeftOutOfThisRun,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReauditBlocked {
    pub archive_id: i64,
    pub source_id: String,
    pub path: PathBuf,
    pub saved_verdict: String,
    pub status: Option<SavedDatEvidenceStatus>,
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

/// One count per saved DAT result. Every saved result is in exactly one of
/// `current`, `not_applicable`, `skipped` and `eligible`; every eligible one
/// is in exactly one of `verified`, `no_match`, `need_choice`, `failed` and
/// `not_reached`. `completed` is the sum of the first three of those.
///
/// Built from a plan (and, after a run, its report), never from stored
/// totals: the counts are what was found when the plan was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReauditTally {
    /// Every saved DAT result in the library.
    pub saved_results: usize,
    /// Already backed by current proof. Not read again.
    pub current: usize,
    /// Historical results that say nothing about the game they are saved
    /// for (a "not in this catalogue" result from a catalogue for another
    /// system, typically). Kept exactly as they are; there is nothing to
    /// check again.
    pub not_applicable: usize,
    /// Could be checked, but not now: the catalogue is unavailable, the
    /// game's system is not known, or the file cannot be reached.
    pub skipped: usize,
    /// What a check will read: saved results that lost their proof, for a
    /// game whose system the catalogue covers.
    pub eligible: usize,
    /// Eligible games for which a definitive result was reached and saved.
    pub completed: usize,
    pub verified: usize,
    pub no_match: usize,
    /// More than one catalogue entry fits; the person has to choose.
    pub need_choice: usize,
    /// Eligible games the check looked at and could save no result for
    /// (each has a reason). They stay as they were.
    pub failed: usize,
    /// Eligible games the run stopped before.
    pub not_reached: usize,
}

impl ReauditTally {
    /// Whether every saved result is counted exactly once.
    pub fn is_consistent(&self) -> bool {
        self.saved_results == self.current + self.not_applicable + self.skipped + self.eligible
            && self.completed == self.verified + self.no_match + self.need_choice
            && self.eligible == self.completed + self.failed + self.not_reached
    }
}

impl ReauditPlan {
    /// Saved results that do not apply to the game they are saved for.
    pub fn not_applicable(&self) -> usize {
        self.blocked
            .iter()
            .filter(|blocked| blocked.block == ReauditBlock::DoesNotApply)
            .count()
    }

    /// Saved results that cannot be checked now, for a reason that can change.
    pub fn skipped(&self) -> usize {
        self.blocked.len() - self.not_applicable()
    }

    /// The counts before anything has been checked.
    pub fn tally(&self) -> ReauditTally {
        ReauditTally {
            saved_results: self.saved_rows,
            current: self.current,
            not_applicable: self.not_applicable(),
            skipped: self.skipped(),
            eligible: self.games(),
            not_reached: self.games(),
            ..Default::default()
        }
    }

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
        for status in self
            .groups
            .iter()
            .flat_map(|group| &group.items)
            .filter_map(|item| item.status)
        {
            *counts.entry(status).or_insert(0) += 1;
        }
        counts
    }

    /// Keeps at most `limit` games, in plan order. The games left out stay
    /// counted, as skipped: they are simply not part of this run.
    pub fn truncated(mut self, limit: usize) -> Self {
        let mut left = limit;
        for group in &mut self.groups {
            for item in group.items.drain(left.min(group.items.len())..) {
                self.blocked.push(ReauditBlocked {
                    archive_id: item.archive_id,
                    source_id: group.source_id.clone(),
                    path: item.path,
                    saved_verdict: item.saved_verdict,
                    status: item.status,
                    block: ReauditBlock::LeftOutOfThisRun,
                });
            }
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
    Ok(plan_from_rows(rows, catalogues))
}

/// [`plan_reaudit`] for one library game: its own saved rows only.
pub fn plan_reaudit_for_game(
    database_path: &Path,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
    archive_id: i64,
) -> Result<ReauditPlan, String> {
    let db = Database::open_read_only(database_path).map_err(|e| e.to_string())?;
    let rows = db
        .saved_dat_evidence_status_for(catalogues, archive_id)
        .map_err(|e| e.to_string())?;
    Ok(plan_from_rows(rows, catalogues))
}

fn plan_from_rows(
    rows: Vec<crate::database::SavedDatEvidenceRow>,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
) -> ReauditPlan {
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
                status: Some(row.status),
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
                status: Some(row.status),
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
            pool: Vec::new(),
        })
        .collect();
    plan
}

/// Why games with no saved DAT result are not matched for the first time now.
/// Nothing is guessed: each of these waits for the person or for storage.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FirstMatchWait {
    /// The game has no known system, so no catalogue can be chosen for it.
    SystemUnknown,
    /// No usable catalogue is installed for the system.
    NoCatalogue,
    /// More than one usable catalogue fits the system. The person chooses
    /// one (Check Games); none is ranked or picked here.
    NeedsChoice { catalogues: Vec<String> },
    /// The system's one catalogue cannot be used for this now (why).
    CatalogueUnusable(String),
    /// The file is missing or its storage is not connected.
    FileUnavailable,
}

/// Games that wait for the same reason on the same system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirstMatchWaiting {
    pub platform: Option<String>,
    pub wait: FirstMatchWait,
    pub games: usize,
}

/// What first-time matching would do. Built from library rows, saved-row
/// ids and file metadata only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirstMatchPlan {
    /// The games to check, one group per system and its one catalogue. Run
    /// it with [`run_reaudit`], exactly like a re-check.
    pub plan: ReauditPlan,
    /// Library games that already have a saved DAT result for the catalogue
    /// that would be used (or that cannot be planned now and have any saved
    /// result). Left alone here; [`plan_reaudit`] is what checks those again.
    pub already_checked: usize,
    /// Planned games whose only saved results are from other catalogues
    /// (another system's, or one since removed). Those rows are kept as they
    /// are; the game is matched against its own system's catalogue.
    pub checked_elsewhere: usize,
    /// Planned games a previous run already tried against these same
    /// catalogue bytes without saving a result. A limited run takes them
    /// last, so it always reaches games never tried.
    pub retry: BTreeSet<i64>,
    /// Games without a saved result that cannot be matched now, by system
    /// and reason, in a stable order.
    pub waiting: Vec<FirstMatchWaiting>,
    /// Games already checked against every catalogue of their system's
    /// current catalogue set and listed in none, unchanged since. Not read.
    pub not_listed: usize,
    /// Catalogues installed for a system that the check cannot use for it
    /// (not a game list for that system, say), with why: `(system,
    /// catalogue, reason)`. The rest of the system's catalogues are used.
    pub not_eligible: Vec<(String, String, String)>,
}

impl FirstMatchPlan {
    pub fn waiting_games(&self) -> usize {
        self.waiting.iter().map(|waiting| waiting.games).sum()
    }

    /// Keeps at most `limit` games to check; the rest wait for a later run
    /// (counted as left out of this run). Games never tried come first, in
    /// plan order, then games tried before without a result; so repeating a
    /// limited run always moves on instead of retrying the same games.
    pub fn truncated(mut self, limit: usize) -> Self {
        let mut budget = limit;
        let mut keep: Vec<Vec<bool>> = self
            .plan
            .groups
            .iter()
            .map(|group| vec![false; group.items.len()])
            .collect();
        for retried in [false, true] {
            for (group, kept) in self.plan.groups.iter().zip(keep.iter_mut()) {
                for (item, kept) in group.items.iter().zip(kept.iter_mut()) {
                    if budget > 0 && self.retry.contains(&item.archive_id) == retried {
                        *kept = true;
                        budget -= 1;
                    }
                }
            }
        }
        for (group, kept) in self.plan.groups.iter_mut().zip(keep) {
            let items = std::mem::take(&mut group.items);
            for (item, kept) in items.into_iter().zip(kept) {
                if kept {
                    group.items.push(item);
                } else {
                    self.plan.blocked.push(ReauditBlocked {
                        archive_id: item.archive_id,
                        source_id: group.source_id.clone(),
                        path: item.path,
                        saved_verdict: item.saved_verdict,
                        status: item.status,
                        block: ReauditBlock::LeftOutOfThisRun,
                    });
                }
            }
        }
        self.plan.groups.retain(|group| !group.items.is_empty());
        let planned: BTreeSet<i64> = self
            .plan
            .groups
            .iter()
            .flat_map(|group| group.items.iter().map(|item| item.archive_id))
            .collect();
        self.retry.retain(|id| planned.contains(id));
        self
    }

    /// Games left out of this run by a limit.
    pub fn left_out(&self) -> usize {
        self.plan
            .blocked
            .iter()
            .filter(|blocked| blocked.block == ReauditBlock::LeftOutOfThisRun)
            .count()
    }
}

/// A planned group: catalogue name, catalogue set (if any) and games.
type PlannedGroup = (String, Vec<PoolCatalogue>, Vec<ReauditItem>);

/// Lists the library games that have no saved DAT result at all and groups
/// the ones a check can cover: a known system with usable installed
/// catalogues. `for_platform` is the catalogue rule Check Games uses
/// ([`ReauditCatalogues::for_platform`]). When it finds several, none is
/// ranked or picked: the game is matched against every one of them that can
/// be used for the system (a catalogue set; see [`run_reaudit`]), and only a
/// single identity they agree on is ever saved. `resolve` reads one of them
/// by source id, to tell whether the check can use it for the system.
///
/// Opens the library read-only; reads no game content and writes nothing.
pub fn plan_first_matching(
    database_path: &Path,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
    for_platform: &dyn Fn(&str) -> Result<ResolvedCatalogue, CatalogueResolveError>,
    resolve: &dyn Fn(&str) -> Result<ResolvedCatalogue, String>,
) -> Result<FirstMatchPlan, String> {
    plan_first_matching_for(database_path, catalogues, for_platform, resolve, None)
}

/// [`plan_first_matching`] limited to one system: `only` is a canonical
/// platform id ([`crate::canonical_platform_for_alias`]). Games of every
/// other system, and games with no known system, are not looked at and are
/// not counted anywhere in the plan, so the plan describes that system
/// alone. `None` is exactly [`plan_first_matching`].
pub fn plan_first_matching_for(
    database_path: &Path,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
    for_platform: &dyn Fn(&str) -> Result<ResolvedCatalogue, CatalogueResolveError>,
    resolve: &dyn Fn(&str) -> Result<ResolvedCatalogue, String>,
    only: Option<&str>,
) -> Result<FirstMatchPlan, String> {
    let db = Database::open_read_only(database_path).map_err(|e| e.to_string())?;
    let saved = db.saved_dat_result_sources().map_err(|e| e.to_string())?;
    // (game, catalogue, catalogue digest) a previous run tried and saved
    // nothing for.
    let tried: BTreeSet<(i64, String, String)> = db
        .dat_recheck_attempts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|note| {
            note.catalogue_sha256
                .map(|sha| (note.archive_id, note.dat_source_id, sha))
        })
        .collect();
    let not_listed = db.catalogue_set_no_matches().map_err(|e| e.to_string())?;
    let mut archives = db.load_archives().map_err(|e| e.to_string())?;
    archives.sort_by_key(|archive| archive.id);
    let mut first = FirstMatchPlan::default();
    let mut choice: BTreeMap<&'static str, Result<Choice, FirstMatchWait>> = BTreeMap::new();
    let mut groups: BTreeMap<(String, &'static str), PlannedGroup> = BTreeMap::new();
    let mut waiting: BTreeMap<(Option<String>, FirstMatchWait), usize> = BTreeMap::new();
    for archive in archives {
        let platform = archive
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias);
        if only.is_some_and(|only| platform != Some(only)) {
            continue;
        }
        let saved_here = saved.get(&archive.id);
        // A game with any saved result that cannot be planned now is left to
        // the re-check; it is not "waiting" for a first match.
        let mut wait = |key: (Option<String>, FirstMatchWait)| {
            if saved_here.is_some() {
                first.already_checked += 1;
            } else {
                *waiting.entry(key).or_insert(0) += 1;
            }
        };
        let Some(platform) = platform else {
            wait((None, FirstMatchWait::SystemUnknown));
            continue;
        };
        let catalogue = choice.entry(platform).or_insert_with(|| {
            catalogues_for(
                platform,
                catalogues,
                for_platform,
                resolve,
                &mut first.not_eligible,
            )
        });
        let (source_id, catalogue_name, catalogue_sha, pool) = match catalogue {
            Ok(Choice::One {
                source_id,
                name,
                sha256,
            }) => (source_id.clone(), name.clone(), sha256.clone(), Vec::new()),
            Ok(Choice::Set { pool, set_sha256 }) => (
                catalogue_set_key(platform),
                format!("{} catalogues for {platform}", pool.len()),
                set_sha256.clone(),
                pool.clone(),
            ),
            Err(why) => {
                wait((Some(platform.to_string()), why.clone()));
                continue;
            }
        };
        // Already has a result from this very catalogue (or one of the set):
        // the re-check's job.
        if saved_here.is_some_and(|sources| {
            sources.contains(&source_id) || pool.iter().any(|c| sources.contains(&c.source_id))
        }) {
            first.already_checked += 1;
            continue;
        }
        // Disconnected storage, a deleted file, or a row the last scan found
        // missing: nothing can be read now.
        if archive.last_verified_missing_at.is_some()
            || std::fs::symlink_metadata(&archive.absolute_path).is_err()
        {
            wait((Some(platform.to_string()), FirstMatchWait::FileUnavailable));
            continue;
        }
        // Read against this very catalogue set before, listed in none, and
        // nothing about the game, its system or the set has changed since.
        if !pool.is_empty()
            && not_listed.get(&archive.id).is_some_and(|record| {
                record.holds(
                    &archive.absolute_path,
                    archive.size_bytes,
                    platform,
                    &catalogue_sha,
                    archive.last_verified_missing_at.is_some(),
                )
            })
        {
            first.not_listed += 1;
            continue;
        }
        if saved_here.is_some() {
            first.checked_elsewhere += 1;
        }
        if tried.contains(&(archive.id, source_id.clone(), catalogue_sha)) {
            first.retry.insert(archive.id);
        }
        groups
            .entry((source_id, platform))
            .or_insert_with(|| (catalogue_name, pool, Vec::new()))
            .2
            .push(ReauditItem {
                archive_id: archive.id,
                path: archive.absolute_path,
                size_bytes: archive.size_bytes,
                saved_verdict: String::new(),
                status: None,
            });
    }
    first.plan.groups = groups
        .into_iter()
        .map(
            |((source_id, platform), (catalogue_name, pool, items))| ReauditGroup {
                catalogue_name,
                source_id,
                platform: platform.to_string(),
                items,
                pool,
            },
        )
        .collect();
    first.waiting = waiting
        .into_iter()
        .map(|((platform, wait), games)| FirstMatchWaiting {
            platform,
            wait,
            games,
        })
        .collect();
    Ok(first)
}

/// The catalogue(s) first-time matching uses for one system.
#[derive(Clone, Debug)]
enum Choice {
    One {
        source_id: String,
        name: String,
        sha256: String,
    },
    /// Two or more, sorted by source id; `set_sha256` is their
    /// [`crate::database::CatalogueSet`] digest.
    Set {
        pool: Vec<PoolCatalogue>,
        set_sha256: String,
    },
}

/// A catalogue a re-check can resolve again by id, with a content digest to
/// bind the results to: (source id, display name, digest).
fn bindable(
    source_id: &str,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
) -> Option<(String, String, String)> {
    catalogues
        .get(source_id)
        .filter(|c| c.kind == DatSourceKind::File)
        .and_then(|c| {
            c.sha256
                .clone()
                .map(|sha| (source_id.to_string(), c.display_name.clone(), sha))
        })
}

/// The catalogue first-time matching may use for `platform`, or every
/// catalogue of the system when there are several, or why there is none.
///
/// Of several, a catalogue the verification itself would refuse for the
/// system before reading any game (another system's list, a list that is
/// not games, one without content hashes) is left out and reported in
/// `not_eligible`. One that could be used but cannot be bound to its bytes
/// (a folder, no digest) or cannot be read now makes the whole system wait:
/// matching against only some of a system's usable catalogues could miss
/// the one that disagrees.
fn catalogues_for(
    platform: &str,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
    for_platform: &dyn Fn(&str) -> Result<ResolvedCatalogue, CatalogueResolveError>,
    resolve: &dyn Fn(&str) -> Result<ResolvedCatalogue, String>,
    not_eligible: &mut Vec<(String, String, String)>,
) -> Result<Choice, FirstMatchWait> {
    let unbindable = || {
        FirstMatchWait::CatalogueUnusable(
            "the catalogue is not a single readable file with a content digest".into(),
        )
    };
    let candidates = match for_platform(platform) {
        Ok(resolved) => {
            if resolved.ecosystem == super::model::DatEcosystem::MAMEArcade {
                return Err(FirstMatchWait::CatalogueUnusable(
                    "arcade sets are checked with the MAME collection tools".into(),
                ));
            }
            let (source_id, name, sha256) =
                bindable(&resolved.source_id, catalogues).ok_or_else(unbindable)?;
            return Ok(Choice::One {
                source_id,
                name,
                sha256,
            });
        }
        Err(CatalogueResolveError::NoCatalogueForPlatform { .. }) => {
            return Err(FirstMatchWait::NoCatalogue);
        }
        Err(CatalogueResolveError::MultipleCandidates { candidates, .. }) => candidates,
        Err(CatalogueResolveError::PlatformAmbiguity { candidates }) => {
            return Err(FirstMatchWait::CatalogueUnusable(format!(
                "the catalogue names several systems ({}); choose its system first",
                candidates.join(", ")
            )));
        }
        Err(other) => return Err(FirstMatchWait::CatalogueUnusable(other.to_string())),
    };
    if candidates.len() > MAX_CATALOGUES_PER_SYSTEM {
        return Err(FirstMatchWait::CatalogueUnusable(format!(
            "{} catalogues describe this system; at most {MAX_CATALOGUES_PER_SYSTEM} are matched together",
            candidates.len()
        )));
    }
    let mut pool = Vec::new();
    for candidate in candidates {
        let configured = catalogues
            .values()
            .find(|c| c.reference == candidate.reference);
        let Some((source_id, name, sha256)) =
            configured.and_then(|c| bindable(&c.source_id, catalogues))
        else {
            return Err(FirstMatchWait::CatalogueUnusable(format!(
                "{} is not a single readable file with a content digest",
                candidate.display_name
            )));
        };
        let resolved = resolve(&source_id).map_err(|why| {
            FirstMatchWait::CatalogueUnusable(format!("{name} cannot be read now: {why}"))
        })?;
        if resolved.ecosystem == super::model::DatEcosystem::MAMEArcade {
            return Err(FirstMatchWait::CatalogueUnusable(
                "arcade sets are checked with the MAME collection tools".into(),
            ));
        }
        if let Err(why) = super::library_verification::catalogue_fits_platform(&resolved, platform)
        {
            not_eligible.push((platform.to_string(), name, why));
            continue;
        }
        pool.push(PoolCatalogue {
            source_id,
            display_name: name,
            sha256,
        });
    }
    pool.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    match pool.len() {
        0 => Err(FirstMatchWait::CatalogueUnusable(
            "none of the system's catalogues is a game list this check can use".into(),
        )),
        1 => {
            let only = pool.remove(0);
            Ok(Choice::One {
                source_id: only.source_id,
                name: only.display_name,
                sha256: only.sha256,
            })
        }
        _ => {
            let set_sha256 = pool_set(platform, &pool).sha256();
            Ok(Choice::Set { pool, set_sha256 })
        }
    }
}

fn pool_set(platform: &str, pool: &[PoolCatalogue]) -> crate::database::CatalogueSet {
    crate::database::CatalogueSet::new(
        platform,
        pool.iter()
            .map(|c| (c.source_id.clone(), c.sha256.clone()))
            .collect(),
    )
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

    /// The one usable game catalogue for a system, by the rule Check Games
    /// uses: none, exactly one, or several for the person to choose from.
    pub fn for_platform(&self, platform: &str) -> Result<ResolvedCatalogue, CatalogueResolveError> {
        super::catalogue_selection::resolve_catalogue_for_platform(platform, self.inputs())
    }

    /// Catalogues set to a system that [`Self::for_platform`] leaves out
    /// until they are validated: `(system, catalogue name)`.
    pub fn needing_validation(&self) -> Vec<(String, String)> {
        super::catalogue_selection::catalogues_needing_validation(self.inputs())
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
    /// game's or one catalogue's fault (the library is changing, cannot be
    /// read, or the database is busy). Nothing from that set was saved.
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
    /// Things that went wrong beside the checks themselves (a reason that
    /// could not be recorded, results a save withheld). Never a result.
    pub warnings: Vec<String>,
}

impl ReauditReport {
    /// The counts after this run of `plan` (the plan that was run).
    pub fn tally(&self, plan: &ReauditPlan) -> ReauditTally {
        ReauditTally {
            completed: self.verified + self.no_match + self.ambiguous,
            verified: self.verified,
            no_match: self.no_match,
            need_choice: self.ambiguous,
            failed: self.not_checked.len(),
            not_reached: self.not_reached,
            ..plan.tally()
        }
    }
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
    /// A set is being read against catalogue `index` (from 1) of `of` in its
    /// system's catalogue set.
    CatalogueOfSet {
        index: usize,
        of: usize,
        catalogue: String,
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
    /// The failure is about this catalogue or system only (it cannot be
    /// resolved, or does not fit these rows); other groups can still run.
    group_only: bool,
    /// Why a game got no saved result although it was settled (catalogues
    /// disagree, say). Recorded like the other reasons, so a limited run
    /// takes it last next time, but already counted in the totals.
    settled_notes: Vec<(i64, PathBuf, String)>,
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
        group_only: false,
        settled_notes: Vec::new(),
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
                // A preview that fails while the library itself can still be
                // read is about this catalogue or system. One that fails
                // because the library cannot be read is everyone's problem.
                outcome.group_only = outcome.catalogue_sha256.is_none()
                    && Database::open_read_only(database_path).is_ok();
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
                    let own = changed.iter().any(|c| c == path);
                    // A changed companion (a disc track, say) belongs to the
                    // row in the same folder that names it; without a cheap
                    // way to tell which, every row beside it is dropped.
                    let beside = !own
                        && changed.iter().any(|c| {
                            !set.iter().any(|i| &i.path == c) && c.parent() == path.parent()
                        });
                    if own || beside {
                        dropped_now += 1;
                        outcome.dropped.push((
                            *id,
                            path.clone(),
                            if own {
                                "The file changed while it was being checked"
                            } else {
                                "Another file in this game's folder changed while it was being checked"
                            }
                            .into(),
                        ));
                    }
                    !(own || beside)
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

/// What one catalogue of a set said about one game.
#[derive(Clone, Debug)]
enum Seen {
    /// A cryptographic match with exactly one entry, with its proof.
    Verified(Box<PersistedLibraryDatIdentity>),
    /// A CRC-only match: probable, never proof, but it names a game.
    Probable(Option<String>),
    /// Several entries fit, or the evidence conflicts.
    Ambiguous,
    /// A definitive "not in this catalogue", with its proof.
    NoMatch(Box<PersistedLibraryDatIdentity>),
    /// The catalogue was usable and the game was read, or its format is not
    /// one the catalogue lists: no match, but no negative either.
    NoEntry,
    /// The game could not be (fully) read or checked against it.
    Unsettled(String),
}

impl Seen {
    fn of(identity: PersistedLibraryDatIdentity) -> Self {
        let name = identity.canonical.canonical_dat_name.clone();
        match identity.verification_state {
            DatVerificationState::VerifiedSingleMatch { .. } => Self::Verified(Box::new(identity)),
            DatVerificationState::Probable => Self::Probable(name),
            DatVerificationState::AmbiguousMultipleCandidates { .. }
            | DatVerificationState::Conflicting { .. } => Self::Ambiguous,
            DatVerificationState::NoMatch => Self::NoMatch(Box::new(identity)),
            DatVerificationState::FilenameOnlyNotVerified => Self::NoEntry,
            DatVerificationState::NoUsableEvidence => {
                Self::Unsettled("no usable content evidence".into())
            }
        }
    }

    fn of_reason(reason: &str) -> Self {
        use super::library_verification::{
            MEMBERS_WITHOUT_RESULT, NO_DEFINITIVE_RESULT, NOT_IN_CATALOGUE_FORMATS,
        };
        if [
            NOT_IN_CATALOGUE_FORMATS,
            MEMBERS_WITHOUT_RESULT,
            NO_DEFINITIVE_RESULT,
        ]
        .contains(&reason)
        {
            Self::NoEntry
        } else {
            Self::Unsettled(reason.to_string())
        }
    }
}

/// How one game of a set is settled from what every catalogue of its
/// system's set said (`seen`, in set order, one per catalogue).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Settled {
    /// One game identity; these catalogues (indexes) verified it.
    Match(Vec<usize>),
    /// Catalogues disagree, or one lists several entries. Never resolved.
    Ambiguous(String),
    /// Every catalogue was read and none lists the game.
    NotListed(Box<PersistedLibraryDatIdentity>),
    /// No definitive answer; nothing is saved.
    Unsettled(String),
}

fn settle(seen: &[Seen], names: &[String]) -> Settled {
    if let Some((index, Seen::Unsettled(why))) = seen
        .iter()
        .enumerate()
        .find(|(_, s)| matches!(s, Seen::Unsettled(_)))
    {
        return Settled::Unsettled(format!("Not settled by {}: {why}", names[index]));
    }
    if let Some(index) = seen.iter().position(|s| matches!(s, Seen::Ambiguous)) {
        return Settled::Ambiguous(format!(
            "{} lists it more than once, or only in part",
            names[index]
        ));
    }
    let named: Vec<(usize, &Option<String>)> = seen
        .iter()
        .enumerate()
        .filter_map(|(index, s)| match s {
            Seen::Verified(identity) => Some((index, &identity.canonical.canonical_dat_name)),
            Seen::Probable(name) => Some((index, name)),
            _ => None,
        })
        .collect();
    let distinct: BTreeSet<&Option<String>> = named.iter().map(|(_, name)| *name).collect();
    if distinct.len() > 1 || distinct.contains(&None) {
        return Settled::Ambiguous(format!(
            "The catalogues disagree: {}",
            named
                .iter()
                .map(|(index, name)| format!(
                    "{} says {}",
                    names[*index],
                    name.as_deref().unwrap_or("(no name)")
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if !named.is_empty() {
        let verified: Vec<usize> = seen
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, Seen::Verified(_)))
            .map(|(index, _)| index)
            .collect();
        if verified.is_empty() {
            return Settled::Unsettled("Only a CRC match; not verified".into());
        }
        // Every verifying catalogue must have read the same content from the
        // same file object.
        let proofs: Vec<&PersistedLibraryDatIdentity> = verified
            .iter()
            .filter_map(|index| match &seen[*index] {
                Seen::Verified(identity) => Some(identity.as_ref()),
                _ => None,
            })
            .collect();
        if proofs.iter().any(|p| !same_read(p, proofs[0])) {
            return Settled::Unsettled("The file changed while it was being checked".into());
        }
        return Settled::Match(verified);
    }
    // No catalogue matched. A definitive "listed in none" needs at least one
    // proof-bearing negative, and every negative must be about the same
    // content read from the same file object.
    let negatives: Vec<&PersistedLibraryDatIdentity> = seen
        .iter()
        .filter_map(|s| match s {
            Seen::NoMatch(identity) => Some(identity.as_ref()),
            _ => None,
        })
        .collect();
    let Some(first) = negatives.first() else {
        return Settled::Unsettled(
            "No catalogue of the system could give a definitive result for it".into(),
        );
    };
    if first.audit_file.is_none() && first.disc_members.is_empty() {
        return Settled::Unsettled("The file read was not recorded; check it again".into());
    }
    if negatives.iter().any(|n| !same_read(n, first)) {
        return Settled::Unsettled("The file changed while it was being checked".into());
    }
    Settled::NotListed(Box::new((*first).clone()))
}

/// Whether two results were read from the same content of the same file
/// object (hashes compared, outer-file and disc-file fingerprints, and the
/// same archive member). Catalogues may prove one member with different
/// algorithms (MD5 in one list, SHA-1 in another): that is still one read,
/// so the member's matching algorithm is not compared here (#501).
fn same_read(a: &PersistedLibraryDatIdentity, b: &PersistedLibraryDatIdentity) -> bool {
    let member = |identity: &PersistedLibraryDatIdentity| {
        identity.archive_member.as_ref().map(|m| {
            (
                m.member_name.clone(),
                m.member_index,
                m.archive_format.clone(),
            )
        })
    };
    a.audited_hashes == b.audited_hashes
        && a.audit_file == b.audit_file
        && a.disc_members == b.disc_members
        && member(a) == member(b)
}

/// Whether a save read the game exactly as the screen did: the same
/// verified entry, from the same content and file object.
fn same_result(
    saved: &PersistedLibraryDatIdentity,
    screened: &PersistedLibraryDatIdentity,
) -> bool {
    matches!(
        saved.verification_state,
        DatVerificationState::VerifiedSingleMatch { .. }
    ) && saved.verification_state == screened.verification_state
        && saved.canonical == screened.canonical
        && saved.matched_entries == screened.matched_entries
        && saved.archive_member == screened.archive_member
        && same_read(saved, screened)
}

/// What one catalogue said about a game, from its projected result.
/// `member_hit`: a member of the archive matched this catalogue. That is
/// content evidence about the game, so a weaker answer about the archive as
/// a whole (CRC only, filename only, no match, no usable evidence) never
/// replaces it: the answer stays ambiguous unless the archive itself is
/// verified or ambiguous (#501).
fn seen_for_finding(identity: PersistedLibraryDatIdentity, member_hit: bool) -> Seen {
    match Seen::of(identity) {
        seen @ (Seen::Verified(_) | Seen::Ambiguous) => seen,
        _ if member_hit => Seen::Ambiguous,
        seen => seen,
    }
}

struct PoolScreen {
    /// What each catalogue (in set order) said, per archive id.
    seen: BTreeMap<i64, Vec<Seen>>,
    /// The digest each catalogue had when it was read, in set order.
    shas: Vec<String>,
}

enum PoolFailure {
    /// About the catalogue set; the system's other sets would fail alike.
    Group(String),
    /// Files changed (they are dropped and the rest tried again) or the
    /// library could not be read.
    Set {
        reason: String,
        changed: Vec<PathBuf>,
    },
}

/// Reads `rows` against every catalogue of the group's set. Nothing is
/// saved. Fails as a whole when any catalogue cannot be read or a file
/// changes during the reads: a partial read never settles anything.
#[allow(clippy::too_many_arguments)]
fn screen_pool(
    database_path: &Path,
    group: &ReauditGroup,
    rows: &BTreeMap<i64, PathBuf>,
    resolve: &dyn Fn(&str) -> Result<ResolvedCatalogue, String>,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    progress: &dyn Fn(ReauditProgress),
    hash_cache: &Path,
) -> Result<PoolScreen, PoolFailure> {
    let set_failure = |reason: String| PoolFailure::Set {
        reason,
        changed: Vec::new(),
    };
    let library = Database::open_read_only(database_path)
        .and_then(|db| db.load_archives())
        .map_err(|e| set_failure(e.to_string()))?;
    let by_path: HashMap<&Path, i64> = rows.iter().map(|(id, p)| (p.as_path(), *id)).collect();
    let mut screen = PoolScreen {
        seen: rows.keys().map(|id| (*id, Vec::new())).collect(),
        shas: Vec::new(),
    };
    let mut watch = None;
    for (index, catalogue) in group.pool.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(set_failure(
                "Cancelled. No verification results saved.".into(),
            ));
        }
        progress(ReauditProgress::CatalogueOfSet {
            index: index + 1,
            of: group.pool.len(),
            catalogue: catalogue.display_name.clone(),
        });
        let preview = resolve(&catalogue.source_id)
            .and_then(|resolved| {
                preview_rows_verification_in(
                    resolved,
                    database_path.to_path_buf(),
                    group.platform.clone(),
                    rows.clone(),
                    &library,
                )
            })
            .map_err(|why| {
                PoolFailure::Group(format!(
                    "{} cannot be used now: {why}",
                    catalogue.display_name
                ))
            })?;
        // The first catalogue's view of the files is the one every later
        // read is compared with, after the last of them.
        let watch = watch.get_or_insert_with(|| preview.input_watch());
        let screened = preview
            .screen(trusted, cancel, hash_cache)
            .map_err(|reason| PoolFailure::Set {
                reason,
                changed: watch.changed(),
            })?;
        screen.shas.push(preview.sha256.clone());
        let mut this: BTreeMap<i64, Seen> = BTreeMap::new();
        for exclusion in &screened.unchecked {
            if let Some(id) = by_path.get(exclusion.path.as_path()) {
                // A member of an archive matched this catalogue, but the
                // archive as a whole got no result (a member listed twice,
                // one disk of several, say): that is evidence about the game,
                // never "not listed".
                let seen = if screened.member_hits.contains(&exclusion.path) {
                    Seen::Ambiguous
                } else {
                    Seen::of_reason(&exclusion.reason)
                };
                this.insert(*id, seen);
            }
        }
        for finding in screened.findings {
            if let Some(id) = by_path.get(finding.path.as_path()) {
                let member_hit = screened.member_hits.contains(&finding.path);
                this.insert(*id, seen_for_finding(finding.identity, member_hit));
            }
        }
        for (id, seen) in &mut screen.seen {
            seen.push(
                this.remove(id)
                    .unwrap_or_else(|| Seen::Unsettled("no result was reported".into())),
            );
        }
    }
    if let Some(watch) = watch {
        let changed = watch.changed();
        if !changed.is_empty() {
            return Err(PoolFailure::Set {
                reason: "A file changed while it was being checked".into(),
                changed,
            });
        }
    }
    Ok(screen)
}

/// One set read against every catalogue of its system's set, settled per
/// game ([`settle`]) and saved:
/// - one identity the catalogues agree on: the ordinary verification is run
///   and saved for each catalogue that verified it, exactly as a check
///   against that catalogue alone would save it (its own source id, bytes
///   and proof);
/// - listed in none: one catalogue-set record for the game (schema 29);
/// - catalogues disagree or one is ambiguous: nothing saved, a reason kept;
/// - anything else: nothing saved, a reason kept.
#[allow(clippy::too_many_arguments)]
fn run_pool_set(
    database_path: &Path,
    group: &ReauditGroup,
    set: &[ReauditItem],
    resolve: &dyn Fn(&str) -> Result<ResolvedCatalogue, String>,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
    progress: &dyn Fn(ReauditProgress),
    hash_cache: &Path,
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
        group_only: false,
        settled_notes: Vec::new(),
    };
    let names: Vec<String> = group.pool.iter().map(|c| c.display_name.clone()).collect();
    let mut plain_retry_used = false;
    let screen = loop {
        if rows.is_empty() {
            return outcome;
        }
        if cancel.load(Ordering::Relaxed) {
            outcome.failure = Some("Cancelled. No verification results saved.".into());
            return outcome;
        }
        match screen_pool(
            database_path,
            group,
            &rows,
            resolve,
            trusted,
            cancel,
            progress,
            hash_cache,
        ) {
            Ok(screen) => break screen,
            Err(PoolFailure::Group(reason)) => {
                outcome.group_only = Database::open_read_only(database_path).is_ok();
                outcome.failure = Some(reason);
                return outcome;
            }
            Err(PoolFailure::Set { reason, changed }) => {
                if cancel.load(Ordering::Relaxed) {
                    outcome.failure = Some(reason);
                    return outcome;
                }
                let before = rows.len();
                rows.retain(|id, path| {
                    let own = changed.iter().any(|c| c == path);
                    let beside = !own
                        && changed.iter().any(|c| {
                            !set.iter().any(|i| &i.path == c) && c.parent() == path.parent()
                        });
                    if own || beside {
                        outcome.dropped.push((
                            *id,
                            path.clone(),
                            if own {
                                "The file changed while it was being checked"
                            } else {
                                "Another file in this game's folder changed while it was being checked"
                            }
                            .into(),
                        ));
                    }
                    !(own || beside)
                });
                let dropped_now = before - rows.len();
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
    };
    let catalogue_set = crate::database::CatalogueSet::new(
        &group.platform,
        group
            .pool
            .iter()
            .zip(&screen.shas)
            .map(|(c, sha)| (c.source_id.clone(), sha.clone()))
            .collect(),
    );
    outcome.catalogue_sha256 = Some(catalogue_set.sha256());
    let mut totals = VerificationTotals {
        catalogue_sha256: catalogue_set.sha256(),
        ..Default::default()
    };
    let mut to_save: BTreeMap<usize, BTreeMap<i64, PathBuf>> = BTreeMap::new();
    let mut not_listed = Vec::new();
    for (id, seen) in &screen.seen {
        let path = rows[id].clone();
        match settle(seen, &names) {
            Settled::Match(verified) => {
                for index in verified {
                    to_save.entry(index).or_default().insert(*id, path.clone());
                }
            }
            Settled::Ambiguous(why) => {
                totals.ambiguous += 1;
                outcome.settled_notes.push((*id, path, why));
            }
            Settled::NotListed(facts) => not_listed.push((*id, path, *facts)),
            Settled::Unsettled(why) => totals
                .unchecked
                .push(VerificationExclusion { path, reason: why }),
        }
    }
    // Each catalogue's own verification, run and saved as usual, but only
    // when it reads every one of its games exactly as the screen did: the
    // same catalogue bytes, the same entry, the same content from the same
    // file object. Anything else saves nothing from that catalogue. A save
    // that fails leaves the others saved.
    let mut verified: BTreeSet<i64> = BTreeSet::new();
    let mut save_failed: BTreeMap<i64, String> = BTreeMap::new();
    for (index, chosen) in &to_save {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let catalogue = &group.pool[*index];
        let expected: HashMap<&Path, &PersistedLibraryDatIdentity> = chosen
            .iter()
            .filter_map(
                |(id, path)| match screen.seen.get(id).and_then(|s| s.get(*index)) {
                    Some(Seen::Verified(identity)) => Some((path.as_path(), identity.as_ref())),
                    _ => None,
                },
            )
            .collect();
        let saved = resolve(&catalogue.source_id)
            .and_then(|resolved| {
                preview_rows_verification(
                    resolved,
                    database_path.to_path_buf(),
                    group.platform.clone(),
                    chosen.clone(),
                )
            })
            .and_then(|preview| {
                if preview.sha256 != screen.shas[*index] {
                    return Err(format!(
                        "{} changed while the games were checked. Check again.",
                        catalogue.display_name
                    ));
                }
                preview.execute_expecting(
                    trusted,
                    cancel,
                    &|p| progress(ReauditProgress::Audit(p)),
                    &|path, read| {
                        expected
                            .get(path)
                            .is_some_and(|screened| same_result(read, screened))
                    },
                )
            });
        match saved {
            Ok(result) => {
                totals.saved += result.saved;
                if let Some(withheld) = result.save_withheld {
                    totals.save_withheld = Some(withheld);
                }
                let by_path: HashMap<&Path, i64> =
                    chosen.iter().map(|(id, p)| (p.as_path(), *id)).collect();
                for finding in &result.findings {
                    if finding.kind == VerificationFindingKind::Verified
                        && let Some(id) = by_path.get(finding.path.as_path())
                    {
                        verified.insert(*id);
                    }
                }
            }
            Err(why) => {
                for id in chosen.keys() {
                    save_failed.entry(*id).or_insert_with(|| why.clone());
                }
            }
        }
    }
    // A stop during the saves: what was saved stays saved; nothing else is
    // recorded, and the rest is simply not reached.
    let stopped = cancel.load(Ordering::Relaxed);
    let matched: BTreeSet<i64> = to_save.values().flat_map(|c| c.keys().copied()).collect();
    if !stopped {
        for id in &matched {
            if !verified.contains(id) {
                totals.unchecked.push(VerificationExclusion {
                    path: rows[id].clone(),
                    reason: save_failed.get(id).cloned().unwrap_or_else(|| {
                        "The result changed between the check and the save; check it again".into()
                    }),
                });
            }
        }
    }
    totals.verified = verified.len();
    let verified_ids: Vec<i64> = verified.iter().copied().collect();
    let not_listed = if stopped { Vec::new() } else { not_listed };
    if !not_listed.is_empty() || !verified_ids.is_empty() {
        match Database::open_existing(database_path).and_then(|mut db| {
            db.record_catalogue_set_no_matches(&catalogue_set, &not_listed, &verified_ids)
        }) {
            Ok(written) => {
                totals.no_match = written.len();
                totals.saved += written.len();
                for (id, path, _) in &not_listed {
                    if !written.contains(id) {
                        totals.unchecked.push(VerificationExclusion {
                            path: path.clone(),
                            reason: "Not saved as listed in no catalogue: the game moved or now has a result".into(),
                        });
                    }
                }
            }
            Err(error) => {
                for (_, path, _) in &not_listed {
                    totals.unchecked.push(VerificationExclusion {
                        path: path.clone(),
                        reason: format!(
                            "The \"listed in no catalogue\" result could not be saved: {error}"
                        ),
                    });
                }
            }
        }
    }
    if stopped {
        // Settled-but-unsaved reasons are only kept for a finished set.
        outcome.settled_notes.clear();
        totals.ambiguous = 0;
        totals.unchecked.clear();
    }
    outcome.totals = Some(totals);
    outcome
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
    // Loose-file hashes shared by the catalogues of one set; private to this
    // run and removed with it.
    let hash_cache = if plan.groups.iter().any(|g| !g.pool.is_empty()) {
        Some(tempfile::tempdir().map_err(|e| format!("Could not prepare the check: {e}"))?)
    } else {
        None
    };
    'groups: for group in &plan.groups {
        let group_limits = if group.pool.is_empty() {
            limits
        } else {
            ReauditLimits {
                max_bytes_per_set: limits.max_bytes_per_set.min(MAX_BYTES_PER_POOL_SET),
                ..limits
            }
        };
        for set in sets(&group.items, group_limits) {
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
            let outcome = match &hash_cache {
                Some(cache) if !group.pool.is_empty() => run_pool_set(
                    database_path,
                    group,
                    set,
                    resolve,
                    trusted,
                    cancel,
                    progress,
                    &cache.path().join("loose-hashes.json"),
                ),
                _ => run_set(
                    database_path,
                    group,
                    set,
                    resolve,
                    trusted,
                    cancel,
                    progress,
                ),
            };
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
            let group_failure = match (&stop, outcome.group_only) {
                (Some(ReauditStop::Interrupted(reason)), true) => Some(reason.clone()),
                _ => None,
            };
            if let Some(reason) = &group_failure {
                // This catalogue or system cannot be used: none of the
                // group's remaining rows will be read in this run.
                notes.extend(
                    group
                        .items
                        .iter()
                        .skip_while(|item| item.archive_id != set[0].archive_id)
                        .map(|item| (item.archive_id, item.path.clone(), reason.clone())),
                );
            }
            if let Some(withheld) = outcome
                .totals
                .as_ref()
                .and_then(|t| t.save_withheld.clone())
            {
                report.warnings.push(withheld);
            }
            // The notes explain, after a restart, why a row is still not
            // current. Failing to write one never undoes a saved set, but
            // it is reported.
            if !notes.is_empty() || !outcome.settled_notes.is_empty() {
                let all: Vec<_> = notes
                    .iter()
                    .chain(&outcome.settled_notes)
                    .cloned()
                    .collect();
                let recorded = Database::open_existing(database_path).and_then(|mut db| {
                    db.record_dat_recheck_attempts(
                        &group.source_id,
                        outcome.catalogue_sha256.as_deref(),
                        &all,
                    )
                });
                if let Err(error) = recorded {
                    report.warnings.push(format!(
                        "The reasons {} games were not checked could not be recorded: {error}",
                        notes.len()
                    ));
                }
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
            if group_failure.is_some() {
                continue 'groups;
            }
            if let Some(stop) = stop {
                report.stopped = Some(stop);
                break 'groups;
            }
            // Stopped while this set was being saved: what it saved stays.
            if cancel.load(Ordering::Relaxed) {
                report.stopped = Some(ReauditStop::Cancelled);
                break 'groups;
            }
        }
    }
    report.not_reached = planned
        .saturating_sub(report.verified + report.no_match + report.ambiguous)
        .saturating_sub(report.not_checked.len());
    if report.sets_saved > 0 {
        report.disc_order = Some(
            Database::open_existing(database_path)
                .and_then(|mut db| db.refresh_dat_disc_order(0))
                .map_err(|e| e.to_string()),
        );
    }
    Ok(report)
}

/// Re-checking one game's saved catalogue results, run after Blue renames a
/// game (and after that rename is undone): a saved result is tied to the
/// file's name, so it stops counting until the file is read again.
#[derive(Clone, Debug)]
pub struct GameRecheck {
    pub plan: ReauditPlan,
    /// `None` when nothing needed reading.
    pub report: Option<ReauditReport>,
}

impl GameRecheck {
    /// One plain sentence for the person; `None` when the game has no saved
    /// catalogue result at all (so there was nothing to keep verified).
    pub fn summary(&self) -> Option<String> {
        const LATER: &str = "It shows as not verified until you re-check it (Re-check this game).";
        if self.plan.saved_rows == 0 {
            return None;
        }
        let blocked = self
            .plan
            .blocked
            .iter()
            .find(|b| b.block != ReauditBlock::DoesNotApply);
        let Some(report) = &self.report else {
            return Some(match blocked.map(|b| &b.block) {
                Some(ReauditBlock::CatalogueUnavailable) => {
                    format!("Its catalogue is not available now, so it was not re-checked. {LATER}")
                }
                Some(ReauditBlock::FileUnavailable) => {
                    format!("Its file could not be reached, so it was not re-checked. {LATER}")
                }
                Some(ReauditBlock::SystemUnknown) => {
                    format!("Its system is not known, so it was not re-checked. {LATER}")
                }
                _ => "Its catalogue result is still current.".into(),
            });
        };
        Some(if report.verified > 0 {
            "Re-checked against its catalogue: still verified.".into()
        } else if report.no_match > 0 {
            "Re-checked: the file no longer matches its catalogue entry, so it is not verified. The earlier result is kept in its history.".into()
        } else if report.ambiguous > 0 {
            "Re-checked: more than one catalogue entry fits, so it is not verified until you choose one (Re-check this game).".into()
        } else if let Some(note) = report.not_checked.first() {
            format!("The re-check could not finish: {}. {LATER}", note.reason)
        } else if let Some(ReauditStop::Interrupted(why)) = &report.stopped {
            format!("The re-check could not finish: {why}. {LATER}")
        } else {
            format!("The re-check produced no result. {LATER}")
        })
    }
}

/// Re-checks only `archive_id`'s saved results that are no longer current,
/// through the ordinary re-check engine ([`run_reaudit`]): the file is read
/// again against the catalogue's bytes now, and nothing is carried over
/// without that. Replaced results are kept in history by the engine's rules.
pub fn recheck_game(
    database_path: &Path,
    archive_id: i64,
    catalogues: &HashMap<String, ConfiguredCatalogue>,
    resolve: &dyn Fn(&str) -> Result<ResolvedCatalogue, String>,
    trusted: &TrustedRoots,
    cancel: &AtomicBool,
) -> Result<GameRecheck, String> {
    let plan = plan_reaudit_for_game(database_path, catalogues, archive_id)?;
    let report = if plan.games() == 0 {
        None
    } else {
        Some(run_reaudit(
            database_path,
            &plan,
            resolve,
            trusted,
            cancel,
            ReauditLimits::default(),
            &|_| {},
        )?)
    };
    Ok(GameRecheck { plan, report })
}
