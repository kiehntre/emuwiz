//! The presentation's I/O boundary. Only this worker opens catalogue/config files.
use super::{
    library::{
        Detail, DuplicateGroup, DuplicateMember, DuplicateReport, Filter, Game, Library,
        SharedLibrary,
    },
    routes::{Route, Section},
};
use archivefs_core::{Database, default_database_path};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::Instant,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Preferences {
    pub route: Route,
    pub filter: Filter,
}

pub(super) enum Command {
    Load {
        scan: bool,
    },
    Filter {
        library: SharedLibrary,
        filter: Filter,
        generation: u64,
    },
    Detail {
        game: Box<Game>,
        generation: u64,
    },
    Verify {
        platform: String,
        games: Vec<Game>,
        cancel: Arc<AtomicBool>,
    },
    ScanDuplicates {
        games: Vec<Game>,
    },
    PrepareDuplicateRepair {
        group: Box<archivefs_core::repair::ExactDuplicateGroup>,
    },
    ApplyDuplicateRepair {
        preview: Box<DuplicateRepairPreview>,
        cancel: Arc<AtomicBool>,
    },
    UndoDuplicateRepair {
        record: Box<DuplicateRepairRecord>,
        cancel: Arc<AtomicBool>,
    },
    LoadRepairHistory,
    OpenFolder(PathBuf),
    Legacy {
        section: Section,
        path: Option<PathBuf>,
    },
    Save(Preferences),
    Restore,
}

pub(super) enum Payload {
    Library(SharedLibrary),
    Filter {
        indices: Vec<usize>,
        generation: u64,
    },
    Detail {
        detail: Detail,
        generation: u64,
    },
    Verification(VerificationResult),
    Duplicates(DuplicateReport),
    DuplicatePreview(Box<DuplicateRepairPreview>),
    DuplicateApplied(Box<DuplicateRepairRecord>),
    DuplicateUndone(Box<DuplicateRepairRecord>),
    RepairHistory {
        duplicates: Vec<DuplicateRepairRecord>,
        playing_libraries: Vec<archivefs_core::dat::rename_apply::model::RenameTransaction>,
    },
    Preferences(Preferences),
    Done,
}

#[derive(Clone, Debug)]
pub(super) struct DuplicateRepairPreview {
    pub group: archivefs_core::repair::ExactDuplicateGroup,
    pub proposals: Vec<archivefs_core::repair::RepairProposal>,
    pub trusted_root: PathBuf,
    pub journal_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub(super) struct DuplicateRepairRecord {
    pub transaction: archivefs_core::dat::rename_apply::model::RenameTransaction,
    pub trusted_root: PathBuf,
    pub journal_dir: PathBuf,
}

pub(super) enum Event {
    Started(u64),
    Progress {
        id: u64,
        done: u64,
        total: u64,
        item: String,
    },
    Finished {
        id: u64,
        outcome: Result<Payload, String>,
    },
}

#[derive(Clone, Debug, Default)]
pub(super) struct VerificationResult {
    pub platform: String,
    pub matched: usize,
    pub attention: usize,
    pub unknown: usize,
    pub missing: usize,
    pub total: usize,
    pub statuses: HashMap<i64, String>,
}

pub(super) struct Backend {
    tx: Sender<(u64, Command)>,
    pub rx: Receiver<Event>,
}

impl Backend {
    pub fn start(context: eframe::egui::Context) -> Self {
        let (tx, requests) = mpsc::channel();
        let (answers, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((id, command)) = requests.recv() {
                if answers.send(Event::Started(id)).is_err() {
                    break;
                }
                context.request_repaint();
                // Contain third-party decoder/backend panics at the worker boundary.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute(id, command, &answers)
                }))
                .unwrap_or_else(|_| Err("The background operation stopped unexpectedly.".into()));
                if answers.send(Event::Finished { id, outcome }).is_err() {
                    break;
                }
                context.request_repaint();
            }
        });
        Self { tx, rx }
    }
    pub fn send(&self, id: u64, command: Command) -> Result<(), String> {
        self.tx.send((id, command)).map_err(|_| "The background worker is unavailable. Close and reopen GUI v2; your games have not been changed.".into())
    }
}

fn execute(id: u64, command: Command, answers: &Sender<Event>) -> Result<Payload, String> {
    match command {
        Command::Load { scan } => {
            let scan_warning = if scan {
                let summary = archivefs_core::scan_all_enabled_sources_default()
                    .map_err(|error| error.to_string())?;
                (!summary.folder_errors.is_empty()).then(|| format!("{} game folders could not be read. Their previous entries were kept. Reconnect the drive or review Sources, then scan again.", summary.folder_errors.len()))
            } else {
                None
            };
            let path = default_database_path().map_err(|error| error.to_string())?;
            let sources = archivefs_core::load_source_folder_configs_default()
                .map(|sources| sources.iter().filter(|source| source.enabled).count())
                .unwrap_or_default();
            let mut library = load_library(&path)?;
            library.sources = sources;
            library.scan_warning = scan_warning;
            Ok(Payload::Library(Arc::new(library)))
        }
        Command::Filter {
            library,
            filter,
            generation,
        } => Ok(Payload::Filter {
            indices: library.filter(&filter),
            generation,
        }),
        Command::Detail { game, generation } => Ok(Payload::Detail {
            detail: load_detail(&game)?,
            generation,
        }),
        Command::Verify {
            platform,
            games,
            cancel,
        } => {
            let total = games.len() as u64;
            let mut result = VerificationResult {
                platform,
                total: games.len(),
                ..Default::default()
            };
            for (index, game) in games.into_iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let is_present = game.archive.absolute_path.is_file()
                    || (matches!(
                        archivefs_core::ArchiveKind::from_storage(&game.archive.archive_kind),
                        Some(archivefs_core::ArchiveKind::ArcadeSetDirectory)
                    ) && game.archive.absolute_path.is_dir());
                let status = if !is_present {
                    result.missing += 1;
                    "Missing"
                } else if game.attention {
                    result.attention += 1;
                    "Needs attention"
                } else if game.identified {
                    result.matched += 1;
                    "Verified"
                } else {
                    result.unknown += 1;
                    "Unknown"
                };
                result.statuses.insert(game.archive.id, status.into());
                let _ = answers.send(Event::Progress {
                    id,
                    done: index as u64 + 1,
                    total,
                    item: game.title,
                });
            }
            Ok(Payload::Verification(result))
        }
        Command::ScanDuplicates { games } => {
            let candidates: Vec<_> = games
                .iter()
                .map(|game| game.archive.absolute_path.clone())
                .collect();
            let config =
                archivefs_core::Config::load_default().map_err(|error| error.to_string())?;
            let trusted =
                archivefs_core::safe_read::TrustedRoots::from_paths(config.source_folders.clone());
            let report = archivefs_core::repair::scan_exact_duplicates(
                &candidates,
                &trusted,
                &config.source_folders,
                &std::collections::BTreeSet::new(),
                None,
            );
            let exact_groups = report.groups.clone();
            let groups = report
                .groups
                .into_iter()
                .enumerate()
                .filter_map(|group| {
                    let (exact_index, group) = group;
                    let members = group
                        .members
                        .into_iter()
                        .filter_map(|member| {
                            let game = games
                                .iter()
                                .find(|game| game.archive.absolute_path == member.path)?;
                            Some(DuplicateMember {
                                path: member.path,
                                title: game.title.clone(),
                                platform: game.platform.clone(),
                                size_bytes: group.size_bytes,
                                evidence: format!("SHA-256 {}", group.sha256),
                            })
                        })
                        .collect::<Vec<_>>();
                    (members.len() > 1).then_some(DuplicateGroup {
                        exact_index,
                        kind: "Exact duplicates".into(),
                        sha256: group.sha256,
                        size_bytes: group.size_bytes,
                        members,
                    })
                })
                .collect();
            Ok(Payload::Duplicates(DuplicateReport {
                groups,
                files_examined: report.files_examined,
                exact_groups,
            }))
        }
        Command::PrepareDuplicateRepair { group } => {
            let config =
                archivefs_core::Config::load_default().map_err(|error| error.to_string())?;
            let trusted =
                archivefs_core::safe_read::TrustedRoots::from_paths(config.source_folders.clone());
            let retained = group
                .recommendation
                .retained_path()
                .ok_or_else(|| "This duplicate group has no safe retained copy.".to_string())?;
            let trusted_root = trusted_root_for(retained, &config.source_folders)?;
            let mut cache = archivefs_core::repair::DuplicateHashCache::new();
            let proposals = archivefs_core::repair::build_exact_duplicate_group_proposals(
                &group,
                &trusted_root,
                &mut cache,
                &trusted,
                None,
            )?;
            let journal_dir =
                archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
                    .map_err(|error| error.to_string())?;
            Ok(Payload::DuplicatePreview(Box::new(
                DuplicateRepairPreview {
                    group: *group,
                    proposals,
                    trusted_root,
                    journal_dir,
                },
            )))
        }
        Command::ApplyDuplicateRepair { preview, cancel } => {
            if cancel.load(Ordering::Relaxed) {
                return Err("The repair was cancelled before it changed anything.".into());
            }
            let trusted = archivefs_core::safe_read::TrustedRoots::from_paths(
                archivefs_core::Config::load_default()
                    .map_err(|error| error.to_string())?
                    .source_folders,
            );
            let retained = preview
                .group
                .recommendation
                .retained_path()
                .ok_or_else(|| {
                    "The approved duplicate group no longer has a retained copy.".to_string()
                })?;
            std::fs::create_dir_all(&preview.journal_dir)
                .map_err(|error| format!("Could not prepare repair history: {error}"))?;
            let mut cache = archivefs_core::repair::DuplicateHashCache::new();
            let mut transaction = archivefs_core::repair::build_quarantine_transaction(
                &preview.proposals,
                retained,
                &preview.trusted_root,
                0,
                &mut cache,
                &trusted,
                Some(&cancel),
            )
            .map_err(|error| format!("The preview is no longer safe: {error}"))?;
            let _ = answers.send(Event::Progress {
                id,
                done: 0,
                total: transaction.entries.len() as u64,
                item: "Validating the approved files".into(),
            });
            let outcome = archivefs_core::repair::apply_quarantine_transaction(
                &mut transaction,
                retained,
                &preview.trusted_root,
                0,
                trusted,
                &preview.journal_dir,
                &cancel,
                &mut cache,
            )
            .map_err(|error| error.to_string())?;
            let _ = answers.send(Event::Progress {
                id,
                done: outcome.transaction.applied_count() as u64,
                total: outcome.transaction.entries.len() as u64,
                item: "Writing the repair receipt".into(),
            });
            Ok(Payload::DuplicateApplied(Box::new(DuplicateRepairRecord {
                transaction: outcome.transaction,
                trusted_root: preview.trusted_root,
                journal_dir: preview.journal_dir,
            })))
        }
        Command::UndoDuplicateRepair { record, cancel } => {
            let mut transaction = record.transaction.clone();
            archivefs_core::repair::rollback_quarantine_transaction(
                &mut transaction,
                &record.journal_dir,
                &cancel,
                &record.trusted_root,
            )
            .map_err(|error| error.to_string())?;
            Ok(Payload::DuplicateUndone(Box::new(DuplicateRepairRecord {
                transaction,
                trusted_root: record.trusted_root,
                journal_dir: record.journal_dir,
            })))
        }
        Command::LoadRepairHistory => {
            let journal_dir =
                archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
                    .map_err(|error| error.to_string())?;
            let (transactions, _problems) =
                archivefs_core::dat::rename_apply::journal::list_journals(&journal_dir);
            {
                let mut duplicates = Vec::new();
                let mut playing_libraries = Vec::new();
                for transaction in transactions {
                    let is_duplicate = transaction.entries.iter().any(|entry| {
                        entry.destination_path.components().any(|component| {
                            component.as_os_str()
                                == archivefs_core::repair::QUARANTINE_DIRECTORY_NAME
                        })
                    });
                    let is_playing_library = transaction.entries.iter().any(|entry| {
                        matches!(
                            &entry.operation,
                            &archivefs_core::dat::rename_apply::model::TransactionOperation::CreateSymlink { .. }
                        )
                    });
                    if is_duplicate {
                        duplicates.push(DuplicateRepairRecord {
                            trusted_root: PathBuf::from(&transaction.source_scan_root),
                            transaction,
                            journal_dir: journal_dir.clone(),
                        });
                    } else if is_playing_library {
                        playing_libraries.push(transaction);
                    }
                }
                Ok(Payload::RepairHistory {
                    duplicates,
                    playing_libraries,
                })
            }
        }
        Command::OpenFolder(path) => {
            let folder = path.parent().ok_or("This game has no containing folder.")?;
            if !folder.is_absolute() || !folder.is_dir() {
                return Err("The game folder is no longer available.".into());
            }
            crate::open_folder_in_file_manager(folder).map_err(|error| error.to_string())?;
            Ok(Payload::Done)
        }
        Command::Legacy { section, path } => {
            super::legacy::open(section, path.as_deref())?;
            Ok(Payload::Done)
        }
        Command::Save(preferences) => {
            save_preferences(&preferences_path()?, &preferences)?;
            Ok(Payload::Done)
        }
        Command::Restore => {
            let path = preferences_path()?;
            let preferences = match fs::read(&path) {
                Ok(bytes) if bytes.len() <= 32 * 1024 => {
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Preferences::default()
                }
                _ => {
                    return Err(
                        "The saved GUI v2 location could not be read. Home remains available."
                            .into(),
                    );
                }
            };
            Ok(Payload::Preferences(preferences))
        }
    }
}

fn trusted_root_for(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, String> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .cloned()
        .ok_or_else(|| "The duplicate is outside every configured trusted game folder.".into())
}

pub(super) fn load_library(path: &Path) -> Result<Library, String> {
    let start = Instant::now();
    if !path.exists() {
        return Ok(Library::default());
    }
    let database = Database::open_read_only(path).map_err(|error| error.to_string())?;
    let archives = database
        .load_archives()
        .map_err(|error| error.to_string())?;
    let mut library = Library::new(archives);
    library.load_ms = start.elapsed().as_millis();
    log::debug!(
        "gui_v2 library: {} games, {} ms",
        library.games.len(),
        library.load_ms
    );
    Ok(library)
}

fn load_detail(game: &Game) -> Result<Detail, String> {
    let metadata = fs::metadata(&game.archive.absolute_path).ok();
    let unchanged = metadata.as_ref().is_some_and(|metadata| {
        Some(metadata.len()) == game.archive.size_bytes
            && metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|duration| i64::try_from(duration.as_secs()).ok())
                == game.archive.modified_time_unix_seconds
    });
    let database =
        Database::open_read_only(&default_database_path().map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let checks = database
        .library_dat_identities_for_item(game.archive.id)
        .map_err(|error| error.to_string())?;
    let compatibility = archivefs_core::launch::platform_map::LAUNCH_COMPATIBILITY
        .iter()
        .find(|entry| entry.platform_id == game.platform);
    let mut installed: Vec<_> =
        archivefs_core::diagnostics::profiles::discover_linux_emulator_installations()
            .into_iter()
            .filter(|evidence| evidence.executable.is_some())
            .filter(|evidence| {
                compatibility.is_some_and(|entry| {
                    entry
                        .standalone_adapters
                        .iter()
                        .any(|adapter| adapter.eq_ignore_ascii_case(&evidence.emulator))
                }) || evidence.emulator.eq_ignore_ascii_case("RetroArch")
            })
            .map(|evidence| evidence.emulator)
            .collect();
    installed.sort();
    installed.dedup();
    Ok(Detail {
        game: game.archive.id,
        file_present: metadata.is_some(),
        unchanged,
        saved_checks: checks.len(),
        installed,
        technical: format!(
            "Saved health: {}\nSaved checks: {}\n{}",
            game.archive.last_known_health,
            checks.len(),
            game.archive.absolute_path.display()
        ),
    })
}

fn preferences_path() -> Result<PathBuf, String> {
    archivefs_core::app_dirs::config_path("gui-v2.json").map_err(|error| error.to_string())
}

pub(super) fn save_preferences(path: &Path, preferences: &Preferences) -> Result<(), String> {
    let parent = path.parent().ok_or("No settings directory.")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if fs::symlink_metadata(path).is_ok_and(|meta| !meta.file_type().is_file()) {
        return Err("GUI v2 settings are not a regular file; no settings were replaced.".into());
    }
    let bytes = serde_json::to_vec(preferences).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    temporary
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}
