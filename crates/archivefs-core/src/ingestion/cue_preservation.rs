//! Verified, read-only physical CUE/BIN snapshots. Parsing and all sector/time
//! arithmetic remain in cue_bin/cue_timeline; opening remains in safe_read.
//! Whole FILE bytes and individual stored ranges are hashed in the same pass.
//! Synthetic declarations have no byte digest and are never materialized.
//! Metadata equality is not content proof: inspection establishes digests with
//! independent reads, and reading/comparison revalidate those earlier bytes.
//! These are bounded read observations, not locked filesystem snapshots.
//!
//! Comparisons concern stored source layout/content, not cooked-sector identity,
//! playability, Redump equivalence, CD-text, or CUE/CHD conversion. They do not
//! authorize any operation, or replace the existing exact-equivalence gate.
use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::cue_bin::{CueError, CueLayout, MAX_CUE_BYTES, resolve_cue_layout_text};
use super::cue_timeline::{
    DiscTimeline, PregapEvidence, SourceRange, TimelineRefusal, build_timeline_with,
};
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::safe_read::{SafeReadRefusal, TrustedRoots, open_bounded_read};

pub const CUE_PRESERVATION_SCHEMA: &str = "emuwiz.cue-preservation.v1";
pub const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuePreservationError {
    Cue(CueError),
    Timeline(TimelineRefusal),
    SafeRead {
        path: PathBuf,
        refusal: SafeReadRefusal,
    },
    ChangedSource {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        detail: String,
    },
    Unsupported {
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueSourceIdentity {
    pub file: CapturedFileIdentity,
    pub change_time: Option<(i64, i64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeDigest {
    pub range: SourceRange,
    /// SHA-256 of stored physical bytes, never cooked program identity.
    pub physical_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEvidence {
    pub ordinal: u32,
    pub file_type: String,
    pub path: PathBuf,
    pub identity: CueSourceIdentity,
    pub physical_sha256: String,
    /// Identity alias of an earlier FILE, including hard links.
    pub shares_source_with: Option<u32>,
    /// Stored bytes before the first declared range; never called a pregap.
    pub unassigned_prefix: Option<RangeDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackContentEvidence {
    pub number: u32,
    pub source_backed_pregap: Option<RangeDigest>,
    pub program: RangeDigest,
}

/// Constructed only by verified inspection/reading. No deserialization or
/// public mutable fields can manufacture trusted comparison evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscLayoutEvidence {
    cue_text: String,
    cue_identity: CueSourceIdentity,
    cue_sha256: String,
    layout: CueLayout,
    timeline: DiscTimeline,
    sources: Vec<SourceEvidence>,
    tracks: Vec<TrackContentEvidence>,
}

impl DiscLayoutEvidence {
    pub fn schema(&self) -> &'static str {
        CUE_PRESERVATION_SCHEMA
    }
    /// Original verified text, including literal FILE references. Resolved
    /// paths and ordinal relationships are in layout/sources; text is provenance.
    pub fn cue_text(&self) -> &str {
        &self.cue_text
    }
    pub fn cue_identity(&self) -> &CueSourceIdentity {
        &self.cue_identity
    }
    pub fn cue_sha256(&self) -> &str {
        &self.cue_sha256
    }
    pub fn layout(&self) -> &CueLayout {
        &self.layout
    }
    pub fn timeline(&self) -> &DiscTimeline {
        &self.timeline
    }
    pub fn sources(&self) -> &[SourceEvidence] {
        &self.sources
    }
    pub fn tracks(&self) -> &[TrackContentEvidence] {
        &self.tracks
    }
}

#[derive(Debug, Clone)]
pub struct CuePreservationInspection {
    cue_text: String,
    cue_identity: CueSourceIdentity,
    cue_sha256: String,
    layout: CueLayout,
    timeline: DiscTimeline,
    sources: Vec<(PathBuf, CueSourceIdentity)>,
    source_sha256: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueComparisonOutcome {
    Equivalent,
    Different,
    Unsupported,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CueLayoutDifference {
    SourceRelationships,
    TrackStructure,
    IndexBoundaries { track: u32 },
    PregapStorage { track: u32 },
    SyntheticGaps { track: u32 },
    StoredPregapContent { track: u32 },
    ProgramContent { track: u32 },
    SourceContent { ordinal: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuePreservationComparison {
    pub outcome: CueComparisonOutcome,
    pub differences: Vec<CueLayoutDifference>,
    pub refusals: Vec<CuePreservationError>,
    /// Corresponding tracks' physical program bytes/modes match. This is never
    /// sufficient for layout equivalence and says nothing about cooked data.
    pub programs_match: Option<bool>,
}

impl std::fmt::Display for CuePreservationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cue(CueError::MissingDataFile(path)) => write!(
                f,
                "Backing FILE is missing or unreadable: {}",
                path.display()
            ),
            Self::Cue(error) => write!(f, "{error}"),
            Self::Timeline(error) => write!(f, "{error}"),
            Self::SafeRead { path, refusal } => {
                write!(f, "{}: {}", path.display(), refusal.detail())
            }
            Self::ChangedSource { path } => write!(
                f,
                "Source identity, content or FILE resolution changed: {}",
                path.display()
            ),
            Self::Io { path, detail } => write!(f, "{}: {detail}", path.display()),
            Self::Unsupported { detail } => {
                write!(f, "Unsupported preservation evidence: {detail}")
            }
        }
    }
}
impl std::error::Error for CuePreservationError {}

fn io(path: &Path, error: std::io::Error) -> CuePreservationError {
    CuePreservationError::Io {
        path: path.into(),
        detail: error.to_string(),
    }
}
fn changed(path: &Path) -> CuePreservationError {
    CuePreservationError::ChangedSource { path: path.into() }
}
fn unsupported(detail: impl Into<String>) -> CuePreservationError {
    CuePreservationError::Unsupported {
        detail: detail.into(),
    }
}
fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn identity(metadata: &Metadata) -> Result<CueSourceIdentity, CuePreservationError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(CueSourceIdentity {
            file: CapturedFileIdentity::capture(metadata),
            change_time: Some((metadata.ctime(), metadata.ctime_nsec())),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Err(unsupported(
            "stable device/inode/change-time evidence is unavailable on this platform",
        ))
    }
}

fn open_source(path: &Path) -> Result<(File, CueSourceIdentity), CuePreservationError> {
    let file = open_bounded_read(path, &TrustedRoots::none())
        .map_err(|refusal| CuePreservationError::SafeRead {
            path: path.into(),
            refusal,
        })?
        .into_file();
    let stamp = identity(&file.metadata().map_err(|e| io(path, e))?)?;
    Ok((file, stamp))
}

fn cue_snapshot(path: &Path) -> Result<(String, CueSourceIdentity, String), CuePreservationError> {
    let (mut file, stamp) = open_source(path)?;
    if stamp.file.size > MAX_CUE_BYTES {
        return Err(CuePreservationError::Cue(CueError::TooLarge));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_CUE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io(path, e))?;
    if identity(&file.metadata().map_err(|e| io(path, e))?)? != stamp {
        return Err(changed(path));
    }
    if bytes.len() as u64 != stamp.file.size {
        return Err(changed(path));
    }
    if bytes.len() as u64 > MAX_CUE_BYTES {
        return Err(CuePreservationError::Cue(CueError::TooLarge));
    }
    let digest = hex(Sha256::digest(&bytes));
    let text = String::from_utf8(bytes).map_err(|_| {
        CuePreservationError::Cue(CueError::Malformed("CUE is not UTF-8 text".into()))
    })?;
    Ok((text, stamp, digest))
}

/// Coverage guard only: does not parse positions, modes or paths. The canonical
/// parser deliberately ignores other directives; evidence must not silently
/// claim to represent them. Only non-layout REM COMMENT/GENRE is skipped.
fn declaration_coverage(text: &str) -> Result<usize, CuePreservationError> {
    let mut files = 0;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return Err(unsupported("control character in CUE directive"));
        }
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        let directive = fields[0].to_ascii_uppercase();
        match directive.as_str() {
            "REM"
                if fields.get(1).is_some_and(|kind| {
                    kind.eq_ignore_ascii_case("COMMENT") || kind.eq_ignore_ascii_case("GENRE")
                }) => {}
            "FILE" => files += 1,
            "TRACK" | "INDEX" if fields.len() == 3 => {}
            "PREGAP" | "POSTGAP" if fields.len() == 2 => {}
            _ => {
                return Err(unsupported(format!(
                    "unrepresented directive or trailing fields: {directive}"
                )));
            }
        }
        if files > 99 {
            return Err(unsupported("more than 99 FILE declarations"));
        }
    }
    Ok(files)
}

/// Capture metadata and every stored byte, then independently reread the
/// content before accepting inspection. Timestamp equality alone cannot prove
/// the earlier bytes, even when change-time exposes nanosecond fields. The CUE
/// path must satisfy the shared safe-read policy (absolute, normal, no links).
pub fn inspect_cue_preservation(
    path: &Path,
) -> Result<CuePreservationInspection, CuePreservationError> {
    inspect_with(path, |_| {})
}

fn inspect_with(
    path: &Path,
    before_final_verification: impl FnOnce(&mut CuePreservationInspection),
) -> Result<CuePreservationInspection, CuePreservationError> {
    let (cue_text, cue_identity, cue_sha256) = cue_snapshot(path)?;
    let file_count = declaration_coverage(&cue_text)?;
    let layout = resolve_cue_layout_text(path, &cue_text).map_err(CuePreservationError::Cue)?;
    if layout.tracks.len() > 99 {
        return Err(unsupported("more than 99 TRACK declarations"));
    }
    for track in &layout.tracks {
        if track
            .extra_indexes
            .windows(2)
            .any(|pair| pair[0].0 >= pair[1].0)
        {
            return Err(unsupported(format!(
                "TRACK {:02} additional INDEX numbers do not increase",
                track.number
            )));
        }
    }
    let declarations: BTreeMap<_, _> = layout
        .tracks
        .iter()
        .map(|t| (t.file_ordinal, &t.path))
        .collect();
    if declarations.len() != file_count
        || declarations
            .keys()
            .enumerate()
            .any(|(i, n)| *n as usize != i)
    {
        return Err(unsupported(
            "a FILE declaration has no represented TRACK; no source was discarded",
        ));
    }
    let mut sources = Vec::new();
    let mut total = 0u64;
    for path in declarations.values() {
        let (_, stamp) = open_source(path)?;
        total = total
            .checked_add(stamp.file.size)
            .ok_or_else(|| unsupported("total source length overflows"))?;
        if total > MAX_SOURCE_BYTES {
            return Err(unsupported("source bytes exceed the 8 GiB evidence limit"));
        }
        sources.push(((*path).clone(), stamp));
    }
    let timeline = build_timeline_with(&layout, |path| {
        sources
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, s)| s.file.size)
            .ok_or_else(|| "uninspected source".into())
    })
    .map_err(CuePreservationError::Timeline)?;
    let mut inspection = CuePreservationInspection {
        cue_text,
        cue_identity,
        cue_sha256,
        layout,
        timeline,
        sources,
        source_sha256: Vec::new(),
    };
    // Enforce the total byte limit and validate the timeline before reading
    // any BIN. Hash prefixes/pregaps as well as programs; no stored byte may
    // become an unverified inspection-time expectation.
    for file in &inspection.timeline.files {
        let (_, stamp) = &inspection.sources[file.ordinal as usize];
        let digest = hash_source(
            &file.path,
            stamp,
            None,
            &source_ranges(&inspection, file.ordinal)?,
        )?;
        inspection.source_sha256.push(digest);
    }
    before_final_verification(&mut inspection);
    validate_inspection(&inspection)?;
    Ok(inspection)
}

fn validate_relationships(
    inspection: &CuePreservationInspection,
) -> Result<(), CuePreservationError> {
    let path = &inspection.layout.cue_path;
    let (_, stamp) = open_source(path)?;
    if stamp != inspection.cue_identity {
        return Err(changed(path));
    }
    let (text, stamp, digest) = cue_snapshot(path)?;
    if stamp != inspection.cue_identity
        || digest != inspection.cue_sha256
        || text != inspection.cue_text
    {
        return Err(changed(path));
    }
    // Re-resolve the original FILE relationships: the CUE text can stay the
    // same while a leaf symlink or an ancestor directory changes its target.
    let current = resolve_cue_layout_text(path, &text).map_err(|error| match error {
        CueError::MissingDataFile(p) => changed(&p),
        _ => changed(path),
    })?;
    if current != inspection.layout {
        return Err(changed(path));
    }
    for (path, expected) in &inspection.sources {
        let (_, stamp) = open_source(path)?;
        if &stamp != expected {
            return Err(changed(path));
        }
    }
    Ok(())
}

fn validate_inspection(inspection: &CuePreservationInspection) -> Result<(), CuePreservationError> {
    validate_relationships(inspection)?;
    for file in &inspection.timeline.files {
        hash_source(
            &file.path,
            &inspection.sources[file.ordinal as usize].1,
            Some(&inspection.source_sha256[file.ordinal as usize]),
            &source_ranges(inspection, file.ordinal)?,
        )?;
    }
    validate_relationships(inspection)
}

fn validate_evidence(evidence: &DiscLayoutEvidence) -> Result<(), CuePreservationError> {
    validate_inspection(&evidence_inspection(evidence))
}

fn evidence_inspection(evidence: &DiscLayoutEvidence) -> CuePreservationInspection {
    CuePreservationInspection {
        cue_text: evidence.cue_text.clone(),
        cue_identity: evidence.cue_identity.clone(),
        cue_sha256: evidence.cue_sha256.clone(),
        layout: evidence.layout.clone(),
        timeline: evidence.timeline.clone(),
        sources: evidence
            .sources
            .iter()
            .map(|s| (s.path.clone(), s.identity.clone()))
            .collect(),
        source_sha256: evidence
            .sources
            .iter()
            .map(|s| s.physical_sha256.clone())
            .collect(),
    }
}

#[derive(Clone, Copy)]
enum RangeRole {
    Prefix,
    Pregap(usize),
    Program(usize),
}

fn source_ranges(
    inspection: &CuePreservationInspection,
    ordinal: u32,
) -> Result<Vec<(RangeRole, SourceRange)>, CuePreservationError> {
    let mut ranges = Vec::new();
    for (i, t) in inspection
        .timeline
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.file_ordinal == ordinal)
    {
        if let PregapEvidence::SourceBacked(range) = t.pregap {
            ranges.push((RangeRole::Pregap(i), range));
        }
        ranges.push((RangeRole::Program(i), t.program));
    }
    if let Some((_, first)) = ranges.first().copied() {
        if first.byte_offset > 0 {
            ranges.insert(
                0,
                (
                    RangeRole::Prefix,
                    SourceRange {
                        start_frame: 0,
                        frames: first.start_frame,
                        byte_offset: 0,
                        byte_len: first.byte_offset,
                    },
                ),
            );
        }
    }
    let mut cursor = 0u64;
    for (_, range) in &ranges {
        if range.byte_offset != cursor {
            return Err(unsupported("source ranges are not contiguous"));
        }
        cursor = cursor
            .checked_add(range.byte_len)
            .ok_or_else(|| unsupported("source range length overflows"))?;
    }
    if cursor != inspection.timeline.files[ordinal as usize].bytes {
        return Err(unsupported("source ranges do not cover every stored byte"));
    }
    Ok(ranges)
}

/// Hash the whole file and its partitioned ranges together. Constant memory,
/// checked lengths, exact reads, no logical-sector decoding or synthetic bytes.
fn read_source(
    path: &Path,
    expected: &CueSourceIdentity,
    expected_sha256: Option<&str>,
    ranges: &[(RangeRole, SourceRange)],
) -> Result<(String, Vec<(RangeRole, RangeDigest)>), CuePreservationError> {
    read_source_with(path, expected, expected_sha256, ranges, true, |_| {})
}

// Verification needs the complete-file proof, but does not publish range
// digests. Keep exactly the same reads and identity/content checks without
// hashing every byte twice on these passes.
fn hash_source(
    path: &Path,
    expected: &CueSourceIdentity,
    expected_sha256: Option<&str>,
    ranges: &[(RangeRole, SourceRange)],
) -> Result<String, CuePreservationError> {
    read_source_with(path, expected, expected_sha256, ranges, false, |_| {})
        .map(|(digest, _)| digest)
}

fn read_source_with(
    path: &Path,
    expected: &CueSourceIdentity,
    expected_sha256: Option<&str>,
    ranges: &[(RangeRole, SourceRange)],
    capture_ranges: bool,
    mut after_chunk: impl FnMut(u64),
) -> Result<(String, Vec<(RangeRole, RangeDigest)>), CuePreservationError> {
    let (mut file, stamp) = open_source(path)?;
    if &stamp != expected {
        return Err(changed(path));
    }
    let mut whole = Sha256::new();
    let mut digests = Vec::new();
    let mut buffer = vec![0u8; 128 * 1024];
    let mut cursor = 0u64;
    for (role, range) in ranges {
        let end = range
            .byte_offset
            .checked_add(range.byte_len)
            .ok_or_else(|| unsupported("source range overflows"))?;
        if range.byte_offset != cursor || end > expected.file.size {
            return Err(unsupported("source range is outside the verified file"));
        }
        let mut digest = capture_ranges.then(Sha256::new);
        let mut remaining = range.byte_len;
        while remaining > 0 {
            let n = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..n]).map_err(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    changed(path)
                } else {
                    io(path, e)
                }
            })?;
            whole.update(&buffer[..n]);
            if let Some(digest) = &mut digest {
                digest.update(&buffer[..n]);
            }
            remaining -= n as u64;
            after_chunk(end - remaining);
            #[cfg(all(test, unix))]
            comparison_tests::after_source_read(path, end - remaining);
        }
        cursor = end;
        if let Some(digest) = digest {
            digests.push((
                *role,
                RangeDigest {
                    range: *range,
                    physical_sha256: hex(digest.finalize()),
                },
            ));
        }
    }
    if cursor != expected.file.size {
        return Err(unsupported("source bytes were not completely read"));
    }
    if identity(&file.metadata().map_err(|e| io(path, e))?)? != *expected {
        return Err(changed(path));
    }
    // A replaced path may still leave this open handle reading the old inode.
    let (_, current) = open_source(path).map_err(|_| changed(path))?;
    let digest = hex(whole.finalize());
    if current != *expected || expected_sha256.is_some_and(|expected| expected != digest) {
        return Err(changed(path));
    }
    Ok((digest, digests))
}

/// Read exactly the inspected bytes. Each FILE digest must match the content
/// independently established at inspection, followed by another complete
/// content/relationship verification before publishing evidence.
pub fn read_cue_preservation(
    inspection: &CuePreservationInspection,
) -> Result<DiscLayoutEvidence, CuePreservationError> {
    read_with(inspection, || {})
}

fn read_with(
    inspection: &CuePreservationInspection,
    before_final_verification: impl FnOnce(),
) -> Result<DiscLayoutEvidence, CuePreservationError> {
    validate_relationships(inspection)?;
    let mut sources: Vec<SourceEvidence> = Vec::new();
    let mut content: BTreeMap<usize, (Option<RangeDigest>, Option<RangeDigest>)> = BTreeMap::new();
    for file in &inspection.timeline.files {
        let stamp = &inspection.sources[file.ordinal as usize].1;
        let (physical_sha256, digests) = read_source(
            &file.path,
            stamp,
            Some(&inspection.source_sha256[file.ordinal as usize]),
            &source_ranges(inspection, file.ordinal)?,
        )?;
        let mut unassigned_prefix = None;
        for (role, digest) in digests {
            match role {
                RangeRole::Prefix => unassigned_prefix = Some(digest),
                RangeRole::Pregap(i) => content.entry(i).or_default().0 = Some(digest),
                RangeRole::Program(i) => content.entry(i).or_default().1 = Some(digest),
            }
        }
        let shares_source_with = sources
            .iter()
            .find(|s| {
                s.identity.file.device == stamp.file.device
                    && s.identity.file.inode == stamp.file.inode
            })
            .map(|s| s.ordinal);
        sources.push(SourceEvidence {
            ordinal: file.ordinal,
            file_type: file.file_type.clone(),
            path: file.path.clone(),
            identity: stamp.clone(),
            physical_sha256,
            shares_source_with,
            unassigned_prefix,
        });
    }
    before_final_verification();
    validate_inspection(inspection)?;
    let mut tracks = Vec::new();
    for (i, track) in inspection.timeline.tracks.iter().enumerate() {
        let (pregap, program) = content
            .remove(&i)
            .ok_or_else(|| unsupported("track has no content evidence"))?;
        tracks.push(TrackContentEvidence {
            number: track.number,
            source_backed_pregap: pregap,
            program: program.ok_or_else(|| unsupported("track program was not read"))?,
        });
    }
    Ok(DiscLayoutEvidence {
        cue_text: inspection.cue_text.clone(),
        cue_identity: inspection.cue_identity.clone(),
        cue_sha256: inspection.cue_sha256.clone(),
        layout: inspection.layout.clone(),
        timeline: inspection.timeline.clone(),
        sources,
        tracks,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let cue = dir.path().join("disc.cue");
        let bin = dir.path().join("disc.bin");
        std::fs::write(&bin, vec![1; 3 * 2048]).unwrap();
        std::fs::write(
            &cue,
            "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\n",
        )
        .unwrap();
        (dir, cue, bin)
    }

    #[test]
    fn indistinguishable_metadata_still_requires_the_inspected_content() {
        let (_dir, cue, bin) = fixture();
        let mut inspection = inspect_cue_preservation(&cue).unwrap();
        std::fs::write(&bin, vec![9; 3 * 2048]).unwrap();
        // Model a coarse filesystem's equality branch deterministically using
        // actual metadata from the file. No filesystem timestamp is fabricated
        // or changed; only the metadata proof is made indistinguishable. The
        // independently captured earlier content proof must remain decisive.
        inspection.sources[0].1 = identity(&std::fs::metadata(&bin).unwrap()).unwrap();
        assert_eq!(open_source(&bin).unwrap().1, inspection.sources[0].1);
        assert_eq!(
            read_cue_preservation(&inspection).unwrap_err(),
            changed(&bin)
        );
    }

    #[test]
    fn indistinguishable_metadata_cannot_refresh_captured_evidence() {
        let (_dir, cue, bin) = fixture();
        let mut evidence = collect_cue_preservation(&cue).unwrap();
        std::fs::write(&bin, vec![9; 3 * 2048]).unwrap();
        evidence.sources[0].identity = identity(&std::fs::metadata(&bin).unwrap()).unwrap();
        let captured = Ok(evidence);
        let comparison = compare_cue_evidence(&captured, &captured);
        assert_eq!(comparison.outcome, CueComparisonOutcome::Indeterminate);
        assert_eq!(comparison.programs_match, None);
        assert_eq!(comparison.refusals, vec![changed(&bin), changed(&bin)]);
    }

    #[test]
    fn inspection_requires_an_independent_content_verification() {
        let (_dir, cue, bin) = fixture();
        let result = inspect_with(&cue, |inspection| {
            assert_eq!(
                inspection.source_sha256,
                vec![hex(Sha256::digest(vec![1; 3 * 2048]))]
            );
            std::fs::write(&bin, vec![9; 3 * 2048]).unwrap();
            // Exercise metadata equality with real observed metadata, keeping
            // the earlier digest intact as in the coarse-filesystem model.
            inspection.sources[0].1 = identity(&std::fs::metadata(&bin).unwrap()).unwrap();
        });
        assert_eq!(result.unwrap_err(), changed(&bin));
    }

    #[test]
    fn mutation_of_unread_bytes_during_reading_is_refused() {
        use std::io::{Seek, SeekFrom, Write};
        let (_dir, cue, bin) = fixture();
        std::fs::write(&bin, vec![1; 2 * 128 * 1024]).unwrap();
        let inspection = inspect_cue_preservation(&cue).unwrap();
        let result = read_source_with(
            &bin,
            &inspection.sources[0].1,
            Some(&inspection.source_sha256[0]),
            &source_ranges(&inspection, 0).unwrap(),
            true,
            |read| {
                if read == 128 * 1024 {
                    let mut writer = File::options().write(true).open(&bin).unwrap();
                    writer.seek(SeekFrom::Start(read)).unwrap();
                    writer.write_all(&vec![9; 2048]).unwrap();
                }
            },
        );
        assert_eq!(result.err().unwrap(), changed(&bin));
    }

    #[test]
    fn atomic_replacement_during_reading_is_refused_even_with_identical_bytes() {
        let (dir, cue, bin) = fixture();
        let original = vec![1; 2 * 128 * 1024];
        std::fs::write(&bin, &original).unwrap();
        let inspection = inspect_cue_preservation(&cue).unwrap();
        let result = read_source_with(
            &bin,
            &inspection.sources[0].1,
            Some(&inspection.source_sha256[0]),
            &source_ranges(&inspection, 0).unwrap(),
            true,
            |read| {
                if read == 128 * 1024 {
                    let replacement = dir.path().join("replacement.bin");
                    std::fs::write(&replacement, &original).unwrap();
                    std::fs::rename(replacement, &bin).unwrap();
                }
            },
        );
        assert_eq!(result.err().unwrap(), changed(&bin));
    }

    #[test]
    fn truncation_during_reading_is_changed_source() {
        let (_dir, cue, bin) = fixture();
        std::fs::write(&bin, vec![1; 2 * 128 * 1024]).unwrap();
        let inspection = inspect_cue_preservation(&cue).unwrap();
        let result = read_source_with(
            &bin,
            &inspection.sources[0].1,
            Some(&inspection.source_sha256[0]),
            &source_ranges(&inspection, 0).unwrap(),
            true,
            |read| {
                if read == 128 * 1024 {
                    File::options()
                        .write(true)
                        .open(&bin)
                        .unwrap()
                        .set_len(read)
                        .unwrap();
                }
            },
        );
        assert_eq!(result.err().unwrap(), changed(&bin));
    }

    #[test]
    fn final_verification_checks_prefix_pregap_and_program_despite_equal_metadata() {
        use std::io::{Seek, SeekFrom, Write};
        for offset in [0, 2048, 4096] {
            let (_dir, cue, bin) = fixture();
            std::fs::write(
                &cue,
                "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 00 00:00:01\n INDEX 01 00:00:02\n",
            )
            .unwrap();
            let mut inspection = inspect_cue_preservation(&cue).unwrap();
            // Simulate the content pass having finished before an edit to
            // already-consumed bytes; final verification must reread them.
            read_source(
                &bin,
                &inspection.sources[0].1,
                Some(&inspection.source_sha256[0]),
                &source_ranges(&inspection, 0).unwrap(),
            )
            .unwrap();
            let mut writer = File::options().write(true).open(&bin).unwrap();
            writer.seek(SeekFrom::Start(offset)).unwrap();
            writer.write_all(&vec![9; 2048]).unwrap();
            inspection.sources[0].1 = identity(&std::fs::metadata(&bin).unwrap()).unwrap();
            assert_eq!(validate_inspection(&inspection).unwrap_err(), changed(&bin));
        }
    }

    #[test]
    fn a_source_replaced_after_hashing_is_refused_by_final_verification() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.bin");
        let second = dir.path().join("second.bin");
        let cue = dir.path().join("disc.cue");
        std::fs::write(&first, vec![1; 2048]).unwrap();
        std::fs::write(&second, vec![2; 2048]).unwrap();
        std::fs::write(&cue, "FILE \"first.bin\" BINARY\n TRACK 01 MODE1/2048\n INDEX 01 00:00:00\nFILE \"second.bin\" BINARY\n TRACK 02 MODE1/2048\n INDEX 01 00:00:00\n").unwrap();
        let inspection = inspect_cue_preservation(&cue).unwrap();
        let result = read_with(&inspection, || {
            let replacement = dir.path().join("replacement.bin");
            std::fs::write(&replacement, vec![1; 2048]).unwrap();
            std::fs::rename(replacement, &first).unwrap();
        });
        assert_eq!(result.unwrap_err(), changed(&first));
    }

    #[test]
    fn overflowing_read_range_is_refused_before_content_access() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disc.bin");
        std::fs::write(&path, vec![1; 2048]).unwrap();
        let (_, stamp) = open_source(&path).unwrap();
        let range = SourceRange {
            start_frame: 0,
            frames: 1,
            byte_offset: 1,
            byte_len: u64::MAX,
        };
        let result = read_source(&path, &stamp, None, &[(RangeRole::Program(0), range)]);
        assert!(matches!(
            result,
            Err(CuePreservationError::Unsupported { .. })
        ));
    }
}

pub fn collect_cue_preservation(path: &Path) -> Result<DiscLayoutEvidence, CuePreservationError> {
    read_cue_preservation(&inspect_cue_preservation(path)?)
}

fn is_unsupported(error: &CuePreservationError) -> bool {
    match error {
        CuePreservationError::Cue(CueError::Io(_) | CueError::MissingDataFile(_))
        | CuePreservationError::ChangedSource { .. }
        | CuePreservationError::Io { .. }
        | CuePreservationError::SafeRead {
            refusal:
                SafeReadRefusal::Unreadable(_)
                | SafeReadRefusal::OpenFailed(_)
                | SafeReadRefusal::UnresolvableTarget(_)
                | SafeReadRefusal::ChangedWhileOpening,
            ..
        } => false,
        _ => true,
    }
}

fn revalidate_comparison_pair(
    a: &DiscLayoutEvidence,
    b: &DiscLayoutEvidence,
) -> Vec<CuePreservationError> {
    let mut refusals = Vec::new();
    // The earlier left validation is not a proof that survives reading
    // the right operand. Recheck actual content on BOTH sides; timestamps
    // can remain equal after a same-size write with restored mtime.
    for evidence in [a, b] {
        if let Err(error) = validate_evidence(evidence) {
            refusals.push(error);
        }
    }
    if refusals.is_empty() {
        // Recheck the whole pair after both final content passes. An
        // observed replacement/mutation of left during the final right
        // read must not survive just because left's own read succeeded.
        for evidence in [a, b] {
            if let Err(error) = validate_relationships(&evidence_inspection(evidence)) {
                refusals.push(error);
            }
        }
    }
    refusals
}

/// Revalidate the CUE, FILE resolution, identities and every stored byte before
/// comparing evidence. Stale or unavailable evidence is indeterminate, even
/// against itself. Absolute paths, names and machine-local identities remain
/// provenance, not comparison facts. After initial validation, reread both
/// operands against their captured digests, then check both operands' source
/// relationships and pathname identities again. This catches changes to one
/// operand during the other's initial validation, even with equal timestamps.
///
/// All observations are sequential. A mutation after a source's final content
/// read that leaves its final metadata indistinguishable, or after its last
/// identity check, can still escape. No atomic snapshot or file lock is promised.
pub fn compare_cue_evidence(
    left: &Result<DiscLayoutEvidence, CuePreservationError>,
    right: &Result<DiscLayoutEvidence, CuePreservationError>,
) -> CuePreservationComparison {
    compare_with(left, right, || {})
}

fn compare_with(
    left: &Result<DiscLayoutEvidence, CuePreservationError>,
    right: &Result<DiscLayoutEvidence, CuePreservationError>,
    before_final_verification: impl FnOnce(),
) -> CuePreservationComparison {
    let mut refusals = Vec::new();
    let mut unsupported = false;
    for result in [left, right] {
        match result {
            Ok(evidence) => {
                if let Err(error) = validate_evidence(evidence) {
                    refusals.push(error);
                }
            }
            Err(error) => {
                unsupported |= is_unsupported(error);
                refusals.push(error.clone());
            }
        }
    }
    if refusals.is_empty() {
        before_final_verification();
        let (Ok(a), Ok(b)) = (left, right) else {
            unreachable!("successful validation requires two evidence objects");
        };
        refusals.extend(revalidate_comparison_pair(a, b));
    }
    if !refusals.is_empty() {
        return CuePreservationComparison {
            outcome: if unsupported {
                CueComparisonOutcome::Unsupported
            } else {
                CueComparisonOutcome::Indeterminate
            },
            differences: vec![],
            refusals,
            programs_match: None,
        };
    }
    let (Ok(a), Ok(b)) = (left, right) else {
        unreachable!("all refusals returned above");
    };
    let mut differences = Vec::new();
    if a.sources.len() != b.sources.len()
        || a.sources.iter().zip(&b.sources).any(|(a, b)| {
            (
                a.ordinal,
                &a.file_type,
                a.identity.file.size,
                a.shares_source_with,
            ) != (
                b.ordinal,
                &b.file_type,
                b.identity.file.size,
                b.shares_source_with,
            )
        })
    {
        differences.push(CueLayoutDifference::SourceRelationships);
    }
    for (a, b) in a.sources.iter().zip(&b.sources) {
        if a.physical_sha256 != b.physical_sha256 {
            differences.push(CueLayoutDifference::SourceContent { ordinal: a.ordinal });
        }
    }
    if a.layout.tracks.len() != b.layout.tracks.len()
        || a.layout.tracks.iter().zip(&b.layout.tracks).any(|(a, b)| {
            (a.number, &a.mode, a.file_ordinal, &a.file_type)
                != (b.number, &b.mode, b.file_ordinal, &b.file_type)
        })
    {
        differences.push(CueLayoutDifference::TrackStructure);
    }
    for (i, (a_decl, b_decl)) in a.layout.tracks.iter().zip(&b.layout.tracks).enumerate() {
        let (a_time, b_time) = (&a.timeline.tracks[i], &b.timeline.tracks[i]);
        if (
            a_decl.index_00,
            a_decl.index_01,
            &a_decl.extra_indexes,
            a_time.program,
        ) != (
            b_decl.index_00,
            b_decl.index_01,
            &b_decl.extra_indexes,
            b_time.program,
        ) {
            differences.push(CueLayoutDifference::IndexBoundaries {
                track: a_decl.number,
            });
        }
        if std::mem::discriminant(&a_time.pregap) != std::mem::discriminant(&b_time.pregap) {
            differences.push(CueLayoutDifference::PregapStorage {
                track: a_decl.number,
            });
        }
        if (a_decl.pregap, a_decl.postgap) != (b_decl.pregap, b_decl.postgap) {
            differences.push(CueLayoutDifference::SyntheticGaps {
                track: a_decl.number,
            });
        }
        let (a_content, b_content) = (&a.tracks[i], &b.tracks[i]);
        if let (Some(a), Some(b)) = (
            &a_content.source_backed_pregap,
            &b_content.source_backed_pregap,
        ) {
            if a.physical_sha256 != b.physical_sha256 {
                differences.push(CueLayoutDifference::StoredPregapContent {
                    track: a_decl.number,
                });
            }
        }
        if a_content.program.physical_sha256 != b_content.program.physical_sha256 {
            differences.push(CueLayoutDifference::ProgramContent {
                track: a_decl.number,
            });
        }
    }
    let programs_match = Some(
        a.tracks.len() == b.tracks.len()
            && a.tracks
                .iter()
                .zip(&b.tracks)
                .enumerate()
                .all(|(i, (a_track, b_track))| {
                    a_track.number == b_track.number
                        && a.layout.tracks[i].mode == b.layout.tracks[i].mode
                        && a_track.program.range.byte_len == b_track.program.range.byte_len
                        && a_track.program.physical_sha256 == b_track.program.physical_sha256
                }),
    );
    CuePreservationComparison {
        outcome: if differences.is_empty() {
            CueComparisonOutcome::Equivalent
        } else {
            CueComparisonOutcome::Different
        },
        differences,
        refusals: vec![],
        programs_match,
    }
}

#[cfg(all(test, unix))]
mod comparison_tests;
