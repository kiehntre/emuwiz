//! Read-only projection over the historical Elkulator validators and main's
//! source binding. No preparation, executable selection, argv or spawn.
use super::ElkulatorMediaFormat;
use crate::disk_format::{DiskFormat, DiskFormatContext, DiskFormatMetadata, inspect_disk_format};
use crate::launch::native_support::{member, read_prefix};
use crate::launch::safe_launch_sandbox::{
    ConfigIsolation, LaunchMediaSafety, LaunchMediaSafetyDeclaration, MediaRole,
    PersistentStatePolicy, ProfileSeed, SandboxPlan, SourceProvenance, plan_sandbox,
};
use crate::safe_read::TrustedRoots;
use crate::tape_analysis::MAX_ANALYSIS_BYTES;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct MediaStructure {
    pub details: Vec<String>,
    pub warnings: Vec<String>,
    pub incomplete: bool,
}

#[derive(Debug, Clone)]
pub struct ElkulatorInspection {
    pub format: ElkulatorMediaFormat,
    pub source: SourceProvenance,
    pub structure: MediaStructure,
    // Never expose a plan that generic callers could prepare or execute.
    binding: SandboxPlan,
}
impl ElkulatorInspection {
    /// Background-thread operation: re-hashes the bound source read-only.
    pub fn is_fresh(&self) -> bool {
        self.binding.revalidate_sources().is_ok()
    }
}

pub fn format_from_path(path: &Path) -> ElkulatorMediaFormat {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "ssd" => ElkulatorMediaFormat::Ssd,
        "dsd" => ElkulatorMediaFormat::Dsd,
        "uef" => ElkulatorMediaFormat::Uef,
        _ => ElkulatorMediaFormat::Unknown,
    }
}

/// Inspection does not require an emulator or resolve Electron versus BBC.
/// `plan_sandbox` only binds/revalidates sources; no workspace is opened or
/// prepared, and its private prospective plan never leaves this module.
pub fn inspect_media(path: &Path) -> Result<ElkulatorInspection, String> {
    let format = format_from_path(path);
    let (kind, suffix) = format.plan_details().map_err(|e| e.to_string())?;
    let binding = plan_sandbox(
        LaunchMediaSafetyDeclaration {
            safety: LaunchMediaSafety::ScratchCopy,
            config_isolation: ConfigIsolation::None,
            persistent_state: PersistentStatePolicy::DisposableSession,
        },
        &[member(path, MediaRole::PrimaryMedia, kind, suffix)],
        ProfileSeed::Empty,
    )
    .map_err(|e| e.to_string())?;
    let source = binding.sources().next().expect("one media source").clone();
    let structure = inspect_structure(format, &source)?;
    binding
        .revalidate_sources()
        .map_err(|e| format!("Source changed during inspection: {e}"))?;
    Ok(ElkulatorInspection {
        format,
        source,
        structure,
        binding,
    })
}

pub(super) fn inspect_structure(
    format: ElkulatorMediaFormat,
    source: &SourceProvenance,
) -> Result<MediaStructure, String> {
    match format {
        ElkulatorMediaFormat::Uef => {
            if source.original_identity.size > MAX_ANALYSIS_BYTES as u64 {
                return Err("UEF exceeds the existing 8 MiB input limit".into());
            }
            let bytes = read_prefix(source, MAX_ANALYSIS_BYTES + 1).map_err(|e| e.to_string())?;
            let report = crate::uef_tape::parse_uef(&bytes)
                .map_err(|e| format!("Malformed or unsupported UEF: {e:?}"))?;
            let mut details: Vec<_> = report.metadata.into_iter().take(16).collect();
            details.push(format!(
                "UEF analysis: {} supported data entries listed; {} unsupported chunks",
                report.block_count, report.unsupported_blocks
            ));
            details.extend(report.semantic_blocks.into_iter().take(32));
            Ok(MediaStructure {
                incomplete: report.unsupported_blocks > 0
                    || report.block_count == 0
                    || !report.warnings.is_empty(),
                details,
                warnings: report.warnings.into_iter().take(32).collect(),
            })
        }
        ElkulatorMediaFormat::Ssd | ElkulatorMediaFormat::Dsd => {
            let evidence = inspect_disk_format(
                &source.original_path,
                &TrustedRoots::none(),
                DiskFormatContext {
                    folder_platform: None,
                },
                None,
            );
            if let Some(refusal) = evidence.refusal {
                return Err(format!("DFS inspection refused: {}", refusal.detail()));
            }
            let sides = if format == ElkulatorMediaFormat::Dsd {
                2
            } else {
                1
            };
            if evidence.format != Some(DiskFormat::AcornDfsDisk)
                || !matches!(evidence.metadata, Some(DiskFormatMetadata::Dfs(ref layout)) if layout.sides.len() == sides)
            {
                return Err("SSD/DSD declaration does not match validated DFS side layout".into());
            }
            Ok(MediaStructure {
                details: evidence.evidence,
                warnings: vec![],
                incomplete: false,
            })
        }
        _ => Err("Unsupported: only standard DFS SSD/DSD and UEF containers are inspected".into()),
    }
}
