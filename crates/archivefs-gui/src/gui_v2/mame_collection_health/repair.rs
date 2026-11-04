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
        MameInternalRepairPlan, MameInternalRepairRequest, MameRepairDisposition,
        build_mame_internal_repair_plan,
    },
    mame_internal_repair_apply::{
        MameInternalRepairApplyOptions, MameInternalRepairApplyPlan, MameInternalRepairApplyResult,
        MameRepairOperationState, apply_mame_internal_repair_plan, plan_mame_internal_repair_apply,
        verify_mame_internal_repair_after_apply,
    },
    mame_post_repair::{
        MAME_VERIFY_TIMEOUT, MamePostRepairVerificationRequest, MameRepairVerification,
        MameRepairVerificationState, MameSetIdentityReceipt, MameTargetedVerificationState,
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
    /// Identity and dependency evidence captured while the repair was planned,
    /// one entry per set the transaction writes to.
    verification_basis: Vec<SetBasis>,
}

/// What the plan knew about one set before any file changed. The post-repair
/// check compares against this, never against evidence gathered afterwards.
#[derive(Clone, Debug)]
pub(super) struct SetBasis {
    set: String,
    identity: MameSetIdentityReceipt,
    missing_dependencies: Vec<String>,
    bad_dump_dependencies: Vec<String>,
    no_dump_present: bool,
}

/// At most this many sets are checked with MAME per Apply, so the background
/// work stays bounded (each check has its own timeout). The rest are reported
/// as not checked.
const MAX_MAME_CHECKS: usize = 16;

/// One set's typed MAME verification result.
#[derive(Clone, Debug)]
pub(super) struct SetVerification {
    pub(super) verification: MameRepairVerification,
}

struct Outcome {
    notice: String,
    verifications: Vec<SetVerification>,
}

fn dependency_basis(
    report: &Report,
    plan: &MameInternalRepairPlan,
    apply: &MameInternalRepairApplyPlan,
    digest: &str,
) -> Vec<SetBasis> {
    let captured = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut sets: Vec<&String> = apply
        .operations
        .iter()
        .filter(|o| o.state == MameRepairOperationState::Planned)
        .map(|o| &o.affected_set)
        .collect();
    sets.sort();
    sets.dedup();
    sets.into_iter()
        .map(|set| {
            let mut missing = Vec::new();
            let mut bad = Vec::new();
            let mut no_dump = false;
            let mut topology = Sha256::new();
            for r in plan.requirements.iter().filter(|r| &r.affected_set == set) {
                topology.update(serde_json::to_vec(&r.relationship).unwrap_or_default());
                let satisfied = apply.operations.iter().any(|o| {
                    &o.affected_set == set
                        && o.destination_member == r.expected_destination_member
                        && matches!(
                            o.state,
                            MameRepairOperationState::Planned
                                | MameRepairOperationState::AlreadySatisfied
                        )
                });
                match r.disposition {
                    MameRepairDisposition::NoDump => no_dump = true,
                    MameRepairDisposition::PresentButBadDump => {
                        bad.push(r.required_filename.clone())
                    }
                    _ if !satisfied => missing.push(r.required_filename.clone()),
                    _ => {}
                }
            }
            let topology: String = topology
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let context = report.context.get(set);
            SetBasis {
                set: set.clone(),
                identity: MameSetIdentityReceipt::new(
                    report.catalogue.sha256.clone(),
                    plan.catalogue_version.clone(),
                    set.clone(),
                    context.and_then(|c| c.clone_of.clone().or_else(|| c.rom_of.clone())),
                    plan.collection_root.clone(),
                    digest.to_string(),
                    topology,
                    captured,
                ),
                missing_dependencies: missing,
                bad_dump_dependencies: bad,
                no_dump_present: no_dump,
            }
        })
        .collect()
}

/// Plain wording for each typed state. A pass is a ROM-file verification only.
pub(super) fn describe(v: &MameRepairVerification) -> String {
    use MameRepairVerificationState as S;
    let detail = match v.state {
        S::VerifiedComplete => "MAME's ROM verification reports this set as good. That checks ROM files only; it does not show the game starts, plays, or is fully compatible.".to_string(),
        S::VerifiedBestAvailable => "MAME's ROM verification finished, but the set contains NO_DUMP material that no copy can complete. This is the best available state, not a full verification.".to_string(),
        S::StillIncomplete => "MAME's ROM verification still reports missing files for this set, so the repair did not complete it.".to_string(),
        S::NeedsRedump => "MAME's ROM verification reports a BAD_DUMP in this set. It needs a better dump; the copied files do not fix that.".to_string(),
        S::VerificationFailed => {
            let how = match v.targeted_mame.as_ref().map(|t| t.state) {
                Some(MameTargetedVerificationState::Unavailable) => "MAME could not be started, so its ROM verification did not run",
                Some(MameTargetedVerificationState::Timeout) => "MAME did not finish in time and was stopped",
                Some(MameTargetedVerificationState::NonZeroExit) => "MAME exited with an error",
                Some(MameTargetedVerificationState::UnexpectedOutput) => "MAME's output was not recognised",
                _ => "MAME's ROM verification did not pass",
            };
            format!("{how}. The set is not shown to verify.")
        }
        S::VerificationUnavailable => "MAME's ROM verification was not run: no MAME executable is configured or it could not be started. Set one under Emulator Setup to check this.".to_string(),
        S::StaleEvidence => "MAME's ROM verification was not run because the evidence changed since this repair was reviewed (catalogue, destination files, or transaction). Inspect the collection again.".to_string(),
    };
    format!("{} · {:?}: {detail}", v.identity.machine_shortname, v.state)
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
    let verification_basis = dependency_basis(report, &plan, &apply, &digest);
    Ok(Reviewed {
        request,
        catalogue_digest: report.catalogue.sha256.clone(),
        sources: report.inspection.sources.clone(),
        plan,
        apply,
        digest,
        verification_basis,
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

/// MAME settings for the post-repair check. `executable` is only ever a path the
/// user configured; nothing is discovered or downloaded.
struct MameCheck {
    executable: Option<std::path::PathBuf>,
    timeout: std::time::Duration,
}

#[cfg(test)]
fn execute(
    reviewed: &Reviewed,
    options: &MameInternalRepairApplyOptions,
    progress: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let check = MameCheck {
        executable: None,
        timeout: MAME_VERIFY_TIMEOUT,
    };
    execute_checked(reviewed, options, &check, progress).map(|o| o.notice)
}

/// One set's check: Core's post-Apply verifier, fed the identity captured at
/// planning time and the evidence just re-read. Only this set's own
/// destinations are compared and hashed.
fn verify_set(
    basis: &SetBasis,
    apply: &MameInternalRepairApplyPlan,
    result: &MameInternalRepairApplyResult,
    current: (&str, &MameInternalRepairPlan),
    check: &MameCheck,
) -> MameRepairVerification {
    let mut for_set = result.clone();
    for_set
        .operations
        .retain(|operation| operation.affected_set == basis.set);
    let request = MamePostRepairVerificationRequest {
        identity: basis.identity.clone(),
        transaction_id: String::new(),
        repair_plan_digest: String::new(),
        expected_destinations: Vec::new(),
        current_catalogue_sha256: current.0.to_string(),
        current_catalogue_version: current.1.catalogue_version.clone(),
        current_machine_shortname: basis.set.clone(),
        current_arcade_root: current.1.collection_root.clone(),
        executable: check.executable.clone(),
        timeout: check.timeout,
        missing_dependencies: basis.missing_dependencies.clone(),
        bad_dump_dependencies: basis.bad_dump_dependencies.clone(),
        no_dump_present: basis.no_dump_present,
    };
    verify_mame_internal_repair_after_apply(apply, &for_set, request)
}

/// The configured MAME is used only if it is an absolute path to a regular,
/// executable file that is not a symlink (the same rule as the MAME launch
/// preflight). Anything else is treated as "no MAME"; nothing is searched for.
fn validated_executable(path: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = path?;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    (path.is_absolute()
        && !metadata.file_type().is_symlink()
        && metadata.is_file()
        && metadata.permissions().mode() & 0o111 != 0)
        .then(|| path.to_path_buf())
}

fn execute_checked(
    reviewed: &Reviewed,
    options: &MameInternalRepairApplyOptions,
    check: &MameCheck,
    progress: &mut dyn FnMut(&str),
) -> Result<Outcome, String> {
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
    let tx = result.outcome.transaction.clone();
    let failures = tx
        .entries
        .iter()
        .filter_map(|e| e.failure_reason.as_deref())
        .collect::<Vec<_>>()
        .join("; ");
    if tx.state != TransactionState::Applied {
        // No MAME process is started for a transaction that did not complete.
        return Ok(Outcome {
            notice: format!(
                "Transaction {} ended {:?}. Successful Apply is not established. No MAME check was run. Open History & Undo and review recovery before retrying. {failures}",
                tx.transaction_id, tx.state
            ),
            verifications: Vec::new(),
        });
    }
    progress(
        "Applied. Copied files are hash-checked; now asking MAME to verify the repaired sets (ROM verification only)",
    );
    let check = &MameCheck {
        executable: validated_executable(check.executable.as_deref()),
        timeout: check.timeout,
    };
    let mut verifications = Vec::new();
    let mut unchecked_sets = Vec::new();
    for (index, basis) in reviewed.verification_basis.iter().enumerate() {
        if index >= MAX_MAME_CHECKS {
            unchecked_sets.push(basis.set.clone());
            continue;
        }
        verifications.push(SetVerification {
            verification: verify_set(
                basis,
                &apply,
                &result,
                (&candidate.catalogue_digest, &candidate.plan),
                check,
            ),
        });
    }
    let passed = verifications
        .iter()
        .filter(|v| v.verification.state == MameRepairVerificationState::VerifiedComplete)
        .count();
    let mut notice = format!(
        "Applied transaction {}: {} copied file(s), destination file hashes verified. That proves the copies, not MAME's view of the sets. MAME ROM verification: {passed} of {} set(s) verified complete; see each result below. A MAME pass checks ROM files only, not that a game plays. Reinspect collection for current health. Open History & Undo for the journal and Undo.",
        tx.transaction_id,
        tx.entries.len(),
        reviewed.verification_basis.len()
    );
    if !unchecked_sets.is_empty() {
        notice.push_str(&format!(
            " {} further set(s) were not checked with MAME (limit {MAX_MAME_CHECKS} per Apply).",
            unchecked_sets.len()
        ));
    }
    Ok(Outcome {
        notice,
        verifications,
    })
}

enum Message {
    Stage(String),
    Finished(Result<Outcome, String>),
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
    /// Typed MAME results of the last Apply, shown beneath the notice.
    pub(super) verifications: Vec<SetVerification>,
    #[cfg(test)]
    mame_check: Option<(Option<std::path::PathBuf>, std::time::Duration)>,
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
                    match result {
                        Ok(outcome) => {
                            self.notice = Some(outcome.notice);
                            self.verifications = outcome.verifications;
                        }
                        Err(e) => {
                            self.verifications.clear();
                            self.notice = Some(format!(
                                "Apply stopped or refused: {e}. Review History & Undo for any journal and recovery details before retrying."
                            ));
                        }
                    }
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
        self.verifications.clear();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        self.notice = Some("Starting repair evidence revalidation…".into());
        let options = MameInternalRepairApplyOptions::default();
        #[cfg(test)]
        let options = MameInternalRepairApplyOptions {
            journal_dir: self.journal_dir.clone(),
            ..options
        };
        let check = MameCheck {
            executable: crate::emulator_setup_overrides::EmulatorPathOverrides::load()
                .mame_executable,
            timeout: MAME_VERIFY_TIMEOUT,
        };
        #[cfg(test)]
        let check = match self.mame_check.clone() {
            Some((executable, timeout)) => MameCheck {
                executable,
                timeout,
            },
            None => MameCheck {
                executable: None,
                ..check
            },
        };
        std::thread::spawn(move || {
            let result = execute_checked(&reviewed, &options, &check, &mut |s| {
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
        for item in &self.verifications {
            ui.label(describe(&item.verification));
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
