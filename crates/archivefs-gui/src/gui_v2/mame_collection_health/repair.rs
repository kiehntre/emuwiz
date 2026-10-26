//! Approval orchestration only. All mutations use the existing journal engine.
use super::worker::{self, Report, Request};
use archivefs_core::{
    dat::{
        archive::hash::hash_member_stream,
        model::ParsedDat,
        rename_apply::{ObjectIdentity, TransactionState},
    },
    mame_collection_analyser::inspection::SourceStamp,
    mame_internal_repair::{
        MameInternalRepairPlan, MameInternalRepairRequest, build_mame_internal_repair_plan,
    },
    mame_internal_repair_apply::{
        MameInternalRepairApplyOptions, MameInternalRepairApplyPlan, MameRepairOperationState,
        apply_mame_internal_repair_plan, plan_mame_internal_repair_apply,
    },
    safe_read::{TrustedRoots, open_bounded_read},
};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::AtomicBool,
    mpsc::{self, Receiver, TryRecvError},
};

/// Private, non-deserializable authority. Projections and clipboard reports
/// cannot become approvals. The original transaction identities are retained.
#[derive(Clone, Debug)]
pub(super) struct Reviewed {
    request: Request,
    catalogue_digest: String,
    sources: Vec<SourceStamp>,
    plan: MameInternalRepairPlan,
    apply: MameInternalRepairApplyPlan,
    digest: String,
}

pub(super) fn prepare(
    request: Request,
    report: &Report,
    dat: &ParsedDat,
    cancel: &AtomicBool,
) -> Result<Reviewed, String> {
    if !cfg!(target_os = "linux") {
        return Err("Internal repair Apply requires Linux's verified no-clobber copy path.".into());
    }
    if !report.inspection.inventory.inspected_completely || report.inspection.cancelled {
        return Err("Repair needs a complete, unchanged inspection.".into());
    }
    let names: std::collections::BTreeSet<_> = report
        .inspection
        .inventory
        .sets
        .iter()
        .map(|s| &s.name)
        .collect();
    if names.len() != report.inspection.inventory.sets.len() {
        return Err("Repair refused: multiple collection containers declare the same set.".into());
    }
    if report
        .context
        .values()
        .any(|c| c.duplicate_machine_declarations)
    {
        return Err("Repair refused: duplicate machine declarations are ambiguous.".into());
    }
    if report.analysis.version_compatibility == archivefs_core::mame_collection_analyser::MameVersionCompatibility::CatalogueVersionMismatch {
        return Err("Repair refused: the selected catalogue version differs from supplied MAME version evidence.".into());
    }
    let plan = build_mame_internal_repair_plan(&MameInternalRepairRequest {
        catalogue: dat,
        inventory: &report.inspection.inventory,
        analysis: &report.analysis,
    })?;
    // Fail closed for ambiguous dependency/owner evidence, including multiple
    // exact sources. No automatic choice is made behind the preview.
    if plan.requirements.iter().any(|r| r.ambiguity.is_some()) {
        return Err("Repair refused: dependency or source evidence is ambiguous.".into());
    }
    if report.analysis.set_results.iter().any(|s| {
        plan.affected_sets.contains(&s.name) && !s.evidence.unresolved_dependencies.is_empty()
    }) {
        return Err(
            "Repair refused: affected sets have unresolved dependency declarations.".into(),
        );
    }
    let mut apply = plan_mame_internal_repair_apply(&plan, &Default::default());
    let planned = apply
        .operations
        .iter()
        .filter(|o| o.state == MameRepairOperationState::Planned)
        .count();
    if planned != apply.transaction.as_ref().map_or(0, |tx| tx.entries.len()) {
        return Err("Repair refused: incomplete transaction source evidence.".into());
    }
    if let Some(tx) = &apply.transaction {
        let destinations: std::collections::BTreeSet<_> =
            tx.entries.iter().map(|e| &e.destination_path).collect();
        if destinations.len() != tx.entries.len() {
            return Err("Repair refused: overlapping destinations.".into());
        }
        for entry in &tx.entries {
            let operation = apply
                .operations
                .iter()
                .find(|o| o.destination_path == entry.destination_path)
                .ok_or("Missing operation evidence")?;
            prove_source(
                &entry.source_path,
                &entry.identity,
                &operation.expected_sha1,
                cancel,
            )?;
        }
    }
    if !report.catalogue.source.still_matches() || !report.inspection.sources_unchanged(cancel) {
        return Err("Sources changed while preparing repair. Inspect again.".into());
    }
    let bytes = serde_json::to_vec(&(
        &report.catalogue.reference,
        &report.catalogue.sha256,
        &plan,
        &apply.operations,
    ))
    .map_err(|e| e.to_string())?;
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if let Some(tx) = &mut apply.transaction {
        tx.unknown.insert(
            "mame_catalogue_sha256".into(),
            report.catalogue.sha256.clone().into(),
        );
        tx.unknown.insert(
            "mame_catalogue_reference".into(),
            report.catalogue.reference.clone().into(),
        );
        tx.unknown.insert(
            "mame_reviewed_snapshot_digest".into(),
            digest.clone().into(),
        );
    }
    Ok(Reviewed {
        request,
        catalogue_digest: report.catalogue.sha256.clone(),
        sources: report.inspection.sources.clone(),
        plan,
        apply,
        digest,
    })
}

fn prove_source(
    path: &std::path::Path,
    identity: &ObjectIdentity,
    sha1: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let stamp = SourceStamp::observe(path)?;
    if stamp.directory {
        return Err("Source is not a regular file".into());
    }
    let file = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.detail())?;
    let hashes = hash_member_stream(file.into_file(), stamp.bytes, cancel)
        .map_err(|e| format!("Source hashing failed: {e:?}"))?;
    let expected_sha256 = identity.freshness.as_ref().map(|f| {
        f.sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    });
    if hashes.hashes.sha1 != sha1
        || Some(hashes.hashes.sha256) != expected_sha256
        || hashes.bytes_read != identity.size_bytes
        || !stamp.still_matches()
    {
        return Err("Source content changed while binding the repair transaction.".into());
    }
    Ok(())
}

fn execute(
    reviewed: &Reviewed,
    options: &MameInternalRepairApplyOptions,
    progress: &mut dyn FnMut(&str),
) -> Result<String, String> {
    progress("Revalidating selected catalogue and every collection source; no files changed yet");
    let cancel = AtomicBool::new(false);
    let fresh = worker::run(reviewed.request.clone(), &cancel, &mut |_| {})?;
    let candidate = fresh.repair.as_ref().ok_or_else(|| {
        fresh
            .repair_unavailable
            .clone()
            .unwrap_or("Repair evidence unavailable".into())
    })?;
    if candidate.digest != reviewed.digest
        || candidate.catalogue_digest != reviewed.catalogue_digest
        || candidate.plan != reviewed.plan
        || candidate.sources != reviewed.sources
        || candidate
            .apply
            .transaction
            .as_ref()
            .map(|t| t.entries.iter().map(|e| &e.identity).collect::<Vec<_>>())
            != reviewed
                .apply
                .transaction
                .as_ref()
                .map(|t| t.entries.iter().map(|e| &e.identity).collect::<Vec<_>>())
    {
        return Err("Reviewed catalogue or collection snapshot is stale. Inspect and approve a new preview.".into());
    }
    let mut apply = reviewed.apply.clone();
    progress(
        "Applying verified copies through the shared transaction journal; completion is not yet established",
    );
    let result = apply_mame_internal_repair_plan(&mut apply, options, &cancel)?;
    let tx = result.outcome.transaction;
    let failures = tx
        .entries
        .iter()
        .filter_map(|e| e.failure_reason.as_deref())
        .collect::<Vec<_>>()
        .join("; ");
    Ok(if tx.state == TransactionState::Applied {
        format!(
            "Applied transaction {}: {} copied file(s), destination hashes verified. MAME verification and runnability were not assessed. Reinspect collection for current health. Open History & Undo for the journal and Undo.",
            tx.transaction_id,
            tx.entries.len()
        )
    } else {
        format!(
            "Transaction {} ended {:?}. Successful Apply is not established. Open History & Undo and review recovery before retrying. {failures}",
            tx.transaction_id, tx.state
        )
    })
}

enum Message {
    Stage(String),
    Finished(Result<String, String>),
}
#[derive(Default)]
pub(super) struct State {
    receiver: Option<Receiver<Message>>,
    #[cfg(test)]
    journal_dir: Option<std::path::PathBuf>,
    pub(super) confirmation: String,
    confirmation_digest: String,
    pub(super) notice: Option<String>,
    pub(super) open_history: bool,
}
impl State {
    pub(super) fn busy(&self) -> bool {
        self.receiver.is_some()
    }
    pub(super) fn poll(&mut self) -> bool {
        let Some(rx) = &self.receiver else {
            return false;
        };
        loop {
            match rx.try_recv() {
                Ok(Message::Stage(s)) => self.notice = Some(s),
                Ok(Message::Finished(result)) => {
                    self.notice = Some(result.unwrap_or_else(|e| format!("Apply stopped or refused: {e}. Review History & Undo for any journal and recovery details before retrying.")));
                    self.receiver = None;
                    return true;
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => {
                    self.notice = Some("Repair worker stopped unexpectedly. Outcome is unknown. Review History & Undo and recovery; do not assume no files changed.".into());
                    self.receiver = None;
                    return true;
                }
            }
        }
    }
    fn start(&mut self, reviewed: Arc<Reviewed>) {
        if self.busy() {
            return;
        }
        self.confirmation.clear();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.notice = Some("Starting repair evidence revalidation…".into());
        let options = MameInternalRepairApplyOptions::default();
        #[cfg(test)]
        let options = MameInternalRepairApplyOptions {
            journal_dir: self.journal_dir.clone(),
            ..options
        };
        std::thread::spawn(move || {
            let result = execute(&reviewed, &options, &mut |s| {
                let _ = sender.send(Message::Stage(s.into()));
            });
            let _ = sender.send(Message::Finished(result));
        });
    }
    pub(super) fn show(&mut self, ui: &mut eframe::egui::Ui, report: Option<&Report>) {
        use eframe::egui;
        ui.separator();
        ui.strong("Internal repair from exact local bytes");
        if let Some(notice) = &self.notice {
            ui.label(notice);
        }
        if self.busy() {
            ui.spinner();
            return;
        }
        if ui.button("Open History & Undo / recovery").clicked() {
            self.open_history = true;
        }
        let Some(report) = report else { return };
        let Some(reviewed) = &report.repair else {
            ui.label(
                report
                    .repair_unavailable
                    .as_deref()
                    .unwrap_or("No repair evidence available."),
            );
            return;
        };
        if self.confirmation_digest != reviewed.digest {
            self.confirmation.clear();
            self.confirmation_digest = reviewed.digest.clone();
        }
        ui.label(format!(
            "Registered catalogue {:?} · version {:?} · SHA-256 {}",
            report.catalogue.reference,
            reviewed
                .plan
                .catalogue_version
                .as_deref()
                .unwrap_or("unknown"),
            reviewed.catalogue_digest
        ));
        ui.label(format!("Reviewed plan {}", reviewed.digest));
        ui.label("Only independent copies to missing members of existing machine folders are supported. Existing files are never replaced. CHDs and archive writes are unsupported. Close other collection writers before Apply.");
        egui::CollapsingHeader::new("Exact affected files and expected results").default_open(true).show(ui, |ui| {
            for o in &reviewed.apply.operations {
                ui.label(format!("{:?} · {:?} · {:?}\n{:?} to {:?}\nExpected SHA-1 {} · size {:?}\n{}", o.affected_set, o.relationship, o.state, o.selected_source, o.destination_path, o.expected_sha1, o.expected_size, o.refusal_reason.as_deref().unwrap_or("Copy missing member; source retained; destination content verified")));
            }
            for r in &reviewed.plan.requirements {
                ui.label(format!("{:?} / {:?} · {} · {} · dependency {:?}, parent {:?}, merge {:?}, devices {:?}", r.affected_set, r.required_filename, r.requirement_class.label(), r.disposition.label(), r.relationship.kind, r.relationship.parent, r.relationship.merge_member, r.relationship.device_refs));
            }
        });
        let count = reviewed
            .apply
            .operations
            .iter()
            .filter(|o| o.state == MameRepairOperationState::Planned)
            .count();
        ui.label(format!("{count} supported copy operation(s). Unresolved requirements remain unresolved; successful placement does not establish a complete or runnable set."));
        if count == 0 || reviewed.apply.transaction.is_none() {
            ui.label("No safely supported Apply operations.");
            return;
        }
        ui.label("Type APPLY to approve this exact preview");
        ui.text_edit_singleline(&mut self.confirmation);
        if ui
            .add_enabled(
                self.confirmation == "APPLY",
                egui::Button::new("Apply internal repairs"),
            )
            .clicked()
        {
            self.start(reviewed.clone());
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
