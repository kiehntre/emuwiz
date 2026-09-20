use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::{
    ArchiveFsError, CatalogueDuplicateReport, CatalogueStats, CompletedScanSummary, Config,
    Database, DatabaseHealth, DatabaseUpgradeReport, PersistedArchive, PlatformAlias,
    PlatformProvenanceDetails, RecentScanAdditions, ScanPersistSummary, SourceFolderView,
};
use eframe::egui;

use super::{
    ActionFeedback, ActivityAction, ActivityOutcome, HistoryEntry, LibraryDatIdentitySummary,
    build_source_folder_views, catalogue_filename_duplicates, check_database_health,
    default_config_path, default_database_path, format_database_upgrade_success,
    format_scan_activity, latest_schema_version, load_source_folder_configs_from, scan_and_persist,
    upgrade_library_database,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DatabaseGeneration(pub(crate) u64);

impl DatabaseGeneration {
    pub(crate) const INITIAL: Self = Self(0);

    pub(crate) fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// A read-only snapshot of the persisted library catalogue: every row
/// `Database::load_archives` returned, plus aggregate stats and the most
/// recent completed scan, all read from one opened `Database` handle in
/// one background pass.
#[derive(Debug, Clone)]
pub(crate) struct CachedLibrarySnapshot {
    pub(crate) database_path: PathBuf,
    pub(crate) schema_version: i64,
    pub(crate) archives: Vec<PersistedArchive>,
    /// Current trusted topology evidence. Empty means unavailable/stale and
    /// deliberately causes the media-set page to retain its bounded fallback.
    pub(crate) media_topology: Vec<archivefs_core::media_set::MediaSet>,
    /// Reconstructed persisted DAT identity, keyed by archive id. Rendering
    /// reads this cache and never audits, hashes, or opens content.
    pub(crate) dat_identities: HashMap<i64, Vec<LibraryDatIdentitySummary>>,
    pub(crate) platform_details: HashMap<i64, PlatformProvenanceDetails>,
    pub(crate) stats: CatalogueStats,
    pub(crate) last_completed_scan: Option<CompletedScanSummary>,
    pub(crate) recently_found: Option<RecentScanAdditions>,
    pub(crate) platform_aliases: Vec<PlatformAlias>,
    /// Computed once on the database worker whenever this snapshot is
    /// loaded. Rendering only filters/sorts these cached groups; it never
    /// reruns duplicate detection per frame.
    pub(crate) duplicate_report: CatalogueDuplicateReport,
    /// Every configured source folder's merged config+database view - the
    /// Sources page's data, computed in this same background pass so the
    /// existing pointer-identity health-cache invalidation (see
    /// `HealthReportCacheKey`) automatically covers source config/status
    /// changes too, with no separate cache to keep in sync. Empty (never
    /// a load failure) if the config file cannot be read at this moment -
    /// source management is additive display data, not required for
    /// Library/Health/Duplicates to function.
    pub(crate) source_views: Vec<SourceFolderView>,
    /// Provider-neutral mod records imported explicitly by a caller. This is
    /// metadata only; records without local payload bytes remain browse-only.
    pub(crate) mod_catalogue_records: Vec<archivefs_core::mod_catalogue::ModCatalogueRecord>,
    /// Explicitly accepted ScreenScraper descriptive metadata, keyed by
    /// archive id. This is presentation enrichment only and is never fed to
    /// identity/platform resolution.
    pub(crate) screenscraper_enrichments:
        HashMap<i64, archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment>,
}

// A one-shot value moved straight out of a worker channel
// (`DatabaseLoadResult`) and destructured by its single consumer; it is
// never stored or held in a collection, so the size gap does not matter.
#[allow(clippy::large_enum_variant)]
pub(crate) enum DatabaseOutcome {
    Loaded(CachedLibrarySnapshot),
    Scanned {
        snapshot: CachedLibrarySnapshot,
        scan_summary: ScanPersistSummary,
        upgrade: Option<DatabaseUpgradeReport>,
    },
}

pub(crate) enum DatabaseLoadError {
    NotCreated { database_path: PathBuf },
    Outdated { health: DatabaseHealth },
    Failed { message: String },
}

pub(crate) type DatabaseLoadResult = Result<DatabaseOutcome, DatabaseLoadError>;
pub(crate) type DatabaseMessage = (DatabaseGeneration, DatabaseLoadResult);

/// The Library Database status area's state - see requirement 3's exact
/// vocabulary ("Not created / Loading / Ready / Outdated / Error").
// Exactly one instance of this lives in `ArchiveFsApp`; the recoverable
// snapshots are already boxed. Boxing the remaining inline
// `ScanPersistSummary` would touch every read of `last_scan_summary` to
// save ~300 bytes once, which is not worth it here.
#[allow(clippy::large_enum_variant)]
pub(crate) enum DatabaseState {
    NotCreated {
        database_path: PathBuf,
    },
    Loading {
        generation: DatabaseGeneration,
        receiver: Receiver<DatabaseMessage>,
        worker: Option<thread::JoinHandle<()>>,
        previous: Option<Box<CachedLibrarySnapshot>>,
        scanning: bool,
    },
    Ready {
        snapshot: Box<CachedLibrarySnapshot>,
        last_scan_summary: Option<ScanPersistSummary>,
    },
    Outdated {
        health: DatabaseHealth,
        previous: Option<Box<CachedLibrarySnapshot>>,
    },
    Error {
        message: String,
        previous: Option<Box<CachedLibrarySnapshot>>,
    },
}

impl DatabaseState {
    /// The most recent known-good cached snapshot regardless of the
    /// current state, so a failed reload never discards useful data
    /// already on screen (requirement 7: retain the last useful database
    /// catalogue where safe).
    pub(crate) fn snapshot(&self) -> Option<&CachedLibrarySnapshot> {
        match self {
            Self::Ready { snapshot, .. } => Some(snapshot),
            Self::Loading { previous, .. }
            | Self::Outdated { previous, .. }
            | Self::Error { previous, .. } => previous.as_deref(),
            Self::NotCreated { .. } => None,
        }
    }

    pub(crate) fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }

    pub(crate) fn is_scanning(&self) -> bool {
        matches!(self, Self::Loading { scanning: true, .. })
    }

    pub(crate) fn status_label(&self) -> &'static str {
        match self {
            Self::NotCreated { .. } => "Not created",
            Self::Loading { .. } => "Loading",
            Self::Ready { .. } => "Ready",
            Self::Outdated { .. } => "Outdated",
            Self::Error { .. } => "Error",
        }
    }
}

pub(crate) fn start_database_load(
    context: egui::Context,
    generation: DatabaseGeneration,
    previous: Option<Box<CachedLibrarySnapshot>>,
    run_scan_first: bool,
) -> DatabaseState {
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        let result = load_database_snapshot(run_scan_first);
        let _ = sender.send((generation, result));
        context.request_repaint();
    });
    DatabaseState::Loading {
        generation,
        receiver,
        worker: Some(worker),
        previous,
        scanning: run_scan_first,
    }
}

pub(crate) fn load_database_snapshot(run_scan_first: bool) -> DatabaseLoadResult {
    let database_path = default_database_path().map_err(|error| DatabaseLoadError::Failed {
        message: error.to_string(),
    })?;
    let config_path = default_config_path().map_err(|error| DatabaseLoadError::Failed {
        message: error.to_string(),
    })?;

    let scan_config = if run_scan_first {
        Some(
            Config::load_default().map_err(|error| DatabaseLoadError::Failed {
                message: error.to_string(),
            })?,
        )
    } else {
        None
    };

    load_database_snapshot_at(&database_path, &config_path, scan_config.as_ref())
}

/// The logic behind [`load_database_snapshot`], taking the already-resolved
/// database path (and, for a scan, the already-loaded config) as
/// parameters instead of reading `HOME`/the default config path itself -
/// the same split `resolve_database_path` uses in
/// `archivefs-core/src/database.rs`, so tests can exercise every branch
/// against a temporary database path without touching the real home
/// directory. `config_path` is used only for `source_views` (see
/// `CachedLibrarySnapshot`'s doc comment) - a missing or unreadable config
/// at this path never fails the whole snapshot load.
pub(crate) fn load_database_snapshot_at(
    database_path: &Path,
    config_path: &Path,
    scan_config: Option<&Config>,
) -> DatabaseLoadResult {
    if let Some(config) = scan_config {
        // A scan is an explicit write-authorized action. If its read-only
        // preflight finds an older database, preserve and verify a consistent
        // SQLite backup before allowing the existing migration chain to run.
        // The normal no-scan load below never reaches this branch.
        let upgrade = if database_path.is_file() {
            let health = check_database_health(database_path);
            if health.migrations_current {
                None
            } else {
                Some(upgrade_library_database(database_path).map_err(|error| {
                    DatabaseLoadError::Failed {
                        message: error.to_string(),
                    }
                })?)
            }
        } else {
            None
        };
        let mut database =
            Database::open_or_create(database_path).map_err(|error| DatabaseLoadError::Failed {
                message: error.to_string(),
            })?;
        let scan_summary =
            scan_and_persist(&mut database, config, "gui-scan-library").map_err(|error| {
                DatabaseLoadError::Failed {
                    message: error.to_string(),
                }
            })?;
        let snapshot = load_snapshot_from(&database, database_path, config_path)?;
        return Ok(DatabaseOutcome::Scanned {
            snapshot,
            scan_summary,
            upgrade,
        });
    }

    let health = check_database_health(database_path);
    if !health.database_exists {
        return Err(DatabaseLoadError::NotCreated {
            database_path: database_path.to_path_buf(),
        });
    }
    if !health.migrations_current {
        return Err(classify_unhealthy_database(health));
    }

    let database =
        Database::open_read_only(database_path).map_err(|error| DatabaseLoadError::Failed {
            message: error.to_string(),
        })?;
    let snapshot = load_snapshot_from(&database, database_path, config_path)?;
    Ok(DatabaseOutcome::Loaded(snapshot))
}

/// Turns a `DatabaseHealth` that is not `migrations_current` into the
/// right `DatabaseLoadError` (requirement 7): a database that will not
/// even open is a hard `Failed`, one whose schema is *newer* than this
/// build understands is also `Failed` (with an explicit upgrade message,
/// not a silent "just run a scan"), and everything else - a database that
/// merely has pending migrations - is `Outdated`, which the caller can
/// offer to fix with a scan. `check_database_health` guarantees
/// `database_opens = false` implies `migrations_current = false`, so this
/// is only ever called when at least one of these three applies.
pub(crate) fn classify_unhealthy_database(health: DatabaseHealth) -> DatabaseLoadError {
    // `database_opens` alone is not enough to rule out a corrupt file:
    // Connection::open is lazy, so a garbage file still "opens" and only
    // fails once something actually reads page 1 - `health.error` carries
    // that failure through (see check_database_health) even when
    // `database_opens` is true.
    if !health.database_opens || health.error.is_some() {
        return DatabaseLoadError::Failed {
            message: health
                .error
                .clone()
                .unwrap_or_else(|| "the database could not be opened".to_string()),
        };
    }
    if let Some(version) = health.schema_version
        && version > latest_schema_version()
    {
        return DatabaseLoadError::Failed {
            message: format!(
                "This database's schema (version {version}) is newer than this build of \
                 EmuWiz supports (version {}). Upgrade EmuWiz, or remove the database \
                 file to rebuild it.",
                latest_schema_version()
            ),
        };
    }
    DatabaseLoadError::Outdated { health }
}

pub(crate) fn load_snapshot_from(
    database: &Database,
    database_path: &Path,
    config_path: &Path,
) -> Result<CachedLibrarySnapshot, DatabaseLoadError> {
    let to_failed = |error: ArchiveFsError| DatabaseLoadError::Failed {
        message: error.to_string(),
    };
    let schema_version = database.schema_version().map_err(to_failed)?;
    let archives = database.load_archives().map_err(to_failed)?;
    let media_topology = database
        .load_media_topology_evidence(&archives)
        .map_err(to_failed)?;
    let screenscraper_enrichments = database
        .load_screenscraper_enrichments()
        .map_err(to_failed)?
        .into_iter()
        .map(|item| (item.archive_id, item))
        .collect();
    let configured_dat_sources =
        archivefs_core::dat::sources::load_dat_sources_config_from(config_path)
            .ok()
            .and_then(|config| config.sources)
            .unwrap_or_default()
            .into_iter()
            .filter(|source| source.enabled.unwrap_or(true))
            .map(|source| (source.id.clone(), source))
            .collect::<HashMap<_, _>>();
    let mut dat_identities = HashMap::new();
    for archive in &archives {
        let persisted = database
            .library_dat_identities_for_item(archive.id)
            .map_err(to_failed)?;
        let mut summaries = Vec::with_capacity(persisted.len());
        // The only current, already-loaded evidence about this archive's
        // bytes - `size_bytes`, refreshed on every scan - with no
        // cryptographic hash: EmuWiz never hashes a ROM outside an
        // explicit "Run Audit"/RomM lookup, so nothing stronger is
        // available here without reopening and rehashing the file, which
        // opening this view must never do. `freshness()` already falls
        // back to comparing `size_bytes` when no hash pair overlaps, so
        // this genuinely lets a same-path/different-size replacement
        // resolve to `Stale` rather than always `Unknown` - a same-size
        // replacement still correctly resolves `Unknown` (insufficient
        // evidence), never fabricated as `Current`.
        let current_hashes = archivefs_core::dat::library_identity_summary::LibraryItemHashes {
            size_bytes: archive.size_bytes,
            ..Default::default()
        };
        for identity in persisted {
            let source_id = identity.source.source_id;
            let configured_source = configured_dat_sources.get(&source_id);
            let current_source_revision = configured_source.and_then(|source| {
                let revisions = source.health_arcade_catalogue_revisions.as_deref()?;
                if revisions.len() != 1 {
                    return None;
                }
                let (_, revision) = revisions[0].split_once('=')?;
                (!revision.is_empty()).then_some(revision)
            });
            if let Some(summary) = database
                .library_dat_identity_summary_for_item(
                    archive.id,
                    &source_id,
                    Some(&current_hashes),
                    current_source_revision,
                    configured_source.is_some(),
                )
                .map_err(to_failed)?
            {
                summaries.push(summary);
            }
        }
        if !summaries.is_empty() {
            dat_identities.insert(archive.id, summaries);
        }
    }
    let platform_details = database
        .load_platform_provenance_details(&archives)
        .map_err(to_failed)?;
    let stats = database.catalogue_stats().map_err(to_failed)?;
    let last_completed_scan = database.latest_completed_scan().map_err(to_failed)?;
    let recently_found = database.latest_scan_additions().map_err(to_failed)?;
    let platform_aliases = database.list_platform_aliases().map_err(to_failed)?;
    let duplicate_report = catalogue_filename_duplicates(&archives);
    let source_views = load_source_folder_configs_from(config_path)
        .ok()
        .map(|sources| {
            let records = database.list_source_folders().unwrap_or_default();
            build_source_folder_views(&sources, &records)
        })
        .unwrap_or_default();
    let mod_catalogue_records = database.list_mod_catalogue_records().map_err(to_failed)?;
    Ok(CachedLibrarySnapshot {
        database_path: database_path.to_path_buf(),
        schema_version,
        archives,
        media_topology,
        dat_identities,
        platform_details,
        stats,
        last_completed_scan,
        recently_found,
        platform_aliases,
        duplicate_report,
        source_views,
        mod_catalogue_records,
        screenscraper_enrichments,
    })
}

/// What a finished database load settled, for the application to apply.
///
/// [`poll_database_load`] owns the database state machine itself - the
/// receiver, both staleness checks, the worker join and the state install.
/// It deliberately does not touch the app-global sinks; the entries it would
/// have written to them come back here instead, so the ordering and the
/// wording stay with the code that knows the outcome.
pub(crate) struct DatabaseLoadSettled {
    pub(crate) history: Option<HistoryEntry>,
    pub(crate) feedback: Option<ActionFeedback>,
}

/// Drain a finished database load into `database_state`.
///
/// Returns `None` while nothing has arrived, and also when the message that
/// arrived belongs to a superseded generation - in that case nothing at all
/// is applied, including `pending_source_scan_summary`, which is consumed
/// only once this load is committed to landing.
pub(crate) fn poll_database_load(
    database_state: &mut DatabaseState,
    database_generation: DatabaseGeneration,
    pending_source_scan_summary: &mut Option<ScanPersistSummary>,
) -> Option<DatabaseLoadSettled> {
    let message = match &*database_state {
        DatabaseState::Loading {
            generation,
            receiver,
            ..
        } => match receiver.try_recv() {
            Ok(message) => Some(message),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some((
                *generation,
                Err(DatabaseLoadError::Failed {
                    message: "background database loader stopped unexpectedly".to_string(),
                }),
            )),
        },
        DatabaseState::NotCreated { .. }
        | DatabaseState::Ready { .. }
        | DatabaseState::Outdated { .. }
        | DatabaseState::Error { .. } => None,
    };

    let (generation, result) = message?;
    // Two independent staleness checks, mirroring poll_load exactly:
    // (1) is this even the current database generation, and (2) does
    // the state we are about to replace still agree it is Loading at
    // that same generation (it could have been replaced by a newer
    // start_database_action call between the channel send and this
    // poll). Either mismatch means this message is from a previous
    // generation and must be ignored, never merged into current state.
    if generation != database_generation {
        return None;
    }
    let (previous, worker) = match std::mem::replace(
        database_state,
        DatabaseState::Error {
            message: "database load result pending".to_string(),
            previous: None,
        },
    ) {
        DatabaseState::Loading {
            generation: state_generation,
            previous,
            worker,
            ..
        } if state_generation == generation => (previous, worker),
        other => {
            *database_state = other;
            return None;
        }
    };
    if let Some(worker) = worker {
        let _ = worker.join();
    }

    // Consumed unconditionally, whatever `result` turns out to be below:
    // a pending Sources-page scan summary is only ever valid for the
    // very next reload completion, never a later one (requirement:
    // never invent a state transition - if this reload doesn't land in
    // `Ready`, there is no `last_scan_summary` to attach it to, so it
    // is simply dropped rather than held over).
    let pending_source_scan_summary = pending_source_scan_summary.take();
    let mut history = None;
    let mut feedback = None;
    *database_state = match result {
        Ok(DatabaseOutcome::Loaded(snapshot)) => DatabaseState::Ready {
            snapshot: Box::new(snapshot),
            last_scan_summary: pending_source_scan_summary,
        },
        Ok(DatabaseOutcome::Scanned {
            snapshot,
            scan_summary,
            upgrade,
        }) => {
            let activity = match &upgrade {
                Some(report) => format_database_upgrade_success(report, &scan_summary),
                None => format_scan_activity(&scan_summary),
            };
            history = Some(HistoryEntry::new(
                ActivityAction::LibraryDatabase,
                None,
                ActivityOutcome::Completed,
                activity.clone(),
            ));
            if upgrade.is_some() {
                feedback = Some(ActionFeedback {
                    succeeded: true,
                    message: activity,
                    cleanup: None,
                    warning: None,
                    more_information: None,
                });
            }
            DatabaseState::Ready {
                snapshot: Box::new(snapshot),
                last_scan_summary: Some(scan_summary),
            }
        }
        Err(DatabaseLoadError::NotCreated { database_path }) => {
            DatabaseState::NotCreated { database_path }
        }
        Err(DatabaseLoadError::Outdated { health }) => DatabaseState::Outdated { health, previous },
        Err(DatabaseLoadError::Failed { message }) => {
            history = Some(HistoryEntry::new(
                ActivityAction::LibraryDatabase,
                None,
                ActivityOutcome::Failed,
                message.clone(),
            ));
            DatabaseState::Error { message, previous }
        }
    };

    Some(DatabaseLoadSettled { history, feedback })
}
