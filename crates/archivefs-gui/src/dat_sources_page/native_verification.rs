//! Native Preview → Confirm orchestration. Workers reuse the core audit engine;
//! the UI owns no matching rules and never walks a ROM folder.
use super::*;
use crate::dat_catalogue_picker::CatalogueInventorySnapshot;
use archivefs_core::dat::catalogue_selection::resolve_catalogue;
use archivefs_core::dat::library_verification::{
    VerificationPreview, VerificationTotals, preview_library_verification,
};
use std::sync::mpsc::{self, Receiver};

/// Route verification into the bounded journey; preserve discovery and Arcade set audits.
pub(super) fn source_card_selection(
    action: &DatSourcesPageAction,
    registry: &DatSourceRegistry,
    catalogues: &[archivefs_core::dat::catalogue_selection::InstalledCatalogueSummary],
    identify_rename: bool,
) -> Option<(CatalogueRef, Option<String>, PathBuf)> {
    if identify_rename {
        return None;
    }
    let (reference, collection) = match action {
        DatSourcesPageAction::Audit { id, scan_root } => {
            (CatalogueRef::local(id), scan_root.clone())
        }
        DatSourcesPageAction::VerifyCatalogue {
            reference,
            scan_root,
        } => (reference.clone(), scan_root.clone()),
        _ => return None,
    };
    let platform = match &reference {
        CatalogueRef::Local { source_id, .. } => registry
            .get(source_id)
            .and_then(|entry| entry.platform.as_deref()),
        _ => catalogues
            .iter()
            .find(|c| c.reference == reference)
            .and_then(|c| c.platform.confirmed().map(String::as_str)),
    }
    .and_then(archivefs_core::canonical_platform_for_alias)
    .map(str::to_string);
    if platform.as_deref() == Some("Arcade") {
        None
    } else {
        Some((reference, platform, collection))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    pub platform: String,
    pub catalogue: CatalogueRef,
    pub collection: Option<PathBuf>,
    pub database: Option<PathBuf>,
    pub source_binding: String,
}
enum Message {
    Preview(Result<Box<VerificationPreview>, String>),
    Progress(String),
    Finished(Result<VerificationTotals, String>),
}
#[derive(Default)]
pub(super) struct Controller {
    selection: Option<Selection>,
    preview: Option<Box<VerificationPreview>>,
    receiver: Option<Receiver<Message>>,
    cancel: Option<Arc<AtomicBool>>,
    progress: String,
    error: Option<String>,
    totals: Option<VerificationTotals>,
    pub saved: bool,
    /// Successful confirmed verifications, so a host (Blue) can reload the
    /// evidence it shows without waiting for a restart.
    completions: u64,
    /// The local catalogue and system of the verification that just
    /// succeeded, while that catalogue has no assigned system: the evidence
    /// the user may explicitly turn into a saved assignment.
    executing: Option<(String, String)>,
    assignable: Option<(String, String)>,
}
impl Controller {
    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }
    pub fn completions(&self) -> u64 {
        self.completions
    }
    /// `(local source id, canonical platform)` a just-confirmed, parsed-
    /// evidence-compatible verification may assign, until it is answered.
    pub fn assignable(&self) -> Option<&(String, String)> {
        self.assignable.as_ref()
    }
    pub fn clear_assignable(&mut self) {
        self.assignable = None;
    }
    #[cfg(test)]
    pub(crate) fn record_completion_for_test(&mut self) {
        self.completions += 1;
    }
    pub fn select(&mut self, selection: Selection) {
        if self.selection.as_ref() != Some(&selection) && !self.busy() {
            self.selection = Some(selection);
            self.preview = None;
            self.totals = None;
            self.error = None;
            self.assignable = None;
        }
    }
    pub fn poll(&mut self) {
        let mut finished = false;
        if let Some(receiver) = &self.receiver {
            loop {
                let message = match receiver.try_recv() {
                    Ok(message) => message,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if !finished {
                            self.error = Some("Verification worker stopped unexpectedly. Completion is unconfirmed; reload library evidence before retrying.".into());
                        }
                        finished = true;
                        break;
                    }
                };
                match message {
                    Message::Preview(result) => {
                        match result {
                            Ok(preview) => self.preview = Some(preview),
                            Err(e) => self.error = Some(e),
                        }
                        finished = true;
                    }
                    Message::Progress(text) => self.progress = text,
                    Message::Finished(result) => {
                        match result {
                            Ok(totals) => {
                                self.saved = true;
                                self.completions += 1;
                                self.assignable = self.executing.take();
                                self.totals = Some(totals);
                            }
                            Err(e) => {
                                self.executing = None;
                                self.error = Some(e);
                            }
                        }
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.receiver = None;
            self.cancel = None;
        }
    }
    fn start_preview(&mut self, inventory: CatalogueInventorySnapshot, database: PathBuf) {
        let Some(selection) = self.selection.clone() else {
            return;
        };
        self.preview = None;
        self.error = None;
        self.totals = None;
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.progress = "Preparing exact library population and catalogue evidence…".into();
        std::thread::spawn(move || {
            let result = resolve_catalogue(&selection.catalogue, inventory.inputs())
                .map_err(|e| e.to_string())
                .and_then(|resolved| {
                    preview_library_verification(
                        resolved,
                        database,
                        selection.platform,
                        selection.collection,
                    )
                })
                .map(Box::new);
            let _ = sender.send(Message::Preview(result));
        });
    }
    fn confirm(&mut self, trusted: archivefs_core::safe_read::TrustedRoots) {
        let Some(preview) = self.preview.take() else {
            return;
        };
        // Only a local catalogue the core preview proved compatible (parsed
        // platform equals the selected system, no disagreeing assignment)
        // can be offered for an explicit assignment afterwards.
        self.executing = self
            .selection
            .as_ref()
            .and_then(|selection| match &selection.catalogue {
                CatalogueRef::Local { source_id, .. } if preview.platform == selection.platform => {
                    Some((source_id.clone(), preview.platform.clone()))
                }
                _ => None,
            });
        self.assignable = None;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.progress = "Starting confirmed verification…".into();
        std::thread::spawn(move || {
            let result = preview.execute(&trusted, &cancel, &|p| {
                let text = match p {
                    archivefs_core::dat::sources::audit_run::DatAuditProgress::Hashing { index,total,file_name } => format!("Hashing {index}/{total}: {file_name}"),
                    archivefs_core::dat::sources::audit_run::DatAuditProgress::ReadingCatalogue { .. } => "Reading confirmed catalogue…".into(),
                    archivefs_core::dat::sources::audit_run::DatAuditProgress::CatalogueReady { .. } => "Catalogue ready; preparing confirmed files…".into(),
                    archivefs_core::dat::sources::audit_run::DatAuditProgress::Comparing { files } => format!("Comparing {files} files and inspecting supported archives…"),
                    _ => "Preparing confirmed library files…".into(),
                };
                let _ = sender.send(Message::Progress(text));
            });
            let _ = sender.send(Message::Finished(result));
        });
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        inventory: Option<CatalogueInventorySnapshot>,
        database: Option<PathBuf>,
        trusted: archivefs_core::safe_read::TrustedRoots,
        enabled: bool,
    ) {
        self.poll();
        if self.busy() {
            ui.label(&self.progress);
            if let Some(cancel) = &self.cancel {
                if ui.button("Cancel verification").clicked() {
                    cancel.store(true, Ordering::Relaxed);
                }
                ui.small("Cancellation stops hashing before saving. A save already committing finishes atomically.");
            } else {
                ui.small("Preparing preview on a worker. No ROM content is read by preview.");
            }
            ui.ctx().request_repaint();
            return;
        }
        if let Some(error) = &self.error {
            ui.colored_label(theme::WARNING, error);
        }
        if let Some(t) = &self.totals {
            ui.strong(if t.errors > 0 || t.not_checked > 0 {
                "Verification finished with unchecked content"
            } else {
                "Verification complete"
            });
            ui.label(format!("Verified: {} · No catalogue match: {} · Ambiguous: {} · Not checked: {} · Errors: {}",t.verified,t.no_match,t.ambiguous,t.not_checked,t.errors));
            ui.label(&t.catalogue);
            ui.small(format!("Catalogue SHA-256: {}", t.catalogue_sha256));
            if !t.unchecked.is_empty() {
                ui.collapsing("Not checked files and reasons", |ui| {
                    let height = ui.text_style_height(&egui::TextStyle::Body);
                    egui::ScrollArea::vertical()
                        .id_salt("verification-unchecked")
                        .max_height(180.0)
                        .show_rows(ui, height, t.unchecked.len(), |ui, range| {
                            for i in range {
                                let item = &t.unchecked[i];
                                ui.label(format!("{} — {}", item.path.display(), item.reason));
                            }
                        });
                });
            }
            ui.label(format!("Saved {} evidence rows; skipped {} updates protected by existing prior-evidence rules.",t.saved,t.protected_prior));
            match &t.disc_order {
                Some(Ok(order)) => {
                    ui.label(crate::dat_sources_page::save_result::disc_order_summary(
                        order,
                    ));
                    if let Some(step) =
                        crate::dat_sources_page::save_result::disc_order_prerequisite(order)
                    {
                        ui.colored_label(ui.visuals().warn_fg_color, step);
                    }
                }
                Some(Err(error)) => {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("Disc order could not be refreshed: {error}"),
                    );
                }
                None => {}
            }
            ui.label("Use View games, then Reload game list to refresh Browse & Play and Game Details. Game files and platform assignments were unchanged.");
        }
        let mut confirm = false;
        if let Some(p) = &self.preview {
            ui.strong(format!(
                "Preview: {} · {} eligible games · {} bytes",
                p.platform, p.eligible_games, p.bytes
            ));
            // Primary action precedes technical details and exclusion rows so it stays reachable at 1024×600.
            confirm = crate::simple_mode::primary_button(
                ui,
                "Confirm verification",
                enabled && p.eligible_games > 0,
            )
            .clicked();
            ui.label(format!("{} selected · {} excluded (Not checked). ZIP/7z member sizes add decoded hashing work.",p.selected_games,p.exclusions.len()));
            ui.label(format!(
                "Catalogue: {} · version {}",
                p.catalogue_name,
                p.version.as_deref().unwrap_or("not stated")
            ));
            ui.label(&p.source);
            ui.small(&p.registration);
            ui.small(format!("SHA-256: {}", p.sha256));
            ui.label(format!("Compatible platform: {} (parsed DAT evidence). Exact serialized bytes / decoded archive members; no media conversion.",p.platform));
            ui.label(format!(
                "Represented formats: {}",
                p.formats
                    .iter()
                    .map(|f| format!(".{f}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if p.negative_formats != p.formats {
                ui.strong("Some catalogue entries lack comparable MD5/SHA-1 hashes. Unproven negatives for those formats remain Not checked; exact positives still verify.");
            }
            ui.label("Use a source you trust. Hash identity does not establish complete preservation sets or launch readiness.");
            ui.collapsing("Scope and exclusions",|ui| {
                ui.label(p.catalogue_path.display().to_string());
                ui.label("Eligible library files (each counted once):");
                let paths: Vec<_> = p.eligible_paths().collect();
                let height = ui.text_style_height(&egui::TextStyle::Body);
                egui::ScrollArea::vertical().id_salt("verification-eligible").max_height(140.0).show_rows(ui,height,paths.len(),|ui,range| {
                    for i in range { ui.label(paths[i].display().to_string()); }
                });
                ui.label(format!("Other systems excluded: {} · Unknown system excluded: {} · Outside collection: {}",p.wrong_system,p.unknown_system,p.outside_collection));
                egui::ScrollArea::vertical().max_height(140.0).show(ui,|ui| {
                    for e in &p.exclusions { ui.label(format!("{} — {}",e.path.display(),e.reason)); }
                });
                ui.label("Unreadable, corrupt, encrypted, nested or incomplete archive content remains Not checked. Runtime depends on storage and decompression; no measured ETA is available.");
            });
        }
        if confirm {
            self.confirm(trusted);
        } else if crate::simple_mode::primary_button(
            ui,
            "Preview verification",
            enabled && inventory.is_some() && database.is_some(),
        )
        .clicked()
        {
            if let (Some(inventory), Some(database)) = (inventory, database.clone()) {
                self.start_preview(inventory, database);
            }
        }
        if database.is_none() {
            ui.label("Open a library database to verify assigned games.");
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
