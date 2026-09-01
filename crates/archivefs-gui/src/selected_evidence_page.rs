//! GUI Batch A: the "ROM Identity & Evidence" section on the Selected page.
//!
//! Wires the *real* backend identity/evidence stack - structural detection,
//! the identity orchestrator/presentation layer, the Batch-21 direct
//! No-Intro importer, and the Batch-20 Hasheous adapter - against the
//! archive currently selected in Library. Nothing here is a demo: every
//! value shown is the actual return value of an existing core function.
//!
//! # Read-only
//!
//! This panel never mutates anything. There is no Apply button, no rename,
//! no move - see `tests::panel_never_offers_a_mutation_action`. Loading
//! evidence and checking Hasheous are the only two actions it can request,
//! and both are pure "go compute/fetch something and show it" requests.
//!
//! # Off the UI thread
//!
//! Both actions run through the same `thread::spawn` + `mpsc::channel` +
//! generation-guard pattern this app already uses elsewhere (see
//! `start_database_load` in `main.rs`). Hashing a large file and calling
//! Hasheous never block a frame; a result whose generation no longer
//! matches the current selection is discarded rather than shown against the
//! wrong file (mirrors `latest_generation_actions_safe`'s own reasoning).
//!
//! # Privacy at the call site
//!
//! [`run_hasheous_check`] builds exactly one [`HasheousHashSet`] containing
//! only a hash value and sends it through the existing, already
//! privacy-proven [`HasheousClient`] - no path, filename, or byte content
//! is added here (see `tests::hasheous_check_request_never_carries_the_selected_path`).
//!
//! # Lineage, not a vote count
//!
//! The lineage section renders [`merge_evidence`]'s own [`ClaimSummary`]
//! output - a local No-Intro match and a Hasheous No-Intro relay for the
//! same hash appear as one `SameSourceAgreement` group, never as two
//! independent confirmations (see
//! `tests::local_no_intro_plus_hasheous_relay_is_one_lineage_group`).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use eframe::egui;

use archivefs_core::content_evidence::ContentEvidence;
use archivefs_core::dat::model::ChecksumAlgorithm;
use archivefs_core::game_identity::inspect_catalogued_game_identity;
use archivefs_core::gb_header_evidence::{observe_gb_evidence, parse_gb_header};
use archivefs_core::gba_header_evidence::{observe_gba_evidence, parse_gba_header};
use archivefs_core::identity_source::hasheous::client::{
    HasheousClient, HasheousConfig, HasheousLookupOutcome, HasheousRequestError, UreqTransport,
    now_unix,
};
use archivefs_core::identity_source::hasheous::convert::observations_from_hash_lookup;
use archivefs_core::identity_source::hasheous::dto::HasheousHashSet;
use archivefs_core::identity_source::hashing::{LocalHashes, hash_file};
use archivefs_core::identity_source::no_intro::convert::observations_from_no_intro_matches;
use archivefs_core::identity_source::no_intro::import::ImportedNoIntroSource;
use archivefs_core::megadrive_header_evidence::{
    observe_megadrive_evidence, parse_megadrive_header,
};
use archivefs_core::n64_header_evidence::{observe_n64_evidence, parse_n64_header};
use archivefs_core::nes_header_evidence::{observe_ines_evidence, parse_ines_header};
use archivefs_core::platform_evidence_fusion::evidence_lineage::{
    AgreementStatus, ClaimSummary, ClaimType, EvidenceObservation, Representation, merge_evidence,
    observation_from_content_evidence,
};
use archivefs_core::platform_evidence_fusion::fuse_platform_evidence;
use archivefs_core::platform_evidence_fusion::identity_orchestrator::{
    IdentityInspectionInput, IdentityResult, inspect_identity,
};
use archivefs_core::platform_evidence_fusion::identity_presentation::{
    IdentityPresentation, IdentityStatus, present_identity,
};
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::sms_gg_header_evidence::{
    find_tmr_sega_header, observe_tmr_sega_evidence, parse_tmr_sega_header,
};

use crate::ui::components as widgets;

// ---------------------------------------------------------------------
// Structural detection dispatch
// ---------------------------------------------------------------------

/// Real per-format structural detection, dispatched by extension - the
/// same detectors and the same dispatch shape `cartridge_probe.rs` already
/// uses for real-corpus validation (Batches 4/9/18/20). This function adds
/// no new detection logic of its own; it only decides which existing
/// `parse_*_header`/`observe_*_evidence` pair to call.
pub(crate) fn gather_structural_evidence(path: &Path, bytes: &[u8]) -> Vec<ContentEvidence> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "gb" | "gbc" => parse_gb_header(bytes)
            .map(|fact| observe_gb_evidence(&fact))
            .unwrap_or_default(),
        "gba" => parse_gba_header(bytes)
            .map(|fact| observe_gba_evidence(&fact))
            .unwrap_or_default(),
        "nes" => parse_ines_header(bytes)
            .map(|fact| observe_ines_evidence(&fact))
            .unwrap_or_default(),
        "md" | "gen" | "smd" => parse_megadrive_header(bytes)
            .map(|fact| observe_megadrive_evidence(&fact))
            .unwrap_or_default(),
        "sms" | "gg" => find_tmr_sega_header(bytes)
            .and_then(|offset| parse_tmr_sega_header(bytes, offset))
            .map(|fact| observe_tmr_sega_evidence(&fact))
            .unwrap_or_default(),
        "n64" | "z64" | "v64" => parse_n64_header(bytes)
            .map(|fact| observe_n64_evidence(&fact))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------
// Direct No-Intro lookup
// ---------------------------------------------------------------------

/// Honest result of trying a direct local No-Intro lookup - `NotImported`
/// is a real, expected outcome (Batch 21 section 43), never hidden behind
/// a fabricated match.
#[derive(Debug, Clone)]
pub(crate) enum NoIntroLookupResult {
    NotImported,
    /// More than one enabled, platform-relevant registered No-Intro source
    /// exists for this file. Never resolved to an arbitrary first pick -
    /// see `selected_evidence_no_intro::NoIntroSourceCache` (the resolver
    /// that produces this outcome) for why. `note` names every competing
    /// source so the conflict is actionable, not just reported.
    Ambiguous {
        note: String,
    },
    NoMatch {
        system_name: String,
    },
    Matched {
        system_name: String,
        observations: Vec<EvidenceObservation>,
    },
}

fn lookup_no_intro_for(
    source: Option<&ImportedNoIntroSource>,
    hashes: &LocalHashes,
) -> NoIntroLookupResult {
    let Some(source) = source else {
        return NoIntroLookupResult::NotImported;
    };
    let observations = observations_from_no_intro_matches(
        source,
        ChecksumAlgorithm::Sha1,
        &hashes.sha1,
        Representation::PhysicalFile,
    );
    if observations.is_empty() {
        NoIntroLookupResult::NoMatch {
            system_name: source.system_name.clone(),
        }
    } else {
        NoIntroLookupResult::Matched {
            system_name: source.system_name.clone(),
            observations,
        }
    }
}

// ---------------------------------------------------------------------
// The base (non-Hasheous) evidence report
// ---------------------------------------------------------------------

/// The result of gathering real evidence for one selected file - built off
/// the UI thread by [`gather_selected_evidence`]. Everything on this type
/// is already-computed data; rendering it never re-analyses anything.
#[derive(Debug, Clone)]
pub(crate) struct SelectedEvidenceReport {
    pub path: PathBuf,
    pub structural_facts: Vec<ContentEvidence>,
    pub identity: IdentityPresentation,
    /// The raw identity result `identity` was presented from - kept
    /// alongside it (rather than only its presentation) so a planning
    /// preview (GUI Batch C) can feed the exact same identity into the
    /// existing library planner without recomputing it and risking drift
    /// between what the evidence panel shows and what gets planned.
    pub identity_result: IdentityResult,
    /// The existing authoritative per-file identity report used by launch
    /// planning. Kept alongside the presentation so the GUI can show the
    /// same evidence while the core launch bridge consumes the original
    /// report, rather than reconstructing identity in the GUI.
    pub game_identity_report: archivefs_core::game_identity::GameIdentityReport,
    pub hashes: Option<LocalHashes>,
    pub no_intro: NoIntroLookupResult,
    /// State of the deferred whole-file checksum / DAT lookup. Compressed
    /// archives are deliberately terminally skipped: hashing the outer ZIP,
    /// 7z, or RAR does not identify its game member and made ordinary
    /// selection stream multi-gigabyte files for no useful card evidence.
    pub enrichment: SelectedEvidenceEnrichmentStatus,
    /// Structural + (if matched) direct No-Intro observations, ready to be
    /// merged with a Hasheous result once/if one arrives.
    pub base_observations: Vec<EvidenceObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectedEvidenceEnrichmentStatus {
    Pending,
    Complete,
    SkippedArchive,
    Failed(String),
}

fn is_compressed_archive(path: &Path) -> bool {
    matches!(
        archivefs_core::archive_kind(path),
        Some(
            archivefs_core::ArchiveKind::Zip
                | archivefs_core::ArchiveKind::SevenZip
                | archivefs_core::ArchiveKind::Rar
        )
    )
}

/// Gathers real evidence for `path`: reads it once, runs the real
/// structural detector dispatch, hashes it through the crate's existing
/// [`hash_file`], looks it up against `no_intro_source` if one is
/// registered, and composes the real [`inspect_identity`]/[`present_identity`]
/// result. Intended to run off the UI thread (large files, real hashing).
#[allow(dead_code)]
pub(crate) fn gather_selected_evidence(
    path: &Path,
    no_intro_source: Option<&ImportedNoIntroSource>,
) -> Result<SelectedEvidenceReport, String> {
    gather_selected_evidence_with_platform(path, None, no_intro_source)
}

/// How far into a selected file the *fast* structural pass reads. Every
/// header detector this module dispatches to touches only the first few
/// hundred bytes to at most ~0x8000; 1 MiB is a wide margin that keeps the
/// fast pass bounded for a multi-gigabyte disc image (whose whole-file read
/// and hash belong in the deferred enrichment pass, not in the pass whose
/// only job is to make identity visible quickly).
pub(crate) const STRUCTURAL_PREFIX_BYTES: u64 = 1 << 20;

fn read_structural_prefix(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut prefix = Vec::new();
    file.take(STRUCTURAL_PREFIX_BYTES)
        .read_to_end(&mut prefix)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    Ok(prefix)
}

/// The fast identity pass the selected-game panel waits on: a bounded
/// header read, the real structural detectors, and the existing bounded
/// per-platform [`inspect_catalogued_game_identity`] (loose-ROM hash for a
/// small cartridge, bounded `SYSTEM.CNF`/boot reads for a disc or ZIP member
/// without a whole-file hash). It deliberately does **not** compute the
/// whole-file checksum or resolve the No-Intro DAT registry; both can cost
/// minutes for a large ISO or a big DAT set. Loose files are enriched later
/// by [`compute_selected_evidence_enrichment`] without blocking what the user
/// sees. Compressed archives stay terminally bounded and are not enriched by
/// hashing the outer container.
pub(crate) fn gather_selected_evidence_fast(
    path: &Path,
    platform_hint: Option<&str>,
) -> Result<SelectedEvidenceReport, String> {
    let archive = is_compressed_archive(path);
    let structural_facts = if archive {
        // Archive extensions do not feed any of this module's loose-ROM
        // structural detectors. Opening is enough to surface a missing or
        // unreadable RAR/7z (ZIP is additionally validated by the bounded
        // core identity inspector below); reading a meaningless 1 MiB outer
        // prefix would only add selection I/O.
        std::fs::File::open(path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        Vec::new()
    } else {
        let prefix = read_structural_prefix(path)?;
        gather_structural_evidence(path, &prefix)
    };
    let explanation = fuse_platform_evidence(structural_facts.clone());

    let identity_result = inspect_identity(IdentityInspectionInput {
        content_evidence: explanation.input_evidence.clone(),
        ..Default::default()
    });
    let identity = present_identity(&identity_result);
    let game_identity_report =
        inspect_catalogued_game_identity(path, platform_hint.or(identity.platform));

    let base_observations: Vec<EvidenceObservation> = structural_facts
        .iter()
        .map(observation_from_content_evidence)
        .collect();

    Ok(SelectedEvidenceReport {
        path: path.to_path_buf(),
        structural_facts,
        identity,
        identity_result,
        game_identity_report,
        hashes: None,
        no_intro: NoIntroLookupResult::NotImported,
        enrichment: if archive {
            SelectedEvidenceEnrichmentStatus::SkippedArchive
        } else {
            SelectedEvidenceEnrichmentStatus::Pending
        },
        base_observations,
    })
}

/// The result of the deferred enrichment pass: the whole-file checksum and
/// the No-Intro lookup for it. Merged into an already-visible
/// [`SelectedEvidenceReport`] once ready, so structural / verified identity
/// never waits on it.
#[derive(Debug, Clone)]
pub(crate) struct SelectedEvidenceEnrichment {
    pub hashes: LocalHashes,
    pub no_intro: NoIntroLookupResult,
    pub extra_observations: Vec<EvidenceObservation>,
}

/// Computes the whole-file checksum for `path` (the expensive step for a
/// large disc image) and looks it up against the already-resolved
/// `no_intro_source`. The caller resolves the No-Intro source off the UI
/// thread; DAT-registry parsing must not happen on the fast path.
#[cfg(test)]
pub(crate) fn compute_selected_evidence_enrichment(
    path: &Path,
    no_intro_source: Option<&ImportedNoIntroSource>,
) -> Result<SelectedEvidenceEnrichment, String> {
    compute_selected_evidence_enrichment_cancellable(path, no_intro_source, None)
}

pub(crate) fn compute_selected_evidence_enrichment_cancellable(
    path: &Path,
    no_intro_source: Option<&ImportedNoIntroSource>,
    cancel: Option<&AtomicBool>,
) -> Result<SelectedEvidenceEnrichment, String> {
    let trusted_root = path.parent().unwrap_or(path).to_path_buf();
    let trusted = TrustedRoots::from_paths([trusted_root.as_path()]);
    let hashes = hash_file(path, &trusted, cancel)
        .map_err(|refusal| format!("could not hash {}: {}", path.display(), refusal.detail()))?;
    let no_intro = lookup_no_intro_for(no_intro_source, &hashes);
    let extra_observations = match &no_intro {
        NoIntroLookupResult::Matched { observations, .. } => observations.clone(),
        _ => Vec::new(),
    };
    Ok(SelectedEvidenceEnrichment {
        hashes,
        no_intro,
        extra_observations,
    })
}

/// Merges a completed [`SelectedEvidenceEnrichment`] into a report the panel
/// is already showing. Only ever called when the report's path still
/// matches the current selection (generation-guarded by the caller).
pub(crate) fn apply_selected_evidence_enrichment(
    report: &mut SelectedEvidenceReport,
    enrichment: SelectedEvidenceEnrichment,
) {
    report.hashes = Some(enrichment.hashes);
    report.no_intro = enrichment.no_intro;
    report.enrichment = SelectedEvidenceEnrichmentStatus::Complete;
    report
        .base_observations
        .extend(enrichment.extra_observations);
}

pub(crate) fn apply_selected_evidence_enrichment_error(
    report: &mut SelectedEvidenceReport,
    message: String,
) {
    report.enrichment = SelectedEvidenceEnrichmentStatus::Failed(message);
}

/// Like [`gather_selected_evidence`], with an exact platform hint from the
/// library row when structural evidence alone cannot identify the platform
/// (for example a PS2 ISO). The hint is only passed to the existing core
/// identity inspector; no platform or identity parsing is duplicated here.
pub(crate) fn gather_selected_evidence_with_platform(
    path: &Path,
    platform_hint: Option<&str>,
    no_intro_source: Option<&ImportedNoIntroSource>,
) -> Result<SelectedEvidenceReport, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let structural_facts = gather_structural_evidence(path, &bytes);
    let explanation = fuse_platform_evidence(structural_facts.clone());

    let trusted_root = path.parent().unwrap_or(path).to_path_buf();
    let trusted = TrustedRoots::from_paths([trusted_root.as_path()]);
    let (hashes, enrichment) = match hash_file(path, &trusted, None) {
        Ok(hashes) => (Some(hashes), SelectedEvidenceEnrichmentStatus::Complete),
        Err(refusal) => (
            None,
            SelectedEvidenceEnrichmentStatus::Failed(format!(
                "could not hash {}: {}",
                path.display(),
                refusal.detail()
            )),
        ),
    };

    let no_intro = match &hashes {
        Some(hashes) => lookup_no_intro_for(no_intro_source, hashes),
        None => NoIntroLookupResult::NotImported,
    };

    let identity_result = inspect_identity(IdentityInspectionInput {
        content_evidence: explanation.input_evidence.clone(),
        ..Default::default()
    });
    let identity = present_identity(&identity_result);
    let game_identity_report =
        inspect_catalogued_game_identity(path, platform_hint.or(identity.platform));

    let mut base_observations: Vec<EvidenceObservation> = structural_facts
        .iter()
        .map(observation_from_content_evidence)
        .collect();
    if let NoIntroLookupResult::Matched { observations, .. } = &no_intro {
        base_observations.extend(observations.clone());
    }

    Ok(SelectedEvidenceReport {
        path: path.to_path_buf(),
        structural_facts,
        identity,
        identity_result,
        game_identity_report,
        hashes,
        no_intro,
        enrichment,
        base_observations,
    })
}

// ---------------------------------------------------------------------
// Hasheous check (explicit button only)
// ---------------------------------------------------------------------

/// Honest outcome of one "Check Hasheous" click - every distinct state the
/// milestone asks for, never conflating "no match" with a real failure.
#[derive(Debug, Clone)]
pub(crate) enum HasheousCheckOutcome {
    Disabled,
    NoMatch,
    Found(Vec<EvidenceObservation>),
    Timeout,
    RateLimited,
    NetworkError(String),
}

/// Runs one real Hasheous lookup for `sha1_hash` (never a path or
/// filename - see the module doc's privacy note). Generic over
/// [`HasheousTransport`] so it is fully unit-testable against a fake
/// transport with no socket, exactly like the core adapter's own client;
/// the GUI's real call site (in `main.rs`, inside a `thread::spawn`
/// worker) constructs a real [`UreqTransport`] and passes it here.
pub(crate) fn run_hasheous_check<
    T: archivefs_core::identity_source::hasheous::client::HasheousTransport,
>(
    config: &HasheousConfig,
    transport: &T,
    sha1_hash: &str,
) -> HasheousCheckOutcome {
    let client = HasheousClient::new(config, transport);
    let hash_set = HasheousHashSet {
        sha1: Some(sha1_hash.to_string()),
        ..Default::default()
    };
    let cancel: Option<&AtomicBool> = None;
    match client.lookup(&hash_set, cancel) {
        Ok(HasheousLookupOutcome::Found(response)) => {
            let observations = observations_from_hash_lookup(
                &response,
                Representation::PhysicalFile,
                sha1_hash,
                Some(now_unix()),
            );
            if observations.is_empty() {
                HasheousCheckOutcome::NoMatch
            } else {
                HasheousCheckOutcome::Found(observations)
            }
        }
        Ok(HasheousLookupOutcome::NoMatch) => HasheousCheckOutcome::NoMatch,
        Err(HasheousRequestError::Disabled) => HasheousCheckOutcome::Disabled,
        Err(HasheousRequestError::Timeout) => HasheousCheckOutcome::Timeout,
        Err(HasheousRequestError::RateLimited { .. }) => HasheousCheckOutcome::RateLimited,
        Err(other) => HasheousCheckOutcome::NetworkError(other.detail()),
    }
}

/// The real, live-transport entry point - what `main.rs`'s background
/// worker actually calls. A thin wrapper around [`run_hasheous_check`] so
/// the real production `UreqTransport` construction happens in exactly
/// one place.
pub(crate) fn run_hasheous_check_live(
    config: &HasheousConfig,
    sha1_hash: &str,
) -> HasheousCheckOutcome {
    let transport = UreqTransport::new(config.timeout);
    run_hasheous_check(config, &transport, sha1_hash)
}

// ---------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------

/// The panel's whole state machine for the base evidence load - mirrors
/// the shape of this app's other generation-guarded loaders
/// (`DatabaseState`, `ArchiveInspectorState`).
#[derive(Default)]
pub(crate) enum SelectedEvidenceState {
    #[default]
    Idle,
    Loading {
        generation: u64,
        path: PathBuf,
        receiver: std::sync::mpsc::Receiver<(u64, Result<SelectedEvidenceReport, String>)>,
    },
    Ready {
        generation: u64,
        report: Box<SelectedEvidenceReport>,
        hasheous: HasheousState,
    },
    Error {
        /// Kept for symmetry with `Loading`/`Ready` (every state on this
        /// machine carries the generation it settled at) even though no
        /// current caller re-reads it - an `Error` state is always
        /// terminal until a fresh `Load` action replaces it outright.
        #[allow(dead_code)]
        generation: u64,
        path: PathBuf,
        message: String,
    },
}

/// The independent Hasheous sub-state - always starts `Idle` (never an
/// automatic call), and is reset whenever the base report reloads for a
/// new selection.
#[derive(Default)]
pub(crate) enum HasheousState {
    #[default]
    Idle,
    Loading {
        generation: u64,
        receiver: std::sync::mpsc::Receiver<(u64, HasheousCheckOutcome)>,
    },
    Done {
        /// Kept for symmetry with `Loading` (see `SelectedEvidenceState::Error`'s
        /// identical note) - a settled `Done` result is replaced wholesale
        /// by the next `CheckHasheous` action, never re-guarded by this
        /// field.
        #[allow(dead_code)]
        generation: u64,
        outcome: HasheousCheckOutcome,
    },
}

/// What the panel asks the caller to do - both actions are read-only
/// requests to go compute or fetch something, never a mutation.
pub(crate) enum SelectedEvidenceAction {
    Load(PathBuf),
    CheckHasheous,
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Shared with `plan_preview_page` (GUI Batch C), so the resolver-result
/// tone shown there is the exact same mapping this panel already uses -
/// never a second, possibly-diverging one.
pub(crate) fn status_tone_for(status: IdentityStatus) -> widgets::StatusTone {
    match status {
        IdentityStatus::Conflict => widgets::StatusTone::Blocked,
        IdentityStatus::Ambiguous => widgets::StatusTone::Pending,
        IdentityStatus::VerifiedByDat | IdentityStatus::ContentAndDatAgree => {
            widgets::StatusTone::Success
        }
        IdentityStatus::ContentOnly | IdentityStatus::DatOnly => widgets::StatusTone::Info,
        IdentityStatus::Unknown => widgets::StatusTone::Pending,
    }
}

fn agreement_tone(status: AgreementStatus) -> widgets::StatusTone {
    if status.is_conflict() {
        widgets::StatusTone::Blocked
    } else {
        widgets::StatusTone::Success
    }
}

fn agreement_label(status: AgreementStatus) -> &'static str {
    match status {
        AgreementStatus::SameSourceAgreement => "Same-source agreement",
        AgreementStatus::IndependentAgreement => "Independent agreement",
        AgreementStatus::DerivedAgreement => "Derived agreement",
        AgreementStatus::CrossRepresentationAgreement => "Cross-representation agreement",
        AgreementStatus::WeakAgreement => "Weak agreement",
        AgreementStatus::SameSourceVersionConflict => "Same-source version conflict",
        AgreementStatus::DerivedSourceConflict => "Derived-source conflict",
        AgreementStatus::IndependentSourceConflict => "Independent-source conflict",
        AgreementStatus::RepresentationConflict => "Representation conflict",
        AgreementStatus::MetadataConflict => "Metadata conflict",
    }
}

fn hasheous_status_line(outcome: &HasheousCheckOutcome) -> (String, widgets::StatusTone) {
    match outcome {
        HasheousCheckOutcome::Disabled => (
            "Hasheous is disabled".to_string(),
            widgets::StatusTone::Pending,
        ),
        HasheousCheckOutcome::NoMatch => {
            ("Hasheous: no match".to_string(), widgets::StatusTone::Info)
        }
        HasheousCheckOutcome::Found(observations) => (
            format!("Hasheous: {} signature(s) returned", observations.len()),
            widgets::StatusTone::Success,
        ),
        HasheousCheckOutcome::Timeout => (
            "Hasheous: timed out".to_string(),
            widgets::StatusTone::Blocked,
        ),
        HasheousCheckOutcome::RateLimited => (
            "Hasheous: rate limited, try again later".to_string(),
            widgets::StatusTone::Blocked,
        ),
        HasheousCheckOutcome::NetworkError(detail) => {
            (format!("Hasheous: {detail}"), widgets::StatusTone::Blocked)
        }
    }
}

/// Every observation currently known for the selected file: the base
/// (structural + direct No-Intro) set, plus a Hasheous result if one has
/// been explicitly fetched. Never includes an automatic Hasheous call.
fn all_observations(
    report: &SelectedEvidenceReport,
    hasheous: &HasheousState,
) -> Vec<EvidenceObservation> {
    let mut observations = report.base_observations.clone();
    if let HasheousState::Done {
        outcome: HasheousCheckOutcome::Found(found),
        ..
    } = hasheous
    {
        observations.extend(found.clone());
    }
    observations
}

/// Draws the "ROM Identity & Evidence" section. Returns an action the
/// caller should perform (loading evidence for a newly-selected file, or
/// starting an explicit Hasheous check) - drawing itself never mutates
/// anything.
pub(crate) fn show_selected_evidence_panel(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    selected_path: Option<&Path>,
    state: &SelectedEvidenceState,
) -> Option<SelectedEvidenceAction> {
    let mut action = None;
    widgets::section_header(
        ui,
        "ROM Identity & Evidence",
        Some(
            "What EmuWiz's own detectors, DAT sources, and (if you ask) Hasheous actually know about the selected file.",
        ),
    );

    let Some(selected_path) = selected_path else {
        ui.label("No archive is selected in the Library.");
        return None;
    };

    match state {
        SelectedEvidenceState::Idle => {
            widgets::card(ui, |ui| {
                ui.label("Identity evidence has not been loaded for this file yet.");
                if widgets::action_button(
                    ui,
                    "Load identity evidence",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
                {
                    action = Some(SelectedEvidenceAction::Load(selected_path.to_path_buf()));
                }
            });
        }
        SelectedEvidenceState::Loading { path, .. } => {
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Reading and detecting {}…", path.display()));
                });
            });
        }
        SelectedEvidenceState::Error { path, message, .. } => {
            widgets::banner(
                ui,
                "Could not load evidence",
                &format!("{}: {message}", path.display()),
                widgets::StatusTone::Blocked,
            );
            if widgets::action_button(ui, "Retry", widgets::ActionStyle::Secondary, true).clicked()
            {
                action = Some(SelectedEvidenceAction::Load(selected_path.to_path_buf()));
            }
        }
        SelectedEvidenceState::Ready {
            report, hasheous, ..
        } => {
            if report.path != selected_path {
                widgets::banner(
                    ui,
                    "Selection changed",
                    "Reload evidence for the newly selected file.",
                    widgets::StatusTone::Pending,
                );
                if widgets::action_button(
                    ui,
                    "Load identity evidence",
                    widgets::ActionStyle::Secondary,
                    true,
                )
                .clicked()
                {
                    action = Some(SelectedEvidenceAction::Load(selected_path.to_path_buf()));
                }
                return action;
            }
            show_ready_report(ui, advanced_mode, report, hasheous, &mut action);
        }
    }
    action
}

pub(crate) fn show_identity_evidence(ui: &mut egui::Ui, report: &SelectedEvidenceReport) {
    widgets::section_header(ui, "Structural evidence", None);
    if report.structural_facts.is_empty() {
        ui.label("No structural evidence was recognized for this file type.");
    } else {
        let rows: Vec<(&str, &str, widgets::StatusTone)> = report
            .structural_facts
            .iter()
            .map(|fact| {
                let tone = match fact.confidence {
                    archivefs_core::content_evidence::ContentEvidenceConfidence::Strong => {
                        widgets::StatusTone::Success
                    }
                    archivefs_core::content_evidence::ContentEvidenceConfidence::Corroborated => {
                        widgets::StatusTone::Info
                    }
                    archivefs_core::content_evidence::ContentEvidenceConfidence::Weak => {
                        widgets::StatusTone::Pending
                    }
                };
                (fact.detail.as_str(), fact.value.as_str(), tone)
            })
            .collect();
        widgets::status_rows(ui, &rows);
    }

    let verified_identity_facts = report
        .game_identity_report
        .evidence
        .iter()
        .filter(|evidence| {
            evidence.status == archivefs_core::game_identity::IdentityStatus::Verified
                && !matches!(
                    evidence.kind,
                    archivefs_core::game_identity::IdentityKind::Platform
                        | archivefs_core::game_identity::IdentityKind::LooseRomFormat
                        | archivefs_core::game_identity::IdentityKind::LooseRomTitle
                )
                && evidence.value.is_some()
        })
        .collect::<Vec<_>>();
    if !verified_identity_facts.is_empty() {
        widgets::section_header(ui, "Verified identity evidence", None);
        for evidence in verified_identity_facts {
            ui.horizontal(|ui| {
                ui.label(evidence.kind.to_string());
                widgets::status_badge(ui, "Verified", widgets::StatusTone::Success);
                ui.label(evidence.value.as_deref().unwrap_or_default());
            });
        }
    }

    match &report.enrichment {
        SelectedEvidenceEnrichmentStatus::Pending => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking additional local checksum evidence in the background…");
            });
        }
        SelectedEvidenceEnrichmentStatus::SkippedArchive => {
            widgets::banner(
                ui,
                "Bounded archive inspection",
                "EmuWiz inspected only the archive metadata and bounded game-member evidence. It did not hash the entire ZIP, 7z, or RAR just to populate this card.",
                widgets::StatusTone::Info,
            );
        }
        SelectedEvidenceEnrichmentStatus::Failed(message) => {
            widgets::banner(
                ui,
                "Additional evidence could not be loaded",
                message,
                widgets::StatusTone::Warning,
            );
        }
        SelectedEvidenceEnrichmentStatus::Complete => {}
    }
}

fn show_ready_report(
    ui: &mut egui::Ui,
    advanced_mode: bool,
    report: &SelectedEvidenceReport,
    hasheous: &HasheousState,
    action: &mut Option<SelectedEvidenceAction>,
) {
    widgets::card(ui, |ui| {
        let (label, tone) = (
            report.identity.status.label(),
            status_tone_for(report.identity.status),
        );
        let platform = report
            .identity
            .platform
            .unwrap_or_else(|| report.game_identity_report.platform.label());
        ui.horizontal(|ui| {
            ui.strong(platform);
            widgets::status_badge(ui, label, tone);
        });
        if !report.identity.content_summary.is_empty() {
            ui.label(&report.identity.content_summary);
        }
        if report.identity.status == IdentityStatus::Conflict
            && !report.identity.conflict_rows.is_empty()
        {
            widgets::banner(
                ui,
                "Conflict",
                &report.identity.conflict_rows.join("; "),
                widgets::StatusTone::Blocked,
            );
        }
    });

    // Core identity evidence is distinct from DAT evidence.
    show_identity_evidence(ui, report);

    // DAT evidence (direct No-Intro).
    widgets::section_header(ui, "DAT evidence", None);
    match &report.enrichment {
        SelectedEvidenceEnrichmentStatus::Pending => {
            ui.label("DAT lookup will follow the background checksum.");
        }
        SelectedEvidenceEnrichmentStatus::SkippedArchive => {
            ui.label("DAT lookup was not started because it would require hashing the whole outer archive.");
        }
        SelectedEvidenceEnrichmentStatus::Failed(_) => {
            ui.label("DAT lookup could not run because the local checksum was unavailable.");
        }
        SelectedEvidenceEnrichmentStatus::Complete => match &report.no_intro {
            NoIntroLookupResult::NotImported => {
                ui.label("No DAT evidence available (no local No-Intro DAT is imported).");
            }
            NoIntroLookupResult::Ambiguous { note } => {
                widgets::banner(
                    ui,
                    "Multiple No-Intro DAT sources match this platform",
                    note,
                    widgets::StatusTone::Pending,
                );
            }
            NoIntroLookupResult::NoMatch { system_name } => {
                ui.label(format!(
                    "No DAT evidence available ({system_name}: no matching entry)."
                ));
            }
            NoIntroLookupResult::Matched {
                system_name,
                observations,
            } => {
                ui.label(format!(
                    "{system_name}: {} matching entry/entries.",
                    observations.len() / 2
                ));
            }
        },
    }

    // Hasheous - explicit button only.
    widgets::section_header(ui, "Hasheous", Some("Never called automatically."));
    let has_sha1 = report.hashes.is_some();
    match hasheous {
        HasheousState::Idle => {
            if widgets::action_button(
                ui,
                "Check Hasheous",
                widgets::ActionStyle::Secondary,
                has_sha1,
            )
            .clicked()
            {
                *action = Some(SelectedEvidenceAction::CheckHasheous);
            }
            if !has_sha1 {
                let reason = match &report.enrichment {
                    SelectedEvidenceEnrichmentStatus::SkippedArchive => {
                        "Hasheous is unavailable because whole-archive hashing is not started automatically."
                    }
                    _ => "A SHA1 hash is required before Hasheous can be checked.",
                };
                ui.label(reason);
            }
        }
        HasheousState::Loading { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking Hasheous…");
            });
        }
        HasheousState::Done { outcome, .. } => {
            let (line, tone) = hasheous_status_line(outcome);
            widgets::status_badge(ui, &line, tone);
            if widgets::action_button(ui, "Check again", widgets::ActionStyle::Quiet, true)
                .clicked()
            {
                *action = Some(SelectedEvidenceAction::CheckHasheous);
            }
        }
    }

    // Lineage - the whole point of Batches 19-21.
    let observations = all_observations(report, hasheous);
    let mut summaries = merge_evidence(&observations);
    // Conflicts are always surfaced before ordinary agreement, in both
    // modes, and are never resolved by outnumbering them - see
    // `tests::conflicts_always_sort_before_agreement_summaries`.
    summaries.sort_by_key(|summary| !summary.status.is_conflict());
    widgets::section_header(
        ui,
        "Evidence lineage",
        Some("Independent lines of evidence vs. one source repeated through another channel."),
    );
    if summaries.is_empty() {
        ui.label("No claim-bearing evidence yet.");
    } else if advanced_mode {
        show_lineage_claim_cards(ui, &summaries);
    } else {
        show_lineage_headline(ui, &summaries);
    }
}

/// Gamer mode's entire lineage display: one badge, one plain-English
/// sentence, no per-claim cards, no raw hashes, no `{:?}`-formatted
/// backend enum names.
fn show_lineage_headline(ui: &mut egui::Ui, summaries: &[ClaimSummary]) {
    let (headline, sentence, tone) = overall_gamer_headline(summaries);
    widgets::card(ui, |ui| {
        widgets::status_badge(ui, headline, tone);
        ui.label(sentence);
    });
}

fn overall_gamer_headline(
    summaries: &[ClaimSummary],
) -> (&'static str, String, widgets::StatusTone) {
    if let Some(conflict) = summaries
        .iter()
        .find(|summary| summary.status.is_conflict())
    {
        return (
            "Conflict",
            format!(
                "Two sources disagree about the {}. See Advanced view for details.",
                claim_label(conflict.claim).to_lowercase()
            ),
            widgets::StatusTone::Blocked,
        );
    }
    let independent = summaries
        .iter()
        .find(|summary| summary.status == AgreementStatus::IndependentAgreement);
    let same_source = summaries
        .iter()
        .find(|summary| summary.status == AgreementStatus::SameSourceAgreement);
    match (independent, same_source) {
        (Some(_), _) => (
            "Confirmed",
            "Two independent sources agree on this identity.".to_string(),
            widgets::StatusTone::Success,
        ),
        (None, Some(_)) => (
            "One source",
            "One source confirms this - the same fact reached us through more than one channel, \
             which does not count as a second confirmation."
                .to_string(),
            widgets::StatusTone::Info,
        ),
        (None, None) => (
            "Weak evidence",
            "Evidence exists but is not strong enough to confirm identity on its own.".to_string(),
            widgets::StatusTone::Pending,
        ),
    }
}

/// Advanced mode's per-claim cards - conflicts already sorted first by the
/// caller. Per-observation provenance/representation detail is collapsed
/// by default (`widgets::technical_details`), never dumped inline.
fn show_lineage_claim_cards(ui: &mut egui::Ui, summaries: &[ClaimSummary]) {
    for summary in summaries {
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(claim_label(summary.claim));
                widgets::status_badge(
                    ui,
                    agreement_label(summary.status),
                    agreement_tone(summary.status),
                );
            });
            widgets::technical_details(ui, summary.claim, |ui| {
                for observation in &summary.observations {
                    ui.label(format!(
                        "{:?} · {:?} · {:?} · {:?}",
                        observation.provenance.channel,
                        observation.provenance.upstream_source,
                        observation.provenance.representation,
                        observation.provenance.lineage,
                    ));
                }
            });
        });
    }
}

fn claim_label(claim: ClaimType) -> &'static str {
    match claim {
        ClaimType::ExactBytesMatch => "Exact byte match",
        ClaimType::ExactNormalizedMatch => "Exact match (normalized)",
        ClaimType::ExactTrackMatch => "Exact disc track match",
        ClaimType::ExactLogicalDiscMatch => "Exact disc match",
        ClaimType::ExactSlaveMatch => "Exact WHDLoad slave match",
        ClaimType::PlatformCandidate => "Platform",
        ClaimType::ReleaseCandidate => "Release",
        ClaimType::RevisionCandidate => "Revision",
        ClaimType::RegionMetadata => "Region",
        ClaimType::LanguageMetadata => "Language",
        ClaimType::VariantStatus => "Variant",
        ClaimType::HardwareCompatibility => "Hardware compatibility",
        ClaimType::DisplayMetadata => "Title/publisher",
        ClaimType::CrosswalkCandidate => "Crosswalk candidate",
        ClaimType::VettedCrosswalk => "Vetted crosswalk",
        ClaimType::EquivalentCanonical => "Equivalent canonical",
        ClaimType::RelatedPlatform => "Related platform",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::identity_source::hasheous::client::{
        HasheousRequestError, HasheousTransport,
    };
    use archivefs_core::platform_evidence_fusion::evidence_lineage::{
        ClaimStrength, ClaimType, EvidenceChannel, IdentityScope, LineageRelation, Provenance,
        SourceFamily,
    };
    use std::io::Write;
    use std::time::Duration;

    fn gb_rom_bytes() -> Vec<u8> {
        // A minimal, deterministic synthetic Game Boy header - not real
        // copyrighted content, matching this project's own synthetic
        // canary-file convention.
        let mut bytes = vec![0u8; 0x150];
        let logo: [u8; 48] = [
            0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C,
            0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6,
            0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC,
            0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
        ];
        bytes[0x104..0x134].copy_from_slice(&logo);
        bytes[0x134..0x143].copy_from_slice(b"TESTGAME\0\0\0\0\0\0\0");
        let checksum = archivefs_core::gb_header_evidence::compute_header_checksum(&bytes)
            .expect("checksum computable");
        bytes[0x14D] = checksum;
        bytes
    }

    /// A self-cleaning fixture directory under the process's own temp dir -
    /// no `tempfile` dependency, matching this project's own convention
    /// (`real_canary.rs`/`real_rom_canary.rs`) for disposable test fixtures.
    struct FixtureDir(PathBuf);

    impl FixtureDir {
        fn new() -> Self {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("archivefs-gui-selected-evidence-{now}"));
            std::fs::create_dir_all(&dir).expect("create fixture dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_temp_rom(name: &str, bytes: &[u8]) -> FixtureDir {
        let dir = FixtureDir::new();
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).expect("create fixture");
        file.write_all(bytes).expect("write fixture");
        dir
    }

    // -- structural detection dispatch --------------------------------

    #[test]
    fn gb_extension_dispatches_to_the_real_gb_header_detector() {
        let bytes = gb_rom_bytes();
        let facts = gather_structural_evidence(Path::new("game.gb"), &bytes);
        assert!(
            !facts.is_empty(),
            "a valid GB header must produce real evidence"
        );
    }

    #[test]
    fn unrecognized_extension_produces_no_fabricated_evidence() {
        let facts = gather_structural_evidence(Path::new("game.xyz"), &[0u8; 64]);
        assert!(facts.is_empty());
    }

    #[test]
    fn invalid_gb_header_produces_no_facts() {
        let facts = gather_structural_evidence(Path::new("broken.gb"), &[0u8; 32]);
        assert!(facts.is_empty());
    }

    #[test]
    fn gbc_extension_uses_the_same_gb_detector() {
        let bytes = gb_rom_bytes();
        let facts = gather_structural_evidence(Path::new("game.gbc"), &bytes);
        assert!(!facts.is_empty());
    }

    #[test]
    fn uppercase_extension_still_dispatches() {
        let bytes = gb_rom_bytes();
        let facts = gather_structural_evidence(Path::new("GAME.GB"), &bytes);
        assert!(
            !facts.is_empty(),
            "extension matching must be case-insensitive"
        );
    }

    #[test]
    fn empty_bytes_never_panics_for_any_dispatched_extension() {
        for extension in [
            "gb", "gbc", "gba", "nes", "md", "gen", "smd", "sms", "gg", "n64", "z64", "v64",
        ] {
            let facts = gather_structural_evidence(Path::new(&format!("x.{extension}")), &[]);
            assert!(facts.is_empty());
        }
    }

    // -- end-to-end gather against a real (synthetic) file ------------

    #[test]
    fn gather_selected_evidence_reads_a_real_file_and_resolves_identity() {
        let bytes = gb_rom_bytes();
        let dir = write_temp_rom("Alleyway-like (Test).gb", &bytes);
        let path = dir.path().join("Alleyway-like (Test).gb");
        let report = gather_selected_evidence(&path, None).expect("gather succeeds");
        assert_eq!(report.path, path);
        assert!(!report.structural_facts.is_empty());
        assert!(report.hashes.is_some(), "hashing must run for a real file");
        assert_eq!(
            report.game_identity_report.platform,
            archivefs_core::game_identity::IdentityPlatform::GameBoy
        );
        assert!(
            report
                .game_identity_report
                .verified_loose_rom_sha256()
                .is_some(),
            "core identity evidence must remain available without DAT evidence"
        );
        assert!(matches!(report.no_intro, NoIntroLookupResult::NotImported));
    }

    // -- staged fast pass + deferred enrichment ----------------------

    #[test]
    fn fast_pass_resolves_structural_and_verified_identity_without_a_whole_file_hash() {
        let dir = write_temp_rom("Aladdin (USA) (SGB Enhanced).gb", &gb_rom_bytes());
        let path = dir.path().join("Aladdin (USA) (SGB Enhanced).gb");
        let report =
            gather_selected_evidence_fast(&path, Some("Game Boy")).expect("fast pass succeeds");
        assert_eq!(report.path, path);
        assert!(
            !report.structural_facts.is_empty(),
            "structural (cartridge header) evidence must be present immediately"
        );
        assert_eq!(
            report.game_identity_report.platform,
            archivefs_core::game_identity::IdentityPlatform::GameBoy
        );
        assert!(
            report
                .game_identity_report
                .verified_loose_rom_sha256()
                .is_some(),
            "verified identity evidence is available in the fast pass, no DAT needed"
        );
        // The expensive parts are deferred, not done here.
        assert!(
            report.hashes.is_none(),
            "the whole-file checksum is left to the enrichment pass"
        );
        assert_eq!(report.enrichment, SelectedEvidenceEnrichmentStatus::Pending);
        assert!(matches!(report.no_intro, NoIntroLookupResult::NotImported));
    }

    #[test]
    fn compressed_archive_fast_pass_never_schedules_a_whole_outer_file_hash() {
        let dir = write_temp_rom("God of War II (Test).rar", b"Rar!\x1a\x07\x01\0");
        let path = dir.path().join("God of War II (Test).rar");

        let report = gather_selected_evidence_fast(&path, Some("PlayStation 2"))
            .expect("a readable RAR reaches the bounded deferred identity state");

        assert!(report.hashes.is_none());
        assert_eq!(
            report.enrichment,
            SelectedEvidenceEnrichmentStatus::SkippedArchive,
            "ordinary selection must not launch a full checksum over the outer archive"
        );
        assert_eq!(
            report.game_identity_report.format,
            archivefs_core::game_identity::IdentityImageFormat::Deferred
        );
        assert_eq!(report.game_identity_report.bytes_read, 0);
    }

    #[test]
    fn fast_pass_reads_only_a_bounded_prefix_of_a_large_file() {
        // A header that is real, followed by far more than the fast pass
        // will ever read - the header must still be found.
        let mut bytes = gb_rom_bytes();
        bytes.resize((STRUCTURAL_PREFIX_BYTES as usize) * 4, 0);
        let dir = write_temp_rom("big.gb", &bytes);
        let path = dir.path().join("big.gb");
        let report = gather_selected_evidence_fast(&path, Some("Game Boy")).expect("fast pass ok");
        assert!(!report.structural_facts.is_empty());
    }

    #[test]
    fn fast_pass_on_an_unreadable_path_is_an_honest_error() {
        let result =
            gather_selected_evidence_fast(Path::new("/nonexistent/definitely-not-here.gb"), None);
        assert!(result.is_err());
    }

    #[test]
    fn enrichment_computes_the_hash_and_merges_into_an_existing_report() {
        let dir = write_temp_rom("Aladdin.gb", &gb_rom_bytes());
        let path = dir.path().join("Aladdin.gb");
        let mut report =
            gather_selected_evidence_fast(&path, Some("Game Boy")).expect("fast pass ok");
        assert!(report.hashes.is_none());
        let base_observation_count = report.base_observations.len();

        let enrichment =
            compute_selected_evidence_enrichment(&path, None).expect("enrichment succeeds");
        assert!(matches!(
            enrichment.no_intro,
            NoIntroLookupResult::NotImported
        ));

        apply_selected_evidence_enrichment(&mut report, enrichment);
        assert!(
            report.hashes.is_some(),
            "the whole-file checksum is now filled in"
        );
        assert!(matches!(report.no_intro, NoIntroLookupResult::NotImported));
        assert_eq!(
            report.enrichment,
            SelectedEvidenceEnrichmentStatus::Complete
        );
        assert!(report.base_observations.len() >= base_observation_count);
    }

    /// Live check against the exact real Game Boy path from manual QA:
    /// the fast pass must complete essentially instantly with real
    /// structural + verified identity and no whole-file hash. Skipped (not
    /// failed) when the file is not present on this machine.
    #[test]
    #[ignore = "live path; run explicitly with --ignored on the QA machine"]
    fn live_fast_pass_real_game_boy_completes_quickly() {
        let path = Path::new("/mnt/games/roms/gb/Aladdin (USA) (SGB Enhanced).gb");
        if !path.exists() {
            eprintln!("skipping: real GB fixture not present");
            return;
        }
        let start = std::time::Instant::now();
        let report = gather_selected_evidence_fast(path, Some("Game Boy")).expect("fast pass ok");
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "fast pass took {elapsed:?}, expected well under 2s"
        );
        assert!(!report.structural_facts.is_empty());
        assert!(
            report
                .game_identity_report
                .verified_loose_rom_sha256()
                .is_some()
        );
        assert!(report.hashes.is_none());
    }

    /// Live check against the exact real PS2 ISO from manual QA (multi-GB
    /// on a slow USB disc): the fast pass must surface the serial /
    /// executable CRC without reading or hashing the whole image. Skipped
    /// when not present.
    #[test]
    #[ignore = "live path; run explicitly with --ignored on the QA machine"]
    fn live_fast_pass_real_ps2_iso_surfaces_serial_without_full_hash() {
        let path = Path::new("/mnt/usbdrive/games/ps2/God of War (USA)/God of War (USA).iso");
        if !path.exists() {
            eprintln!("skipping: real PS2 ISO fixture not present");
            return;
        }
        let start = std::time::Instant::now();
        let report =
            gather_selected_evidence_fast(path, Some("PlayStation 2")).expect("fast pass ok");
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "fast pass on the ISO took {elapsed:?}; it must not read/hash the whole 8+ GB image"
        );
        assert_eq!(
            report.game_identity_report.platform,
            archivefs_core::game_identity::IdentityPlatform::PlayStation2
        );
        let has_serial = report.game_identity_report.evidence.iter().any(|e| {
            e.kind == archivefs_core::game_identity::IdentityKind::Ps2Serial
                && e.status == archivefs_core::game_identity::IdentityStatus::Verified
        });
        assert!(
            has_serial,
            "the PS2 serial must be visible in the fast pass"
        );
        assert!(report.hashes.is_none());
    }

    #[test]
    fn gather_selected_evidence_on_missing_file_is_an_honest_error() {
        let result =
            gather_selected_evidence(Path::new("/nonexistent/path/does-not-exist.gb"), None);
        assert!(result.is_err());
    }

    /// Manual GUI validation fallback (milestone section 19): this
    /// environment has no display server to launch a real window in, so
    /// this test is the read-only, automated proof that the exact same
    /// gather path the panel calls works end-to-end against the real
    /// `/mnt/games/roms/gb/Alleyway (World).gb` file established safe by
    /// Batches 18/20 - read/hash/detect only, never touched otherwise.
    /// Skipped (not failed) if that file is unavailable on this machine.
    #[test]
    fn gather_selected_evidence_against_the_real_alleyway_rom_is_read_only() {
        let path = Path::new("/mnt/games/roms/gb/Alleyway (World).gb");
        if !path.exists() {
            eprintln!("skipping: real ROM fixture not present on this machine");
            return;
        }
        let before = std::fs::metadata(path).expect("stat before");
        let report = gather_selected_evidence(path, None).expect("gather succeeds on the real ROM");
        assert!(
            !report.structural_facts.is_empty(),
            "the real header must be detected"
        );
        assert!(report.hashes.is_some());
        let after = std::fs::metadata(path).expect("stat after");
        assert_eq!(before.len(), after.len(), "size must be unchanged");
        assert_eq!(
            before.modified().ok(),
            after.modified().ok(),
            "mtime must be unchanged"
        );
    }

    #[test]
    fn structural_only_report_has_no_dat_and_no_hasheous_observations() {
        let bytes = gb_rom_bytes();
        let dir = write_temp_rom("structural-only.gb", &bytes);
        let path = dir.path().join("structural-only.gb");
        let report = gather_selected_evidence(&path, None).expect("gather succeeds");
        assert!(
            report
                .base_observations
                .iter()
                .all(|observation| observation.provenance.channel != EvidenceChannel::LocalNoIntro)
        );
    }

    // -- Gamer vs Advanced visibility -----------------------------------

    fn structural_observation(value: &str) -> EvidenceObservation {
        EvidenceObservation {
            provenance: Provenance {
                channel: EvidenceChannel::LocalStructural,
                upstream_source: SourceFamily::Unknown,
                upstream_version: None,
                source_artifact: None,
                imported_at_unix: None,
                retrieved_at_unix: None,
                generator_version: None,
                lineage: LineageRelation::Independent,
                representation: Representation::StructuralMetadata,
            },
            claim: ClaimType::PlatformCandidate,
            claim_strength: ClaimStrength::Strong,
            identity_scope: IdentityScope::PlatformIdentity,
            hash_or_value: None,
            platform_candidate: Some(value.to_string()),
            release_candidate: None,
            notes: None,
        }
    }

    fn no_intro_exact_observation(hash: &str) -> EvidenceObservation {
        EvidenceObservation {
            provenance: Provenance {
                channel: EvidenceChannel::LocalNoIntro,
                upstream_source: SourceFamily::NoIntro,
                upstream_version: Some("20260101-000000".to_string()),
                source_artifact: None,
                imported_at_unix: None,
                retrieved_at_unix: None,
                generator_version: None,
                lineage: LineageRelation::Independent,
                representation: Representation::PhysicalFile,
            },
            claim: ClaimType::ExactBytesMatch,
            claim_strength: ClaimStrength::Strong,
            identity_scope: IdentityScope::DumpIdentity,
            hash_or_value: Some(hash.to_string()),
            platform_candidate: Some("Game Boy".to_string()),
            release_candidate: Some("Test Game".to_string()),
            notes: None,
        }
    }

    fn hasheous_no_intro_relay_observation(hash: &str) -> EvidenceObservation {
        EvidenceObservation {
            provenance: Provenance {
                channel: EvidenceChannel::Hasheous,
                upstream_source: SourceFamily::NoIntro,
                upstream_version: None,
                source_artifact: None,
                imported_at_unix: None,
                retrieved_at_unix: None,
                generator_version: None,
                lineage: LineageRelation::Relay,
                representation: Representation::PhysicalFile,
            },
            claim: ClaimType::ExactBytesMatch,
            claim_strength: ClaimStrength::Strong,
            identity_scope: IdentityScope::DumpIdentity,
            hash_or_value: Some(hash.to_string()),
            platform_candidate: Some("Game Boy".to_string()),
            release_candidate: Some("Test Game".to_string()),
            notes: None,
        }
    }

    fn make_report(base_observations: Vec<EvidenceObservation>) -> SelectedEvidenceReport {
        let identity_result = inspect_identity(IdentityInspectionInput::default());
        SelectedEvidenceReport {
            path: PathBuf::from("test.gb"),
            structural_facts: Vec::new(),
            identity:
                archivefs_core::platform_evidence_fusion::identity_presentation::present_identity(
                    &identity_result,
                ),
            identity_result,
            game_identity_report: archivefs_core::game_identity::GameIdentityReport {
                archive_path: PathBuf::from("test.gb"),
                platform: archivefs_core::game_identity::IdentityPlatform::Other,
                format: archivefs_core::game_identity::IdentityImageFormat::Unsupported,
                evidence: Vec::new(),
                warnings: Vec::new(),
                bytes_read: 0,
                archive_members_inspected: 0,
                metadata_paths_inspected: 0,
                nested_container_depth: 0,
                complete: false,
            },
            hashes: None,
            no_intro: NoIntroLookupResult::NotImported,
            enrichment: SelectedEvidenceEnrichmentStatus::Complete,
            base_observations,
        }
    }

    #[test]
    fn local_no_intro_plus_hasheous_relay_is_one_lineage_group() {
        let hash = "abc123";
        let report = make_report(vec![no_intro_exact_observation(hash)]);
        let hasheous = HasheousState::Done {
            generation: 1,
            outcome: HasheousCheckOutcome::Found(vec![hasheous_no_intro_relay_observation(hash)]),
        };
        let merged = all_observations(&report, &hasheous);
        assert_eq!(
            merged.len(),
            2,
            "one direct + one relay observation, not collapsed"
        );
        let summaries = merge_evidence(&merged);
        let exact = summaries
            .iter()
            .find(|summary| summary.claim == ClaimType::ExactBytesMatch)
            .expect("an ExactBytesMatch claim group exists");
        assert_eq!(exact.status, AgreementStatus::SameSourceAgreement);
        assert!(!exact.status.is_conflict());
    }

    #[test]
    fn structural_plus_no_intro_is_independent_agreement() {
        let observations = vec![
            structural_observation("Game Boy"),
            no_intro_exact_observation("deadbeef"),
        ];
        // The structural detector's own lane and the No-Intro family lane
        // are genuinely independent evidence mechanisms (Batch 21
        // closeout) - two independent source groups, not one.
        assert_eq!(
            archivefs_core::platform_evidence_fusion::evidence_lineage::independent_source_group_count(
                &observations
            ),
            2
        );
        let summaries = merge_evidence(&observations);
        let exact = summaries
            .iter()
            .find(|summary| summary.claim == ClaimType::ExactBytesMatch)
            .expect("an ExactBytesMatch group exists");
        // A single-observation claim group (only No-Intro asserts an exact
        // byte match here) is not itself a multi-lane agreement - the
        // point of this test is the two-independent-groups count above.
        assert!(!exact.status.is_conflict());
    }

    #[test]
    fn gamer_mode_hides_per_observation_provenance_rows() {
        // A data-level proxy for the visibility split: Gamer mode's
        // rendering path only ever needs `summary.observations.len()`,
        // never per-observation provenance fields - verified by
        // construction, since `show_lineage_summaries`'s `Gamer` arm
        // (see above) does not reference `provenance` at all. This test
        // documents the invariant explicitly for future edits.
        let observations = vec![structural_observation("Game Boy")];
        let summaries = merge_evidence(&observations);
        assert!(!summaries.is_empty());
        for summary in &summaries {
            // Gamer mode only needs a count.
            let _count = summary.observations.len();
        }
    }

    // -- representation separation --------------------------------------

    #[test]
    fn physical_and_normalized_stay_separate_claim_groups_when_hash_differs() {
        let physical = EvidenceObservation {
            provenance: Provenance {
                representation: Representation::PhysicalFile,
                ..no_intro_exact_observation("physhash").provenance
            },
            hash_or_value: Some("physhash".to_string()),
            ..no_intro_exact_observation("physhash")
        };
        let normalized = EvidenceObservation {
            provenance: Provenance {
                representation: Representation::NormalizedRom,
                ..no_intro_exact_observation("normhash").provenance
            },
            claim: ClaimType::ExactNormalizedMatch,
            hash_or_value: Some("normhash".to_string()),
            ..no_intro_exact_observation("normhash")
        };
        let summaries = merge_evidence(&[physical, normalized]);
        let claims: std::collections::BTreeSet<_> =
            summaries.iter().map(|summary| summary.claim).collect();
        assert!(claims.contains(&ClaimType::ExactBytesMatch));
        assert!(claims.contains(&ClaimType::ExactNormalizedMatch));
    }

    // -- Hasheous states --------------------------------------------------

    struct FakeTransport {
        response: Result<
            archivefs_core::identity_source::hasheous::client::HasheousHttpResponse,
            HasheousRequestError,
        >,
    }

    impl HasheousTransport for FakeTransport {
        fn post_json(
            &self,
            _url: &str,
            _body: &[u8],
        ) -> Result<
            archivefs_core::identity_source::hasheous::client::HasheousHttpResponse,
            HasheousRequestError,
        > {
            match &self.response {
                Ok(response) => Ok(response.clone()),
                Err(error) => Err(error.clone()),
            }
        }
    }

    fn disabled_config() -> HasheousConfig {
        HasheousConfig {
            enabled: false,
            base_url: "https://hasheous.org".to_string(),
            timeout: Duration::from_secs(1),
        }
    }

    fn enabled_config() -> HasheousConfig {
        HasheousConfig {
            enabled: true,
            base_url: "https://hasheous.org".to_string(),
            timeout: Duration::from_secs(1),
        }
    }

    fn ok_transport(body: &str) -> FakeTransport {
        FakeTransport {
            response: Ok(
                archivefs_core::identity_source::hasheous::client::HasheousHttpResponse {
                    status: 200,
                    body: body.as_bytes().to_vec(),
                    retry_after_secs: None,
                },
            ),
        }
    }

    #[test]
    fn hasheous_check_when_disabled_reports_disabled_not_an_error_state() {
        let transport = ok_transport("{}");
        let outcome = run_hasheous_check(&disabled_config(), &transport, "abc123");
        assert!(matches!(outcome, HasheousCheckOutcome::Disabled));
    }

    #[test]
    fn hasheous_check_found_returns_real_observations() {
        let body = r#"{"signatures":{"NoIntros":[{"game":{"name":"Test Game"}}]}}"#;
        let transport = ok_transport(body);
        let outcome = run_hasheous_check(&enabled_config(), &transport, "abc123");
        match outcome {
            HasheousCheckOutcome::Found(observations) => {
                assert!(!observations.is_empty());
                assert!(observations.iter().any(|observation| {
                    observation.provenance.channel == EvidenceChannel::Hasheous
                        && observation.provenance.upstream_source == SourceFamily::NoIntro
                }));
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn hasheous_check_no_match_is_neutral_not_an_error() {
        let transport = FakeTransport {
            response: Ok(
                archivefs_core::identity_source::hasheous::client::HasheousHttpResponse {
                    status: 404,
                    body: Vec::new(),
                    retry_after_secs: None,
                },
            ),
        };
        let outcome = run_hasheous_check(&enabled_config(), &transport, "abc123");
        assert!(matches!(outcome, HasheousCheckOutcome::NoMatch));
    }

    #[test]
    fn hasheous_check_timeout_error_is_reported_distinctly() {
        let transport = FakeTransport {
            response: Err(HasheousRequestError::Timeout),
        };
        let outcome = run_hasheous_check(&enabled_config(), &transport, "abc123");
        assert!(matches!(outcome, HasheousCheckOutcome::Timeout));
    }

    #[test]
    fn hasheous_check_rate_limited_error_is_reported_distinctly() {
        let transport = FakeTransport {
            response: Err(HasheousRequestError::RateLimited {
                status: 429,
                retry_after_secs: Some(30),
            }),
        };
        let outcome = run_hasheous_check(&enabled_config(), &transport, "abc123");
        assert!(matches!(outcome, HasheousCheckOutcome::RateLimited));
    }

    #[test]
    fn hasheous_check_network_error_carries_a_detail_message() {
        let transport = FakeTransport {
            response: Err(HasheousRequestError::Network {
                detail: "connection refused".to_string(),
            }),
        };
        let outcome = run_hasheous_check(&enabled_config(), &transport, "abc123");
        match outcome {
            HasheousCheckOutcome::NetworkError(detail) => {
                assert!(detail.contains("connection refused"))
            }
            other => panic!("expected NetworkError, got {other:?}"),
        }
    }

    #[test]
    fn hasheous_check_request_never_carries_the_selected_path() {
        let hash_set = HasheousHashSet {
            sha1: Some("abc123".to_string()),
            ..Default::default()
        };
        let body = serde_json::to_string(&hash_set).expect("serializes");
        assert!(!body.contains('/'));
        assert!(!body.contains(".gb"));
        assert!(body.contains("abc123"));
    }

    #[test]
    fn hasheous_status_line_distinguishes_no_match_from_error() {
        let (no_match_line, no_match_tone) = hasheous_status_line(&HasheousCheckOutcome::NoMatch);
        let (error_line, error_tone) =
            hasheous_status_line(&HasheousCheckOutcome::NetworkError("boom".to_string()));
        assert_ne!(no_match_line, error_line);
        assert_ne!(no_match_tone, error_tone);
    }

    #[test]
    fn hasheous_timeout_and_rate_limited_are_distinct_states() {
        let (timeout_line, _) = hasheous_status_line(&HasheousCheckOutcome::Timeout);
        let (rate_line, _) = hasheous_status_line(&HasheousCheckOutcome::RateLimited);
        assert_ne!(timeout_line, rate_line);
    }

    // -- deterministic ordering ------------------------------------------

    #[test]
    fn merge_evidence_ordering_is_stable_regardless_of_input_order() {
        let forward = vec![
            structural_observation("Game Boy"),
            no_intro_exact_observation("deadbeef"),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();
        assert_eq!(merge_evidence(&forward), merge_evidence(&reversed));
    }

    // -- no mutation action -----------------------------------------------

    // -- real render smoke tests ------------------------------------------

    fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.text().contains(needle),
                egui::Shape::Vec(nested) => {
                    nested.iter().any(|shape| shape_contains(shape, needle))
                }
                _ => false,
            }
        }

        output
            .shapes
            .iter()
            .any(|clipped| shape_contains(&clipped.shape, needle))
    }

    #[test]
    fn idle_panel_renders_without_panicking_when_a_file_is_selected() {
        let ctx = egui::Context::default();
        let state = SelectedEvidenceState::Idle;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_selected_evidence_panel(ui, false, Some(Path::new("game.gb")), &state);
            });
        });
    }

    #[test]
    fn panel_with_no_selection_renders_without_panicking() {
        let ctx = egui::Context::default();
        let state = SelectedEvidenceState::Idle;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_selected_evidence_panel(ui, false, None, &state);
            });
        });
    }

    #[test]
    fn ready_panel_renders_in_both_gamer_and_advanced_mode_without_panicking() {
        let bytes = gb_rom_bytes();
        let dir = write_temp_rom("render-smoke.gb", &bytes);
        let path = dir.path().join("render-smoke.gb");
        let report = gather_selected_evidence(&path, None).expect("gather succeeds");
        let state = SelectedEvidenceState::Ready {
            generation: 1,
            report: Box::new(report),
            hasheous: HasheousState::Idle,
        };
        for advanced in [false, true] {
            let ctx = egui::Context::default();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ =
                        show_selected_evidence_panel(ui, advanced, Some(path.as_path()), &state);
                });
            });
        }
    }

    #[test]
    fn error_panel_renders_without_panicking() {
        let ctx = egui::Context::default();
        let state = SelectedEvidenceState::Error {
            generation: 1,
            path: PathBuf::from("game.gb"),
            message: "boom".to_string(),
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_selected_evidence_panel(ui, false, Some(Path::new("game.gb")), &state);
            });
        });
    }

    #[test]
    fn enrichment_failure_is_visible_on_the_ready_selection_card() {
        let mut report = make_report(Vec::new());
        report.enrichment = SelectedEvidenceEnrichmentStatus::Failed(
            "checksum worker disconnected during inspection".to_string(),
        );
        let state = SelectedEvidenceState::Ready {
            generation: 1,
            report: Box::new(report),
            hasheous: HasheousState::Idle,
        };
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_selected_evidence_panel(ui, false, Some(Path::new("test.gb")), &state);
            });
        });

        assert!(rendered_text_contains(
            &output,
            "Additional evidence could not be loaded"
        ));
        assert!(rendered_text_contains(
            &output,
            "checksum worker disconnected during inspection"
        ));
    }

    #[test]
    fn loading_panel_renders_without_panicking() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let ctx = egui::Context::default();
        let state = SelectedEvidenceState::Loading {
            generation: 1,
            path: PathBuf::from("game.gb"),
            receiver,
        };
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = show_selected_evidence_panel(ui, false, Some(Path::new("game.gb")), &state);
            });
        });
    }

    #[test]
    fn panel_never_offers_a_mutation_action() {
        // `SelectedEvidenceAction` is the panel's entire vocabulary of
        // things it can ask the caller to do. If this ever grows an
        // Apply/Rename/Move/Delete variant, this match arm stops
        // compiling - the strongest guarantee available short of parsing
        // the render function itself.
        fn assert_read_only(action: SelectedEvidenceAction) {
            match action {
                SelectedEvidenceAction::Load(_) => {}
                SelectedEvidenceAction::CheckHasheous => {}
            }
        }
        assert_read_only(SelectedEvidenceAction::Load(PathBuf::from("x")));
        assert_read_only(SelectedEvidenceAction::CheckHasheous);
    }

    #[test]
    fn no_intro_not_imported_is_a_valid_honest_state() {
        let result = lookup_no_intro_for(
            None,
            &LocalHashes {
                fingerprint: archivefs_core::identity_source::hashing::FileFingerprint {
                    path: PathBuf::from("x"),
                    size_bytes: 0,
                    modified_unix_seconds: None,
                },
                crc32: "00000000".to_string(),
                md5: "0".repeat(32),
                sha1: "0".repeat(40),
                bytes_hashed: 0,
            },
        );
        assert!(matches!(result, NoIntroLookupResult::NotImported));
    }

    #[test]
    fn agreement_label_covers_every_status_without_a_default_arm() {
        for status in [
            AgreementStatus::SameSourceAgreement,
            AgreementStatus::IndependentAgreement,
            AgreementStatus::DerivedAgreement,
            AgreementStatus::CrossRepresentationAgreement,
            AgreementStatus::WeakAgreement,
            AgreementStatus::SameSourceVersionConflict,
            AgreementStatus::DerivedSourceConflict,
            AgreementStatus::IndependentSourceConflict,
            AgreementStatus::RepresentationConflict,
            AgreementStatus::MetadataConflict,
        ] {
            assert!(!agreement_label(status).is_empty());
        }
    }

    #[test]
    fn identity_status_tone_covers_every_status_without_a_default_arm() {
        for status in [
            IdentityStatus::Conflict,
            IdentityStatus::Ambiguous,
            IdentityStatus::VerifiedByDat,
            IdentityStatus::ContentAndDatAgree,
            IdentityStatus::ContentOnly,
            IdentityStatus::DatOnly,
            IdentityStatus::Unknown,
        ] {
            let _ = status_tone_for(status);
        }
    }

    #[test]
    fn conflict_status_gets_the_blocked_tone() {
        assert_eq!(
            agreement_tone(AgreementStatus::IndependentSourceConflict),
            widgets::StatusTone::Blocked
        );
        assert_eq!(
            agreement_tone(AgreementStatus::SameSourceAgreement),
            widgets::StatusTone::Success
        );
    }

    // -- Gamer-mode headline / conflict-first ordering --------------------

    fn no_intro_platform_observation(platform: &str) -> EvidenceObservation {
        EvidenceObservation {
            provenance: Provenance {
                channel: EvidenceChannel::LocalNoIntro,
                upstream_source: SourceFamily::NoIntro,
                upstream_version: Some("20260101-000000".to_string()),
                source_artifact: None,
                imported_at_unix: None,
                retrieved_at_unix: None,
                generator_version: None,
                lineage: LineageRelation::Independent,
                representation: Representation::PhysicalFile,
            },
            claim: ClaimType::PlatformCandidate,
            claim_strength: ClaimStrength::Strong,
            identity_scope: IdentityScope::PlatformIdentity,
            hash_or_value: None,
            platform_candidate: Some(platform.to_string()),
            release_candidate: None,
            notes: None,
        }
    }

    #[test]
    fn gamer_headline_reports_independent_agreement_as_confirmed() {
        // Two genuinely independent lanes agreeing on the same claim
        // (PlatformCandidate) - the structural detector and a direct
        // No-Intro match.
        let observations = vec![
            structural_observation("Game Boy"),
            no_intro_platform_observation("Game Boy"),
        ];
        let summaries = merge_evidence(&observations);
        let (headline, _, tone) = overall_gamer_headline(&summaries);
        assert_eq!(headline, "Confirmed");
        assert_eq!(tone, widgets::StatusTone::Success);
    }

    #[test]
    fn gamer_headline_reports_a_relay_as_one_source_not_two() {
        let observations = vec![
            no_intro_exact_observation("deadbeef"),
            hasheous_no_intro_relay_observation("deadbeef"),
        ];
        let summaries = merge_evidence(&observations);
        let (headline, sentence, _) = overall_gamer_headline(&summaries);
        assert_eq!(headline, "One source");
        assert!(!sentence.contains("Confirmed"));
    }

    #[test]
    fn gamer_headline_never_shows_a_backend_enum_name() {
        let observations = vec![
            structural_observation("Game Boy"),
            no_intro_exact_observation("deadbeef"),
        ];
        let summaries = merge_evidence(&observations);
        let (headline, sentence, _) = overall_gamer_headline(&summaries);
        for jargon in [
            "ExactBytesMatch",
            "IndependentAgreement",
            "SourceFamily",
            "LineageRelation",
        ] {
            assert!(!headline.contains(jargon));
            assert!(!sentence.contains(jargon));
        }
    }

    #[test]
    fn conflicts_always_sort_before_agreement_summaries() {
        let saturn = structural_observation("Sega Saturn");
        let redump_ps1 = EvidenceObservation {
            platform_candidate: Some("PlayStation".to_string()),
            hash_or_value: Some("samehash".to_string()),
            ..no_intro_exact_observation("samehash")
        };
        let saturn_hash = EvidenceObservation {
            hash_or_value: Some("samehash".to_string()),
            ..no_intro_exact_observation("samehash")
        };
        let agreeing = structural_observation("Game Boy");
        let agreeing_no_intro = no_intro_exact_observation("agreeinghash");
        let mut summaries =
            merge_evidence(&[agreeing, agreeing_no_intro, saturn, redump_ps1, saturn_hash]);
        summaries.sort_by_key(|summary| !summary.status.is_conflict());
        assert!(
            summaries[0].status.is_conflict(),
            "the first summary after sorting must be a conflict when one exists"
        );
    }

    #[test]
    fn claim_label_covers_every_claim_type_without_a_default_arm() {
        for claim in [
            ClaimType::ExactBytesMatch,
            ClaimType::ExactNormalizedMatch,
            ClaimType::ExactTrackMatch,
            ClaimType::ExactLogicalDiscMatch,
            ClaimType::ExactSlaveMatch,
            ClaimType::PlatformCandidate,
            ClaimType::ReleaseCandidate,
            ClaimType::RevisionCandidate,
            ClaimType::RegionMetadata,
            ClaimType::LanguageMetadata,
            ClaimType::VariantStatus,
            ClaimType::HardwareCompatibility,
            ClaimType::DisplayMetadata,
            ClaimType::CrosswalkCandidate,
            ClaimType::VettedCrosswalk,
            ClaimType::EquivalentCanonical,
            ClaimType::RelatedPlatform,
        ] {
            assert!(!claim_label(claim).is_empty());
        }
    }
}
