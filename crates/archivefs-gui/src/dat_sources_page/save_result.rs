//! Session results for the existing DAT workers. These are observations of
//! completed persistence attempts, never authority to replay a write.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DatSaveOperation {
    Audit,
    Validation,
    CombinedAudit,
}

impl DatSaveOperation {
    fn label(self) -> &'static str {
        match self {
            Self::Audit => "Audit",
            Self::Validation => "Catalogue validation",
            Self::CombinedAudit => "Combined audit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatSaveOutcome {
    Success,
    SuccessWithWarnings,
    PersistenceFailure,
    PartialFailure,
    NotSaved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatSaveResult {
    pub(crate) operation: DatSaveOperation,
    pub(crate) source_id: String,
    pub(crate) source_name: String,
    /// Audit target's basename; full paths are diagnostic detail only.
    pub(crate) target: Option<String>,
    pub(crate) outcome: DatSaveOutcome,
    pub(crate) explanation: String,
    /// Original errors stay in page state for local diagnostics. Never sent
    /// to the general log or included in the ordinary status text.
    pub(crate) technical_details: Vec<String>,
    /// Known private locations are shortened for display, including when
    /// diagnostics are expanded; the original error remains available above.
    private_paths: Vec<String>,
}

impl DatSaveResult {
    fn for_audit(outcome: &DatAuditOutcome) -> Self {
        Self {
            operation: DatSaveOperation::Audit,
            source_id: outcome.source_id.clone(),
            source_name: outcome.source_display_name.clone(),
            target: PathBuf::from(&outcome.scan_root)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            outcome: DatSaveOutcome::Success,
            explanation: "DAT audit results saved to the catalogue.".to_string(),
            technical_details: vec![
                format!("Catalogue: {}", outcome.dat_path),
                format!("Checked: {}", outcome.scan_root),
            ],
            private_paths: vec![outcome.dat_path.clone(), outcome.scan_root.clone()],
        }
    }

    pub(super) fn validation(report: &DatValidationReport) -> Self {
        Self {
            operation: DatSaveOperation::Validation,
            source_id: report.source_id.clone(),
            source_name: PathBuf::from(&report.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "DAT source".to_string()),
            target: None,
            outcome: DatSaveOutcome::Success,
            explanation: "Expected DAT inventory saved to the catalogue.".to_string(),
            technical_details: vec![format!("Catalogue: {}", report.path)],
            private_paths: vec![report.path.clone()],
        }
    }

    pub(super) fn key(&self) -> (DatSaveOperation, String) {
        (self.operation, self.source_id.clone())
    }

    pub(super) fn private_database_path(&mut self, path: &std::path::Path) {
        self.private_paths.push(path.to_string_lossy().into_owned());
        if let Some(parent) = path
            .parent()
            .filter(|parent| parent.components().count() > 1)
        {
            self.private_paths
                .push(parent.to_string_lossy().into_owned());
        }
    }

    fn display_detail(&self, detail: &str) -> String {
        let mut text = detail.to_string();
        let mut paths: Vec<_> = self
            .private_paths
            .iter()
            .filter(|path| !path.is_empty())
            .collect();
        paths.sort_by_key(|path| std::cmp::Reverse(path.len()));
        for path in paths {
            text = text.replace(path.as_str(), &shorten_path(path));
        }
        text
    }

    pub(super) fn is_failure(&self) -> bool {
        matches!(
            self.outcome,
            DatSaveOutcome::PersistenceFailure | DatSaveOutcome::PartialFailure
        )
    }

    pub(super) fn failed(&mut self, stage: &str, error: archivefs_core::ArchiveFsError) {
        self.outcome = DatSaveOutcome::PersistenceFailure;
        self.explanation = format!(
            "DAT results could not be fully saved ({stage}). Earlier save steps may have completed. \
             Check the catalogue database is available and writable, then run this operation again."
        );
        self.technical_details.push(format!("{stage}: {error}"));
    }

    pub(super) fn warn(&mut self, warning: impl Into<String>) {
        if self.outcome == DatSaveOutcome::Success {
            self.outcome = DatSaveOutcome::SuccessWithWarnings;
            self.explanation =
                "DAT results saved with warnings. Some items need review; see the details below."
                    .to_string();
        }
        self.technical_details.push(warning.into());
    }
}

/// Uses the existing writers, in their existing order. Each failure names its
/// step; a later step failing must never turn earlier commits into overall
/// success. Completeness/refusal policy remains owned by core.
pub(super) fn persist_audit(
    database_path: Option<&std::path::Path>,
    outcome: &DatAuditOutcome,
    generation: u64,
    cancel: &AtomicBool,
) -> (
    DatSaveResult,
    Option<Box<archivefs_core::PlatformIdentityEnrichmentSummary>>,
) {
    let mut result = DatSaveResult::for_audit(outcome);
    result
        .technical_details
        .push(format!("Audit generation: {generation}"));
    if cancel.load(Ordering::Acquire) {
        result.outcome = DatSaveOutcome::NotSaved;
        result.explanation =
            "Audit cancelled before saving; no audit results were saved.".to_string();
        return (result, None);
    }
    let Some(database_path) = database_path else {
        result.outcome = if audit_incomplete(outcome) {
            DatSaveOutcome::PartialFailure
        } else {
            DatSaveOutcome::NotSaved
        };
        result.explanation = "Audit results are available for this session only. No catalogue database was available; run the audit again after opening a catalogue to save them.".to_string();
        if audit_incomplete(outcome) {
            result
                .explanation
                .push_str(" Some sources or inputs could not be fully audited.");
            result
                .technical_details
                .extend(outcome.unreadable_catalogues.iter().cloned());
        }
        return (result, None);
    };
    result.private_database_path(database_path);
    let mut database = match archivefs_core::Database::open_or_create(database_path) {
        Ok(database) => database,
        Err(error) => {
            result.failed("opening the catalogue database", error);
            return (result, None);
        }
    };
    let persisted = match database.persist_dat_audit_results(outcome) {
        Ok(persisted) => persisted,
        Err(error) => {
            result.failed("saving set verdicts", error);
            return (result, None);
        }
    };
    result
        .technical_details
        .push(format!("{persisted} set verdict(s) saved."));
    let identities = match database.persist_library_dat_identities_from_audit(outcome) {
        Ok(identities) => identities,
        Err(error) => {
            result.failed("saving library DAT identities", error);
            return (result, None);
        }
    };
    result.technical_details.push(format!(
        "{} library identity row(s) inserted, {} updated.",
        identities.inserted, identities.updated
    ));
    if identities.refused.is_some() || identities.ambiguous > 0 {
        result.outcome = DatSaveOutcome::PartialFailure;
        result.explanation = "Some DAT identities could not be saved or associated with library items. Review the details and run the audit again after resolving the source or library problem.".to_string();
        result.technical_details.push(format!(
            "Identity projection refusal: {:?}; {} item association(s) failed or were ambiguous.",
            identities.refused, identities.ambiguous
        ));
    }
    if audit_incomplete(outcome) {
        result.outcome = DatSaveOutcome::PartialFailure;
        result.explanation = "The audit was incomplete. Available safe results were saved, but this is not a complete saved audit. Resolve the unreadable or limited inputs and run it again.".to_string();
        result.technical_details.push(format!(
            "Scan truncated: {}; unreadable catalogues: {}; unhashed files: {}. Incomplete set verdicts and unsafe negative identities are withheld by core.",
            outcome.truncated, outcome.unreadable_catalogues.len(), outcome.unhashed.len()
        ));
        result
            .technical_details
            .extend(outcome.unreadable_catalogues.iter().cloned());
    }
    if identities.unassociated > 0
        || identities.skipped_protected_prior_result > 0
        || identities.negative_incomplete_skipped > 0
    {
        result.warn(format!(
            "{} item(s) outside the library; {} prior result(s) protected; {} unsafe negative identity result(s) skipped.",
            identities.unassociated, identities.skipped_protected_prior_result, identities.negative_incomplete_skipped
        ));
    }
    match database.enrich_platforms_from_dat_audit(outcome, generation) {
        Ok(enrichment) => {
            if enrichment.conflicts > 0 {
                result.warn(format!(
                    "{} platform conflict(s) require review.",
                    enrichment.conflicts
                ));
            }
            result.technical_details.push(format!(
                "Platform enrichment: {} applied, {} already current, {} manual assignment(s) preserved.",
                enrichment.applied, enrichment.unchanged, enrichment.manual_preserved
            ));
            (result, Some(Box::new(enrichment)))
        }
        Err(error) => {
            result.failed("saving platform enrichment", error);
            (result, None)
        }
    }
}

fn audit_incomplete(outcome: &DatAuditOutcome) -> bool {
    outcome.truncated
        || !outcome.unreadable_catalogues.is_empty()
        || !outcome.unhashed.is_empty()
        || outcome.archives.iter().any(|archive| {
            !matches!(
                archive.completion,
                archivefs_core::dat::archive::ArchivePassCompletion::Complete
            )
        })
}

pub(super) fn combined_result(
    outcome: &DatAuditOutcome,
    sources: &[CombinedDatAuditSource],
) -> DatSaveResult {
    let mut result = DatSaveResult::for_audit(outcome);
    result.operation = DatSaveOperation::CombinedAudit;
    result.private_paths.extend(
        sources
            .iter()
            .map(|source| source.dat_path.to_string_lossy().into_owned()),
    );
    result.outcome = if !audit_incomplete(outcome) {
        DatSaveOutcome::NotSaved
    } else {
        DatSaveOutcome::PartialFailure
    };
    result.explanation = "Combined audit results are available for this session only and are not saved to the catalogue. Run an individual source audit to save its results.".to_string();
    if result.outcome == DatSaveOutcome::PartialFailure {
        result
            .explanation
            .push_str(" Some sources or inputs could not be fully audited.");
    }
    result
        .technical_details
        .extend(sources.iter().map(|source| {
            format!(
                "Source: {} ({})",
                source.source_display_name, source.source_id
            )
        }));
    result
        .technical_details
        .extend(outcome.unreadable_catalogues.iter().cloned());
    result
}

pub(super) fn show(ui: &mut egui::Ui, results: &[DatSaveResult]) {
    if results.is_empty() {
        return;
    }
    let saved = results
        .iter()
        .filter(|result| result.outcome == DatSaveOutcome::Success)
        .count();
    let warnings = results
        .iter()
        .filter(|result| result.outcome == DatSaveOutcome::SuccessWithWarnings)
        .count();
    let failures = results.iter().filter(|result| result.is_failure()).count();
    let not_saved = results
        .iter()
        .filter(|result| result.outcome == DatSaveOutcome::NotSaved)
        .count();
    // Keep successful bulk validation compact; unresolved failures open the
    // section on a fresh page. Every source remains recoverable by expanding it.
    egui::CollapsingHeader::new(format!(
        "DAT save results · {saved} saved · {warnings} with warnings · {failures} failed · {not_saved} not saved"
    ))
    .id_salt(("dat-save-results", failures > 0))
    .default_open(failures > 0)
    .show(ui, |ui| {
        ui.label("Latest completed result per source and operation · this session");
    for result in results {
        let (label, tone) = match result.outcome {
            DatSaveOutcome::Success => ("Saved", widgets::StatusTone::Success),
            DatSaveOutcome::SuccessWithWarnings => {
                ("Saved with warnings", widgets::StatusTone::Warning)
            }
            DatSaveOutcome::PersistenceFailure => ("Save failed", widgets::StatusTone::Blocked),
            DatSaveOutcome::PartialFailure => ("Partial failure", widgets::StatusTone::Blocked),
            DatSaveOutcome::NotSaved => ("Not saved", widgets::StatusTone::Warning),
        };
        // Source labels may themselves be a selected path. Only the basename
        // is needed in the ordinary card; exact provenance stays in details.
        let name = std::path::Path::new(&result.source_name)
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "DAT source".into());
        widgets::banner(
            ui,
            &format!("{} · {name} · {label}", result.operation.label()),
            &result.explanation,
            tone,
        );
        if let Some(target) = &result.target {
            ui.label(format!("Checked: {target}"));
        }
        egui::CollapsingHeader::new("Technical details (may include local paths)")
            .id_salt(("dat-save-result", result.key()))
            .show(ui, |ui| {
                ui.label(result.display_detail(&format!(
                    "Source: {} ({})",
                    result.source_name, result.source_id
                )));
                for detail in &result.technical_details {
                    ui.label(result.display_detail(detail));
                }
            });
    }
    });
}
