//! Read-only projection of the existing ArchiveScanner. No import, identity
//! assignment or DAT verification state is written. Content comparison is an
//! explicit, bounded operation, never part of a catalogue scan.
use crate::dat::archive::{
    ArchiveMemberSource, ArchiveRunBudget, limits::ArchiveLimits, sevenz::SevenZArchiveSource,
    zip::ZipArchiveSource,
};
use crate::dat::{index::DatIndex, model::ChecksumAlgorithm};
use crate::identity_source::hashing::{LocalHashes, hash_file};
use crate::safe_read::TrustedRoots;
use crate::{ArchiveScanner, Config, PersistedArchive};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_COMPARISON_INPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_PREVIEW_DECODE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaDisposition {
    Existing,
    Eligible,
    RepresentedArchive,
    Excluded,
}
#[derive(Clone, Debug)]
pub struct MediaPreviewItem {
    pub path: PathBuf,
    pub disposition: MediaDisposition,
    pub explanation: String,
    pub alternate: Option<String>,
    pub multipart: Option<String>,
    pub relationship_checks_complete: bool,
}
#[derive(Clone, Debug, Default)]
pub struct MediaPreview {
    pub(super) import_evidence: Option<super::bounded_import::ImportEvidence>,
    pub bounded_import: Option<super::bounded_import::BoundedImportPlan>,
    pub roots: Vec<PathBuf>,
    pub items: Vec<MediaPreviewItem>,
    pub problems: Vec<String>,
    pub omitted_exclusion_details: usize,
    pub charged_input_bytes: u64,
    pub cancelled: bool,
    pub partial_coverage: bool,
    pub catalogue_basis: Vec<String>,
}

impl MediaPreview {
    pub fn count(&self, disposition: MediaDisposition) -> usize {
        self.items
            .iter()
            .filter(|i| i.disposition == disposition)
            .count()
    }
}

#[derive(Clone, Debug)]
pub(super) struct Identity {
    pub(super) game: String,
    title: String,
    pub(super) multipart: Option<String>,
}

/// Constructible only after checking the parsed catalogue's own platform.
/// The historical public name is retained for existing Spectrum callers.
#[derive(Debug)]
pub struct SpectrumCatalogue {
    pub(super) platform: String,
    pub(super) index: DatIndex,
    pub(super) source: crate::dat::model::DatSource,
    pub(super) provenance: String,
    pub(super) basis: String,
    pub(super) inputs: Vec<(PathBuf, crate::identity_source::hashing::FileFingerprint)>,
}
impl SpectrumCatalogue {
    pub fn from_parsed(dat: &crate::dat::model::ParsedDat) -> Result<Self, String> {
        if crate::dat::identity::identify_dat_source(dat)
            .platform()
            .and_then(crate::canonical_platform_for_alias)
            != Some("ZX Spectrum")
        {
            return Err("Catalogue does not identify ZX Spectrum.".into());
        }
        Self::from_parsed_for_platform(dat)
    }
    pub fn from_parsed_for_platform(dat: &crate::dat::model::ParsedDat) -> Result<Self, String> {
        let identity = crate::dat::identity::identify_dat_source(dat);
        let platform = identity
            .platform()
            .and_then(crate::canonical_platform_for_alias)
            .ok_or("Catalogue platform is unknown/ambiguous")?;
        Ok(Self {
            platform: platform.into(),
            index: DatIndex::build(dat),
            source: dat.source.clone(),
            provenance: "Parsed catalogue; registered provenance not supplied".into(),
            inputs: crate::identity_source::hashing::FileFingerprint::observe(
                std::path::Path::new(&dat.source.file_path),
            )
            .map(|g| vec![(g.path.clone(), g)])
            .unwrap_or_default(),
            basis: format!(
                "{} | version {} | {}",
                dat.source.name.as_deref().unwrap_or("Unnamed"),
                dat.source.version.as_deref().unwrap_or("not recorded"),
                dat.source.file_path
            ),
        })
    }
}

pub(super) fn identities(
    hash: &LocalHashes,
    catalogues: &[SpectrumCatalogue],
    platform: &str,
) -> Vec<Identity> {
    let mut found = BTreeMap::new();
    for catalogue in catalogues.iter().filter(|c| c.platform == platform) {
        for hit in catalogue.index.lookup_sha1(&hash.sha1) {
            let r = hit.to_owned_ref();
            if r.size_bytes != Some(hash.fingerprint.size_bytes) {
                continue;
            }
            // Every shared checksum must agree. A published SHA256 that was
            // not computed cannot be silently ignored.
            if !r.checksums.iter().all(|c| match c.algorithm {
                ChecksumAlgorithm::Crc32 => c.value.eq_ignore_ascii_case(&hash.crc32),
                ChecksumAlgorithm::Md5 => c.value.eq_ignore_ascii_case(&hash.md5),
                ChecksumAlgorithm::Sha1 => c.value.eq_ignore_ascii_case(&hash.sha1),
                ChecksumAlgorithm::Sha256 => false,
            }) {
                continue;
            }
            let metadata =
                crate::identity_source::tosec::filename_metadata::parse_tosec_filename_metadata(
                    &r.game_name,
                );
            let multipart = crate::dat::classification::multidisc_group_key(&r.game_name)
                .map(|token| format!("{token:?}"))
                .or_else(|| {
                    ["(Side A)", "(Side B)", " Side A)", " Side B)"]
                        .iter()
                        .any(|side| r.game_name.contains(side))
                        .then(|| {
                            format!(
                                "Catalogue-labelled side: {} (automatic grouping unsupported)",
                                r.game_name
                            )
                        })
                });
            found.insert(
                r.game_name.clone(),
                Identity {
                    game: r.game_name,
                    title: metadata.title,
                    multipart,
                },
            );
        }
    }
    found.into_values().collect()
}

/// Catalogue inputs must come from validated sources. Parsed headers establish
/// their canonical systems; lookups never cross platform boundaries.
/// Cancellation is checked between files/hashing chunks. The existing scanner
/// itself has no running cancellation API; cancelling discards that walk.
pub fn preview_media(
    config: &Config,
    excluded_roots: &[PathBuf],
    known: &[PersistedArchive],
    catalogues: &[SpectrumCatalogue],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<MediaPreview, String> {
    let observed_at = std::time::Instant::now();
    if config.source_folders.len() != 1 {
        return Err("Select exactly one root for a bounded media preview.".into());
    }
    let mut report = MediaPreview {
        roots: config.source_folders.clone(),
        catalogue_basis: catalogues.iter().map(|c| c.basis.clone()).collect(),
        ..Default::default()
    };
    if cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
        return Ok(report);
    }
    let root_binding = crate::catalogue_health::BoundRoot::open(&config.source_folders[0])
        .ok_or("Selected root is unavailable.")?;
    let scan = ArchiveScanner::new(config)
        .scan_archives_with_cache_excluding(&[], excluded_roots)
        .map_err(|e| e.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
        return Ok(report);
    }
    report.partial_coverage = !scan.is_complete();
    report.problems.extend(
        scan.scan_errors
            .iter()
            .map(|e| format!("{}: {}", e.path.display(), e.message)),
    );
    if scan.scan_errors_truncated() {
        report
            .problems
            .push("Additional source errors were not retained.".into());
    }
    report.omitted_exclusion_details = scan
        .total_skipped()
        .saturating_sub(scan.skipped_files.len());
    let known: Vec<_> = known
        .iter()
        .filter(|k| {
            k.absolute_path.starts_with(&config.source_folders[0])
                && !excluded_roots
                    .iter()
                    .any(|r| k.absolute_path.starts_with(r))
        })
        .collect();
    let known_map: BTreeMap<_, _> = known
        .iter()
        .map(|k| (k.absolute_path.clone(), *k))
        .collect();
    let known_paths: BTreeSet<_> = known.iter().map(|k| k.absolute_path.clone()).collect();
    let trusted = TrustedRoots::from_paths(config.source_folders.iter());
    if catalogues.is_empty() {
        report.problems.push("No fresh validated catalogue was available. Catalogue-based alternate releases and multipart identity have not been compared.".into());
    }
    let mut hashed = BTreeMap::new();
    let mut companion_hashes = BTreeMap::new();
    let mut consumed_companions = BTreeSet::new();
    let mut identified = BTreeMap::new();
    let mut incomplete = BTreeSet::new();
    let mut games = BTreeSet::new();
    let mut titles = BTreeSet::new();
    // Existing loose records first: relationships never depend on walk order.
    let mut archives = scan.archives;
    archives.sort_by_key(|a| (!known_paths.contains(&a.path), a.path.clone()));
    let total = archives.len();
    for (n, a) in archives.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        progress(n, total);
        let platform = a
            .identity
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias);
        let archive = matches!(
            a.kind,
            crate::ArchiveKind::SevenZip
                | crate::ArchiveKind::Zip
                | crate::ArchiveKind::Rar
                | crate::ArchiveKind::Lha
        );
        let reference_platform_ok = known_map.get(&a.path).is_none_or(|k| {
            k.platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias)
                == platform
        });
        if platform.is_none()
            || !reference_platform_ok
            || (archive && known_paths.contains(&a.path))
            || a.kind == crate::ArchiveKind::ArcadeSetDirectory
        {
            continue;
        }
        let size = a.identity.size_bytes.unwrap_or(u64::MAX);
        if size > MAX_COMPARISON_INPUT_BYTES.saturating_sub(report.charged_input_bytes) {
            incomplete.insert(a.path.clone());
            continue;
        }
        // Reserve the whole attempt even on refusal so errors cannot bypass
        // the run budget by repeatedly consuming unaccounted bytes.
        report.charged_input_bytes += size;
        match hash_file(&a.path, &trusted, Some(cancel)) {
            Ok(h) => {
                if let Some(item) =
                    super::discovery::inspect_spectrum_media(&a.path, &config.source_folders[0])
                    && item.validation_state != super::ValidationState::Accepted
                {
                    return Err("Media changed or failed structural revalidation during preview. Prepare again.".into());
                }
                if crate::identity_source::hashing::FileFingerprint::observe(&a.path).as_ref()
                    != Some(&h.fingerprint)
                {
                    return Err("Media changed during preview. Prepare again.".into());
                }
                let mut parts = Vec::new();
                match super::bounded_import::companions(&a.path) {
                    Ok(paths) => {
                        for path in paths {
                            if !path.starts_with(&config.source_folders[0]) {
                                incomplete.insert(a.path.clone());
                                report.problems.push(format!(
                                    "{}: companion outside selected root",
                                    a.path.display()
                                ));
                                break;
                            }
                            let size =
                                crate::identity_source::hashing::FileFingerprint::observe(&path)
                                    .map(|g| g.size_bytes)
                                    .unwrap_or(u64::MAX);
                            if size
                                > MAX_COMPARISON_INPUT_BYTES
                                    .saturating_sub(report.charged_input_bytes)
                            {
                                incomplete.insert(a.path.clone());
                                break;
                            }
                            report.charged_input_bytes += size;
                            match hash_file(&path, &trusted, Some(cancel)) {
                                Ok(h) => {
                                    consumed_companions.insert(path);
                                    parts.push(h);
                                }
                                Err(e) => {
                                    incomplete.insert(a.path.clone());
                                    report
                                        .problems
                                        .push(format!("{}: companion {e:?}", a.path.display()));
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        incomplete.insert(a.path.clone());
                        report.problems.push(format!("{}: {e}", a.path.display()));
                    }
                }
                companion_hashes.insert(a.path.clone(), parts);
                let ids = identities(&h, catalogues, platform.unwrap());
                if known_paths.contains(&a.path) {
                    for id in &ids {
                        games.insert((platform.unwrap().to_string(), id.game.clone()));
                        titles.insert((platform.unwrap().to_string(), id.title.to_lowercase()));
                    }
                }
                identified.insert(a.path.clone(), ids);
                hashed.insert(a.path.clone(), h);
            }
            Err(e) => {
                incomplete.insert(a.path.clone());
                report.problems.push(format!("{}: {e:?}", a.path.display()));
            }
        }
    }
    let mut incomplete_platforms = BTreeSet::new();
    for k in &known {
        if let Some(platform) = k
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias)
        {
            let archive = matches!(k.archive_kind.as_str(), "sevenzip" | "zip" | "rar" | "lha");
            if (archive && !archives.iter().any(|a| a.path == k.absolute_path))
                || (!archive && !hashed.contains_key(&k.absolute_path))
            {
                incomplete_platforms.insert(platform.to_string());
            }
        }
    }
    let mut member_hashes = BTreeMap::<(String, u64, String), Vec<(PathBuf, String)>>::new();
    let mut archive_generations = BTreeMap::new();
    let mut archive_refusals = BTreeMap::new();
    let mut budget = ArchiveRunBudget::new(MAX_PREVIEW_DECODE_BYTES);
    let known_archive_paths: BTreeSet<_> = known
        .iter()
        .filter(|k| {
            k.platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias)
                .is_some()
        })
        .map(|k| k.absolute_path.clone())
        .collect();
    for a in archives.iter().filter(|a| {
        a.identity
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias)
            .is_some()
            && matches!(
                a.kind,
                crate::ArchiveKind::SevenZip
                    | crate::ArchiveKind::Zip
                    | crate::ArchiveKind::Rar
                    | crate::ArchiveKind::Lha
            )
    }) {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        let size = a.identity.size_bytes.unwrap_or(u64::MAX);
        if size > MAX_COMPARISON_INPUT_BYTES.saturating_sub(report.charged_input_bytes) {
            incomplete.insert(a.path.clone());
            if known_paths.contains(&a.path) {
                incomplete_platforms.insert(
                    a.identity
                        .platform
                        .as_deref()
                        .and_then(crate::canonical_platform_for_alias)
                        .unwrap()
                        .to_string(),
                );
            }
            continue;
        }
        report.charged_input_bytes += size;
        let Some(generation) = crate::identity_source::hashing::FileFingerprint::observe(&a.path)
        else {
            incomplete.insert(a.path.clone());
            if known_paths.contains(&a.path) {
                incomplete_platforms.insert(
                    a.identity
                        .platform
                        .as_deref()
                        .and_then(crate::canonical_platform_for_alias)
                        .unwrap()
                        .to_string(),
                );
            }
            continue;
        };
        if a.kind == crate::ArchiveKind::Lha {
            let platform = a
                .identity
                .platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias)
                .unwrap();
            match super::saved_archive_checks::read_members(
                &a.path,
                platform,
                &trusted,
                cancel,
                &mut budget,
            ) {
                Ok(rows) => {
                    archive_generations.insert(a.path.clone(), generation);
                    if known_archive_paths.contains(&a.path) {
                        for (size, sha1, sha256) in rows {
                            member_hashes
                                .entry((platform.to_string(), size, sha1))
                                .or_default()
                                .push((a.path.clone(), sha256));
                        }
                    }
                }
                Err(e) => {
                    incomplete.insert(a.path.clone());
                    if known_paths.contains(&a.path) {
                        incomplete_platforms.insert(platform.to_string());
                    }
                    archive_refusals.insert(a.path.clone(), e.clone());
                    report.problems.push(format!("{}: {e}", a.path.display()));
                }
            }
            continue;
        }
        let mut source: Box<dyn ArchiveMemberSource> = match a.kind {
            crate::ArchiveKind::SevenZip => {
                match SevenZArchiveSource::open(&a.path, &trusted, ArchiveLimits::default(), cancel)
                {
                    Ok(s) => Box::new(s),
                    Err(e) => {
                        incomplete.insert(a.path.clone());
                        if known_paths.contains(&a.path) {
                            incomplete_platforms.insert(
                                a.identity
                                    .platform
                                    .as_deref()
                                    .and_then(crate::canonical_platform_for_alias)
                                    .unwrap()
                                    .to_string(),
                            );
                        }
                        report.problems.push(format!("{}: {e:?}", a.path.display()));
                        continue;
                    }
                }
            }
            crate::ArchiveKind::Zip => {
                match ZipArchiveSource::open(&a.path, &trusted, ArchiveLimits::default(), cancel) {
                    Ok(s) => Box::new(s),
                    Err(e) => {
                        incomplete.insert(a.path.clone());
                        if known_paths.contains(&a.path) {
                            incomplete_platforms.insert(
                                a.identity
                                    .platform
                                    .as_deref()
                                    .and_then(crate::canonical_platform_for_alias)
                                    .unwrap()
                                    .to_string(),
                            );
                        }
                        report.problems.push(format!("{}: {e:?}", a.path.display()));
                        continue;
                    }
                }
            }
            _ => {
                incomplete.insert(a.path.clone());
                if known_paths.contains(&a.path) {
                    incomplete_platforms.insert(
                        a.identity
                            .platform
                            .as_deref()
                            .and_then(crate::canonical_platform_for_alias)
                            .unwrap()
                            .to_string(),
                    );
                }
                continue;
            }
        };
        let pass = source.verify_all(cancel, &mut budget);
        if !pass.is_complete() || pass.members.is_empty() {
            incomplete.insert(a.path.clone());
            if known_paths.contains(&a.path) {
                incomplete_platforms.insert(
                    a.identity
                        .platform
                        .as_deref()
                        .and_then(crate::canonical_platform_for_alias)
                        .unwrap()
                        .to_string(),
                );
            }
            report.problems.push(format!(
                "{}: archive comparison incomplete: {:?}",
                a.path.display(),
                pass.completion
            ));
            continue;
        }
        archive_generations.insert(a.path.clone(), generation);
        let mut supported_members = 0usize;
        for m in pass.members {
            if super::discovery::is_known_non_game_extension(
                std::path::Path::new(&m.member_name_display)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default(),
            ) {
                continue;
            }
            let member_platforms = crate::platform::platform_candidates_for_extension(
                std::path::Path::new(&m.member_name_display)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str(),
            );
            let parent_platform = a
                .identity
                .platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias);
            if member_platforms.is_empty()
                || !member_platforms.iter().any(|p| Some(*p) == parent_platform)
            {
                incomplete.insert(a.path.clone());
                incomplete.insert(a.path.clone());
                if known_paths.contains(&a.path) {
                    incomplete_platforms.insert(
                        a.identity
                            .platform
                            .as_deref()
                            .and_then(crate::canonical_platform_for_alias)
                            .unwrap()
                            .to_string(),
                    );
                }
                report.problems.push(format!(
                    "{}: unsupported or conflicting member platform for {}",
                    a.path.display(),
                    m.member_name_display
                ));
                continue;
            }
            if !m.is_hash_complete() || m.is_nested_archive {
                incomplete.insert(a.path.clone());
                if known_paths.contains(&a.path) {
                    incomplete_platforms.insert(
                        a.identity
                            .platform
                            .as_deref()
                            .and_then(crate::canonical_platform_for_alias)
                            .unwrap()
                            .to_string(),
                    );
                }
                continue;
            }
            supported_members += 1;
            if let Some(h) = m.hashes
                && known_archive_paths.contains(&a.path)
            {
                member_hashes
                    .entry((
                        a.identity
                            .platform
                            .as_deref()
                            .and_then(crate::canonical_platform_for_alias)
                            .unwrap()
                            .to_string(),
                        m.logical_size,
                        h.sha1,
                    ))
                    .or_default()
                    .push((a.path.clone(), h.sha256));
            }
        }
        if supported_members == 0 {
            incomplete.insert(a.path.clone());
            if known_paths.contains(&a.path) {
                incomplete_platforms.insert(
                    a.identity
                        .platform
                        .as_deref()
                        .and_then(crate::canonical_platform_for_alias)
                        .unwrap()
                        .to_string(),
                );
            }
            report.problems.push(format!(
                "{}: archive has no supported game-content members",
                a.path.display()
            ));
        }
    }
    let candidate_traits: BTreeMap<_, _> = archives
        .iter()
        .map(|a| {
            (
                a.path.clone(),
                (
                    a.kind.storage_name().to_string(),
                    a.identity.platform.clone(),
                ),
            )
        })
        .collect();
    for a in archives {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        let existing = known_paths.contains(&a.path);
        let platform = a
            .identity
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias);
        if !existing && consumed_companions.contains(&a.path) {
            report.items.push(MediaPreviewItem {path:a.path.clone(),disposition:MediaDisposition::Excluded,explanation:"Supporting media already represented by its reviewed CUE/GDI descriptor; not a separate import record.".into(),alternate:None,multipart:None,relationship_checks_complete:false});
            continue;
        }
        let mut item=MediaPreviewItem {path:a.path.clone(),disposition:if existing{MediaDisposition::Existing}else{MediaDisposition::Eligible},explanation:if existing{"Already has a library record."}else{"Eligible media candidate under the existing scanner; no game identity or emulator usability is implied."}.into(),alternate:None,multipart:None,relationship_checks_complete:!incomplete.contains(&a.path)&&!incomplete_platforms.contains(platform.unwrap_or_default())&&(platform!=Some("ZX Spectrum") || catalogues.iter().any(|c|c.platform=="ZX Spectrum"))};
        if !existing
            && incomplete.contains(&a.path)
            && matches!(
                a.kind,
                crate::ArchiveKind::Zip
                    | crate::ArchiveKind::SevenZip
                    | crate::ArchiveKind::Rar
                    | crate::ArchiveKind::Lha
            )
        {
            item.disposition = MediaDisposition::Excluded;
            item.explanation = archive_refusals.get(&a.path).cloned().unwrap_or_else(|| {
                "Archive content was not completely recognized/inspected; excluded from bounded import. See comparison problems.".into()
            });
        }
        if !existing {
            if let Some(h) = hashed.get(&a.path) {
                if let Some(members) = member_hashes.get(&(
                    platform.unwrap_or_default().to_string(),
                    h.fingerprint.size_bytes,
                    h.sha1.clone(),
                )) {
                    if h.fingerprint.size_bytes
                        <= MAX_COMPARISON_INPUT_BYTES.saturating_sub(report.charged_input_bytes)
                    {
                        report.charged_input_bytes += h.fingerprint.size_bytes;
                        match crate::repair::exact_duplicate::hash_full_file_sha256(
                            &a.path,
                            &trusted,
                            Some(cancel),
                        ) {
                            Ok(full)
                                if full.size_bytes == h.fingerprint.size_bytes
                                    && crate::identity_source::hashing::FileFingerprint::observe(
                                        &a.path,
                                    )
                                    .as_ref()
                                        == Some(&h.fingerprint)
                                    && members.iter().any(|(_, sha)| *sha == full.sha256) =>
                            {
                                let parent = &members
                                    .iter()
                                    .find(|(_, sha)| *sha == full.sha256)
                                    .unwrap()
                                    .0;
                                item.disposition = MediaDisposition::RepresentedArchive;
                                item.explanation = format!(
                                    "Exact decoded archive content already represented by {} (full SHA-256 and size). No merge or deletion is proposed.",
                                    parent.display()
                                );
                            }
                            Ok(_) => {}
                            Err(e) => {
                                item.relationship_checks_complete = false;
                                report.problems.push(format!("{}: {e:?}", a.path.display()));
                            }
                        }
                    } else {
                        item.relationship_checks_complete = false;
                    }
                }
            } else {
                item.relationship_checks_complete = false;
            }
            if let Some(ids) = identified.get(&a.path) {
                if ids.len() > 1 {
                    item.alternate = Some("Ambiguous catalogue identities; none selected.".into());
                } else if let Some(id) = ids.first() {
                    if games.contains(&(platform.unwrap_or_default().to_string(), id.game.clone()))
                    {
                        item.alternate = Some(format!(
                            "Same exact hash-backed catalogue game name as indexed loose media: {}. Different representations remain separate.",
                            id.game
                        ));
                    } else if titles.contains(&(
                        platform.unwrap_or_default().to_string(),
                        id.title.to_lowercase(),
                    )) {
                        item.alternate = Some(format!(
                            "Possible alternate release: matching catalogue title {} only. This is weak relationship evidence, not duplicate proof.",
                            id.title
                        ));
                    }
                    item.multipart = id.multipart.clone();
                }
            }
        }
        if platform.is_none() {
            item.disposition = MediaDisposition::Excluded;
            item.explanation =
                "Platform remains unknown or ambiguous; no platform selected.".into();
        }
        if item.disposition == MediaDisposition::Eligible && !item.relationship_checks_complete {
            item.explanation.push_str(" Archive/catalogue relationships were not fully compared; do not treat this as proof of a new unique game.");
        }
        report.items.push(item);
    }
    if cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
        return Ok(report);
    }
    if !incomplete_platforms.is_empty() {
        report.problems.push("Some indexed reference content was unavailable, outside comparison limits, or unsupported by the archive reader. Absence of a relationship is not established.".into());
    }
    for skipped in scan.skipped_files {
        let hdf_refusal = if skipped
            .path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("hdf") || ext.eq_ignore_ascii_case("hdfx"))
        {
            crate::amiga_disk::inspect_amiga_image(&skipped.path).err().map(|e|format!("HDF/HDFX not accepted by the existing Amiga structural inspector: {e}. Other HDF variants retain their specialist limitation."))
        } else {
            None
        };
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        let explanation =
            super::discovery::inspect_spectrum_media(&skipped.path, &config.source_folders[0])
                .map(|d| {
                    format!(
                        "{} {}",
                        d.explanation,
                        d.skip_reason.map(|r| format!("{r:?}")).unwrap_or_default()
                    )
                })
                .unwrap_or_else(|| hdf_refusal.unwrap_or_else(|| skipped.reason.label().into()));
        report.items.push(MediaPreviewItem {
            path: skipped.path,
            disposition: MediaDisposition::Excluded,
            explanation,
            alternate: None,
            multipart: None,
            relationship_checks_complete: false,
        });
    }
    progress(total, total);
    if cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
        return Ok(report);
    }
    if companion_hashes.values().flatten().any(|h| {
        crate::identity_source::hashing::FileFingerprint::observe(&h.fingerprint.path).as_ref()
            != Some(&h.fingerprint)
    }) {
        return Err("Companion changed during preview; prepare again.".into());
    }
    if !root_binding.current()
        || archive_generations.iter().any(|(p, g)| {
            crate::identity_source::hashing::FileFingerprint::observe(p).as_ref() != Some(g)
        })
        || hashed.iter().any(|(path, h)| {
            crate::identity_source::hashing::FileFingerprint::observe(path).as_ref()
                != Some(&h.fingerprint)
        })
    {
        return Err(
            "Selected storage or file content changed during preview. Discarded; prepare again."
                .into(),
        );
    }
    report.items.sort_by(|a, b| a.path.cmp(&b.path));
    report.import_evidence = Some(super::bounded_import::ImportEvidence {
        observed_at,
        root: config.source_folders[0].clone(),
        complete: !report.partial_coverage
            && !report.cancelled
            && report.omitted_exclusion_details == 0
            && catalogues.iter().all(|c| !c.inputs.is_empty()),
        catalogue_basis: report.catalogue_basis.clone(),
        binding: root_binding.binding.clone(),
        known: known.into_iter().cloned().collect(),
        candidates: report
            .items
            .iter()
            .filter(|i| i.disposition == MediaDisposition::Eligible)
            .filter_map(|i| {
                hashed
                    .get(&i.path)
                    .map(|h| super::bounded_import::Candidate {
                        hashes: h.clone(),
                        platform: candidate_traits
                            .get(&i.path)
                            .and_then(|(_, p)| p.as_deref())
                            .and_then(crate::canonical_platform_for_alias)
                            .unwrap_or_default()
                            .to_string(),
                        kind: candidate_traits
                            .get(&i.path)
                            .map(|(k, _)| k.clone())
                            .unwrap_or_default(),
                        companions: companion_hashes.remove(&i.path).unwrap_or_default(),
                        alternate: i.alternate.clone(),
                        multipart: i.multipart.clone(),
                        ambiguous: i
                            .alternate
                            .as_deref()
                            .is_some_and(|s| s.starts_with("Ambiguous")),
                        relationships_complete: i.relationship_checks_complete,
                    })
            })
            .collect(),
        references: archive_generations
            .into_iter()
            .chain(
                hashed
                    .iter()
                    .filter(|(p, _)| known_paths.contains(*p))
                    .map(|(p, h)| (p.clone(), h.fingerprint.clone())),
            )
            .chain(catalogues.iter().flat_map(|c| c.inputs.iter().cloned()))
            .collect(),
        exclusions: report
            .items
            .iter()
            .filter_map(|i| {
                if matches!(
                    i.disposition,
                    MediaDisposition::Excluded | MediaDisposition::RepresentedArchive
                ) {
                    Some((i.path.clone(), i.explanation.clone()))
                } else if i.disposition == MediaDisposition::Eligible
                    && (!hashed.contains_key(&i.path))
                {
                    Some((
                        i.path.clone(),
                        "Outside the hashed, supported native file allowlist.".into(),
                    ))
                } else {
                    None
                }
            })
            .collect(),
    });
    Ok(report)
}

/// Use only enabled, previously validated Spectrum file sources. Parsing and
/// header identity are checked again; names and folder placement grant no trust.
pub fn load_spectrum_catalogues() -> Result<Vec<SpectrumCatalogue>, String> {
    load_discovery_catalogues_for(&["ZX Spectrum".into()])
}
pub fn load_discovery_catalogues() -> Result<Vec<SpectrumCatalogue>, String> {
    load_discovery_catalogues_for(&[])
}
/// Filter canonical platforms before charging catalogue bounds; unrelated systems
/// cannot exhaust the budget of a bounded single-platform preview.
pub fn load_discovery_catalogues_for(
    platforms: &[String],
) -> Result<Vec<SpectrumCatalogue>, String> {
    let local_config =
        crate::dat::sources::default_dat_sources_config_path().map_err(|e| e.to_string())?;
    let mut catalogues = match std::fs::symlink_metadata(&local_config) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        _ => load_local_discovery_catalogues_for(platforms)?,
    };
    let bytes =
        catalogues
            .iter()
            .flat_map(|c| c.inputs.iter())
            .try_fold(0u64, |sum, (path, _)| {
                std::fs::metadata(path)
                    .map(|m| sum.saturating_add(m.len()))
                    .map_err(|e| e.to_string())
            })?;
    let managed = discovery_catalogues::load_managed(
        platforms,
        32usize.saturating_sub(catalogues.len()),
        (128 * 1024 * 1024u64).saturating_sub(bytes),
    )?;
    catalogues.extend(managed);
    Ok(catalogues)
}

#[path = "discovery_catalogues.rs"]
mod discovery_catalogues;

fn load_local_discovery_catalogues_for(
    platforms: &[String],
) -> Result<Vec<SpectrumCatalogue>, String> {
    use crate::dat::sources::{DatHealthState, DatSourceKind, DatSourceRegistry};
    let config_path =
        crate::dat::sources::default_dat_sources_config_path().map_err(|e| e.to_string())?;
    let config_generation = crate::identity_source::hashing::FileFingerprint::observe(&config_path)
        .ok_or("Catalogue configuration unavailable.")?;
    let config =
        crate::dat::sources::load_dat_sources_config_default().map_err(|e| e.to_string())?;
    let (registry, problems) = DatSourceRegistry::from_config(&config);
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    let mut result = Vec::new();
    let mut input_bytes = 0u64;
    for source in registry.entries().iter().filter(|s| {
        s.enabled
            && s.kind == DatSourceKind::File
            && s.platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias)
                .is_some_and(|p| platforms.is_empty() || platforms.iter().any(|wanted| wanted == p))
    }) {
        if !matches!(
            source.health.state(),
            DatHealthState::Valid | DatHealthState::ValidWithWarnings
        ) || source.health.is_stale_for(&source.path, source.kind)
        {
            continue;
        }
        if result.len() >= 32 {
            return Err("Too many catalogue inputs for this bounded preview.".into());
        }
        let size = std::fs::metadata(&source.path)
            .map_err(|e| e.to_string())?
            .len();
        input_bytes = input_bytes.saturating_add(size);
        if size > 32 * 1024 * 1024 || input_bytes > 128 * 1024 * 1024 {
            return Err("Catalogue inputs exceed the bounded preview limit.".into());
        }
        let generation = crate::identity_source::hashing::FileFingerprint::observe(&source.path)
            .ok_or("Catalogue identity unavailable.")?;
        let parsed = crate::dat::parsers::parse_dat_file(
            &source.path,
            crate::dat::limits::DatLimits {
                max_file_size: 32 * 1024 * 1024,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        if parsed.warnings.iter().any(|w| !w.is_note()) {
            return Err("Catalogue parse warnings need review before content comparison.".into());
        }
        let identity = crate::dat::identity::identify_dat_source(&parsed.dat);
        if identity
            .platform()
            .and_then(crate::canonical_platform_for_alias)
            != source
                .platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias)
        {
            return Err(
                "Registered catalogue and parsed platform disagree; no comparison performed."
                    .into(),
            );
        }
        let mut catalogue = SpectrumCatalogue::from_parsed_for_platform(&parsed.dat)?;
        if crate::identity_source::hashing::FileFingerprint::observe(&source.path).as_ref()
            != Some(&generation)
        {
            return Err("Catalogue changed while parsing. Prepare again.".into());
        }
        catalogue.provenance = format!("local:{} | {:?}", source.id, source.ownership());
        catalogue.inputs = vec![
            (source.path.clone(), generation),
            (config_path.clone(), config_generation.clone()),
        ];
        result.push(catalogue);
    }
    if crate::identity_source::hashing::FileFingerprint::observe(&config_path).as_ref()
        != Some(&config_generation)
    {
        return Err("Catalogue configuration changed. Prepare again.".into());
    }
    Ok(result)
}
