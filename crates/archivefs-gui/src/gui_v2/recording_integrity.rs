//! Explicit, read-only integrity check for preservation-only RZX recordings.
use super::{library::Game, routes::Route};
use archivefs_core::identity_source::hashing::FileFingerprint;
use archivefs_core::{Database, PreservationFreshness};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Clone, PartialEq, Eq)]
struct Scope {
    id: i64,
    path: PathBuf,
    size: Option<u64>,
    modified: Option<i64>,
}

impl Scope {
    fn of(game: &Game) -> Self {
        Self {
            id: game.archive.id,
            path: game.archive.absolute_path.clone(),
            size: game.archive.size_bytes,
            modified: game.archive.modified_time_unix_seconds,
        }
    }
}

struct Reply {
    outcome: Result<PreservationFreshness, String>,
    stamp: Option<FileFingerprint>,
}

#[derive(Default)]
pub(super) struct State {
    scope: Option<Scope>,
    pending: Option<Receiver<Reply>>,
    result: Option<Reply>,
}

fn unavailable(reason: &str) -> Reply {
    Reply {
        outcome: Err(reason.into()),
        stamp: None,
    }
}

pub(super) fn eligible(game: &Game) -> bool {
    game.is_preservation_only()
        && game
            .archive
            .absolute_path
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("rzx"))
}

fn check_at(database_path: &Path, scope: &Scope, clicked: Option<FileFingerprint>) -> Reply {
    let run = || -> Result<Reply, String> {
        let db = Database::open_read_only(database_path).map_err(|e| e.to_string())?;
        let same_row = || -> Result<bool, String> {
            db.find_archive_id_by_absolute_path(&scope.path)
                .map(|id| id == Some(scope.id))
                .map_err(|e| e.to_string())
        };
        if !same_row()?
            || !db
                .is_preservation_recording(scope.id)
                .map_err(|e| e.to_string())?
        {
            return Ok(unavailable(
                "The library entry moved or was replaced. Refresh Games and check again.",
            ));
        }
        let recorded = db
            .preservation_fingerprint(scope.id)
            .map_err(|e| e.to_string())?;
        if recorded.is_none() {
            return Ok(Reply {
                outcome: Ok(PreservationFreshness::NotRecorded),
                stamp: None,
            });
        }
        let before = FileFingerprint::observe(&scope.path);
        if before.is_none() || before != clicked {
            return Ok(unavailable(
                "The recording is missing or its file identity changed before checking.",
            ));
        }
        let before = before.unwrap();
        let outcome = db
            .verify_preservation_fingerprint_bound(scope.id, &before)
            .map_err(|e| e.to_string())?;
        let after = FileFingerprint::observe(&scope.path);
        if after.as_ref() != Some(&before) {
            return Ok(unavailable(
                "The recording changed while integrity was being checked.",
            ));
        }
        if !same_row()?
            || db
                .preservation_fingerprint(scope.id)
                .map_err(|e| e.to_string())?
                != recorded
        {
            return Ok(unavailable(
                "The library entry changed while integrity was being checked. Refresh Games and check again.",
            ));
        }
        Ok(Reply {
            outcome: Ok(outcome),
            stamp: after,
        })
    };
    run().unwrap_or_else(|reason| unavailable(&reason))
}

impl State {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn retain_route(&mut self, route: &Route) {
        if self
            .scope
            .as_ref()
            .is_some_and(|scope| !matches!(route, Route::Game(id) if *id == scope.id))
        {
            *self = Self::default();
        }
    }

    fn start_at(&mut self, context: &egui::Context, scope: Scope, database_path: PathBuf) {
        let clicked = FileFingerprint::observe(&scope.path);
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.result = None;
        let context = context.clone();
        std::thread::spawn(move || {
            let result = check_at(&database_path, &scope, clicked);
            let _ = tx.send(result);
            context.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(receiver) = &self.pending else {
            return;
        };
        match receiver.try_recv() {
            Ok(mut reply) => {
                self.pending = None;
                if reply
                    .stamp
                    .as_ref()
                    .is_some_and(|stamp| !stamp.still_current())
                {
                    reply = unavailable("The recording changed after the check. Check again.");
                }
                self.result = Some(reply);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.result = Some(unavailable("The integrity check stopped. Check again."));
            }
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui, game: &Game) {
        if !eligible(game) {
            self.clear();
            return;
        }
        let scope = Scope::of(game);
        if self.scope.as_ref() != Some(&scope) {
            *self = Self {
                scope: Some(scope.clone()),
                ..Self::default()
            };
        }
        self.poll();
        ui.label("Compare this recording with its import fingerprint. This does not check playback compatibility.");
        if ui
            .add_enabled(
                self.pending.is_none(),
                egui::Button::new("Check recording integrity"),
            )
            .clicked()
        {
            match archivefs_core::default_database_path() {
                Ok(path) => self.start_at(ui.ctx(), scope, path),
                Err(error) => self.result = Some(unavailable(&error.to_string())),
            }
        }
        if self.pending.is_some() {
            ui.spinner();
        }
        if let Some(reply) = &mut self.result {
            if reply
                .stamp
                .as_ref()
                .is_some_and(|stamp| !stamp.still_current())
            {
                *reply = unavailable("The recording changed after the check. Check again.");
            }
            let message = match &reply.outcome {
                Ok(PreservationFreshness::Fresh { .. }) => {
                    "Unchanged: bytes match the recorded import fingerprint.".into()
                }
                Ok(PreservationFreshness::Changed { .. }) => {
                    "Changed: bytes differ from the recorded import fingerprint.".into()
                }
                Ok(PreservationFreshness::NotRecorded) => {
                    "No recorded fingerprint is available for this recording.".into()
                }
                Ok(PreservationFreshness::Unavailable { reason, .. }) | Err(reason) => {
                    format!("Unavailable: {reason}")
                }
            };
            ui.label(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::{Config, scan_and_persist};

    fn fixture() -> (tempfile::TempDir, PathBuf, Scope) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let path = source.join("session.rzx");
        let mut bytes = b"RZX!\0\x0d\0\0\0\0".to_vec();
        bytes.extend_from_slice(&[0x80, 10, 0, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(&path, bytes).unwrap();
        let database_path = dir.path().join("library.sqlite3");
        let mut db = Database::open_or_create(&database_path).unwrap();
        scan_and_persist(
            &mut db,
            &Config {
                source_folders: vec![source],
                mount_root: dir.path().join("mount"),
                ratarmount_bin: String::new(),
                master_rom_root: None,
            },
            "recording-integrity",
        )
        .unwrap();
        let row = db.load_archives().unwrap().remove(0);
        assert_eq!(row.archive_kind, "preservation_recording");
        assert!(db.preservation_fingerprint(row.id).unwrap().is_some());
        let scope = Scope {
            id: row.id,
            path: row.absolute_path,
            size: row.size_bytes,
            modified: row.modified_time_unix_seconds,
        };
        drop(db);
        (dir, database_path, scope)
    }

    #[test]
    fn matching_changed_missing_and_replaced_recordings() {
        let (_dir, db, scope) = fixture();
        let clicked = FileFingerprint::observe(&scope.path);
        assert!(matches!(
            check_at(&db, &scope, clicked.clone()).outcome,
            Ok(PreservationFreshness::Fresh { .. })
        ));

        let mut bytes = std::fs::read(&scope.path).unwrap();
        bytes[10] ^= 1;
        std::fs::write(&scope.path, &bytes).unwrap();
        assert!(matches!(
            check_at(&db, &scope, FileFingerprint::observe(&scope.path)).outcome,
            Ok(PreservationFreshness::Changed { .. })
        ));
        let _ = std::fs::remove_file(&scope.path);
        assert!(check_at(&db, &scope, None).outcome.is_err());

        std::fs::write(&scope.path, bytes).unwrap();
        assert!(check_at(&db, &scope, clicked).outcome.is_err());
    }

    #[test]
    fn absent_import_fingerprint_has_its_own_outcome() {
        let (_dir, db, scope) = fixture();
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute(
                "UPDATE archives SET content_hash = NULL WHERE id = ?1",
                [scope.id],
            )
            .unwrap();
        assert!(matches!(
            check_at(&db, &scope, FileFingerprint::observe(&scope.path)).outcome,
            Ok(PreservationFreshness::NotRecorded)
        ));
    }

    #[test]
    fn selection_change_discards_pending_result_without_waiting() {
        let (_dir, _db, scope) = fixture();
        let (_tx, rx) = mpsc::channel();
        let mut state = State {
            scope: Some(scope.clone()),
            pending: Some(rx),
            result: None,
        };
        state.poll(); // An unfinished hash must not block the GUI frame.
        assert!(state.pending.is_some());
        state.retain_route(&Route::Game(scope.id + 1));
        assert!(state.scope.is_none());
        assert!(state.pending.is_none());
    }

    #[test]
    fn replacement_after_hash_invalidates_the_completed_result() {
        let (_dir, db, scope) = fixture();
        let reply = check_at(&db, &scope, FileFingerprint::observe(&scope.path));
        assert!(matches!(
            reply.outcome,
            Ok(PreservationFreshness::Fresh { .. })
        ));
        let bytes = std::fs::read(&scope.path).unwrap();
        std::fs::remove_file(&scope.path).unwrap();
        std::fs::write(&scope.path, bytes).unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(reply).unwrap();
        let mut state = State {
            scope: Some(scope),
            pending: Some(rx),
            result: None,
        };
        state.poll();
        assert!(
            state
                .result
                .as_ref()
                .is_some_and(|reply| reply.outcome.is_err())
        );
    }

    #[test]
    fn blue_render_remains_responsive_while_hash_worker_is_pending() {
        let (_dir, db, scope) = fixture();
        let row = Database::open_read_only(&db)
            .unwrap()
            .load_archives()
            .unwrap()
            .remove(0);
        let game = Game::from_archive(row);
        let (_tx, rx) = mpsc::channel();
        let mut state = State {
            scope: Some(scope),
            pending: Some(rx),
            result: None,
        };
        let context = egui::Context::default();
        super::super::readable_style(&context);
        let started = std::time::Instant::now();
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| state.show(ui, &game));
        });
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(state.pending.is_some());
    }
}
