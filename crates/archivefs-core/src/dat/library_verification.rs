//! Bounded library verification over the existing catalogue resolver and audit engine.
//! No platform discovery, folder-walk fallback, media conversion or set-completeness claim.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use super::catalogue_selection::ResolvedCatalogue;
use super::identity::{DatPlatformIdentity, identify_dat_source};
use super::library_identity_projection::project_dat_audit_for_library_identity;
use super::library_identity_summary::DatVerificationState;
use super::limits::DatLimits;
use super::model::{DatEcosystem, DatPackingPolicy};
use super::parsers::parse_dat_file;
use super::sources::DatSourceKind;
use super::sources::audit_cache::AuditCacheConfig;
use super::sources::audit_run::{
    DatAuditOutcome, DatAuditProgress, DatAuditRequest, run_dat_audit_with_targets,
};
use super::sources::audit_targets::{AuditTargets, CatalogueAuditTargets};
use crate::Database;
use crate::database::PersistedArchive;
use crate::safe_read::TrustedRoots;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct VerificationMember {
    pub id: i64,
    pub path: PathBuf,
    pub platform: Option<String>,
}

fn membership(rows: &[PersistedArchive]) -> Vec<VerificationMember> {
    let mut members: Vec<_> = rows
        .iter()
        .map(|r| VerificationMember {
            id: r.id,
            path: r.absolute_path.clone(),
            platform: r.platform.clone(),
        })
        .collect();
    members.sort();
    members
}

/// A preview-bound membership guard; its fields cannot be widened by callers.
#[derive(Debug)]
pub struct VerificationScope {
    members: Vec<VerificationMember>,
    eligible: BTreeMap<PathBuf, i64>,
    platform: String,
    source_id: String,
    pub(crate) catalogue_sha256: String,
}
impl VerificationScope {
    pub(crate) fn validate(
        &self,
        rows: &[PersistedArchive],
        outcome: &DatAuditOutcome,
    ) -> Result<(), String> {
        if membership(rows) != self.members {
            return Err(
                "Library membership changed. Preview again; no verification results were saved."
                    .into(),
            );
        }
        if outcome.catalogue_sha256.as_deref() != Some(self.catalogue_sha256.as_str())
            || outcome.source_id != self.source_id
            || outcome.platform.as_deref() != Some(self.platform.as_str())
            || outcome
                .report
                .entries
                .iter()
                .any(|e| !self.eligible.contains_key(Path::new(&e.local_path)))
            || outcome
                .archives
                .iter()
                .any(|a| !self.eligible.contains_key(&a.archive_path))
        {
            return Err("Audit outcome is outside the confirmed platform population.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct VerificationExclusion {
    pub path: PathBuf,
    pub reason: String,
}

pub struct VerificationPreview {
    pub platform: String,
    pub catalogue_name: String,
    pub version: Option<String>,
    pub source: String,
    pub registration: String,
    pub sha256: String,
    pub catalogue_path: PathBuf,
    pub collection: Option<PathBuf>,
    pub selected_games: usize,
    pub eligible_games: usize,
    pub bytes: u64,
    pub formats: BTreeSet<String>,
    /// Formats for which every catalogue ROM has comparable loose-file hashes.
    pub negative_formats: BTreeSet<String>,
    pub exclusions: Vec<VerificationExclusion>,
    pub wrong_system: usize,
    pub unknown_system: usize,
    pub outside_collection: usize,
    scope: VerificationScope,
    database_path: PathBuf,
    request: DatAuditRequest,
    fingerprints: BTreeMap<PathBuf, std::fs::Metadata>,
    // Private catalogue bytes live through the run and are never a user write target.
    _snapshot: tempfile::TempDir,
}

fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(path: &Path) -> Result<String, String> {
    let mut file = crate::safe_read::open_bounded_read(path, &TrustedRoots::none())
        .map_err(|e| e.detail())?
        .into_file();
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex(&hasher.finalize()))
}
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.is_file()
        && b.is_file()
        && a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

/// Metadata-only ROM population preview; DAT bytes are frozen and parsed on the worker.
pub fn preview_library_verification(
    resolved: ResolvedCatalogue,
    database_path: PathBuf,
    platform: String,
    collection: Option<PathBuf>,
) -> Result<VerificationPreview, String> {
    if resolved.kind != DatSourceKind::File {
        return Err("Choose one installed DAT file, not an aggregate folder.".into());
    }
    let canonical =
        crate::canonical_platform_for_alias(&platform).ok_or("Choose a known system.")?;
    if canonical != platform {
        return Err("Choose a canonical system.".into());
    }
    let limits = DatLimits::default();
    let snapshot = tempfile::tempdir().map_err(|e| e.to_string())?;
    let frozen = snapshot.path().join("catalogue.dat");
    let mut bytes = Vec::new();
    crate::safe_read::open_bounded_read(&resolved.backing_path, &TrustedRoots::none())
        .map_err(|e| e.detail())?
        .into_file()
        .take(limits.max_file_size + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limits.max_file_size {
        return Err("Catalogue exceeds the parser limit.".into());
    }
    let sha256 = hex(&Sha256::digest(&bytes));
    std::fs::write(&frozen, bytes).map_err(|e| e.to_string())?;
    let parsed = parse_dat_file(&frozen, limits)
        .map_err(|e| e.to_string())?
        .dat;
    if parsed.source.ecosystem != resolved.ecosystem
        || parsed.source.name != resolved.parsed.source.name
        || digest(&resolved.backing_path)? != sha256
    {
        return Err("Catalogue changed while preparing the preview. Preview again.".into());
    }
    if let super::catalogue_selection::CatalogueRef::ManagedCurrent {
        snapshot_sha256, ..
    } = &resolved.reference
        && snapshot_sha256 != &sha256
    {
        return Err("Managed catalogue snapshot changed.".into());
    }
    match identify_dat_source(&parsed) {
        DatPlatformIdentity::Resolved { platform: p, .. } if p == platform => {}
        _ => return Err(
            "Catalogue platform is unknown, ambiguous or incompatible with the selected system."
                .into(),
        ),
    }
    if resolved
        .assigned_platform
        .as_deref()
        .and_then(crate::canonical_platform_for_alias)
        .is_some_and(|p| p != platform)
    {
        return Err("Assigned and parsed catalogue platforms disagree.".into());
    }
    if parsed.source.ecosystem == DatEcosystem::MAMEArcade {
        return Err(
            "Use the existing MAME collection health workflow for arcade set verification.".into(),
        );
    }
    let formats: BTreeSet<_> = parsed
        .games
        .iter()
        .flat_map(|g| &g.roms)
        .filter(|r| r.loadflag.is_none() && (r.md5.is_some() || r.sha1.is_some()))
        .map(|r| ext(Path::new(&r.name)))
        .filter(|e| !e.is_empty())
        .collect();
    let weak_formats: BTreeSet<_> = parsed
        .games
        .iter()
        .flat_map(|g| &g.roms)
        .filter(|r| r.loadflag.is_some() || (r.md5.is_none() && r.sha1.is_none()))
        .map(|r| ext(Path::new(&r.name)))
        .collect();
    let negative_formats = if weak_formats.contains("") {
        BTreeSet::new()
    } else {
        formats.difference(&weak_formats).cloned().collect()
    };
    if formats.is_empty() {
        return Err("This catalogue has no supported ROM content hashes. Names, CRC-only, SHA-256-only and disk-only catalogues cannot provide this verification.".into());
    }
    let db = Database::open_read_only(&database_path).map_err(|e| e.to_string())?;
    let rows = db.load_archives().map_err(|e| e.to_string())?;
    let mut path_counts = BTreeMap::new();
    for row in &rows {
        *path_counts
            .entry(row.absolute_path.clone())
            .or_insert(0usize) += 1;
    }
    let mut preview = VerificationPreview {
        platform: platform.clone(),
        catalogue_name: parsed
            .source
            .name
            .clone()
            .unwrap_or(resolved.display_name.clone()),
        version: parsed.source.version.clone(),
        source: format!(
            "{} · {} · {}",
            parsed.source.ecosystem.label(),
            parsed
                .source
                .author
                .as_deref()
                .unwrap_or("author not stated"),
            parsed
                .source
                .homepage
                .as_deref()
                .unwrap_or("source not stated")
        ),
        registration: match &resolved.provenance {
            super::catalogue_selection::CatalogueProvenance::UserRegistered => {
                "User-registered local DAT; verify its publisher before confirming".into()
            }
            super::catalogue_selection::CatalogueProvenance::NoIntroPackProjection => {
                "Imported No-Intro pack".into()
            }
            super::catalogue_selection::CatalogueProvenance::TosecReleasePackProjection {
                pack_id,
            } => format!("Imported TOSEC release pack {pack_id}"),
            super::catalogue_selection::CatalogueProvenance::EmuWizManaged { provider } => {
                format!("Managed installed catalogue ({})", provider.label())
            }
        },
        sha256: sha256.clone(),
        catalogue_path: resolved.backing_path.clone(),
        collection: collection.clone(),
        selected_games: 0,
        eligible_games: 0,
        bytes: 0,
        formats,
        negative_formats,
        exclusions: vec![],
        wrong_system: 0,
        unknown_system: 0,
        outside_collection: 0,
        scope: VerificationScope {
            members: membership(&rows),
            eligible: BTreeMap::new(),
            platform: platform.clone(),
            source_id: resolved.source_id.clone(),
            catalogue_sha256: sha256.clone(),
        },
        database_path,
        request: resolved.to_dat_audit_request(PathBuf::from("/"), limits, None),
        fingerprints: BTreeMap::new(),
        _snapshot: snapshot,
    };
    preview.request.dat_path = frozen;
    preview.request.platform = Some(platform);
    for row in rows {
        let assigned = row
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias);
        if assigned != Some(preview.platform.as_str()) {
            if assigned.is_none() {
                preview.unknown_system += 1;
            } else {
                preview.wrong_system += 1;
            }
            continue;
        }
        if collection
            .as_ref()
            .is_some_and(|root| !row.absolute_path.starts_with(root))
        {
            preview.outside_collection += 1;
            continue;
        }
        preview.selected_games += 1;
        let path = row.absolute_path;
        let metadata = std::fs::symlink_metadata(&path);
        let reason = if path_counts[&path] != 1 {
            Some("Ambiguous library path shared by multiple rows")
        } else if metadata.as_ref().is_err() {
            Some("Unavailable file")
        } else if !metadata.as_ref().unwrap().is_file() {
            Some("Unsupported directory, symlink or special file")
        } else if path.to_str().is_none() {
            Some("Unsupported non-Unicode path in the current audit engine")
        } else if {
            use std::os::unix::fs::PermissionsExt;
            metadata.as_ref().unwrap().permissions().mode() & 0o444 == 0
        } {
            Some("Unreadable file permissions")
        } else if crate::ingestion::discovery::is_known_non_game_extension(&ext(&path)) {
            Some("Unrelated support file")
        } else if !preview.formats.contains(&ext(&path))
            && !(parsed.source.packing_policy != DatPackingPolicy::FileOnly
                && matches!(ext(&path).as_str(), "zip" | "7z"))
        {
            Some("Format not represented by this catalogue or unsupported container")
        } else {
            None
        };
        if let Some(reason) = reason {
            preview.exclusions.push(VerificationExclusion {
                path,
                reason: reason.into(),
            });
            continue;
        }
        let metadata = metadata.unwrap();
        preview.bytes += metadata.len();
        preview.eligible_games += 1;
        preview.fingerprints.insert(path.clone(), metadata);
        preview.scope.eligible.insert(path, row.id);
    }
    Ok(preview)
}

#[derive(Clone, Debug, Default)]
pub struct VerificationTotals {
    pub catalogue: String,
    pub catalogue_sha256: String,
    pub unchecked: Vec<VerificationExclusion>,
    pub verified: usize,
    pub no_match: usize,
    pub ambiguous: usize,
    pub not_checked: usize,
    pub errors: usize,
    pub saved: usize,
    pub protected_prior: usize,
    /// The disc-order refresh that follows a successful save. An error here
    /// never undoes the saved evidence.
    pub disc_order: Option<Result<crate::DatDiscOrderOutcome, String>>,
}

impl VerificationPreview {
    /// Exact physical paths in this confirmed library population (no folder walk).
    pub fn eligible_paths(&self) -> impl Iterator<Item = &Path> {
        self.scope.eligible.keys().map(PathBuf::as_path)
    }

    fn validate_inputs(&self, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled. No verification results saved.".into());
        }
        if digest(&self.catalogue_path)? != self.sha256 {
            return Err("Catalogue bytes changed. Preview again; no results saved.".into());
        }
        for (path, before) in &self.fingerprints {
            if !std::fs::symlink_metadata(path).is_ok_and(|after| same_file(before, &after)) {
                return Err(format!(
                    "File changed after preview: {}. Preview again; no results saved.",
                    path.display()
                ));
            }
        }
        Ok(())
    }

    /// Hash exactly the confirmed files, using a private immutable DAT snapshot.
    /// Saving revalidates membership transactionally and never runs discovery.
    pub fn execute(
        self,
        trusted: &TrustedRoots,
        cancel: &AtomicBool,
        progress: &dyn Fn(DatAuditProgress),
    ) -> Result<VerificationTotals, String> {
        self.validate_inputs(cancel)?;
        let db = Database::open_read_only(&self.database_path).map_err(|e| e.to_string())?;
        let rows = db.load_archives().map_err(|e| e.to_string())?;
        if membership(&rows) != self.scope.members {
            return Err("Library membership changed. Preview again.".into());
        }
        drop(db);
        let targets = AuditTargets::Catalogue(CatalogueAuditTargets {
            files: self.scope.eligible.keys().cloned().collect(),
            catalogue_rows: self.eligible_games,
            catalogue_rows_unavailable: self
                .exclusions
                .iter()
                .filter(|e| e.reason == "Unavailable file")
                .count(),
            companion_files: 0,
        });
        let mut outcome = run_dat_audit_with_targets(
            &self.request,
            trusted,
            cancel,
            progress,
            AuditCacheConfig::Disabled,
            &targets,
        )
        .map_err(|e| e.to_string())?;
        // A same-system catalogue may cover only TAP, for example. Decoded ZIP
        // members in other representations cannot authorize a negative identity.
        let incomplete_coverage: BTreeSet<_> = outcome
            .archives
            .iter()
            .filter(|a| {
                a.members.iter().any(|m| {
                    m.verdict.as_ref().is_some_and(|v| {
                        !matches!(
                            v,
                            super::audit::AuditVerdict::NotInDat
                                | super::audit::AuditVerdict::NoUsableEvidence
                        )
                    })
                }) || !a.members.iter().any(|m| {
                    self.negative_formats
                        .contains(&ext(Path::new(&m.evidence.member_name_display)))
                }) || a.members.iter().any(|m| {
                    let extension = ext(Path::new(&m.evidence.member_name_display));
                    !self.negative_formats.contains(&extension)
                        && !crate::ingestion::discovery::is_known_non_game_extension(&extension)
                })
            })
            .map(|a| a.archive_path.to_string_lossy().into_owned())
            .collect();
        let unhashed: BTreeSet<_> = outcome.unhashed.iter().map(|f| f.path.as_str()).collect();
        outcome.report.entries.retain(|e| {
            // Existing exact/member positives stand. A definitive negative is
            // unavailable when either the content representation or any relevant
            // catalogue hashes could not be compared by this engine.
            !unhashed.contains(e.local_path.as_str())
                && (!matches!(e.verdict, super::audit::AuditVerdict::NotInDat)
                    || (!incomplete_coverage.contains(&e.local_path)
                        && (self
                            .negative_formats
                            .contains(&ext(Path::new(&e.local_path)))
                            || outcome
                                .archives
                                .iter()
                                .any(|a| a.archive_path == Path::new(&e.local_path)))))
        });
        outcome.dat_path = self.catalogue_path.to_string_lossy().into_owned();
        self.validate_inputs(cancel)?;
        let projection = project_dat_audit_for_library_identity(&outcome, "preview")
            .map_err(|e| e.to_string())?;
        let mut totals = VerificationTotals {
            catalogue: format!(
                "{} · version {}",
                self.catalogue_name,
                self.version.as_deref().unwrap_or("not stated")
            ),
            catalogue_sha256: self.sha256.clone(),
            unchecked: self.exclusions.clone(),
            not_checked: self.selected_games,
            ..Default::default()
        };
        let mut definitive = BTreeSet::new();
        for item in &projection.items {
            match &item.identity.verification_state {
                DatVerificationState::VerifiedSingleMatch { .. } => {
                    totals.verified += 1;
                    totals.not_checked -= 1;
                    definitive.insert(item.local_path.as_str());
                }
                DatVerificationState::NoMatch => {
                    totals.no_match += 1;
                    totals.not_checked -= 1;
                    definitive.insert(item.local_path.as_str());
                }
                DatVerificationState::AmbiguousMultipleCandidates { .. }
                | DatVerificationState::Conflicting { .. } => {
                    totals.ambiguous += 1;
                    totals.not_checked -= 1;
                    definitive.insert(item.local_path.as_str());
                }
                _ => {}
            }
        }
        for path in self
            .scope
            .eligible
            .keys()
            .filter(|p| !definitive.contains(p.to_string_lossy().as_ref()))
        {
            let reason = if let Some(failure) =
                outcome.unhashed.iter().find(|f| Path::new(&f.path) == path)
            {
                format!("Could not read content: {}", failure.detail)
            } else if let Some(archive) = outcome.archives.iter().find(|a| a.archive_path == *path)
            {
                if archive.completion != super::archive::ArchivePassCompletion::Complete
                    || archive.members.iter().any(|m| {
                        !matches!(
                            m.evidence.status,
                            super::archive::ArchiveMemberStatus::HashComplete
                                | super::archive::ArchiveMemberStatus::EmptyFile
                        )
                    })
                {
                    "Archive content was unreadable, unsupported or incompletely inspected".into()
                } else {
                    "Archive member evidence is ambiguous, partial, weak or outside this catalogue's formats".into()
                }
            } else {
                "No definitive content identity; catalogue hash coverage or available evidence is insufficient".into()
            };
            totals.unchecked.push(VerificationExclusion {
                path: path.clone(),
                reason,
            });
        }
        let mut error_paths: BTreeSet<String> =
            outcome.unhashed.iter().map(|f| f.path.clone()).collect();
        for archive in &outcome.archives {
            if archive.completion != super::archive::ArchivePassCompletion::Complete
                || archive.members.iter().any(|m| {
                    matches!(
                        m.evidence.status,
                        super::archive::ArchiveMemberStatus::Corrupt { .. }
                    )
                })
            {
                error_paths.insert(archive.archive_path.to_string_lossy().into_owned());
            }
        }
        totals.errors = error_paths.len();
        let mut db = Database::open_or_create(&self.database_path).map_err(|e| e.to_string())?;
        let saved = db
            .persist_scoped_library_dat_verification(&outcome, &self.scope)
            .map_err(|e| e.to_string())?;
        if saved.refused.is_some()
            || saved.unassociated != 0
            || saved.ambiguous != 0
            || saved.negative_platform_unproven_skipped != 0
        {
            return Err(format!("Verification save was incomplete: {saved:?}"));
        }
        totals.saved = saved.inserted + saved.updated;
        totals.protected_prior = saved.skipped_protected_prior_result;
        totals.disc_order = Some(db.refresh_dat_disc_order(0).map_err(|e| e.to_string()));
        Ok(totals)
    }
}

#[cfg(test)]
mod tests;
