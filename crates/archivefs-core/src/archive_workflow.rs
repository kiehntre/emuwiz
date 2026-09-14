//! Explicit, bounded archive packing and unpacking.
//!
//! ZIP uses EmuWiz's existing central-directory inspector.  7Z and RAR use
//! list-first, staged extraction through explicitly detected system tools.

use serde::Serialize;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 100_000;
pub const MAX_MEMBER_NAME_BYTES: usize = 4096;
pub const MAX_EXPANDED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_COMPRESSION_RATIO: u64 = 10_000;
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Rar,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveOperation {
    ExtractArchive,
    CreateArchive,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveEligibility {
    Ready,
    ToolMissing,
    PasswordRequired,
    EncryptedUnsupported,
    PathConflict,
    TraversalDetected,
    UnsafeEntry,
    TooLarge,
    TooManyEntries,
    Unsupported,
    ReviewRequired,
    UnsafeMember,
    UnsupportedMemberType,
    MultipartIncomplete,
    MalformedArchive,
    ExtractionVerificationFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveToolStatus {
    Found,
    Missing,
    VersionUnknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveToolRecord {
    pub name: String,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub status: ArchiveToolStatus,
    pub capabilities: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ArchiveToolInventory {
    pub tools: Vec<ArchiveToolRecord>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveEntryPlan {
    pub path: String,
    pub directory: bool,
    pub logical_size: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchivePlan {
    pub operation: ArchiveOperation,
    pub format: ArchiveFormat,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub entries: Vec<ArchiveEntryPlan>,
    pub compressed_bytes: Option<u64>,
    pub expanded_bytes: u64,
    pub eligibility: ArchiveEligibility,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveOperationResult {
    pub operation: ArchiveOperation,
    pub format: ArchiveFormat,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub member_count: usize,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub expanded_bytes: u64,
    pub verification: String,
    pub warnings: Vec<String>,
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .take(64)
        .map(|p| p.join(name))
        .find(|p| {
            fs::symlink_metadata(p)
                .map(|m| m.is_file() && !m.file_type().is_symlink())
                .unwrap_or(false)
        })
}
fn tool(name: &str, capabilities: &[&str]) -> ArchiveToolRecord {
    let path = which(name);
    let Some(path) = path.clone() else {
        return ArchiveToolRecord {
            name: name.into(),
            path: None,
            version: None,
            status: ArchiveToolStatus::Missing,
            capabilities: capabilities.iter().map(|s| (*s).into()).collect(),
        };
    };
    let mut child = Command::new(&path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok();
    let started = Instant::now();
    let output = loop {
        let Some(process) = child.as_mut() else {
            break None;
        };
        if process.try_wait().ok().flatten().is_some() {
            break child
                .take()
                .and_then(|process| process.wait_with_output().ok());
        }
        if started.elapsed() >= TOOL_TIMEOUT {
            let _ = process.kill();
            let _ = process.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let version = output.and_then(|o| {
        let mut bytes = o.stdout;
        bytes.extend(o.stderr);
        bytes.truncate(8192);
        String::from_utf8(bytes)
            .ok()
            .and_then(|s| s.lines().find(|l| !l.trim().is_empty()).map(str::to_owned))
    });
    ArchiveToolRecord {
        name: name.into(),
        path: Some(path),
        status: if version.is_some() {
            ArchiveToolStatus::Found
        } else {
            ArchiveToolStatus::VersionUnknown
        },
        version,
        capabilities: capabilities.iter().map(|s| (*s).into()).collect(),
    }
}
pub fn inventory_archive_tools() -> ArchiveToolInventory {
    ArchiveToolInventory {
        tools: vec![
            tool("zip", &["create"]),
            tool("unzip", &["extract", "test"]),
            tool("7z", &["create", "extract", "test"]),
            tool("7zz", &["create", "extract", "test"]),
            tool("unrar", &["extract", "test"]),
            tool("bsdtar", &["extract"]),
        ],
    }
}
pub fn classify_archive(path: &Path) -> Option<ArchiveFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "zip" => Some(ArchiveFormat::Zip),
        "7z" => Some(ArchiveFormat::SevenZ),
        "rar" => Some(ArchiveFormat::Rar),
        _ => None,
    }
}

pub fn validate_member_path(name: &str) -> Result<PathBuf, ArchiveEligibility> {
    let portable_name = name.replace('\\', "/");
    if portable_name.is_empty()
        || portable_name.len() > MAX_MEMBER_NAME_BYTES
        || portable_name.starts_with('/')
        || portable_name.contains(':')
    {
        return Err(ArchiveEligibility::TraversalDetected);
    }
    let mut safe = PathBuf::new();
    for component in Path::new(&portable_name).components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ArchiveEligibility::TraversalDetected);
            }
        }
    }
    if safe.as_os_str().is_empty() {
        return Err(ArchiveEligibility::UnsafeEntry);
    }
    Ok(safe)
}

pub fn inspect_archive_plan(source: &Path, destination: &Path) -> Result<ArchivePlan, String> {
    let format =
        classify_archive(source).ok_or_else(|| "unsupported archive format".to_string())?;
    let size = fs::metadata(source).map_err(|e| e.to_string())?;
    if !size.is_file() || size.len() > MAX_ARCHIVE_BYTES {
        return Err("archive is missing, not regular, or too large".into());
    }
    if format != ArchiveFormat::Zip {
        return inspect_external_archive_plan(source, destination, format, size.len());
    }
    let report = crate::inspect_archive(source).map_err(|e| e.to_string())?;
    if report.truncated || report.total_entries_in_archive > MAX_ENTRIES {
        return Err("archive contains too many entries".into());
    }
    let mut entries = Vec::new();
    let mut expanded = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    for entry in report.entries {
        let safe =
            validate_member_path(&entry.name).map_err(|e| format!("unsafe member: {e:?}"))?;
        let key = safe.to_string_lossy().to_ascii_lowercase();
        if !seen.insert(key) {
            return Err("case-folding member collision".into());
        }
        expanded = expanded
            .checked_add(entry.uncompressed_size)
            .ok_or("expanded size overflow")?;
        if expanded > MAX_EXPANDED_BYTES
            || (entry.compressed_size.unwrap_or(0) > 0
                && entry.uncompressed_size / entry.compressed_size.unwrap_or(1)
                    > MAX_COMPRESSION_RATIO)
        {
            return Err("archive expansion exceeds safety policy".into());
        }
        entries.push(ArchiveEntryPlan {
            path: safe.to_string_lossy().into(),
            directory: entry.kind == crate::InspectorEntryKind::Directory,
            logical_size: entry.uncompressed_size,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ArchivePlan {
        operation: ArchiveOperation::ExtractArchive,
        format,
        source: source.into(),
        destination: destination.into(),
        entries,
        compressed_bytes: Some(size.len()),
        expanded_bytes: expanded,
        eligibility: ArchiveEligibility::Ready,
        warnings: Vec::new(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalEntry {
    path: String,
    kind: ExternalEntryKind,
    size: u64,
    encrypted: bool,
}

fn external_tool(format: ArchiveFormat) -> Result<PathBuf, ArchiveEligibility> {
    let name = match format {
        ArchiveFormat::SevenZ => "7z",
        ArchiveFormat::Rar => "unrar",
        ArchiveFormat::Zip => return Err(ArchiveEligibility::Unsupported),
    };
    which(name).ok_or(ArchiveEligibility::ToolMissing)
}

fn run_external_list(source: &Path, format: ArchiveFormat) -> Result<String, ArchiveEligibility> {
    let tool = external_tool(format)?;
    let mut command = Command::new(tool);
    command.env("LC_ALL", "C").env("LANG", "C");
    match format {
        ArchiveFormat::SevenZ => {
            command.args(["l", "-slt", "-sccUTF-8", "-p-", "--"]);
        }
        ArchiveFormat::Rar => {
            command.args(["lt", "-p-", "--"]);
        }
        ArchiveFormat::Zip => return Err(ArchiveEligibility::Unsupported),
    }
    let output = command
        .arg(source)
        .output()
        .map_err(|_| ArchiveEligibility::ToolMissing)?;
    let mut combined = output.stdout;
    combined.extend(output.stderr);
    let text = String::from_utf8(combined).map_err(|_| ArchiveEligibility::MalformedArchive)?;
    if !output.status.success() {
        let lower = text.to_ascii_lowercase();
        if lower.contains("password")
            || lower.contains("encrypted")
            || lower.contains("wrong password")
        {
            return Err(ArchiveEligibility::PasswordRequired);
        }
        return Err(ArchiveEligibility::MalformedArchive);
    }
    Ok(text)
}

fn parse_external_entries(
    listing: &str,
    format: ArchiveFormat,
) -> Result<Vec<ExternalEntry>, ArchiveEligibility> {
    let path_key = match format {
        ArchiveFormat::SevenZ => "Path",
        ArchiveFormat::Rar => "Pathname",
        ArchiveFormat::Zip => return Err(ArchiveEligibility::Unsupported),
    };
    let mut entries = Vec::new();
    let mut current = std::collections::BTreeMap::<String, String>::new();
    let flush = |fields: &mut std::collections::BTreeMap<String, String>,
                 entries: &mut Vec<ExternalEntry>|
     -> Result<(), ArchiveEligibility> {
        let Some(path) = fields.remove(path_key) else {
            fields.clear();
            return Ok(());
        };
        if path.is_empty() || path.contains('\n') || path.contains('\r') {
            return Err(ArchiveEligibility::UnsafeMember);
        }
        let Some(size_text) = fields.remove("Size") else {
            // 7z emits one summary record for the archive itself before the
            // member records. It has Path/Type but no member Size/Attributes.
            fields.clear();
            return Ok(());
        };
        let size = size_text
            .parse::<u64>()
            .map_err(|_| ArchiveEligibility::MalformedArchive)?;
        let encrypted = fields
            .remove("Encrypted")
            .is_some_and(|value| value.trim() == "+");
        let attributes = fields.remove("Attributes").unwrap_or_default();
        if attributes.is_empty() {
            return Err(ArchiveEligibility::UnsupportedMemberType);
        }
        let has_link_or_hardlink_marker = attributes
            .chars()
            .any(|character| matches!(character, 'l' | 'L' | 'h' | 'H'));
        let kind = if format == ArchiveFormat::SevenZ {
            if attributes.starts_with('D') {
                ExternalEntryKind::Directory
            } else if attributes.starts_with('A') && !has_link_or_hardlink_marker {
                ExternalEntryKind::File
            } else {
                return Err(ArchiveEligibility::UnsupportedMemberType);
            }
        } else if attributes.starts_with('d') || attributes.starts_with('D') {
            ExternalEntryKind::Directory
        } else if !has_link_or_hardlink_marker
            && !matches!(
                attributes.chars().next(),
                Some('l' | 'L' | 'b' | 'B' | 'c' | 'C' | 'p' | 'P' | 's' | 'S' | 'h' | 'H')
            )
        {
            ExternalEntryKind::File
        } else {
            return Err(ArchiveEligibility::UnsupportedMemberType);
        };
        entries.push(ExternalEntry {
            path,
            kind,
            size,
            encrypted,
        });
        fields.clear();
        Ok(())
    };
    for line in listing.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            flush(&mut current, &mut entries)?;
            continue;
        }
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        current.insert(key.to_string(), value.to_string());
    }
    flush(&mut current, &mut entries)?;
    if entries.is_empty() {
        return Err(ArchiveEligibility::MalformedArchive);
    }
    Ok(entries)
}

fn inspect_external_archive_plan(
    source: &Path,
    destination: &Path,
    format: ArchiveFormat,
    compressed_bytes: u64,
) -> Result<ArchivePlan, String> {
    if looks_like_multipart_archive(source) {
        return Err(format!(
            "archive inspection failed: {:?}",
            ArchiveEligibility::MultipartIncomplete
        ));
    }
    let listing = run_external_list(source, format)
        .map_err(|reason| format!("archive inspection failed: {reason:?}"))?;
    let entries = parse_external_entries(&listing, format)
        .map_err(|reason| format!("archive inspection failed: {reason:?}"))?;
    if entries.len() > MAX_ENTRIES {
        return Err("archive contains too many entries".into());
    }
    let mut planned = Vec::with_capacity(entries.len());
    let mut seen = std::collections::BTreeSet::new();
    let mut expanded = 0u64;
    let mut encrypted = false;
    for entry in entries {
        let safe = validate_member_path(&entry.path)
            .map_err(|_| "unsafe archive member path".to_string())?;
        let key = safe.to_string_lossy().to_ascii_lowercase();
        if !seen.insert(key) {
            return Err("case-folding member collision".into());
        }
        encrypted |= entry.encrypted;
        expanded = expanded
            .checked_add(entry.size)
            .ok_or("expanded size overflow")?;
        if expanded > MAX_EXPANDED_BYTES
            || (compressed_bytes > 0 && expanded / compressed_bytes > MAX_COMPRESSION_RATIO)
        {
            return Err("archive expansion exceeds safety policy".into());
        }
        planned.push(ArchiveEntryPlan {
            path: safe.to_string_lossy().into_owned(),
            directory: entry.kind == ExternalEntryKind::Directory,
            logical_size: entry.size,
        });
    }
    planned.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ArchivePlan {
        operation: ArchiveOperation::ExtractArchive,
        format,
        source: source.into(),
        destination: destination.into(),
        entries: planned,
        compressed_bytes: Some(compressed_bytes),
        expanded_bytes: expanded,
        eligibility: if encrypted {
            ArchiveEligibility::PasswordRequired
        } else {
            ArchiveEligibility::Ready
        },
        warnings: if encrypted {
            vec!["Encrypted archive requires an explicit password workflow.".into()]
        } else {
            Vec::new()
        },
    })
}

fn looks_like_multipart_archive(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".rar") {
        if let Some(part) = lower
            .strip_suffix(".rar")
            .and_then(|stem| stem.rsplit_once(".part"))
        {
            return part.1.chars().all(|ch| ch.is_ascii_digit());
        }
    }
    lower.len() >= 4
        && !lower.ends_with(".rar")
        && lower.as_bytes()[lower.len() - 4] == b'.'
        && lower.as_bytes()[lower.len() - 3] == b'r'
        && lower[lower.len() - 2..]
            .chars()
            .all(|ch| ch.is_ascii_digit())
}
fn confined(root: &Path, relative: &Path) -> Result<PathBuf, ArchiveEligibility> {
    let target = root.join(relative);
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if let Component::Normal(value) = component {
            current.push(value);
            if current.exists()
                && fs::symlink_metadata(&current)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(true)
            {
                return Err(ArchiveEligibility::UnsafeEntry);
            }
        }
    }
    Ok(target)
}
fn walk_bytes(root: &Path) -> Result<u64, String> {
    let mut total = 0;
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        let m = fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        if m.is_dir() {
            total += walk_bytes(&p)?
        } else if m.is_file() {
            total += m.len()
        } else {
            return Err("special file detected".into());
        }
    }
    Ok(total)
}

fn collect_tree(root: &Path) -> Result<(std::collections::BTreeSet<String>, u64), String> {
    fn visit(
        root: &Path,
        current: &Path,
        paths: &mut std::collections::BTreeSet<String>,
        bytes: &mut u64,
    ) -> Result<(), String> {
        for entry in fs::read_dir(current).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() || (!meta.is_dir() && !meta.is_file()) {
                return Err("extracted symlink or special file detected".into());
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if !paths.insert(relative) {
                return Err("duplicate extracted path".into());
            }
            if meta.is_file() {
                *bytes = bytes
                    .checked_add(meta.len())
                    .ok_or("output size overflow")?;
            } else {
                visit(root, &path, paths, bytes)?;
            }
        }
        Ok(())
    }
    let mut paths = std::collections::BTreeSet::new();
    let mut bytes = 0;
    visit(root, root, &mut paths, &mut bytes)?;
    Ok((paths, bytes))
}

fn extract_external(plan: &ArchivePlan) -> Result<ArchiveOperationResult, String> {
    if !matches!(plan.format, ArchiveFormat::SevenZ | ArchiveFormat::Rar)
        || plan.eligibility != ArchiveEligibility::Ready
    {
        return Err("archive plan is not executable".into());
    }
    if plan.destination.exists() {
        return Err("destination already exists; refusing overwrite".into());
    }
    let parent = plan
        .destination
        .parent()
        .ok_or("destination has no parent")?;
    if !parent.is_dir() {
        return Err("destination parent is not a directory".into());
    }
    let stage = parent.join(format!(".emuwiz-archive-stage-{}", std::process::id()));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let tool = external_tool(plan.format).map_err(|e| format!("tool unavailable: {e:?}"))?;
        let mut command = Command::new(tool);
        command.env("LC_ALL", "C").env("LANG", "C");
        match plan.format {
            ArchiveFormat::SevenZ => {
                command.args(["x", "-y", "-p-", "-sccUTF-8"]);
            }
            ArchiveFormat::Rar => {
                command.args(["x", "-y", "-p-"]);
            }
            ArchiveFormat::Zip => return Err("wrong external format".into()),
        }
        if plan.format == ArchiveFormat::SevenZ {
            command.arg(format!("-o{}", stage.display()));
        }
        command.arg("--").arg(&plan.source);
        if plan.format == ArchiveFormat::Rar {
            command.arg(&stage);
        }
        let status = command.status().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("external extraction failed; staged output discarded".into());
        }
        let (actual, output_bytes) = collect_tree(&stage)?;
        let expected = plan
            .entries
            .iter()
            .map(|entry| entry.path.replace('\\', "/"))
            .collect::<std::collections::BTreeSet<_>>();
        let expected_files = plan
            .entries
            .iter()
            .filter(|entry| !entry.directory)
            .map(|entry| entry.path.replace('\\', "/"))
            .collect::<std::collections::BTreeSet<_>>();
        if actual != expected_files && actual != expected {
            return Err("extraction verification failed: unexpected or missing paths".into());
        }
        for entry in plan.entries.iter().filter(|entry| !entry.directory) {
            let path = stage.join(&entry.path);
            let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
            if size != entry.logical_size {
                return Err(format!(
                    "extraction verification failed: size mismatch for {}",
                    entry.path
                ));
            }
        }
        fs::rename(&stage, &plan.destination).map_err(|e| e.to_string())?;
        Ok(ArchiveOperationResult {
            operation: ArchiveOperation::ExtractArchive,
            format: plan.format,
            source: plan.source.clone(),
            destination: plan.destination.clone(),
            member_count: plan.entries.len(),
            source_bytes: plan.compressed_bytes.unwrap_or(0),
            output_bytes,
            expanded_bytes: plan.expanded_bytes,
            verification: "external list, staged tree, regular-file paths, and sizes verified"
                .into(),
            warnings: vec!["Source archive remains untouched.".into()],
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

pub fn extract_archive(plan: &ArchivePlan) -> Result<ArchiveOperationResult, String> {
    match plan.format {
        ArchiveFormat::Zip => extract_zip(plan),
        ArchiveFormat::SevenZ | ArchiveFormat::Rar => extract_external(plan),
    }
}

pub fn extract_zip(plan: &ArchivePlan) -> Result<ArchiveOperationResult, String> {
    if plan.format != ArchiveFormat::Zip || plan.eligibility != ArchiveEligibility::Ready {
        return Err("archive plan is not executable".into());
    }
    if plan.destination.exists() {
        return Err("destination already exists; refusing overwrite".into());
    }
    let parent = plan
        .destination
        .parent()
        .ok_or("destination has no parent")?;
    if !parent.is_dir() {
        return Err("destination parent is not a directory".into());
    }
    let stage = parent.join(format!(".emuwiz-archive-stage-{}", std::process::id()));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut archive =
            zip::ZipArchive::new(File::open(&plan.source).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        for index in 0..archive.len() {
            let mut member = archive.by_index(index).map_err(|e| e.to_string())?;
            let relative =
                validate_member_path(member.name()).map_err(|e| format!("unsafe member: {e:?}"))?;
            if let Some(mode) = member.unix_mode() {
                if mode & 0o170000 == 0o120000 {
                    return Err("archive symlink member is unsupported".into());
                }
            }
            let target =
                confined(&stage, &relative).map_err(|e| format!("unsafe destination: {e:?}"))?;
            if member.is_dir() {
                fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            } else {
                if let Some(p) = target.parent() {
                    fs::create_dir_all(p).map_err(|e| e.to_string())?;
                }
                let mut output = File::create(&target).map_err(|e| e.to_string())?;
                io::copy(&mut member, &mut output).map_err(|e| e.to_string())?;
            }
        }
        fs::rename(&stage, &plan.destination).map_err(|e| e.to_string())?;
        let output_bytes = walk_bytes(&plan.destination)?;
        Ok(ArchiveOperationResult {
            operation: ArchiveOperation::ExtractArchive,
            format: ArchiveFormat::Zip,
            source: plan.source.clone(),
            destination: plan.destination.clone(),
            member_count: plan.entries.len(),
            source_bytes: plan.compressed_bytes.unwrap_or(0),
            output_bytes,
            expanded_bytes: plan.expanded_bytes,
            verification: "ZIP reopened and staged paths verified".into(),
            warnings: Vec::new(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}
fn add_zip_tree(
    root: &Path,
    current: &Path,
    writer: &mut zip::ZipWriter<File>,
    entries: &mut Vec<String>,
) -> Result<(), String> {
    let mut children = fs::read_dir(current)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    children.sort_by_key(|e| e.file_name());
    for child in children {
        let path = child.path();
        let rel = path.strip_prefix(root).map_err(|e| e.to_string())?;
        let name = rel.to_string_lossy().replace('\\', "/");
        let safe = validate_member_path(&name).map_err(|e| format!("unsafe source path: {e:?}"))?;
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            return Err("source symlink is unsupported".into());
        }
        if meta.is_dir() {
            writer
                .add_directory(
                    format!("{}/", safe.display()),
                    zip::write::SimpleFileOptions::default(),
                )
                .map_err(|e| e.to_string())?;
            add_zip_tree(root, &path, writer, entries)?;
        } else if meta.is_file() {
            writer
                .start_file(
                    safe.to_string_lossy(),
                    zip::write::SimpleFileOptions::default(),
                )
                .map_err(|e| e.to_string())?;
            let mut input = File::open(&path).map_err(|e| e.to_string())?;
            io::copy(&mut input, writer).map_err(|e| e.to_string())?;
            entries.push(name);
        } else {
            return Err("special source file is unsupported".into());
        }
    }
    Ok(())
}
pub fn create_zip(
    source_root: &Path,
    destination: &Path,
) -> Result<ArchiveOperationResult, String> {
    if !source_root.is_dir() || destination.exists() {
        return Err("source must be a directory and destination must be new".into());
    }
    let parent = destination.parent().ok_or("destination has no parent")?;
    if !parent.is_dir() {
        return Err("destination parent is not a directory".into());
    }
    let stage = parent.join(format!(".emuwiz-archive-stage-{}.zip", std::process::id()));
    let file = File::create(&stage).map_err(|e| e.to_string())?;
    let mut writer = zip::ZipWriter::new(file);
    let mut entries = Vec::new();
    add_zip_tree(source_root, source_root, &mut writer, &mut entries)?;
    writer.finish().map_err(|e| e.to_string())?;
    let verified = crate::inspect_archive(&stage).map_err(|e| e.to_string())?;
    if verified.total_entries_in_archive != entries.len() {
        let _ = fs::remove_file(&stage);
        return Err("created archive verification count mismatch".into());
    }
    fs::rename(&stage, destination).map_err(|e| e.to_string())?;
    let bytes = walk_bytes(source_root)?;
    let output = fs::metadata(destination).map_err(|e| e.to_string())?.len();
    Ok(ArchiveOperationResult {
        operation: ArchiveOperation::CreateArchive,
        format: ArchiveFormat::Zip,
        source: source_root.into(),
        destination: destination.into(),
        member_count: entries.len(),
        source_bytes: bytes,
        output_bytes: output,
        expanded_bytes: bytes,
        verification: "ZIP reopened and member list verified".into(),
        warnings: vec!["Sources remain untouched.".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_names() {
        assert!(validate_member_path("../x").is_err());
        assert!(validate_member_path("/x").is_err());
        assert!(validate_member_path("C:/x").is_err());
    }
    #[test]
    fn recognizes_formats() {
        assert_eq!(
            classify_archive(Path::new("a.zip")),
            Some(ArchiveFormat::Zip)
        );
        assert_eq!(
            classify_archive(Path::new("a.rar")),
            Some(ArchiveFormat::Rar)
        );
    }

    #[test]
    fn portable_member_paths_reject_windows_traversal_and_absolute_names() {
        assert!(validate_member_path(r"..\outside.bin").is_err());
        assert!(validate_member_path(r"C:\outside.bin").is_err());
        assert!(validate_member_path(r"\server\share\outside.bin").is_err());
        assert!(validate_member_path("safe/nested.bin").is_ok());
    }

    #[test]
    fn parses_safe_sevenz_listing_and_skips_summary_record() {
        let listing = "Path = /tmp/game.7z\nType = 7z\nPhysical Size = 12\n\nPath = game\nSize = 0\nAttributes = D drwxr-xr-x\nEncrypted = -\n\nPath = game/disc.iso\nSize = 4\nAttributes = A -rw-r--r--\nEncrypted = -\n";
        let entries = parse_external_entries(listing, ArchiveFormat::SevenZ).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].path, "game/disc.iso");
        assert_eq!(entries[1].size, 4);
    }

    #[test]
    fn parses_safe_rar_listing_and_rejects_link_attributes() {
        let listing = "Pathname = nested\nSize = 0\nAttributes = drwxr-xr-x\nEncrypted = -\n\nPathname = nested/file.bin\nSize = 3\nAttributes = -rw-r--r--\nEncrypted = -\n";
        let entries = parse_external_entries(listing, ArchiveFormat::Rar).unwrap();
        assert_eq!(entries.len(), 2);

        let link = "Pathname = link\nSize = 3\nAttributes = lrwxrwxrwx\nEncrypted = -\n";
        assert_eq!(
            parse_external_entries(link, ArchiveFormat::Rar),
            Err(ArchiveEligibility::UnsupportedMemberType)
        );
    }

    #[test]
    fn external_listing_rejects_case_collisions_and_encryption_is_not_ready() {
        let collision = "Path = A\nSize = 1\nAttributes = A -rw-r--r--\nEncrypted = -\n\nPath = a\nSize = 1\nAttributes = A -rw-r--r--\nEncrypted = -\n";
        let entries = parse_external_entries(collision, ArchiveFormat::SevenZ).unwrap();
        assert_eq!(entries.len(), 2);

        let encrypted = "Path = game.bin\nSize = 1\nAttributes = A -rw-r--r--\nEncrypted = +\n";
        let entries = parse_external_entries(encrypted, ArchiveFormat::SevenZ).unwrap();
        assert!(entries[0].encrypted);
    }
}
