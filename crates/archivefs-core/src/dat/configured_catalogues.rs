//! Current configured game catalogues across the local and managed stores.
//! Reads DAT bytes only to establish content identity; never scans a library or
//! infers a verification. Call once on a worker, not once per library row.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::catalogue_selection::{
    CatalogueInventoryInputs, CatalogueRef, ManagedRowState, bounded_sha256, load_managed_row,
    managed_dat_audit_source_id, resolve_catalogue,
};
use super::managed_sources::load_managed_dat_sources_default;
use super::sources::{DatSourceKind, DatSourceRegistry, load_dat_sources_config_from};
use super::updates::{ManagedDatProvider, managed_dat_root};

#[derive(Debug, Clone)]
pub struct ConfiguredCatalogue {
    pub reference: CatalogueRef,
    pub source_id: String,
    pub display_name: String,
    pub path: std::path::PathBuf,
    pub kind: DatSourceKind,
    pub sha256: Option<String>,
    pub revision: Option<String>,
}

impl ConfiguredCatalogue {
    /// Applies current catalogue content identity to an already reconstructed
    /// item summary. This can only lower freshness, never grant verification.
    pub fn check_saved_identity(
        &self,
        summary: &mut super::library_identity_summary::LibraryDatIdentitySummary,
    ) {
        use super::library_identity_summary::DatProvenanceFreshness;
        match (
            summary.source.catalogue_sha256.as_deref(),
            self.sha256.as_deref(),
        ) {
            (Some(saved), Some(current)) if saved != current => {
                summary.provenance_freshness = DatProvenanceFreshness::Stale;
            }
            (Some(_), Some(_)) => {}
            _ if summary.provenance_freshness != DatProvenanceFreshness::Stale => {
                summary.provenance_freshness = DatProvenanceFreshness::Unknown;
            }
            _ => {}
        }
    }
}

/// Ambiguous IDs (including collisions between stores) are withheld, never
/// resolved by map insertion order. Disabled and missing sources are absent.
pub fn configured_catalogues(
    inputs: CatalogueInventoryInputs<'_>,
) -> HashMap<String, ConfiguredCatalogue> {
    let mut rows = HashMap::new();
    let mut seen = HashSet::new();
    let mut insert = |row: ConfiguredCatalogue| {
        if !seen.insert(row.source_id.clone()) {
            rows.remove(&row.source_id);
        } else {
            rows.insert(row.source_id.clone(), row);
        }
    };
    // Reserve managed IDs even while disabled or uninstalled: a local source
    // must never impersonate saved managed evidence by taking its audit ID.
    let managed_ids: HashSet<_> = inputs
        .managed_sources
        .descriptors()
        .unwrap_or_default()
        .iter()
        .map(|descriptor| managed_dat_audit_source_id(descriptor.source_id()))
        .collect();
    for entry in inputs.local_registry.entries() {
        if !entry.enabled
            || super::sources::validation::validate_dat_path(&entry.path, entry.kind).is_err()
            || managed_ids.contains(&entry.id)
            || ["mame:", "redump-games:", "redump-bios:", "fbneo:"]
                .iter()
                .any(|prefix| entry.id.starts_with(prefix))
        {
            continue;
        }
        let sha256 = if entry.kind == DatSourceKind::File {
            let Ok(sha) = bounded_sha256(&entry.path, inputs.limits.max_file_size) else {
                continue;
            };
            Some(sha)
        } else {
            None
        };
        let revisions = &entry.health.arcade_catalogue_revisions;
        // Validation health is historical metadata, not the current file's
        // version. Exact single-file digest identity supersedes that cache.
        let revision = (sha256.is_none() && revisions.len() == 1)
            .then(|| revisions[0].version.clone())
            .flatten();
        insert(ConfiguredCatalogue {
            reference: CatalogueRef::local(&entry.id),
            source_id: entry.id.clone(),
            display_name: entry.display_name.clone(),
            path: entry.path.clone(),
            kind: entry.kind,
            sha256,
            revision,
        });
    }
    for descriptor in inputs.managed_sources.descriptors().unwrap_or_default() {
        if descriptor.source_id().provider == ManagedDatProvider::RedumpBios {
            continue;
        }
        let ManagedRowState::Installed {
            state,
            current_path,
        } = load_managed_row(inputs.managed_root, &descriptor)
        else {
            continue;
        };
        if descriptor.update_policy() == super::updates::ManagedDatUpdatePolicy::Disabled {
            continue;
        }
        let Ok(actual) = bounded_sha256(&current_path, inputs.limits.max_file_size) else {
            continue;
        };
        if actual != state.sha256 {
            continue;
        }
        insert(ConfiguredCatalogue {
            reference: CatalogueRef::managed_current(state.source_id.clone(), &state.sha256),
            source_id: managed_dat_audit_source_id(&state.source_id),
            display_name: state.authoritative_name.clone(),
            path: current_path,
            kind: DatSourceKind::File,
            sha256: Some(state.sha256.clone()),
            // Transport revision (e.g. a git SHA) is not the DAT's <version>.
            revision: None,
        });
    }
    rows
}

pub fn load_configured_catalogues(local_config: &Path) -> HashMap<String, ConfiguredCatalogue> {
    let config = load_dat_sources_config_from(local_config).unwrap_or_default();
    let (local, _) = DatSourceRegistry::from_config(&config);
    let managed = load_managed_dat_sources_default().unwrap_or_default();
    let Ok(root) = managed_dat_root() else {
        return HashMap::new();
    };
    configured_catalogues(CatalogueInventoryInputs {
        local_registry: &local,
        managed_sources: &managed,
        managed_root: &root,
        limits: super::limits::DatLimits::default(),
    })
}

/// Resolves the exact current source by its durable audit ID. Managed resolution
/// revalidates its descriptor, policy and content-addressed snapshot.
pub fn resolve_configured_catalogue(
    source_id: &str,
    inputs: CatalogueInventoryInputs<'_>,
) -> Result<super::catalogue_selection::ResolvedCatalogue, String> {
    let rows = configured_catalogues(inputs);
    let row = rows.get(source_id).ok_or_else(|| {
        "the catalogue is disabled, removed, unavailable or ambiguous".to_string()
    })?;
    resolve_catalogue(&row.reference, inputs).map_err(|error| error.to_string())
}
