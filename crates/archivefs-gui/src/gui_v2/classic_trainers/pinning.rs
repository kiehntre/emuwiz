//! Selection, exact preview and explicit confirmation for Gold-format pins.
use super::*;
use archivefs_core::classic_trainer_evidence::pinning::{PinPreview, create_pin, preview_pin};
use archivefs_core::safe_read::TrustedRoots;
use std::sync::mpsc::{self, Receiver};

type CurrentEvidence = std::sync::Arc<
    dyn Fn(&Game) -> Result<(ClassicTrainerGame, TrustedRoots), String> + Send + Sync,
>;

#[derive(Default)]
pub(super) struct Pinning {
    scope: Option<(i64, PathBuf, ClassicTrainerGame)>,
    selected: bool,
    preview: Option<PinPreview>,
    pending: Option<Receiver<Result<Option<PinPreview>, String>>>,
    notice: Option<String>,
}

/// Refresh source-aware DAT evidence at both Preview and Confirm. Cached
/// Game Details alone never authorizes a write.
fn current(game: &Game) -> Result<(ClassicTrainerGame, TrustedRoots), String> {
    let path = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
    let library = super::super::backend::load_library(&path)?;
    let fresh = library
        .games
        .iter()
        .find(|g| g.archive.id == game.archive.id)
        .ok_or("Game no longer exists in the current library.")?;
    if fresh.archive.absolute_path != game.archive.absolute_path
        || trainer_game(fresh) != trainer_game(game)
    {
        return Err(
            "DAT identity or game path changed; refresh Game Details and preview again.".into(),
        );
    }
    let config = archivefs_core::Config::load_default().map_err(|e| e.to_string())?;
    if fresh.platform == "Commodore 64" {
        // The same bounded cartridge/DAT contract used by Blue VICE Play.
        let _ = super::super::vice_evidence::current(&fresh.archive.absolute_path)?;
    }
    Ok((
        trainer_game(fresh),
        TrustedRoots::from_paths(config.source_folders),
    ))
}

impl Pinning {
    /// Returns true only after verified publication, invalidating the report.
    pub(super) fn show(&mut self, ui: &mut egui::Ui, game: &Game, candidate: bool) -> bool {
        self.show_with_current(ui, game, candidate, std::sync::Arc::new(current))
    }

    fn show_with_current(
        &mut self,
        ui: &mut egui::Ui,
        game: &Game,
        candidate: bool,
        current: CurrentEvidence,
    ) -> bool {
        let identity = trainer_game(game);
        let scope = (
            game.archive.id,
            game.archive.absolute_path.clone(),
            identity.clone(),
        );
        if self.scope.as_ref() != Some(&scope) {
            *self = Self {
                scope: Some(scope),
                ..Self::default()
            };
        }
        let mut refresh = false;
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(Some(preview)) => {
                            self.preview = Some(preview);
                            self.notice = None;
                        }
                        Ok(None) => {
                            self.preview = None;
                            self.selected = false;
                            refresh = true;
                            self.notice = Some("Association created and verified. Trainer evidence refreshed. No POKEs were applied.".into());
                        }
                        Err(error) => {
                            self.preview = None;
                            self.notice = Some(format!(
                                "Association refused or could not be confirmed: {error}"
                            ));
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.notice = Some("Worker stopped; result is unknown. Inspect the pin and refresh before retrying.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if identity.identity_verified {
            line(
                ui,
                format!(
                    "{}: {}",
                    game.dat_exact_label()
                        .unwrap_or_else(|| "Verified DAT identity".into()),
                    identity.identities.join(", ")
                ),
            );
        } else {
            line(
                ui,
                "Pin creation requires a current DAT-verified game identity. Verify this game first.",
            );
        }
        if let Some(notice) = &self.notice {
            line(ui, notice);
        }
        if self.pending.is_some() {
            ui.spinner();
            line(ui, "Checking current evidence…");
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            return refresh;
        }
        if let Some(preview) = &self.preview {
            line(
                ui,
                format!("Source evidence: {}", preview.trainer_path().display()),
            );
            line(
                ui,
                format!("Create exactly: {}", preview.destination().display()),
            );
            ui.label(RichText::new(preview.contents()).monospace());
            line(
                ui,
                "This is your explicit association of the whole trainer file with this release, not proof that its POKEs are compatible. Existing pins are never replaced. Backup and Undo are not supported for trainer pins. Creation requires Linux, a plain game file of at most 64 MiB and one same-name .pok file.",
            );
            if ui.button("Confirm create association").clicked() {
                let current = current.clone();
                let preview = preview.clone();
                let game = game.clone();
                self.start(ui.ctx(), move || {
                    let (identity, roots) = current(&game)?;
                    create_pin(&preview, &identity, &roots)?;
                    Ok(None)
                });
            }
            if ui.button("Cancel association").clicked() {
                self.preview = None;
            }
        } else if candidate {
            let response = ui.checkbox(
                &mut self.selected,
                "Select existing same-name .pok trainer file",
            );
            response.on_hover_text("All trainer options in this file share one association.");
            if ui
                .add_enabled(
                    self.selected && identity.identity_verified,
                    egui::Button::new("Preview trainer association"),
                )
                .clicked()
            {
                let current = current.clone();
                let game = game.clone();
                self.start(ui.ctx(), move || {
                    let (identity, roots) = current(&game)?;
                    preview_pin(&identity, &game.archive.absolute_path, &roots).map(Some)
                });
            }
        }
        refresh
    }

    fn start(
        &mut self,
        context: &egui::Context,
        work: impl FnOnce() -> Result<Option<PinPreview>, String> + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        let context = context.clone();
        std::thread::spawn(move || {
            let _ = sender.send(work());
            context.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::dat::library_identity_summary::LibraryItemHashes;
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};

    fn fixture() -> (tempfile::TempDir, Game, CurrentEvidence, Arc<Mutex<bool>>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.tap");
        std::fs::write(&path, "synthetic game").unwrap();
        std::fs::write(path.with_extension("pok"), "N Lives\nZ 8 35899 0 53\nY\n").unwrap();
        let mut game = Game::from_archive(archivefs_core::PersistedArchive {
            id: 1,
            source_folder_id: 1,
            relative_path: "game.tap".into(),
            absolute_path: path,
            archive_kind: "loose".into(),
            display_name: "Game".into(),
            normalized_name: "game".into(),
            size_bytes: Some(14),
            modified_time_unix_seconds: None,
            platform: Some("ZX Spectrum".into()),
            platform_source: None,
            last_known_health: "ok".into(),
            last_seen_at: String::new(),
            last_verified_missing_at: None,
            identity_report: None,
        });
        game.mark_dat_exact(Some("TOSEC"));
        game.dat_exact_hashes = Some(LibraryItemHashes {
            sha256: Some(
                Sha256::digest(b"synthetic game")
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            ),
            ..Default::default()
        });
        let roots = TrustedRoots::from_paths([dir.path()]);
        let valid = Arc::new(Mutex::new(true));
        let validity = valid.clone();
        let current: CurrentEvidence = Arc::new(move |game| {
            if !*validity.lock().unwrap() {
                return Err("DAT source withdrawn".into());
            }
            Ok((trainer_game(game), roots.clone()))
        });
        (dir, game, current, valid)
    }

    fn frame(
        state: &mut Pinning,
        ctx: &egui::Context,
        game: &Game,
        current: &CurrentEvidence,
        events: Vec<egui::Event>,
    ) -> (bool, Vec<(String, egui::Pos2)>) {
        let mut refreshed = false;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 1600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    refreshed = state.show_with_current(ui, game, true, current.clone());
                });
            },
        );
        let texts = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some((
                        text.galley.text().to_string(),
                        text.pos + text.galley.size() * 0.5,
                    ))
                } else {
                    None
                }
            })
            .collect();
        (refreshed, texts)
    }

    fn click(
        state: &mut Pinning,
        ctx: &egui::Context,
        game: &Game,
        current: &CurrentEvidence,
        label: &str,
    ) {
        let (_, texts) = frame(state, ctx, game, current, vec![]);
        let position = texts
            .iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("missing {label}: {texts:?}"))
            .1;
        for pressed in [true, false] {
            frame(
                state,
                ctx,
                game,
                current,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    fn settle(
        state: &mut Pinning,
        ctx: &egui::Context,
        game: &Game,
        current: &CurrentEvidence,
    ) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut refreshed = false;
        while state.pending.is_some() && std::time::Instant::now() < deadline {
            refreshed |= frame(state, ctx, game, current, vec![]).0;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(state.pending.is_none(), "worker timed out");
        refreshed
    }

    fn select_preview(
        state: &mut Pinning,
        ctx: &egui::Context,
        game: &Game,
        current: &CurrentEvidence,
    ) {
        click(
            state,
            ctx,
            game,
            current,
            "Select existing same-name .pok trainer file",
        );
        click(state, ctx, game, current, "Preview trainer association");
        settle(state, ctx, game, current);
        assert!(state.preview.is_some(), "{:?}", state.notice);
    }

    #[test]
    fn blue_click_journey_previews_confirms_and_refreshes_verified_section() {
        let (_dir, game, current, _) = fixture();
        let context = egui::Context::default();
        let mut state = Pinning::default();
        select_preview(&mut state, &context, &game, &current);
        let preview = state.preview.clone().unwrap();
        assert!(!preview.destination().exists());
        let (_, texts) = frame(&mut state, &context, &game, &current, vec![]);
        assert!(texts.iter().any(|(text, _)| text == preview.contents()));
        assert!(
            texts
                .iter()
                .any(|(text, _)| text.contains("Undo are not supported"))
        );
        click(
            &mut state,
            &context,
            &game,
            &current,
            "Confirm create association",
        );
        assert!(settle(&mut state, &context, &game, &current));
        assert_eq!(
            std::fs::read_to_string(preview.destination()).unwrap(),
            preview.contents()
        );
        let report = assess_sidecar_pok(&trainer_game(&game), &game.archive.absolute_path)
            .unwrap()
            .unwrap();
        let lines = super::super::tests::texts(&context, |ui| render(ui, &Ok(Some(report))));
        assert!(lines.iter().any(|line| line == "Verified for this release"));
        assert!(
            state
                .notice
                .as_ref()
                .unwrap()
                .contains("created and verified")
        );
    }

    #[test]
    fn cancel_and_stale_evidence_never_write() {
        let (_dir, game, current, valid) = fixture();
        let context = egui::Context::default();
        let mut state = Pinning::default();
        select_preview(&mut state, &context, &game, &current);
        click(&mut state, &context, &game, &current, "Cancel association");
        assert!(state.preview.is_none());
        assert!(!trainer_target_path(&game.archive.absolute_path).exists());
        // Selection remains explicit; a fresh preview must be requested.
        click(
            &mut state,
            &context,
            &game,
            &current,
            "Preview trainer association",
        );
        settle(&mut state, &context, &game, &current);
        *valid.lock().unwrap() = false;
        click(
            &mut state,
            &context,
            &game,
            &current,
            "Confirm create association",
        );
        assert!(!settle(&mut state, &context, &game, &current));
        assert!(
            state
                .notice
                .as_ref()
                .unwrap()
                .contains("DAT source withdrawn")
        );
        assert!(!trainer_target_path(&game.archive.absolute_path).exists());
    }

    #[test]
    fn game_switch_discards_preview_and_late_worker_results() {
        let (_dir, game, current, _) = fixture();
        let context = egui::Context::default();
        let mut state = Pinning::default();
        select_preview(&mut state, &context, &game, &current);
        let (sender, receiver) = mpsc::channel();
        state.pending = Some(receiver);
        let preview = state.preview.clone().unwrap();
        let mut other = game.clone();
        other.archive.id = 2;
        frame(&mut state, &context, &other, &current, vec![]);
        assert!(state.preview.is_none());
        assert!(!state.selected);
        assert!(sender.send(Ok(Some(preview))).is_err());
        assert!(!trainer_target_path(&game.archive.absolute_path).exists());
    }
}
