//! New-set-only GD-ROM patching. Exact-length file replacements and the
//! existing IP.BIN edit policy; no mastering, mode conversion or audio writes.
//! Review binds every source byte, then the shared optical tree transaction
//! owns staging, immutable receipt, no-clobber publication, recovery and undo.
//! Verification claims extent-preserving output, never canonical game identity.
use crate::chd_identity::GDROM_HIGH_DENSITY_START_FRAME;
use crate::dat::rename_apply::identity::capture_identity;
use crate::dat::rename_apply::model::ObjectIdentity;
use crate::dreamcast_boot_evidence::ip_bin::{
    self, IP_BIN_BYTES, IpBinEdit, IpBinInspection, IpBinStatus,
};
use crate::ingestion::gdi::{self, GdiDescriptor, GdiSectorSize, GdiTrack, GdiTrackType};
use crate::logical_media::{LogicalMedia, LogicalMediaError};
use crate::optical_patch_tree::{Content, digest, file_content, refuse};
use crate::patch_output_recovery::tree::{self, PreparedTreePatch, TreePatchPlan};
use crate::raw_cd_logical_media::{self, CookedCdFileLogicalMedia, RawCdFileLogicalMedia};
use crate::raw_cd_sector::mode1::{regenerate_mode1, verify_mode1};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

mod extents;
pub use extents::{ExtentCoordinates, FileExtent};
use extents::{ExtentMap, map_extents};

/// Includes all descriptor/tracks and replacement inputs; sparse lengths count.
pub const MAX_SET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Bounds review memory: only touched sector records are retained, never a disc.
pub const MAX_PATCH_BYTES: u64 = 64 * 1024 * 1024;
const RECEIPT_NAME: &str = ".emuwiz-extent-receipt.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SectorFormat {
    Cooked2048,
    Mode1Raw2352,
}
impl SectorFormat {
    fn bytes(self) -> u64 {
        match self {
            Self::Cooked2048 => 2048,
            Self::Mode1Raw2352 => 2352,
        }
    }
}
enum TrackMedia {
    Cooked(CookedCdFileLogicalMedia),
    Raw(RawCdFileLogicalMedia),
}
impl LogicalMedia for TrackMedia {
    fn len(&self) -> u64 {
        match self {
            Self::Cooked(m) => m.len(),
            Self::Raw(m) => m.len(),
        }
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8]) -> Result<(), LogicalMediaError> {
        match self {
            Self::Cooked(m) => m.read_at(offset, bytes),
            Self::Raw(m) => m.read_at(offset, bytes),
        }
    }
}
fn open_media(path: &Path, format: SectorFormat) -> io::Result<TrackMedia> {
    match format {
        SectorFormat::Cooked2048 => Ok(TrackMedia::Cooked(
            raw_cd_logical_media::open_cooked_cd_file_logical_media(path)?,
        )),
        SectorFormat::Mode1Raw2352 => Ok(TrackMedia::Raw(
            raw_cd_logical_media::open_raw_cd_file_logical_media(path).map_err(refuse)?,
        )),
    }
}
fn hash_logical(media: &TrackMedia, offset: u64, size: u64) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut buf = [0; 64 * 1024];
    let mut done = 0;
    while done < size {
        let n = (size - done).min(buf.len() as u64) as usize;
        media
            .read_at(offset + done, &mut buf[..n])
            .map_err(refuse)?;
        hash.update(&buf[..n]);
        done += n as u64;
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
#[derive(Clone, Debug)]
struct SourceFile {
    name: PathBuf,
    path: PathBuf,
    content: Content,
    identity: ObjectIdentity,
}
/// Cannot be manufactured from a filename or caller flag: obtained only by a
/// complete readable topology, byte/hash/format/extent and IP.BIN inspection.
#[derive(Clone, Debug)]
pub struct VerifiedGdiSource {
    descriptor: GdiDescriptor,
    files: Vec<SourceFile>,
    data_index: usize,
    format: SectorFormat,
    coordinates: ExtentCoordinates,
    extents: ExtentMap,
    ip: IpBinInspection,
    fingerprint: String,
}
impl VerifiedGdiSource {
    pub fn sha256(&self) -> &str {
        &self.fingerprint
    }
    pub fn descriptor(&self) -> &GdiDescriptor {
        &self.descriptor
    }
    pub fn extents(&self) -> impl Iterator<Item = &FileExtent> {
        self.extents.0.values()
    }
    pub fn ip_bin(&self) -> &IpBinInspection {
        &self.ip
    }
    fn revalidate(&self) -> io::Result<()> {
        for file in &self.files {
            if capture_identity(&file.path)? != file.identity {
                return Err(refuse("STALE GDI SOURCE: component fingerprint changed"));
            }
        }
        Ok(())
    }
    fn data(&self) -> &GdiTrack {
        &self.descriptor.tracks[self.data_index]
    }
}
fn single_name(path: &Path) -> bool {
    path.components().count() == 1
        && matches!(path.components().next(), Some(Component::Normal(_)))
        && path
            .to_str()
            .is_some_and(|s| !s.contains(['\\', ':']) && !s.chars().any(char::is_control))
}
/// All coordinates are explicitly selected and validated against the ISO
/// volume and every extent. Nonzero file offsets, repeated files, overlapping
/// tracks, supplementary filesystems and unsupported raw modes fail closed.
pub fn inspect_gdi_for_extent_patching(
    descriptor: &Path,
    coordinates: ExtentCoordinates,
) -> io::Result<VerifiedGdiSource> {
    if !descriptor
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("gdi"))
    {
        return Err(refuse("V1 writable input must be GDI"));
    }
    let parsed = gdi::parse_gdi_descriptor(descriptor).map_err(refuse)?;
    let mut names = BTreeSet::new();
    let descriptor_name = descriptor
        .file_name()
        .ok_or_else(|| refuse("descriptor filename missing"))?;
    names.insert(PathBuf::from(descriptor_name));
    let mut files = vec![source_file(PathBuf::from(descriptor_name), descriptor)?];
    let mut total = files[0].content.size;
    let mut source_paths = BTreeSet::new();
    let mut selected = None;
    let mut previous_end = 0;
    for (index, track) in parsed.tracks.iter().enumerate() {
        let frames = track.source_length_bytes / u64::from(track.sector_size.bytes());
        if track.file_offset != 0
            || frames == 0
            || track.source_length_bytes % u64::from(track.sector_size.bytes()) != 0
            || !single_name(&track.source_filename)
            || !source_paths.insert(track.source_path.clone())
            || fs::symlink_metadata(descriptor.parent().unwrap().join(&track.source_filename))?
                .file_type()
                .is_symlink()
            || !names.insert(track.source_filename.clone())
            || track.source_filename == Path::new(RECEIPT_NAME)
            || u64::from(track.start_lba) < previous_end
        {
            return Err(refuse(
                "unsupported GDI offset/length/path/duplicate/overlap",
            ));
        }
        previous_end = u64::from(track.start_lba) + frames;
        if previous_end > 549_150 {
            return Err(refuse("track exceeds GD-ROM addressable range"));
        }
        files.push(source_file(
            track.source_filename.clone(),
            &track.source_path,
        )?);
        total = total
            .checked_add(track.source_length_bytes)
            .ok_or_else(|| refuse("set size overflow"))?;
        if total > MAX_SET_BYTES {
            return Err(refuse("GDI set exceeds bounded size policy"));
        }
        if track.track_type == GdiTrackType::Data {
            // Validate the complete data track, not just its first sector.
            if track.sector_size == GdiSectorSize::Bytes2352 {
                let mut file = File::open(&track.source_path)?;
                let mut sector = [0; 2352];
                for _ in 0..frames {
                    file.read_exact(&mut sector)?;
                    verify_mode1(&sector).map_err(refuse)?;
                }
            }
            if track.start_lba >= GDROM_HIGH_DENSITY_START_FRAME
                && selected.replace(index).is_some()
            {
                return Err(refuse("ambiguous high-density data tracks"));
            }
        }
    }
    if Path::new(descriptor_name) == Path::new(RECEIPT_NAME) {
        return Err(refuse("reserved receipt filename"));
    }
    let data_index = selected.ok_or_else(|| refuse("high-density data track absent"))?;
    let track = &parsed.tracks[data_index];
    let format = match track.sector_size {
        GdiSectorSize::Bytes2048 => SectorFormat::Cooked2048,
        GdiSectorSize::Bytes2352 => SectorFormat::Mode1Raw2352,
    };
    let media = open_media(&track.source_path, format)?;
    let extents = map_extents(&media, track, coordinates, format)?;
    let mut ip_bytes = [0; IP_BIN_BYTES];
    media.read_at(0, &mut ip_bytes).map_err(refuse)?;
    let ip = ip_bin::inspect_ip_bin(&ip_bytes);
    if !matches!(
        ip.status,
        IpBinStatus::Valid | IpBinStatus::SuspiciousButParseable
    ) {
        return Err(refuse("supported complete Dreamcast IP.BIN required"));
    }
    let boot = &ip.ip_bin.as_ref().unwrap().metadata.boot_filename.value;
    if !extents.0.contains_key(boot) {
        return Err(refuse("IP.BIN boot target missing or ambiguous"));
    }
    let fingerprint = digest(
        &serde_json::to_vec(
            &files
                .iter()
                .map(|f| (&f.name, &f.content))
                .collect::<Vec<_>>(),
        )
        .map_err(refuse)?,
    );
    let verified = VerifiedGdiSource {
        descriptor: parsed,
        files,
        data_index,
        format,
        coordinates,
        extents,
        ip,
        fingerprint,
    };
    verified.revalidate()?;
    Ok(verified)
}
fn source_file(name: PathBuf, path: &Path) -> io::Result<SourceFile> {
    let metadata = fs::symlink_metadata(path)?;
    if !path.is_absolute()
        || !metadata.is_file()
        || metadata.len() > MAX_SET_BYTES
        || fs::canonicalize(path)? != path
    {
        return Err(refuse(
            "absolute regular non-aliased bounded source required",
        ));
    }
    let identity = capture_identity(path)?;
    let content = file_content(path, MAX_SET_BYTES)?;
    if capture_identity(path)? != identity {
        return Err(refuse("source changed during inspection"));
    }
    Ok(SourceFile {
        name,
        path: path.to_owned(),
        content,
        identity,
    })
}
/// Explicit source-byte identity is required, independent of the lookup path.
#[derive(Clone, Debug)]
pub struct FileReplacement {
    pub filesystem_path: String,
    pub expected_source_sha256: String,
    pub replacement: PathBuf,
}
#[derive(Clone, Debug)]
struct PlannedSector {
    before: Vec<u8>,
    after: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PatchedFileEvidence {
    pub extent: FileExtent,
    pub output_sha256: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct SectorEvidence {
    pub track_relative_sector: u64,
    pub source_sha256: String,
    pub output_sha256: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct GdiExtentReceipt {
    pub version: u32,
    /// This is a byte/layout verification claim, not retail/DAT identity.
    pub claim: String,
    pub source_set_sha256: String,
    pub extent_coordinates: ExtentCoordinates,
    pub data_track: u32,
    pub sector_format: SectorFormat,
    pub patched_files: Vec<PatchedFileEvidence>,
    pub changed_sectors: Vec<SectorEvidence>,
    pub ip_bin_output_sha256: Option<String>,
    pub output_tracks: Vec<TrackHashEvidence>,
}
#[derive(Clone, Debug, Serialize)]
pub struct TrackHashEvidence {
    pub track: u32,
    pub audio: bool,
    pub source_sha256: String,
    pub output_sha256: String,
}
#[derive(Clone, Debug)]
pub struct GdiExtentPatchPlan {
    tree: TreePatchPlan,
    source: VerifiedGdiSource,
    sectors: BTreeMap<u64, PlannedSector>,
    patched_files: Vec<PatchedFileEvidence>,
    ip: Option<IpBinInspection>,
}
impl GdiExtentPatchPlan {
    pub fn changed_sector_count(&self) -> usize {
        self.sectors.len()
    }
    pub fn patched_files(&self) -> &[PatchedFileEvidence] {
        &self.patched_files
    }
    pub fn prepare(&self) -> io::Result<PreparedTreePatch> {
        self.prepare_with_fault(None)
    }
    fn prepare_with_fault(&self, fault: Option<Fault>) -> io::Result<PreparedTreePatch> {
        self.source.revalidate()?;
        tree::prepare(
            &self.tree,
            |staging| self.produce(staging, fault),
            |staging| {
                checkpoint(fault, Fault::Verification)?;
                self.verify(staging)
            },
        )
    }
    fn produce(&self, staging: &Path, fault: Option<Fault>) -> io::Result<()> {
        checkpoint(fault, Fault::Staging)?;
        self.source.revalidate()?;
        for (index, file) in self.source.files.iter().enumerate() {
            if index == 2 {
                checkpoint(fault, Fault::SecondTrackCopy)?;
            }
            let mut input = File::open(&file.path)?.take(file.content.size + 1);
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staging.join(&file.name))?;
            if io::copy(&mut input, &mut output)? != file.content.size {
                return Err(refuse("source length changed during copy"));
            }
            output.sync_all()?;
            if file_content(&staging.join(&file.name), MAX_SET_BYTES)? != file.content {
                return Err(refuse("source bytes changed during copy"));
            }
        }
        let track = self.source.data();
        let mut output = OpenOptions::new()
            .read(true)
            .write(true)
            .open(staging.join(&track.source_filename))?;
        for (written, (index, sector)) in self.sectors.iter().enumerate() {
            let mut bytes = sector.after.clone();
            if self.source.format == SectorFormat::Mode1Raw2352 {
                checkpoint(fault, Fault::Regeneration)?;
                regenerate_mode1(bytes.as_mut_slice().try_into().unwrap()).map_err(refuse)?;
                if bytes != sector.after {
                    return Err(refuse("planned regenerated sector mismatch"));
                }
            }
            if written == 1 {
                checkpoint(fault, Fault::SectorWrite)?;
            }
            output.seek(SeekFrom::Start(index * self.source.format.bytes()))?;
            output.write_all(&bytes)?;
        }
        output.sync_all()?;
        // No receipt exists until every sector, file, track and IP.BIN passes.
        self.verify_payload(staging)?;
        let receipt = self.receipt(staging)?;
        checkpoint(fault, Fault::Receipt)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staging.join(RECEIPT_NAME))?;
        file.write_all(&serde_json::to_vec_pretty(&receipt).map_err(refuse)?)?;
        file.sync_all()?;
        Ok(())
    }
    fn verify_payload(&self, root: &Path) -> io::Result<()> {
        self.source.revalidate()?;
        let descriptor = root.join(self.source.descriptor.descriptor_path.file_name().unwrap());
        let parsed = gdi::parse_gdi_descriptor(&descriptor).map_err(refuse)?;
        if parsed.logical_topology() != self.source.descriptor.logical_topology() {
            return Err(refuse("output topology changed"));
        }
        // The descriptor itself is copied byte-for-byte too.
        if file_content(&descriptor, MAX_SET_BYTES)? != self.source.files[0].content {
            return Err(refuse("descriptor bytes changed"));
        }
        for (position, track) in self.source.descriptor.tracks.iter().enumerate() {
            let expected = &self.source.files[position + 1];
            if position != self.source.data_index {
                if file_content(&root.join(&track.source_filename), MAX_SET_BYTES)?
                    != expected.content
                {
                    return Err(refuse("untouched/audio track changed"));
                }
                continue;
            }
            let mut source = File::open(&track.source_path)?;
            let mut output = File::open(root.join(&track.source_filename))?;
            let size = self.source.format.bytes() as usize;
            let mut before = vec![0; size];
            let mut after = vec![0; size];
            for index in 0..track.source_length_bytes / size as u64 {
                source.read_exact(&mut before)?;
                output.read_exact(&mut after)?;
                let required = if let Some(patch) = self.sectors.get(&index) {
                    if before != patch.before {
                        return Err(refuse("stale planned sector"));
                    }
                    if self.source.format == SectorFormat::Mode1Raw2352 {
                        verify_mode1(after.as_slice().try_into().unwrap()).map_err(refuse)?;
                    }
                    &patch.after
                } else {
                    &before
                };
                if &after != required {
                    return Err(refuse(
                        "changed or untouched sector failed byte verification",
                    ));
                }
            }
        }
        let media = open_media(
            &root.join(&self.source.data().source_filename),
            self.source.format,
        )?;
        let output_map = map_extents(
            &media,
            self.source.data(),
            self.source.coordinates,
            self.source.format,
        )?;
        let mut expected_map = self.source.extents.0.clone();
        for patch in &self.patched_files {
            expected_map
                .get_mut(&patch.extent.path)
                .unwrap()
                .source_sha256 = patch.output_sha256.clone();
        }
        if output_map.0 != expected_map {
            return Err(refuse("filesystem extent/content verification failed"));
        }
        let mut ip = [0; IP_BIN_BYTES];
        media.read_at(0, &mut ip).map_err(refuse)?;
        if ip_bin::inspect_ip_bin(&ip) != *self.ip.as_ref().unwrap_or(&self.source.ip) {
            return Err(refuse("IP.BIN reinspection differs from plan"));
        }
        self.source.revalidate()
    }
    fn receipt(&self, root: &Path) -> io::Result<GdiExtentReceipt> {
        let mut tracks = Vec::new();
        for (index, track) in self.source.descriptor.tracks.iter().enumerate() {
            tracks.push(TrackHashEvidence {
                track: track.number,
                audio: track.track_type == GdiTrackType::Audio,
                source_sha256: self.source.files[index + 1].content.sha256.clone(),
                output_sha256: file_content(&root.join(&track.source_filename), MAX_SET_BYTES)?
                    .sha256,
            });
        }
        Ok(GdiExtentReceipt {
            version: 1,
            claim: "EXTENT-PRESERVING VERIFIED OUTPUT".into(),
            source_set_sha256: self.source.fingerprint.clone(),
            extent_coordinates: self.source.coordinates,
            data_track: self.source.data().number,
            sector_format: self.source.format,
            patched_files: self.patched_files.clone(),
            changed_sectors: self
                .sectors
                .iter()
                .map(|(index, p)| SectorEvidence {
                    track_relative_sector: *index,
                    source_sha256: digest(&p.before),
                    output_sha256: digest(&p.after),
                })
                .collect(),
            ip_bin_output_sha256: self
                .ip
                .as_ref()
                .map(|ip| digest(ip.ip_bin.as_ref().unwrap().raw_bytes())),
            output_tracks: tracks,
        })
    }
    fn verify(&self, root: &Path) -> io::Result<()> {
        self.verify_payload(root)?;
        let expected = serde_json::to_vec_pretty(&self.receipt(root)?).map_err(refuse)?;
        if fs::read(root.join(RECEIPT_NAME))? != expected {
            return Err(refuse("extent receipt mismatch"));
        }
        let mut names: BTreeSet<PathBuf> =
            self.source.files.iter().map(|f| f.name.clone()).collect();
        names.insert(RECEIPT_NAME.into());
        let actual = fs::read_dir(root)?
            .map(|e| e.map(|e| PathBuf::from(e.file_name())))
            .collect::<io::Result<BTreeSet<_>>>()?;
        if actual != names {
            return Err(refuse("unexpected output set membership"));
        }
        Ok(())
    }
}
/// Preview is read-only. Caller binds the reviewed complete set hash and each
/// replaced file hash; exact path alone never establishes patch identity.
pub fn review_gdi_extent_patch(
    source: &VerifiedGdiSource,
    expected_source_sha256: &str,
    replacements: &[FileReplacement],
    ip_edits: &[IpBinEdit],
    destination: &Path,
) -> io::Result<GdiExtentPatchPlan> {
    source.revalidate()?;
    if source.sha256() != expected_source_sha256 {
        return Err(refuse("wrong GDI source fingerprint"));
    }
    if replacements.is_empty() && ip_edits.is_empty() {
        return Err(refuse("no modifications requested"));
    }
    if replacements.len() > 16384 {
        return Err(refuse("replacement count bound exceeded"));
    }
    let mut inputs: Vec<_> = source.files.iter().map(|f| f.path.clone()).collect();
    let mut requests = Vec::new();
    let mut patched_files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut patch_bytes = 0u64;
    for replacement in replacements {
        if !seen.insert(&replacement.filesystem_path) {
            return Err(refuse("duplicate replacement"));
        }
        let extent = source
            .extents
            .0
            .get(&replacement.filesystem_path)
            .ok_or_else(|| refuse("extent lookup failed"))?;
        if extent.source_sha256 != replacement.expected_source_sha256 {
            return Err(refuse("wrong filesystem file source hash"));
        }
        let content = file_content(&replacement.replacement, MAX_PATCH_BYTES)?;
        if content.size != extent.byte_length {
            return Err(refuse("V1 requires exact same byte length"));
        }
        patch_bytes += content.size;
        if patch_bytes > MAX_PATCH_BYTES {
            return Err(refuse("patch memory policy exceeded"));
        }
        inputs.push(replacement.replacement.clone());
        requests.push((
            extent.track_relative_sector,
            extent.byte_length,
            replacement.replacement.clone(),
            content.clone(),
        ));
        patched_files.push(PatchedFileEvidence {
            extent: extent.clone(),
            output_sha256: content.sha256,
        });
    }
    let ip = if ip_edits.is_empty() {
        None
    } else {
        let (ip, _) = ip_bin::edit_ip_bin_inspection(&source.ip, ip_edits)?;
        // A changed boot target must be an exact existing ISO file. Region and
        // VGA policy are wholly owned by the existing IP.BIN tooling.
        if !source
            .extents
            .0
            .contains_key(&ip.ip_bin.as_ref().unwrap().metadata.boot_filename.value)
        {
            return Err(refuse("edited boot target absent"));
        }
        Some(ip)
    };
    let tree = TreePatchPlan::review_with_max_total_bytes(&inputs, destination, MAX_SET_BYTES)?;
    let mut sectors = BTreeMap::new();
    for (start, length, path, expected) in requests {
        let mut file = File::open(&path)?;
        build_sectors(source, &mut sectors, start, length, &mut file)?;
        if file_content(&path, MAX_PATCH_BYTES)? != expected {
            return Err(refuse("replacement changed during review"));
        }
    }
    if let Some(ip) = &ip {
        build_sectors(
            source,
            &mut sectors,
            0,
            IP_BIN_BYTES as u64,
            &mut io::Cursor::new(ip.ip_bin.as_ref().unwrap().raw_bytes()),
        )?;
    }
    if sectors.is_empty() {
        return Err(refuse("no byte changes requested"));
    }
    source.revalidate()?;
    Ok(GdiExtentPatchPlan {
        tree,
        source: source.clone(),
        sectors,
        patched_files,
        ip,
    })
}
fn build_sectors(
    source: &VerifiedGdiSource,
    sectors: &mut BTreeMap<u64, PlannedSector>,
    start: u64,
    length: u64,
    replacement: &mut impl Read,
) -> io::Result<()> {
    let mut input = File::open(&source.data().source_path)?;
    let size = source.format.bytes() as usize;
    let user_offset = if size == 2352 { 16 } else { 0 };
    for n in 0..length.div_ceil(2048) {
        let index = start + n;
        if sectors.contains_key(&index) {
            return Err(refuse("overlapping patch sectors"));
        }
        let mut before = vec![0; size];
        input.seek(SeekFrom::Start(index * size as u64))?;
        input.read_exact(&mut before)?;
        let mut after = before.clone();
        let take = (length - n * 2048).min(2048) as usize;
        replacement.read_exact(&mut after[user_offset..user_offset + take])?;
        if size == 2352 {
            regenerate_mode1(after.as_mut_slice().try_into().unwrap()).map_err(refuse)?;
        }
        if after != before {
            sectors.insert(index, PlannedSector { before, after });
        }
        // Two buffers per sector, plus request/map/receipt bookkeeping.
        if sectors.len() as u64 * size as u64 > MAX_PATCH_BYTES + IP_BIN_BYTES as u64 + 16384 * 2352
        {
            return Err(refuse("changed-sector bound exceeded"));
        }
    }
    Ok(())
}

// Private deterministic seam: no production caller can inject a fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Staging,
    SecondTrackCopy,
    Regeneration,
    SectorWrite,
    Verification,
    Receipt,
}
fn checkpoint(fault: Option<Fault>, point: Fault) -> io::Result<()> {
    if fault == Some(point) {
        return Err(refuse(format!("injected extent patch failure: {point:?}")));
    }
    Ok(())
}
#[cfg(test)]
mod tests;
