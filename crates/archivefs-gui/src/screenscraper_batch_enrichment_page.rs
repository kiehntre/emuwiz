//! Bounded, explicitly selected ScreenScraper enrichment.
//!
//! This is a foreground coordinator around the existing single-game lookup
//! and apply paths. It never discovers its own library selection, downloads
//! artwork, or changes identity evidence.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::identity_source::screenscraper::{
    LookupKind, LookupOutcome, ScreenScraperClient, ScreenScraperConfig, ScreenScraperCredentials,
    ScreenScraperEnrichment, ScreenScraperError, ScreenScraperLookup, UreqTransport,
};
use archivefs_core::screenscraper_enrichment::{
    AcceptedScreenScraperMetadata, EnrichmentField, MAX_EXPLICIT_BATCH_SIZE,
    ScreenScraperBatchLookupRoute, ScreenScraperEnrichmentReceipt, batch_lookup_route,
    validate_explicit_batch_size,
};
use archivefs_core::{ArchiveRecord, PersistedArchive};
use eframe::egui;

use crate::database_load::CachedLibrarySnapshot;
use crate::screenscraper_enrichment_page as single;
use crate::screenscraper_page::ScreenScraperPageState;
use crate::ui::components as widgets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchItemState {
    Pending,
    LookingUp,
    CandidateReviewRequired,
    Applied,
    Skipped,
    NoResult,
    AuthenticationFailed,
    QuotaStopped,
    ProviderUnavailable,
    ApplyFailed,
}

impl BatchItemState {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::LookingUp => "Looking up",
            Self::CandidateReviewRequired => "Candidate review required",
            Self::Applied => "Applied",
            Self::Skipped => "Skipped",
            Self::NoResult => "No result",
            Self::AuthenticationFailed => "Authentication failed",
            Self::QuotaStopped => "Stopped by quota",
            Self::ProviderUnavailable => "Provider unavailable",
            Self::ApplyFailed => "Apply failed",
        }
    }
}

#[derive(Debug, Clone)]
struct BatchItem {
    archive_id: i64,
    path: PathBuf,
    record: ArchiveRecord,
    provider_record_id: Option<String>,
}

#[derive(Debug)]
struct BatchSession {
    items: Vec<BatchItem>,
    states: Vec<BatchItemState>,
    current: usize,
    started: bool,
    cancelled: bool,
    reply: Option<Receiver<Result<LookupOutcome, ScreenScraperError>>>,
    candidate: Option<ScreenScraperEnrichment>,
    candidate_options: Vec<ScreenScraperEnrichment>,
    choices: [bool; 9],
    quota: Option<String>,
    requests_used: usize,
    message: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ScreenScraperBatchState {
    session: Option<BatchSession>,
}

impl ScreenScraperBatchState {
    pub(crate) fn mark_applied(&mut self, archive_id: i64) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let Some(item) = session.items.get(session.current) else {
            return false;
        };
        if item.archive_id != archive_id {
            return false;
        }
        session.states[session.current] = BatchItemState::Applied;
        session.current += 1;
        session.candidate = None;
        session.reply = None;
        session.message = Some(
            "Metadata applied for this game; identity and source files were unchanged.".into(),
        );
        true
    }

    fn prepare(
        &mut self,
        selected: &std::collections::HashSet<PathBuf>,
        records: &[ArchiveRecord],
        cached: &CachedLibrarySnapshot,
    ) -> Result<(), String> {
        validate_explicit_batch_size(selected.len())?;
        let mut paths: Vec<_> = selected.iter().cloned().collect();
        paths.sort();
        let mut items = Vec::with_capacity(paths.len());
        for path in paths {
            let record = records
                .iter()
                .find(|record| record.mount_plan.archive.path == path)
                .cloned()
                .ok_or_else(|| {
                    format!("{} is not in the current library snapshot.", path.display())
                })?;
            let persisted: &PersistedArchive = cached
                .archives
                .iter()
                .find(|archive| archive.absolute_path == path)
                .ok_or_else(|| format!("{} has no persisted catalogue ID.", path.display()))?;
            let provider_record_id = cached
                .screenscraper_enrichments
                .get(&persisted.id)
                .map(|item| item.receipt.provider_record_id.clone());
            items.push(BatchItem {
                archive_id: persisted.id,
                path,
                record,
                provider_record_id,
            });
        }
        self.session = Some(BatchSession {
            states: vec![BatchItemState::Pending; items.len()],
            items,
            current: 0,
            started: false,
            cancelled: false,
            reply: None,
            candidate: None,
            candidate_options: Vec::new(),
            choices: [false; 9],
            quota: None,
            requests_used: 0,
            message: None,
        });
        Ok(())
    }

    fn begin_current(session: &mut BatchSession, credentials: ScreenScraperCredentials) {
        let Some(item) = session.items.get(session.current) else {
            return;
        };
        if session.reply.is_some() || session.candidate.is_some() || session.cancelled {
            return;
        }
        let title = item
            .record
            .metadata
            .title
            .clone()
            .or_else(|| Some(item.record.identity.display_name.clone()));
        let region = item.record.metadata.region.clone();
        let rom_name = item
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        let lookup_kind = match batch_lookup_route(item.provider_record_id.as_deref()) {
            ScreenScraperBatchLookupRoute::DirectProviderId(game_id) => {
                LookupKind::GameById { game_id }
            }
            ScreenScraperBatchLookupRoute::Search => LookupKind::Search,
        };
        let (sender, receiver) = mpsc::channel();
        session.states[session.current] = BatchItemState::LookingUp;
        session.reply = Some(receiver);
        session.requests_used += 1;
        thread::spawn(move || {
            let client = ScreenScraperClient::new(
                ScreenScraperConfig {
                    enabled: true,
                    ..ScreenScraperConfig::default()
                },
                Some(credentials),
                UreqTransport::default(),
            );
            let result = client.lookup(
                lookup_kind,
                &ScreenScraperLookup {
                    title,
                    region,
                    rom_name,
                    ..ScreenScraperLookup::default()
                },
            );
            let _ = sender.send(result);
        });
    }

    fn poll(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(reply) = session.reply.take() else {
            return;
        };
        match reply.try_recv() {
            Ok(Ok(LookupOutcome::Candidates { candidates, quota })) => {
                session.quota = quota.map(|value| format!("{value:?}"));
                if candidates.len() == 1 {
                    let candidate = candidates.into_iter().next().expect("length checked");
                    session.choices = single::default_choices(
                        &candidate,
                        &session.items[session.current].record.metadata,
                    );
                    session.candidate = Some(candidate);
                    session.states[session.current] = BatchItemState::CandidateReviewRequired;
                } else if candidates.len() > 1 {
                    session.candidate_options = candidates;
                    session.states[session.current] = BatchItemState::CandidateReviewRequired;
                } else {
                    session.states[session.current] = BatchItemState::NoResult;
                    session.message =
                        Some("No metadata candidate was returned. The game was unchanged.".into());
                }
            }
            Ok(Ok(LookupOutcome::NoResult { quota })) => {
                session.quota = quota.map(|value| format!("{value:?}"));
                session.states[session.current] = BatchItemState::NoResult;
                session.message =
                    Some("No metadata candidate was returned. The game was unchanged.".into());
            }
            Ok(Err(error)) => {
                session.states[session.current] = match error {
                    ScreenScraperError::Authentication { .. } => {
                        BatchItemState::AuthenticationFailed
                    }
                    ScreenScraperError::QuotaExhausted { .. } => BatchItemState::QuotaStopped,
                    ScreenScraperError::Network { .. } | ScreenScraperError::Temporary { .. } => {
                        BatchItemState::ProviderUnavailable
                    }
                    _ => BatchItemState::ApplyFailed,
                };
                session.message = Some(single::public_error(&error));
            }
            Err(TryRecvError::Empty) => session.reply = Some(reply),
            Err(TryRecvError::Disconnected) => {
                session.states[session.current] = BatchItemState::ProviderUnavailable;
                session.message =
                    Some("The metadata request ended unexpectedly. The game was unchanged.".into());
            }
        }
    }

    fn advance(session: &mut BatchSession) {
        if session.current < session.items.len()
            && matches!(
                session.states[session.current],
                BatchItemState::NoResult
                    | BatchItemState::Skipped
                    | BatchItemState::AuthenticationFailed
                    | BatchItemState::ProviderUnavailable
                    | BatchItemState::ApplyFailed
            )
        {
            session.current += 1;
            session.message = None;
        }
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut ScreenScraperBatchState,
    settings: &ScreenScraperPageState,
    records: &[ArchiveRecord],
    cached: Option<&CachedLibrarySnapshot>,
    selected: &std::collections::HashSet<PathBuf>,
) -> Option<crate::screenscraper_enrichment_page::ScreenScraperEnrichmentAction> {
    let mut action = None;
    widgets::card(ui, |ui| {
        widgets::section_header(
            ui,
            "Batch metadata enrichment",
            Some("Explicit selection only; identity and source files are unchanged."),
        );
        if state.session.is_none() {
            let count = selected.len();
            ui.label(format!(
                "{count} game(s) selected. Maximum per batch: {MAX_EXPLICIT_BATCH_SIZE}."
            ));
            if count > MAX_EXPLICIT_BATCH_SIZE {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!(
                        "Reduce the selection to {MAX_EXPLICIT_BATCH_SIZE} games before starting."
                    ),
                );
            }
            let enabled = count > 0
                && count <= MAX_EXPLICIT_BATCH_SIZE
                && settings.credentials().is_some()
                && cached.is_some();
            if ui
                .add_enabled(enabled, egui::Button::new("Prepare enrichment batch"))
                .clicked()
                && let Some(snapshot) = cached
                && let Err(error) = state.prepare(selected, records, snapshot)
            {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if settings.credentials().is_none() {
                ui.weak(
                    "Configure ScreenScraper credentials in Settings → Metadata Providers first.",
                );
            }
            return;
        }

        state.poll();
        let Some(session) = state.session.as_mut() else {
            return;
        };
        let total = session.items.len();
        let applied = session
            .states
            .iter()
            .filter(|state| **state == BatchItemState::Applied)
            .count();
        let review = session
            .states
            .iter()
            .filter(|state| **state == BatchItemState::CandidateReviewRequired)
            .count();
        let skipped = session
            .states
            .iter()
            .filter(|state| **state == BatchItemState::Skipped)
            .count();
        let no_result = session
            .states
            .iter()
            .filter(|state| **state == BatchItemState::NoResult)
            .count();
        let quota_stopped = session
            .states
            .iter()
            .filter(|state| **state == BatchItemState::QuotaStopped)
            .count();
        let not_processed = session
            .states
            .iter()
            .filter(|state| **state == BatchItemState::Pending)
            .count();
        ui.label(format!(
            "Selected: {total}    Applied: {applied}    Skipped: {skipped}    Review: {review}    No result: {no_result}    Requests used: {}",
            session.requests_used
        ));
        if quota_stopped > 0 || not_processed > 0 {
            ui.weak(format!(
                "Stopped by quota: {quota_stopped}    Not processed: {not_processed}"
            ));
        }
        if let Some(quota) = &session.quota {
            ui.weak(format!("Provider quota: {quota}"));
        }

        if !session.started {
            ui.label("Preview ready. Each game will be processed one at a time and reviewed before its own atomic apply.");
            let direct = session
                .items
                .iter()
                .filter(|item| {
                    matches!(
                        batch_lookup_route(item.provider_record_id.as_deref()),
                        ScreenScraperBatchLookupRoute::DirectProviderId(_)
                    )
                })
                .count();
            ui.label(format!(
                "Direct provider-ID lookups: {direct}    Search required: {}    Minimum requests: {total}",
                total - direct
            ));
            ui.weak("Provider status: credentials available; quota is reported and updated by responses.");
            if ui.button("Start selected enrichment").clicked() {
                session.started = true;
            }
            if ui.button("Cancel batch").clicked() {
                state.session = None;
            }
            return;
        }

        if session.current >= total {
            widgets::banner(
                ui,
                "Batch complete",
                "All selected games have a recorded outcome. No identity or source files were changed.",
                widgets::StatusTone::Success,
            );
            if ui.button("Close batch").clicked() {
                state.session = None;
            }
        } else if session.cancelled {
            ui.label("Batch cancelled. Completed games remain committed; remaining games were untouched.");
            if ui.button("Close batch").clicked() {
                state.session = None;
            }
        } else {
            let current = session.current;
            let item = session.items[current].clone();
            ui.label(format!(
                "Game {}/{}: {}",
                current + 1,
                total,
                item.path.display()
            ));
            if session.reply.is_none()
                && session.candidate.is_none()
                && session.states[current] == BatchItemState::Pending
                && let Some(credentials) = settings.credentials()
            {
                ScreenScraperBatchState::begin_current(session, credentials);
            }
            if !session.candidate_options.is_empty() {
                ui.label("Multiple metadata candidates require review. Choose one; none is accepted automatically.");
                let options = session.candidate_options.clone();
                for (index, option) in options.iter().enumerate() {
                    let title = option
                        .title
                        .as_ref()
                        .map(|field| field.value.as_str())
                        .unwrap_or("Untitled provider result");
                    if ui
                        .button(format!(
                            "Review candidate {} — {} ({})",
                            index + 1,
                            title,
                            option.provider_game_id
                        ))
                        .clicked()
                    {
                        session.choices = single::default_choices(option, &item.record.metadata);
                        session.candidate = Some(option.clone());
                        session.candidate_options.clear();
                    }
                }
                if ui.button("Skip this game").clicked() {
                    session.states[current] = BatchItemState::Skipped;
                    session.candidate_options.clear();
                    ScreenScraperBatchState::advance(session);
                }
            } else if let Some(candidate) = session.candidate.clone() {
                ui.label("Candidate review required. No candidate is accepted automatically.");
                ui.label(format!("Provider record: {}", candidate.provider_game_id));
                egui::Grid::new("screenscraper_batch_diff")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Field");
                        ui.strong("Existing");
                        ui.strong("ScreenScraper");
                        ui.strong("Use");
                        ui.end_row();
                        for (index, field) in EnrichmentField::ALL.into_iter().enumerate() {
                            let provider = single::field_value(&candidate, field);
                            if provider.is_none() {
                                continue;
                            }
                            ui.label(field.label());
                            ui.label(
                                single::existing_value(&item.record.metadata, field)
                                    .unwrap_or("(empty)"),
                            );
                            ui.label(provider.unwrap_or("(empty)"));
                            ui.checkbox(&mut session.choices[index], "Use");
                            ui.end_row();
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("Skip this game").clicked() {
                        session.states[current] = BatchItemState::Skipped;
                        session.candidate = None;
                        ScreenScraperBatchState::advance(session);
                    }
                    if widgets::action_button(ui, "Apply selected fields", widgets::ActionStyle::Primary, true).clicked() {
                        let values = single::selected_values(&candidate, &session.choices);
                        let receipt = ScreenScraperEnrichmentReceipt {
                            provider: "ScreenScraper".into(),
                            provider_record_id: candidate.provider_game_id.clone(),
                            retrieved_at_unix_seconds: candidate.title.as_ref().map(|field| field.provenance.retrieved_at_unix_seconds).unwrap_or(0),
                            match_basis: candidate.title.as_ref().map(|field| field.provenance.match_basis.clone()).unwrap_or_else(|| "provider result".into()),
                            before: AcceptedScreenScraperMetadata::from_existing(&item.record.metadata),
                            accepted: values.clone(),
                            media_reference_count: candidate.media_references.len(),
                        };
                        action = Some(crate::screenscraper_enrichment_page::ScreenScraperEnrichmentAction::Apply { archive_id: item.archive_id, values, receipt });
                    }
                });
            } else if session.reply.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking up this game…");
                });
            } else {
                ui.label(session.states[current].label());
                if let Some(message) = &session.message {
                    ui.weak(message);
                }
                if matches!(
                    session.states[current],
                    BatchItemState::NoResult
                        | BatchItemState::Skipped
                        | BatchItemState::ApplyFailed
                ) && ui.button("Continue").clicked()
                {
                    ScreenScraperBatchState::advance(session);
                }
                if matches!(
                    session.states[current],
                    BatchItemState::AuthenticationFailed
                        | BatchItemState::QuotaStopped
                        | BatchItemState::ProviderUnavailable
                ) {
                    session.cancelled = true;
                }
            }
            if ui.button("Cancel remaining games").clicked() {
                session.cancelled = true;
            }
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::screenscraper_enrichment::MAX_EXPLICIT_BATCH_SIZE;

    #[test]
    fn batch_limit_is_conservative_and_explicit() {
        assert_eq!(MAX_EXPLICIT_BATCH_SIZE, 25);
    }

    #[test]
    fn states_remain_per_game() {
        assert_ne!(BatchItemState::Applied, BatchItemState::QuotaStopped);
        assert_ne!(BatchItemState::Skipped, BatchItemState::NoResult);
    }
}
