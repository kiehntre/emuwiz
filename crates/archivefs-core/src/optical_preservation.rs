//! Shared preservation admission and verification for CD preview/execution.
//! Uses the existing CueLayout; not a general CUE
//! parser. The inspection parser intentionally exposes a data-track view and
//! drops some CUE declarations. Validate the original text before using that
//! view as conversion evidence, so discarded facts cannot authorize output.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::chd_identity::{
    ChdMetadataFact, ChdMetadataOutcome, meta_tag, observe_chd_identity_file,
};
use crate::dat::archive::chd::read_chd_v5_header;
use crate::ingestion::cue_bin::{CueLayout, resolve_cue_layout_text};
use crate::repair::optical_conversion::ChdConversionSourceMode;

pub(crate) const MAX_CONVERSION_CUE_BYTES: u64 = crate::ingestion::cue_bin::MAX_CUE_BYTES;

/// Prove exactly one BINARY FILE, TRACK 01 MODE1/2048, and INDEX 01 at file
/// frame zero. Anything else needs a wider layout verifier before conversion
/// can be admitted. In particular, even zero-length gap declarations are
/// refused; no declaration is silently discarded or normalized.
pub(crate) fn source_layout(
    path: &Path,
    source_mode: ChdConversionSourceMode,
) -> Result<CueLayout, String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_CONVERSION_CUE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > MAX_CONVERSION_CUE_BYTES {
        return Err("CUE exceeds the conversion size limit".into());
    }
    let mut state = 0;
    let mut source_name = "";
    for (line_number, raw) in text.lines().enumerate() {
        let line = raw.trim_matches([' ', '\t', '\r']);
        if line.is_empty() {
            continue;
        }
        let invalid = || {
            format!(
                "line {} is outside the verified layout: requires one BINARY FILE, TRACK 01 MODE1/2048, INDEX 01 00:00:00; gaps, additional indexes and other directives are not verified",
                line_number + 1
            )
        };
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return Err(invalid());
        }
        let mut fields = line.split_ascii_whitespace();
        let directive = fields.next().ok_or_else(invalid)?;
        // These annotations carry no track/index/sector semantics. The original
        // CUE remains byte-bound and retained; do not claim CHD preserves CD-text.
        if directive.eq_ignore_ascii_case("REM")
            && fields.next().is_some_and(|kind| {
                kind.eq_ignore_ascii_case("COMMENT") || kind.eq_ignore_ascii_case("GENRE")
            })
        {
            continue;
        }
        if directive.eq_ignore_ascii_case("TITLE") {
            let value = line[directive.len()..].trim_matches([' ', '\t']);
            if value.starts_with('"')
                && value.ends_with('"')
                && value.len() >= 2
                && !value[1..value.len() - 1].contains('"')
            {
                continue;
            }
            return Err(invalid());
        }
        match state {
            0 if directive.eq_ignore_ascii_case("FILE") => {
                let rest = line[directive.len()..].trim_start_matches([' ', '\t']);
                let quoted = rest.strip_prefix('"').ok_or_else(invalid)?;
                let end = quoted.find('"').ok_or_else(invalid)?;
                let name = &quoted[..end];
                if name.is_empty()
                    || name.contains('\\')
                    || !quoted[end + 1..]
                        .trim_matches([' ', '\t'])
                        .eq_ignore_ascii_case("BINARY")
                {
                    return Err(invalid());
                }
                source_name = name;
            }
            1 if directive.eq_ignore_ascii_case("TRACK") => {
                if !matches!(fields.next(), Some("01" | "1"))
                    || !fields
                        .next()
                        .is_some_and(|mode| mode.eq_ignore_ascii_case("MODE1/2048"))
                    || fields.next().is_some()
                {
                    return Err(invalid());
                }
            }
            2 if directive.eq_ignore_ascii_case("INDEX") => {
                if !matches!(fields.next(), Some("01" | "1"))
                    || fields.next() != Some("00:00:00")
                    || fields.next().is_some()
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        state += 1;
    }
    if state != 3 {
        return Err("incomplete verified CUE layout: FILE, TRACK and INDEX 01 are required".into());
    }
    // Reuse the existing resolver for canonical paths, containment and modes.
    // The gate above also keeps unrecognized UTF-8 directives away from that
    // parser's byte-prefix slicing and prevents FILE type reinterpretation.
    let layout = resolve_cue_layout_text(path, &text).map_err(|error| error.to_string())?;
    let track = layout
        .supported_single_mode1_2048()
        .map_err(|error| error.to_string())?;
    if track.number != 1
        || track.index_01.is_none_or(|index| index.frames != 0)
        || track.index_00.is_some()
        || track.pregap.is_some()
        || track.postgap.is_some()
    {
        return Err("CUE is outside the verified layout".into());
    }
    if std::fs::canonicalize(path).map_err(|error| error.to_string())? == track.path {
        return Err("CUE cannot reference itself as a data component".into());
    }
    // The legacy quarantine mover flattens both basenames into one directory.
    // Admit it only when that preserves the original FILE reference verbatim.
    if source_mode == ChdConversionSourceMode::QuarantineSource
        && Path::new(source_name)
            .components()
            .filter(|component| *component != std::path::Component::CurDir)
            .ne([std::path::Component::Normal(
                track.path.file_name().ok_or("BIN has no filename")?,
            )])
    {
        return Err(
            "source quarantine cannot preserve this FILE reference; keep the source instead".into(),
        );
    }
    let size = std::fs::metadata(&track.path)
        .map_err(|error| error.to_string())?
        .len();
    if size == 0 || !size.is_multiple_of(2048) || size / 2048 > i32::MAX as u64 {
        return Err(
            "BIN must be a non-empty aligned MODE1/2048 stream within converter frame bounds"
                .into(),
        );
    }
    Ok(layout)
}

/// Check the track facts independently of the cooked payload fingerprint.
/// Only the exact single-track CHT2 metadata emitted for our admitted source
/// is supported. Unknown/session/legacy entries cannot establish equivalence.
pub(crate) fn verify_output_layout(path: &Path, frames: u64) -> Result<(), String> {
    let identity = observe_chd_identity_file(path).map_err(|error| error.to_string())?;
    let ChdMetadataOutcome::Observed(metadata) = identity.metadata else {
        return Err("CHD layout metadata is unavailable or malformed".into());
    };
    let [entry] = metadata.entries.as_slice() else {
        return Err("CHD must contain exactly one verified CHT2 track entry".into());
    };
    let ChdMetadataFact::CdromTrack(_) = &entry.fact else {
        return Err("CHD does not contain a verified CD track".into());
    };
    let expected = format!(
        "TRACK:1 TYPE:MODE1 SUBTYPE:NONE FRAMES:{frames} PREGAP:0 PGTYPE:MODE1 PGSUB:NONE POSTGAP:0"
    );
    if entry.tag != meta_tag::CDROM_TRACK2
        || !(expected.len()..=expected.len() + 1).contains(&(entry.length as usize))
    {
        return Err("CHD track mode, frame count, gaps or subchannel facts differ from the verified CUE layout".into());
    }
    // The general metadata observer uses a token map. Compare the original
    // payload too: duplicate/unknown tokens and data after a NUL must never
    // be hidden by that projection (MAME reads the fixed CHT2 field order).
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let header = read_chd_v5_header(&mut file).map_err(|error| error.to_string())?;
    let offset = header
        .meta_offset
        .checked_add(16)
        .ok_or("CHD metadata offset overflows")?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut payload = vec![0; entry.length as usize];
    file.read_exact(&mut payload)
        .map_err(|error| error.to_string())?;
    let text = payload.strip_suffix(&[0]).unwrap_or(&payload);
    if text != expected.as_bytes() {
        return Err("CHD metadata text differs from the verified layout contract".into());
    }
    Ok(())
}

/// A payload match cannot certify missing hunks: the decoder intentionally
/// zero-fills sparse hunks and can also return zeros for truncated raw hunks.
/// Use its parsed map to distinguish the two before fingerprinting the output.
pub(crate) fn verify_output_storage(path: &Path, frames: u64) -> Result<(), String> {
    use chd::map::{CompressionTypeV5, MapEntry};

    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let file_len = file.metadata().map_err(|error| error.to_string())?.len();
    let header = read_chd_v5_header(&mut file).map_err(|error| error.to_string())?;
    // CHT2 CD tracks are padded to a multiple of four 2448-byte frames.
    // Recorded chdman cases cover partial tracks and multiple hunks.
    if frames == 0
        || frames > i32::MAX as u64
        || header.parent_required()
        || header.unit_bytes != 2448
        || !header.hunk_bytes.is_multiple_of(header.unit_bytes)
        || header.logical_bytes != frames.div_ceil(4) * 4 * 2448
    {
        return Err(
            "CHD geometry or parent dependency is outside the verified track layout".into(),
        );
    }
    let chd = chd::Chd::open(file, None).map_err(|error| error.to_string())?;
    for (index, entry) in chd.map().iter().enumerate() {
        let (offset, size) = match entry {
            MapEntry::V5Uncompressed(entry) => {
                let offset = entry.block_offset().map_err(|error| error.to_string())?;
                if offset == 0 {
                    // An explicit sparse zero hunk is defined by the format.
                    continue;
                }
                (offset, entry.block_size())
            }
            MapEntry::V5Compressed(entry) => {
                let offset = entry.block_offset().map_err(|error| error.to_string())?;
                let size = entry.block_size().map_err(|error| error.to_string())?;
                match entry.hunk_type().map_err(|error| error.to_string())? {
                    CompressionTypeV5::CompressionSelf if offset < index as u64 => {
                        let Some(MapEntry::V5Compressed(target)) =
                            chd.map().get_entry(offset as usize)
                        else {
                            return Err("CHD self-reference has no stored hunk".into());
                        };
                        if matches!(
                            target.hunk_type().map_err(|error| error.to_string())?,
                            CompressionTypeV5::CompressionType0
                                | CompressionTypeV5::CompressionType1
                                | CompressionTypeV5::CompressionType2
                                | CompressionTypeV5::CompressionType3
                                | CompressionTypeV5::CompressionNone
                        ) {
                            continue;
                        }
                        return Err("CHD chained self-references are not supported".into());
                    }
                    CompressionTypeV5::CompressionType0
                    | CompressionTypeV5::CompressionType1
                    | CompressionTypeV5::CompressionType2
                    | CompressionTypeV5::CompressionType3 => {}
                    CompressionTypeV5::CompressionNone if size == header.hunk_bytes => {}
                    _ => return Err("CHD has an unsupported or cyclic hunk reference".into()),
                }
                (offset, size)
            }
            _ => return Err("CHD hunk map version is unsupported".into()),
        };
        if size == 0
            || size > header.hunk_bytes
            || offset < 124
            || offset
                .checked_add(u64::from(size))
                .is_none_or(|end| end > file_len)
        {
            return Err("CHD hunk data is truncated or outside the verified file bounds".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
