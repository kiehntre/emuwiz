//! Read-only proof model for a future Saturn data-track rebuild.
//!
//! This is intentionally a comparator, not a builder or patch applier.  It
//! accepts two already-materialized manifests and the corresponding cooked
//! MODE1/2048 data-track files, identifies changed/preserved sectors, and
//! refuses preservation claims when audio, System ID, or track topology move
//! unexpectedly.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::saturn_disc_manifest::{
    SaturnDiscManifest, SaturnManifestStatus, SaturnTrackMode, SaturnTrackType,
};

const PROOF_SECTOR_BYTES: u64 = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturnRebuildRefusal {
    MissingDataTrack,
    MultipleDataTracks,
    UnsupportedSectorMode { mode: SaturnTrackMode },
    DataTrackReadFailed { detail: String },
    AudioChanged { track: u32 },
    SystemIdChanged,
    DescriptorChanged,
    TopologyChanged { detail: String },
    IncompleteManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnTopologyDelta {
    pub track: u32,
    pub changes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnFilesystemDelta {
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnRebuildProof {
    /// This is set only by comparing two outputs made from identical inputs;
    /// one before/after comparison cannot prove determinism.
    pub deterministic: bool,
    pub changed_sectors: Vec<u64>,
    pub preserved_sectors: Vec<u64>,
    pub topology_delta: Vec<SaturnTopologyDelta>,
    pub system_id_delta: Vec<String>,
    pub audio_invariant: bool,
    pub filesystem_delta: Vec<SaturnFilesystemDelta>,
    pub warnings: Vec<String>,
    pub refusals: Vec<SaturnRebuildRefusal>,
}

impl SaturnRebuildProof {
    pub fn safe_for_production_rebuild(&self) -> bool {
        self.refusals.is_empty()
            && self.audio_invariant
            && self.system_id_delta.is_empty()
            && self.topology_delta.is_empty()
            && self.deterministic
    }
}

fn data_track(
    manifest: &SaturnDiscManifest,
) -> Result<&crate::saturn_disc_manifest::SaturnTrackManifest, SaturnRebuildRefusal> {
    let mut tracks = manifest
        .tracks
        .iter()
        .filter(|track| track.track_type == SaturnTrackType::Data);
    let Some(track) = tracks.next() else {
        return Err(SaturnRebuildRefusal::MissingDataTrack);
    };
    if tracks.next().is_some() {
        return Err(SaturnRebuildRefusal::MultipleDataTracks);
    }
    Ok(track)
}

fn read_sector(
    file: &mut File,
    offset: u64,
) -> Result<[u8; PROOF_SECTOR_BYTES as usize], SaturnRebuildRefusal> {
    let mut bytes = [0_u8; PROOF_SECTOR_BYTES as usize];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut bytes))
        .map_err(|error| SaturnRebuildRefusal::DataTrackReadFailed {
            detail: error.to_string(),
        })?;
    Ok(bytes)
}

fn compare_data_sectors(
    before: &Path,
    after: &Path,
    before_start: u64,
    after_start: u64,
    before_count: u64,
    after_count: u64,
) -> Result<(Vec<u64>, Vec<u64>), SaturnRebuildRefusal> {
    let mut before_file =
        File::open(before).map_err(|error| SaturnRebuildRefusal::DataTrackReadFailed {
            detail: error.to_string(),
        })?;
    let mut after_file =
        File::open(after).map_err(|error| SaturnRebuildRefusal::DataTrackReadFailed {
            detail: error.to_string(),
        })?;
    let common = before_count.min(after_count);
    let mut changed = Vec::new();
    let mut preserved = Vec::new();
    for sector in 0..common {
        let left = read_sector(
            &mut before_file,
            before_start
                .checked_add(sector.checked_mul(PROOF_SECTOR_BYTES).ok_or_else(|| {
                    SaturnRebuildRefusal::DataTrackReadFailed {
                        detail: "sector offset overflow".into(),
                    }
                })?)
                .ok_or_else(|| SaturnRebuildRefusal::DataTrackReadFailed {
                    detail: "sector offset overflow".into(),
                })?,
        )?;
        let right = read_sector(
            &mut after_file,
            after_start
                .checked_add(sector.checked_mul(PROOF_SECTOR_BYTES).ok_or_else(|| {
                    SaturnRebuildRefusal::DataTrackReadFailed {
                        detail: "sector offset overflow".into(),
                    }
                })?)
                .ok_or_else(|| SaturnRebuildRefusal::DataTrackReadFailed {
                    detail: "sector offset overflow".into(),
                })?,
        )?;
        if left == right {
            preserved.push(sector);
        } else {
            changed.push(sector);
        }
    }
    changed.extend(common..before_count.max(after_count));
    Ok((changed, preserved))
}

fn topology_delta(
    before: &SaturnDiscManifest,
    after: &SaturnDiscManifest,
) -> Vec<SaturnTopologyDelta> {
    let mut deltas = Vec::new();
    for (left, right) in before.tracks.iter().zip(after.tracks.iter()) {
        let mut changes = Vec::new();
        if left.number != right.number {
            changes.push("track number".into());
        }
        if left.track_type != right.track_type {
            changes.push("track type".into());
        }
        if left.mode != right.mode {
            changes.push("track mode".into());
        }
        if left.sector_size != right.sector_size {
            changes.push("sector size".into());
        }
        if left.sector_count != right.sector_count {
            changes.push("sector count".into());
        }
        if left.index_00_frame != right.index_00_frame
            || left.index_01_frame != right.index_01_frame
        {
            changes.push("INDEX 00/01".into());
        }
        if left.in_file_pregap_frames != right.in_file_pregap_frames
            || left.declared_pregap_frames != right.declared_pregap_frames
        {
            changes.push("pregap".into());
        }
        if left.postgap_frames != right.postgap_frames {
            changes.push("postgap".into());
        }
        if !changes.is_empty() {
            deltas.push(SaturnTopologyDelta {
                track: left.number,
                changes,
            });
        }
    }
    if before.tracks.len() != after.tracks.len() {
        deltas.push(SaturnTopologyDelta {
            track: 0,
            changes: vec!["track count".into()],
        });
    }
    deltas
}

/// Compare one source data track with one candidate rebuilt data track.
/// Paths may differ; all other preservation facts come from the manifests.
pub fn prove_saturn_rebuild(
    before: &SaturnDiscManifest,
    after: &SaturnDiscManifest,
    before_data_path: &Path,
    after_data_path: &Path,
) -> SaturnRebuildProof {
    let mut proof = SaturnRebuildProof {
        deterministic: false,
        changed_sectors: Vec::new(),
        preserved_sectors: Vec::new(),
        topology_delta: topology_delta(before, after),
        system_id_delta: Vec::new(),
        audio_invariant: true,
        filesystem_delta: Vec::new(),
        warnings: Vec::new(),
        refusals: Vec::new(),
    };
    if !matches!(
        before.status,
        SaturnManifestStatus::Complete | SaturnManifestStatus::CompleteWithWarnings
    ) || !matches!(
        after.status,
        SaturnManifestStatus::Complete | SaturnManifestStatus::CompleteWithWarnings
    ) {
        proof
            .refusals
            .push(SaturnRebuildRefusal::IncompleteManifest);
    }
    if before.descriptor_sha256 != after.descriptor_sha256 {
        proof.refusals.push(SaturnRebuildRefusal::DescriptorChanged);
    }
    if before.system_id != after.system_id {
        proof
            .system_id_delta
            .push("System ID content or location changed".into());
        proof.refusals.push(SaturnRebuildRefusal::SystemIdChanged);
    }
    for left in before
        .tracks
        .iter()
        .filter(|track| track.track_type == SaturnTrackType::Audio)
    {
        match after
            .tracks
            .iter()
            .find(|track| track.number == left.number)
        {
            Some(right) if left.audio_sha256 == right.audio_sha256 => {}
            _ => {
                proof.audio_invariant = false;
                proof
                    .refusals
                    .push(SaturnRebuildRefusal::AudioChanged { track: left.number });
            }
        }
    }
    if !proof.topology_delta.is_empty() {
        proof.refusals.push(SaturnRebuildRefusal::TopologyChanged {
            detail: "track topology, index, pregap, postgap, mode, or length changed".into(),
        });
    }
    match (data_track(before), data_track(after)) {
        (Ok(left), Ok(right))
            if left.mode == SaturnTrackMode::Mode1_2048
                && right.mode == SaturnTrackMode::Mode1_2048 =>
        {
            match compare_data_sectors(
                before_data_path,
                after_data_path,
                left.data_byte_offset,
                right.data_byte_offset,
                left.sector_count,
                right.sector_count,
            ) {
                Ok((changed, preserved)) => {
                    proof.changed_sectors = changed;
                    proof.preserved_sectors = preserved;
                }
                Err(error) => proof.refusals.push(error),
            }
        }
        (Ok(left), Ok(right)) => {
            let mode = if left.mode != SaturnTrackMode::Mode1_2048 {
                left.mode
            } else {
                right.mode
            };
            proof
                .refusals
                .push(SaturnRebuildRefusal::UnsupportedSectorMode { mode });
        }
        (Err(error), _) => proof.refusals.push(error),
        (_, Err(error)) => proof.refusals.push(error),
    }
    if proof.changed_sectors.is_empty() {
        proof.warnings.push(
            "No data-track sector changed; this is not evidence that a replacement was applied."
                .into(),
        );
    }
    proof
}

/// Compare two independently generated outputs while ignoring path names.
/// This is the only operation that can set the deterministic proof flag.
pub fn deterministic_output_match(first: &SaturnDiscManifest, second: &SaturnDiscManifest) -> bool {
    first.descriptor_sha256 == second.descriptor_sha256
        && first.components.len() == second.components.len()
        && first
            .components
            .iter()
            .zip(&second.components)
            .all(|(left, right)| left.sha256 == right.sha256 && left.size_bytes == right.size_bytes)
        && first
            .tracks
            .iter()
            .zip(&second.tracks)
            .all(|(left, right)| {
                left.number == right.number
                    && left.track_type == right.track_type
                    && left.mode == right.mode
                    && left.sector_size == right.sector_size
                    && left.sector_count == right.sector_count
                    && left.index_00_frame == right.index_00_frame
                    && left.index_01_frame == right.index_01_frame
                    && left.in_file_pregap_frames == right.in_file_pregap_frames
                    && left.declared_pregap_frames == right.declared_pregap_frames
                    && left.postgap_frames == right.postgap_frames
                    && left.audio_sha256 == right.audio_sha256
                    && left.data_logical_sha256 == right.data_logical_sha256
            })
        && first.system_id == second.system_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::saturn_disc_manifest::inspect_saturn_disc;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn system_id() -> Vec<u8> {
        let mut bytes = vec![b' '; 2048];
        bytes[..16].copy_from_slice(b"SEGA SEGASATURN ");
        bytes[0x10..0x20].copy_from_slice(b"SYNTHETIC AUTHOR");
        bytes[0x20..0x2a].copy_from_slice(b"T-9000G   ");
        bytes[0x2a..0x30].copy_from_slice(b"V1.000");
        bytes[0x30..0x38].copy_from_slice(b"20000101");
        bytes[0x38..0x40].copy_from_slice(b"CD-1/1  ");
        bytes[0x40..0x4a].copy_from_slice(b"JTU       ");
        bytes[0x50..0x60].copy_from_slice(b"JAMKST          ");
        bytes[0x60..0x60 + 16].copy_from_slice(b"SYNTHETIC SATURN");
        bytes
    }

    fn directory_record(name: &[u8], extent: u32, size: u32) -> Vec<u8> {
        let length = (33 + name.len() + usize::from(name.len().is_multiple_of(2))) as u8;
        let mut record = vec![0_u8; length as usize];
        record[0] = length;
        record[2..6].copy_from_slice(&extent.to_le_bytes());
        record[10..14].copy_from_slice(&size.to_le_bytes());
        record[18..25].copy_from_slice(&[100, 1, 1, 0, 0, 0, 0]);
        record[25] = 0;
        record[28..30].copy_from_slice(&1_u16.to_le_bytes());
        record[30..32].copy_from_slice(&1_u16.to_be_bytes());
        record[32] = name.len() as u8;
        record[33..33 + name.len()].copy_from_slice(name);
        record
    }

    fn synthetic_iso(replacement_size: usize, replacement_byte: u8) -> Vec<u8> {
        let replacement_sectors = replacement_size.div_ceil(2048);
        let other_extent = 22 + replacement_sectors as u32;
        let total_sectors = other_extent as usize + 1;
        let mut data = vec![0_u8; total_sectors * 2048];
        data[2048..4096].copy_from_slice(&system_id());
        let pvd = &mut data[16 * 2048..17 * 2048];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[6] = 1;
        pvd[40..49].copy_from_slice(b"SYNTHETIC");
        pvd[80..84].copy_from_slice(&(total_sectors as u32).to_le_bytes());
        pvd[84..88].copy_from_slice(&(total_sectors as u32).to_be_bytes());
        pvd[128..130].copy_from_slice(&2048_u16.to_le_bytes());
        pvd[130..132].copy_from_slice(&2048_u16.to_be_bytes());
        let root_record = directory_record(&[0], 21, 2048);
        pvd[156..156 + root_record.len()].copy_from_slice(&root_record);
        let mut root = Vec::new();
        root.extend(directory_record(&[0], 21, 2048));
        root.extend(directory_record(&[1], 21, 2048));
        root.extend(directory_record(
            b"REPLACE.BIN;1",
            22,
            replacement_size as u32,
        ));
        root.extend(directory_record(b"UNCHANGED.TXT;1", other_extent, 1024));
        data[21 * 2048..21 * 2048 + root.len()].copy_from_slice(&root);
        data[22 * 2048..22 * 2048 + replacement_size].fill(replacement_byte);
        data[other_extent as usize * 2048..other_extent as usize * 2048 + 1024].fill(0x7a);
        data
    }

    fn unique_dir(label: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("emuwiz-saturn-proof-{label}-{suffix}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_disc(
        label: &str,
        replacement_size: usize,
        replacement_byte: u8,
    ) -> (PathBuf, PathBuf) {
        let root = unique_dir(label);
        let cue = root.join("synthetic.cue");
        std::fs::write(
            &cue,
            "FILE \"data.bin\" BINARY\nTRACK 01 MODE1/2048\nPREGAP 00:00:02\nINDEX 00 00:00:00\nINDEX 01 00:00:01\nFILE \"audio01.bin\" BINARY\nTRACK 02 AUDIO\nPREGAP 00:00:02\nINDEX 01 00:00:00\nFILE \"audio02.bin\" BINARY\nTRACK 03 AUDIO\nINDEX 00 00:00:00\nINDEX 01 00:00:01\n",
        )
        .unwrap();
        let data = root.join("data.bin");
        std::fs::write(&data, synthetic_iso(replacement_size, replacement_byte)).unwrap();
        std::fs::write(root.join("audio01.bin"), vec![0x11_u8; 2352 * 2]).unwrap();
        std::fs::write(root.join("audio02.bin"), vec![0x22_u8; 2352 * 2]).unwrap();
        (cue, data)
    }

    #[test]
    fn repeated_synthetic_builds_are_byte_identical_and_same_size_change_is_local() {
        let (first_cue, first_data) = write_disc("repeat-a", 2048, 0x41);
        let (second_cue, second_data) = write_disc("repeat-b", 2048, 0x41);
        let first = inspect_saturn_disc(&first_cue).unwrap();
        let second = inspect_saturn_disc(&second_cue).unwrap();
        assert!(deterministic_output_match(&first, &second));

        let (base_cue, base_data) = write_disc("base", 2048, 0x41);
        let (changed_cue, changed_data) = write_disc("changed", 2048, 0x42);
        let base = inspect_saturn_disc(&base_cue).unwrap();
        let changed = inspect_saturn_disc(&changed_cue).unwrap();
        let mut proof = prove_saturn_rebuild(&base, &changed, &base_data, &changed_data);
        proof.deterministic = deterministic_output_match(&first, &second);
        assert!(proof.deterministic);
        assert!(proof.refusals.is_empty());
        assert!(proof.audio_invariant);
        assert!(proof.topology_delta.is_empty());
        assert!(!proof.changed_sectors.is_empty());
        assert!(proof.preserved_sectors.len() > proof.changed_sectors.len());
        assert_eq!(
            std::fs::read(first_data).unwrap(),
            std::fs::read(second_data).unwrap()
        );
    }

    #[test]
    fn smaller_and_larger_replacements_refuse_extent_movement() {
        let (base_cue, base_data) = write_disc("extent-base", 4096, 0x41);
        let base = inspect_saturn_disc(&base_cue).unwrap();
        for (label, size) in [("smaller", 2048_usize), ("larger", 6144_usize)] {
            let (cue, data) = write_disc(label, size, 0x43);
            let output = inspect_saturn_disc(&cue).unwrap();
            let proof = prove_saturn_rebuild(&base, &output, &base_data, &data);
            assert!(
                proof
                    .topology_delta
                    .iter()
                    .any(|delta| delta.changes.iter().any(|change| change == "sector count"))
            );
            assert!(
                proof
                    .refusals
                    .iter()
                    .any(|refusal| matches!(refusal, SaturnRebuildRefusal::TopologyChanged { .. }))
            );
            assert!(proof.audio_invariant);
        }
    }
}
