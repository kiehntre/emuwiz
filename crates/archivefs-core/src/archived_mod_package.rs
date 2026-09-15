//! List-first, read-only inspection of archived mod packages.
//!
//! Archive safety belongs to [`crate::archive_workflow`].  This module only
//! projects its validated member list into the existing standalone-patch and
//! local-mod vocabulary; it never extracts into an emulator directory or
//! applies a patch.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::archive_workflow::{self, ArchiveEligibility, ArchiveFormat, ArchivePlan};
use crate::standalone_patch::{StandalonePatchInspection, inspect_standalone_patch};

pub const MAX_MOD_README_BYTES: u64 = 256 * 1024;
pub const MAX_MOD_MANIFEST_BYTES: u64 = 256 * 1024;
pub const MAX_MOD_PATCHES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchivedModMemberRole {
    Patch,
    Manifest,
    Readme,
    Documentation,
    Texture,
    Config,
    Model,
    Audio,
    Script,
    Executable,
    Archive,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchivedPatchSelection {
    OnePatch,
    MultiplePatchesReviewRequired,
    NoPatch,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArchivedModMember {
    pub path: String,
    pub size: u64,
    pub role: ArchivedModMemberRole,
    pub provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArchivedModPackageInspection {
    pub package_path: PathBuf,
    pub package_sha256: String,
    pub archive_format: ArchiveFormat,
    pub archive_eligibility: ArchiveEligibility,
    pub members: Vec<ArchivedModMember>,
    pub patch_selection: ArchivedPatchSelection,
    pub patches: Vec<StandalonePatchInspection>,
    pub title: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub warnings: Vec<String>,
    pub no_changes_made: bool,
}

pub fn inspect_archived_mod_package(
    path: impl AsRef<Path>,
) -> Result<ArchivedModPackageInspection, String> {
    let path = path.as_ref().to_path_buf();
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    let package_sha256 = hex_digest(&bytes);
    let plan =
        archive_workflow::inspect_archive_plan(&path, &path.with_extension("emuwiz-inspection"))?;
    let mut members = plan
        .entries
        .iter()
        .map(|entry| ArchivedModMember {
            path: entry.path.clone(),
            size: entry.logical_size,
            role: role_for(&entry.path),
            provenance: "local archive member listing".into(),
        })
        .collect::<Vec<_>>();
    members.sort_by(|a, b| a.path.cmp(&b.path));
    let patch_paths = members
        .iter()
        .filter(|m| m.role == ArchivedModMemberRole::Patch)
        .map(|m| m.path.clone())
        .collect::<Vec<_>>();
    let patch_selection = match patch_paths.len() {
        0 => ArchivedPatchSelection::NoPatch,
        1 => ArchivedPatchSelection::OnePatch,
        _ => ArchivedPatchSelection::MultiplePatchesReviewRequired,
    };
    let mut warnings = plan.warnings.clone();
    if plan.eligibility != ArchiveEligibility::Ready {
        warnings.push(format!(
            "archive is not ready for member inspection: {:?}",
            plan.eligibility
        ));
    }
    let mut patches = Vec::new();
    if plan.format == ArchiveFormat::Zip && plan.eligibility == ArchiveEligibility::Ready {
        let temp = std::env::temp_dir().join(format!("emuwiz-mod-inspect-{}", std::process::id()));
        fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
        let result = inspect_zip_patches(&path, &patch_paths, &temp, &mut patches);
        let _ = fs::remove_dir_all(&temp);
        result?;
    } else if !patch_paths.is_empty() {
        warnings.push("patch members are listed but their bytes were not extracted by the external archive adapter".into());
    }
    let (title, version, author) = read_readme_hints(&path, &plan)?;
    Ok(ArchivedModPackageInspection {
        package_path: path,
        package_sha256,
        archive_format: plan.format,
        archive_eligibility: plan.eligibility,
        members,
        patch_selection,
        patches,
        title,
        version,
        author,
        warnings,
        no_changes_made: true,
    })
}

fn inspect_zip_patches(
    path: &Path,
    names: &[String],
    temp: &Path,
    out: &mut Vec<StandalonePatchInspection>,
) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    for (index, name) in names.iter().enumerate().take(MAX_MOD_PATCHES) {
        let mut member = archive.by_name(name).map_err(|e| e.to_string())?;
        if member.size() > crate::standalone_patch::MAX_PATCH_BYTES as u64 {
            return Err("patch member exceeds inspection limit".into());
        }
        let destination = temp.join(format!("patch-{index}"));
        let mut bytes = Vec::with_capacity(member.size().min(8 * 1024 * 1024) as usize);
        member.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        fs::write(&destination, &bytes).map_err(|e| e.to_string())?;
        out.push(inspect_standalone_patch(&destination).map_err(|e| e.to_string())?);
    }
    Ok(())
}

fn read_readme_hints(
    path: &Path,
    plan: &ArchivePlan,
) -> Result<(Option<String>, Option<String>, Option<String>), String> {
    if plan.format != ArchiveFormat::Zip {
        return Ok((None, None, None));
    }
    let Some(name) = plan.entries.iter().map(|e| e.path.as_str()).find(|name| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "readme" | "readme.txt" | "readme.md" | "install.txt" | "changelog"
        )
    }) else {
        return Ok((None, None, None));
    };
    let mut archive = zip::ZipArchive::new(fs::File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut member = archive.by_name(name).map_err(|e| e.to_string())?;
    if member.size() > MAX_MOD_README_BYTES {
        return Ok((None, None, None));
    }
    let mut bytes = Vec::new();
    member.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let title = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|s| s.trim().trim_matches('#').trim().to_string());
    Ok((title, None, None))
}

fn role_for(path: &str) -> ArchivedModMemberRole {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".ips")
        || lower.ends_with(".bps")
        || lower.ends_with(".ups")
        || lower.ends_with(".xdelta")
        || lower.ends_with(".vcdiff")
        || lower.ends_with(".ppf")
    {
        return ArchivedModMemberRole::Patch;
    }
    if lower.ends_with(".json") || lower.ends_with(".toml") {
        return ArchivedModMemberRole::Manifest;
    }
    if matches!(
        lower.rsplit('/').next().unwrap_or_default(),
        "readme" | "readme.txt" | "readme.md" | "install.txt" | "changelog"
    ) {
        return ArchivedModMemberRole::Readme;
    }
    if lower.ends_with(".exe")
        || lower.ends_with(".dll")
        || lower.ends_with(".bat")
        || lower.ends_with(".cmd")
        || lower.ends_with(".ps1")
        || lower.ends_with(".sh")
        || lower.ends_with(".py")
        || lower.ends_with(".so")
        || lower.ends_with(".appimage")
    {
        return ArchivedModMemberRole::Executable;
    }
    if lower.ends_with(".zip") || lower.ends_with(".7z") || lower.ends_with(".rar") {
        return ArchivedModMemberRole::Archive;
    }
    if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".dds") {
        return ArchivedModMemberRole::Texture;
    }
    if lower.ends_with(".ini")
        || lower.ends_with(".cfg")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
    {
        return ArchivedModMemberRole::Config;
    }
    if lower.ends_with(".md") || lower.ends_with(".txt") || lower.ends_with(".nfo") {
        return ArchivedModMemberRole::Documentation;
    }
    ArchivedModMemberRole::Unknown
}
fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
