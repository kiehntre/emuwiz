//! One typed, read-only abstraction for an *installed* DAT catalogue.
//!
//! Before this module, "installed catalogue" was four different things
//! (see `docs/INSTALLED_DAT_CATALOGUE_PICKER_AUDIT.md`): a local registry
//! entry in `dat_sources.toml`, a No-Intro pack snapshot projected locally,
//! a selected TOSEC release-pack member projected locally, and a typed
//! managed MAME / Redump snapshot in `managed_dat_sources.toml` +
//! `managed-dats`. The three user journeys (Build Playing Library / 1G1R,
//! Verify Games, Repair) each selected a catalogue differently, and 1G1R
//! selected by *raw filesystem path*.
//!
//! This module adds:
//!
//! * [`CatalogueRef`] - a stable, explicit logical identity for a chosen
//!   catalogue. **Never a `PathBuf`.** A path is an implementation detail
//!   surfaced only as technical metadata.
//! * [`InstalledCatalogueSummary`] - a shared projection suitable for a
//!   future beginner-facing picker. Every derived field carries an
//!   [`EvidenceValue`] so "not inspected" and "inspected, still unknown"
//!   are distinguishable.
//! * [`list_installed_catalogues`] - deterministic, de-duplicated,
//!   per-row-fault-tolerant enumeration across the supported stores.
//! * [`resolve_catalogue`] - binds a [`CatalogueRef`] to real bytes,
//!   **fails closed**, and for content-addressed managed snapshots
//!   **re-hashes the backing file** and proves it still matches the
//!   selected snapshot (the gap the audit identified).
//! * thin adapters into the existing [`DatAuditRequest`],
//!   [`CombinedDatAuditSource`] and [`LibraryScanRequest`] so Verify and
//!   Repair can consume the same reference without a second parser or a
//!   second chooser, plus [`ResolvedCatalogue::parsed`] which is exactly
//!   what `build_playing_library_plan` already needs.
//!
//! No election / 1G1R semantics, DAT parsing limit, managed-trust
//! contract, repair re-proof rule, or combined-audit disagreement rule
//! changes here. This module only unifies *selection*.
//!
//! ## Not yet in v1
//!
//! * A multi-DAT **folder** local source is represented honestly as an
//!   [`CatalogueAvailability::AggregateFolder`] and is **not** eligible for
//!   single-catalogue 1G1R / Repair. Stable per-member references are a
//!   later step.
//! * No-Intro pack members are enriched only where their local projection
//!   is already registered; the pack store is not enumerated separately, so
//!   there is nothing to de-duplicate there yet.
//! * A persisted per-workflow "active catalogue" choice is deliberately
//!   deferred - see the audit's step 8.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dat::identity::identify_dat_source;
use crate::dat::limits::DatLimits;
use crate::dat::managed_sources::ManagedDatSources;
use crate::dat::model::{DatEcosystem, ParsedDat};
use crate::dat::parsers::parse_dat_file;
use crate::dat::sources::audit_run::{CombinedDatAuditSource, DatAuditRequest};
use crate::dat::sources::{
    DatHealthState, DatSourceEntry, DatSourceKind, DatSourceOwnership, DatSourceRegistry,
};
use crate::dat::updates::{
    ManagedDatProvider, ManagedDatSourceDescriptor, ManagedDatSourceId, ManagedDatState,
    load_managed_dat_state, resolve_current_managed_dat_source,
};
#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// Reference
// ---------------------------------------------------------------------------

/// The stable, explicit logical identity of a chosen installed catalogue.
///
/// This is what a picker keys its selection by - **never** a displayed
/// path, a list index, or a filename. A path can move, a folder can hold
/// many catalogues, and a managed snapshot can be replaced by an update;
/// none of those may silently change what an already-open choice meant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CatalogueRef {
    /// A catalogue registered in the local `dat_sources.toml` registry,
    /// named by its stable registration source ID. This covers manually
    /// added local files, and the local projections of accepted No-Intro
    /// pack members and applied TOSEC release-pack members (all of which
    /// already own a local source ID).
    ///
    /// `member` is reserved for a future per-file reference inside a
    /// folder source; until stable member identity exists a folder source
    /// is an aggregate and cannot be a single-catalogue choice.
    Local {
        source_id: String,
        member: Option<LocalCatalogueMemberRef>,
    },
    /// The *current* validated snapshot of a typed EmuWiz-managed source,
    /// bound to the exact snapshot SHA-256 so a later update cannot
    /// silently redefine the selection. The provider is carried inside
    /// `source_id`.
    ManagedCurrent {
        source_id: ManagedDatSourceId,
        snapshot_sha256: String,
    },
}

impl CatalogueRef {
    /// A local file/registry source by ID (no folder member).
    pub fn local(source_id: impl Into<String>) -> Self {
        Self::Local {
            source_id: source_id.into(),
            member: None,
        }
    }

    /// The current managed snapshot of a typed source.
    pub fn managed_current(
        source_id: ManagedDatSourceId,
        snapshot_sha256: impl Into<String>,
    ) -> Self {
        Self::ManagedCurrent {
            source_id,
            snapshot_sha256: snapshot_sha256.into(),
        }
    }

    /// A stable, human-inspectable token. Used only for deterministic
    /// ordering and diagnostics - it is **not** parsed back into a
    /// reference.
    pub fn token(&self) -> String {
        match self {
            Self::Local { source_id, member } => match member {
                Some(member) => format!("local:{source_id}#{}", member.relative_path.display()),
                None => format!("local:{source_id}"),
            },
            Self::ManagedCurrent {
                source_id,
                snapshot_sha256,
            } => format!(
                "managed:{}:{}@{}",
                managed_provider_token(source_id.provider),
                source_id.source_key,
                &snapshot_sha256[..snapshot_sha256.len().min(12)]
            ),
        }
    }
}

/// A future per-file reference inside a folder source. Source-scoped, never
/// a raw absolute child path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocalCatalogueMemberRef {
    /// The member's path relative to the folder source root.
    pub relative_path: PathBuf,
}

fn managed_provider_token(provider: ManagedDatProvider) -> &'static str {
    match provider {
        ManagedDatProvider::MameSoftwareList => "mame",
        ManagedDatProvider::RedumpBios => "redump-bios",
        ManagedDatProvider::RedumpGames => "redump-games",
    }
}

// ---------------------------------------------------------------------------
// Evidence-carrying values
// ---------------------------------------------------------------------------

/// A derived value plus how confident we are in it.
///
/// A plain `Option<T>` cannot tell "we never looked" from "we looked and it
/// is genuinely unknown", and cannot represent honest ambiguity. Every
/// derived catalogue field uses this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceValue<T> {
    /// The user explicitly set this (e.g. a manual platform assignment).
    Assigned(T),
    /// Derived from strong typed / parsed evidence (e.g. a managed
    /// descriptor, or a resolved DAT platform identity).
    Confirmed(T),
    /// The evidence points at more than one equally valid answer. Never
    /// collapsed to the first.
    Ambiguous(Vec<T>),
    /// Inspected, and still not determinable.
    Unknown,
    /// Could not be inspected at all (missing / corrupt backing artifact).
    Unavailable,
}

impl<T> EvidenceValue<T> {
    pub fn confirmed(&self) -> Option<&T> {
        match self {
            Self::Confirmed(value) | Self::Assigned(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous(_))
    }
}

/// A conservatively detected catalogue variant. Matches the set the
/// No-Intro pack importer already derives; no general cross-provider
/// variant field is invented, and `Unknown` is common and is never a
/// silent stand-in for either headered or headerless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogueVariant {
    Headered,
    Headerless,
    Aftermarket,
    Bios,
    Unknown,
}

impl CatalogueVariant {
    pub fn label(self) -> &'static str {
        match self {
            Self::Headered => "Headered",
            Self::Headerless => "Headerless",
            Self::Aftermarket => "Aftermarket",
            Self::Bios => "BIOS",
            Self::Unknown => "Unknown",
        }
    }
}

/// Where a catalogue's bytes came from, as trust evidence (not free text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueProvenance {
    /// A path the user registered themselves. Registered is not "trusted"
    /// or "current".
    UserRegistered,
    /// A local projection of an accepted browser-assisted No-Intro pack
    /// member.
    NoIntroPackProjection,
    /// A local projection of an applied TOSEC release-pack member.
    TosecReleasePackProjection { pack_id: String },
    /// A typed EmuWiz-managed snapshot.
    EmuWizManaged { provider: ManagedDatProvider },
}

/// Availability is separate from `enabled` / selected. An enabled local
/// entry can be missing; a configured managed source can be uninstalled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueAvailability {
    /// Backing artifact present and shaped correctly; the summary was
    /// built without a parse error. (A parse is still done at resolve
    /// time.)
    Ready,
    /// The registered file/folder is not present.
    Missing { reason: String },
    /// Present but its persisted health is stale or never established;
    /// revalidate before use.
    NeedsValidation { reason: String },
    /// Present but unreadable / not a DAT / parser refused it.
    Corrupt { reason: String },
    /// A managed source whose typed state is inconsistent or whose current
    /// snapshot object is not resolvable.
    StaleManagedState { reason: String },
    /// A local *folder* source: potentially many catalogues. Not a
    /// single-catalogue choice in v1.
    AggregateFolder { note: String },
    /// The reference names a source this build cannot resolve.
    Unregistered { reason: String },
}

impl CatalogueAvailability {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// A short, plain-English reason, or `""` for `Ready`.
    pub fn reason(&self) -> &str {
        match self {
            Self::Ready => "",
            Self::Missing { reason }
            | Self::NeedsValidation { reason }
            | Self::Corrupt { reason }
            | Self::StaleManagedState { reason }
            | Self::Unregistered { reason } => reason,
            Self::AggregateFolder { note } => note,
        }
    }
}

/// Which workflows a summary may legitimately feed. BIOS / non-game
/// catalogues are never silently offered as game catalogues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogueCapabilities {
    /// Explicit one-catalogue Verify audit.
    pub verify: bool,
    /// Participation in the existing combined multi-evidence audit / rename.
    pub combined_evidence: bool,
    /// Single-catalogue Repair Review scan.
    pub repair: bool,
    /// Single-catalogue Build Playing Library (1G1R).
    pub single_catalogue_1g1r: bool,
}

impl CatalogueCapabilities {
    const NONE: Self = Self {
        verify: false,
        combined_evidence: false,
        repair: false,
        single_catalogue_1g1r: false,
    };

    /// A ready, non-BIOS single catalogue can feed every game flow.
    const GAME_SINGLE: Self = Self {
        verify: true,
        combined_evidence: true,
        repair: true,
        single_catalogue_1g1r: true,
    };
}

/// The store a summary was enumerated from - the primary ordering key
/// after platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CatalogueStore {
    LocalRegistry,
    ManagedMameSoftwareList,
    ManagedRedumpGames,
    ManagedRedumpBios,
}

impl CatalogueStore {
    fn order_key(self) -> u8 {
        match self {
            Self::LocalRegistry => 0,
            Self::ManagedMameSoftwareList => 1,
            Self::ManagedRedumpGames => 2,
            Self::ManagedRedumpBios => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::LocalRegistry => "Local registry",
            Self::ManagedMameSoftwareList => "Managed · MAME software list",
            Self::ManagedRedumpGames => "Managed · Redump games",
            Self::ManagedRedumpBios => "Managed · Redump BIOS",
        }
    }
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

/// A shared, read-only projection of one installed catalogue for a picker.
///
/// Only claims supported by this row's evidence are represented; unknowns
/// are honest. Technical `technical_path` / `content_sha256` / IDs are
/// retained but are not the primary label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCatalogueSummary {
    pub reference: CatalogueRef,
    pub store: CatalogueStore,
    pub display_name: String,
    /// Canonical-platform display where derivable, plus how it was
    /// derived. `Assigned` = manual local assignment; `Confirmed` = strong
    /// typed/parsed identity.
    pub platform: EvidenceValue<String>,
    pub ecosystem: EvidenceValue<DatEcosystem>,
    pub variant: EvidenceValue<CatalogueVariant>,
    /// Header/upstream revision string where known.
    pub revision: Option<String>,
    /// Local `added_unix_seconds` for a local row (projection time only for
    /// a No-Intro projection), or a managed source's retrieval time.
    pub imported_or_retrieved_at: Option<u64>,
    /// A content SHA-256 where one is persisted (managed snapshots; some
    /// TOSEC members). Generic local files have none until re-hashed.
    pub content_sha256: Option<String>,
    pub provenance: CatalogueProvenance,
    pub availability: CatalogueAvailability,
    /// Local enabled flag / managed "current snapshot exists". Distinct
    /// from availability.
    pub enabled: bool,
    pub capabilities: CatalogueCapabilities,
    /// The backing file/folder path - technical metadata only.
    pub technical_path: Option<PathBuf>,
}

impl InstalledCatalogueSummary {
    /// The tuple used for deterministic, stable ordering. Never uses raw
    /// path or list index.
    fn order_tuple(&self) -> (String, u8, String, String, String, String) {
        let platform = match &self.platform {
            EvidenceValue::Assigned(p) | EvidenceValue::Confirmed(p) => p.clone(),
            EvidenceValue::Ambiguous(_) => "\u{fffe}ambiguous".to_string(),
            EvidenceValue::Unknown | EvidenceValue::Unavailable => "\u{ffff}unknown".to_string(),
        };
        let ecosystem = self
            .ecosystem
            .confirmed()
            .map(|e| e.label().to_string())
            .unwrap_or_else(|| "\u{ffff}".to_string());
        let variant = self
            .variant
            .confirmed()
            .map(|v| v.label().to_string())
            .unwrap_or_else(|| "\u{ffff}".to_string());
        (
            platform,
            self.store.order_key(),
            ecosystem,
            variant,
            self.display_name.clone(),
            self.reference.token(),
        )
    }
}

// ---------------------------------------------------------------------------
// Inventory inputs (injected for tests)
// ---------------------------------------------------------------------------

/// Everything [`list_installed_catalogues`] / [`resolve_catalogue`] read.
/// All roots are injected so a test never touches the real app data dir.
#[derive(Debug, Clone, Copy)]
pub struct CatalogueInventoryInputs<'a> {
    pub local_registry: &'a DatSourceRegistry,
    pub managed_sources: &'a ManagedDatSources,
    pub managed_root: &'a Path,
    /// DAT parser limits used by [`resolve_catalogue`]. Enumeration itself
    /// does not parse; this is carried so a single inputs value drives both.
    pub limits: DatLimits,
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

/// Enumerate every installed catalogue across the supported stores.
///
/// * deterministic order: `(platform, store, ecosystem, variant,
///   display_name, typed reference)`;
/// * de-duplicated by [`CatalogueRef`];
/// * per-row fault tolerant - a broken managed state or missing local file
///   produces an *unavailable* row, it does not drop unrelated rows;
/// * distinct ecosystems and distinct variants for one platform stay
///   distinct;
/// * **no row is selected.**
pub fn list_installed_catalogues(
    inputs: CatalogueInventoryInputs<'_>,
) -> Vec<InstalledCatalogueSummary> {
    let mut rows: Vec<InstalledCatalogueSummary> = Vec::new();

    for entry in inputs.local_registry.sorted_all() {
        rows.push(summarise_local(entry));
    }

    // Managed MAME software lists + Redump games are game catalogues.
    // Redump BIOS is enumerated but never game-capable. Each configured
    // entry is resolved on its own so a single stale state cannot drop the
    // rest of its store.
    for (store, provider, is_game) in [
        (
            CatalogueStore::ManagedMameSoftwareList,
            ManagedDatProvider::MameSoftwareList,
            true,
        ),
        (
            CatalogueStore::ManagedRedumpGames,
            ManagedDatProvider::RedumpGames,
            true,
        ),
        (
            CatalogueStore::ManagedRedumpBios,
            ManagedDatProvider::RedumpBios,
            false,
        ),
    ] {
        for descriptor in managed_descriptors(inputs.managed_sources, provider) {
            let descriptor = match descriptor {
                Ok(descriptor) => descriptor,
                Err(_) => continue,
            };
            rows.push(summarise_managed(
                store,
                descriptor.source_id().clone(),
                load_managed_row(inputs.managed_root, &descriptor),
                is_game,
            ));
        }
    }

    // De-duplicate by reference (keep the first, which is store-ordered).
    let mut seen: HashSet<CatalogueRef> = HashSet::new();
    rows.retain(|row| seen.insert(row.reference.clone()));

    rows.sort_by_key(|row| row.order_tuple());
    rows
}

fn summarise_local(entry: &DatSourceEntry) -> InstalledCatalogueSummary {
    let path = entry.path.clone();
    let present = path.exists();

    let (availability, capabilities) = if !present {
        (
            CatalogueAvailability::Missing {
                reason: format!("{} is not present", path.display()),
            },
            CatalogueCapabilities::NONE,
        )
    } else if entry.kind == DatSourceKind::Folder {
        (
            CatalogueAvailability::AggregateFolder {
                note: "This registered folder can contain several catalogues. \
                       Pick an individual catalogue for a single-catalogue action."
                    .to_string(),
            },
            // A folder may still take part in the existing combined /
            // Verify per-source flows, which already accept folder input.
            CatalogueCapabilities {
                verify: entry.enabled,
                combined_evidence: entry.enabled,
                repair: false,
                single_catalogue_1g1r: false,
            },
        )
    } else if matches!(
        entry.health.state(),
        DatHealthState::Invalid | DatHealthState::Unreadable
    ) {
        (
            CatalogueAvailability::Corrupt {
                reason: entry.health.detail.clone().unwrap_or_else(|| {
                    "The last validation could not parse this catalogue.".to_string()
                }),
            },
            CatalogueCapabilities::NONE,
        )
    } else if matches!(entry.health.state(), DatHealthState::NotChecked)
        || entry.health.is_stale_for(&path, entry.kind)
    {
        (
            CatalogueAvailability::NeedsValidation {
                reason: "Not validated yet, or the file changed since it was last checked."
                    .to_string(),
            },
            CatalogueCapabilities {
                // Present but unproven: allow combined evidence (which
                // re-parses), block single-catalogue actions until
                // validated.
                verify: entry.enabled,
                combined_evidence: entry.enabled,
                repair: false,
                single_catalogue_1g1r: false,
            },
        )
    } else {
        (
            CatalogueAvailability::Ready,
            if entry.enabled {
                CatalogueCapabilities::GAME_SINGLE
            } else {
                // Disabled: visible for management, not for automatic /
                // combined participation until enabled.
                CatalogueCapabilities::NONE
            },
        )
    };

    let provenance = match entry.ownership() {
        DatSourceOwnership::UserLocal => match entry.origin.as_deref() {
            Some(origin) if origin.contains("No-Intro pack import") => {
                CatalogueProvenance::NoIntroPackProjection
            }
            _ => CatalogueProvenance::UserRegistered,
        },
        DatSourceOwnership::EmuWizManaged => {
            // A projected managed row is not expected in the local
            // registry today, but represent it honestly if present.
            CatalogueProvenance::UserRegistered
        }
        DatSourceOwnership::ImportedTosecReleasePack { pack_id, .. } => {
            CatalogueProvenance::TosecReleasePackProjection {
                pack_id: pack_id.clone(),
            }
        }
    };

    let platform = match entry.platform.as_deref() {
        Some(assigned) => EvidenceValue::Assigned(
            entry
                .platform_display()
                .unwrap_or_else(|| assigned.to_string()),
        ),
        None => EvidenceValue::Unknown,
    };

    InstalledCatalogueSummary {
        reference: CatalogueRef::local(entry.id.clone()),
        store: CatalogueStore::LocalRegistry,
        display_name: entry.display_name.clone(),
        platform,
        // Generic local rows have no persisted parsed ecosystem/variant.
        ecosystem: EvidenceValue::Unknown,
        variant: EvidenceValue::Unknown,
        revision: None,
        imported_or_retrieved_at: entry.added_unix_seconds,
        content_sha256: None,
        provenance,
        availability,
        enabled: entry.enabled,
        capabilities,
        technical_path: Some(path),
    }
}

/// The per-entry outcome of loading one typed managed source's local
/// state. Kept distinct from a bare `Option` so "never installed" and
/// "installed but its saved state is stale" are not conflated.
enum ManagedRowState {
    /// Configured, but this build has no local state for it yet.
    NotInstalled,
    /// A `state.json` exists but does not load / does not validate, or its
    /// current snapshot object cannot be resolved to a regular file.
    Stale { reason: String },
    /// Loaded and validated, with the current snapshot's resolved path.
    Installed {
        state: Box<ManagedDatState>,
        current_path: PathBuf,
    },
}

fn managed_descriptors(
    managed_sources: &ManagedDatSources,
    provider: ManagedDatProvider,
) -> Vec<Result<ManagedDatSourceDescriptor, CatalogueResolveError>> {
    let map = |result: crate::Result<ManagedDatSourceDescriptor>| result.map_err(stale_managed);
    match provider {
        ManagedDatProvider::MameSoftwareList => managed_sources
            .entries()
            .iter()
            .map(|c| map(c.descriptor()))
            .collect(),
        ManagedDatProvider::RedumpGames => managed_sources
            .redump_games_entries()
            .iter()
            .map(|c| map(c.descriptor()))
            .collect(),
        ManagedDatProvider::RedumpBios => managed_sources
            .redump_bios_entries()
            .iter()
            .map(|c| map(c.descriptor()))
            .collect(),
    }
}

fn load_managed_row(
    managed_root: &Path,
    descriptor: &ManagedDatSourceDescriptor,
) -> ManagedRowState {
    let state = match load_managed_dat_state(managed_root, descriptor) {
        Ok(state) => state,
        Err(crate::ArchiveFsError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return ManagedRowState::NotInstalled;
        }
        Err(error) => {
            return ManagedRowState::Stale {
                reason: error.to_string(),
            };
        }
    };
    match resolve_current_managed_dat_source(managed_root, &state) {
        Ok(current) => ManagedRowState::Installed {
            current_path: current.path().to_path_buf(),
            state: Box::new(state),
        },
        Err(error) => ManagedRowState::Stale {
            reason: error.to_string(),
        },
    }
}

fn summarise_managed(
    store: CatalogueStore,
    source_id: ManagedDatSourceId,
    row: ManagedRowState,
    is_game_catalogue: bool,
) -> InstalledCatalogueSummary {
    let provider = source_id.provider;
    let state = match &row {
        ManagedRowState::Installed { state, .. } => Some(state.as_ref()),
        _ => None,
    };

    let display_name = state
        .map(|s| s.authoritative_name.clone())
        .unwrap_or_else(|| source_id.source_key.clone());

    let (availability, enabled) = match &row {
        ManagedRowState::Installed { .. } => (CatalogueAvailability::Ready, true),
        ManagedRowState::Stale { reason } => (
            CatalogueAvailability::StaleManagedState {
                reason: reason.clone(),
            },
            false,
        ),
        ManagedRowState::NotInstalled => (
            CatalogueAvailability::Missing {
                reason: "This managed source is configured but has never been installed."
                    .to_string(),
            },
            false,
        ),
    };

    let capabilities = if is_game_catalogue && availability.is_ready() {
        CatalogueCapabilities::GAME_SINGLE
    } else {
        // A BIOS catalogue, or an unavailable game catalogue: no workflow.
        CatalogueCapabilities::NONE
    };

    let ecosystem = state
        .map(|s| EvidenceValue::Confirmed(s.parsed_ecosystem))
        .unwrap_or(EvidenceValue::Unavailable);

    let platform = state
        .map(|s| EvidenceValue::Confirmed(s.authoritative_name.clone()))
        .unwrap_or(EvidenceValue::Unavailable);

    let variant = if is_game_catalogue {
        EvidenceValue::Unknown
    } else {
        EvidenceValue::Confirmed(CatalogueVariant::Bios)
    };

    InstalledCatalogueSummary {
        reference: CatalogueRef::ManagedCurrent {
            source_id,
            snapshot_sha256: state.map(|s| s.sha256.clone()).unwrap_or_default(),
        },
        store,
        display_name,
        platform,
        ecosystem,
        variant,
        revision: state.and_then(|s| s.upstream_revision.clone()),
        imported_or_retrieved_at: state.and_then(|s| s.retrieved_at_unix_seconds),
        content_sha256: state.map(|s| s.sha256.clone()),
        provenance: CatalogueProvenance::EmuWizManaged { provider },
        availability,
        enabled,
        capabilities,
        technical_path: match &row {
            ManagedRowState::Installed { current_path, .. } => Some(current_path.clone()),
            _ => None,
        },
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// A [`CatalogueRef`] bound to real, revalidated bytes.
///
/// `parsed` is exactly what `build_playing_library_plan` and the audit
/// engines need. `backing_path` / `kind` are the legacy data the existing
/// request types still carry.
#[derive(Debug, Clone)]
pub struct ResolvedCatalogue {
    pub reference: CatalogueRef,
    pub source_id: String,
    pub display_name: String,
    pub backing_path: PathBuf,
    pub kind: DatSourceKind,
    pub parsed: ParsedDat,
    pub ecosystem: DatEcosystem,
    pub provenance: CatalogueProvenance,
    /// The canonical platform the caller assigned, if any (never inferred
    /// here from the DAT header alone). A DAT-derived platform identity is
    /// available via [`ResolvedCatalogue::dat_platform_identity`].
    pub assigned_platform: Option<String>,
}

impl ResolvedCatalogue {
    /// The DAT-derived platform identity (`Resolved` / `Ambiguous` /
    /// `Unknown`) - fail-closed, never guessed.
    pub fn dat_platform_identity(&self) -> crate::dat::identity::DatPlatformIdentity {
        identify_dat_source(&self.parsed)
    }

    /// Borrow the parsed catalogue as the Build Playing Library planner
    /// input. The planner already works purely on `&ParsedDat`, so this is
    /// the whole 1G1R seam: a resolved [`CatalogueRef`] in, the exact same
    /// `PlayingLibraryRequest` the path-based caller built out. No election,
    /// region-preference, or explanation behaviour changes.
    pub fn playing_library_request<'a>(
        &'a self,
        matches: Vec<crate::playing_library::DatArchiveMatch>,
        destination_root: PathBuf,
        policy: crate::playing_library::PlayingLibraryPolicy,
    ) -> crate::playing_library::PlayingLibraryRequest<'a> {
        crate::playing_library::PlayingLibraryRequest {
            dat: &self.parsed,
            matches,
            destination_root,
            policy,
        }
    }

    /// Adapt into the existing single-source Verify audit request. Verdict /
    /// disagreement / policy semantics are unchanged; only selection is
    /// unified.
    pub fn to_dat_audit_request(
        &self,
        scan_root: PathBuf,
        limits: DatLimits,
        policy: Option<crate::dat::policy::EffectiveDatPolicy>,
    ) -> DatAuditRequest {
        DatAuditRequest {
            source_id: self.source_id.clone(),
            source_display_name: self.display_name.clone(),
            dat_path: self.backing_path.clone(),
            dat_kind: self.kind,
            scan_root,
            limits,
            policy,
            platform: self.assigned_platform.clone(),
        }
    }

    /// Adapt into one participant of the existing combined multi-evidence
    /// audit. Its agreement / disagreement semantics are unchanged.
    pub fn to_combined_dat_audit_source(&self) -> CombinedDatAuditSource {
        CombinedDatAuditSource {
            source_id: self.source_id.clone(),
            source_display_name: self.display_name.clone(),
            dat_path: self.backing_path.clone(),
            dat_kind: self.kind,
            platform: self.assigned_platform.clone(),
        }
    }

    /// Adapt into the existing Repair whole-library scan request. The
    /// repair engine, rename plan, plan provenance, and apply re-proof are
    /// all unchanged; this only replaces how the catalogue was chosen.
    pub fn to_library_scan_request(
        &self,
        scan_root: PathBuf,
        limits: DatLimits,
        profile: crate::repair::library::RepairProfile,
        audit_cache: crate::dat::sources::audit_cache::AuditCacheConfig,
    ) -> crate::repair::library::LibraryScanRequest {
        crate::repair::library::LibraryScanRequest {
            source_id: self.source_id.clone(),
            source_display_name: self.display_name.clone(),
            dat_path: self.backing_path.clone(),
            dat_kind: self.kind,
            scan_root,
            limits,
            profile,
            audit_cache,
        }
    }
}

/// Why a catalogue reference could not be bound to usable bytes.
/// **Every arm fails closed** - the resolver never falls back to
/// "whatever file is at this path" or to a retained previous snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueResolveError {
    /// The reference names a local source ID or managed source this build
    /// does not have configured.
    UnknownReference { detail: String },
    /// The backing file is not present.
    MissingBackingFile { path: PathBuf },
    /// A content-addressed managed snapshot's backing bytes no longer hash
    /// to the selected snapshot digest.
    SnapshotHashMismatch {
        reference: CatalogueRef,
        expected: String,
        actual: String,
    },
    /// The catalogue could not be parsed within limits.
    CorruptCatalogue { detail: String },
    /// A managed source whose typed state does not resolve (missing object,
    /// inconsistent descriptor, symlinked path, ...).
    StaleManagedState { detail: String },
    /// The chosen variant is unusable for the requested action (e.g. a
    /// BIOS catalogue for a game flow, or a headered/headerless mismatch).
    /// `Unknown` variant is never treated as either.
    UnsupportedVariant { detail: String },
    /// A local *folder* source cannot be a single catalogue in v1.
    AggregateFolderNotSingleCatalogue { source_id: String },
    /// The source exists but is disabled; enable it first.
    Disabled { source_id: String },
    /// The DAT parsed, but its platform identity is ambiguous and this
    /// flow is platform-scoped.
    PlatformAmbiguity { candidates: Vec<String> },
    /// A `for_platform` request found several equally valid installed
    /// catalogues and requires an explicit choice.
    MultipleCandidates {
        platform: String,
        candidates: Vec<InstalledCatalogueSummary>,
    },
    /// A `for_platform` request found no usable installed catalogue for the
    /// platform.
    NoCatalogueForPlatform { platform: String },
}

impl std::fmt::Display for CatalogueResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownReference { detail } => write!(f, "unknown catalogue reference: {detail}"),
            Self::MissingBackingFile { path } => {
                write!(f, "the catalogue's file is not present: {}", path.display())
            }
            Self::SnapshotHashMismatch {
                expected, actual, ..
            } => write!(
                f,
                "the managed snapshot's bytes changed (expected {expected}, found {actual})"
            ),
            Self::CorruptCatalogue { detail } => {
                write!(f, "the catalogue could not be read: {detail}")
            }
            Self::StaleManagedState { detail } => {
                write!(f, "the managed catalogue's saved state is stale: {detail}")
            }
            Self::UnsupportedVariant { detail } => {
                write!(f, "unsupported catalogue variant: {detail}")
            }
            Self::AggregateFolderNotSingleCatalogue { source_id } => write!(
                f,
                "'{source_id}' is a folder of catalogues; choose one catalogue inside it"
            ),
            Self::Disabled { source_id } => write!(f, "'{source_id}' is disabled; enable it first"),
            Self::PlatformAmbiguity { candidates } => write!(
                f,
                "the catalogue's platform is ambiguous ({} candidates)",
                candidates.len()
            ),
            Self::MultipleCandidates {
                platform,
                candidates,
            } => write!(
                f,
                "{} installed catalogues describe {platform}; choose one",
                candidates.len()
            ),
            Self::NoCatalogueForPlatform { platform } => {
                write!(f, "no installed catalogue describes {platform}")
            }
        }
    }
}

impl std::error::Error for CatalogueResolveError {}

/// The largest DAT file this resolver will hash / read, taken from the
/// caller's parser limits so hashing and parsing share one ceiling.
fn bounded_sha256(path: &Path, max_bytes: u64) -> Result<String, CatalogueResolveError> {
    let file = std::fs::File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CatalogueResolveError::MissingBackingFile {
                path: path.to_path_buf(),
            }
        } else {
            CatalogueResolveError::CorruptCatalogue {
                detail: format!("{}: {error}", path.display()),
            }
        }
    })?;
    let mut reader = file.take(max_bytes.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let read =
            reader
                .read(&mut buffer)
                .map_err(|error| CatalogueResolveError::CorruptCatalogue {
                    detail: format!("{}: {error}", path.display()),
                })?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > max_bytes {
            return Err(CatalogueResolveError::CorruptCatalogue {
                detail: format!(
                    "{} exceeds the {max_bytes}-byte catalogue limit",
                    path.display()
                ),
            });
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Resolve one explicit [`CatalogueRef`] to revalidated, parsed bytes.
///
/// Fail-closed on: missing backing file, changed managed-snapshot bytes,
/// hash mismatch, corrupt catalogue, unknown/unregistered reference, stale
/// managed state, an aggregate folder, or a disabled source. No silent
/// fallback of any kind.
pub fn resolve_catalogue(
    reference: &CatalogueRef,
    inputs: CatalogueInventoryInputs<'_>,
) -> Result<ResolvedCatalogue, CatalogueResolveError> {
    match reference {
        CatalogueRef::Local { source_id, member } => {
            if member.is_some() {
                return Err(CatalogueResolveError::AggregateFolderNotSingleCatalogue {
                    source_id: source_id.clone(),
                });
            }
            let entry = inputs
                .local_registry
                .entries()
                .iter()
                .find(|entry| &entry.id == source_id)
                .ok_or_else(|| CatalogueResolveError::UnknownReference {
                    detail: format!("no local DAT source with ID '{source_id}'"),
                })?;
            if !entry.enabled {
                return Err(CatalogueResolveError::Disabled {
                    source_id: source_id.clone(),
                });
            }
            if entry.kind == DatSourceKind::Folder {
                return Err(CatalogueResolveError::AggregateFolderNotSingleCatalogue {
                    source_id: source_id.clone(),
                });
            }
            if !entry.path.exists() {
                return Err(CatalogueResolveError::MissingBackingFile {
                    path: entry.path.clone(),
                });
            }
            let outcome = parse_dat_file(&entry.path, inputs.limits).map_err(|error| {
                CatalogueResolveError::CorruptCatalogue {
                    detail: format!("{error:?}"),
                }
            })?;
            Ok(ResolvedCatalogue {
                reference: reference.clone(),
                source_id: entry.id.clone(),
                display_name: entry.display_name.clone(),
                backing_path: entry.path.clone(),
                kind: DatSourceKind::File,
                ecosystem: outcome.dat.source.ecosystem,
                parsed: outcome.dat,
                provenance: local_provenance(entry),
                assigned_platform: entry.platform.clone(),
            })
        }
        CatalogueRef::ManagedCurrent {
            source_id,
            snapshot_sha256,
        } => {
            let (state, resolved_path, display_name) =
                load_managed_current(source_id, inputs.managed_sources, inputs.managed_root)?;

            // The audit's identified gap: bind the choice to the exact
            // snapshot bytes, not just the state's shape.
            if state.sha256 != *snapshot_sha256 {
                return Err(CatalogueResolveError::SnapshotHashMismatch {
                    reference: reference.clone(),
                    expected: snapshot_sha256.clone(),
                    actual: state.sha256.clone(),
                });
            }
            let actual = bounded_sha256(&resolved_path, inputs.limits.max_file_size)?;
            if actual != *snapshot_sha256 {
                return Err(CatalogueResolveError::SnapshotHashMismatch {
                    reference: reference.clone(),
                    expected: snapshot_sha256.clone(),
                    actual,
                });
            }

            if source_id.provider == ManagedDatProvider::RedumpBios {
                return Err(CatalogueResolveError::UnsupportedVariant {
                    detail: "a Redump BIOS catalogue is not a game catalogue".to_string(),
                });
            }

            let outcome = parse_dat_file(&resolved_path, inputs.limits).map_err(|error| {
                CatalogueResolveError::CorruptCatalogue {
                    detail: format!("{error:?}"),
                }
            })?;
            Ok(ResolvedCatalogue {
                reference: reference.clone(),
                source_id: format!(
                    "{}:{}",
                    managed_provider_token(source_id.provider),
                    source_id.source_key
                ),
                display_name,
                backing_path: resolved_path,
                kind: DatSourceKind::File,
                ecosystem: outcome.dat.source.ecosystem,
                parsed: outcome.dat,
                provenance: CatalogueProvenance::EmuWizManaged {
                    provider: source_id.provider,
                },
                assigned_platform: None,
            })
        }
    }
}

fn local_provenance(entry: &DatSourceEntry) -> CatalogueProvenance {
    match entry.ownership() {
        DatSourceOwnership::ImportedTosecReleasePack { pack_id, .. } => {
            CatalogueProvenance::TosecReleasePackProjection {
                pack_id: pack_id.clone(),
            }
        }
        DatSourceOwnership::UserLocal => match entry.origin.as_deref() {
            Some(origin) if origin.contains("No-Intro pack import") => {
                CatalogueProvenance::NoIntroPackProjection
            }
            _ => CatalogueProvenance::UserRegistered,
        },
        DatSourceOwnership::EmuWizManaged => CatalogueProvenance::UserRegistered,
    }
}

/// Loads a typed managed source's *current* snapshot and its resolved
/// regular-file path. Errors with [`CatalogueResolveError`] rather than the
/// crate's generic error so the resolver stays fail-closed and typed.
fn load_managed_current(
    source_id: &ManagedDatSourceId,
    managed_sources: &ManagedDatSources,
    managed_root: &Path,
) -> Result<(ManagedDatState, PathBuf, String), CatalogueResolveError> {
    let descriptor = managed_descriptors(managed_sources, source_id.provider)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|descriptor| descriptor.source_id() == source_id)
        .ok_or_else(|| CatalogueResolveError::UnknownReference {
            detail: format!(
                "no configured managed source '{}:{}'",
                managed_provider_token(source_id.provider),
                source_id.source_key
            ),
        })?;

    match load_managed_row(managed_root, &descriptor) {
        ManagedRowState::Installed {
            state,
            current_path,
        } => {
            let display_name = state.authoritative_name.clone();
            Ok((*state, current_path, display_name))
        }
        ManagedRowState::NotInstalled => Err(CatalogueResolveError::StaleManagedState {
            detail: "this managed source is configured but has never been installed".to_string(),
        }),
        ManagedRowState::Stale { reason } => {
            Err(CatalogueResolveError::StaleManagedState { detail: reason })
        }
    }
}

fn stale_managed(error: crate::ArchiveFsError) -> CatalogueResolveError {
    CatalogueResolveError::StaleManagedState {
        detail: error.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Platform-scoped resolution with explicit-choice-required ambiguity
// ---------------------------------------------------------------------------

/// Resolve *the* installed game catalogue for one canonical platform, or
/// return a typed ambiguous result. **Never** ranks candidates by local
/// priority, filename, newest-looking version, or provider name.
///
/// * 0 usable catalogues -> [`CatalogueResolveError::NoCatalogueForPlatform`];
/// * exactly 1 -> resolved deterministically;
/// * more than 1 -> [`CatalogueResolveError::MultipleCandidates`] carrying
///   every candidate summary for the caller to choose from.
pub fn resolve_catalogue_for_platform(
    platform_display: &str,
    inputs: CatalogueInventoryInputs<'_>,
) -> Result<ResolvedCatalogue, CatalogueResolveError> {
    let summaries = list_installed_catalogues(inputs);
    let candidates: Vec<&InstalledCatalogueSummary> = summaries
        .iter()
        .filter(|row| row.capabilities.single_catalogue_1g1r)
        .filter(|row| match &row.platform {
            EvidenceValue::Assigned(p) | EvidenceValue::Confirmed(p) => {
                platform_matches(p, platform_display)
            }
            _ => false,
        })
        .collect();

    match candidates.as_slice() {
        [] => Err(CatalogueResolveError::NoCatalogueForPlatform {
            platform: platform_display.to_string(),
        }),
        [only] => {
            let resolved = resolve_catalogue(&only.reference, inputs)?;
            // The single row named this platform, but the DAT's own strong
            // evidence must not itself be internally ambiguous for a
            // platform-scoped flow.
            if let crate::dat::identity::DatPlatformIdentity::Ambiguous { candidates } =
                resolved.dat_platform_identity()
            {
                return Err(CatalogueResolveError::PlatformAmbiguity {
                    candidates: candidates
                        .into_iter()
                        .map(|evidence| evidence.platform)
                        .collect(),
                });
            }
            Ok(resolved)
        }
        many => Err(CatalogueResolveError::MultipleCandidates {
            platform: platform_display.to_string(),
            candidates: many.iter().map(|row| (*row).clone()).collect(),
        }),
    }
}

fn platform_matches(a: &str, b: &str) -> bool {
    if a.eq_ignore_ascii_case(b) {
        return true;
    }
    match (
        crate::canonical_platform_for_alias(a),
        crate::canonical_platform_for_alias(b),
    ) {
        (Some(ca), Some(cb)) => ca == cb,
        _ => false,
    }
}
