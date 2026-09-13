//! Generic, frontend-agnostic types for Publisher Profile Phase 1.
//!
//! Nothing in this file performs I/O. A [`PublisherProfile`] only
//! *describes* one frontend's publishing conventions; [`super::planner`] is
//! the single place that reads a profile plus an existing, already-elected
//! [`crate::playing_library::PlayingLibraryPlan`] and produces a
//! [`PublisherPlan`]. See the crate module doc comment on `publisher_profile`
//! for the full Phase 1 read-only boundary.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which frontend a [`PublisherProfile`] targets. Adding a frontend later
/// (Pegasus, LaunchBox, ROMNight, Steam, ...) means adding one variant here
/// plus one small profile-construction module (see `romm.rs`, `es_de.rs`)
/// - the generic planner in `planner.rs` never changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublisherFrontend {
    RomM,
    EsDe,
}

impl PublisherFrontend {
    pub fn label(self) -> &'static str {
        match self {
            Self::RomM => "RomM",
            Self::EsDe => "ES-DE",
        }
    }

    /// The plain-language novice-facing framing (task section 20) - never
    /// shown alongside internal profile terminology by default.
    pub fn plain_language_goal(self) -> &'static str {
        match self {
            Self::RomM => "Create a RomM-ready library",
            Self::EsDe => "Create an ES-DE-ready library",
        }
    }
}

/// One path segment in a [`PublisherPathRule`]. `PlatformFolder` is
/// substituted with whatever this frontend's own reviewed mapping resolved
/// for the game's canonical platform (see [`PublisherPlatformMapping`]) -
/// never a guessed or derived folder name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    Literal(&'static str),
    PlatformFolder,
}

/// How a profile turns a destination root + resolved platform folder into
/// the concrete parent directory a game's files are proposed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherPathRule {
    pub segments: Vec<PathSegment>,
}

impl PublisherPathRule {
    /// Resolves this rule against a destination root and an already-mapped
    /// platform folder name, never touching the filesystem.
    pub fn resolve(&self, destination_root: &std::path::Path, platform_folder: &str) -> PathBuf {
        let mut path = destination_root.to_path_buf();
        for segment in &self.segments {
            match segment {
                PathSegment::Literal(value) => path.push(value),
                PathSegment::PlatformFolder => path.push(platform_folder),
            }
        }
        path
    }
}

/// How a profile wants a destination file named. Phase 1 only ever
/// preserves the source file's own name - no renaming policy exists yet,
/// per the explicit "avoid unnecessary renaming, never rename source
/// files" instruction. The type exists so a future naming rule can be
/// added without reshaping [`super::planner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublisherNamingRule {
    PreserveSourceFileName,
}

/// How a profile expects a multi-file/multi-disc release to be published.
/// Phase 1 only implements the one rule every reviewed target actually
/// supports today: publish exactly the files the 1G1R election already
/// resolved (a CUE/GDI/M3U launcher plus its required companions),
/// unchanged. No playlist is generated and no representation is swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublisherMediaRule {
    PublishElectedFilesUnchanged,
}

/// Whether a profile requires any metadata file. Phase 1 profiles never
/// read or require one - no `es_systems.xml`/RomM API metadata write path
/// exists yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublisherMetadataRule {
    None,
}

/// A frontend's declared publishing conventions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherProfile {
    pub frontend: PublisherFrontend,
    pub path_rule: PublisherPathRule,
    pub naming_rule: PublisherNamingRule,
    pub media_rule: PublisherMediaRule,
    pub metadata_rule: PublisherMetadataRule,
    /// Lower-cased extensions (no leading dot) this frontend is reviewed to
    /// accept for a launchable file. Empty is a real "not reviewed yet",
    /// never treated as "accepts everything".
    pub accepted_extensions: Vec<&'static str>,
    /// Named capabilities this profile explicitly does not implement in
    /// Phase 1 (e.g. `"playlist_generation"`, `"bios_separation"`,
    /// `"metadata_write"`), surfaced to callers/GUI instead of silently
    /// doing nothing.
    pub unsupported_features: Vec<&'static str>,
}

/// One resolved platform -> frontend-folder mapping outcome. Always built
/// from the frontend's own existing, reviewed mapping table
/// (`production_romm_status`/`es_de_system_for_platform`) - never guessed
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherPlatformMapping {
    Mapped {
        canonical_platform_id: String,
        folder: String,
    },
    Unmapped {
        canonical_platform_id: String,
    },
    Ambiguous {
        canonical_platform_id: String,
    },
    Unsupported {
        canonical_platform_id: String,
    },
}

impl PublisherPlatformMapping {
    pub fn canonical_platform_id(&self) -> &str {
        match self {
            Self::Mapped {
                canonical_platform_id,
                ..
            }
            | Self::Unmapped {
                canonical_platform_id,
            }
            | Self::Ambiguous {
                canonical_platform_id,
            }
            | Self::Unsupported {
                canonical_platform_id,
            } => canonical_platform_id,
        }
    }

    pub fn folder(&self) -> Option<&str> {
        match self {
            Self::Mapped { folder, .. } => Some(folder),
            _ => None,
        }
    }
}

/// One future action a Phase 2 execution engine could take. Phase 1 never
/// performs any of these - see `planner::build_publisher_plan`'s own
/// module doc comment and the crate-level zero-side-effect test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublisherActionKind {
    Hardlink,
    Symlink,
    Copy,
    DirectoryCreate,
    MetadataWrite,
    PlaylistCreate,
}

/// One planned-but-unexecuted action, paired with whether the target
/// frontend actually needs it for this specific item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherPlannedAction {
    pub kind: PublisherActionKind,
    pub required_by_target: bool,
}

/// Whether a planned item is safe to hand to a future execution engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublisherActionSafety {
    SafeToAct,
    ReviewRequired,
    Blocked,
    Unsupported,
}

/// The read-only outcome of inspecting an existing destination path -
/// never a write. See [`super::destination_inspection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationState {
    /// Nothing to compare against - no destination root was supplied.
    Unknown,
    /// The destination does not exist yet.
    Missing,
    /// A symlink (or, on filesystems without symlinks, a same-size regular
    /// file) already points at exactly this planned source.
    AlreadyCorrect,
    /// Something already exists at the destination but does not match this
    /// planned source (a different symlink target, or a regular file).
    Conflicting,
    /// A symlink exists at the destination but its target no longer exists
    /// on disk.
    Stale,
}

/// A typed destination collision - task section 15. Two source items must
/// never silently share a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherConflict {
    DestinationExistsDifferentContent {
        destination: PathBuf,
    },
    DestinationPlanCollision {
        destination: PathBuf,
        contenders: Vec<String>,
    },
    CaseFoldCollision {
        destination_basename: String,
        contenders: Vec<String>,
    },
    MultipleReleasesSameName {
        name: String,
        contenders: Vec<String>,
    },
    RegionCollision {
        name: String,
        contenders: Vec<String>,
    },
}

/// A non-fatal, explicit surfaced concern - never silently swallowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherWarning {
    AmbiguousPlatformMapping { canonical_platform_id: String },
    UnmappedPlatform { canonical_platform_id: String },
    UnsupportedPlatform { canonical_platform_id: String },
    IncompleteMediaSet { reason: String },
    UnsupportedExtension { extension: String },
    RepresentationAmbiguous { reason: String },
    MissingRequiredBios { bios_name: String },
    ExistingDestinationConflict { destination: PathBuf },
}

/// How a required BIOS/firmware file's publication is treated - task
/// section 12. BIOS content is represented separately from game content
/// and is never silently placed into a game folder unless the profile
/// explicitly requires it (no reviewed profile does, in Phase 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiosPublishPolicy {
    /// This profile has no reviewed BIOS-placement behavior yet.
    NotReviewed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherBiosRequirement {
    pub bios_name: String,
    pub present_in_source: bool,
    pub policy: BiosPublishPolicy,
}

/// One companion file a launcher requires alongside it (a CUE's BIN/audio
/// tracks, a GDI's other tracks, an M3U's other discs) - always empty for
/// an ordinary single-file release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherCompanionItem {
    pub source_path: PathBuf,
    pub planned_destination: PathBuf,
}

/// One planned publication of one elected game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherPlanItem {
    pub dat_entry_name: String,
    pub platform_mapping: PublisherPlatformMapping,
    pub source_path: PathBuf,
    /// `None` only when no destination could be safely computed at all
    /// (an unmapped/ambiguous/unsupported platform) - never a guessed path.
    pub planned_destination: Option<PathBuf>,
    pub companions: Vec<PublisherCompanionItem>,
    pub reason: String,
    pub planned_action: PublisherPlannedAction,
    pub safety: PublisherActionSafety,
    pub destination_state: DestinationState,
    pub conflicts: Vec<PublisherConflict>,
    pub warnings: Vec<PublisherWarning>,
}

/// Dry-run counts - task section 17's exact summary shape.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct PublisherPlanSummary {
    pub source_items: usize,
    pub will_publish: usize,
    pub already_present: usize,
    pub review_required: usize,
    pub blocked: usize,
    pub unsupported: usize,
    pub planned_hardlinks: usize,
    pub planned_symlinks: usize,
    pub planned_copies: usize,
}

/// The complete read-only result of one Publisher Profile planning run.
/// Nothing in this type is ever executed by Phase 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherPlan {
    pub frontend: PublisherFrontend,
    pub destination_root: PathBuf,
    pub items: Vec<PublisherPlanItem>,
    pub bios_requirements: Vec<PublisherBiosRequirement>,
    /// A deterministic, order-independent fingerprint of every path,
    /// mapping, action, and conflict this plan carries - task section 23.
    pub plan_hash: String,
}

impl PublisherPlan {
    pub fn summary(&self) -> PublisherPlanSummary {
        let mut summary = PublisherPlanSummary {
            source_items: self.items.len(),
            ..Default::default()
        };
        for item in &self.items {
            match item.safety {
                PublisherActionSafety::SafeToAct => {
                    if item.destination_state == DestinationState::AlreadyCorrect {
                        summary.already_present += 1;
                    } else {
                        summary.will_publish += 1;
                        match item.planned_action.kind {
                            PublisherActionKind::Hardlink => summary.planned_hardlinks += 1,
                            PublisherActionKind::Symlink => summary.planned_symlinks += 1,
                            PublisherActionKind::Copy => summary.planned_copies += 1,
                            _ => {}
                        }
                    }
                }
                PublisherActionSafety::ReviewRequired => summary.review_required += 1,
                PublisherActionSafety::Blocked => summary.blocked += 1,
                PublisherActionSafety::Unsupported => summary.unsupported += 1,
            }
        }
        summary
    }
}
