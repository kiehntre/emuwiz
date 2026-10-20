//! Selected-game manuals reuse the existing document model and read-only viewer.
use super::{
    App,
    documents::{self, DocumentPreferences, GameDocument},
    library::Game,
    media_sources::MediaIndex,
};
use eframe::egui;
use std::sync::Arc;

pub(super) fn discover_game_documents(
    game: &Game,
    preferences: &DocumentPreferences,
    index: Option<&MediaIndex>,
) -> (Vec<GameDocument>, Option<String>) {
    let mut documents = documents::discover_documents(documents::DocumentDiscoveryRequest {
        game_id: game.archive.id,
        game_title: &game.title,
        platform: &game.platform,
        game_path: &game.archive.absolute_path,
        roots: &preferences.roots,
        associations: &preferences.associations,
        verified_identity: None,
    });
    let Some(index) = index else {
        return (documents, None);
    };
    let (record_id, manual) = match index.manual_reference_checked(game) {
        Ok(Some(found)) => found,
        Ok(None) => return (documents, None),
        Err(refusal) => return (documents, Some(refusal.to_string())),
    };
    let mapping = index
        .settings
        .source
        .media_mapping
        .as_ref()
        .and_then(|mapping| {
            archivefs_core::identity_source::romm::media_mapping::validate_romm_media_mapping(
                mapping,
            )
            .ok()
        });
    match documents::project_romm_manual_document(documents::RommDocumentRequest {
        game_id: game.archive.id,
        platform: &game.platform,
        mapping: mapping.as_ref(),
        manual,
        verified_identity: None,
    }) {
        Ok(mut document) => {
            if !documents::associated_with_another_game(
                &document.path,
                game.archive.id,
                &preferences.associations,
            ) {
                document.association_reason = format!(
                    "RomM record {record_id} matched this game's exact library path and platform. This is a provider association, not document-content verification."
                );
                documents::merge_romm_document(&mut documents, document);
            }
            (documents, None)
        }
        Err(reason) => (documents, Some(reason.user_message().to_string())),
    }
}

impl App {
    pub(super) fn documents_panel(&mut self, ui: &mut egui::Ui, game_id: i64, game: &Game) {
        let path = game.archive.absolute_path.clone();
        let provider_changed = match (&self.document_index, &self.artwork.index) {
            (Some(old), Some(current)) => !Arc::ptr_eq(old, current),
            (None, None) => false,
            _ => true,
        };
        if provider_changed
            || self
                .document_cache
                .as_ref()
                .is_none_or(|(id, cached_path, _)| *id != game_id || *cached_path != path)
        {
            let (documents, issue) = discover_game_documents(
                game,
                &self.document_preferences,
                self.artwork.index.as_deref(),
            );
            self.document_cache = Some((game_id, path, documents));
            self.document_index = self.artwork.index.clone();
            self.document_issue = issue;
        }
        let documents = self
            .document_cache
            .as_ref()
            .map(|(_, _, docs)| docs.clone())
            .unwrap_or_default();
        egui::CollapsingHeader::new("Manuals & Guides")
            .id_salt(("v2_manuals", game_id))
            .default_open(true)
            .show(ui, |ui| {
                ui.label("Local documents and saved associations are shown here. EmuWiz never downloads or changes document files.");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Associate local document…").clicked()
                        && let Some(path) = rfd::FileDialog::new().set_title("Associate a manual or guide with this game").add_filter("Manuals and guides", &["pdf", "cbz", "cbr"]).pick_file()
                    {
                        match documents::associate_local_document(&path, game_id, &mut self.document_preferences) {
                            Ok(()) => {
                                self.document_cache = None;
                                self.preferences_dirty = Some(std::time::Instant::now());
                            }
                            Err(message) => self.document_issue = Some(message),
                        }
                    }
                    if ui.button("Refresh documents").clicked() { self.document_cache = None; }
                });
                if let Some(issue) = &self.document_issue { ui.label(issue); }
                if documents.is_empty() {
                    ui.weak("No manual or guide is associated with this game. Choose a local document to add one.");
                    return;
                }
            for document in documents {
            ui.push_id(("document", &document.path), |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&document.title);
                    ui.label(format!("{} · {}", document.format.label(), document.page_count.map_or("page count unknown".into(), |count| format!("{count} pages"))));
                    ui.weak(document.kind.label());
                });
                match &document.association {
                    super::documents::GameDocumentAssociation::Explicit
                    | super::documents::GameDocumentAssociation::RommRecord
                    | super::documents::GameDocumentAssociation::ExactGameIdentity(_) => {
                        ui.label("This manual is associated with the selected game.");
                    }
                    _ => {
                        ui.label("Needs review · this document is nearby or its filename resembles the game; ownership is not confirmed.");
                    }
                }
                ui.weak(super::manual_viewer::capability_text(document.readiness));
                if let Some(reason) = &document.refusal_reason { ui.label(reason); }
                ui.horizontal_wrapped(|ui| {
                    self.manual_document_actions(ui, game_id, &document);
                    if document.association != super::documents::GameDocumentAssociation::Explicit
                        && ui.button("Associate").clicked()
                    {
                        match documents::associate_local_document(&document.path, game_id, &mut self.document_preferences) {
                            Ok(()) => {
                                self.document_cache = None;
                                self.preferences_dirty = Some(std::time::Instant::now());
                            }
                            Err(message) => self.document_issue = Some(message),
                        }
                    } else if document.association == super::documents::GameDocumentAssociation::Explicit
                        && ui.button("Remove association").clicked()
                    {
                        documents::remove_local_association(&document.path, game_id, &mut self.document_preferences);
                        self.document_cache = None;
                        self.preferences_dirty = Some(std::time::Instant::now());
                    }
                    if document.viewer != super::documents::DocumentOpenCapability::Supported {
                        ui.weak(match document.viewer {
                            super::documents::DocumentOpenCapability::NoHandler => "No desktop application is available to open this format.",
                            super::documents::DocumentOpenCapability::UnsupportedFormat => "External opening is unavailable for this document.",
                            super::documents::DocumentOpenCapability::MissingFile => "The document file is no longer available.",
                            super::documents::DocumentOpenCapability::Supported => "",
                        });
                    }
                });
                ui.collapsing("View details", |ui| {
                    ui.label(format!("Association evidence: {}", document.association_reason));
                    ui.label(format!("Source: {}", document.source_label()));
                    ui.monospace(format!("Path: {}", document.path.display()));
                    ui.label(format!("File size: {} bytes", document.file_size));
                    ui.label(format!("Association confidence: {}", document.association.label()));
                    ui.label("Documents are read-only. Internal reading and external opening do not change the source file.");
                });
                });
            });
            }
        });
    }
}
