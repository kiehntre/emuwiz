use std::fs;
use std::path::{Component, Path};

use crate::source_root_migration::{MigrationClassification, MigrationProposal};

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupPathReview {
    pub reference: SetupPath,
    /// Reuses the migration planner's evidence vocabulary. Complete-field
    /// replacement is distinct from root rebasing: it never grants a migration
    /// or apply authority, even when the selected location exists.
    pub proposal: MigrationProposal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupImportPreview {
    pub paths: Vec<SetupPathReview>,
    pub reusable_settings: Vec<String>,
    pub missing_emulators: Vec<String>,
    pub attention: Vec<String>,
    pub not_included: Vec<String>,
    pub read_only: bool,
}

/// Uses only metadata for explicitly chosen local locations. Imported paths
/// are never searched, executed or opened for content. Even a matching source
/// path must be explicitly confirmed before a local existence check occurs.
pub fn preview_setup_import(
    manifest: &SetupManifest,
    remaps: &SetupPathRemaps,
) -> Result<SetupImportPreview, String> {
    manifest.validate()?;
    let references = manifest.paths();
    if remaps
        .keys()
        .any(|key| !references.iter().any(|reference| &reference.id == key))
    {
        return Err(
            "A location choice no longer belongs to this setup file. Preview again.".into(),
        );
    }
    let mut preview = SetupImportPreview {
        paths: Vec::new(), reusable_settings: Vec::new(), missing_emulators: Vec::new(),
        attention: vec!["Preview only. No settings will be applied, and no emulator configuration will be overwritten.".into()],
        not_included: vec!["ROMs, credentials, saves, emulator configuration bytes and recovery history are not carried by this format.".into()],
        read_only: true,
    };
    if manifest.source_os != std::env::consts::OS {
        preview.attention.push("This setup comes from another operating system. Choose local locations and check emulator availability again.".into());
    }
    if let Some(policy) = &manifest.dat_policy {
        preview
            .reusable_settings
            .push("DAT matching preferences (including platform overrides)".into());
        if !crate::dat::policy::validate_policy_config(policy).is_empty() {
            preview.attention.push("Some DAT matching preferences are not recognised by this version. Review them in DAT settings.".into());
        }
    }
    if !manifest.library.sources.is_empty() {
        preview.reusable_settings.push(format!(
            "{} game-source enable/disable choices",
            manifest.library.sources.len()
        ));
    }
    if !manifest.dat_sources.is_empty() {
        preview.reusable_settings.push(format!(
            "{} DAT registrations and priority choices",
            manifest.dat_sources.len()
        ));
    }
    if let Some(binary) = &manifest.library.ratarmount_bin {
        if !Path::new(binary).is_absolute() && !binary.contains(['/', '\\']) {
            preview.attention.push("The archive mount tool uses a command name. Setup must resolve it on the destination; it has not been executed.".into());
        }
    }
    if manifest
        .dat_sources
        .iter()
        .any(|source| !source.ownership.is_user_local())
    {
        preview.attention.push("Managed/imported DAT registrations need their original source packs on this device. Exported ownership does not authorise replacement or updates.".into());
    }
    if manifest.romm.is_some() {
        preview
            .reusable_settings
            .push("RomM enablement, paging preferences and declared mapping style".into());
        preview.attention.push("Re-enter RomM credentials and the full server address. Review folder mappings before explicitly testing a connection.".into());
    }
    for emulator in &manifest.emulators {
        if emulator.executable.is_none() {
            preview.attention.push(format!("{}: configuration location only. Detect or select the emulator in Setup; availability has not been checked.", emulator.emulator.label()));
        }
    }
    for reference in references {
        let candidate = remaps.get(&reference.id).cloned();
        let (classification, reason) = match candidate.as_deref() {
            None => (
                MigrationClassification::ManualReview,
                "Choose a location on this device, or confirm the original location.",
            ),
            Some(path) if !plain_absolute(path) => (
                MigrationClassification::ManualReview,
                "Choose an absolute local location without parent traversal.",
            ),
            Some(path) => match metadata_without_links(path) {
                Ok(metadata)
                    if match reference.kind {
                        SetupPathKind::Directory => metadata.is_dir(),
                        SetupPathKind::File => metadata.is_file(),
                        SetupPathKind::Executable => executable_file(&metadata),
                    } =>
                {
                    (
                        MigrationClassification::AlreadyCurrent,
                        "The selected location exists with the expected file type. Contents and compatibility have not been verified.",
                    )
                }
                Ok(_) => (
                    MigrationClassification::ManualReview,
                    "The selected location has the wrong file type or is not executable.",
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                    MigrationClassification::TargetMissing,
                    "The selected location is missing. Choose another location or set it up separately.",
                ),
                Err(_) => (
                    MigrationClassification::ManualReview,
                    "The selected location could not be checked safely; links and inaccessible locations need review.",
                ),
            },
        };
        if reference.kind == SetupPathKind::Executable
            && classification == MigrationClassification::TargetMissing
        {
            preview.missing_emulators.push(format!(
                "{} is missing at the selected location.",
                reference.label
            ));
        }
        let proposal = MigrationProposal {
            reference_id: reference.id.clone(),
            subsystem: "setup_import_preview".into(),
            old_path: reference.path.clone(),
            candidate_path: candidate,
            classification,
            reason: reason.into(),
        };
        preview.paths.push(SetupPathReview {
            reference,
            proposal,
        });
    }
    for notice in &manifest.notices {
        match notice.coverage {
            SetupCoverage::Included => {}
            SetupCoverage::RequiresAttention => preview.attention.push(notice.message.clone()),
            SetupCoverage::NotIncluded => preview.not_included.push(notice.message.clone()),
        }
    }
    Ok(preview)
}

fn plain_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

/// Check each component; a selected path may not use an intermediate symlink
/// to turn a location-only check into a probe of a different directory.
fn metadata_without_links(path: &Path) -> std::io::Result<fs::Metadata> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&prefix)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "link requires review",
            ));
        }
    }
    fs::symlink_metadata(path)
}

fn executable_file(metadata: &fs::Metadata) -> bool {
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
