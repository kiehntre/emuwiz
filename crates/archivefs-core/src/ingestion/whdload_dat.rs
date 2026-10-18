//! Scan-time WHDLoad association, using the existing bounded catalogue loader
//! and slave reconciler. No audit, identity persistence or Verified authority.
use super::preview::{SpectrumCatalogue, load_discovery_catalogues_for};
use crate::dat::{catalogue_selection::bounded_sha256, model::DatEcosystem};
use crate::identity_source::{
    hashing::FileFingerprint,
    whdload::{SlaveArtifact, WhdloadDatContext, reconcile_whdload_slaves},
};
use crate::platform_evidence_fusion::evidence_lineage::{SourceArtifactIdentity, SourceFamily};
use std::{collections::BTreeSet, path::Path};

struct Catalogue {
    catalogue: SpectrumCatalogue,
    artifact: SourceArtifactIdentity,
}
impl Catalogue {
    fn context(&self) -> WhdloadDatContext<'_> {
        WhdloadDatContext {
            index: &self.catalogue.index,
            source_family: self.artifact.source_family,
            upstream_version: self.catalogue.source.version.as_deref(),
            source_artifact: Some(&self.artifact),
        }
    }
    fn recheck(&self) -> Result<(), String> {
        if self
            .catalogue
            .inputs
            .iter()
            .any(|(p, g)| FileFingerprint::observe(p).as_ref() != Some(g))
        {
            return Err("WHDLoad catalogue or source configuration changed during scan; no catalogue association may be committed. Scan rolled back.".into());
        }
        Ok(())
    }
}

/// Loaded once per scan, only after a valid WHDLoad slave is found. Catalogue
/// parsing and hashing retain the existing 32-input / 32MiB / 128MiB bounds.
#[derive(Default)]
pub(crate) struct ScanCatalogues {
    loaded: Option<Result<Vec<Catalogue>, String>>,
}
impl ScanCatalogues {
    fn load(catalogues: Vec<SpectrumCatalogue>) -> Result<Vec<Catalogue>, String> {
        let mut result = Vec::new();
        for catalogue in catalogues {
            let path = Path::new(&catalogue.source.file_path);
            if !catalogue.inputs.iter().any(|(p, _)| p == path) {
                return Err("Catalogue bytes have no retained source fingerprint.".into());
            }
            let sha256 = bounded_sha256(path, 32 * 1024 * 1024).map_err(|e| e.to_string())?;
            let family = match catalogue.source.ecosystem {
                DatEcosystem::Tosec => SourceFamily::TOSEC,
                DatEcosystem::NoIntro => SourceFamily::NoIntro,
                DatEcosystem::Redump => SourceFamily::Redump,
                DatEcosystem::FBNeo => SourceFamily::FBNeo,
                DatEcosystem::MAMEArcade => SourceFamily::MAMEArcade,
                DatEcosystem::MAMESoftwareList => SourceFamily::MAMESoftwareList,
                _ => SourceFamily::Unknown,
            };
            let artifact = SourceArtifactIdentity {
                source_family: family,
                upstream_version: catalogue.source.version.clone(),
                artifact_sha256: Some(sha256),
                // The loader preserves the managed reference token, or the
                // local catalogue name/version/path. Never infer ownership.
                artifact_name: Some(format!("{} | {}", catalogue.provenance, catalogue.basis)),
            };
            let entry = Catalogue {
                catalogue,
                artifact,
            };
            entry.recheck()?;
            result.push(entry);
        }
        Ok(result)
    }
    fn annotate(&mut self, slaves: &[SlaveArtifact]) -> String {
        let loaded = self.loaded.get_or_insert_with(|| {
            load_discovery_catalogues_for(&["Amiga".into()]).and_then(Self::load)
        });
        match loaded {
            Ok(entries) if !entries.is_empty() => {
                let contexts: Vec<_> = entries.iter().map(Catalogue::context).collect();
                explain(slaves, &contexts)
            }
            Ok(_) => " No current validated Amiga DAT context is available; this is structural WHDLoad evidence only. Package and installed game content are not verified.".into(),
            Err(error) => format!(" WHDLoad DAT context unavailable: {error}. Structural evidence only; package and installed game content are not verified."),
        }
    }
    /// Called inside the existing scan transaction before it can commit.
    pub(crate) fn recheck(&self) -> Result<(), String> {
        #[cfg(test)]
        BEFORE_COMMIT_FAULT.with(|fault| {
            if let Some(fault) = fault.borrow_mut().take() {
                fault();
            }
        });
        if let Some(Ok(entries)) = &self.loaded {
            for entry in entries {
                entry.recheck()?;
            }
        }
        Ok(())
    }
}

pub(crate) fn discover(
    root: &Path,
    fingerprints: &[(i64, crate::ScanFingerprint)],
    catalogues: &mut ScanCatalogues,
) -> Result<super::SourceDiscoveryReport, super::DiscoveryError> {
    super::discovery::discover_with_annotations(root, fingerprints, &mut |slaves| {
        catalogues.annotate(slaves)
    })
}

/// Combine the existing single-catalogue reconciliation without selecting a
/// winner across catalogues or changing a structural discovery's authority.
pub(super) fn explain(slaves: &[SlaveArtifact], contexts: &[WhdloadDatContext<'_>]) -> String {
    if contexts.is_empty() {
        return String::new();
    }
    let mut releases = BTreeSet::new();
    let mut sources = Vec::new();
    let mut ambiguous = false;
    for context in contexts {
        let result = reconcile_whdload_slaves(slaves, Some(context));
        ambiguous |= result.ambiguous;
        let hits: Vec<_> = result
            .slaves
            .iter()
            .filter_map(|s| s.matched_release.clone())
            .collect();
        if !hits.is_empty() || result.ambiguous {
            releases.extend(hits);
            sources.push(format!(
                "{}; catalogue SHA-256 {}; version {}",
                context
                    .source_artifact
                    .and_then(|a| a.artifact_name.as_deref())
                    .unwrap_or("source not recorded"),
                context
                    .source_artifact
                    .and_then(|a| a.artifact_sha256.as_deref())
                    .unwrap_or("not recorded"),
                context.upstream_version.unwrap_or("not recorded")
            ));
        }
    }
    let association = if ambiguous || releases.len() > 1 {
        format!(
            "Conflicting WHDLoad catalogue associations ({:?}); no release selected.",
            releases
        )
    } else if let Some(release) = releases.first() {
        format!("Exact catalogue match: {release}.")
    } else {
        "No exact whole-.slave SHA-1 catalogue match; structural WHDLoad evidence only.".into()
    };
    let hashes: Vec<_> = slaves
        .iter()
        .map(|s| format!("{}={}", s.name, s.hashes.sha1))
        .collect();
    format!(
        " {association} At this scan, whole-.slave SHA-1 evidence: {}. {} Package and installed game content are not verified. WHDLoad metadata and filenames alone are not exact content evidence.",
        hashes.join(", "),
        sources.join(" | ")
    )
}

#[cfg(test)]
#[path = "whdload_dat/tests.rs"]
mod tests;

#[cfg(test)]
thread_local! {
    static BEFORE_COMMIT_FAULT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
