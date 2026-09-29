//! Read-only applicability for one logical cheat and one selected game.
//! Identity facts retain the inspector's Verified/Candidate distinction.
//! This is neither activation policy nor proof of runtime execution.

use serde::{Deserialize, Serialize};

use super::cheat_ir::{
    CheatConversionPreview, CheatDocument, CheatPlatform, CheatReconciliationResult,
    CheatRelationship, CheatSourceFormat, CheatTargetFormat, convert_cheat_document,
};
use super::cheat_route::{
    CheatApplySupport, CheatRoute, CheatRouteTarget, canonical_cheat_platform,
};
use super::cht_document::{ChtEntry, ChtEntryWarningKind};
use super::dolphin_gecko_provider::{GeckoRegion, region_for_game_id};
use crate::game_identity::{GameIdentityReport, IdentityEvidence, IdentityKind, IdentityStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatReleaseEvidence {
    pub value: String,
    pub status: IdentityStatus,
}

/// Metadata and filenames remain separate from the verified identity facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatSelectedGame {
    pub title: Option<String>,
    pub filename: Option<String>,
    pub platform: Option<CheatReleaseEvidence>,
    pub region: Option<CheatReleaseEvidence>,
    pub revision: Option<CheatReleaseEvidence>,
    pub facts: Vec<IdentityEvidence>,
}

impl CheatSelectedGame {
    /// Keep every observation, including contradictory and candidate facts.
    /// Do not convert a report's platform guess into a verified fact.
    pub fn from_identity_report(report: &GameIdentityReport) -> Self {
        let release = |kind| {
            let mut values: Vec<_> = report
                .evidence
                .iter()
                .filter(|fact| fact.kind == kind && fact.status == IdentityStatus::Verified)
                .filter_map(|fact| fact.value.as_deref())
                .filter(|value| !value.trim().is_empty())
                .collect();
            values.sort();
            values.dedup();
            match values.as_slice() {
                [value] => Some(CheatReleaseEvidence {
                    value: (*value).into(),
                    status: IdentityStatus::Verified,
                }),
                [] => None,
                _ => Some(CheatReleaseEvidence {
                    value: values.join(" / "),
                    status: IdentityStatus::Ambiguous,
                }),
            }
        };
        // The inspector intentionally stores the raw region byte. Reuse the
        // Dolphin provider's reviewed decoder; never display E as a locale.
        let region = release(IdentityKind::DolphinGameId).and_then(|identity| {
            if identity.status != IdentityStatus::Verified {
                return None;
            }
            match region_for_game_id(&identity.value)? {
                GeckoRegion::Unknown(_) => None,
                region => Some(CheatReleaseEvidence {
                    value: region.display_name().into(),
                    status: IdentityStatus::Verified,
                }),
            }
        });
        Self {
            title: report
                .verified_value(IdentityKind::LooseRomTitle)
                .map(str::to_owned),
            filename: report
                .archive_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned),
            platform: release(IdentityKind::Platform),
            region,
            revision: release(IdentityKind::DolphinRevision),
            facts: report.evidence.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatIdentityRequirement {
    pub kind: IdentityKind,
    pub value: String,
}

/// Source declarations are expectations, not verification of the selected ROM.
/// Callers must not turn filename tokens into identities or release metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatGameAssociation {
    pub title: Option<String>,
    pub filename: Option<String>,
    pub platform: Option<String>,
    pub region: Option<String>,
    pub revision: Option<String>,
    pub identities: Vec<CheatIdentityRequirement>,
    pub manually_associated: bool,
}

/// Parser evidence is independent of IR semantics: opaque native codes may
/// parse correctly even when the format-neutral IR cannot decode them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatParseEvidence {
    Valid,
    Malformed { details: Vec<String> },
    MissingCode,
    Unknown,
}

impl CheatParseEvidence {
    pub fn from_cht_entry(entry: &ChtEntry) -> Self {
        if entry
            .code
            .as_deref()
            .is_none_or(|code| code.trim().is_empty())
        {
            return Self::MissingCode;
        }
        let mut details: Vec<_> = entry
            .blocking_warnings()
            .filter(|warning| {
                !matches!(
                    warning.kind,
                    ChtEntryWarningKind::MissingCode | ChtEntryWarningKind::EmptyCode
                )
            })
            .map(|warning| warning.detail.clone())
            .collect();
        details.sort();
        if details.is_empty() {
            Self::Valid
        } else {
            Self::Malformed { details }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatApplicabilityMatch {
    Unknown,
    ManualAssociation,
    FilenameAssociation,
    TitleOnly,
    Strong,
    VerifiedIdentifier,
    ExactHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatApplicabilityState {
    Ready,
    ExactGameMatch,
    StrongMatch,
    PossibleMatch,
    WrongRegion,
    WrongRevision,
    DifferentGame,
    UnsupportedFormat,
    UnsupportedEmulator,
    ConflictingVariants,
    Malformed,
    MissingRequiredEvidence,
    NeedsReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatApplicabilityIssue {
    Malformed,
    MissingCode,
    ConflictingVariants,
    ConflictingIdentity,
    DifferentGame,
    WrongRegion,
    WrongRevision,
    UnsupportedEmulator,
    UnsupportedFormat,
    EmulatorCapabilityUnknown,
    FormatCapabilityUnknown,
    EngineCapabilityUnknown,
    ParsingUnknown,
    IdentityUnknown,
    RequiredIdentityUnknown,
    RegionUnknown,
    RevisionUnknown,
    UnverifiedAssociation,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CheatApplicabilityFinding {
    /// Stable machine-readable category, independent of normal UI wording.
    pub kind: CheatApplicabilityFindingKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatApplicabilityFindingKind {
    HashMatch,
    IdentifierMatch,
    PlatformMatch,
    PlatformMismatch,
    DocumentPlatformMismatch,
    RegionMatch,
    RegionMismatch,
    RevisionMatch,
    RevisionMismatch,
    TitleMatch,
    FilenameAssociation,
    ManualAssociation,
    CorroboratedDuplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheatSupportState {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatApplicabilitySupport {
    pub route: Option<CheatRoute>,
    pub emulator: CheatSupportState,
    pub format: CheatSupportState,
    pub engine: CheatSupportState,
    pub target_format: Option<CheatTargetFormat>,
    pub conversion: Option<CheatConversionPreview>,
}

/// One logical cheat. The full reconciliation result keeps all source variants
/// and their provenance; applicability never chooses a winning implementation.
#[derive(Debug, Clone)]
pub struct CheatApplicabilityInput {
    pub game: CheatSelectedGame,
    pub association: CheatGameAssociation,
    pub document: CheatDocument,
    pub parsing: CheatParseEvidence,
    /// Exact parser output for native RetroArch files. Enables recognition
    /// of the existing .cht writer without claiming an opaque code works in
    /// a particular core's cheat engine.
    pub native_cht: Option<ChtEntry>,
    pub route: Option<CheatRoute>,
    pub reconciliation: Option<CheatReconciliationResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatApplicabilityReport {
    pub state: CheatApplicabilityState,
    pub identity_match: CheatApplicabilityMatch,
    pub evidence: Vec<CheatApplicabilityFinding>,
    pub blockers: Vec<CheatApplicabilityIssue>,
    pub warnings: Vec<CheatApplicabilityIssue>,
    pub game: CheatSelectedGame,
    pub association: CheatGameAssociation,
    pub source_format: CheatSourceFormat,
    pub document: CheatDocument,
    pub parsing: CheatParseEvidence,
    pub native_cht: Option<ChtEntry>,
    pub support: CheatApplicabilitySupport,
    pub provenance: Vec<String>,
    pub reconciliation: Option<CheatReconciliationResult>,
}

fn text_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_hash(kind: IdentityKind) -> bool {
    matches!(
        kind,
        IdentityKind::LooseRomSha256 | IdentityKind::LooseRomCanonicalSha256
    )
}

fn is_identifier(kind: IdentityKind) -> bool {
    matches!(
        kind,
        IdentityKind::AmigaWHDLoad
            | IdentityKind::Pcsx2ExecutableCrc
            | IdentityKind::Ps1Serial
            | IdentityKind::Ps2Serial
            | IdentityKind::PspDiscId
            | IdentityKind::Ps3TitleId
            | IdentityKind::Ps4TitleId
            | IdentityKind::Ps4ContentId
            | IdentityKind::SaturnProductNumber
            | IdentityKind::DreamcastProductCode
            | IdentityKind::SegaCdProductCode
            | IdentityKind::DolphinGameId
            | IdentityKind::MameMachineName
            | IdentityKind::XbeTitleId
            | IdentityKind::XexTitleId
            | IdentityKind::XexMediaId
            | IdentityKind::ScummVmGameId
            | IdentityKind::ThreeDoDiscId
    )
}

fn release_matches(
    kind: &str,
    selected: Option<&CheatReleaseEvidence>,
    expected: Option<&str>,
    mismatch: CheatApplicabilityIssue,
    unknown: CheatApplicabilityIssue,
    evidence: &mut Vec<CheatApplicabilityFinding>,
    blockers: &mut Vec<CheatApplicabilityIssue>,
    warnings: &mut Vec<CheatApplicabilityIssue>,
) -> bool {
    let selected = selected
        .filter(|fact| fact.status == IdentityStatus::Verified && !fact.value.trim().is_empty());
    match (selected, expected.filter(|v| !v.trim().is_empty())) {
        (Some(left), Some(right)) if release_key(kind, &left.value) == release_key(kind, right) => {
            evidence.push(CheatApplicabilityFinding {
                kind: release_finding_kind(kind, true),
                detail: right.into(),
            });
            true
        }
        (Some(left), Some(right)) => {
            blockers.push(mismatch);
            evidence.push(CheatApplicabilityFinding {
                kind: release_finding_kind(kind, false),
                detail: format!("cheat: {right}; selected game: {}", left.value),
            });
            false
        }
        _ => {
            warnings.push(unknown);
            false
        }
    }
}

fn release_finding_kind(kind: &str, matched: bool) -> CheatApplicabilityFindingKind {
    use CheatApplicabilityFindingKind as Kind;
    match (kind, matched) {
        ("platform", true) => Kind::PlatformMatch,
        ("platform", false) => Kind::PlatformMismatch,
        ("region", true) => Kind::RegionMatch,
        ("region", false) => Kind::RegionMismatch,
        (_, true) => Kind::RevisionMatch,
        (_, false) => Kind::RevisionMismatch,
    }
}

fn release_key(kind: &str, value: &str) -> String {
    if kind == "platform" {
        return text_key(canonical_cheat_platform(value).unwrap_or(value));
    }
    // These labels describe the same provider region. Do not fold revisions
    // (e.g. 0, 1.0, Rev A) or raw region bytes into guessed equivalents.
    match (kind, text_key(value).as_str()) {
        ("region", "ntsc-u" | "usa") => "usa".into(),
        ("region", "pal" | "europe") => "europe".into(),
        ("region", "ntsc-j" | "japan") => "japan".into(),
        _ => text_key(value),
    }
}

/// Support is derived from the existing selected route and conversion assessor.
/// A route alone cannot prove a particular format/operation is supported.
fn assess_support(input: &CheatApplicabilityInput) -> CheatApplicabilitySupport {
    let mut result = CheatApplicabilitySupport {
        route: input.route.clone(),
        emulator: CheatSupportState::Unknown,
        format: CheatSupportState::Unknown,
        engine: CheatSupportState::Unknown,
        target_format: None,
        conversion: None,
    };
    let Some(route) = &input.route else {
        return result;
    };
    if route.apply_support != CheatApplySupport::Supported {
        result.emulator = CheatSupportState::Unsupported;
        return result;
    }
    if matches!(route.target, CheatRouteTarget::RetroArch { core: None }) {
        return result;
    }
    result.emulator = CheatSupportState::Supported;
    if matches!(route.target, CheatRouteTarget::RetroArch { .. })
        && input.document.source_format == CheatSourceFormat::RetroArch
        && input
            .native_cht
            .as_ref()
            .is_some_and(ChtEntry::is_selectable)
    {
        // The native parser and renderer support this container. They do not
        // verify every libretro core's opaque code syntax or cheat engine.
        result.format = CheatSupportState::Supported;
        result.target_format = Some(CheatTargetFormat::RetroArch);
        return result;
    }
    let target = match &route.target {
        CheatRouteTarget::RetroArch { .. } => Some(CheatTargetFormat::RetroArch),
        CheatRouteTarget::Standalone { adapter_id } if adapter_id == "pcsx2" => {
            Some(CheatTargetFormat::Pnach)
        }
        CheatRouteTarget::Standalone { adapter_id } if adapter_id == "dolphin" => {
            Some(match input.document.source_format {
                CheatSourceFormat::DolphinActionReplay => CheatTargetFormat::DolphinActionReplay,
                CheatSourceFormat::DolphinOnFrame => CheatTargetFormat::DolphinOnFrame,
                _ => CheatTargetFormat::Gecko,
            })
        }
        _ => None,
    };
    if let Some(target) = target {
        let conversion = convert_cheat_document(&input.document, target.clone());
        result.format = if conversion.can_apply {
            CheatSupportState::Supported
        } else {
            CheatSupportState::Unsupported
        };
        result.target_format = Some(target);
        result.engine = result.format;
        result.conversion = Some(conversion);
    }
    result
}

/// Deterministic evaluation; every blocker survives priority selection.
pub fn assess_cheat_applicability(input: &CheatApplicabilityInput) -> CheatApplicabilityReport {
    let mut evidence = Vec::new();
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let mut identity_match = CheatApplicabilityMatch::Unknown;
    if [
        &input.game.platform,
        &input.game.region,
        &input.game.revision,
    ]
    .into_iter()
    .any(|value| {
        value
            .as_ref()
            .is_some_and(|fact| fact.status == IdentityStatus::Ambiguous)
    }) {
        blockers.push(CheatApplicabilityIssue::ConflictingIdentity);
    }
    let document_platform = match &input.document.platform {
        CheatPlatform::GameCube => "GameCube",
        CheatPlatform::Wii => "Wii",
        CheatPlatform::Nintendo64 => "N64",
        CheatPlatform::Ps2 => "PS2",
        CheatPlatform::NintendoDs => "NDS",
        CheatPlatform::Other(value) => value,
    };
    if let Some(platform) = input
        .game
        .platform
        .as_ref()
        .filter(|fact| fact.status == IdentityStatus::Verified)
    {
        if release_key("platform", &platform.value) != release_key("platform", document_platform) {
            blockers.push(CheatApplicabilityIssue::DifferentGame);
            evidence.push(CheatApplicabilityFinding {
                kind: CheatApplicabilityFindingKind::DocumentPlatformMismatch,
                detail: format!(
                    "cheat document: {document_platform}; selected game: {}",
                    platform.value
                ),
            });
        }
    }
    let parsing = input
        .native_cht
        .as_ref()
        .map(CheatParseEvidence::from_cht_entry)
        .unwrap_or_else(|| input.parsing.clone());
    match &parsing {
        CheatParseEvidence::Malformed { .. } => blockers.push(CheatApplicabilityIssue::Malformed),
        CheatParseEvidence::MissingCode => blockers.push(CheatApplicabilityIssue::MissingCode),
        CheatParseEvidence::Unknown => blockers.push(CheatApplicabilityIssue::ParsingUnknown),
        CheatParseEvidence::Valid => {}
    }
    if input.document.operations.is_empty()
        && input.native_cht.as_ref().is_none_or(|entry| {
            entry
                .code
                .as_deref()
                .is_none_or(|code| code.trim().is_empty())
        })
    {
        blockers.push(CheatApplicabilityIssue::MissingCode);
    }
    // Contradictory inspector facts must not be resolved by input ordering,
    // even when the cheat does not declare the contradictory identity kind.
    for (index, fact) in input.game.facts.iter().enumerate() {
        if fact.status == IdentityStatus::Ambiguous
            && (is_hash(fact.kind) || is_identifier(fact.kind))
        {
            blockers.push(CheatApplicabilityIssue::ConflictingIdentity);
        }
        if fact.status != IdentityStatus::Verified
            || (!is_hash(fact.kind)
                && !is_identifier(fact.kind)
                && !matches!(
                    fact.kind,
                    IdentityKind::Platform
                        | IdentityKind::DolphinRegion
                        | IdentityKind::DolphinRevision
                ))
        {
            continue;
        }
        if input.game.facts[index + 1..].iter().any(|other| {
            other.kind == fact.kind
                && other.status == IdentityStatus::Verified
                && fact
                    .value
                    .as_deref()
                    .zip(other.value.as_deref())
                    .is_some_and(|(left, right)| text_key(left) != text_key(right))
        }) {
            blockers.push(CheatApplicabilityIssue::ConflictingIdentity);
        }
    }
    for expected in &input.association.identities {
        if (!is_hash(expected.kind) && !is_identifier(expected.kind))
            || expected.value.trim().is_empty()
        {
            continue;
        }
        let facts: Vec<_> = input
            .game
            .facts
            .iter()
            .filter(|fact| {
                fact.kind == expected.kind
                    && fact.status == IdentityStatus::Verified
                    && fact.value.as_deref().is_some_and(|v| !v.trim().is_empty())
            })
            .collect();
        let matches = facts
            .iter()
            .any(|fact| text_key(fact.value.as_deref().unwrap()) == text_key(&expected.value));
        if facts.is_empty() {
            blockers.push(CheatApplicabilityIssue::RequiredIdentityUnknown);
        }
        let disagrees = facts
            .iter()
            .any(|fact| text_key(fact.value.as_deref().unwrap()) != text_key(&expected.value));
        if disagrees {
            blockers.push(if matches {
                CheatApplicabilityIssue::ConflictingIdentity
            } else {
                CheatApplicabilityIssue::DifferentGame
            });
        }
        if matches {
            identity_match = identity_match.max(if is_hash(expected.kind) {
                CheatApplicabilityMatch::ExactHash
            } else {
                CheatApplicabilityMatch::VerifiedIdentifier
            });
            evidence.push(CheatApplicabilityFinding {
                kind: if is_hash(expected.kind) {
                    CheatApplicabilityFindingKind::HashMatch
                } else {
                    CheatApplicabilityFindingKind::IdentifierMatch
                },
                detail: format!("{:?}: {}", expected.kind, expected.value),
            });
        }
    }
    let platform_matches = release_matches(
        "platform",
        input.game.platform.as_ref(),
        input.association.platform.as_deref(),
        CheatApplicabilityIssue::DifferentGame,
        CheatApplicabilityIssue::IdentityUnknown,
        &mut evidence,
        &mut blockers,
        &mut warnings,
    );
    let region_matches = release_matches(
        "region",
        input.game.region.as_ref(),
        input.association.region.as_deref(),
        CheatApplicabilityIssue::WrongRegion,
        CheatApplicabilityIssue::RegionUnknown,
        &mut evidence,
        &mut blockers,
        &mut warnings,
    );
    let revision_matches = release_matches(
        "revision",
        input.game.revision.as_ref(),
        input.association.revision.as_deref(),
        CheatApplicabilityIssue::WrongRevision,
        CheatApplicabilityIssue::RevisionUnknown,
        &mut evidence,
        &mut blockers,
        &mut warnings,
    );
    if let (Some(left), Some(right)) = (&input.game.title, &input.association.title) {
        if !left.trim().is_empty() && text_key(left) == text_key(right) {
            identity_match = identity_match.max(if platform_matches {
                CheatApplicabilityMatch::Strong
            } else {
                CheatApplicabilityMatch::TitleOnly
            });
            evidence.push(CheatApplicabilityFinding {
                kind: CheatApplicabilityFindingKind::TitleMatch,
                detail: right.clone(),
            });
        } else if identity_match == CheatApplicabilityMatch::Unknown
            && !left.trim().is_empty()
            && !right.trim().is_empty()
        {
            // A metadata disagreement cannot override a verified identifier.
            blockers.push(CheatApplicabilityIssue::DifferentGame);
        }
    }
    if let (Some(left), Some(right)) = (&input.game.filename, &input.association.filename) {
        if !left.trim().is_empty() && text_key(left) == text_key(right) {
            identity_match = identity_match.max(CheatApplicabilityMatch::FilenameAssociation);
            evidence.push(CheatApplicabilityFinding {
                kind: CheatApplicabilityFindingKind::FilenameAssociation,
                detail: right.clone(),
            });
        }
    }
    if input.association.manually_associated {
        identity_match = identity_match.max(CheatApplicabilityMatch::ManualAssociation);
        evidence.push(CheatApplicabilityFinding {
            kind: CheatApplicabilityFindingKind::ManualAssociation,
            detail: "Explicit user association".into(),
        });
    }
    if identity_match == CheatApplicabilityMatch::Unknown {
        blockers.push(CheatApplicabilityIssue::IdentityUnknown);
    }
    if identity_match < CheatApplicabilityMatch::VerifiedIdentifier {
        warnings.push(CheatApplicabilityIssue::UnverifiedAssociation);
    }
    if let Some(result) = &input.reconciliation {
        for group in &result.groups {
            match group.relationship {
                CheatRelationship::SameTitleDifferentCode | CheatRelationship::RelatedUnproven => {
                    blockers.push(CheatApplicabilityIssue::ConflictingVariants)
                }
                CheatRelationship::ExactSemanticDuplicate
                | CheatRelationship::ExactRawDuplicate => {
                    evidence.push(CheatApplicabilityFinding {
                        kind: CheatApplicabilityFindingKind::CorroboratedDuplicate,
                        detail: format!(
                            "{} source records agree; source independence is not established",
                            group.entry_indices.len()
                        ),
                    })
                }
                CheatRelationship::Unique => {}
            }
        }
    }
    let support = assess_support(input);
    match support.emulator {
        CheatSupportState::Unsupported => {
            blockers.push(CheatApplicabilityIssue::UnsupportedEmulator)
        }
        CheatSupportState::Unknown => {
            blockers.push(CheatApplicabilityIssue::EmulatorCapabilityUnknown)
        }
        CheatSupportState::Supported => {}
    }
    match support.format {
        CheatSupportState::Unsupported => blockers.push(CheatApplicabilityIssue::UnsupportedFormat),
        CheatSupportState::Unknown => {
            blockers.push(CheatApplicabilityIssue::FormatCapabilityUnknown)
        }
        CheatSupportState::Supported => {}
    }
    if support.engine == CheatSupportState::Unknown {
        blockers.push(CheatApplicabilityIssue::EngineCapabilityUnknown);
    }
    evidence.sort();
    evidence.dedup();
    blockers.sort();
    blockers.dedup();
    warnings.sort();
    warnings.dedup();
    let state = if blockers.contains(&CheatApplicabilityIssue::Malformed) {
        CheatApplicabilityState::Malformed
    } else if blockers.contains(&CheatApplicabilityIssue::MissingCode) {
        CheatApplicabilityState::MissingRequiredEvidence
    } else if blockers.contains(&CheatApplicabilityIssue::ConflictingVariants) {
        CheatApplicabilityState::ConflictingVariants
    } else if blockers.contains(&CheatApplicabilityIssue::ConflictingIdentity) {
        CheatApplicabilityState::NeedsReview
    } else if blockers.contains(&CheatApplicabilityIssue::WrongRegion) {
        CheatApplicabilityState::WrongRegion
    } else if blockers.contains(&CheatApplicabilityIssue::WrongRevision) {
        CheatApplicabilityState::WrongRevision
    } else if blockers.contains(&CheatApplicabilityIssue::DifferentGame) {
        CheatApplicabilityState::DifferentGame
    } else if blockers.contains(&CheatApplicabilityIssue::UnsupportedEmulator) {
        CheatApplicabilityState::UnsupportedEmulator
    } else if blockers.contains(&CheatApplicabilityIssue::UnsupportedFormat) {
        CheatApplicabilityState::UnsupportedFormat
    } else if blockers.contains(&CheatApplicabilityIssue::IdentityUnknown)
        || blockers.contains(&CheatApplicabilityIssue::RequiredIdentityUnknown)
        || blockers.contains(&CheatApplicabilityIssue::ParsingUnknown)
    {
        CheatApplicabilityState::MissingRequiredEvidence
    } else if !blockers.is_empty() {
        CheatApplicabilityState::NeedsReview
    } else if identity_match == CheatApplicabilityMatch::ExactHash
        || (identity_match >= CheatApplicabilityMatch::VerifiedIdentifier
            && region_matches
            && revision_matches)
    {
        CheatApplicabilityState::Ready
    } else if identity_match >= CheatApplicabilityMatch::VerifiedIdentifier {
        CheatApplicabilityState::ExactGameMatch
    } else if identity_match == CheatApplicabilityMatch::Strong {
        CheatApplicabilityState::StrongMatch
    } else {
        CheatApplicabilityState::PossibleMatch
    };
    let mut provenance = input.document.provenance.clone();
    provenance.sort();
    CheatApplicabilityReport {
        state,
        identity_match,
        evidence,
        blockers,
        warnings,
        game: input.game.clone(),
        association: input.association.clone(),
        source_format: input.document.source_format.clone(),
        document: input.document.clone(),
        parsing,
        native_cht: input.native_cht.clone(),
        support,
        provenance,
        reconciliation: input.reconciliation.clone(),
    }
}

/// Minimal GUI projection. Advanced surfaces can retain the complete report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheatApplicabilityPresentation {
    pub label: &'static str,
    pub summary: String,
}

impl CheatApplicabilityReport {
    pub fn presentation(&self) -> CheatApplicabilityPresentation {
        use CheatApplicabilityState as State;
        let (label, mut summary) = match self.state {
            State::Ready if self.identity_match == CheatApplicabilityMatch::ExactHash => ("Ready to use", "The game data matches exactly, and this cheat is supported by the selected emulator.".into()),
            State::Ready => ("Ready to use", "Exact match for this game, region, and revision.".into()),
            State::ExactGameMatch => ("Exact game match", "The game identifier matches, but EmuWiz could not verify the exact region or revision.".into()),
            State::StrongMatch => ("Strong match", "The title and platform match, but EmuWiz could not verify the exact game release.".into()),
            State::PossibleMatch if self.identity_match == CheatApplicabilityMatch::FilenameAssociation => ("Possible match", "The filename matches, but EmuWiz could not verify the game or revision.".into()),
            State::PossibleMatch if self.identity_match == CheatApplicabilityMatch::ManualAssociation => ("Possible match", "You associated this cheat with the game. EmuWiz has not verified compatibility.".into()),
            State::PossibleMatch => ("Possible match", "The title matches, but EmuWiz could not verify the exact game revision.".into()),
            State::WrongRegion => ("Wrong region", format!("This cheat is for the {} release. Your selected game is {}.", self.association.region.as_deref().unwrap_or("unknown"), self.game.region.as_ref().map_or("unknown", |v| v.value.as_str()))),
            State::WrongRevision => ("Wrong revision", format!("This cheat is for revision {}. Your selected game is revision {}.", self.association.revision.as_deref().unwrap_or("unknown"), self.game.revision.as_ref().map_or("unknown", |v| v.value.as_str()))),
            State::DifferentGame => ("Different game", "The available game identity does not match this cheat.".into()),
            State::UnsupportedFormat => ("Unsupported cheat format", "EmuWiz cannot use these codes with the selected emulator's cheat format.".into()),
            State::UnsupportedEmulator => ("Unsupported emulator", "EmuWiz cannot apply cheats through the selected emulator.".into()),
            State::ConflictingVariants => ("Needs review", "Sources provide conflicting versions of this cheat. Review every version before choosing.".into()),
            State::Malformed => ("Malformed cheat", "The cheat contains invalid or incomplete code data.".into()),
            State::MissingRequiredEvidence if self.blockers.contains(&CheatApplicabilityIssue::MissingCode) => ("Missing required evidence", "This cheat has no usable code.".into()),
            State::MissingRequiredEvidence => ("Missing required evidence", "EmuWiz needs verified game identity and readable cheat data to assess this cheat.".into()),
            State::NeedsReview if self.blockers.contains(&CheatApplicabilityIssue::ConflictingIdentity) => ("Needs review", "The verified game identity evidence disagrees.".into()),
            State::NeedsReview => ("Needs review", "EmuWiz could not verify support for this cheat in the selected emulator.".into()),
        };
        if self.state == State::Ready && self.identity_match == CheatApplicabilityMatch::ExactHash {
            let region_unknown = self
                .warnings
                .contains(&CheatApplicabilityIssue::RegionUnknown);
            let revision_unknown = self
                .warnings
                .contains(&CheatApplicabilityIssue::RevisionUnknown);
            summary.push_str(match (region_unknown, revision_unknown) {
                (true, true) => " Region and revision details are unknown.",
                (true, false) => " Region details are unknown.",
                (false, true) => " Revision details are unknown.",
                (false, false) => "",
            });
        }
        CheatApplicabilityPresentation { label, summary }
    }
}

#[cfg(test)]
mod tests;
