//! The steps the CLI and Blue share, so both do exactly the same thing.

use super::ContinuityError;
use super::discover::contained_saves;
use super::model::{DiscoveredSave, SaveFormat};
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
