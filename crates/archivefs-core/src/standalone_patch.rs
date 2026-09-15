//! Bounded, read-only inspection of standalone ROM patch files.
//!
//! This module deliberately stops at an immutable derived-output plan.  It
//! never invokes a patcher, writes the base game, or creates the derivative.
//! Format parsers validate framing and declared sizes without allocating from
//! untrusted output-size fields.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const MAX_PATCH_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_METADATA_BYTES: usize = 1024 * 1024;
pub const MAX_RECORDS: usize = 1_000_000;
pub const MAX_DECLARED_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StandalonePatchFormat {
    Ips,
    Bps,
    Ups,
    XdeltaVcdiff,
    Ppf,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchInspectionState {
    Valid,
    Invalid,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchCompatibility {
    Compatible,
    Incompatible,
    ReviewRequired,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandalonePatchInspection {
    pub path: PathBuf,
    pub format: StandalonePatchFormat,
    pub state: PatchInspectionState,
    pub patch_size: u64,
    pub patch_sha256: String,
    pub source_size: Option<u64>,
    pub target_size: Option<u64>,
    pub source_crc32: Option<u32>,
    pub target_crc32: Option<u32>,
    pub patch_crc32: Option<u32>,
    pub metadata: Option<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandalonePatchMatch {
    pub compatibility: PatchCompatibility,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DerivedPatchPlan {
    pub base_path: PathBuf,
    pub patch_path: PathBuf,
    pub output_path: PathBuf,
    pub patch_format: StandalonePatchFormat,
    pub base_sha256: Option<String>,
    pub patch_sha256: String,
    pub expected_source_size: Option<u64>,
    pub expected_output_size: Option<u64>,
    pub expected_output_crc32: Option<u32>,
    pub overwrite_existing: bool,
    pub confirmation_required: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StandalonePatchError {
    Io(String),
    TooLarge,
    Malformed(String),
    Unsupported(String),
    UnsafeOutput(String),
    SourceEqualsOutput,
}

impl std::fmt::Display for StandalonePatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "patch I/O error: {s}"),
            Self::TooLarge => write!(f, "patch exceeds the bounded inspection limit"),
            Self::Malformed(s) => write!(f, "malformed patch: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported patch: {s}"),
            Self::UnsafeOutput(s) => write!(f, "unsafe derived output: {s}"),
            Self::SourceEqualsOutput => write!(f, "derived output must differ from the base"),
        }
    }
}
impl std::error::Error for StandalonePatchError {}

pub fn inspect_standalone_patch(
    path: impl AsRef<Path>,
) -> Result<StandalonePatchInspection, StandalonePatchError> {
    let path = path.as_ref().to_path_buf();
    let bytes = fs::read(&path).map_err(|e| StandalonePatchError::Io(e.to_string()))?;
    if bytes.len() > MAX_PATCH_BYTES {
        return Err(StandalonePatchError::TooLarge);
    }
    let sha = hex_digest(&bytes);
    let (format, state, fields) = parse(&bytes);
    Ok(StandalonePatchInspection {
        path,
        format,
        state,
        patch_size: bytes.len() as u64,
        patch_sha256: sha,
        source_size: fields.source_size,
        target_size: fields.target_size,
        source_crc32: fields.source_crc32,
        target_crc32: fields.target_crc32,
        patch_crc32: fields.patch_crc32,
        metadata: fields.metadata,
        warnings: fields.warnings,
        error: fields.error,
    })
}

/// Match only evidence that can identify the exact source.  `title_or_region`
/// is intentionally not enough for a confident update/apply decision.
pub fn match_patch_source(
    inspection: &StandalonePatchInspection,
    base_bytes: Option<&[u8]>,
    title_or_region: bool,
) -> StandalonePatchMatch {
    if !matches!(inspection.state, PatchInspectionState::Valid) {
        return StandalonePatchMatch {
            compatibility: PatchCompatibility::Unknown,
            reason: "patch is not structurally valid".into(),
        };
    }
    if let Some(bytes) = base_bytes {
        if inspection
            .source_size
            .is_some_and(|size| size != bytes.len() as u64)
        {
            return StandalonePatchMatch {
                compatibility: PatchCompatibility::Incompatible,
                reason: "base size differs from patch source size".into(),
            };
        }
        if let Some(expected) = inspection.source_crc32 {
            if crc32(bytes) != expected {
                return StandalonePatchMatch {
                    compatibility: PatchCompatibility::Incompatible,
                    reason: "base CRC32 differs from patch source checksum".into(),
                };
            }
            return StandalonePatchMatch {
                compatibility: PatchCompatibility::Compatible,
                reason: "base size and embedded source checksum match".into(),
            };
        }
    }
    if inspection.source_size.is_none() && inspection.source_crc32.is_none() {
        return StandalonePatchMatch {
            compatibility: if title_or_region {
                PatchCompatibility::ReviewRequired
            } else {
                PatchCompatibility::Unknown
            },
            reason: "format does not contain source identity".into(),
        };
    }
    StandalonePatchMatch {
        compatibility: PatchCompatibility::ReviewRequired,
        reason: "source checksum must be compared with the selected base bytes".into(),
    }
}

pub fn build_derived_patch_plan(
    inspection: &StandalonePatchInspection,
    base_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    approved_output_root: impl AsRef<Path>,
    base_sha256: Option<String>,
) -> Result<DerivedPatchPlan, StandalonePatchError> {
    if !matches!(inspection.state, PatchInspectionState::Valid) {
        return Err(StandalonePatchError::Malformed(
            "cannot plan an invalid patch".into(),
        ));
    }
    let base = base_path.as_ref().to_path_buf();
    let output = output_path.as_ref().to_path_buf();
    if base == output {
        return Err(StandalonePatchError::SourceEqualsOutput);
    }
    let root = approved_output_root.as_ref();
    if !is_safe_child(root, &output) {
        return Err(StandalonePatchError::UnsafeOutput(
            "output is outside the approved derivative root or uses a symlink component".into(),
        ));
    }
    if output.exists() && !output.is_file() {
        return Err(StandalonePatchError::UnsafeOutput(
            "existing output is not a regular file".into(),
        ));
    }
    Ok(DerivedPatchPlan {
        base_path: base,
        patch_path: inspection.path.clone(),
        output_path: output,
        patch_format: inspection.format,
        base_sha256,
        patch_sha256: inspection.patch_sha256.clone(),
        expected_source_size: inspection.source_size,
        expected_output_size: inspection.target_size,
        expected_output_crc32: inspection.target_crc32,
        overwrite_existing: false,
        confirmation_required: true,
        warnings: inspection.warnings.clone(),
    })
}

#[derive(Default)]
struct Fields {
    source_size: Option<u64>,
    target_size: Option<u64>,
    source_crc32: Option<u32>,
    target_crc32: Option<u32>,
    patch_crc32: Option<u32>,
    metadata: Option<String>,
    warnings: Vec<String>,
    error: Option<String>,
}

fn parse(b: &[u8]) -> (StandalonePatchFormat, PatchInspectionState, Fields) {
    if b.starts_with(b"PATCH") {
        let (s, f) = parse_ips(b);
        return (StandalonePatchFormat::Ips, s, f);
    }
    if b.starts_with(b"BPS1") {
        let (s, f) = parse_bps(b);
        return (StandalonePatchFormat::Bps, s, f);
    }
    if b.starts_with(b"UPS1") {
        let (s, f) = parse_ups(b);
        return (StandalonePatchFormat::Ups, s, f);
    }
    if b.starts_with(&[0xd6, 0xc4, 0xc3]) {
        return (
            StandalonePatchFormat::XdeltaVcdiff,
            PatchInspectionState::Valid,
            Fields {
                warnings: vec![
                    "VCDIFF structure recognized; source identity is not embedded".into(),
                ],
                ..Default::default()
            },
        );
    }
    if b.len() >= 4 && &b[..3] == b"PPF" && (b[3] == b'1' || b[3] == b'2' || b[3] == b'3') {
        if b.len() < 6 {
            let (state, fields) = invalid(StandalonePatchFormat::Ppf, "truncated PPF header");
            return (StandalonePatchFormat::Ppf, state, fields);
        }
        return (
            StandalonePatchFormat::Ppf,
            PatchInspectionState::Valid,
            Fields {
                warnings: vec![
                    "PPF is disc-image oriented; topology and source identity require review"
                        .into(),
                ],
                ..Default::default()
            },
        );
    }
    (
        StandalonePatchFormat::Unknown,
        PatchInspectionState::Unsupported,
        Fields {
            error: Some("no supported patch signature".into()),
            ..Default::default()
        },
    )
}

fn invalid(_format: StandalonePatchFormat, msg: &str) -> (PatchInspectionState, Fields) {
    (
        PatchInspectionState::Invalid,
        Fields {
            error: Some(msg.into()),
            ..Default::default()
        },
    )
}

fn parse_ips(b: &[u8]) -> (PatchInspectionState, Fields) {
    let mut f = Fields::default();
    let mut p = 5;
    let mut records = 0;
    let mut max = 0u64;
    while p < b.len() {
        if b[p..].starts_with(b"EOF") {
            p += 3;
            if p < b.len() {
                if b.len() - p != 3 {
                    return invalid(StandalonePatchFormat::Ips, "trailing bytes after IPS EOF");
                }
            }
            return (PatchInspectionState::Valid, {
                f.target_size = Some(max);
                f
            });
        }
        if b.len() - p < 5 {
            return invalid(StandalonePatchFormat::Ips, "truncated IPS record");
        }
        records += 1;
        if records > MAX_RECORDS {
            return invalid(StandalonePatchFormat::Ips, "too many records");
        }
        let offset = ((b[p] as u64) << 16) | ((b[p + 1] as u64) << 8) | b[p + 2] as u64;
        let size = u16::from_be_bytes([b[p + 3], b[p + 4]]) as u64;
        p += 5;
        let length = if size == 0 {
            if b.len() - p < 3 {
                return invalid(StandalonePatchFormat::Ips, "truncated IPS RLE record");
            }
            let n = u16::from_be_bytes([b[p], b[p + 1]]) as u64;
            p += 3;
            if n == 0 {
                return invalid(StandalonePatchFormat::Ips, "zero-length IPS RLE record");
            }
            n
        } else {
            size
        };
        if size != 0 {
            if b.len() - p < size as usize {
                return invalid(StandalonePatchFormat::Ips, "truncated IPS data");
            }
            p += size as usize;
        }
        max = max.max(offset.checked_add(length).unwrap_or(u64::MAX));
        if max > MAX_DECLARED_OUTPUT_BYTES {
            return invalid(StandalonePatchFormat::Ips, "output bound is excessive");
        }
    }
    invalid(StandalonePatchFormat::Ips, "missing IPS EOF")
}

fn read_var(b: &[u8], p: &mut usize) -> Result<u64, &'static str> {
    let mut value = 0u64;
    let mut shift = 0;
    for _ in 0..10 {
        let x = *b.get(*p).ok_or("truncated variable integer")?;
        *p += 1;
        value = value
            .checked_add(
                ((x & 0x7f) as u64)
                    .checked_shl(shift)
                    .ok_or("variable integer overflow")?,
            )
            .ok_or("variable integer overflow")?;
        if x & 0x80 == 0 {
            return Ok(value);
        };
        shift += 7;
    }
    Err("variable integer too long")
}

fn parse_bps(b: &[u8]) -> (PatchInspectionState, Fields) {
    let mut f = Fields::default();
    if b.len() < 16 {
        return invalid(StandalonePatchFormat::Bps, "truncated BPS");
    }
    let end = b.len() - 12;
    let mut p = 4;
    let source = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Bps, e),
    };
    let target = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Bps, e),
    };
    let meta = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Bps, e),
    };
    if target > MAX_DECLARED_OUTPUT_BYTES
        || meta as usize > MAX_METADATA_BYTES
        || meta as usize > end.saturating_sub(p)
    {
        return invalid(StandalonePatchFormat::Bps, "invalid size or metadata");
    }
    if end - p < meta as usize {
        return invalid(StandalonePatchFormat::Bps, "truncated metadata");
    }
    if meta > 0 {
        f.metadata = Some(String::from_utf8_lossy(&b[p..p + meta as usize]).into_owned());
    }
    p += meta as usize;
    let mut output = 0u64;
    let mut records = 0;
    while p < end {
        let a = match read_var(b, &mut p) {
            Ok(v) => v,
            Err(e) => return invalid(StandalonePatchFormat::Bps, e),
        };
        let len = (a >> 2).checked_add(1).unwrap_or(u64::MAX);
        if len > target.saturating_sub(output) {
            return invalid(StandalonePatchFormat::Bps, "operations exceed target size");
        }
        match a & 3 {
            0 => {}
            1 => {
                if p.checked_add(len as usize).is_none() || p + len as usize > end {
                    return invalid(StandalonePatchFormat::Bps, "truncated target data");
                }
                p += len as usize;
            }
            2 | 3 => {
                if read_var(b, &mut p).is_err() {
                    return invalid(StandalonePatchFormat::Bps, "truncated copy offset");
                }
            }
            _ => unreachable!(),
        }
        output += len;
        records += 1;
        if records > MAX_RECORDS {
            return invalid(StandalonePatchFormat::Bps, "too many records");
        }
    }
    f.source_size = Some(source);
    f.target_size = Some(target);
    f.source_crc32 = Some(u32::from_le_bytes(b[end..end + 4].try_into().unwrap()));
    f.target_crc32 = Some(u32::from_le_bytes(b[end + 4..end + 8].try_into().unwrap()));
    f.patch_crc32 = Some(u32::from_le_bytes(b[end + 8..].try_into().unwrap()));
    if crc32(&b[..end + 8]) != f.patch_crc32.unwrap() {
        return invalid(StandalonePatchFormat::Bps, "patch CRC mismatch");
    }
    if output != target {
        return invalid(
            StandalonePatchFormat::Bps,
            "operations do not produce target size",
        );
    }
    (PatchInspectionState::Valid, f)
}

fn parse_ups(b: &[u8]) -> (PatchInspectionState, Fields) {
    let mut f = Fields::default();
    if b.len() < 16 {
        return invalid(StandalonePatchFormat::Ups, "truncated UPS");
    }
    let end = b.len() - 12;
    let mut p = 4;
    let s = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Ups, e),
    };
    let t = match read_var(b, &mut p) {
        Ok(v) => v,
        Err(e) => return invalid(StandalonePatchFormat::Ups, e),
    };
    let mut records = 0;
    while p < end {
        if read_var(b, &mut p).is_err() {
            return invalid(StandalonePatchFormat::Ups, "truncated offset");
        }
        let mut terminated = false;
        while p < end {
            let x = b[p];
            p += 1;
            if x == 0 {
                terminated = true;
                break;
            }
        }
        if !terminated {
            return invalid(StandalonePatchFormat::Ups, "truncated XOR record");
        }
        records += 1;
        if records > MAX_RECORDS {
            return invalid(StandalonePatchFormat::Ups, "too many records");
        }
    }
    f.source_size = Some(s);
    f.target_size = Some(t);
    f.source_crc32 = Some(u32::from_le_bytes(b[end..end + 4].try_into().unwrap()));
    f.target_crc32 = Some(u32::from_le_bytes(b[end + 4..end + 8].try_into().unwrap()));
    f.patch_crc32 = Some(u32::from_le_bytes(b[end + 8..].try_into().unwrap()));
    if crc32(&b[..end + 8]) != f.patch_crc32.unwrap() {
        return invalid(StandalonePatchFormat::Ups, "patch CRC mismatch");
    }
    (PatchInspectionState::Valid, f)
}

fn is_safe_child(root: &Path, child: &Path) -> bool {
    if !child.is_absolute() || !root.is_absolute() {
        return false;
    }
    let Ok(rel) = child.strip_prefix(root) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return false;
    }
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        if !matches!(c, Component::Normal(_)) {
            return false;
        }
        cur.push(c);
        if cur
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return false;
        }
    }
    true
}
fn hex_digest(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}
fn crc32(b: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff;
    for &x in b {
        crc ^= x as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn var(mut n: u64) -> Vec<u8> {
        let mut o = Vec::new();
        loop {
            let x = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                o.push(x | 0x80);
                break;
            } else {
                o.push(x);
                n -= 1;
            }
        }
        o
    }
    fn bps(source: u64, target: u64, body: &[u8]) -> Vec<u8> {
        let mut b = b"BPS1".to_vec();
        b.extend(var(source));
        b.extend(var(target));
        b.push(0x80);
        b.extend(body);
        b.extend([0, 0, 0, 0]);
        b.extend([0, 0, 0, 0]);
        let c = crc32(&b);
        b.extend(c.to_le_bytes());
        b
    }
    fn temp_file(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(name);
        fs::File::create(&p).unwrap().write_all(bytes).unwrap();
        (d, p)
    }
    #[test]
    fn valid_bps_and_hash() {
        let body = var(1);
        let (_d, p) = temp_file("x.bps", &bps(1, 1, &body));
        let i = inspect_standalone_patch(p).unwrap();
        assert_eq!(i.format, StandalonePatchFormat::Bps);
        assert_eq!(i.state, PatchInspectionState::Valid);
        assert_eq!(i.source_size, Some(1));
    }
    #[test]
    fn malformed_and_extension_spoofing() {
        let (_d, p) = temp_file("x.bps", b"not-bps");
        let i = inspect_standalone_patch(p).unwrap();
        assert_eq!(i.format, StandalonePatchFormat::Unknown);
        let (_d, p) = temp_file("x.bin", b"BPS1");
        assert_eq!(
            inspect_standalone_patch(p).unwrap().state,
            PatchInspectionState::Invalid
        );
    }
    #[test]
    fn ips_and_ups_and_unknown() {
        let (_d, p) = temp_file("x.ips", b"PATCH\0\0\0\0\x01xEOF");
        assert_eq!(
            inspect_standalone_patch(p).unwrap().state,
            PatchInspectionState::Valid
        );
        let (_d, p) = temp_file("x.ups", b"UPS1\x80\x80\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(
            inspect_standalone_patch(p).unwrap().format,
            StandalonePatchFormat::Ups
        );
    }
    #[test]
    fn plan_is_safe_and_deterministic() {
        let body = var(1);
        let (_d, p) = temp_file("x.bps", &bps(1, 1, &body));
        let i = inspect_standalone_patch(&p).unwrap();
        let root = p.parent().unwrap();
        let base = root.join("base.rom");
        fs::write(&base, [7u8]).unwrap();
        let a = build_derived_patch_plan(&i, &base, root.join("out.rom"), root, None).unwrap();
        let b = build_derived_patch_plan(&i, &base, root.join("out.rom"), root, None).unwrap();
        assert_eq!(a, b);
        assert_eq!(fs::read(&base).unwrap(), [7]);
        assert!(matches!(
            build_derived_patch_plan(&i, &base, &base, root, None),
            Err(StandalonePatchError::SourceEqualsOutput)
        ));
    }
    #[test]
    fn unsafe_output_and_vcdiff_ppf() {
        let (_d, p) = temp_file("x", &[0xd6, 0xc4, 0xc3]);
        let i = inspect_standalone_patch(p).unwrap();
        assert_eq!(i.format, StandalonePatchFormat::XdeltaVcdiff);
        let (_d, p) = temp_file("x.ppf", b"PPF3.0");
        assert_eq!(
            inspect_standalone_patch(p).unwrap().format,
            StandalonePatchFormat::Ppf
        );
    }
}
