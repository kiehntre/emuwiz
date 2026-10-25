//! Read-only presentation of plain-text guides. All file safety and decoding
//! lives in `archivefs_core::manual_document::text_guide`; this only shows
//! the already-sanitised lines.
use super::documents::{
    DocumentOpenCapability, GameDocument, GameDocumentAssociation, GameDocumentFormat,
};
use archivefs_core::manual_document::{
    ManualDocumentId, ManualViewerError,
    text_guide::{TextGuide, read_text_guide},
};
use eframe::egui;
use std::{path::PathBuf, sync::mpsc};

pub(super) fn can_open(document: &GameDocument) -> bool {
    document.format == GameDocumentFormat::Text
        && document.document_id.is_some()
        && !matches!(
            document.association,
            GameDocumentAssociation::WeakFilename | GameDocumentAssociation::Unmatched
        )
}

struct Session {
    path: PathBuf,
    title: String,
    expected: ManualDocumentId,
    external: DocumentOpenCapability,
    pending: Option<mpsc::Receiver<Result<TextGuide, ManualViewerError>>>,
    guide: Option<TextGuide>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct TextGuideViewer {
    session: Option<Session>,
}

impl TextGuideViewer {
    pub(super) fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// Reads off the UI thread: guides may sit on slow or network storage.
    pub(super) fn open(&mut self, context: &egui::Context, document: &GameDocument) {
        let Some(expected) = document.document_id.clone().filter(|_| can_open(document)) else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let path = document.path.clone();
        let id = expected.clone();
        let repaint = context.clone();
        let started = std::thread::Builder::new()
            .name("emuwiz-text-guide".into())
            .spawn(move || {
                let _ = sender.send(read_text_guide(&path, Some(&id)));
                repaint.request_repaint();
            });
        self.session = Some(Session {
            path: document.path.clone(),
            title: document.title.clone(),
            expected,
            external: document.viewer,
            pending: started.is_ok().then_some(receiver),
            guide: None,
            error: started
                .err()
                .map(|_| "EmuWiz could not start reading this guide.".into()),
        });
    }

    fn poll(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(receiver) = &session.pending else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(guide)) => {
                session.guide = Some(guide);
                session.pending = None;
            }
            Ok(Err(error)) => {
                session.error = Some(error.user_message().into());
                session.pending = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                session.error = Some("EmuWiz could not read this guide.".into());
                session.pending = None;
            }
        }
    }

    /// Returns a request to open the same, still-unchanged file externally.
    pub(super) fn show(&mut self, context: &egui::Context) -> Option<(PathBuf, ManualDocumentId)> {
        self.poll();
        let mut close =
            context.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let mut external = None;
        let session = self.session.as_ref()?;
        egui::CentralPanel::default().show(context, |ui| {
            ui.heading(&session.title);
            ui.horizontal_wrapped(|ui| {
                ui.label(GameDocumentFormat::Text.label());
                if let Some(guide) = &session.guide {
                    ui.label(format!("{} lines", guide.lines.len()));
                }
                if ui
                    .add_enabled(
                        session.external == DocumentOpenCapability::Supported,
                        egui::Button::new("Open externally"),
                    )
                    .clicked()
                {
                    external = Some((session.path.clone(), session.expected.clone()));
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
            ui.weak(format!("Path: {}", session.path.display()));
            ui.weak(
                "Read-only. EmuWiz shows this text as plain characters and never changes the file.",
            );
            if let Some(error) = &session.error {
                ui.colored_label(crate::ui::theme::warning(ui), error);
                return;
            }
            let Some(guide) = &session.guide else {
                ui.spinner();
                return;
            };
            for note in notices(guide) {
                ui.weak(note);
            }
            ui.separator();
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show_rows(ui, row_height, guide.lines.len(), |ui, rows| {
                    for line in &guide.lines[rows] {
                        ui.add(egui::Label::new(egui::RichText::new(line).monospace()).extend());
                    }
                });
        });
        if close {
            self.session = None;
        }
        external
    }
}

fn notices(guide: &TextGuide) -> Vec<String> {
    let mut notes = Vec::new();
    if guide.replaced_invalid_bytes {
        notes.push("Some characters are not valid UTF-8 and are shown as �.".into());
    }
    if guide.removed_control_characters {
        notes.push("Control and text-direction characters were hidden.".into());
    }
    if guide.shortened_lines > 0 {
        notes.push(format!(
            "{} very long lines are shortened (…); open externally to see them in full.",
            guide.shortened_lines
        ));
    }
    notes
}

impl super::App {
    pub(super) fn text_guide_actions(
        &mut self,
        ui: &mut egui::Ui,
        game_id: i64,
        document: &GameDocument,
    ) {
        if ui
            .add_enabled(can_open(document), egui::Button::new("Open internally"))
            .clicked()
        {
            self.text_guide_viewer.open(ui.ctx(), document);
        }
        if ui
            .add_enabled(
                can_open(document) && document.viewer == DocumentOpenCapability::Supported,
                egui::Button::new("Open externally"),
            )
            .clicked()
            && let Some(expected) = document.document_id.clone()
        {
            let job =
                self.activity
                    .queue("Opening local document", super::Route::Game(game_id), false);
            self.send(
                job,
                super::Command::OpenDocument {
                    path: document.path.clone(),
                    expected,
                },
            );
        }
    }

    pub(super) fn show_text_guide_viewer(&mut self, context: &egui::Context) -> bool {
        if !self.text_guide_viewer.is_open() {
            return false;
        }
        if let Some((path, expected)) = self.text_guide_viewer.show(context) {
            let job =
                self.activity
                    .queue("Opening local document", self.router.current.clone(), false);
            self.send(job, super::Command::OpenDocument { path, expected });
        }
        if !self.text_guide_viewer.is_open() {
            self.document_cache = None;
        }
        // Even the closing frame belongs to the viewer: Escape must not also run Back.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_v2::documents::{DocumentDiscoveryRequest, discover_documents};
    use std::{collections::BTreeMap, fs};

    fn discover(dir: &std::path::Path, associations: &BTreeMap<PathBuf, i64>) -> Vec<GameDocument> {
        discover_documents(DocumentDiscoveryRequest {
            game_id: 7,
            game_title: "Chrono Trigger",
            platform: "SNES",
            game_path: &dir.join("Chrono Trigger.sfc"),
            roots: &[],
            associations,
            verified_identity: None,
        })
    }

    #[test]
    fn title_matched_text_guide_opens_read_only_and_loose_text_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Chrono Trigger.sfc"), b"rom").unwrap();
        fs::write(dir.path().join("readme.txt"), b"collection notes\n").unwrap();
        fs::create_dir(dir.path().join("guides")).unwrap();
        let guide = dir.path().join("guides/chrono_trigger.txt");
        fs::write(&guide, b"Walkthrough\r\nChapter 1\n").unwrap();
        let docs = discover(dir.path(), &BTreeMap::new());
        assert_eq!(docs.len(), 1, "{docs:?}");
        let document = &docs[0];
        assert_eq!(document.format, GameDocumentFormat::Text);
        assert_eq!(document.association, GameDocumentAssociation::ExactTitle);
        assert!(document.refusal_reason.is_none());
        assert!(can_open(document));

        let context = egui::Context::default();
        let mut viewer = TextGuideViewer::default();
        viewer.open(&context, document);
        let receiver = viewer.session.as_mut().unwrap().pending.take().unwrap();
        let read = receiver.recv().unwrap().unwrap();
        assert_eq!(read.lines, ["Walkthrough", "Chapter 1"]);
        assert_eq!(fs::read(&guide).unwrap(), b"Walkthrough\r\nChapter 1\n");
    }

    #[test]
    fn explicit_text_association_is_shown_and_binary_text_is_labelled_not_opened() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Chrono Trigger.sfc"), b"rom").unwrap();
        let faq = dir.path().join("FAQ by someone.txt");
        fs::write(&faq, b"Hints\n").unwrap();
        let mut preferences = crate::gui_v2::documents::DocumentPreferences::default();
        crate::gui_v2::documents::associate_local_document(&faq, 7, &mut preferences).unwrap();
        let docs = discover(dir.path(), &preferences.associations);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].association, GameDocumentAssociation::Explicit);
        assert!(can_open(&docs[0]));

        let fake = dir.path().join("Chrono Trigger.txt");
        fs::write(&fake, b"MZ\x00\x01binary").unwrap();
        assert!(
            crate::gui_v2::documents::associate_local_document(&fake, 7, &mut preferences).is_err()
        );
        let docs = discover(dir.path(), &BTreeMap::new());
        let refused = docs
            .iter()
            .find(|doc| doc.path.ends_with("Chrono Trigger.txt"))
            .unwrap();
        assert_eq!(refused.format, GameDocumentFormat::Text);
        assert!(refused.refusal_reason.is_some());
        assert_eq!(refused.viewer, DocumentOpenCapability::UnsupportedFormat);
        assert!(!can_open(refused));
    }
}
