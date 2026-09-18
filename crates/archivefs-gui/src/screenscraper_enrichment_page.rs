//! Explicit, single-game ScreenScraper enrichment review.
//!
//! Search is started only by the user from the selected-game action panel.
//! Candidates remain candidates until the user chooses one, and every field
//! is independently accepted or kept. No artwork is fetched and no identity
//! evidence is touched.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::identity_source::screenscraper::{
    LookupKind, LookupOutcome, ScreenScraperClient, ScreenScraperConfig, ScreenScraperCredentials,
    ScreenScraperEnrichment, ScreenScraperError, ScreenScraperLookup, UreqTransport,
};
use archivefs_core::screenscraper_enrichment::{
    AcceptedScreenScraperMetadata, EnrichmentField, ScreenScraperEnrichmentReceipt,
};
use archivefs_core::{ArchiveMetadata, ArchiveRecord};
use eframe::egui;

use crate::screenscraper_page::ScreenScraperPageState;
use crate::ui::components as widgets;

#[derive(Debug)]
pub(crate) enum ScreenScraperEnrichmentAction {
    Apply {
        archive_id: i64,
        values: AcceptedScreenScraperMetadata,
        receipt: ScreenScraperEnrichmentReceipt,
    },
}

#[derive(Debug)]
enum WorkflowView {
    Idle,
    Searching {
        path: PathBuf,
    },
    Candidates {
        path: PathBuf,
        candidates: Vec<ScreenScraperEnrichment>,
        quota: Option<String>,
    },
    Review {
        path: PathBuf,
        candidate: Box<ScreenScraperEnrichment>,
        choices: [bool; 9],
    },
    Applied,
    Error(String),
}

pub(crate) struct ScreenScraperEnrichmentState {
    view: WorkflowView,
    reply: Option<Receiver<Result<LookupOutcome, ScreenScraperError>>>,
}

impl Default for ScreenScraperEnrichmentState {
    fn default() -> Self {
        Self {
            view: WorkflowView::Idle,
            reply: None,
        }
    }
}

impl ScreenScraperEnrichmentState {
    pub(crate) fn mark_applied(&mut self) {
        self.view = WorkflowView::Applied;
    }

    fn poll(&mut self) {
        let Some(reply) = self.reply.take() else {
            return;
        };
        match reply.try_recv() {
            Ok(Ok(LookupOutcome::Candidates { candidates, quota })) => {
                let path = match &self.view {
                    WorkflowView::Searching { path } => path.clone(),
                    _ => return,
                };
                self.view = if candidates.is_empty() {
                    WorkflowView::Error("ScreenScraper returned no metadata candidates.".into())
                } else {
                    WorkflowView::Candidates {
                        path,
                        candidates,
                        quota: quota.map(|q| format!("{:?}", q)),
                    }
                };
            }
            Ok(Ok(LookupOutcome::NoResult { .. })) => {
                self.view = WorkflowView::Error(
                    "ScreenScraper found no metadata candidates. The library is unchanged.".into(),
                );
            }
            Ok(Err(error)) => self.view = WorkflowView::Error(public_error(&error)),
            Err(TryRecvError::Empty) => self.reply = Some(reply),
            Err(TryRecvError::Disconnected) => {
                self.view = WorkflowView::Error("The metadata request ended unexpectedly.".into())
            }
        }
    }

    fn begin_search(
        &mut self,
        record: &ArchiveRecord,
        credentials: Option<ScreenScraperCredentials>,
        provider_record_id: Option<&str>,
    ) {
        let Some(credentials) = credentials else {
            self.view = WorkflowView::Error(
                "Configure ScreenScraper credentials in Settings → Metadata Providers first."
                    .into(),
            );
            return;
        };
        let path = record.mount_plan.archive.path.clone();
        let title = record
            .metadata
            .title
            .clone()
            .or_else(|| Some(record.identity.display_name.clone()));
        let region = record.metadata.region.clone();
        let rom_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        let (sender, receiver) = mpsc::channel();
        self.view = WorkflowView::Searching { path };
        self.reply = Some(receiver);
        let lookup_kind = provider_record_id
            .and_then(|value| value.parse::<u64>().ok())
            .map_or(LookupKind::Search, |game_id| LookupKind::GameById {
                game_id,
            });
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
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut ScreenScraperEnrichmentState,
    settings: &ScreenScraperPageState,
    record: &ArchiveRecord,
    archive_id: i64,
    existing: Option<&archivefs_core::screenscraper_enrichment::PersistedScreenScraperEnrichment>,
) -> Option<ScreenScraperEnrichmentAction> {
    state.poll();
    let mut action = None;
    let mut next_view = None;
    ui.add_space(6.0);
    widgets::section_header(
        ui,
        "Metadata enrichment",
        Some("Optional ScreenScraper suggestions. Identity is unchanged."),
    );
    match &mut state.view {
        WorkflowView::Idle => {
            if widgets::action_button(
                ui,
                "Enrich metadata",
                widgets::ActionStyle::Secondary,
                state.reply.is_none() && archive_id != 0,
            )
            .clicked()
            {
                state.begin_search(
                    record,
                    settings.credentials(),
                    existing.map(|item| item.receipt.provider_record_id.as_str()),
                );
            }
        }
        WorkflowView::Applied => {
            widgets::banner(
                ui,
                "Metadata applied",
                "The selected fields were saved. Identity and source files were not changed.",
                widgets::StatusTone::Success,
            );
            if widgets::action_button(
                ui,
                "Enrich metadata",
                widgets::ActionStyle::Secondary,
                state.reply.is_none() && archive_id != 0,
            )
            .clicked()
            {
                state.begin_search(
                    record,
                    settings.credentials(),
                    existing.map(|item| item.receipt.provider_record_id.as_str()),
                );
            }
        }
        WorkflowView::Error(message) => {
            widgets::banner(
                ui,
                "Metadata enrichment",
                message,
                widgets::StatusTone::Warning,
            );
            if widgets::action_button(
                ui,
                "Enrich metadata",
                widgets::ActionStyle::Secondary,
                state.reply.is_none() && archive_id != 0,
            )
            .clicked()
            {
                state.begin_search(
                    record,
                    settings.credentials(),
                    existing.map(|item| item.receipt.provider_record_id.as_str()),
                );
            }
        }
        WorkflowView::Searching { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Searching ScreenScraper…");
            });
        }
        WorkflowView::Candidates {
            path,
            candidates,
            quota,
        } => {
            ui.label(format!("ScreenScraper found {} metadata candidate(s). Choose one to review; none is accepted automatically.", candidates.len()));
            if let Some(quota) = quota {
                ui.weak(format!("Provider quota: {quota}"));
            }
            for (index, candidate) in candidates.iter().enumerate() {
                let title = candidate
                    .title
                    .as_ref()
                    .map(|field| field.value.as_str())
                    .unwrap_or("Untitled provider result");
                let id = &candidate.provider_game_id;
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("Metadata candidate {}", index + 1));
                    ui.label(title);
                    ui.weak(format!("Provider ID {id}"));
                    if ui.button("Review").clicked() {
                        let choices = default_choices(candidate, &record.metadata);
                        next_view = Some(WorkflowView::Review {
                            path: path.clone(),
                            candidate: Box::new(candidate.clone()),
                            choices,
                        });
                    }
                });
            }
        }
        WorkflowView::Review {
            path,
            candidate,
            choices,
        } => {
            ui.label("Review the proposed changes before applying them. Keep existing values unless you explicitly select a provider value.");
            ui.label(format!("Provider result: {}", candidate.provider_game_id));
            egui::Grid::new("screenscraper_enrichment_diff")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("Field");
                    ui.strong("Existing");
                    ui.strong("ScreenScraper");
                    ui.strong("Use provider");
                    ui.end_row();
                    for (index, field) in EnrichmentField::ALL.into_iter().enumerate() {
                        let provider = field_value(candidate, field);
                        if provider.is_none() {
                            continue;
                        }
                        ui.label(field.label());
                        ui.label(existing_value(&record.metadata, field).unwrap_or("(empty)"));
                        ui.label(provider.unwrap_or("(empty)"));
                        ui.checkbox(&mut choices[index], "Use");
                        ui.end_row();
                    }
                });
            ui.label("Identity is unchanged. Media references are retained as provider references only; artwork is not downloaded.");
            ui.horizontal(|ui| {
                if ui.button("Back to candidates").clicked() {
                    // Preserve a reviewable state without silently applying.
                    next_view = Some(WorkflowView::Candidates { path: path.clone(), candidates: vec![candidate.as_ref().clone()], quota: None });
                }
                if widgets::action_button(ui, "Apply selected fields", widgets::ActionStyle::Primary, true).clicked() {
                    let values = selected_values(candidate, choices);
                    let receipt = ScreenScraperEnrichmentReceipt {
                        provider: "ScreenScraper".into(),
                        provider_record_id: candidate.provider_game_id.clone(),
                        retrieved_at_unix_seconds: candidate.title.as_ref().map(|field| field.provenance.retrieved_at_unix_seconds).unwrap_or(0),
                        match_basis: candidate.title.as_ref().map(|field| field.provenance.match_basis.clone()).unwrap_or_else(|| "provider result".into()),
                        before: archivefs_core::screenscraper_enrichment::AcceptedScreenScraperMetadata::from_existing(&record.metadata),
                        accepted: values.clone(),
                        media_reference_count: candidate.media_references.len(),
                    };
                    action = Some(ScreenScraperEnrichmentAction::Apply { archive_id, values, receipt });
                }
            });
        }
    }
    if let Some(next_view) = next_view {
        state.view = next_view;
    }
    // Keep the compiler honest about the selected path being part of the
    // session identity; it also prevents a result for another selection from
    // being presented as this game.
    if let WorkflowView::Review { path, .. } | WorkflowView::Candidates { path, .. } = &state.view
        && path != &record.mount_plan.archive.path
    {
        state.view = WorkflowView::Idle;
    }
    if let Some(existing) = existing {
        ui.weak(format!(
            "Last accepted ScreenScraper metadata: provider record {}.",
            existing.receipt.provider_record_id
        ));
    }
    action
}

fn default_choices(candidate: &ScreenScraperEnrichment, current: &ArchiveMetadata) -> [bool; 9] {
    EnrichmentField::ALL.map(|field| {
        existing_value(current, field).is_none() && field_value(candidate, field).is_some()
    })
}

fn existing_value(metadata: &ArchiveMetadata, field: EnrichmentField) -> Option<&str> {
    match field {
        EnrichmentField::Title => metadata.title.as_deref(),
        EnrichmentField::Synopsis => metadata.synopsis.as_deref(),
        EnrichmentField::Developer => metadata.developer.as_deref(),
        EnrichmentField::Publisher => metadata.publisher.as_deref(),
        EnrichmentField::Genre => metadata.genre.as_deref(),
        EnrichmentField::Players => metadata.players.as_deref(),
        EnrichmentField::Region => metadata.region.as_deref(),
        EnrichmentField::Languages => metadata.languages.as_ref().map(|_| "(set)"),
        EnrichmentField::ReleaseYear => metadata.release_year.map(|_| "(set)"),
    }
}

fn field_value(candidate: &ScreenScraperEnrichment, field: EnrichmentField) -> Option<&str> {
    match field {
        EnrichmentField::Title => candidate.title.as_ref().map(|v| v.value.as_str()),
        EnrichmentField::Synopsis => candidate.description.as_ref().map(|v| v.value.as_str()),
        EnrichmentField::Developer => candidate.developer.as_ref().map(|v| v.value.as_str()),
        EnrichmentField::Publisher => candidate.publisher.as_ref().map(|v| v.value.as_str()),
        EnrichmentField::Genre => candidate.genre.as_ref().map(|v| v.value.as_str()),
        EnrichmentField::Players => candidate.players.as_ref().map(|v| v.value.as_str()),
        EnrichmentField::Region => candidate.region.as_ref().map(|v| v.value.as_str()),
        EnrichmentField::Languages => candidate.language.as_ref().map(|v| v.value.as_str()),
        EnrichmentField::ReleaseYear => candidate.release_date.as_ref().map(|v| v.value.as_str()),
    }
}

fn selected_values(
    candidate: &ScreenScraperEnrichment,
    choices: &[bool; 9],
) -> AcceptedScreenScraperMetadata {
    let value = |field| field_value(candidate, field).map(str::to_owned);
    let languages = value(EnrichmentField::Languages).map(|value| {
        value
            .split(',')
            .map(|part| part.trim().to_owned())
            .filter(|part| !part.is_empty())
            .collect()
    });
    let release_year = value(EnrichmentField::ReleaseYear)
        .and_then(|value| value.get(0..4).and_then(|year| year.parse().ok()));
    AcceptedScreenScraperMetadata {
        title: choices[0].then(|| value(EnrichmentField::Title)).flatten(),
        synopsis: choices[1]
            .then(|| value(EnrichmentField::Synopsis))
            .flatten(),
        developer: choices[2]
            .then(|| value(EnrichmentField::Developer))
            .flatten(),
        publisher: choices[3]
            .then(|| value(EnrichmentField::Publisher))
            .flatten(),
        genre: choices[4].then(|| value(EnrichmentField::Genre)).flatten(),
        players: choices[5]
            .then(|| value(EnrichmentField::Players))
            .flatten(),
        region: choices[6].then(|| value(EnrichmentField::Region)).flatten(),
        languages: choices[7].then_some(languages).flatten(),
        release_year: choices[8].then_some(release_year).flatten(),
    }
}

fn public_error(error: &ScreenScraperError) -> String {
    match error {
        ScreenScraperError::Authentication { .. } => {
            "ScreenScraper rejected the credentials. The library is unchanged.".into()
        }
        ScreenScraperError::QuotaExhausted { .. } => {
            "ScreenScraper quota is exhausted. The library is unchanged.".into()
        }
        ScreenScraperError::Network { .. } => {
            "ScreenScraper is unavailable offline. The library is unchanged.".into()
        }
        _ => format!(
            "ScreenScraper could not provide metadata ({error:?}). The library is unchanged."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::identity_source::screenscraper::{
        EnrichedField, IdentityContribution, MetadataProvenance,
    };

    fn candidate() -> ScreenScraperEnrichment {
        let field = |value: &str| EnrichedField {
            value: value.into(),
            provenance: MetadataProvenance {
                provider: "ScreenScraper",
                provider_record_id: "7".into(),
                retrieved_at_unix_seconds: 9,
                match_basis: "search".into(),
            },
        };
        ScreenScraperEnrichment {
            identity_contribution: IdentityContribution::None,
            provider_game_id: "7".into(),
            title: Some(field("Provider title")),
            alternative_title: None,
            description: Some(field("Description")),
            release_date: Some(field("1998-01-01")),
            developer: Some(field("Developer")),
            publisher: None,
            genre: None,
            players: None,
            rating: None,
            region: Some(field("Europe")),
            language: Some(field("English,French")),
            external_url: None,
            media_references: Vec::new(),
        }
    }

    #[test]
    fn empty_fields_default_to_provider_and_existing_fields_default_to_keep() {
        let mut current = ArchiveMetadata::empty();
        current.title = Some("Local title".into());
        let choices = default_choices(&candidate(), &current);
        assert!(
            !choices[0],
            "stronger existing title is not silently replaced"
        );
        assert!(choices[1], "an empty description may be explicitly filled");
        assert!(choices[8], "a missing year may be explicitly filled");
    }

    #[test]
    fn selected_fields_are_the_only_values_in_the_apply_proposal() {
        let values = selected_values(
            &candidate(),
            &[false, true, false, false, false, false, false, true, false],
        );
        assert_eq!(values.title, None);
        assert_eq!(values.synopsis.as_deref(), Some("Description"));
        assert_eq!(
            values.languages,
            Some(vec!["English".into(), "French".into()])
        );
        assert_eq!(values.region, None);
    }
}
