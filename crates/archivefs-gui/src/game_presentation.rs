use std::path::PathBuf;

use archivefs_core::{
    ArchiveKind, ArchiveRecord, CUSTOM_FOLDER_ALIAS_SOURCE, DAT_ROMM_AGREEMENT_SOURCE,
    MANUAL_PLATFORM_SOURCE, MountState, PersistedArchive, PlatformProvenanceDetails,
    ROMM_PLATFORM_SOURCE, VERIFIED_DAT_PLATFORM_SOURCE, game_identity::IdentityStatus,
};

use crate::LibraryRowFilters;
use crate::status_wording::{PlainStatus, StatusContext, plain_status};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathAvailability {
    Available(PathBuf),
    Missing(PathBuf),
    Unavailable,
}

impl PathAvailability {
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Available(path) | Self::Missing(path) => Some(path),
            Self::Unavailable => None,
        }
    }
}

/// Raw values intended for Advanced View diagnostics. Beginner wording is
/// stored separately in `SelectedGamePresentation::status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameTechnicalStatus {
    pub archive_kind: String,
    pub mount_state: Option<MountState>,
    pub identity_status: Option<IdentityStatus>,
    pub health: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedGamePresentation {
    pub title: String,
    pub platform: String,
    pub format: String,
    pub path: PathAvailability,
    pub mounting_applies: bool,
    pub direct_use_applies: bool,
    pub cheats_mods_available: bool,
    pub undo_available: bool,
    pub status: PlainStatus,
    pub technical_status: GameTechnicalStatus,
}

impl SelectedGamePresentation {
    /// Builds presentation data from the live `ArchiveRecord` already chosen
    /// by the GUI. Capability booleans are accepted from their existing
    /// backend gates so this layer never reimplements provider or history
    /// policy.
    pub fn from_live(
        record: &ArchiveRecord,
        identity_status: Option<IdentityStatus>,
        cheats_mods_available: bool,
        undo_available: bool,
    ) -> Self {
        let kind = record.mount_plan.archive.kind;
        let mounting_applies = kind.is_mount_input();
        let path = PathAvailability::Available(record.mount_plan.archive.path.clone());
        let status = plain_status(StatusContext {
            path_available: true,
            mounting_applies,
            mount_state: Some(record.mount_state),
            identity_status,
        });
        Self {
            title: record
                .metadata
                .title
                .clone()
                .unwrap_or_else(|| record.identity.display_name.clone()),
            platform: record
                .metadata
                .platform
                .clone()
                .or_else(|| record.identity.platform.clone())
                .unwrap_or_else(|| "Unknown platform".to_string()),
            format: archive_kind_label(kind).to_string(),
            path,
            mounting_applies,
            direct_use_applies: !mounting_applies,
            cheats_mods_available,
            undo_available,
            status,
            technical_status: GameTechnicalStatus {
                archive_kind: format!("{kind:?}"),
                mount_state: Some(record.mount_state),
                identity_status,
                health: Some(record.health.to_string()),
            },
        }
    }

    /// Cache-only counterpart for a selected catalogue row. It deliberately
    /// exposes missing/unavailable paths and never claims a mount state that
    /// the live snapshot did not provide.
    pub fn from_cached(
        archive: &PersistedArchive,
        identity_status: Option<IdentityStatus>,
        cheats_mods_available: bool,
        undo_available: bool,
    ) -> Self {
        let missing = archive.last_verified_missing_at.is_some();
        let path = if missing {
            PathAvailability::Missing(archive.absolute_path.clone())
        } else {
            PathAvailability::Unavailable
        };
        let mounting_applies = persisted_kind_mounts(&archive.archive_kind);
        let status = plain_status(StatusContext {
            path_available: false,
            mounting_applies,
            mount_state: None,
            identity_status,
        });
        Self {
            title: archive.display_name.clone(),
            platform: archive
                .platform
                .clone()
                .unwrap_or_else(|| "Unknown platform".to_string()),
            format: persisted_kind_label(&archive.archive_kind).to_string(),
            path,
            mounting_applies,
            direct_use_applies: !mounting_applies,
            cheats_mods_available,
            undo_available,
            status,
            technical_status: GameTechnicalStatus {
                archive_kind: archive.archive_kind.clone(),
                mount_state: None,
                identity_status,
                health: Some(archive.last_known_health.clone()),
            },
        }
    }
}

pub const fn archive_kind_label(kind: ArchiveKind) -> &'static str {
    match kind {
        ArchiveKind::Zip => "ZIP archive",
        ArchiveKind::SevenZip => "7z archive",
        ArchiveKind::Rar => "RAR archive",
        ArchiveKind::MegaDriveRom => "Mega Drive ROM",
        ArchiveKind::DirectGameImage => "Game image",
    }
}

fn persisted_kind_mounts(kind: &str) -> bool {
    !matches!(kind, "megadrive_rom" | "direct_game_image")
}

fn persisted_kind_label(kind: &str) -> &str {
    match kind {
        "zip" => "ZIP archive",
        "sevenzip" => "7z archive",
        "rar" => "RAR archive",
        "megadrive_rom" => "Mega Drive ROM",
        "direct_game_image" => "Game image",
        _ => kind,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use archivefs_core::{
        Archive, ArchiveHealth, ArchiveMetadata, ArchiveRecord, MountPlan, MountState,
    };

    use super::*;

    fn live_record(path: &str, mount_state: MountState) -> ArchiveRecord {
        let archive = Archive::from_path(Path::new(path)).expect("supported fixture extension");
        let plan = MountPlan::new(archive, PathBuf::from("/mnt/game"));
        let metadata = ArchiveMetadata {
            title: Some("Friendly Title".to_string()),
            platform: Some("GameCube".to_string()),
            region: None,
            languages: None,
            version: None,
            disc: None,
            publisher: None,
            developer: None,
            release_year: None,
            genre: None,
            notes: None,
            source: None,
            synopsis: None,
            players: None,
            rating: None,
        };
        ArchiveRecord::new(plan, mount_state, metadata, ArchiveHealth::Pending)
    }

    fn cached_archive(missing: bool) -> PersistedArchive {
        PersistedArchive {
            id: 1,
            source_folder_id: 2,
            relative_path: PathBuf::from("Game.rvz"),
            absolute_path: PathBuf::from("/games/Game.rvz"),
            archive_kind: "direct_game_image".to_string(),
            display_name: "Game".to_string(),
            normalized_name: "game".to_string(),
            size_bytes: Some(10),
            modified_time_unix_seconds: Some(20),
            platform: Some("Wii".to_string()),
            platform_source: Some("header-identity".to_string()),
            last_known_health: "present".to_string(),
            last_seen_at: "now".to_string(),
            last_verified_missing_at: missing.then(|| "later".to_string()),
            identity_report: None,
        }
    }

    #[test]
    fn live_mountable_game_uses_existing_record_fields() {
        let model = SelectedGamePresentation::from_live(
            &live_record("/games/Game.zip", MountState::Pending),
            Some(IdentityStatus::Verified),
            true,
            false,
        );
        assert_eq!(model.title, "Friendly Title");
        assert_eq!(model.platform, "GameCube");
        assert_eq!(model.format, "ZIP archive");
        assert!(model.mounting_applies);
        assert!(!model.direct_use_applies);
        assert!(model.cheats_mods_available);
        assert_eq!(model.status.headline, "Ready to mount");
        assert_eq!(
            model.technical_status.mount_state,
            Some(MountState::Pending)
        );
    }

    #[test]
    fn direct_image_is_ready_without_mounting() {
        let model = SelectedGamePresentation::from_live(
            &live_record("/games/Game.rvz", MountState::NotMountable),
            Some(IdentityStatus::Verified),
            false,
            true,
        );
        assert!(!model.mounting_applies);
        assert!(model.direct_use_applies);
        assert!(model.undo_available);
        assert_eq!(model.status.headline, "Ready to use directly");
        assert_eq!(model.status.detail, Some("No mounting needed"));
    }

    #[test]
    fn cached_missing_game_never_claims_live_availability() {
        let model =
            SelectedGamePresentation::from_cached(&cached_archive(true), None, false, false);
        assert!(matches!(model.path, PathAvailability::Missing(_)));
        assert_eq!(model.status.headline, "Game file is unavailable");
        assert_eq!(model.technical_status.mount_state, None);
    }
}

/// The one honest explanation shown everywhere platform detection came up
/// empty. `detect_platform_with_details` (archivefs-core) only ever
/// returns `Some` detection or `None` - it does not currently distinguish
/// *why* it found nothing (unsupported extension vs. ambiguous folder vs.
/// archive contents never inspected vs. no alias match), so the GUI must
/// not invent a specific-sounding reason it cannot back up. This is that
/// one generic, still-useful explanation, kept in one place so a future
/// core change that adds a real per-entry reason only has to update the
/// call sites below, not invent new copy. See docs/GUI_SIMPLIFICATION.md
/// for the core API shape that would unlock per-entry reasons.
pub(crate) const UNKNOWN_PLATFORM_EXPLANATION: &str = "EmuWiz checks the filename, title, and folder \
    path against known platform names and folder aliases. When none of those match, the \
    platform is left Unknown rather than guessed. Assign a platform manually below, or add a \
    folder alias in Sources so future scans recognize it automatically.";

/// Aggregate-form headline for the Unknown-platform explanation banner
/// shown on the Library page - see `UNKNOWN_PLATFORM_EXPLANATION`.
pub(crate) fn unknown_platform_aggregate_headline(count: usize) -> String {
    let noun = if count == 1 { "entry" } else { "entries" };
    format!("{count} {noun} with unknown platform")
}

/// Gates the Library page's aggregate Unknown-platform banner: only worth
/// showing once the user has actually asked to see Unknown-platform rows
/// (the filter checkbox), and only when there is at least one such row to
/// explain.
pub(crate) fn unknown_platform_banner_visible(
    filters: &LibraryRowFilters,
    unknown_count: usize,
) -> bool {
    filters.unknown_platform && unknown_count > 0
}

pub(crate) fn platform_source_label(source: Option<&str>) -> &'static str {
    match source {
        Some(MANUAL_PLATFORM_SOURCE) => "Manual assignment",
        Some(VERIFIED_DAT_PLATFORM_SOURCE) => "Verified by DAT",
        Some(ROMM_PLATFORM_SOURCE) => "Detected from RomM",
        Some(DAT_ROMM_AGREEMENT_SOURCE) => "Verified by DAT and RomM",
        Some(CUSTOM_FOLDER_ALIAS_SOURCE) => "Custom folder alias",
        Some("source_assignment") => "Source assignment",
        Some("header_identity") => "Format/header identity",
        Some("folder_alias") => "Built-in folder alias",
        Some("heuristic-path-detector") => "Filename/path heuristic",
        Some(_) => "Automatic detection",
        None => "Unknown",
    }
}

/// The confidence a stored platform source implies, using the same four-level
/// scale as [`archivefs_core::platform::DetectionConfidence`].
///
/// An explicit assignment and a format/header identity are decisive; a folder
/// alias or a filename heuristic is good evidence that could still be wrong;
/// no platform at all is Unknown. Kept as a mapping from the stored source
/// rather than re-running detection, so what a person sees is the confidence of
/// the assignment that is actually recorded.
pub(crate) fn platform_confidence_label(details: &PlatformProvenanceDetails) -> &'static str {
    use archivefs_core::platform::DetectionConfidence;
    if details.platform.is_none() {
        return DetectionConfidence::Unknown.label();
    }
    match details.source.as_deref() {
        Some(MANUAL_PLATFORM_SOURCE)
        | Some("header_identity")
        | Some(VERIFIED_DAT_PLATFORM_SOURCE)
        | Some(DAT_ROMM_AGREEMENT_SOURCE) => DetectionConfidence::Confirmed.label(),
        Some(ROMM_PLATFORM_SOURCE) => "High",
        Some(_) => DetectionConfidence::Probable.label(),
        None => DetectionConfidence::Unknown.label(),
    }
}

pub(crate) fn platform_provenance_lines(
    details: &PlatformProvenanceDetails,
) -> Vec<(&'static str, String)> {
    // The canonical display name, with the stored identifier alongside it when
    // the two differ - a person reads "Sega Mega Drive / Genesis" while the
    // library stores "MegaDrive", and both matter.
    let platform_line = match details.platform.as_deref() {
        Some(stored) => {
            let display = archivefs_core::platform::display_name_for(stored);
            if display == stored {
                stored.to_string()
            } else {
                format!("{display} ({stored})")
            }
        }
        None => "Unknown".to_string(),
    };
    let mut lines = vec![
        ("Platform", platform_line),
        ("Confidence", platform_confidence_label(details).to_string()),
        (
            "Source",
            platform_source_label(details.source.as_deref()).to_string(),
        ),
        (
            "Assignment",
            if details.source.as_deref() == Some(MANUAL_PLATFORM_SOURCE) {
                "Manually assigned".to_string()
            } else if details.platform.is_some() {
                "Automatically detected".to_string()
            } else {
                "Not assigned".to_string()
            },
        ),
    ];
    if details.platform.is_none() {
        lines.push((
            "Reason",
            "No explicit override, header identity, source assignment, folder alias, or filename evidence matched."
                .to_string(),
        ));
    }

    match (
        details.source.as_deref(),
        details.matched_component.as_ref(),
    ) {
        (Some(CUSTOM_FOLDER_ALIAS_SOURCE), Some(matched)) => {
            lines.push(("Matched alias", matched.clone()));
        }
        (Some("folder_alias"), Some(matched)) => {
            lines.push(("Matched folder", matched.clone()));
        }
        _ => {}
    }

    if details.source.as_deref() == Some(MANUAL_PLATFORM_SOURCE) {
        let fallback = details.automatic_fallback.as_ref();
        lines.push((
            "Automatic fallback",
            fallback
                .map(|fallback| fallback.platform.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
        ));
        if let Some(fallback) = fallback {
            lines.push((
                "Fallback source",
                platform_source_label(Some(&fallback.source)).to_string(),
            ));
            match (
                fallback.source.as_str(),
                fallback.matched_component.as_ref(),
            ) {
                (CUSTOM_FOLDER_ALIAS_SOURCE, Some(matched)) => {
                    lines.push(("Fallback matched alias", matched.clone()));
                }
                ("folder_alias", Some(matched)) => {
                    lines.push(("Fallback matched folder", matched.clone()));
                }
                _ => {}
            }
        }
    }

    lines
}
