//! Read-only orchestration. No database, journal, emulator process or mutation plan.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use archivefs_core::dat::archive::hash::hash_member_stream;
use archivefs_core::dat::catalogue_selection::{CatalogueRef, resolve_catalogue};
use archivefs_core::dat::model::{DatDiskEntry, DatEcosystem, DatRomEntry};
use archivefs_core::dat::parsers::mame_listxml::MAME_LISTXML_PARSER_SCHEMA_VERSION;
use archivefs_core::mame_collection_analyser::inspection::{
    CollectionInspection, InspectionLimits, InspectionProgress, SourceStamp, inspect_collection,
};
use archivefs_core::mame_collection_analyser::{MameCollectionAnalysis, analyse_collection};
use archivefs_core::safe_read::{TrustedRoots, open_bounded_read};
use serde::Serialize;

use crate::dat_catalogue_picker::CatalogueInventorySnapshot;

#[derive(Clone)]
pub(in crate::gui_v2) struct Request {
    pub id: u64,
    pub reference: CatalogueRef,
    pub snapshot: CatalogueInventorySnapshot,
    pub catalogue_path: PathBuf,
    pub root: PathBuf,
    /// Explicit user-supplied captured output. Health never executes MAME.
    pub version_output: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::gui_v2) struct CatalogueEvidence {
    pub reference: String,
    pub source_id: String,
    pub display_name: String,
    pub recorded_author: Option<String>,
    pub provenance: String,
    pub sha256: String,
    pub source: SourceStamp,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::gui_v2) struct SetContext {
    pub description: Option<String>,
    pub clone_of: Option<String>,
    pub rom_of: Option<String>,
    pub bios: bool,
    pub device: bool,
    pub device_references: Vec<String>,
    pub duplicate_machine_declarations: bool,
    pub rom_declarations: Vec<DatRomEntry>,
    pub disk_declarations: Vec<DatDiskEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::gui_v2) struct Report {
    pub job_id: u64,
    pub catalogue: CatalogueEvidence,
    pub inspection: CollectionInspection,
    pub analysis: MameCollectionAnalysis,
    pub context: BTreeMap<String, SetContext>,
    pub unrecognised_sets: Vec<String>,
    pub installed_version_origin: &'static str,
    /// Prepared off the GUI thread; not part of the exported receipt.
    #[serde(skip)]
    pub json: String,
    #[serde(skip)]
    pub json_unavailable: Option<String>,
}

pub(in crate::gui_v2) fn run(
    request: Request,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(InspectionProgress),
) -> Result<Report, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Inspection cancelled before it started.".into());
    }
    let before = SourceStamp::observe(&request.catalogue_path)?;
    if before.directory {
        return Err("Choose one MAME arcade catalogue file.".into());
    }
    let file = open_bounded_read(&request.catalogue_path, &TrustedRoots::none())
        .map_err(|e| e.detail())?;
    if file.len() > request.snapshot.inputs().limits.max_file_size {
        return Err("Catalogue exceeds the bounded parser file-size limit.".into());
    }
    let hash = hash_member_stream(file.into_file(), before.bytes, cancel)
        .map_err(|e| format!("Catalogue hashing stopped: {e:?}"))?;
    let mut inputs = request.snapshot.inputs();
    inputs.limits.max_entries = 100_000;
    inputs.limits.max_roms_per_entry = 16_384;
    let resolved = resolve_catalogue(&request.reference, inputs).map_err(|e| e.to_string())?;
    if resolved.backing_path != request.catalogue_path
        || !before.still_matches()
        || hash.bytes_read != before.bytes
    {
        return Err(
            "The selected catalogue changed while being loaded; refresh and choose it again."
                .into(),
        );
    }
    let dat = resolved.parsed;
    if dat
        .source
        .version
        .as_deref()
        .and_then(archivefs_core::mame_collection_analyser::parse_version_evidence)
        .is_none()
    {
        return Err("Health needs a supported declared MAME version. Import a versioned MAME -listxml catalogue; no version is guessed.".into());
    }
    if dat.source.ecosystem != DatEcosystem::MAMEArcade
        || dat.games.is_empty()
        || dat
            .games
            .iter()
            .any(|g| g.mame_listxml_schema_version != Some(MAME_LISTXML_PARSER_SCHEMA_VERSION))
    {
        return Err("Health V1 needs a current MAME arcade machine catalogue from MAME -listxml. Software lists and older catalogue projections are not supported.".into());
    }
    let declarations = dat.games.iter().try_fold(0usize, |n, g| {
        n.checked_add(g.roms.len())
            .and_then(|n| n.checked_add(g.disks.len()))
    });
    if declarations.is_none_or(|n| n > 500_000) {
        return Err("Catalogue exceeds the 500,000-member analysis limit.".into());
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("Catalogue loading cancelled.".into());
    }
    let mut inspection =
        inspect_collection(&request.root, InspectionLimits::default(), cancel, progress)?;
    if !dat.source.parse_warnings.is_empty() {
        inspection.invalidate(format!(
            "Catalogue contains {} parser warning(s); complete health cannot be established.",
            dat.source.parse_warnings.len()
        ));
        inspection.inventory.warnings.extend(
            dat.source
                .parse_warnings
                .iter()
                .take(20)
                .map(|w| format!("Catalogue warning: {w:?}")),
        );
    }
    progress(InspectionProgress {
        current: "Calculating health from inspected evidence".into(),
        containers: inspection.inventory.sets.len(),
        entries: declarations.unwrap(),
        logical_bytes: inspection.logical_bytes,
    });
    // A free-text GUI field is not the backend's trusted bare-version caller.
    // Establish a MAME product marker here; leave numeric/build validation to
    // the existing strict backend parser and retain the original text below.
    let version_evidence = request
        .version_output
        .as_ref()
        .filter(|text| captured_mame_product(text))
        .cloned();
    let mut analysis =
        analyse_collection(&dat, &inspection.inventory, version_evidence.clone(), None);
    // Bind the completed result to unchanged catalogue and collection sources.
    // Analysis is one bounded, non-interruptible step in the existing backend.
    if cancel.load(Ordering::Relaxed)
        || !before.still_matches()
        || !inspection.sources_unchanged(cancel)
    {
        inspection.cancelled = cancel.load(Ordering::Relaxed);
        inspection.invalidate(if inspection.cancelled {
            "Cancelled during analysis; this is an incomplete snapshot."
        } else {
            "Sources changed during analysis; recheck before relying on this snapshot."
        });
        analysis = analyse_collection(&dat, &inspection.inventory, version_evidence.clone(), None);
    }
    analysis.emulator_version = request.version_output.clone();
    let mut context = BTreeMap::<String, SetContext>::new();
    for g in &dat.games {
        let value = SetContext {
            description: g.description.clone(),
            clone_of: g.clone_of.clone(),
            rom_of: g.rom_of.clone(),
            bios: g
                .is_bios
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case("yes")),
            device: g
                .is_device
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case("yes")),
            device_references: g
                .device_refs
                .iter()
                .map(|r| {
                    r.name
                        .clone()
                        .unwrap_or_else(|| "(unnamed device reference)".into())
                })
                .collect(),
            duplicate_machine_declarations: false,
            rom_declarations: g.roms.clone(),
            disk_declarations: g.disks.clone(),
        };
        if let Some(previous) = context.get_mut(&g.name) {
            previous.duplicate_machine_declarations = true;
            previous.rom_declarations.extend(value.rom_declarations);
            previous.disk_declarations.extend(value.disk_declarations);
        } else {
            context.insert(g.name.clone(), value);
        }
    }
    let unrecognised_sets = inspection
        .inventory
        .sets
        .iter()
        .filter(|s| !context.contains_key(&s.name))
        .map(|s| s.name.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut report = Report {
        job_id: request.id,
        catalogue: CatalogueEvidence {
            reference: request.reference.token(),
            source_id: resolved.source_id,
            display_name: resolved.display_name,
            recorded_author: dat.source.author.clone(),
            provenance: format!("{:?}", resolved.provenance),
            sha256: hash.hashes.sha256,
            source: before,
        },
        inspection,
        analysis,
        context,
        unrecognised_sets,
        installed_version_origin: if version_evidence.is_some() {
            "User-supplied captured MAME version output; executable identity is not authenticated"
        } else if request.version_output.is_some() {
            "Supplied output has no MAME product marker; version comparison is Unknown"
        } else {
            "No captured installed MAME version evidence"
        },
        json: String::new(),
        json_unavailable: None,
    };
    match bounded_json(&report, 32 * 1024 * 1024) {
        Ok(json) => report.json = json,
        Err(error) => report.json_unavailable = Some(error),
    }
    Ok(report)
}

fn captured_mame_product(text: &str) -> bool {
    text.split_ascii_whitespace()
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("mame"))
        || text.split_ascii_whitespace().last().is_some_and(|word| {
            word.to_ascii_lowercase().starts_with("(mame") && word.ends_with(')')
        })
}

/// Cap serialization itself, not just the eventual clipboard operation.
pub(in crate::gui_v2) fn bounded_json(
    value: &impl Serialize,
    limit: usize,
) -> Result<String, String> {
    struct BoundedBuffer {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for BoundedBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other(
                    "JSON report exceeds the 32 MiB export limit. Copy the summary or a selected set instead.",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = BoundedBuffer {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer_pretty(&mut buffer, value).map_err(|e| e.to_string())?;
    String::from_utf8(buffer.bytes).map_err(|e| e.to_string())
}
