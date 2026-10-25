//! Bounded image-to-IP.BIN adapter. No image extraction, rebuilding or writes.
use super::ip_bin::{IP_BIN_BYTES, IpBinInspection, inspect_ip_bin};
use crate::dreamcast_patch_readiness::{self as dcp, DreamcastPatchReadiness};
use crate::ingestion::gdi::{self, GdiDataTrackMode, GdiDescriptor};
use crate::logical_media::LogicalMedia;
use crate::raw_cd_logical_media::{
    open_cooked_cd_file_logical_media, open_raw_cd_file_logical_media,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileObservation {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
}
fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
fn observe(path: &Path) -> io::Result<FileObservation> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(invalid("expected a regular non-symlink file"));
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(FileObservation {
        path: path.to_owned(),
        size: meta.len(),
        modified: meta.modified()?,
        #[cfg(unix)]
        device: meta.dev(),
        #[cfg(unix)]
        inode: meta.ino(),
    })
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// This is a boot-area snapshot, NOT a whole-image hash or target binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DreamcastMediaInspection {
    pub source: PathBuf,
    pub files: Vec<FileObservation>,
    pub descriptor_sha256: Option<String>,
    pub topology: Option<GdiDescriptor>,
    pub data_path: PathBuf,
    pub boot_sha256: String,
    pub inspection: IpBinInspection,
}

fn descriptor_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(invalid("GDI exceeds 64 KiB"));
    }
    Ok(bytes)
}

/// Supported: GDI with the canonical resolver's proven zero-offset layout,
/// and whole-sector cooked ISO images. Other containers are explicitly refused.
pub fn inspect_image(path: &Path) -> io::Result<DreamcastMediaInspection> {
    let source = observe(path)?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut files = vec![source];
    let mut topology = None;
    let mut descriptor_sha256 = None;
    let mut descriptor_snapshot = None;
    let (data_path, raw) = match ext.as_str() {
        "gdi" => {
            let before = descriptor_bytes(path)?;
            // Resolve through the canonical identity policy: preserve every
            // track relationship and refuse unsupported offsets/overlaps.
            let desc = gdi::parse_gdi_descriptor(path).map_err(invalid)?;
            let data = gdi::data_track_from_descriptor(&desc).map_err(invalid)?;
            if desc != gdi::parse_gdi_descriptor(path).map_err(invalid)?
                || before != descriptor_bytes(path)?
            {
                return Err(invalid("GDI changed during inspection"));
            }
            for track in &desc.tracks {
                let observed = observe(&track.source_path)?;
                if observed.size != track.source_length_bytes {
                    return Err(invalid("GDI track changed during inspection"));
                }
                files.push(observed);
            }
            descriptor_sha256 = Some(hash(&before));
            descriptor_snapshot = Some(before);
            topology = Some(desc);
            (data.path, data.mode == GdiDataTrackMode::Raw2352)
        }
        "iso" => (path.to_owned(), false),
        _ => {
            return Err(invalid(
                "This inspector supports GDI and cooked 2048-byte ISO only; CDI, CHD, CUE and loose BIN are unsupported",
            ));
        }
    };
    let media: Box<dyn LogicalMedia> = if raw {
        Box::new(open_raw_cd_file_logical_media(&data_path).map_err(invalid)?)
    } else {
        Box::new(open_cooked_cd_file_logical_media(&data_path)?)
    };
    let mut boot = vec![0; IP_BIN_BYTES];
    media.read_at(0, &mut boot).map_err(invalid)?;
    let inspection = inspect_ip_bin(&boot);
    for observed in &files {
        if observe(&observed.path)? != *observed {
            return Err(invalid("source changed during inspection"));
        }
    }
    if let Some(before) = descriptor_snapshot {
        if topology.as_ref() != Some(&gdi::parse_gdi_descriptor(path).map_err(invalid)?)
            || descriptor_bytes(path)? != before
        {
            return Err(invalid("GDI changed during inspection"));
        }
    }
    Ok(DreamcastMediaInspection {
        source: path.to_owned(),
        files,
        descriptor_sha256,
        topology,
        data_path,
        boot_sha256: hash(&boot),
        inspection,
    })
}

/// Review a local package against a fresh boot snapshot. Standard DCP carries
/// no exact target binding, so this never establishes compatibility or Apply.
/// Re-reading catches changed boot bytes/descriptors, plus observed track stat
/// changes. Unread sectors are deliberately NOT claimed verified unchanged.
pub fn review_patch(
    expected: &DreamcastMediaInspection,
    patch: &Path,
) -> io::Result<DreamcastPatchReadiness> {
    if inspect_image(&expected.source)? != *expected {
        return Err(invalid("STALE SOURCE: inspect the image again"));
    }
    let package = dcp::inspect_dreamcast_dcp(patch).map_err(invalid)?;
    if inspect_image(&expected.source)? != *expected {
        return Err(invalid(
            "STALE SOURCE: source changed during package inspection",
        ));
    }
    let metadata = expected.inspection.ip_bin.as_ref().map(|ip| &ip.metadata);
    let evidence = dcp::DreamcastPatchTargetEvidence {
        product_code: metadata.map(|m| m.product_number.value.clone()),
        input_format: expected
            .source
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_owned),
        has_audio_tracks: expected.topology.as_ref().is_some_and(|d| {
            d.tracks
                .iter()
                .any(|t| t.track_type == gdi::GdiTrackType::Audio)
        }),
        ip_bin: metadata.map(Into::into),
        ..Default::default()
    };
    Ok(dcp::assess_dreamcast_patch_readiness(
        &package,
        &Default::default(),
        &evidence,
    ))
}

#[cfg(test)]
mod tests;
