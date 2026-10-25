use super::{adapters::*, model::*};
use crate::{
    platform_evidence_fusion::cue_m3u_parsing::{
        MAX_PARSE_BYTES, parse_cue_file_references, parse_m3u_references,
    },
    safe_read::{TrustedRoots, open_bounded_read},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
};

#[derive(Debug, Clone)]
pub struct InspectionLimits {
    pub max_files: usize,
    pub max_read_bytes: u64,
    pub native_optical: bool,
}
impl Default for InspectionLimits {
    fn default() -> Self {
        Self {
            max_files: 256,
            max_read_bytes: 128 * 1024 * 1024,
            native_optical: true,
        }
    }
}
/// Explicit inputs only: no catalogue DB, recursive walk, hashing, writes or subprocesses.
/// M3U references are bounded inputs, never authority for a complete release.
pub fn inspect_paths(
    paths: &[PathBuf],
    platform: Option<&str>,
    trusted: &TrustedRoots,
    limits: &InspectionLimits,
) -> Vec<MediaRecord> {
    let mut queue = paths
        .iter()
        .cloned()
        .map(|p| (p, None, BTreeSet::new()))
        .collect::<VecDeque<_>>();
    let mut result: BTreeMap<PathBuf, MediaRecord> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut companions = BTreeSet::new();
    let mut bytes = 0u64;
    let mut processed = 0usize;
    while let Some((path, playlist, mut ancestors)) = queue.pop_front() {
        if !seen.insert(path.clone()) {
            if let Some(e) = playlist
                && let Some(record) = result.get_mut(&path)
            {
                record.evidence.push(e);
            }
            continue;
        }
        let mut record = media_record(&path, platform);
        if let Some(e) = playlist {
            record.evidence.push(e);
        }
        if processed >= limits.max_files {
            record
                .warnings
                .push("Inspection file limit reached; input retained as unverified".into());
            result.insert(path, record);
            continue;
        }
        processed += 1;
        let mut file = match open_bounded_read(&path, trusted) {
            Ok(f) => f,
            Err(reason) => {
                record.availability = if matches!(path.try_exists(), Ok(false)) {
                    MediaAvailability::Missing
                } else {
                    MediaAvailability::Unverified
                };
                record
                    .warnings
                    .push(format!("Source unavailable: {}", reason.detail()));
                result.insert(path, record);
                continue;
            }
        };
        record.availability = MediaAvailability::Observed;
        if record.format == "m3u" || record.format == "cue" {
            if file.len() > MAX_PARSE_BYTES as u64
                || bytes.saturating_add(file.len()) > limits.max_read_bytes
            {
                record
                    .warnings
                    .push("Descriptor inspection exceeded its read budget".into());
                record.availability = MediaAvailability::Unverified;
                result.insert(path, record);
                continue;
            }
            let data = file
                .read_exact_at(0, file.len() as usize, MAX_PARSE_BYTES)
                .unwrap_or_default();
            bytes += data.len() as u64;
            let text = match std::str::from_utf8(&data) {
                Ok(s) => s,
                Err(_) => {
                    record
                        .warnings
                        .push("Descriptor text is not UTF-8; references unverified".into());
                    record.availability = MediaAvailability::Unverified;
                    result.insert(path, record);
                    continue;
                }
            };
            let refs = if record.format == "m3u" {
                parse_m3u_references(&path, text)
            } else {
                parse_cue_file_references(&path, text)
            };
            if record.format == "m3u" {
                ancestors.insert(path.clone());
                let mut accepted = 0;
                let mut rejected = false;
                for (i, reference) in refs.into_iter().enumerate() {
                    if let Some(target) = reference.resolved {
                        if ancestors.contains(&target)
                            || (seen.contains(&target)
                                && target
                                    .extension()
                                    .is_some_and(|s| s.eq_ignore_ascii_case("m3u")))
                            || processed + queue.len() >= limits.max_files
                        {
                            rejected = true;
                            continue;
                        }
                        let mut e = MediaEvidence::new(
                            EvidenceKind::Metadata,
                            format!("M3U order from {}", path.display()),
                        );
                        if let Ok(number) = u16::try_from(i + 1) {
                            e.ordinal = Some(MediaOrdinal {
                                number,
                                unit: OrdinalUnit::Medium,
                            });
                        }
                        e.notes.push("Playlist position is an ordering hint, not a release identity or expected total".into());
                        queue.push_back((target, Some(e), ancestors.clone()));
                        accepted += 1;
                    } else {
                        rejected = true;
                    }
                }
                if rejected || accepted == 0 {
                    record.warnings.push("Playlist has rejected, cyclic, empty or over-budget references; no complete playlist topology is claimed".into());
                    result.insert(path, record);
                }
                continue;
            }
            if refs.is_empty() {
                record.availability = MediaAvailability::Unverified;
                record
                    .warnings
                    .push("CUE has no usable file references".into());
            }
            for reference in refs {
                if let Some(target) = reference.resolved {
                    companions.insert(target.clone());
                    if open_bounded_read(&target, trusted).is_err() {
                        record.availability = MediaAvailability::Missing;
                        record
                            .warnings
                            .push(format!("CUE companion unavailable: {}", reference.raw));
                    }
                } else {
                    record.availability = MediaAvailability::Unverified;
                    record
                        .warnings
                        .push("CUE contains a rejected companion reference".into());
                }
            }
        }
        if record.format == "gdi" {
            if file.len() > MAX_PARSE_BYTES as u64
                || file.len() > limits.max_read_bytes.saturating_sub(bytes)
            {
                record.availability = MediaAvailability::Unverified;
                record
                    .warnings
                    .push("GDI descriptor exceeds inspection budget".into());
                result.insert(path, record);
                continue;
            }
            bytes += file.len();
            match crate::ingestion::gdi::resolve_gdi_all_tracks_lenient(&path) {
                Ok(tracks) => {
                    for track in tracks {
                        match track {
                            Ok(target) => {
                                companions.insert(target);
                            }
                            Err(reason) => {
                                record.availability = MediaAvailability::Missing;
                                record
                                    .warnings
                                    .push(format!("GDI companion unavailable: {reason}"));
                            }
                        }
                    }
                }
                Err(reason) => {
                    record.availability = MediaAvailability::Unverified;
                    record
                        .warnings
                        .push(format!("GDI structure unavailable: {reason}"));
                }
            }
        }
        // The existing ADZ adapter creates an anonymous decompression file; do
        // not call it here. ADF inspection uses the already-approved pinned fd.
        #[cfg(target_os = "linux")]
        if record.format == "adf"
            && file.len() <= crate::disk_format::MAX_RAW_FLOPPY_BYTES
            && limits.max_read_bytes.saturating_sub(bytes)
                >= crate::disk_format::MAX_RAW_FLOPPY_BYTES
        {
            use std::os::fd::AsRawFd;
            let handle = file.into_file();
            let pinned = PathBuf::from(format!("/proc/self/fd/{}", handle.as_raw_fd()));
            match crate::amiga_disk::inspect_amiga_floppy(&pinned) {
                Ok(inspection) => attach_amiga_floppy(&mut record, &inspection),
                Err(reason) => record
                    .warnings
                    .push(format!("Amiga filesystem not established: {reason}")),
            }
            bytes += crate::disk_format::MAX_RAW_FLOPPY_BYTES;
            result.insert(path, record);
            continue;
        }
        match record.family {
            Some(MediaFamily::Optical)
                if limits.native_optical && limits.max_read_bytes.saturating_sub(bytes) > 0 =>
            {
                let native =
                    crate::game_identity::inspect_catalogued_game_identity_in_roots_with_budget(
                        &path,
                        platform,
                        trusted,
                        limits.max_read_bytes.saturating_sub(bytes),
                    );
                bytes = bytes.saturating_add(native.bytes_read);
                attach_native_identity(&mut record, &native);
            }
            Some(MediaFamily::Floppy)
                if limits.max_read_bytes.saturating_sub(bytes)
                    >= crate::disk_format::MAX_DISK_FORMAT_BYTES_READ =>
            {
                let native = crate::disk_format::inspect_disk_format(
                    &path,
                    trusted,
                    crate::disk_format::DiskFormatContext {
                        folder_platform: platform,
                    },
                    None,
                );
                bytes = bytes.saturating_add(native.bytes_inspected);
                attach_disk_evidence(&mut record, &native);
            }
            Some(MediaFamily::Tape)
                if file.len() <= crate::tape_analysis::MAX_ANALYSIS_BYTES as u64
                    && file.len() <= limits.max_read_bytes.saturating_sub(bytes) =>
            {
                let data = file
                    .read_exact_at(
                        0,
                        file.len() as usize,
                        crate::tape_analysis::MAX_ANALYSIS_BYTES,
                    )
                    .unwrap_or_default();
                bytes += data.len() as u64;
                match crate::tape_analysis::analyze_tape(&data) {
                    Ok(analysis) => attach_tape_analysis(&mut record, &analysis),
                    Err(reason) => record
                        .warnings
                        .push(format!("Deep tape analysis unavailable: {reason:?}")),
                }
            }
            _ => record
                .warnings
                .push("No native inspection was available within the selected budget".into()),
        }
        result.insert(path, record);
    }
    result
        .into_iter()
        .filter_map(|(path, record)| (!companions.contains(&path)).then_some(record))
        .collect()
}
