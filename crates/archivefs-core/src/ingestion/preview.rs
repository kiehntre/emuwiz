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
struct Identity {
    game: String,
    title: String,
    multipart: Option<String>,
}

/// Constructible only after checking the parsed catalogue's own platform.
#[derive(Debug)]
pub struct SpectrumCatalogue {
    index: DatIndex,
    basis: String,
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
        Ok(Self {
            index: DatIndex::build(dat),
            basis: format!(
                "{} | version {} | {}",
                dat.source.name.as_deref().unwrap_or("Unnamed"),
                dat.source.version.as_deref().unwrap_or("not recorded"),
                dat.source.file_path
            ),
        })
    }
}

fn identities(hash: &LocalHashes, catalogues: &[SpectrumCatalogue]) -> Vec<Identity> {
    let mut found = BTreeMap::new();
    for catalogue in catalogues {
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

/// `catalogues` must already be selected for ZX Spectrum from registered,
/// validated sources. SpectrumCatalogue checks the parsed header at construction.
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
        report.problems.push("No fresh validated Spectrum catalogue was available. Alternate releases and multipart identity have not been compared.".into());
    }
    let mut hashed = BTreeMap::new();
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
        let spectrum = a
            .identity
            .platform
            .as_deref()
            .and_then(crate::canonical_platform_for_alias)
            == Some("ZX Spectrum");
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
                == Some("ZX Spectrum")
        });
        if !spectrum || !reference_platform_ok || archive {
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
                let ids = identities(&h, catalogues);
                if known_paths.contains(&a.path) {
                    for id in &ids {
                        games.insert(id.game.clone());
                        titles.insert(id.title.to_lowercase());
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
    let all_reference_hashes_complete = known
        .iter()
        .filter(|k| {
            k.platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias)
                == Some("ZX Spectrum")
                && !matches!(k.archive_kind.as_str(), "sevenzip" | "zip" | "rar" | "lha")
        })
        .all(|k| hashed.contains_key(&k.absolute_path));
    let mut member_hashes = BTreeMap::<(u64, String), Vec<(PathBuf, String)>>::new();
    let mut archive_generations = BTreeMap::new();
    let mut budget = ArchiveRunBudget::new(MAX_PREVIEW_DECODE_BYTES);
    let mut archive_relationship_checks_complete = known
        .iter()
        .filter(|k| {
            k.platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias)
                == Some("ZX Spectrum")
                && matches!(k.archive_kind.as_str(), "sevenzip" | "zip" | "rar" | "lha")
        })
        .all(|k| archives.iter().any(|a| a.path == k.absolute_path));
    let spectrum_archives: BTreeSet<_> = known
        .iter()
        .filter(|k| {
            k.platform
                .as_deref()
                .and_then(crate::canonical_platform_for_alias)
                == Some("ZX Spectrum")
        })
        .map(|k| k.absolute_path.clone())
        .collect();
    for a in archives.iter().filter(|a| {
        spectrum_archives.contains(&a.path)
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
            archive_relationship_checks_complete = false;
            continue;
        }
        report.charged_input_bytes += size;
        let Some(generation) = crate::identity_source::hashing::FileFingerprint::observe(&a.path)
        else {
            archive_relationship_checks_complete = false;
            continue;
        };
        let mut source: Box<dyn ArchiveMemberSource> = match a.kind {
            crate::ArchiveKind::SevenZip => {
                match SevenZArchiveSource::open(&a.path, &trusted, ArchiveLimits::default(), cancel)
                {
                    Ok(s) => Box::new(s),
                    Err(e) => {
                        archive_relationship_checks_complete = false;
                        report.problems.push(format!("{}: {e:?}", a.path.display()));
                        continue;
                    }
                }
            }
            crate::ArchiveKind::Zip => {
                match ZipArchiveSource::open(&a.path, &trusted, ArchiveLimits::default(), cancel) {
                    Ok(s) => Box::new(s),
                    Err(e) => {
                        archive_relationship_checks_complete = false;
                        report.problems.push(format!("{}: {e:?}", a.path.display()));
                        continue;
                    }
                }
            }
            _ => {
                archive_relationship_checks_complete = false;
                continue;
            }
        };
        let pass = source.verify_all(cancel, &mut budget);
        if !pass.is_complete() {
            archive_relationship_checks_complete = false;
            report.problems.push(format!(
                "{}: archive comparison incomplete: {:?}",
                a.path.display(),
                pass.completion
            ));
            continue;
        }
        archive_generations.insert(a.path.clone(), generation);
        for m in pass.members {
            if !m.is_hash_complete() || m.is_nested_archive {
                archive_relationship_checks_complete = false;
                continue;
            }
            if let Some(h) = m.hashes {
                member_hashes
                    .entry((m.logical_size, h.sha1))
                    .or_default()
                    .push((a.path.clone(), h.sha256));
            }
        }
    }
    for a in archives {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            return Ok(report);
        }
        let existing = known_paths.contains(&a.path);
        let mut item=MediaPreviewItem {path:a.path.clone(),disposition:if existing{MediaDisposition::Existing}else{MediaDisposition::Eligible},explanation:if existing{"Already has a library record."}else{"Eligible media candidate under the existing scanner; no game identity or emulator usability is implied."}.into(),alternate:None,multipart:None,relationship_checks_complete:!incomplete.contains(&a.path)&&archive_relationship_checks_complete&&all_reference_hashes_complete&&!catalogues.is_empty()};
        if !existing {
            if let Some(h) = hashed.get(&a.path) {
                if let Some(members) =
                    member_hashes.get(&(h.fingerprint.size_bytes, h.sha1.clone()))
                {
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
                    if games.contains(&id.game) {
                        item.alternate = Some(format!(
                            "Same exact hash-backed catalogue game name as indexed loose media: {}. Different representations remain separate.",
                            id.game
                        ));
                    } else if titles.contains(&id.title.to_lowercase()) {
                        item.alternate = Some(format!(
                            "Possible alternate release: matching catalogue title {} only. This is weak relationship evidence, not duplicate proof.",
                            id.title
                        ));
                    }
                    item.multipart = id.multipart.clone();
                }
            }
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
    if !archive_relationship_checks_complete || !all_reference_hashes_complete {
        report.problems.push("Some indexed reference content was unavailable, outside comparison limits, or unsupported by the archive reader. Absence of a relationship is not established.".into());
    }
    for skipped in scan.skipped_files {
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
                .unwrap_or_else(|| skipped.reason.label().into());
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
    Ok(report)
}

/// Use only enabled, previously validated Spectrum file sources. Parsing and
/// header identity are checked again; names and folder placement grant no trust.
pub fn load_spectrum_catalogues() -> Result<Vec<SpectrumCatalogue>, String> {
    use crate::dat::sources::{DatHealthState, DatSourceKind, DatSourceRegistry};
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
                == Some("ZX Spectrum")
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
        let parsed = crate::dat::parsers::parse_dat_file(
            &source.path,
            crate::dat::limits::DatLimits {
                max_file_size: 32 * 1024 * 1024,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
        if !parsed.warnings.is_empty() {
            return Err("Catalogue parse warnings need review before content comparison.".into());
        }
        let identity = crate::dat::identity::identify_dat_source(&parsed.dat);
        if identity
            .platform()
            .and_then(crate::canonical_platform_for_alias)
            != Some("ZX Spectrum")
        {
            return Err(
                "Registered catalogue and parsed platform disagree; no comparison performed."
                    .into(),
            );
        }
        result.push(SpectrumCatalogue::from_parsed(&parsed.dat)?);
    }
    Ok(result)
}
