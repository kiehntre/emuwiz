//! Bootstrap native saved Discovery from a bounded, explicitly reviewed path list.
//! Input is read-only evidence, never a deserialized write capability. No root walk.
use super::{
    bounded_import::*,
    preview::*,
    saved_discovery::{MAX_SELECTED, SubsetPreview},
};
use crate::{
    Archive, ArchiveKind, Database,
    identity_source::hashing::{LocalHashes, hash_file},
    safe_read::TrustedRoots,
};
use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedPath {
    pub path: PathBuf,
    /// Expected canonical platform; native recognition must independently agree.
    pub platform: String,
    /// Preserved full content and filesystem identity, where previous evidence exists.
    pub expected: Option<LocalHashes>,
}

/// Read-only bootstrap; native saved-subset validation performs final recognition,
/// reference/member-copy checks, companion checks, and authority/freshness sealing.
/// No Apply is performed, and no filesystem enumeration is reachable here.
pub fn preview_paths(
    database: &Path,
    config: &Path,
    root: &Path,
    selected: &[ReviewedPath],
    exclusions: &[(PathBuf, String)],
    catalogues: &[SpectrumCatalogue],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<SubsetPreview, String> {
    if selected.is_empty() || selected.len() > MAX_SELECTED {
        return Err("Explicit preview requires 1 to 1000 reviewed paths.".into());
    }
    let safe = |p: &Path| {
        p.is_absolute()
            && p.starts_with(root)
            && !p
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    };
    let mut paths = BTreeSet::new();
    if !safe(root)
        || selected
            .iter()
            .any(|r| !safe(&r.path) || !paths.insert(r.path.clone()))
    {
        return Err("Duplicate or escaping selected paths; no preview prepared.".into());
    }
    let mut excluded = BTreeSet::new();
    if exclusions.len() > 100_000
        || exclusions
            .iter()
            .any(|(p, r)| !safe(p) || r.is_empty() || !excluded.insert(p.clone()))
    {
        return Err("Invalid reviewed exclusions.".into());
    }
    if selected
        .iter()
        .any(|r| crate::canonical_platform_for_alias(&r.platform) != Some(r.platform.as_str()))
    {
        return Err("Reviewed candidates must name an existing canonical platform.".into());
    }
    let db = Database::open_read_only(database).map_err(|e| e.to_string())?;
    let context = begin_bounded_preview(&db, config)?;
    let source = context
        .sources
        .iter()
        .filter(|s| root.starts_with(&s.path))
        .max_by_key(|s| s.path.components().count())
        .ok_or("No configured source authority")?;
    let source_root = source.path.clone();
    let binding =
        crate::catalogue_health::BoundRoot::open(root).ok_or("Selected root is unavailable")?;
    let mut preview = MediaPreview::default();
    preview.import_evidence = Some(ImportEvidence {
        observed_at: Instant::now(),
        root: root.into(),
        binding: binding.binding.clone(),
        known: db
            .load_archives()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|a| a.absolute_path.starts_with(root))
            .collect(),
        candidates: vec![],
        references: catalogues
            .iter()
            .flat_map(|c| c.inputs.iter().cloned())
            .collect(),
        exclusions: exclusions.to_vec(),
        complete: catalogues.iter().all(|c| !c.inputs.is_empty()),
        catalogue_basis: catalogues.iter().map(|c| c.basis.clone()).collect(),
    });
    // Validate source roles/binding/overrides and database identity before reading media.
    prepare_bounded_import(&preview, context.clone())?;
    let trusted = TrustedRoots::from_paths([root]);
    let mut bytes = 0u64;
    let mut rejected = Vec::new();
    let mut charge = |path: &Path| -> Result<(), String> {
        let size = crate::identity_source::hashing::FileFingerprint::observe(path)
            .ok_or("Missing or inaccessible candidate")?
            .size_bytes;
        bytes = bytes.checked_add(size).ok_or("Read budget overflow")?;
        if bytes > MAX_COMPARISON_INPUT_BYTES {
            return Err("Explicit-path read budget exceeded".into());
        }
        Ok(())
    };
    for (n, r) in selected.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Preview cancelled; no database changes.".into());
        }
        progress(n, selected.len());
        let checked = (|| -> Result<Candidate, String> {
            if excluded.contains(&r.path) {
                return Err("Excluded by reviewed evidence; cannot be selected.".into());
            }
            if preview
                .import_evidence
                .as_ref()
                .unwrap()
                .known
                .iter()
                .any(|a| a.absolute_path == r.path)
            {
                return Err("Already represented by a protected library record.".into());
            }
            charge(&r.path)?;
            // Existing factory handles supported formats and structural validation.
            let a = Archive::from_path_in_root(&r.path, &source_root).ok_or_else(|| {
                super::discovery::inspect_spectrum_media(&r.path, root)
                    .map(|d| d.explanation)
                    .unwrap_or_else(|| {
                        "Unsupported media or existing parser refused this path.".into()
                    })
            })?;
            if a.kind == ArchiveKind::ArcadeSetDirectory
                || a.identity
                    .platform
                    .as_deref()
                    .and_then(crate::canonical_platform_for_alias)
                    != Some(r.platform.as_str())
            {
                return Err(
                    "Native platform is unknown/ambiguous or conflicts with reviewed platform."
                        .into(),
                );
            }
            let hashes = hash_file(&r.path, &trusted, Some(cancel))
                .map_err(|e| format!("Candidate read refused: {e:?}"))?;
            if r.expected.as_ref().is_some_and(|h| *h != hashes) {
                return Err(
                    "Content or filesystem identity changed from reviewed evidence.".into(),
                );
            }
            let mut parts = vec![];
            for p in companions(&r.path)? {
                if !safe(&p) || excluded.contains(&p) {
                    return Err("Companion outside scope or explicitly excluded.".into());
                }
                charge(&p)?;
                parts.push(
                    hash_file(&p, &trusted, Some(cancel))
                        .map_err(|e| format!("Companion refused: {e:?}"))?,
                );
            }
            let ids = identities(&hashes, catalogues, &r.platform);
            if ids.len() > 1 {
                return Err("Ambiguous exact catalogue identities; none selected.".into());
            }
            if r.platform == "ZX Spectrum" && !catalogues.iter().any(|c| c.platform == r.platform) {
                return Err(
                    "Spectrum relationship review requires its fresh validated catalogues.".into(),
                );
            }
            Ok(Candidate { hashes, platform: r.platform.clone(), kind: a.kind.storage_name().into(), companions: parts,
                alternate: ids.first().map(|i| format!("Exact catalogue release: {}. Independent representation retained; no automatic merge.", i.game)),
                multipart: ids.first().and_then(|i| i.multipart.clone()), ambiguous: false, relationships_complete: true })
        })();
        match checked {
            Ok(c) => preview.import_evidence.as_mut().unwrap().candidates.push(c),
            Err(reason) => {
                rejected.push((r.path.clone(), reason.clone()));
                if excluded.insert(r.path.clone()) {
                    preview
                        .import_evidence
                        .as_mut()
                        .unwrap()
                        .exclusions
                        .push((r.path.clone(), reason));
                }
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Preview cancelled; no database changes.".into());
    }
    let seed = prepare_bounded_import(&preview, context)?;
    let ids: Vec<_> = seed.selection_rows().into_iter().map(|r| r.id).collect();
    if ids.is_empty() {
        return Ok(SubsetPreview {
            plan: seed,
            saved_eligible: 0,
            requested: selected.len(),
            inspected_input_bytes: bytes,
            rejected,
            unselected: 0,
        });
    }
    let mut result = seed
        .saved_evidence()?
        .prepare_subset(&ids, database, config, cancel, progress)?;
    result.requested = selected.len();
    result.inspected_input_bytes = result.inspected_input_bytes.saturating_add(bytes);
    result.rejected.splice(0..0, rejected);
    Ok(result)
}

/// Pinned, bounded path input. Unlike saved native evidence this is only a read
/// request: each selected item must pass fresh native validation before sealing.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitPreviewRequest {
    pub root: PathBuf,
    pub selected: Vec<ReviewedPath>,
    pub exclusions: Vec<(PathBuf, String)>,
    #[serde(default)]
    pub catalogues: Vec<ReviewedCatalogue>,
}
pub fn load_explicit_request(path: &Path, pin: &str) -> Result<ExplicitPreviewRequest, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Supply the separately retained request SHA-256.".into());
    }
    const LIMIT: u64 = 16 * 1024 * 1024;
    let safe = crate::safe_read::open_bounded_read(path, &TrustedRoots::none())
        .map_err(|e| format!("Request read refused: {e:?}"))?;
    let mut bytes = vec![];
    safe.into_file()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > LIMIT {
        return Err("Explicit request exceeds 16 MiB.".into());
    }
    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !actual.eq_ignore_ascii_case(pin) {
        return Err("Explicit request SHA-256 changed.".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid explicit request: {e}"))
}
pub fn preview_pinned_paths(
    database: &Path,
    config: &Path,
    request: &Path,
    pin: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<SubsetPreview, String> {
    let request = load_explicit_request(request, pin)?;
    let platforms: Vec<_> = request
        .selected
        .iter()
        .map(|r| r.platform.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let catalogues = if request.catalogues.is_empty() {
        load_discovery_catalogues_for(&platforms)?
    } else {
        load_reviewed_catalogues_from(
            &crate::dat::sources::default_dat_sources_config_path().map_err(|e| e.to_string())?,
            &request.catalogues,
            &platforms,
        )?
    };
    preview_paths(
        database,
        config,
        &request.root,
        &request.selected,
        &request.exclusions,
        &catalogues,
        cancel,
        progress,
    )
}

/// Explicit installed catalogue choice, keyed by registration identity and a
/// separately preserved whole-file digest. Display names never grant trust.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedCatalogue {
    pub source_id: String,
    pub sha256: String,
}
/// Fresh read-only validation of explicitly pinned, enabled registered files.
/// TOSEC pack registration intentionally leaves legacy health/platform fields
/// unset; typed source identity + full digest + parsed platform remain usable.
pub fn load_reviewed_catalogues_from(
    registry_path: &Path,
    choices: &[ReviewedCatalogue],
    platforms: &[String],
) -> Result<Vec<SpectrumCatalogue>, String> {
    use crate::dat::sources::{DatSourceKind, DatSourceRegistry};
    use crate::identity_source::hashing::FileFingerprint;
    let config_generation =
        FileFingerprint::observe(registry_path).ok_or("Catalogue registry unavailable")?;
    let config = crate::dat::sources::load_dat_sources_config_from(registry_path)
        .map_err(|e| e.to_string())?;
    let (registry, problems) = DatSourceRegistry::from_config(&config);
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    let mut seen = BTreeSet::new();
    if choices.len() > 32
        || choices.iter().any(|c| {
            !seen.insert(&c.source_id)
                || c.sha256.len() != 64
                || !c.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err("Invalid or repeated pinned catalogue choices (maximum 32).".into());
    }
    let mut result = Vec::new();
    let mut bytes = 0u64;
    for choice in choices {
        let source = registry
            .entries()
            .iter()
            .find(|s| s.id == choice.source_id)
            .ok_or("Pinned catalogue registration is missing")?;
        if !source.enabled || source.kind != DatSourceKind::File {
            return Err("Pinned catalogue is disabled or not a registered file.".into());
        }
        let generation =
            FileFingerprint::observe(&source.path).ok_or("Pinned catalogue unavailable")?;
        bytes = bytes
            .checked_add(generation.size_bytes)
            .ok_or("Catalogue size overflow")?;
        if generation.size_bytes > 32 * 1024 * 1024 || bytes > 128 * 1024 * 1024 {
            return Err("Pinned catalogue inputs exceed preview limits.".into());
        }
        let hash = crate::repair::exact_duplicate::hash_full_file_sha256(
            &source.path,
            &TrustedRoots::none(),
            None,
        )
        .map_err(|e| format!("Catalogue read refused: {e:?}"))?;
        if !hash.sha256.eq_ignore_ascii_case(&choice.sha256) {
            return Err("Pinned catalogue content changed.".into());
        }
        let parsed = crate::dat::parsers::parse_dat_file(
            &source.path,
            crate::dat::limits::DatLimits {
                max_file_size: 32 * 1024 * 1024,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        if parsed.warnings.iter().any(|w| !w.is_note()) {
            return Err("Pinned catalogue parse warnings require review.".into());
        }
        let mut catalogue = SpectrumCatalogue::from_parsed_for_platform(&parsed.dat)?;
        if !platforms.contains(&catalogue.platform)
            || source.platform.as_deref().is_some_and(|p| {
                crate::canonical_platform_for_alias(p) != Some(catalogue.platform.as_str())
            })
        {
            return Err(
                "Pinned catalogue platform conflicts with reviewed scope or registration.".into(),
            );
        }
        if FileFingerprint::observe(&source.path).as_ref() != Some(&generation) {
            return Err("Pinned catalogue changed while parsing.".into());
        }
        catalogue.basis = format!(
            "{} | registered source {} | SHA-256 {}",
            catalogue.basis, source.id, choice.sha256
        );
        catalogue.inputs = vec![
            (source.path.clone(), generation),
            (registry_path.into(), config_generation.clone()),
        ];
        result.push(catalogue);
    }
    if FileFingerprint::observe(registry_path).as_ref() != Some(&config_generation) {
        return Err("Catalogue registration changed during preview.".into());
    }
    Ok(result)
}
