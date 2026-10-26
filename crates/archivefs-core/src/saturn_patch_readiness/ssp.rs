//! Bounded read-only SSP-style ZIP inspection, not a patch interpreter.
//! The patcher author's documented ZIP/version.txt convention and the pinned
//! OpenROM builder are evidence for packaging only. No native SSP payload or
//! base/output identity contract is inferred from filenames, MD5 text or titles.
use crate::{
    dat::archive::{limits::ArchiveLimits, zip_preflight::preflight_zip},
    ingestion::cue_bin::resolve_cue_all_files,
    mame_collection_analyser::inspection::SourceStamp,
    safe_read::{TrustedRoots, open_bounded_read},
    saturn_disc_manifest::{SaturnDiscManifest, SaturnManifestStatus, inspect_saturn_disc},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

pub const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_MEMBERS: usize = 512;
pub const MAX_TEXT_BYTES: u64 = 16 * 1024;
pub const FORMAT_REFERENCE: &str =
    "https://segaxtreme.net/threads/sega-saturn-patcher-discussion-thread.24721/page-3";
pub const PRODUCER_REFERENCE: &str = "https://github.com/M5Devs/OpenROM/blob/d9434b9385392d3a9e845b4f24082e543ee3c043/core/lib/src/ssp_builder.dart";
pub const APPLY_BLOCKER: &str = "Apply disabled: native SSP payload/application semantics, exact base/output identity and a verified layout-preserving Saturn rebuild are not established. No output is created or verified.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Member {
    pub name: String,
    pub directory: bool,
    pub logical_bytes: u64,
    pub sha256: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Package {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub source: SourceStamp,
    /// Raw UTF-8 declaration, not authenticated patch/tool/version authority.
    pub version_text: Option<String>,
    /// Observed OpenROM producer metadata; per-file MD5 text is not a disc hash.
    pub changes_md5_text: Option<String>,
    pub members: Vec<Member>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Compatibility {
    /// The selected native Saturn disc was observed, but SSP has no verified
    /// target binding in this reader. This is never Compatible or Applicable.
    UnknownBaseBinding,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Preview {
    pub package: Package,
    pub manifest: SaturnDiscManifest,
    pub manifest_sha256: String,
    pub compatibility: Compatibility,
    pub sources: Vec<SourceStamp>,
    pub optical_layout: super::optical::SaturnOpticalPreflight,
}
impl Preview {
    pub fn can_apply(&self) -> bool {
        false
    }
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn read_package(path: &Path) -> Result<(Vec<u8>, SourceStamp), String> {
    let before = SourceStamp::observe(path)?;
    if before.directory || before.bytes > MAX_PACKAGE_BYTES {
        return Err("SSP package exceeds the regular-file/64 MiB inspection limit.".into());
    }
    let mut file = open_bounded_read(path, &TrustedRoots::none())
        .map_err(|e| e.detail())?
        .into_file();
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.dev() != before.device
            || metadata.ino() != before.inode
            || metadata.len() != before.bytes
            || metadata.ctime() != before.changed_seconds
            || metadata.ctime_nsec() != before.changed_nanoseconds
        {
            return Err("SSP source changed while opening.".into());
        }
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(before.bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != before.bytes
        || !before.still_matches()
        || file.metadata().map_err(|e| e.to_string())?.modified().ok() != before.modified
    {
        return Err("SSP source changed while reading; inspect again.".into());
    }
    Ok((bytes, before))
}

fn safe_name(raw: &[u8], directory: bool) -> Result<String, String> {
    let name = std::str::from_utf8(raw).map_err(|_| "Non-UTF8 ZIP member name is unsupported.")?;
    let name = if directory {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    if name.is_empty()
        || name.len() > 1024
        || name
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == ':')
        || name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err("Unsafe or ambiguous SSP member path.".into());
    }
    Ok(name.to_owned())
}

pub fn inspect_package(path: &Path) -> Result<Package, String> {
    let (bytes, source) = read_package(path)?;
    let limits = ArchiveLimits {
        max_members: MAX_MEMBERS,
        max_member_logical_bytes: MAX_PACKAGE_BYTES,
        max_archive_logical_bytes: MAX_EXPANDED_BYTES,
        ..Default::default()
    };
    // Preflight bounds member count/central-directory bytes before the ZIP
    // crate allocates and cross-checks local/central metadata. Expanded-size
    // policy must additionally be enforced here, before constructing readers.
    let preflight = preflight_zip(
        &mut Cursor::new(&bytes),
        bytes.len() as u64,
        &limits,
        &AtomicBool::new(false),
    )
    .map_err(|e| format!("SSP ZIP preflight refused: {e:?}"))?;
    if preflight.entries.is_empty() {
        return Err("Empty ZIP is not an inspectable SSP package.".into());
    }
    let mut names = BTreeSet::new();
    let mut declared_total = 0_u64;
    for entry in &preflight.entries {
        if entry.logical_size > MAX_PACKAGE_BYTES {
            return Err("SSP declared member size exceeds 64 MiB.".into());
        }
        declared_total = declared_total
            .checked_add(entry.logical_size)
            .ok_or("SSP declared expanded size overflow")?;
        if declared_total > MAX_EXPANDED_BYTES {
            return Err("SSP declared expanded size exceeds 128 MiB.".into());
        }
        let name = safe_name(&entry.name_raw, entry.is_directory)?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err("Duplicate or case-alias SSP member names.".into());
        }
        let mode = entry.external_attributes >> 16;
        if !matches!(entry.method, 0 | 8)
            || entry.flags & ((1 << 0) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 13)) != 0
        {
            return Err("Only unencrypted stored/deflated SSP ZIP entries are inspectable.".into());
        }
        let kind = mode & 0o170000;
        if !matches!(kind, 0 | 0o100000 | 0o040000)
            || (kind == 0o040000 && !entry.is_directory)
            || (entry.is_directory && entry.logical_size != 0)
        {
            return Err("Special or contradictory SSP ZIP entry.".into());
        }
    }
    for entry in &preflight.entries {
        let name = safe_name(&entry.name_raw, entry.is_directory)?.to_ascii_lowercase();
        if !entry.is_directory
            && names
                .iter()
                .any(|other| other.starts_with(&format!("{name}/")))
        {
            return Err("SSP file/directory namespace conflict.".into());
        }
    }
    let sha256 = hash(&bytes);
    let file_bytes = bytes.len() as u64;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() != preflight.entry_count {
        return Err("ZIP indexing contradicts bounded SSP directory.".into());
    }
    let mut members = Vec::new();
    let mut version_text = None;
    let mut changes_md5_text = None;
    let mut total = 0_u64;
    for (i, expected) in preflight.entries.iter().enumerate() {
        let name = safe_name(&expected.name_raw, expected.is_directory)?;
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        if file.name_raw() != expected.name_raw || file.size() != expected.logical_size {
            return Err("ZIP entry metadata contradiction.".into());
        }
        let text = !expected.is_directory
            && (name.eq_ignore_ascii_case("version.txt")
                || name.eq_ignore_ascii_case("changes.md5"));
        if text && expected.logical_size > MAX_TEXT_BYTES {
            return Err("SSP metadata text exceeds 16 KiB.".into());
        }
        let mut digest = Sha256::new();
        let mut read = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        let mut metadata = Vec::new();
        let read_limit = expected
            .logical_size
            .checked_add(1)
            .ok_or("SSP member read bound overflow")?;
        let mut reader = (&mut file).take(read_limit);
        loop {
            let n = reader
                .read(&mut buffer)
                .map_err(|e| format!("SSP member decode/CRC failure: {e}"))?;
            if n == 0 {
                break;
            }
            read = read
                .checked_add(n as u64)
                .ok_or("SSP member size overflow")?;
            total = total
                .checked_add(n as u64)
                .ok_or("SSP expanded size overflow")?;
            if read > expected.logical_size || total > MAX_EXPANDED_BYTES {
                return Err("SSP decompression exceeded its declared bounds.".into());
            }
            digest.update(&buffer[..n]);
            if text {
                metadata.extend_from_slice(&buffer[..n]);
            }
        }
        if read != expected.logical_size {
            return Err("Truncated SSP member.".into());
        }
        if text {
            let value = String::from_utf8(metadata)
                .map_err(|_| "SSP metadata is not supported UTF-8 text.")?;
            if name.eq_ignore_ascii_case("version.txt") {
                version_text = Some(value);
            } else {
                changes_md5_text = Some(value);
            }
        }
        members.push(Member {
            name,
            directory: expected.is_directory,
            logical_bytes: read,
            sha256: if expected.is_directory {
                None
            } else {
                Some(
                    digest
                        .finalize()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                )
            },
        });
    }
    Ok(Package {
        path: path.to_path_buf(),
        sha256,
        bytes: file_bytes,
        source,
        version_text,
        changes_md5_text,
        members,
    })
}

/// CUE-only, exact-byte/native-ID context. No SSP metadata is promoted to a
/// verified game match. The complete original topology is retained verbatim.
pub fn preview(package: &Path, disc: &Path) -> Result<Preview, String> {
    if !disc
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("cue"))
    {
        return Err("SSP preview requires a complete Saturn CUE/BIN set; lone BIN, CHD and other layouts are unsupported.".into());
    }
    let package_report = inspect_package(package)?;
    let mut paths = vec![disc.to_path_buf()];
    paths.extend(resolve_cue_all_files(disc).map_err(|e| e.to_string())?);
    let sources = paths
        .iter()
        .map(|p| SourceStamp::observe(p))
        .collect::<Result<Vec<_>, _>>()?;
    let total = sources
        .iter()
        .try_fold(0_u64, |n, s| n.checked_add(s.bytes))
        .ok_or("Disc size overflow")?;
    if total > crate::saturn_patch_apply::MAX_SOURCE_BYTES || sources.iter().any(|s| s.directory) {
        return Err("Saturn disc inspection exceeds the 1 GiB regular-component bound.".into());
    }
    let manifest = inspect_saturn_disc(disc).map_err(|e| e.to_string())?;
    if manifest.status != SaturnManifestStatus::Complete
        || !manifest.system_id.as_ref().is_some_and(|id| {
            id.fact.hardware_id_recognized
                && !id.fact.product_number.is_empty()
                && !id.fact.version.is_empty()
                && !id.fact.device_info.is_empty()
        })
    {
        return Err("A complete layout and native Saturn product/version/disc identity are required; titles and filenames are not identity.".into());
    }
    if !package_report.source.still_matches() || sources.iter().any(|s| !s.still_matches()) {
        return Err("Saturn source changed during inspection.".into());
    }
    let optical = super::optical::assess(&manifest);
    if !optical.refusals.is_empty() {
        return Err(optical.refusals.join("; "));
    }
    let manifest_sha256 = super::saturn_manifest_fingerprint(&manifest);
    Ok(Preview {
        package: package_report,
        manifest,
        manifest_sha256,
        compatibility: Compatibility::UnknownBaseBinding,
        sources,
        optical_layout: optical,
    })
}

pub fn revalidate(reviewed: &Preview) -> Result<Preview, String> {
    let fresh = preview(&reviewed.package.path, &reviewed.manifest.source_descriptor)?;
    if &fresh != reviewed {
        return Err(
            "Patch or Saturn source evidence changed; inspect again and review a fresh preview."
                .into(),
        );
    }
    Ok(fresh)
}
