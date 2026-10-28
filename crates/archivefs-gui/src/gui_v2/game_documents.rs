//! Selected-game manuals reuse the existing document model and read-only viewer.
use super::{
    App,
    documents::{self, DocumentPreferences, GameDocument},
    library::Game,
    media_sources::MediaIndex,
};
use eframe::egui;
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const STALE_AFTER: Duration = Duration::from_secs(8);

struct RefreshRequest {
    generation: u64,
    game: Game,
    preferences: DocumentPreferences,
    index: Option<Arc<MediaIndex>>,
}

struct RefreshReply {
    generation: u64,
    documents: Vec<GameDocument>,
    issue: Option<String>,
}

/// One worker and one pending request; slow mounts cannot accumulate threads or
/// filesystem work on the UI thread. Results for a previous game are ignored.
#[derive(Default)]
pub(super) struct DocumentRefresh {
    sender: Option<SyncSender<RefreshRequest>>,
    receiver: Option<Receiver<RefreshReply>>,
    generation: u64,
    pending: bool,
    last_confirmed: Option<Instant>,
}

impl DocumentRefresh {
    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.last_confirmed = Some(Instant::now());
    }

    fn ensure_worker(&mut self, context: &egui::Context) {
        if self.sender.is_some() {
            return;
        }
        let (sender, requests) = mpsc::sync_channel::<RefreshRequest>(1);
        let (replies, receiver) = mpsc::channel();
        let repaint = context.clone();
        if std::thread::Builder::new()
            .name("manual-document-refresh".into())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    let (documents, issue) = discover_game_documents(
                        &request.game,
                        &request.preferences,
                        request.index.as_deref(),
                    );
                    if replies
                        .send(RefreshReply {
                            generation: request.generation,
                            documents,
                            issue,
                        })
                        .is_err()
                    {
                        break;
                    }
                    repaint.request_repaint();
                }
            })
            .is_ok()
        {
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
    }

    fn poll(&mut self, context: &egui::Context) -> Option<RefreshReply> {
        let mut latest = None;
        if let Some(receiver) = &self.receiver {
            while let Ok(reply) = receiver.try_recv() {
                if reply.generation == self.generation {
                    self.pending = false;
                    self.last_confirmed = Some(Instant::now());
                    latest = Some(reply);
                }
            }
        }
        context.request_repaint_after(REFRESH_INTERVAL);
        latest
    }

    fn request(
        &mut self,
        context: &egui::Context,
        game: &Game,
        preferences: &DocumentPreferences,
        index: Option<Arc<MediaIndex>>,
    ) {
        if self.pending
            || self
                .last_confirmed
                .is_some_and(|at| at.elapsed() < REFRESH_INTERVAL)
        {
            return;
        }
        self.ensure_worker(context);
        if let Some(sender) = &self.sender {
            match sender.try_send(RefreshRequest {
                generation: self.generation,
                game: game.clone(),
                preferences: preferences.clone(),
                index,
            }) {
                Ok(()) => self.pending = true,
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => {
                    self.sender = None;
                    self.receiver = None;
                }
            }
        }
    }

    pub(super) fn stale(&self) -> bool {
        self.last_confirmed
            .is_none_or(|at| at.elapsed() >= STALE_AFTER)
    }

    #[cfg(test)]
    pub(super) fn block_worker_for_test(&mut self) -> mpsc::Sender<()> {
        let (sender, requests) = mpsc::sync_channel::<RefreshRequest>(1);
        let (replies, receiver) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(request) = requests.recv() {
                if blocked.recv().is_ok() {
                    let (documents, issue) = discover_game_documents(
                        &request.game,
                        &request.preferences,
                        request.index.as_deref(),
                    );
                    let _ = replies.send(RefreshReply {
                        generation: request.generation,
                        documents,
                        issue,
                    });
                }
            }
        });
        self.sender = Some(sender);
        self.receiver = Some(receiver);
        self.last_confirmed = Some(Instant::now() - REFRESH_INTERVAL);
        release
    }

    #[cfg(test)]
    pub(super) fn expire_for_test(&mut self) {
        self.last_confirmed = Some(Instant::now() - STALE_AFTER);
    }
}

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
            self.document_refresh.reset();
        } else if let Some(reply) = self.document_refresh.poll(ui.ctx()) {
            self.document_cache = Some((game_id, path, reply.documents));
            self.document_issue = reply.issue;
        }
        self.document_refresh.request(
            ui.ctx(),
            game,
            &self.document_preferences,
            self.artwork.index.clone(),
        );
        let unverified = self.document_refresh.stale();
        let mut documents = self
            .document_cache
            .as_ref()
            .map(|(_, _, docs)| docs.clone())
            .unwrap_or_default();
        if unverified {
            for document in &mut documents {
                document.document_id = None;
                document.readiness = None;
                document.refusal_reason = Some("Document freshness could not be confirmed yet. Opening is paused until the background check finishes.".into());
            }
        }
        egui::CollapsingHeader::new("Manuals & Guides")
            .id_salt(("v2_manuals", game_id))
            .default_open(true)
            .show(ui, |ui| {
                ui.label("Local documents and saved associations are shown here. EmuWiz never downloads or changes document files.");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Associate local document…").clicked()
                        && let Some(path) = rfd::FileDialog::new().set_title("Associate a manual or guide with this game").add_filter("Manuals and guides", &["pdf", "cbz", "cbr", "txt"]).pick_file()
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
                if unverified { ui.label("Document files are being checked in the background. Opening is paused until they can be verified."); }
                if documents.is_empty() {
                    ui.weak("No manual or guide is associated with this game. Choose a local document to add one.");
                    return;
                }
            for document in documents {
            ui.push_id(("document", &document.path), |ui| {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.strong(&document.title);
                    ui.label(if document.format == super::documents::GameDocumentFormat::Text {
                        format!("Text guide · {} KB", document.file_size.div_ceil(1024))
                    } else {
                        format!("{} · {}", document.format.label(), document.page_count.map_or("page count unknown".into(), |count| format!("{count} pages")))
                    });
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
                ui.weak(if document.format == super::documents::GameDocumentFormat::Text {
                    if document.document_id.is_some() { "Plain text · Read inside EmuWiz." } else { "Plain text · EmuWiz cannot show this file." }
                } else {
                    super::manual_viewer::capability_text(document.format, document.readiness)
                });
                if let Some(reason) = &document.refusal_reason { ui.label(reason); }
                ui.horizontal_wrapped(|ui| {
                    if document.format == super::documents::GameDocumentFormat::Text {
                        self.text_guide_actions(ui, game_id, &document);
                    } else {
                        self.manual_document_actions(ui, game_id, &document);
                    }
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
