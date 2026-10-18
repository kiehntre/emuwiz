//! Discovery comparison adapter over the authoritative managed catalogue resolver.
//! Never imports, audits, persists identity or grants Verified status.
use super::SpectrumCatalogue;
use crate::dat::{
    catalogue_selection::{
        CatalogueInventoryInputs, CatalogueRef, list_installed_catalogues, resolve_catalogue,
    },
    limits::DatLimits,
    managed_sources::{default_managed_dat_sources_config_path, load_managed_dat_sources_default},
    sources::DatSourceRegistry,
    updates::{managed_dat_root, managed_dat_state_path},
};
use crate::identity_source::hashing::FileFingerprint;

pub(super) fn load_managed(
    platforms: &[String],
    slots: usize,
    bytes: u64,
) -> Result<Vec<SpectrumCatalogue>, String> {
    let config = default_managed_dat_sources_config_path().map_err(|e| e.to_string())?;
    let guard = FileFingerprint::observe(&config);
    let managed = load_managed_dat_sources_default().map_err(|e| e.to_string())?;
    if FileFingerprint::observe(&config) != guard {
        return Err(
            "Managed catalogue configuration changed while loading. Discover again.".into(),
        );
    }
    let Some(guard) = guard else {
        return Ok(Vec::new());
    };
    let root = managed_dat_root().map_err(|e| e.to_string())?;
    load_from(
        CatalogueInventoryInputs {
            local_registry: &DatSourceRegistry::new(),
            managed_sources: &managed,
            managed_root: &root,
            limits: DatLimits {
                max_file_size: 32 * 1024 * 1024,
                ..Default::default()
            },
        },
        &guard,
        platforms,
        slots,
        bytes,
    )
}

fn load_from(
    inputs: CatalogueInventoryInputs<'_>,
    config: &FileFingerprint,
    platforms: &[String],
    slots: usize,
    mut bytes: u64,
) -> Result<Vec<SpectrumCatalogue>, String> {
    if FileFingerprint::observe(&config.path).as_ref() != Some(config) {
        return Err("Managed catalogue configuration changed. Discover again.".into());
    }
    let allowed: std::collections::HashSet<_> = inputs
        .managed_sources
        .descriptors()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|d| d.update_policy() != crate::dat::updates::ManagedDatUpdatePolicy::Disabled)
        .map(|d| d.source_id().clone())
        .collect();
    let mut result = Vec::new();
    for row in list_installed_catalogues(inputs).into_iter().filter(|row| {
        matches!(&row.reference, CatalogueRef::ManagedCurrent { source_id, .. } if allowed.contains(source_id)) && row.enabled && row.capabilities.verify
            && row.availability.is_ready() && row.platform.confirmed().and_then(|p| crate::canonical_platform_for_alias(p))
                .is_some_and(|p| platforms.is_empty() || platforms.iter().any(|wanted| wanted == p))
    }) {
        if result.len() >= slots { return Err("Too many catalogue inputs for this bounded preview.".into()); }
        let CatalogueRef::ManagedCurrent { source_id, .. } = &row.reference else { unreachable!() };
        // These are the existing Discovery input guards, not another managed
        // freshness policy. Keep descriptor/state selection bound until Apply;
        // the shared resolver remains the authority for trust and content hash.
        let state = managed_dat_state_path(inputs.managed_root, source_id).map_err(|e| e.to_string())?;
        let mut guards = vec![
            config.clone(),
            FileFingerprint::observe(&state).ok_or("Managed catalogue state unavailable")?,
        ];
        let path = row.technical_path.as_deref().ok_or("Managed catalogue backing path unavailable")?;
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if size > 32 * 1024 * 1024 || size > bytes { return Err("Catalogue inputs exceed the bounded preview limit.".into()); }
        bytes -= size;
        guards.push(FileFingerprint::observe(path).ok_or("Managed catalogue backing file unavailable")?);
        let resolved = resolve_catalogue(&row.reference, inputs).map_err(|e| e.to_string())?;
        let mut catalogue = SpectrumCatalogue::from_parsed_for_platform(&resolved.parsed)?;
        if row.platform.confirmed().and_then(|p| crate::canonical_platform_for_alias(p)) != Some(catalogue.platform.as_str()) {
            return Err("Managed catalogue metadata and parsed platform disagree; no comparison performed.".into());
        }
        if guards.iter().any(|g| FileFingerprint::observe(&g.path).as_ref() != Some(g)) {
            return Err("Managed catalogue changed during resolution. Discover again.".into());
        }
        catalogue.provenance = format!("{} | {:?}", row.reference.token(), row.provenance);
        catalogue.basis = format!("{} | {}", row.reference.token(), catalogue.basis);
        catalogue.inputs = guards.into_iter().map(|g| (g.path.clone(), g)).collect();
        result.push(catalogue);
    }
    if FileFingerprint::observe(&config.path).as_ref() != Some(config) {
        return Err(
            "Managed catalogue configuration changed during resolution. Discover again.".into(),
        );
    }
    Ok(result)
}

#[cfg(test)]
#[path = "discovery_catalogues/tests.rs"]
mod tests;
