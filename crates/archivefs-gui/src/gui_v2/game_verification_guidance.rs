//! Read-only Game Details guidance. Catalogue readiness is never game identity.
//! Selection, catalogue resolution, format eligibility and persistence remain
//! owned by the existing verification workflow.
use super::library::Game;
use archivefs_core::dat::catalogue_selection::{
    CatalogueAvailability, InstalledCatalogueSummary, list_installed_catalogues,
};
use archivefs_core::dat::library_identity_summary::DatProvenanceFreshness;
use archivefs_core::{Database, LibraryDatState};
use std::sync::mpsc::{self, Receiver};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    SelectDat,
    Validate,
    Audit,
    Recheck,
    Review,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::SelectDat => "Select DAT",
            Self::Validate => "Validate",
            Self::Audit => "Audit",
            Self::Recheck => "Re-check",
            Self::Review => "View evidence",
        }
    }

    pub(super) fn route(self, game: &Game) -> super::routes::Route {
        use super::routes::{Route, Section};
        match self {
            Self::Validate => Route::Section(Section::Dat),
            Self::Review => Route::ReviewIdentity(game.archive.id),
            Self::SelectDat | Self::Audit | Self::Recheck => {
                super::identity_review_page::setup_verification_route(Some(&game.platform))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Guidance {
    pub reason: &'static str,
    pub explanation: String,
    pub action: Action,
}

#[derive(Default)]
pub(super) struct State {
    game: Option<i64>,
    receiver: Option<Receiver<Guidance>>,
    result: Option<Guidance>,
}

impl State {
    /// Reuse the existing read-only worker; no separate evidence lookup or audit.
    pub(super) fn current(
        &mut self,
        game: &Game,
        context: &eframe::egui::Context,
    ) -> Option<&Guidance> {
        self.poll(game, context);
        self.result.as_ref()
    }

    pub fn invalidate(&mut self) {
        *self = Self::default();
    }

    fn poll(&mut self, game: &Game, context: &eframe::egui::Context) {
        if self.game != Some(game.archive.id) {
            self.invalidate();
            self.game = Some(game.archive.id);
            let game = game.clone();
            let context = context.clone();
            let (sender, receiver) = mpsc::channel();
            self.receiver = Some(receiver);
            std::thread::spawn(move || {
                let result = read_game(&game);
                let _ = sender.send(result);
                context.request_repaint();
            });
        }
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.result = Some(result);
                    self.receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => (),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.result = Some(unavailable());
                    self.receiver = None;
                }
            }
        }
    }
}

fn unavailable() -> Guidance {
    guidance(
        "Verification evidence unavailable",
        "Saved evidence or catalogue status could not be read. Open the existing verification workflow to check its status; no verification was concluded.",
        Action::SelectDat,
    )
}

fn read_game(game: &Game) -> Guidance {
    let result = (|| -> Result<_, String> {
        let metadata = std::fs::symlink_metadata(&game.archive.absolute_path).ok();
        let unchanged = metadata.as_ref().is_some_and(|m| {
            Some(m.len()) == game.archive.size_bytes
                && m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|d| i64::try_from(d.as_secs()).ok())
                    == game.archive.modified_time_unix_seconds
        });
        let database = Database::open_read_only(
            &archivefs_core::default_database_path().map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        // Structural refusals already documented by the bounded verifier;
        // catalogue-specific format exclusions remain with the preview owner.
        let unsupported = if metadata.as_ref().is_some_and(|m| !m.is_file()) {
            Some(
                "the bounded verifier requires a regular file, not a directory, symlink or special file",
            )
        } else if game.archive.absolute_path.to_str().is_none() {
            Some("the current audit engine cannot represent this non-Unicode path")
        } else {
            None
        };
        Ok(load(
            game,
            &database,
            metadata.is_some(),
            unchanged,
            unsupported,
        ))
    })();
    result.unwrap_or_else(|_| unavailable())
}

fn guidance(reason: &'static str, explanation: impl Into<String>, action: Action) -> Guidance {
    Guidance {
        reason,
        explanation: explanation.into(),
        action,
    }
}

/// Called only by the guidance worker. No ROM reads, DAT parsing,
/// resolution, scan, database write or background audit is started here.
fn load(
    game: &Game,
    database: &Database,
    present: bool,
    unchanged: bool,
    format_exclusion: Option<&str>,
) -> Guidance {
    let result = (|| -> Result<_, String> {
        let (checks, freshness) = saved_evidence(game, database)?;
        let snapshot =
            crate::dat_catalogue_picker::load_inventory_snapshot().map_err(|e| e.to_string())?;
        Ok((
            list_installed_catalogues(snapshot.inputs()),
            checks,
            freshness,
        ))
    })();
    match result {
        Ok((catalogues, checks, freshness)) => explain(
            game,
            present,
            unchanged,
            checks,
            freshness,
            Ok(&catalogues),
            format_exclusion,
        ),
        Err(error) => explain(
            game,
            present,
            unchanged,
            0,
            DatProvenanceFreshness::Unknown,
            Err(&error),
            None,
        ),
    }
}

/// Read stored stale marks and whole-disc metadata through the existing API.
/// With no current hashes/revision, absence of staleness is Unknown, never Current.
fn saved_evidence(
    game: &Game,
    database: &Database,
) -> Result<(usize, DatProvenanceFreshness), String> {
    let checks = database
        .library_dat_identities_for_item(game.archive.id)
        .map_err(|e| e.to_string())?;
    let mut stale = false;
    for check in &checks {
        let summary = database
            .library_dat_identity_summary_for_item(
                game.archive.id,
                &check.source.source_id,
                None,
                None,
                true,
            )
            .map_err(|e| e.to_string())?;
        if summary.is_some_and(|s| s.provenance_freshness == DatProvenanceFreshness::Stale) {
            stale = true;
        }
    }
    Ok((
        checks.len(),
        if stale {
            DatProvenanceFreshness::Stale
        } else {
            DatProvenanceFreshness::Unknown
        },
    ))
}

/// `format_exclusion` is reserved for a reason supplied by the owning audit
/// preview. Do not infer catalogue compatibility or exact format support from
/// file extensions, catalogue counts, a provider title or scan identification.
pub(super) fn explain(
    game: &Game,
    present: bool,
    unchanged: bool,
    saved_checks: usize,
    freshness: DatProvenanceFreshness,
    catalogues: Result<&[InstalledCatalogueSummary], &str>,
    format_exclusion: Option<&str>,
) -> Guidance {
    if !present || !unchanged || freshness == DatProvenanceFreshness::Stale {
        return guidance(
            "File evidence stale or changed",
            if !present {
                "The game file is unavailable. Reconnect its location, then re-check it against a DAT."
            } else if !unchanged {
                "The current file metadata does not agree with the saved scan, or the scan has no comparison baseline. Re-check before relying on a saved content result."
            } else {
                "The saved audit needs a re-check because its file evidence or catalogue revision is stale."
            },
            Action::Recheck,
        );
    }
    let catalogues = match catalogues {
        Ok(rows) => rows,
        Err(_) => {
            return guidance(
                "Catalogue status unavailable",
                "The catalogue inventory could not be read. Open DATs & Verification to check its status; no verification was concluded.",
                Action::SelectDat,
            );
        }
    };
    let platform = archivefs_core::canonical_platform_for_alias(&game.platform);
    let compatible: Vec<_> = catalogues
        .iter()
        .filter(|row| {
            row.enabled
                && platform.is_some()
                && row
                    .platform
                    .confirmed()
                    .and_then(|p| archivefs_core::canonical_platform_for_alias(p))
                    == platform
        })
        .collect();
    if compatible.is_empty() {
        let enabled: Vec<_> = catalogues.iter().filter(|row| row.enabled).collect();
        let (reason, explanation) = if catalogues.is_empty() {
            (
                "No trusted DAT configured",
                "No enabled DAT is configured. Explicitly import or enable compatible local game identification data, then Preview and confirm a check. No game verification has been attempted by opening this page.",
            )
        } else if enabled.is_empty() {
            (
                "Configured DAT unavailable or disabled",
                "DAT sources are registered, but none is enabled and available in the catalogue inventory. Restore or enable compatible local data before checking content; no mismatch was concluded.",
            )
        } else if platform.is_some()
            && enabled.iter().all(|row| {
                row.platform
                    .confirmed()
                    .and_then(|p| archivefs_core::canonical_platform_for_alias(p))
                    .is_some()
            })
        {
            (
                "Catalogue does not cover this system",
                "The enabled catalogues are assigned to other systems. This is a coverage gap, not a mismatch with this game. Import or select compatible data explicitly; no game check starts here.",
            )
        } else {
            (
                "Catalogue system coverage unresolved",
                "An enabled catalogue has unknown or ambiguous system coverage, or this game's system is unresolved. Review its system assignment before checking content. This does not establish a match or a mismatch.",
            )
        };
        return guidance(reason, explanation, Action::SelectDat);
    }
    if !compatible
        .iter()
        .any(|row| row.availability == CatalogueAvailability::Ready && row.capabilities.verify)
    {
        if compatible.iter().any(|row| {
            matches!(
                row.availability,
                CatalogueAvailability::AggregateFolder { .. } | CatalogueAvailability::Ready
            )
        }) {
            return guidance(
                "Catalogue selection required",
                "The assigned source is a folder of catalogues, or does not support game verification. Select an individual game DAT in the existing verification workflow; validating the same source alone will not select one.",
                Action::SelectDat,
            );
        }
        let reason = compatible
            .first()
            .map(|row| row.availability.reason())
            .unwrap_or("");
        return guidance(
            "Catalogue not validated or unavailable",
            format!(
                "A catalogue is assigned to this system, but none is ready for exact verification. {reason} Open DATs & Verification to validate it or restore an available catalogue."
            ),
            Action::Validate,
        );
    }
    if let Some(reason) = format_exclusion {
        return guidance(
            "File format unsupported for exact verification",
            format!(
                "This representation cannot be verified exactly: {reason}. Review the supported content and catalogue in the existing verification workflow."
            ),
            Action::SelectDat,
        );
    }
    // Only the existing library projection can supply this exact-match flag.
    if game.dat_exact.is_some() && freshness == DatProvenanceFreshness::Current {
        return guidance(
            "Verified",
            "A saved exact DAT match has current audit evidence. This is separate from whether the game is ready to play.",
            Action::Review,
        );
    }
    // The current-evidence integration belongs to the persistence owner. A
    // scan comparison or Unknown summary must never stand in for that proof.
    if game.dat_exact.is_some() {
        return guidance(
            "Saved exact match; freshness needs checking",
            "An exact DAT match was saved, but current file and catalogue freshness has not been established here. Re-check before relying on it as current verification. Catalogue installation and a later library scan cannot renew an earlier audit.",
            Action::Recheck,
        );
    }
    match game.dat_state {
        Some(LibraryDatState::NoMatch) => guidance(
            "No catalogue match",
            "A saved audit found no match in the catalogue applicable to this game's system. This does not prove the file is corrupt. Re-check with the appropriate catalogue.",
            Action::Recheck,
        ),
        Some(
            LibraryDatState::Verified | LibraryDatState::Probable | LibraryDatState::Ambiguous,
        ) => guidance(
            "Audit needs review",
            "The saved evidence is weak, ambiguous or conflicting; it does not establish one exact game identity. Review the evidence before choosing the next audit.",
            Action::Review,
        ),
        None if saved_checks > 0 => guidance(
            "Audit has no usable exact result",
            "A saved audit exists but did not establish an applicable exact match or a reliable no-match result. Review its evidence and re-check; this is not a new no-match verdict.",
            Action::Recheck,
        ),
        None => guidance(
            "Audit not yet performed",
            "A compatible catalogue is ready, but no applicable DAT audit result is saved for this game. Catalogue installation and validation do not check game content. Open Audit to select the catalogue and preview eligible files; nothing starts from this page.",
            Action::Audit,
        ),
    }
}

impl super::App {
    pub(super) fn detail_verification_guidance(&mut self, ui: &mut eframe::egui::Ui, game: &Game) {
        // Preserve already-loaded non-exact audit outcomes on the first frame,
        // while the read-only guidance worker loads. These are saved results,
        // not assertions that the file or catalogue is current.
        if game.dat_exact.is_none() && game.dat_state.is_some() {
            ui.small("Saved DAT audit result (not a current freshness check)");
            ui.label(format!("DAT: {}", game.dat_identity_summary()));
        }
        self.review.verification_guidance.poll(game, ui.ctx());
        let guidance = self.review.verification_guidance.result.clone();
        let Some(guidance) = guidance else {
            ui.strong("DAT: checking verification guidance…");
            ui.label("Reading saved evidence and catalogue status; no content audit is running.");
            return;
        };
        if show(ui, &guidance) {
            if guidance.action == Action::Validate {
                self.review.dat_context = Some((game.archive.id, game.platform.clone()));
            }
            // Navigation only: the existing workflow owns selection,
            // population preview and explicit audit confirmation.
            self.go(guidance.action.route(game));
        }
    }
}

fn show(ui: &mut eframe::egui::Ui, guidance: &Guidance) -> bool {
    ui.strong(format!("DAT: {}", guidance.reason));
    ui.label(&guidance.explanation);
    let clicked = ui.button(guidance.action.label()).clicked();
    ui.small(match guidance.action {
            Action::Validate => "Opens DATs & Verification. Validate the catalogue there.",
            Action::Review => "Opens this game's saved evidence.",
            _ => "Opens verification for this system. Choose a DAT and review the preview before starting an audit.",
        });
    clicked
}
