//! One typed, GUI-ready summary of what a DAT catalogue says about a single
//! selected library item.
//!
//! This is a pure projection over models EmuWiz already produces - it runs
//! no matching of its own, opens no file, and reads no database. The GUI
//! passes the [`DatAuditOutcome`] it already holds for a completed audit
//! plus the verdict recorded for the selected item, and gets back one
//! [`LibraryDatIdentitySummary`] with every field a "DAT identity" panel
//! needs, so no raw SQL and no re-run of DAT matching is required from UI
//! code.
//!
//! # Deliberately not persisted (yet)
//!
//! There is currently no table that stores a per-item DAT identity or an
//! Arcade [`SetResolution`], so this module derives everything from the
//! transient audit outcome it is given. When such persistence lands (see
//! [`crate::dat::set::SetIdentity`]'s own note), a thin `Database` query can
//! populate the same [`LibraryDatIdentitySummary`] shape without changing
//! this model. Until then [`DatSetDependencySummary::Pending`] reports that
//! state honestly rather than inventing one.
//!
//! # Never filename-derived verification
//!
//! A filename-only match maps to
//! [`DatVerificationState::FilenameOnlyNotVerified`] and
//! [`LibraryDatIdentitySummary::is_verified`] stays `false`. Only a
//! single cryptographic-hash match is `is_verified`.

use serde::{Deserialize, Serialize};

use crate::dat::audit::AuditVerdict;
use crate::dat::dependency::DependencyState;
use crate::dat::index::DatRomRef;
use crate::dat::model::DatEcosystem;
use crate::dat::set::{SetResolution, SetState};
use crate::dat::sources::audit_run::DatAuditOutcome;

/// The item hashes EmuWiz already holds for a library item. Mirrors the hash
/// fields of [`crate::dat::audit::KnownFileEvidence`]; nothing here hashes a
/// file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryItemHashes {
    pub size_bytes: Option<u64>,
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
}

impl LibraryItemHashes {
    /// The names of every algorithm this snapshot actually carries, strongest
    /// first.
    fn available_algorithms(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.sha256.is_some() {
            names.push("SHA-256");
        }
        if self.sha1.is_some() {
            names.push("SHA-1");
        }
        if self.md5.is_some() {
            names.push("MD5");
        }
        if self.crc32.is_some() {
            names.push("CRC32");
        }
        names
    }

    fn value_for(&self, algorithm: &str) -> Option<String> {
        let raw = match algorithm {
            "SHA-256" => self.sha256.as_deref(),
            "SHA-1" => self.sha1.as_deref(),
            "MD5" => self.md5.as_deref(),
            "CRC32" | "CRC32+size" => self.crc32.as_deref(),
            _ => None,
        }?;
        Some(raw.trim().to_ascii_lowercase())
    }
}

/// The inputs for [`summarize_library_dat_identity`]. Every field is a fact
/// EmuWiz already produced elsewhere.
#[derive(Debug, Clone, Copy)]
pub struct LibraryDatIdentityQuery<'a> {
    /// The provenance-carrying audit outcome the selected item was matched
    /// in.
    pub outcome: &'a DatAuditOutcome,
    /// The verdict the audit recorded for this item - from
    /// `outcome.report.entries[i].verdict`, or from an archive member's
    /// `verdict`.
    pub verdict: &'a AuditVerdict,
    /// The matched DAT ROM references the audit retained, when it did.
    /// Archive-member audits carry these; flat physical-file audits leave
    /// this empty and the summary falls back to the verdict's own names.
    pub matched_refs: &'a [DatRomRef],
    /// The item hashes the audit actually compared.
    pub audited_hashes: &'a LibraryItemHashes,
    /// The item's hashes as the library currently knows them (latest scan).
    /// `None` makes [`DatProvenanceFreshness`] `Unknown`.
    pub current_hashes: Option<&'a LibraryItemHashes>,
}

/// Where a library item stands against the DAT it was audited with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DatVerificationState {
    /// Exactly one DAT entry matched on a cryptographic hash.
    VerifiedSingleMatch { algorithm: String },
    /// CRC32 (with size where known) matched exactly one entry - probable,
    /// never proven.
    Probable,
    /// A hash matched more than one DAT entry; identity cannot be settled.
    AmbiguousMultipleCandidates {
        algorithm: String,
        candidate_count: usize,
    },
    /// Candidates existed but the available evidence conflicts.
    Conflicting { detail: String },
    /// Every comparable hash found no DAT entry.
    NoMatch,
    /// Only the filename matched - explicitly not verification.
    FilenameOnlyNotVerified,
    /// No hash was available to compare and the filename matched nothing.
    NoUsableEvidence,
}

impl DatVerificationState {
    fn from_verdict(verdict: &AuditVerdict) -> Self {
        match verdict {
            AuditVerdict::Exact { algorithm, .. } => Self::VerifiedSingleMatch {
                algorithm: (*algorithm).to_string(),
            },
            AuditVerdict::ExactMultipleCandidates {
                algorithm, count, ..
            } => Self::AmbiguousMultipleCandidates {
                algorithm: (*algorithm).to_string(),
                candidate_count: *count,
            },
            AuditVerdict::Probable { .. } => Self::Probable,
            AuditVerdict::ProbableMultipleCandidates {
                algorithm, count, ..
            } => Self::AmbiguousMultipleCandidates {
                algorithm: (*algorithm).to_string(),
                candidate_count: *count,
            },
            AuditVerdict::FilenameOnly { .. } => Self::FilenameOnlyNotVerified,
            AuditVerdict::Ambiguous { detail } => Self::Conflicting {
                detail: detail.clone(),
            },
            AuditVerdict::NotInDat => Self::NoMatch,
            AuditVerdict::NoUsableEvidence => Self::NoUsableEvidence,
        }
    }
}

/// The DAT source and catalogue snapshot this summary came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatSourceProvenance {
    /// The configured source's stable id.
    pub source_id: String,
    /// The configured source's display name.
    pub source_name: String,
    /// The DAT publisher family, already classified at parse time. `None`
    /// for a combined multi-source audit.
    pub ecosystem: Option<DatEcosystem>,
    /// The catalogue `<version>` header - the closest thing most publishers
    /// carry to a source revision. `None` when the DAT had none or for a
    /// combined audit.
    pub source_revision: Option<String>,
    /// The `<author>` header, when present.
    pub author: Option<String>,
    /// Every catalogue `<name>` header this audit actually read.
    pub catalogue_names: Vec<String>,
    /// The catalogue path the audit read from - provenance only.
    pub dat_path: String,
}

impl DatSourceProvenance {
    fn from_outcome(outcome: &DatAuditOutcome) -> Self {
        Self {
            source_id: outcome.source_id.clone(),
            source_name: outcome.source_display_name.clone(),
            ecosystem: outcome.catalogue_ecosystem,
            source_revision: outcome.catalogue_version.clone(),
            author: outcome.catalogue_author.clone(),
            catalogue_names: outcome.catalogue_names.clone(),
            dat_path: outcome.dat_path.clone(),
        }
    }
}

/// The canonical identity the DAT entry declares for a matched item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatCanonicalIdentity {
    /// The DAT `<game>` / `<machine>` name. `None` when nothing matched or
    /// the match was ambiguous between several names.
    pub canonical_dat_name: Option<String>,
    /// The DAT `<rom>` name for the matched member.
    pub canonical_rom_name: Option<String>,
    /// Region, when the DAT entry itself declares one (a `region=` attribute)
    /// or an unambiguous parenthesised region token is present in the entry
    /// name. `None` otherwise - never inferred from the file on disk.
    pub region: Option<String>,
    /// Revision, when the entry name carries an explicit `(Rev ...)` /
    /// `(v1.1)` token. `None` otherwise.
    pub revision: Option<String>,
}

/// Which hash confirmed the match, and its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatHashEvidenceSummary {
    /// e.g. `"SHA-1"`, `"MD5"`, `"CRC32"`, `"CRC32+size"`. `None` for a
    /// filename-only match, a no-match, or no usable evidence.
    pub matched_algorithm: Option<String>,
    /// The item's own hash value for [`Self::matched_algorithm`], lower-case
    /// hex. `None` when that value is not among the item's known hashes.
    pub matched_value: Option<String>,
    /// Every hash algorithm EmuWiz already holds for this item.
    pub available_algorithms: Vec<String>,
}

/// Whether the audit this summary rests on still describes the item as the
/// library currently knows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatProvenanceFreshness {
    /// The strongest hash the audit compared equals the item's current one.
    Current,
    /// The item's current hashes differ from those the audit compared - this
    /// summary describes a previous state of the file.
    Stale,
    /// Not enough overlapping hash evidence to tell current from stale.
    Unknown,
}

fn freshness(
    audited: &LibraryItemHashes,
    current: Option<&LibraryItemHashes>,
) -> DatProvenanceFreshness {
    let Some(current) = current else {
        return DatProvenanceFreshness::Unknown;
    };
    let compare = |a: &Option<String>, c: &Option<String>| match (a, c) {
        (Some(a), Some(c)) => Some(a.trim().eq_ignore_ascii_case(c.trim())),
        _ => None,
    };
    for verdict in [
        compare(&audited.sha256, &current.sha256),
        compare(&audited.sha1, &current.sha1),
        compare(&audited.md5, &current.md5),
        compare(&audited.crc32, &current.crc32),
    ]
    .into_iter()
    .flatten()
    {
        return if verdict {
            DatProvenanceFreshness::Current
        } else {
            DatProvenanceFreshness::Stale
        };
    }
    match (audited.size_bytes, current.size_bytes) {
        (Some(a), Some(c)) if a == c => DatProvenanceFreshness::Current,
        (Some(_), Some(_)) => DatProvenanceFreshness::Stale,
        _ => DatProvenanceFreshness::Unknown,
    }
}

/// A compact rollup of Arcade set / dependency completeness for the matched
/// entry, when the audit produced one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DatSetDependencySummary {
    /// No set resolution is available for this item. `reason` says why.
    Pending { reason: String },
    /// A single set resolution was found for the matched entry.
    Resolved {
        set_name: String,
        source_id: String,
        state: SetState,
        members_required: usize,
        members_verified: usize,
        members_missing: usize,
        members_bad: usize,
        members_borrowed: usize,
        disks_required: usize,
        disks_verified: usize,
        dependency_state: DependencyState,
        dependency_requirements: usize,
    },
}

fn set_dependency_summary(
    outcome: &DatAuditOutcome,
    canonical_name: Option<&str>,
) -> DatSetDependencySummary {
    if outcome.sets.is_empty() {
        return DatSetDependencySummary::Pending {
            reason: "this audit computed no catalogue set resolutions (DAT set data is not \
                     persisted on this build)"
                .to_string(),
        };
    }
    let Some(name) = canonical_name else {
        return DatSetDependencySummary::Pending {
            reason: "no single canonical DAT entry to attribute a set resolution to".to_string(),
        };
    };
    let matches: Vec<&SetResolution> = outcome
        .sets
        .iter()
        .filter(|resolution| resolution.identity.game_name == name)
        .collect();
    match matches.as_slice() {
        [] => DatSetDependencySummary::Pending {
            reason: "the matched entry is not part of a multi-member catalogue set in this DAT"
                .to_string(),
        },
        [resolution] => DatSetDependencySummary::Resolved {
            set_name: resolution.identity.game_name.clone(),
            source_id: resolution.identity.source_id.clone(),
            state: resolution.state.clone(),
            members_required: resolution.members_required.len(),
            members_verified: resolution.members_verified.len(),
            members_missing: resolution
                .members_required
                .len()
                .saturating_sub(resolution.members_verified.len()),
            members_bad: resolution.members_bad.len(),
            members_borrowed: resolution.members_borrowed.len(),
            disks_required: resolution.disks_required.len(),
            disks_verified: resolution.disks_verified.len(),
            dependency_state: resolution.dependencies.state,
            dependency_requirements: resolution.dependencies.requirements.len(),
        },
        _ => DatSetDependencySummary::Pending {
            reason: format!(
                "the DAT declares {} sets named {name:?}; identity cannot be attributed to one",
                matches.len()
            ),
        },
    }
}

/// One typed, GUI-ready DAT identity summary for a selected library item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryDatIdentitySummary {
    pub verification_state: DatVerificationState,
    pub source: DatSourceProvenance,
    pub canonical: DatCanonicalIdentity,
    pub hash_evidence: DatHashEvidenceSummary,
    pub provenance_freshness: DatProvenanceFreshness,
    /// The competing DAT entry names for an ambiguous / conflicting match,
    /// so a person can see what the audit could not decide between. Empty
    /// for every other state.
    pub ambiguous_candidates: Vec<String>,
    pub set_dependency: DatSetDependencySummary,
}

impl LibraryDatIdentitySummary {
    /// `true` only for a single cryptographic-hash match. A CRC32 probable
    /// match, a filename-only match, and every ambiguous / conflicting /
    /// no-match state are all `false`.
    pub fn is_verified(&self) -> bool {
        matches!(
            self.verification_state,
            DatVerificationState::VerifiedSingleMatch { .. }
        )
    }

    /// `true` when the match resolved to more than one candidate or the
    /// evidence conflicts.
    pub fn is_ambiguous(&self) -> bool {
        matches!(
            self.verification_state,
            DatVerificationState::AmbiguousMultipleCandidates { .. }
                | DatVerificationState::Conflicting { .. }
        )
    }

    /// `true` when nothing in the DAT matched this item.
    pub fn is_no_match(&self) -> bool {
        matches!(
            self.verification_state,
            DatVerificationState::NoMatch | DatVerificationState::NoUsableEvidence
        )
    }
}

/// Names carried by a verdict: `(game_name, rom_name)` when it identifies one
/// entry, or the competing game names when it does not.
fn verdict_names(verdict: &AuditVerdict) -> (Option<String>, Option<String>, Vec<String>) {
    match verdict {
        AuditVerdict::Exact {
            game_name,
            rom_name,
            ..
        }
        | AuditVerdict::Probable {
            game_name,
            rom_name,
        }
        | AuditVerdict::FilenameOnly {
            game_name,
            rom_name,
        } => (Some(game_name.clone()), Some(rom_name.clone()), Vec::new()),
        AuditVerdict::ExactMultipleCandidates { game_names, .. }
        | AuditVerdict::ProbableMultipleCandidates { game_names, .. } => {
            (None, None, game_names.clone())
        }
        AuditVerdict::Ambiguous { .. }
        | AuditVerdict::NotInDat
        | AuditVerdict::NoUsableEvidence => (None, None, Vec::new()),
    }
}

/// Conservative, post-hash-match only region/revision extraction from a DAT
/// entry name. Reads only the entry the DAT already identified; never a
/// filename on disk. `None` whenever a token is not unambiguous.
fn region_revision_from_name(name: &str) -> (Option<String>, Option<String>) {
    const REGIONS: &[&str] = &[
        "usa",
        "europe",
        "japan",
        "world",
        "asia",
        "australia",
        "brazil",
        "canada",
        "china",
        "france",
        "germany",
        "italy",
        "korea",
        "netherlands",
        "spain",
        "sweden",
        "taiwan",
        "uk",
        "russia",
        "hong kong",
        "scandinavia",
        "latin america",
    ];
    let mut region = None;
    let mut revision = None;
    for token in name
        .split('(')
        .skip(1)
        .filter_map(|part| part.split_once(')').map(|(inner, _)| inner.trim()))
    {
        let lower = token.to_ascii_lowercase();
        if region.is_none() {
            let parts: Vec<&str> = token.split(',').map(str::trim).collect();
            if !parts.is_empty()
                && parts
                    .iter()
                    .all(|part| REGIONS.contains(&part.to_ascii_lowercase().as_str()))
            {
                region = Some(parts.join(", "));
                continue;
            }
        }
        if revision.is_none()
            && (lower.starts_with("rev ")
                || lower.starts_with("rev.")
                || lower == "rev"
                || lower.starts_with("revision")
                || (lower.starts_with('v') && lower[1..].starts_with(|c: char| c.is_ascii_digit())))
        {
            revision = Some(token.to_string());
        }
    }
    (region, revision)
}

/// Builds the [`LibraryDatIdentitySummary`] for one selected library item
/// from an audit outcome and the verdict recorded for that item.
///
/// Pure: no I/O, no matching, no database access. `query.matched_refs` is
/// used for richer region evidence when present; otherwise the verdict's own
/// entry name is the only source and region/revision come from a
/// conservative name scan.
pub fn summarize_library_dat_identity(
    query: &LibraryDatIdentityQuery<'_>,
) -> LibraryDatIdentitySummary {
    let verification_state = DatVerificationState::from_verdict(query.verdict);
    let (game_name, rom_name, ambiguous_candidates) = verdict_names(query.verdict);

    // Region from the DAT ROM ref's own `region=` metadata, if the audit
    // retained one for this exact entry; otherwise a conservative name scan.
    let dat_region = query.matched_refs.iter().find_map(|reference| {
        reference
            .original_metadata
            .fields
            .get("region")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    let (name_region, name_revision) = game_name
        .as_deref()
        .map(region_revision_from_name)
        .unwrap_or((None, None));

    let canonical = DatCanonicalIdentity {
        canonical_dat_name: game_name.clone(),
        canonical_rom_name: rom_name,
        region: dat_region.or(name_region),
        revision: name_revision,
    };

    let matched_algorithm = match query.verdict {
        AuditVerdict::Exact { algorithm, .. }
        | AuditVerdict::ExactMultipleCandidates { algorithm, .. }
        | AuditVerdict::ProbableMultipleCandidates { algorithm, .. } => {
            Some((*algorithm).to_string())
        }
        AuditVerdict::Probable { .. } => Some(
            if query.audited_hashes.crc32.is_some() && query.audited_hashes.size_bytes.is_some() {
                "CRC32+size".to_string()
            } else {
                "CRC32".to_string()
            },
        ),
        AuditVerdict::FilenameOnly { .. }
        | AuditVerdict::Ambiguous { .. }
        | AuditVerdict::NotInDat
        | AuditVerdict::NoUsableEvidence => None,
    };
    let matched_value = matched_algorithm
        .as_deref()
        .and_then(|algorithm| query.audited_hashes.value_for(algorithm));

    let hash_evidence = DatHashEvidenceSummary {
        matched_algorithm,
        matched_value,
        available_algorithms: query
            .audited_hashes
            .available_algorithms()
            .into_iter()
            .map(str::to_string)
            .collect(),
    };

    LibraryDatIdentitySummary {
        verification_state,
        source: DatSourceProvenance::from_outcome(query.outcome),
        canonical,
        hash_evidence,
        provenance_freshness: freshness(query.audited_hashes, query.current_hashes),
        ambiguous_candidates,
        set_dependency: set_dependency_summary(query.outcome, game_name.as_deref()),
    }
}

// ---------------------------------------------------------------------------
// Durable persistence bridge
// ---------------------------------------------------------------------------

/// Whether a DAT audit run examined everything it was asked to. Only an
/// [`DatAuditCompleteness::Exhaustive`] run may overwrite a prior stored
/// result with a *negative* verdict; a [`DatAuditCompleteness::Partial`]
/// run (cancelled, truncated, or otherwise incomplete) may only add or
/// improve, never destroy a prior name-bearing result with a false
/// no-match/ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatAuditCompleteness {
    Exhaustive,
    Partial,
}

/// The smallest set of already-produced facts needed to reconstruct a
/// [`LibraryDatIdentitySummary`] for one library item + one DAT source,
/// later, without re-running an audit, reopening a DAT file, or rehashing
/// anything.
///
/// Everything a person would see is snapshotted here (source display name,
/// catalogue names, canonical entry name, matched hash) so reconstruction
/// never depends on an external DAT file that may since have been deleted.
/// [`Self::source`]`.source_id` is still carried as the stable reference key
/// for source scoping and revision-drift detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedLibraryDatIdentity {
    pub verification_state: DatVerificationState,
    pub source: DatSourceProvenance,
    pub canonical: DatCanonicalIdentity,
    pub hash_evidence: DatHashEvidenceSummary,
    /// Competing DAT entry names for an ambiguous / conflicting match,
    /// preserved verbatim - never collapsed into one winner.
    pub ambiguous_candidates: Vec<String>,
    /// A durable, minimal reference to each matched catalogue entry: enough
    /// to name it and to preserve ambiguity, never enough (or needed) to
    /// reparse the DAT.
    pub matched_entries: Vec<DurableDatEntryRef>,
    /// The item hashes the audit actually compared - the baseline every
    /// later freshness check is made against.
    pub audited_hashes: LibraryItemHashes,
    /// When the audit that produced this ran (`now_utc_string()`-style).
    pub audited_at: String,
    /// Whether the audit that produced this examined everything.
    pub completeness: DatAuditCompleteness,
}

/// A durable pointer to one catalogue entry a library item matched. Carries
/// only what identifies the entry to a person; the original DAT is never
/// needed to use it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableDatEntryRef {
    /// The stable DAT source id the entry belongs to.
    pub source_id: String,
    /// The `<game>` / `<machine>` name.
    pub game_name: String,
    /// The `<rom>` name for the matched member, when known.
    pub rom_name: Option<String>,
    /// The DAT's own checksums for the matched member, when the audit
    /// retained a [`DatRomRef`] (archive-member audits do). Lower-case hex,
    /// exactly as the parser normalised them.
    pub checksums: Vec<(String, String)>,
}

impl PersistedLibraryDatIdentity {
    /// Builds the durable snapshot from a freshly computed summary plus the
    /// hashes the audit compared and the completeness of that run.
    ///
    /// `matched_refs` is the transient [`DatRomRef`] slice from the audit
    /// (empty for a flat physical-file audit); only its durable parts are
    /// kept.
    pub fn from_summary(
        summary: &LibraryDatIdentitySummary,
        matched_refs: &[DatRomRef],
        audited_hashes: &LibraryItemHashes,
        audited_at: impl Into<String>,
        completeness: DatAuditCompleteness,
    ) -> Self {
        let source_id = summary.source.source_id.clone();
        let mut matched_entries: Vec<DurableDatEntryRef> = matched_refs
            .iter()
            .map(|reference| DurableDatEntryRef {
                source_id: source_id.clone(),
                game_name: reference.game_name.clone(),
                rom_name: Some(reference.rom_name.clone()),
                checksums: reference
                    .checksums
                    .iter()
                    .map(|checksum| {
                        (
                            checksum.algorithm.label().to_string(),
                            checksum.value.clone(),
                        )
                    })
                    .collect(),
            })
            .collect();
        if matched_entries.is_empty() {
            // Flat physical-file audit: fall back to the names the summary
            // itself carries (single match) or the competing candidate list
            // (ambiguous) - never invent a winner.
            if let Some(game_name) = &summary.canonical.canonical_dat_name {
                matched_entries.push(DurableDatEntryRef {
                    source_id: source_id.clone(),
                    game_name: game_name.clone(),
                    rom_name: summary.canonical.canonical_rom_name.clone(),
                    checksums: Vec::new(),
                });
            } else {
                matched_entries.extend(summary.ambiguous_candidates.iter().map(|game_name| {
                    DurableDatEntryRef {
                        source_id: source_id.clone(),
                        game_name: game_name.clone(),
                        rom_name: None,
                        checksums: Vec::new(),
                    }
                }));
            }
        }
        Self {
            verification_state: summary.verification_state.clone(),
            source: summary.source.clone(),
            canonical: summary.canonical.clone(),
            hash_evidence: summary.hash_evidence.clone(),
            ambiguous_candidates: summary.ambiguous_candidates.clone(),
            matched_entries,
            audited_hashes: audited_hashes.clone(),
            audited_at: audited_at.into(),
            completeness,
        }
    }

    /// Whether this stored verdict carries a positive, name- or
    /// candidate-bearing identity that a partial run must never clobber.
    pub fn carries_identity(&self) -> bool {
        !matches!(
            self.verification_state,
            DatVerificationState::NoMatch | DatVerificationState::NoUsableEvidence
        )
    }
}

/// What the caller knows about the DAT source *now*, used to derive
/// revision-drift and source-unavailable staleness at read time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SourceFreshnessContext<'a> {
    /// The source's currently-configured catalogue `<version>`, when known.
    pub current_source_revision: Option<&'a str>,
    /// `false` when the DAT source is no longer configured / its file is
    /// gone - the stored identity stays visible, just `Unknown` freshness.
    pub source_available: bool,
    /// A bulk "this source's DAT was updated" flag persisted alongside the
    /// row (see
    /// `crate::database::Database::mark_library_dat_identity_stale_for_source_revision`).
    pub revision_marked_stale: bool,
}

impl PersistedLibraryDatIdentity {
    /// Rebuilds a [`LibraryDatIdentitySummary`] from this durable snapshot
    /// plus whatever is known about the item and source right now.
    ///
    /// Freshness is derived, never trusted from storage:
    /// - source not available now -> `Unknown`;
    /// - the source's current revision differs from the audited one, or a
    ///   bulk stale-mark is set -> `Stale`;
    /// - otherwise the current item hashes are compared to the audited
    ///   snapshot exactly as [`summarize_library_dat_identity`] does
    ///   (`Current` / `Stale` / `Unknown`).
    ///
    /// `set_dependency` is always [`DatSetDependencySummary::Pending`]: this
    /// is the seam where a persisted Arcade set/dependency verdict is meant
    /// to attach once that table exists.
    pub fn reconstruct_summary(
        &self,
        current_hashes: Option<&LibraryItemHashes>,
        context: SourceFreshnessContext<'_>,
    ) -> LibraryDatIdentitySummary {
        let provenance_freshness = if !context.source_available {
            DatProvenanceFreshness::Unknown
        } else if context.revision_marked_stale
            || revision_drifted(
                self.source.source_revision.as_deref(),
                context.current_source_revision,
            )
        {
            DatProvenanceFreshness::Stale
        } else {
            freshness(&self.audited_hashes, current_hashes)
        };

        LibraryDatIdentitySummary {
            verification_state: self.verification_state.clone(),
            source: self.source.clone(),
            canonical: self.canonical.clone(),
            hash_evidence: self.hash_evidence.clone(),
            provenance_freshness,
            ambiguous_candidates: self.ambiguous_candidates.clone(),
            set_dependency: DatSetDependencySummary::Pending {
                reason: "a persisted Arcade set / dependency verdict is not yet linked into \
                         per-item identity persistence"
                    .to_string(),
            },
        }
    }
}

/// `true` only when both revision strings are known and differ - a
/// confirmed source-revision change. An unknown current revision is not a
/// change, so it never on its own makes a stored identity `Stale` (that
/// path falls through to the hash comparison instead).
fn revision_drifted(audited: Option<&str>, current: Option<&str>) -> bool {
    matches!((audited, current), (Some(a), Some(c)) if a.trim() != c.trim())
}

#[cfg(test)]
mod tests;
