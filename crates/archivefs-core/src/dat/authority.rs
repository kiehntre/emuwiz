//! Read-only projections of existing DAT configuration, inventory and audit evidence.
//! No persistence, filesystem traversal, authority selection or mutation lives here.

use super::model::DatEcosystem;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AuthorityFreshness {
    /// Reserved for independently checked publisher evidence, not an import date.
    Current,
    Stale,
    Unknown,
}

/// Configuration is an input, not a second identity store. Missing platform
/// assignment is never interpreted as authority for every platform.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DatAuthoritySource {
    pub id: String,
    pub name: String,
    pub platform: Option<String>,
    pub enabled: bool,
    pub imported_at: Option<String>,
    pub revision: Option<String>,
    pub sha256: Option<String>,
    pub ecosystem: Option<DatEcosystem>,
    /// An already recorded invalid/unreadable validation outcome, not a new
    /// filesystem probe. Failed remote update checks do not invalidate a
    /// previously verified retained snapshot.
    pub validation_problem: Option<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatAuthorityStatus {
    pub source: DatAuthoritySource,
    pub ecosystem: Option<DatEcosystem>,
    pub variant: Option<String>,
    pub validated_at: Option<String>,
    pub inventory_revision: Option<String>,
    pub authority_confidence: String,
    pub freshness: AuthorityFreshness,
    pub inventory_usable: bool,
    pub preparation: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CompletenessState {
    Complete,
    Incomplete,
    PartialAuthority,
    Unverified,
    Ambiguous,
    NoAuthority,
}

impl CompletenessState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Complete => "Complete against imported DAT",
            Self::Incomplete => "Incomplete",
            Self::PartialAuthority => "Partial authority",
            Self::Unverified => "Not yet verified",
            Self::Ambiguous => "Ambiguous matches need review",
            Self::NoAuthority => "No assigned DAT authority",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CompletenessCounts {
    pub local: u64,
    pub expected: Option<u64>,
    pub matched: Option<u64>,
    /// Only entries without verified OR pending/ambiguous representation.
    /// Unknown if unaudited local files could represent them.
    pub missing: Option<u64>,
    pub ambiguous: u64,
    pub pending_entries: Option<u64>,
    /// Exhaustively audited no-matches, not every unverified local file.
    pub extra: u64,
    pub bios_missing: Option<u64>,
    pub unidentified_local: u64,
    pub verified_local: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CollectionCompleteness {
    pub platform: String,
    pub source_id: Option<String>,
    pub state: CompletenessState,
    pub counts: CompletenessCounts,
    pub explanations: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DatAuthorityDashboard {
    pub authorities: Vec<DatAuthorityStatus>,
    /// One row per platform/source. Overlapping catalogues are never summed.
    pub collections: Vec<CollectionCompleteness>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatRefreshImpact {
    pub old_source: String,
    pub new_source: String,
    pub added: u64,
    pub removed: u64,
    /// Available only for a caller-confirmed shared catalogue/variant namespace.
    pub renamed: Option<u64>,
    pub hash_changed: Option<u64>,
    pub bios_requirements_changed: Option<u64>,
    pub explanation: String,
}
