//! Check My Games, after a system's check: games its catalogue did not
//! match can be looked for in the other systems' catalogues (owner
//! 2026-11-27). Only on request, never automatically, so ordinary
//! verification is unchanged. The search reads files and saves nothing;
//! matches open the same Platform Corrections preview (with its Undo) as
//! the DAT page.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use archivefs_core::dat::library_verification::{
    MEMBERS_WITHOUT_RESULT, NO_DEFINITIVE_RESULT, NOT_IN_CATALOGUE_FORMATS,
    VerificationFindingKind, VerificationTotals,
};
use archivefs_core::dat::platform_discovery::{
    DiscoveryOutcome, DiscoveryProgress, DiscoveryRequest, UnmatchedGame,
    discover_other_system_matches, other_system_catalogues,
};
use eframe::egui;

use crate::dat_catalogue_picker::CatalogueInventorySnapshot;
use crate::dat_sources_page::platform_corrections_panel::{
    self as panel, PlatformCorrectionsState,
};

enum Message {
    Progress(String),
    Finished(Result<DiscoveryOutcome, String>),
}

#[derive(Default)]
pub(crate) struct OtherSystems {
    platform: String,
    unmatched: Vec<UnmatchedGame>,
    receiver: Option<Receiver<Message>>,
    cancel: Option<Arc<AtomicBool>>,
    progress: String,
    error: Option<String>,
    outcome: Option<DiscoveryOutcome>,
    /// Which found catalogue's preview is open in `corrections`.
    open: Option<usize>,
    pub(crate) corrections: PlatformCorrectionsState,
    generation: u64,
}

/// The games a finished check left unmatched: no catalogue match, or not
/// checkable by this catalogue (its formats, no definitive result).
pub(crate) fn unmatched_games(totals: &VerificationTotals, platform: &str) -> Vec<UnmatchedGame> {
    let mut paths: Vec<PathBuf> = totals
        .findings
        .iter()
        .filter(|f| f.kind == VerificationFindingKind::NoMatch)
        .map(|f| f.path.clone())
        .chain(
            totals
                .unchecked
                .iter()
                .filter(|u| {
                    [
                        NOT_IN_CATALOGUE_FORMATS,
                        MEMBERS_WITHOUT_RESULT,
                        NO_DEFINITIVE_RESULT,
                    ]
                    .contains(&u.reason.as_str())
                })
                .map(|u| u.path.clone()),
        )
        .collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| UnmatchedGame {
            path,
            platform: platform.to_string(),
        })
        .collect()
}

impl OtherSystems {
    /// A finished check of `platform`: offer the search for its unmatched games.
    pub(crate) fn offer(&mut self, totals: &VerificationTotals, platform: &str, generation: u64) {
        self.reset();
        self.platform = platform.to_string();
        self.unmatched = unmatched_games(totals, platform);
        self.generation = generation;
    }

    pub(crate) fn reset(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        *self = Self::default();
    }

    pub(crate) fn busy(&self) -> bool {
        self.receiver.is_some() || self.corrections.is_busy()
    }

    fn start(
        &mut self,
        inventory: CatalogueInventorySnapshot,
        database: PathBuf,
        trusted: archivefs_core::safe_read::TrustedRoots,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.error = None;
        self.outcome = None;
        self.open = None;
        self.corrections.set_preview(None, None);
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.progress = "Looking for these games in other systems' catalogues…".into();
        let games = self.unmatched.clone();
        let platform = self.platform.clone();
        let generation = self.generation;
        std::thread::spawn(move || {
            let result = (|| {
                let work = tempfile::Builder::new()
                    .prefix("emuwiz-other-systems-")
                    .tempdir()
                    .map_err(|e| format!("Could not make a private work folder: {e}"))?;
                let (catalogues, mut skipped) =
                    other_system_catalogues(inventory.inputs(), &platform);
                let progress_sender = sender.clone();
                let mut outcome = discover_other_system_matches(
                    &DiscoveryRequest {
                        database: &database,
                        games: &games,
                        catalogues: &catalogues,
                        trusted: &trusted,
                        generation,
                        work_dir: work.path(),
                    },
                    &cancel,
                    &move |p| {
                        let DiscoveryProgress::Catalogue {
                            index,
                            of,
                            name,
                            files,
                        } = p;
                        let _ = progress_sender.send(Message::Progress(format!(
                            "Catalogue {index} of {of}: {name} ({files} file(s) to compare)"
                        )));
                    },
                )
                .map_err(|e| e.to_string())?;
                skipped.append(&mut outcome.skipped);
                outcome.skipped = skipped;
                Ok(outcome)
            })();
            let _ = sender.send(Message::Finished(result));
        });
    }

    /// Returns true when the library changed (a correction or its Undo).
    pub(crate) fn poll(&mut self, database: Option<&Path>) -> bool {
        let mut finished = false;
        if let Some(receiver) = &self.receiver {
            loop {
                match receiver.try_recv() {
                    Ok(Message::Progress(text)) => self.progress = text,
                    Ok(Message::Finished(result)) => {
                        match result {
                            Ok(outcome) => {
                                if outcome.previews.len() == 1 {
                                    self.open = Some(0);
                                    self.corrections.set_preview(
                                        Some(Box::new(outcome.previews[0].clone())),
                                        database,
                                    );
                                }
                                self.outcome = Some(outcome);
                            }
                            Err(error) => self.error = Some(error),
                        }
                        finished = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.error =
                            Some("The search stopped unexpectedly. Nothing was changed.".into());
                        finished = true;
                        break;
                    }
                }
            }
        }
        if finished {
            self.receiver = None;
            self.cancel = None;
        }
        self.corrections.poll(database).1
    }

    /// Opens the preview of found catalogue `index`.
    pub(crate) fn open_preview(&mut self, index: usize, database: Option<&Path>) {
        let Some(preview) = self.outcome.as_ref().and_then(|o| o.previews.get(index)) else {
            return;
        };
        self.open = Some(index);
        self.corrections
            .set_preview(Some(Box::new(preview.clone())), database);
    }

    #[cfg(test)]
    pub(crate) fn unmatched(&self) -> &[UnmatchedGame] {
        &self.unmatched
    }

    #[cfg(test)]
    pub(crate) fn outcome(&self) -> Option<&DiscoveryOutcome> {
        self.outcome.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn start_for_test(
        &mut self,
        inventory: CatalogueInventorySnapshot,
        database: PathBuf,
    ) {
        self.start(
            inventory,
            database,
            archivefs_core::safe_read::TrustedRoots::none(),
        );
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        inventory: Option<&CatalogueInventorySnapshot>,
        database: Option<&Path>,
        trusted: &archivefs_core::safe_read::TrustedRoots,
        enabled: bool,
    ) {
        if self.unmatched.is_empty() {
            return;
        }
        ui.separator();
        ui.strong("Games on the wrong system?");
        if self.receiver.is_some() {
            ui.label(&self.progress);
            if let Some(cancel) = &self.cancel
                && ui.button("Stop looking").clicked()
            {
                cancel.store(true, Ordering::Relaxed);
            }
            ui.ctx().request_repaint();
            return;
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
        let mut chosen = None;
        match &self.outcome {
            None => {
                ui.label(format!(
                    "{} game(s) were not matched by the {} catalogue. Some may belong to another \
                     system (for example NES games in an SNES folder). Looking reads these files \
                     and compares them with your other systems' catalogues; it changes nothing.",
                    self.unmatched.len(),
                    archivefs_core::platform::display_name_for(&self.platform),
                ));
                if ui
                    .add_enabled(
                        enabled && inventory.is_some() && database.is_some(),
                        egui::Button::new("Look in other systems' catalogues"),
                    )
                    .clicked()
                    && let (Some(inventory), Some(database)) = (inventory, database)
                {
                    self.start(inventory.clone(), database.to_path_buf(), trusted.clone());
                }
            }
            Some(outcome) => {
                let found: usize = outcome.previews.iter().map(|p| p.candidates.len()).sum();
                if found == 0 {
                    ui.label(format!(
                        "None of these games matched another system's catalogue \
                         ({} catalogue(s) compared). Nothing was changed.",
                        outcome.catalogues_checked
                    ));
                } else {
                    ui.label(format!(
                        "{found} game(s) matched another system's catalogue. Review them below; \
                         nothing changes until you confirm, and every correction can be undone."
                    ));
                }
                if outcome.previews.len() > 1 {
                    for (index, preview) in outcome.previews.iter().enumerate() {
                        let label = format!(
                            "{} · {} game(s)",
                            preview.catalogue.display_name,
                            preview.candidates.len()
                        );
                        if ui
                            .selectable_label(self.open == Some(index), label)
                            .clicked()
                        {
                            chosen = Some(index);
                        }
                    }
                }
                if !outcome.skipped.is_empty() {
                    ui.collapsing("Catalogues not used", |ui| {
                        for (name, why) in &outcome.skipped {
                            ui.label(format!("{name}: {why}"));
                        }
                    });
                }
            }
        }
        if let Some(index) = chosen {
            self.open_preview(index, database);
        }
        if let Some(view) = self.corrections.view()
            && let Some(action) = panel::show(ui, &view)
        {
            self.corrections.handle(action, database);
        }
    }
}

impl Drop for OtherSystems {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat_sources_page::platform_corrections_panel::CorrectionsAction as A;
    use archivefs_core::dat::library_verification::{VerificationExclusion, VerificationFinding};
    use archivefs_core::dat::sources::{
        DatHealthState, DatSourceEntry, DatSourceKind, DatSourceRegistry,
    };
    use sha1::{Digest, Sha1};
    use std::os::unix::ffi::OsStrExt;

    const NES: &str = "Nintendo - Nintendo Entertainment System";

    #[test]
    fn only_unmatched_and_uncheckable_games_are_offered() {
        let totals = VerificationTotals {
            findings: vec![
                VerificationFinding {
                    path: "/r/verified.sfc".into(),
                    kind: VerificationFindingKind::Verified,
                },
                VerificationFinding {
                    path: "/r/nomatch.sfc".into(),
                    kind: VerificationFindingKind::NoMatch,
                },
                VerificationFinding {
                    path: "/r/ambiguous.sfc".into(),
                    kind: VerificationFindingKind::Ambiguous,
                },
            ],
            unchecked: vec![
                VerificationExclusion {
                    path: "/r/game.nes".into(),
                    reason: NOT_IN_CATALOGUE_FORMATS.into(),
                },
                VerificationExclusion {
                    path: "/r/unreadable.sfc".into(),
                    reason: "Unreadable".into(),
                },
            ],
            ..Default::default()
        };
        let paths: Vec<_> = unmatched_games(&totals, "SNES")
            .into_iter()
            .map(|g| g.path)
            .collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/r/game.nes"),
                PathBuf::from("/r/nomatch.sfc")
            ]
        );
    }

    fn settle(state: &mut OtherSystems, database: &Path) -> bool {
        let start = std::time::Instant::now();
        let mut changed = false;
        loop {
            changed |= state.poll(Some(database));
            if !state.busy() {
                return changed;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(30),
                "worker hung"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The real case through the Check My Games step: an SNES check leaves
    /// an NES game unmatched; looking in other systems' catalogues finds it
    /// in the NES catalogue as a Ready correction; confirming corrects it,
    /// Undo puts it back, and the file is never touched.
    #[test]
    fn an_nes_game_left_unmatched_by_snes_is_found_corrected_and_undone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("SNES");
        std::fs::create_dir(&root).unwrap();
        let game = root.join("Game One.nes");
        let bytes = b"nes game one".to_vec();
        std::fs::write(&game, &bytes).unwrap();
        let db = dir.path().join("library.sqlite3");
        archivefs_core::Database::open_or_create(&db).unwrap();
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute(
                "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')",
                [root.as_os_str().as_bytes()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) \
                 VALUES(1,1,?1,?2,?1,'direct_game_image','Game One','game one','now','now','now','now')",
                rusqlite::params![b"Game One.nes".as_slice(), game.as_os_str().as_bytes()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) VALUES(1,'SNES','folder_alias',1,'now')",
                [],
            )
            .unwrap();
        }
        let dat = dir.path().join("nes.dat");
        let sha1: String = Sha1::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        std::fs::write(
            &dat,
            format!(
                "<datafile><header><name>{NES}</name><description>{NES}</description><version>1</version><author>No-Intro</author></header>\
                 <game name=\"Game One (USA)\"><rom name=\"Game One.nes\" size=\"{}\" sha1=\"{sha1}\"/></game></datafile>",
                bytes.len()
            ),
        )
        .unwrap();
        let mut entry = DatSourceEntry::new(
            "nes".into(),
            "No-Intro NES".into(),
            dat.clone(),
            DatSourceKind::File,
        );
        entry.platform = Some("NES".into());
        entry.platform_chosen_by_person = true;
        entry.health.state = Some(DatHealthState::Valid);
        let mut local_registry = DatSourceRegistry::default();
        local_registry.add(entry).unwrap();
        let inventory = CatalogueInventorySnapshot {
            local_registry,
            managed_sources: Default::default(),
            managed_root: dir.path().join("managed"),
        };
        let current = || -> (String, String) {
            rusqlite::Connection::open(&db)
                .unwrap()
                .query_row(
                    "SELECT platform, source FROM platform_assignments WHERE archive_id = 1 AND is_current = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap()
        };

        // The SNES check left the game unmatched (its format is not SNES's).
        let totals = VerificationTotals {
            unchecked: vec![VerificationExclusion {
                path: game.clone(),
                reason: NOT_IN_CATALOGUE_FORMATS.into(),
            }],
            ..Default::default()
        };
        let mut state = OtherSystems::default();
        state.offer(&totals, "SNES", 1);
        assert_eq!(state.unmatched().len(), 1);

        // Looking changes nothing.
        state.start_for_test(inventory, db.clone());
        assert!(!settle(&mut state, &db));
        assert_eq!(state.error(), None);
        let outcome = state.outcome().expect("finished");
        assert_eq!(outcome.previews.len(), 1, "{:?}", outcome.skipped);
        assert_eq!(current(), ("SNES".into(), "folder_alias".into()));
        let view = state.corrections.view().expect("preview opened");
        assert_eq!(view.rows.len(), 1);
        assert_eq!(
            view.rows[0].group,
            archivefs_core::PlatformCorrectionGroup::Ready
        );
        assert!(view.rows[0].selected);
        assert!(
            view.rows[0].evidence.contains("SHA-1"),
            "{:?}",
            view.rows[0]
        );

        // Confirm.
        state.corrections.handle(A::Review, Some(&db));
        state.corrections.handle(
            A::Apply {
                proceed_with_rest: false,
            },
            Some(&db),
        );
        assert!(settle(&mut state, &db), "the library changed");
        assert_eq!(current().1, "verified_dat");
        assert_eq!(
            current().0,
            archivefs_core::canonical_platform_for_alias("NES").unwrap()
        );
        let receipt = state.corrections.view().unwrap().receipts[0].path.clone();

        // Undo.
        state.corrections.handle(
            A::Undo {
                receipt,
                game: None,
                proceed_with_rest: false,
            },
            Some(&db),
        );
        assert!(settle(&mut state, &db));
        assert_eq!(current(), ("SNES".into(), "folder_alias".into()));
        assert_eq!(
            std::fs::read(&game).unwrap(),
            bytes,
            "the file is never touched"
        );
    }
}
