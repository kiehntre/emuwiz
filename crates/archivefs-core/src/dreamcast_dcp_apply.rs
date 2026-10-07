//! Bounded DCP replacement of a reviewed extracted Dreamcast tree.
//! No image reconstruction or launch. Callers must supply an independently
//! reviewed package/source binding: DCP itself has no cryptographic base claim.
//! A returned shared receipt is staged, not published; use tree::{publish,inspect,undo}.
use crate::dreamcast_boot_evidence::ip_bin::{
    IpBinBootTargetStatus, IpBinFileInspection, IpBinPreview, check_boot_target,
    inspect_ip_bin_file,
};
use crate::dreamcast_boot_evidence::{
    DreamcastIpBinValidationStatus, IP_BIN_META_BYTES, inspect_ip_bin_meta,
};
use crate::dreamcast_patch_readiness::{
    DreamcastPatchEntryKind, DreamcastPatchPackage, MAX_DCP_EXPANDED_BYTES, inspect_dreamcast_dcp,
    safe_relative_path,
};
use crate::optical_patch_tree::{Content, Contents, refuse};
use crate::optical_patch_tree::{digest, file_content};
use crate::patch_output_recovery::tree::{self, PreparedTreePatch, TreePatchPlan};
use crate::standalone_patch::{locate_xdelta3, run_xdelta3_decode, xdelta3_version};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Logical size of the COMPLETE reviewed extracted tree (every regular file).
/// A GD-ROM high-density area spans LBA 45000..=549149, i.e. 504,150 sectors of
/// 2048 bytes = 984.7 MiB, so a genuine extracted tree cannot exceed 1 GiB. The
/// former 512 MiB bound refused roughly every retail disc that fills more than
/// half of the area. Sizes are logical (sparse holes count) and hashing streams
/// with fixed memory.
pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024 * 1024;
/// Source tree plus package: the largest combined input or output tree. Well
/// under the shared tree helper's `HARD_MAX_TOTAL_BYTES` (8 GiB).
pub const MAX_STAGING_BYTES: u64 = MAX_SOURCE_BYTES + MAX_DCP_EXPANDED_BYTES;

/// Extra staging headroom granted when a package carries xdelta members: a
/// decoded output may be larger than its base. Each output is also bounded by
/// [`MAX_DCP_ENTRY_BYTES`].
pub const DELTA_GROWTH_HEADROOM_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DELTA_OUTPUT_BYTES: u64 = crate::dreamcast_patch_readiness::MAX_DCP_ENTRY_BYTES;
const VCDIFF_MAGIC: [u8; 3] = [0xd6, 0xc3, 0xc4];

/// Typed reasons a DCP xdelta member (or its target) is refused. Carried as the
/// source of the returned `io::Error`; recover it with [`delta_refusal`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastDeltaRefusal {
    /// No supported `xdelta3` executable is available (readiness, not a bug).
    ToolUnavailable,
    /// The member's delta suffix is unsupported, empty, or names another patch
    /// format whose target exists in the tree.
    UnsupportedDeltaSuffix { member: String },
    /// The member is not a VCDIFF stream.
    NotVcdiff { member: String },
    /// A delta may not target `bootsector/IP.BIN`; that has its own handling.
    DeltaTargetsIpBin { member: String },
    /// The base file the delta applies to does not exist.
    MissingBase { member: String, target: String },
    /// The exact base path is absent but differently-cased files exist.
    AmbiguousBase {
        member: String,
        candidates: Vec<String>,
    },
    /// Two members produce the same destination.
    DuplicateDestination { destination: String },
    /// A direct replacement and a delta produce the same destination.
    DirectAndDeltaCollision { destination: String },
    /// A direct member would create a file that is not in the reviewed tree.
    NewFileNotSupported { member: String },
    /// The base file differs from the one bound at review.
    BaseChanged { target: String },
    /// xdelta3 failed (non-zero exit, timeout, bad base, ...).
    ToolFailed { member: String, detail: String },
    /// xdelta3 reported success but its output is not a usable regular file.
    BadOutput { member: String, detail: String },
}

impl std::fmt::Display for DreamcastDeltaRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToolUnavailable => f.write_str("xdelta3 is not available"),
            Self::UnsupportedDeltaSuffix { member } => {
                write!(f, "unsupported delta member {member}")
            }
            Self::NotVcdiff { member } => write!(f, "{member} is not a VCDIFF stream"),
            Self::DeltaTargetsIpBin { member } => {
                write!(f, "{member}: a delta may not target bootsector/IP.BIN")
            }
            Self::MissingBase { member, target } => {
                write!(f, "{member}: base file {target} does not exist")
            }
            Self::AmbiguousBase { member, candidates } => {
                write!(f, "{member}: ambiguous base ({})", candidates.join(", "))
            }
            Self::DuplicateDestination { destination } => {
                write!(f, "more than one member produces {destination}")
            }
            Self::DirectAndDeltaCollision { destination } => write!(
                f,
                "a direct replacement and a delta both produce {destination}"
            ),
            Self::NewFileNotSupported { member } => {
                write!(f, "{member}: new files are not supported")
            }
            Self::BaseChanged { target } => {
                write!(f, "base file {target} changed after review")
            }
            Self::ToolFailed { member, detail } => {
                write!(f, "xdelta3 failed for {member}: {detail}")
            }
            Self::BadOutput { member, detail } => {
                write!(f, "xdelta3 output for {member} is unusable: {detail}")
            }
        }
    }
}
impl std::error::Error for DreamcastDeltaRefusal {}

fn typed(refusal: DreamcastDeltaRefusal) -> io::Error {
    io::Error::other(refusal)
}
/// The typed delta refusal inside an error returned by this module, if any.
pub fn delta_refusal(error: &io::Error) -> Option<&DreamcastDeltaRefusal> {
    error.get_ref()?.downcast_ref::<DreamcastDeltaRefusal>()
}

/// What a successful decode proves, and nothing more.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamcastDeltaClaim {
    /// This patch transformed exactly this base file into exactly these bytes.
    /// It is NOT a claim of Redump, retail or canonical identity.
    DecodedFromExactBase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DreamcastDeltaReceiptEntry {
    pub member: String,
    pub target: String,
    pub member_sha256: String,
    pub base_size: u64,
    pub base_sha256: String,
    pub output_size: u64,
    pub output_sha256: String,
    pub claim: DreamcastDeltaClaim,
}

/// Evidence for the xdelta members of one prepared DCP application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DreamcastDeltaReceipt {
    pub package_sha256: String,
    pub source_tree_sha256: String,
    pub tool_path: String,
    pub tool_identity: Option<String>,
    pub entries: Vec<DreamcastDeltaReceiptEntry>,
}

#[derive(Clone, Debug)]
struct PlannedDelta {
    member: String,
    target: PathBuf,
    member_sha256: String,
    base: Content,
}

#[derive(Clone, Debug)]
struct DeltaOutput {
    target: PathBuf,
    output: Content,
}

fn usable_tool(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_absolute()
        && fs::symlink_metadata(path).is_ok_and(|m| {
            m.is_file() && !m.file_type().is_symlink() && m.permissions().mode() & 0o111 != 0
        })
}

/// `<target>` for `<target>.xdelta` / `<target>.vcdiff` (case-insensitive).
fn strip_delta_suffix(name: &str) -> Option<&str> {
    let lower = name.to_ascii_lowercase();
    [".xdelta", ".vcdiff"]
        .iter()
        .find(|suffix| lower.ends_with(**suffix))
        .map(|suffix| &name[..name.len() - suffix.len()])
}
/// Other patch-format suffixes that are not applied here.
fn other_patch_suffix(name: &str) -> Option<&str> {
    let lower = name.to_ascii_lowercase();
    [
        ".ips", ".bps", ".ups", ".ppf", ".xdelta3", ".vcdelta", ".delta",
    ]
    .iter()
    .find(|suffix| lower.ends_with(**suffix))
    .map(|suffix| &name[..name.len() - suffix.len()])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DreamcastIdentity {
    pub product_code: String,
    pub revision: String,
    pub region: String,
}
/// Expected values supplied by a trusted catalogue or explicit package/source
/// review. Merely computing two hashes does not establish their relationship.
#[derive(Clone, Debug)]
pub struct DreamcastDcpBinding {
    pub package_sha256: String,
    pub source_tree_sha256: String,
    pub identity: DreamcastIdentity,
}
#[derive(Clone, Debug)]
pub struct DreamcastDcpPlan {
    tree: TreePatchPlan,
    source: PathBuf,
    package: DreamcastPatchPackage,
    original: Contents,
    expected: Contents,
    identity: DreamcastIdentity,
    boot_filename: String,
    allowance: u64,
    deltas: Vec<PlannedDelta>,
    tool: Option<PathBuf>,
    tool_identity: Option<String>,
    source_tree_sha256: String,
    outputs: Arc<Mutex<Vec<DeltaOutput>>>,
}
/// Read-only review evidence, never an inferred package/source association.
pub fn source_tree_sha256(source: &Path) -> io::Result<String> {
    Contents::read(source, MAX_SOURCE_BYTES)?.fingerprint()
}

/// Full IP.BIN facts from the existing extracted-tree convention. Root members
/// are only listed/stat'ed for this check; no image mounting or executable run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedDreamcastIpBinInspection {
    pub ip_bin: IpBinFileInspection,
    pub boot_target: IpBinBootTargetStatus,
}
pub fn inspect_extracted_dreamcast_ip_bin(
    source: &Path,
) -> io::Result<ExtractedDreamcastIpBinInspection> {
    let ip_bin = inspect_ip_bin_file(&source.join("bootsector/IP.BIN"))?;
    let boot_target = match &ip_bin.inspection.ip_bin {
        Some(ip) => check_boot_target(source, &ip.metadata.boot_filename.value)?,
        None => IpBinBootTargetStatus::NotChecked,
    };
    Ok(ExtractedDreamcastIpBinInspection {
        ip_bin,
        boot_target,
    })
}

/// Explicit IP.BIN-only edits to an extracted tree use the same staging,
/// immutable receipts, revalidation, publication and undo as DCP replacements.
#[derive(Clone, Debug)]
pub struct DreamcastIpBinTreePlan {
    tree: TreePatchPlan,
    source: PathBuf,
    original: Contents,
    expected: Contents,
    preview: IpBinPreview,
}
pub fn review_extracted_dreamcast_ip_bin(
    source: &Path,
    destination: &Path,
    preview: &IpBinPreview,
) -> io::Result<DreamcastIpBinTreePlan> {
    if preview.source().path != source.join("bootsector/IP.BIN") {
        return Err(refuse(
            "IP.BIN preview belongs to a different extracted tree",
        ));
    }
    preview.verify_source()?;
    let original = Contents::read(source, MAX_SOURCE_BYTES)?;
    let mut expected = original.clone();
    expected.0.insert(
        PathBuf::from("bootsector/IP.BIN"),
        Some(Content::bytes(preview.expected_bytes())),
    );
    let ip = preview.expected().ip_bin.as_ref().unwrap();
    let original_boot = inspect_extracted_dreamcast_ip_bin(source)?
        .ip_bin
        .inspection
        .ip_bin
        .unwrap()
        .metadata
        .boot_filename
        .value;
    if original_boot != ip.metadata.boot_filename.value
        && !matches!(
            check_boot_target(source, &ip.metadata.boot_filename.value)?,
            IpBinBootTargetStatus::Present(_)
        )
    {
        return Err(refuse(
            "edited boot filename needs an exact, unambiguous regular tree member",
        ));
    }
    let tree = TreePatchPlan::review_with_max_total_bytes(
        &[source.to_owned()],
        destination,
        MAX_SOURCE_BYTES,
    )?;
    original.verify(source, MAX_SOURCE_BYTES)?;
    preview.verify_source()?;
    Ok(DreamcastIpBinTreePlan {
        tree,
        source: source.to_owned(),
        original,
        expected,
        preview: preview.clone(),
    })
}
impl DreamcastIpBinTreePlan {
    pub fn prepare(&self) -> io::Result<PreparedTreePatch> {
        self.preview.verify_source()?;
        tree::prepare(
            &self.tree,
            |staging| {
                self.original.verify(&self.source, MAX_SOURCE_BYTES)?;
                self.original.copy(&self.source, staging)?;
                fs::write(
                    staging.join("bootsector/IP.BIN"),
                    self.preview.expected_bytes(),
                )
            },
            |staging| {
                self.expected.verify(staging, MAX_SOURCE_BYTES)?;
                self.preview
                    .verify_output(&staging.join("bootsector/IP.BIN"))
            },
        )
    }
}
fn verify_ip(root: &Path, identity: &DreamcastIdentity) -> io::Result<String> {
    let mut bytes = [0u8; IP_BIN_META_BYTES];
    File::open(root.join("bootsector/IP.BIN"))?.read_exact(&mut bytes)?;
    let ip = inspect_ip_bin_meta(&bytes).map_err(|e| refuse(e.message))?;
    if matches!(
        ip.validation_status,
        DreamcastIpBinValidationStatus::Invalid | DreamcastIpBinValidationStatus::Truncated
    ) || !matches!(
        ip.hardware_id.value.as_str(),
        "SEGA SEGAKATANA" | "SEGA SEGAMARIO"
    ) || !ip.device_information.value.contains("GD-ROM")
        || identity.product_code.is_empty()
        || identity.revision.is_empty()
        || identity.region.is_empty()
        || ip.product_number.value != identity.product_code
        || ip.product_version.value != identity.revision
        || ip.area_symbols.value != identity.region
    {
        return Err(refuse(
            "Dreamcast IP.BIN domain/product/revision/region mismatch",
        ));
    }
    safe_relative_path(&ip.boot_filename.value).map_err(refuse)?;
    if !fs::symlink_metadata(root.join(&ip.boot_filename.value))?.is_file() {
        return Err(refuse("missing regular Dreamcast boot member"));
    }
    Ok(ip.boot_filename.value)
}
/// Only a directory with bootsector/IP.BIN and reviewed existing replacement
/// targets is accepted. Passing GDI/CHD/CDI/raw media fails before patch decoding.
pub fn review_dreamcast_dcp(
    source: &Path,
    package_path: &Path,
    destination: &Path,
    binding: &DreamcastDcpBinding,
) -> io::Result<DreamcastDcpPlan> {
    review_dreamcast_dcp_with_tool(source, package_path, destination, binding, None)
}

/// As [`review_dreamcast_dcp`], with an explicit `xdelta3` executable (tests,
/// or a caller-configured tool). `None` uses the `xdelta3` on `PATH`. The tool
/// is only required when the package contains xdelta members; it is never
/// downloaded.
pub fn review_dreamcast_dcp_with_tool(
    source: &Path,
    package_path: &Path,
    destination: &Path,
    binding: &DreamcastDcpBinding,
    tool: Option<&Path>,
) -> io::Result<DreamcastDcpPlan> {
    if package_path.starts_with(source) {
        return Err(refuse("DCP must be outside the source tree"));
    }
    let original = Contents::read(source, MAX_SOURCE_BYTES)?;
    if original.fingerprint()? != binding.source_tree_sha256 {
        return Err(refuse("exact extracted source binding mismatch"));
    }
    // Shared validation also rejects patch symlink ancestors, hardlinks, input
    // overlap and destinations inside source before any package decoding.
    TreePatchPlan::review_with_max_total_bytes(
        &[source.to_owned(), package_path.to_owned()],
        destination,
        MAX_STAGING_BYTES,
    )?;
    let package = inspect_dreamcast_dcp(package_path).map_err(refuse)?;
    if package.package_sha256 != binding.package_sha256 {
        return Err(refuse("exact package binding mismatch"));
    }
    let boot_filename = verify_ip(source, &binding.identity)?;
    let mut expected = original.clone();
    let mut replacements = 0;
    let mut deltas: Vec<PlannedDelta> = Vec::new();
    // lower-cased destination -> (member, is_delta)
    let mut destinations: BTreeMap<String, (String, bool)> = BTreeMap::new();
    let mut archive = zip::ZipArchive::new(File::open(&package.path)?).map_err(refuse)?;
    for entry in &package.entries {
        let is_delta = match entry.kind {
            DreamcastPatchEntryKind::Metadata => continue,
            DreamcastPatchEntryKind::FileReplacement | DreamcastPatchEntryKind::IpBin => false,
            DreamcastPatchEntryKind::FileDelta => true,
            DreamcastPatchEntryKind::Unknown => {
                return Err(refuse("opaque DCP delta/unsupported patch operation"));
            }
        };
        let member = entry.relative_path.clone();
        let unsupported = || {
            typed(DreamcastDeltaRefusal::UnsupportedDeltaSuffix {
                member: member.clone(),
            })
        };
        let target_name = if is_delta {
            let name = strip_delta_suffix(&member).ok_or_else(unsupported)?;
            if name.is_empty() || name.ends_with('/') {
                return Err(unsupported());
            }
            safe_relative_path(name).map_err(refuse)?;
            name.to_owned()
        } else {
            member.clone()
        };
        if is_delta && target_name.eq_ignore_ascii_case("bootsector/IP.BIN") {
            return Err(typed(DreamcastDeltaRefusal::DeltaTargetsIpBin { member }));
        }
        if let Some((_, other_is_delta)) =
            destinations.insert(target_name.to_ascii_lowercase(), (member.clone(), is_delta))
        {
            return Err(typed(if other_is_delta != is_delta {
                DreamcastDeltaRefusal::DirectAndDeltaCollision {
                    destination: target_name,
                }
            } else {
                DreamcastDeltaRefusal::DuplicateDestination {
                    destination: target_name,
                }
            }));
        }
        let path = Path::new(&target_name);
        if !is_delta
            && let Some(stripped) = other_patch_suffix(&member)
            && original
                .0
                .get(Path::new(stripped))
                .is_some_and(Option::is_some)
        {
            return Err(unsupported());
        }
        let before = match original.file(path) {
            Ok(before) => before,
            Err(_) if is_delta => {
                let candidates: Vec<String> = original
                    .0
                    .iter()
                    .filter(|(rel, content)| {
                        content.is_some()
                            && rel.to_string_lossy().eq_ignore_ascii_case(&target_name)
                    })
                    .map(|(rel, _)| rel.to_string_lossy().into_owned())
                    .collect();
                return Err(typed(if candidates.is_empty() {
                    DreamcastDeltaRefusal::MissingBase {
                        member,
                        target: target_name,
                    }
                } else {
                    DreamcastDeltaRefusal::AmbiguousBase { member, candidates }
                }));
            }
            // No new/outside targets for direct members.
            Err(_) => {
                return Err(typed(DreamcastDeltaRefusal::NewFileNotSupported { member }));
            }
        };
        if is_delta {
            let mut magic = [0u8; 3];
            archive
                .by_name(&member)
                .map_err(refuse)?
                .read_exact(&mut magic)
                .map_err(|_| {
                    typed(DreamcastDeltaRefusal::NotVcdiff {
                        member: member.clone(),
                    })
                })?;
            if magic != VCDIFF_MAGIC {
                return Err(typed(DreamcastDeltaRefusal::NotVcdiff { member }));
            }
            deltas.push(PlannedDelta {
                member,
                target: path.to_owned(),
                member_sha256: entry.sha256.clone(),
                base: before.clone(),
            });
            replacements += 1;
            continue;
        }
        if entry.kind == DreamcastPatchEntryKind::IpBin && entry.size_bytes != before.size {
            return Err(refuse(
                "IP.BIN replacement must preserve the reviewed boot-sector length",
            ));
        }
        expected.0.insert(
            path.to_owned(),
            Some(Content {
                size: entry.size_bytes,
                sha256: entry.sha256.clone(),
            }),
        );
        replacements += 1;
    }
    let (tool, tool_identity) = if deltas.is_empty() {
        (None, None)
    } else {
        let tool = tool
            .map(Path::to_path_buf)
            .or_else(locate_xdelta3)
            .filter(|path| usable_tool(path))
            .ok_or_else(|| typed(DreamcastDeltaRefusal::ToolUnavailable))?;
        let identity = xdelta3_version(&tool);
        (Some(tool), identity)
    };
    if replacements == 0 {
        return Err(refuse("DCP has no supported replacements"));
    }
    let input_size = original
        .size()?
        .checked_add(fs::metadata(package_path)?.len())
        .ok_or_else(|| refuse("input size overflow"))?;
    let headroom = if deltas.is_empty() {
        0
    } else {
        DELTA_GROWTH_HEADROOM_BYTES
    };
    let allowance = input_size
        .max(expected.size()?)
        .max(1)
        .saturating_add(headroom);
    if allowance > MAX_STAGING_BYTES + headroom {
        return Err(refuse("DCP staging byte policy exceeded"));
    }
    // Bind freshness, then recheck the independently reviewed content.
    let tree = TreePatchPlan::review_with_max_total_bytes(
        &[source.to_owned(), package_path.to_owned()],
        destination,
        allowance,
    )?;
    original.verify(source, MAX_SOURCE_BYTES)?;
    if inspect_dreamcast_dcp(package_path).map_err(refuse)? != package {
        return Err(refuse("package changed during review"));
    }
    Ok(DreamcastDcpPlan {
        tree,
        source: source.to_owned(),
        package,
        original,
        expected,
        identity: binding.identity.clone(),
        boot_filename,
        allowance,
        deltas,
        tool,
        tool_identity,
        source_tree_sha256: binding.source_tree_sha256.clone(),
        outputs: Arc::new(Mutex::new(Vec::new())),
    })
}
impl DreamcastDcpPlan {
    pub fn max_total_bytes(&self) -> u64 {
        self.allowance
    }
    /// Every delta base must still be the exact file bound at review.
    fn check_bases(&self) -> io::Result<()> {
        for delta in &self.deltas {
            let bound = file_content(&self.source.join(&delta.target), MAX_SOURCE_BYTES).ok();
            if bound.as_ref() != Some(&delta.base) {
                return Err(typed(DreamcastDeltaRefusal::BaseChanged {
                    target: delta.target.display().to_string(),
                }));
            }
        }
        Ok(())
    }
    fn produce(&self, staging: &Path) -> io::Result<()> {
        self.check_bases()?;
        if inspect_dreamcast_dcp(&self.package.path).map_err(refuse)? != self.package {
            return Err(refuse("DCP changed after review"));
        }
        self.original.verify(&self.source, MAX_SOURCE_BYTES)?;
        self.original.copy(&self.source, staging)?;
        let mut archive = zip::ZipArchive::new(File::open(&self.package.path)?).map_err(refuse)?;
        for entry in &self.package.entries {
            if matches!(
                entry.kind,
                DreamcastPatchEntryKind::Metadata | DreamcastPatchEntryKind::FileDelta
            ) {
                continue;
            }
            let mut bytes = Vec::new();
            archive
                .by_name(&entry.relative_path)
                .map_err(refuse)?
                .take(entry.size_bytes + 1)
                .read_to_end(&mut bytes)?;
            if Content::bytes(&bytes) != *self.expected.file(Path::new(&entry.relative_path))? {
                return Err(refuse("DCP member changed"));
            }
            fs::write(staging.join(&entry.relative_path), bytes)?;
        }
        self.stage_deltas(&mut archive, staging)
    }
    /// VERIFY BASE -> DECODE EACH DELTA IN SCRATCH -> MOVE INTO STAGING. Every
    /// base is re-hashed against the review-time binding before any decode, and
    /// all output stays in staging/scratch: the live tree is never touched.
    fn stage_deltas(&self, archive: &mut zip::ZipArchive<File>, staging: &Path) -> io::Result<()> {
        self.outputs
            .lock()
            .map_err(|_| refuse("receipt state poisoned"))?
            .clear();
        if self.deltas.is_empty() {
            return Ok(());
        }
        let tool = self
            .tool
            .as_deref()
            .ok_or_else(|| typed(DreamcastDeltaRefusal::ToolUnavailable))?;
        let scratch = tempfile::Builder::new()
            .prefix(".emuwiz-dcp-xdelta-")
            .tempdir()?;
        for (index, delta) in self.deltas.iter().enumerate() {
            let patch_path = scratch.path().join(format!("{index}.patch"));
            let output_path = scratch.path().join(format!("{index}.out"));
            let mut patch = Vec::new();
            archive
                .by_name(&delta.member)
                .map_err(refuse)?
                .take(MAX_DELTA_OUTPUT_BYTES + 1)
                .read_to_end(&mut patch)?;
            if digest(&patch) != delta.member_sha256 {
                return Err(refuse("DCP delta member changed"));
            }
            fs::write(&patch_path, &patch)?;
            run_xdelta3_decode(
                tool,
                &self.source.join(&delta.target),
                &patch_path,
                &output_path,
            )
            .map_err(|error| {
                typed(DreamcastDeltaRefusal::ToolFailed {
                    member: delta.member.clone(),
                    detail: format!("{error:?}"),
                })
            })?;
            let bad = |detail: &str| {
                typed(DreamcastDeltaRefusal::BadOutput {
                    member: delta.member.clone(),
                    detail: detail.to_string(),
                })
            };
            let metadata = fs::symlink_metadata(&output_path)
                .map_err(|_| bad("no output file was produced"))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(bad("output is not a regular file"));
            }
            if metadata.len() > MAX_DELTA_OUTPUT_BYTES {
                return Err(bad("output exceeds the per-file bound"));
            }
            // The staged copy of the (unchanged) base is replaced by the output.
            let staged = staging.join(&delta.target);
            fs::remove_file(&staged)?;
            let mut input = File::open(&output_path)?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
            let content = file_content(&staged, MAX_DELTA_OUTPUT_BYTES)?;
            self.outputs
                .lock()
                .map_err(|_| refuse("receipt state poisoned"))?
                .push(DeltaOutput {
                    target: delta.target.clone(),
                    output: content,
                });
            let _ = fs::remove_file(&output_path);
            let _ = fs::remove_file(&patch_path);
        }
        Ok(())
    }
    fn verify(&self, staging: &Path) -> io::Result<()> {
        let outputs = self
            .outputs
            .lock()
            .map_err(|_| refuse("receipt state poisoned"))?
            .clone();
        if outputs.len() != self.deltas.len() {
            return Err(refuse("not every delta produced a staged output"));
        }
        let mut expected = self.expected.clone();
        for output in &outputs {
            expected
                .0
                .insert(output.target.clone(), Some(output.output.clone()));
        }
        expected.verify(staging, self.allowance)?;
        if verify_ip(staging, &self.identity)? != self.boot_filename {
            return Err(refuse("Dreamcast boot mapping changed"));
        }
        Ok(())
    }
    pub fn prepare(&self) -> io::Result<PreparedTreePatch> {
        let prepared = tree::prepare(
            &self.tree,
            |staging| self.produce(staging),
            |staging| self.verify(staging),
        )?;
        if let Some(receipt) = self.delta_receipt() {
            // Written only after the whole transaction staged and verified, so
            // a failed run never leaves a success receipt behind.
            let path = prepared.journal_path.with_extension("dcp-delta.json");
            let bytes = serde_json::to_vec_pretty(&receipt).map_err(refuse)?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            io::Write::write_all(&mut file, &bytes)?;
            file.sync_all()?;
        }
        Ok(prepared)
    }
    /// Evidence for the xdelta members, available once `prepare` has staged
    /// them. `None` for packages without deltas or before staging.
    pub fn delta_receipt(&self) -> Option<DreamcastDeltaReceipt> {
        let outputs = self.outputs.lock().ok()?.clone();
        if self.deltas.is_empty() || outputs.len() != self.deltas.len() {
            return None;
        }
        let entries = self
            .deltas
            .iter()
            .filter_map(|delta| {
                let output = outputs.iter().find(|o| o.target == delta.target)?;
                Some(DreamcastDeltaReceiptEntry {
                    member: delta.member.clone(),
                    target: delta.target.display().to_string(),
                    member_sha256: delta.member_sha256.clone(),
                    base_size: delta.base.size,
                    base_sha256: delta.base.sha256.clone(),
                    output_size: output.output.size,
                    output_sha256: output.output.sha256.clone(),
                    claim: DreamcastDeltaClaim::DecodedFromExactBase,
                })
            })
            .collect();
        Some(DreamcastDeltaReceipt {
            package_sha256: self.package.package_sha256.clone(),
            source_tree_sha256: self.source_tree_sha256.clone(),
            tool_path: self.tool.as_ref()?.display().to_string(),
            tool_identity: self.tool_identity.clone(),
            entries,
        })
    }
}

#[cfg(test)]
#[path = "dreamcast_dcp_apply_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "dreamcast_dcp_xdelta_tests.rs"]
mod xdelta_tests;
