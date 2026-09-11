use crate::*;

/// The deferred whole-file-checksum + No-Intro-lookup pass for a selected
/// loose file. Independent of `SelectedEvidenceState` so the structural /
/// verified identity in a `Ready` report is never held back by it. Guarded by
/// the same `selected_evidence_generation` the fast pass uses. Compressed
/// archives do not enter this state machine.
pub(crate) enum SelectedEvidenceEnrichmentState {
    Idle,
    Loading {
        generation: u64,
        path: PathBuf,
        receiver: mpsc::Receiver<(
            u64,
            Result<selected_evidence_page::SelectedEvidenceEnrichment, String>,
        )>,
    },
    /// Terminal: the enrichment was merged into the `Ready` report, or it
    /// failed (structural / verified identity stays visible regardless).
    /// Not retried until the selection changes.
    Done {
        generation: u64,
        path: PathBuf,
    },
}

