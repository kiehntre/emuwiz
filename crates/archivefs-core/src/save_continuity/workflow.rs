//! The steps the CLI and Blue share, so both do exactly the same thing.

use super::ContinuityError;
use super::discover::contained_saves;
use super::model::{DiscoveredSave, SaveFormat};
use super::session::SaveMark;
use super::status::{
    Advisory, GameIntegrity, PrecedingSession, StructureCheck, describe_integrity,
    describe_preceding, describe_structure,
};
use super::store::{
    CaptureBoundary, CaptureFile, CaptureReason, CaptureRequest, ContainedSave, Provenance,
    SnapshotManifest, SnapshotStore,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// What a snapshot of `save` has to copy. A card save is snapshotted as its
/// whole card (the file the emulator writes), with every save on it recorded;
/// a save folder as every file directly inside it.
pub fn capture_request(
    save: &DiscoveredSave,
    all: &[DiscoveredSave],
    device_id: &str,
    reason: CaptureReason,
    session_id: Option<String>,
    allow_unproven_closure: bool,
    preceding_session: PrecedingSession,
) -> Result<CaptureRequest, ContinuityError> {
    let source = &save.source_path;
    let meta =
        fs::symlink_metadata(source).map_err(|error| super::io("cannot check", source, error))?;
    let name_of = |path: &std::path::Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let mut files = Vec::new();
    if meta.is_dir() {
        if matches!(
            save.format,
            SaveFormat::Ps1CardSave | SaveFormat::Ps2CardSave
        ) {
            return Err(ContinuityError::Refused(
                "PCSX2 folder cards are listed only; EmuWiz does not snapshot them yet".into(),
            ));
        }
        let mut members: Vec<PathBuf> = fs::read_dir(source)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        members.sort();
        for member in members {
            let kind = fs::symlink_metadata(&member)
                .map_err(|error| super::io("cannot check", &member, error))?;
            if !kind.is_file() {
                return Err(ContinuityError::Refused(format!(
                    "{} is not a plain file; this save folder cannot be copied faithfully",
                    member.display()
                )));
            }
            files.push(CaptureFile {
                name: name_of(&member).unwrap_or_default(),
                source: member,
            });
        }
    } else {
        files.push(CaptureFile {
            name: name_of(source).unwrap_or_default(),
            source: source.clone(),
        });
    }
    let mut contained = contained_saves(all, source);
    if contained.is_empty() {
        if let (Some(code), Some(fingerprint)) = (&save.identity.product_code, &save.content_sha256)
        {
            let prefix = match save.format {
                SaveFormat::GameCubeGci => "gc",
                SaveFormat::Ps3Savedata => "ps3",
                SaveFormat::PspSavedata => "psp",
                _ => "save",
            };
            let name = save.entry_name.clone().unwrap_or_else(|| code.clone());
            contained.push(ContainedSave {
                key: format!("{prefix}:{code}:{name}"),
                name,
                product_code: Some(code.clone()),
                title: save.identity.title.clone(),
                fingerprint: fingerprint.clone(),
            });
        }
    }
    Ok(CaptureRequest {
        provenance: Provenance {
            device_id: device_id.to_string(),
            emulator: save.emulator.clone(),
            profile: save.profile.clone(),
            emulator_version: save.emulator_version.clone(),
            platform: save.platform.clone(),
            source_root: source.clone(),
        },
        files,
        contained_saves: contained,
        reason,
        session_id,
        allow_unproven_closure,
        captured_unix: None,
        structure: structure_of(save),
        preceding_session,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimelineEntry {
    pub snapshot_id: String,
    pub captured_unix: i64,
    pub device_id: String,
    pub emulator: String,
    pub profile: String,
    pub source: PathBuf,
    pub boundary: CaptureBoundary,
    /// True only for a snapshot taken at a verified safe point.
    pub verified_safe_point: bool,
    pub reason: CaptureReason,
    pub session_id: Option<String>,
    pub total_bytes: u64,
    pub saves: Vec<ContainedSave>,
    /// What differs from the snapshot before, by content.
    pub changes: Vec<String>,
    /// Three separate statuses; none of them implies another.
    pub structure: Option<StructureCheck>,
    pub game_integrity: Option<GameIntegrity>,
    pub preceding_session: Option<PrecedingSession>,
    pub advisory: Advisory,
    /// Shown prominently when the emulator did not end normally (or that is
    /// not known) before this snapshot.
    pub warning: Option<String>,
    /// The same three statuses as sentences, for display.
    pub status_lines: Vec<String>,
}

/// One save location's snapshots, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Timeline {
    pub lineage: String,
    pub source: PathBuf,
    pub emulator: String,
    pub entries: Vec<TimelineEntry>,
}

fn changes(before: Option<&SnapshotManifest>, now: &SnapshotManifest) -> Vec<String> {
    let Some(before) = before else {
        return vec!["first snapshot of this save".to_string()];
    };
    let index = |manifest: &SnapshotManifest| -> BTreeMap<String, String> {
        if manifest.body.contained_saves.is_empty() {
            manifest
                .body
                .files
                .iter()
                .map(|file| (file.name.clone(), file.sha256.clone()))
                .collect()
        } else {
            manifest
                .body
                .contained_saves
                .iter()
                .map(|save| (save.name.clone(), save.fingerprint.clone()))
                .collect()
        }
    };
    let (old, new) = (index(before), index(now));
    let mut out = Vec::new();
    for (name, fingerprint) in &new {
        match old.get(name) {
            None => out.push(format!("{name}: new")),
            Some(was) if was != fingerprint => out.push(format!("{name}: content changed")),
            Some(_) => {}
        }
    }
    for name in old.keys().filter(|name| !new.contains_key(*name)) {
        out.push(format!("{name}: no longer present"));
    }
    if out.is_empty() {
        out.push("no save's content changed (only other data in the file did)".to_string());
    }
    out
}

/// Every save location's history. Reads manifests only; use
/// [`SnapshotStore::verify`] to re-check a snapshot's bytes.
pub fn timelines(store: &SnapshotStore) -> Vec<Timeline> {
    let mut by_lineage: BTreeMap<String, Vec<SnapshotManifest>> = BTreeMap::new();
    for manifest in store.list() {
        by_lineage
            .entry(manifest.body.lineage.clone())
            .or_default()
            .push(manifest);
    }
    let mut out = Vec::new();
    for (lineage, mut chain) in by_lineage {
        chain.sort_by_key(|manifest| (manifest.body.captured_unix, manifest.snapshot_id.clone()));
        let mut entries = Vec::new();
        for (index, manifest) in chain.iter().enumerate() {
            let before = index.checked_sub(1).map(|at| &chain[at]);
            entries.push(TimelineEntry {
                snapshot_id: manifest.snapshot_id.clone(),
                captured_unix: manifest.body.captured_unix,
                device_id: manifest.body.provenance.device_id.clone(),
                emulator: manifest.body.provenance.emulator.clone(),
                profile: manifest.body.provenance.profile.clone(),
                source: manifest.body.provenance.source_root.clone(),
                boundary: manifest.body.boundary,
                verified_safe_point: manifest.body.boundary
                    == CaptureBoundary::EmulatorClosedObserved,
                reason: manifest.body.reason,
                session_id: manifest.body.session_id.clone(),
                total_bytes: manifest.total_bytes(),
                saves: manifest.body.contained_saves.clone(),
                changes: changes(before, manifest),
                structure: manifest.body.structure.clone(),
                game_integrity: manifest.body.game_integrity.clone(),
                preceding_session: manifest.body.preceding_session.clone(),
                advisory: advisory_of(manifest),
                warning: warning_of(manifest),
                status_lines: status_lines(manifest),
            });
        }
        if let Some(first) = chain.first() {
            out.push(Timeline {
                lineage,
                source: first.body.provenance.source_root.clone(),
                emulator: first.body.provenance.emulator.clone(),
                entries,
            });
        }
    }
    out
}

/// The structural result for one discovered save, worded so that it claims
/// exactly what discovery checked and nothing more.
pub fn structure_of(save: &DiscoveredSave) -> StructureCheck {
    match save.format {
        SaveFormat::Ps1CardSave | SaveFormat::Ps2CardSave => {
            let disagree = save
                .integrity
                .iter()
                .any(|line| line.contains("DISAGREE"));
            let warnings = save
                .unsupported
                .iter()
                .any(|line| line == super::discover::CARD_HAS_WARNINGS);
            if disagree || warnings {
                StructureCheck::Problems {
                    what: "the memory card does not read cleanly (its two readers disagree or it has warnings)".to_string(),
                }
            } else {
                StructureCheck::Passed {
                    what: "the memory card's directory and allocation were walked by two independent readers, which list the same saves; the game's own data inside each save was not checked".to_string(),
                }
            }
        }
        SaveFormat::GameCubeGci => StructureCheck::Passed {
            what: "the file's length matches the block count in its header; nothing inside the save (including its checksums) was checked".to_string(),
        },
        SaveFormat::Ps3Savedata | SaveFormat::PspSavedata => StructureCheck::NotChecked {
            why: "only PARAM.SFO was read; the save's signature (PARAM.PFD) and data files were not checked".to_string(),
        },
        SaveFormat::CartridgeBattery => StructureCheck::NotChecked {
            why: "a cartridge save is raw memory with no structure EmuWiz can check".to_string(),
        },
        SaveFormat::SaveState => StructureCheck::NotChecked {
            why: "a save state's layout belongs to one emulator build".to_string(),
        },
    }
}

/// The current content of every save sharing `save`'s file, for matching
/// against a session's record.
pub fn marks_of(save: &DiscoveredSave, all: &[DiscoveredSave]) -> Vec<SaveMark> {
    all.iter()
        .filter(|other| other.source_path == save.source_path)
        .filter_map(|other| {
            Some(SaveMark {
                save_id: other.id.clone(),
                label: other
                    .entry_name
                    .clone()
                    .unwrap_or_else(|| other.source_path.display().to_string()),
                content_sha256: other.content_sha256.clone()?,
            })
        })
        .collect()
}

pub fn advisory_of(manifest: &SnapshotManifest) -> Advisory {
    let from_session = manifest
        .body
        .preceding_session
        .as_ref()
        .map_or(Advisory::Caution, PrecedingSession::advisory);
    let structural = if matches!(
        manifest.body.structure,
        Some(StructureCheck::Problems { .. })
    ) {
        Advisory::Warning
    } else {
        Advisory::None
    };
    from_session.max(structural)
}

pub fn warning_of(manifest: &SnapshotManifest) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(StructureCheck::Problems { what }) = &manifest.body.structure {
        parts.push(format!("WARNING: {what}"));
    }
    match &manifest.body.preceding_session {
        Some(preceding) => parts.extend(preceding.warning()),
        None => {
            parts.push("How the emulator last ended was not recorded for this snapshot".to_string())
        }
    }
    if manifest.body.boundary != CaptureBoundary::EmulatorClosedObserved {
        parts.push(manifest.body.boundary.label().to_string());
    }
    (!parts.is_empty()).then(|| parts.join(". "))
}

/// Capture safety, structure, game data and last-exit context, one line
/// each, in that order. Never merged into one word.
pub fn status_lines(manifest: &SnapshotManifest) -> Vec<String> {
    vec![
        format!("Copy: {}", manifest.body.boundary.label()),
        describe_structure(manifest.body.structure.as_ref()),
        describe_integrity(manifest.body.game_integrity.as_ref()),
        describe_preceding(manifest.body.preceding_session.as_ref()),
    ]
}
