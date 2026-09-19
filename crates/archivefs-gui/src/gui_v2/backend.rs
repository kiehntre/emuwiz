//! The presentation's I/O boundary. Only this worker opens catalogue/config files.
use super::{
    library::{Detail, Filter, Game, Library, SharedLibrary},
    routes::{Route, Section},
};
use archivefs_core::{Database, default_database_path};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
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
    Preferences(Preferences),
    Done,
}

pub(super) enum Event {
    Started(u64),
    Finished {
        id: u64,
        outcome: Result<Payload, String>,
    },
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
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(command)))
                        .unwrap_or_else(|_| {
                            Err("The background operation stopped unexpectedly.".into())
                        });
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

fn execute(command: Command) -> Result<Payload, String> {
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
