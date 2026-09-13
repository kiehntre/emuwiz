use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

macro_rules! vocabulary {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum $name { $($variant),+ }
    };
}
vocabulary!(MediaFamily {
    Optical,
    Floppy,
    Tape
});
vocabulary!(OrdinalUnit {
    Medium,
    Disc,
    Disk,
    Tape,
    Part,
    Reel
});
vocabulary!(MediaRole {
    GameMedia,
    InstallMedia,
    PlayMedia,
    BootMedia,
    DataMedia,
    SaveMedia,
    BonusMedia,
    ExtrasMedia,
    DemoMedia,
    AudioMedia,
    SystemMedia,
    UtilityMedia,
    UnknownMedia
});
vocabulary!(MediaSetState {
    CompleteSet,
    IncompleteSet,
    AmbiguousSet,
    ConflictingSet,
    UnverifiedSet,
    UnsupportedSet
});
vocabulary!(MediaSetConfidence {
    Unverified,
    Likely,
    Proven,
    Ambiguous,
    Conflicting
});
// Declaration order is the evidence hierarchy. It never upgrades a weak claim.
vocabulary!(EvidenceKind {
    FuzzyTitle,
    Directory,
    Filename,
    Metadata,
    Embedded,
    TrustedDat,
    VerifiedNative
});
vocabulary!(Equivalence {
    None,
    ExactFile,
    CanonicalContent,
    AuthorityMapping
});
vocabulary!(SideLayout {
    Unknown,
    WholeMedium,
    SeparateSideImages
});
vocabulary!(MediaAvailability {
    Observed,
    Missing,
    Unverified
});
vocabulary!(ConflictKind {
    PlatformConflict,
    ReleaseConflict,
    VariantConflict,
    OrdinalConflict,
    SideConflict,
    RoleConflict,
    CountConflict,
    IdentityConflict,
    CompetingMedia,
    MissingMedium,
    MissingSide,
    UnknownCount,
    UnknownOrdinal,
    UnprovenGrouping,
    UnsupportedFormat,
    InvalidEvidence,
    UnavailableRepresentation,
    UnresolvedRepresentation,
    RelationshipConflict
});
vocabulary!(SwapSemantics {
    OpticalSequence,
    FloppySwap,
    TapeLoad
});
vocabulary!(TransitionKind {
    InsertMedium,
    ChangeSide,
    LoadPart,
    LoaderToProgram,
    ProgramToData
});

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Provenance {
    pub kind: EvidenceKind,
    pub source: String,
    pub version: Option<String>,
}
impl Provenance {
    pub fn new(kind: EvidenceKind, source: impl Into<String>) -> Self {
        Self {
            kind,
            source: source.into(),
            version: None,
        }
    }
    pub fn trusted(&self) -> bool {
        self.kind >= EvidenceKind::TrustedDat
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IdentityKey {
    pub namespace: String,
    pub value: String,
}
impl IdentityKey {
    pub fn new(namespace: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            value: value.into(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MediaOrdinal {
    pub number: u16,
    pub unit: OrdinalUnit,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MediaSide {
    pub number: u8,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReleaseVariant {
    pub region: Option<String>,
    pub revision: Option<String>,
    pub language: Option<String>,
    pub video_standard: Option<String>,
    pub edition: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExpectedCount {
    pub count: u16,
    pub unit: OrdinalUnit,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MediumRequirement {
    pub ordinal: Option<MediaOrdinal>,
    pub side: Option<MediaSide>,
    pub role: Option<MediaRole>,
    pub medium: Option<IdentityKey>,
    pub optional: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MediaRelationship {
    pub target: IdentityKey,
    pub kind: TransitionKind,
}
/// A claim supplied by an existing evidence producer, not an authority parser.
/// Native product IDs must use Equivalence::None unless content equivalence is proven.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MediaEvidence {
    pub provenance: Provenance,
    pub release: Option<IdentityKey>,
    pub medium: Option<IdentityKey>,
    pub equivalence: Equivalence,
    pub title: Option<String>,
    pub ordinal: Option<MediaOrdinal>,
    pub side: Option<MediaSide>,
    pub side_layout: Option<SideLayout>,
    pub expected_sides: BTreeSet<MediaSide>,
    pub role: Option<MediaRole>,
    pub expected_count: Option<ExpectedCount>,
    pub variant: ReleaseVariant,
    pub requirements: Vec<MediumRequirement>,
    pub relationships: Vec<MediaRelationship>,
    pub notes: Vec<String>,
}
impl MediaEvidence {
    pub fn new(kind: EvidenceKind, source: impl Into<String>) -> Self {
        Self {
            provenance: Provenance::new(kind, source),
            release: None,
            medium: None,
            equivalence: Equivalence::None,
            title: None,
            ordinal: None,
            side: None,
            side_layout: None,
            expected_sides: BTreeSet::new(),
            role: None,
            expected_count: None,
            variant: ReleaseVariant::default(),
            requirements: Vec::new(),
            relationships: Vec::new(),
            notes: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ArchiveMemberLocator {
    pub index: usize,
    pub name_bytes: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MediaSource {
    pub path: PathBuf,
    pub archive_member: Option<ArchiveMemberLocator>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRecord {
    pub source: MediaSource,
    pub platform: Option<String>,
    pub family: Option<MediaFamily>,
    pub format: String,
    pub availability: MediaAvailability,
    pub evidence: Vec<MediaEvidence>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MediaSetConflict {
    pub kind: ConflictKind,
    pub detail: String,
    pub blocking: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSetIdentity {
    pub key: IdentityKey,
    pub provenance: Vec<Provenance>,
    pub verified: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaIdentity {
    pub key: IdentityKey,
    pub provenance: Provenance,
    pub equivalence: Equivalence,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRepresentation {
    pub record: MediaRecord,
    pub release_identity: Option<MediaSetIdentity>,
    pub media_identity: Option<MediaIdentity>,
    pub ordinal: Option<MediaOrdinal>,
    pub side: Option<MediaSide>,
    pub role: MediaRole,
    pub variant: ReleaseVariant,
    pub expected_count: Option<(ExpectedCount, Provenance)>,
    pub side_layout: SideLayout,
    pub expected_sides: BTreeSet<MediaSide>,
    pub confidence: MediaSetConfidence,
    pub conflicts: Vec<MediaSetConflict>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSetMember {
    pub id: String,
    pub ordinal: Option<MediaOrdinal>,
    pub role: MediaRole,
    pub sides: BTreeSet<MediaSide>,
    pub side_layout: SideLayout,
    pub representations: Vec<MediaRepresentation>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSet {
    pub identity: MediaSetIdentity,
    pub platform: Option<String>,
    pub family: Option<MediaFamily>,
    pub variant: ReleaseVariant,
    pub members: Vec<MediaSetMember>,
    pub expected_count: Option<(ExpectedCount, Provenance)>,
    pub state: MediaSetState,
    pub confidence: MediaSetConfidence,
    pub conflicts: Vec<MediaSetConflict>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyStats {
    pub input_records: usize,
    pub grouping_buckets: usize,
    pub candidate_comparisons: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyReport {
    pub sets: Vec<MediaSet>,
    pub stats: TopologyStats,
}
/// Caller-selected profile evidence. No global format ranking or readiness probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaProfile {
    pub id: String,
    pub platform: String,
    pub supported_formats: BTreeSet<String>,
    pub preferred_formats: Vec<String>,
    pub readiness_hint: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSwapStep {
    pub member_id: String,
    pub ordinal: Option<MediaOrdinal>,
    pub side: Option<MediaSide>,
    pub role: MediaRole,
    pub preferred_representation: Option<MediaSource>,
    pub alternatives: Vec<MediaSource>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaTransition {
    pub from: usize,
    pub to: usize,
    pub kind: TransitionKind,
    pub provenance: Vec<Provenance>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSwapPlan {
    pub release_identity: MediaSetIdentity,
    pub platform: Option<String>,
    pub semantics: Option<SwapSemantics>,
    pub ordered_media: Vec<MediaSwapStep>,
    pub transitions: Vec<MediaTransition>,
    pub profile: Option<MediaProfile>,
    pub warnings: Vec<String>,
    pub blockers: Vec<MediaSetConflict>,
    pub confidence: MediaSetConfidence,
    pub provenance: Vec<Provenance>,
}
