//! Cross-catalogue discovery for Check Games (owner 2026-11-26/27): games
//! whose own system's catalogue did not match them are checked against the
//! catalogues of *other* systems, so a misfiled game (the real case: NES
//! games in the SNES folder) can be found and offered as a Platform
//! Correction.
//!
//! Read-only from start to finish. Each other catalogue is audited against
//! exactly the unmatched files it could list (by format), the outcome goes
//! into [`crate::Database::preview_platform_corrections`], and nothing is
//! saved: no result, no system, no file. Folder and system guesses never
//! count as evidence; only the preview's strong-evidence rules decide what
//! is Ready, Needs review or Ambiguous.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::dat::catalogue_selection::{
    CatalogueInventoryInputs, ResolvedCatalogue, list_installed_catalogues, resolve_catalogue,
};
use crate::dat::library_verification::catalogue_fits_platform;
use crate::dat::model::DatEcosystem;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{DatAuditError, run_dat_audit_with_targets};
use crate::dat::sources::audit_targets::{AuditTargets, catalogue_audit_targets};
use crate::database::PlatformCorrectionPreview;
use crate::safe_read::TrustedRoots;
use crate::{ArchiveFsError, Result};

/// Archive containers whose members a catalogue may list under any format.
const ARCHIVE_FORMATS: &[&str] = &["zip", "7z", "rar"];

/// A game the own-system check did not match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmatchedGame {
    pub path: PathBuf,
    /// The game's current (canonical) system.
    pub platform: String,
}

/// A catalogue of another system, resolved and trusted by the caller.
pub struct DiscoveryCatalogue {
    pub resolved: ResolvedCatalogue,
    /// The catalogue's canonical system.
    pub platform: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryProgress {
    Catalogue {
        index: usize,
        of: usize,
        name: String,
        files: usize,
    },
}

/// What discovery found, per catalogue that matched anything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscoveryOutcome {
    pub previews: Vec<PlatformCorrectionPreview>,
    /// Catalogues that could not be used, and why (never silently dropped).
    pub skipped: Vec<(String, String)>,
    pub catalogues_checked: usize,
}

/// The file formats a catalogue lists with a hash that can match.
fn listed_formats(resolved: &ResolvedCatalogue) -> BTreeSet<String> {
    resolved
        .parsed
        .games
        .iter()
        .flat_map(|g| &g.roms)
        .filter(|r| r.loadflag.is_none() && (r.md5.is_some() || r.sha1.is_some()))
        .filter_map(|r| {
            Path::new(&r.name)
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
        })
        .collect()
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// The installed catalogues of every system other than `own_platform`
/// that can verify on their own: enabled, ready, a single game catalogue
/// (never BIOS or a folder) with one confirmed or assigned system. MAME
/// arcade sets are left to the MAME tools. Anything not usable is listed in
/// the second value with its reason.
pub fn other_system_catalogues(
    inputs: CatalogueInventoryInputs<'_>,
    own_platform: &str,
) -> (Vec<DiscoveryCatalogue>, Vec<(String, String)>) {
    let own = crate::canonical_platform_for_alias(own_platform);
    let mut chosen = Vec::new();
    let mut skipped = Vec::new();
    for summary in list_installed_catalogues(inputs) {
        let Some(platform) = summary
            .platform
            .confirmed()
            .and_then(|p| crate::canonical_platform_for_alias(p))
        else {
            continue;
        };
        if Some(platform) == own || platform == "Arcade" {
            continue;
        }
        if !summary.enabled
            || !summary.capabilities.verify
            || !summary.capabilities.single_catalogue_1g1r
        {
            continue;
        }
        match resolve_catalogue(&summary.reference, inputs) {
            Ok(resolved) if resolved.ecosystem == DatEcosystem::MAMEArcade => {}
            Ok(resolved) => chosen.push(DiscoveryCatalogue {
                resolved,
                platform: platform.to_string(),
            }),
            Err(error) => skipped.push((summary.display_name, error.to_string())),
        }
    }
    (chosen, skipped)
}

/// What one discovery run checks.
pub struct DiscoveryRequest<'a> {
    pub database: &'a Path,
    pub games: &'a [UnmatchedGame],
    pub catalogues: &'a [DiscoveryCatalogue],
    pub trusted: &'a TrustedRoots,
    /// The library generation the preview is bound to.
    pub generation: u64,
    /// Holds a run-private hash cache, so each loose file is read once
    /// whatever the number of catalogues.
    pub work_dir: &'a Path,
}

/// Checks the request's games against every catalogue that is for a
/// different system, and previews the corrections the matches support.
pub fn discover_other_system_matches(
    request: &DiscoveryRequest<'_>,
    cancel: &AtomicBool,
    progress: &dyn Fn(DiscoveryProgress),
) -> Result<DiscoveryOutcome> {
    let DiscoveryRequest {
        database,
        games,
        catalogues,
        trusted,
        generation,
        work_dir,
    } = *request;
    let mut outcome = DiscoveryOutcome::default();
    let db = crate::Database::open_read_only(database)?;
    let cache = AuditCacheConfig::At(work_dir.join("discovery-hashes.json"));
    for (index, catalogue) in catalogues.iter().enumerate() {
        if cancel.load(Ordering::Acquire) {
            return Err(ArchiveFsError::Database(
                "Wrong-system check cancelled; nothing was changed.".into(),
            ));
        }
        let name = catalogue.resolved.display_name.clone();
        if let Err(why) = catalogue_fits_platform(&catalogue.resolved, &catalogue.platform) {
            outcome.skipped.push((name, why));
            continue;
        }
        let formats = listed_formats(&catalogue.resolved);
        let paths: Vec<PathBuf> = games
            .iter()
            .filter(|g| {
                crate::canonical_platform_for_alias(&g.platform)
                    != crate::canonical_platform_for_alias(&catalogue.platform)
            })
            .filter(|g| {
                let ext = extension(&g.path);
                formats.contains(&ext) || ARCHIVE_FORMATS.contains(&ext.as_str())
            })
            .map(|g| g.path.clone())
            .collect();
        progress(DiscoveryProgress::Catalogue {
            index: index + 1,
            of: catalogues.len(),
            name: name.clone(),
            files: paths.len(),
        });
        if paths.is_empty() {
            continue;
        }
        let mut audit_request =
            catalogue
                .resolved
                .to_dat_audit_request(PathBuf::from("/"), Default::default(), None);
        // The catalogue's own system: header removal (e.g. iNES) and the
        // result's provenance depend on it.
        audit_request.platform = Some(catalogue.platform.clone());
        let targets = AuditTargets::Catalogue(catalogue_audit_targets(paths, Path::new("/")));
        let audit = match run_dat_audit_with_targets(
            &audit_request,
            trusted,
            cancel,
            &|_| {},
            cache.clone(),
            &targets,
        ) {
            Ok(audit) => audit,
            Err(DatAuditError::Cancelled) => {
                return Err(ArchiveFsError::Database(
                    "Wrong-system check cancelled; nothing was changed.".into(),
                ));
            }
            Err(error) => {
                outcome.skipped.push((name, error.to_string()));
                continue;
            }
        };
        outcome.catalogues_checked += 1;
        let preview = db.preview_platform_corrections(&audit, generation)?;
        if !preview.candidates.is_empty() {
            outcome.previews.push(preview);
        }
    }
    Ok(outcome)
}

#[cfg(test)]
#[path = "platform_discovery_tests.rs"]
mod tests;
