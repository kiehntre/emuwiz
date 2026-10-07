//! Typed vocabulary of the emulator profile resolver.
//!
//! Nothing here is stringly-typed control flow: every decision the resolver
//! makes is carried by an enum. The `detail` strings exist only to explain a
//! decision to a person.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use crate::emulator_inventory::InventoryEmulator;
use crate::launch::installation::LaunchInstallation;

/// What an installation/profile pair is, independent of how it was found.
///
/// A candidate is an executable *together with the profile that executable
/// would actually use*. Two candidates are the same candidate only when both
/// halves match.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct CandidateIdentity {
    pub emulator: InventoryEmulator,
    /// `None` for a profile that has no installed executable (stale data).
    pub executable: Option<PathBuf>,
    pub profile_root: PathBuf,
}

impl CandidateIdentity {
    /// Stable text form used when a choice is remembered.
    pub fn storage_key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.emulator.label(),
            self.executable
                .as_deref()
                .map_or(String::new(), |path| path.display().to_string()),
            self.profile_root.display()
        )
    }
}

impl fmt::Display for CandidateIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.executable {
            Some(executable) => write!(
                formatter,
                "{} + {}",
                executable.display(),
                self.profile_root.display()
            ),
            None => write!(
                formatter,
                "(no executable) + {}",
                self.profile_root.display()
            ),
        }
    }
}

/// How the profile relates to the executable that would use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ProfileLayout {
    /// The profile lives beside the executable (portable marker or sibling
    /// configuration).
    Portable,
    /// The emulator's own per-user default location.
    DefaultUser,
    /// A Flatpak sandbox's own data.
    FlatpakSandbox,
    /// A root the user supplied explicitly.
    Explicit,
}

/// The user's requested way of choosing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EmulatorSelectionMode {
    /// EmuWiz picks the strongest, healthiest candidate.
    Auto,
    /// The user prefers this candidate; EmuWiz may bypass it, loudly, if it
    /// is unusable and another candidate is clearly healthy.
    Preferred(CandidateIdentity),
    /// The user pinned exactly this candidate. Never substituted.
    Forced(CandidateIdentity),
}

impl EmulatorSelectionMode {
    pub fn kind(&self) -> SelectionModeKind {
        match self {
            Self::Auto => SelectionModeKind::Auto,
            Self::Preferred(_) => SelectionModeKind::Preferred,
            Self::Forced(_) => SelectionModeKind::Forced,
        }
    }

    pub fn identity(&self) -> Option<&CandidateIdentity> {
        match self {
            Self::Auto => None,
            Self::Preferred(identity) | Self::Forced(identity) => Some(identity),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SelectionModeKind {
    Auto,
    Preferred,
    Forced,
}

/// Whether the emulator's first-run setup is known to be finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum SetupState {
    /// The emulator says setup is unfinished (it would show its wizard).
    Incomplete,
    /// A configuration exists but nothing says setup was ever completed.
    NotRecorded,
    /// The emulator recorded that setup is finished.
    Complete,
    /// The emulator has no first-run setup step.
    NotApplicable,
}

impl SetupState {
    fn rank(self) -> u8 {
        match self {
            Self::Incomplete => 0,
            Self::NotRecorded => 1,
            Self::Complete | Self::NotApplicable => 2,
        }
    }
}

/// Facts about whether the profile has been set up and used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileReadiness {
    pub profile_directory_present: bool,
    pub main_config_present: bool,
    pub setup: SetupState,
}

/// Why a candidate cannot be used at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum UnusableReason {
    ExecutableMissing,
    ExecutableNotARegularFile,
    ExecutableNotExecutable,
    ExecutableNotAbsolute,
    /// Profile data of a Flatpak that is not installed.
    FlatpakNotInstalled,
    ProfileDirectoryMissing,
    ProfileDirectoryNotADirectory,
    MainConfigMissing,
    MainConfigUnreadable,
    /// The executable would use a different profile than the one named.
    IncompatiblePairing,
}

/// The kinds of fact the resolver weighs. Behaviour depends on these, never
/// on the wording of `SelectionEvidence::detail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum EvidenceKind {
    /// The user named this executable or profile (weak: only evidence in AUTO).
    UserOverrideNamesIt,
    /// A portable marker sits beside the executable.
    PortableMarker,
    /// The profile configuration beside the executable is what it will use.
    PackageProfileRelationship,
    /// The emulator recorded setup as finished.
    SetupComplete,
    SetupIncomplete,
    SetupNotRecorded,
    /// Play history, memory cards or save states exist.
    UseHistory,
    NoUseHistory,
    /// Per-game configuration exists.
    GameSpecificConfig,
    /// A desktop launcher starts this executable.
    DesktopLauncher,
    /// The configured BIOS/firmware location exists.
    ValidBiosConfiguration,
    BiosLocationBroken,
    /// Profile data that no installed executable owns.
    StaleProfileData,
    /// The remembered/pinned candidate no longer matches what is installed.
    PinnedIdentityMismatch,
    /// Pair derived by the emulator's own rule.
    PairingProven,
    /// An unusable candidate (see `UnusableReason`).
    Unusable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EvidenceEffect {
    Supports,
    Neutral,
    CountsAgainst,
    Disqualifies,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectionEvidence {
    pub kind: EvidenceKind,
    pub effect: EvidenceEffect,
    pub detail: String,
}

impl SelectionEvidence {
    pub fn supports(kind: EvidenceKind, detail: impl Into<String>) -> Self {
        Self::new(kind, EvidenceEffect::Supports, detail)
    }
    pub fn neutral(kind: EvidenceKind, detail: impl Into<String>) -> Self {
        Self::new(kind, EvidenceEffect::Neutral, detail)
    }
    pub fn against(kind: EvidenceKind, detail: impl Into<String>) -> Self {
        Self::new(kind, EvidenceEffect::CountsAgainst, detail)
    }
    pub fn disqualifies(kind: EvidenceKind, detail: impl Into<String>) -> Self {
        Self::new(kind, EvidenceEffect::Disqualifies, detail)
    }
    fn new(kind: EvidenceKind, effect: EvidenceEffect, detail: impl Into<String>) -> Self {
        Self {
            kind,
            effect,
            detail: detail.into(),
        }
    }
}

/// A config folder the emulator uses, resolved read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ProfileFolder {
    Cheats,
    GameSettings,
    BiosSearch,
    Patches,
    Textures,
    MemoryCards,
    SaveStates,
    SaveData,
    GameData,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedFolder {
    pub path: PathBuf,
    pub exists: bool,
    /// The emulator's own configuration moved it away from the default.
    pub custom: bool,
}

/// A configuration problem that was *observed*, never repaired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ConfigurationWarning {
    /// The configured BIOS directory does not exist.
    BiosDirectoryMissing {
        configured: String,
        resolved: PathBuf,
    },
    SetupWizardIncomplete,
    /// A `[Folders]` override points at a folder that does not exist.
    CustomFolderMissing {
        folder: ProfileFolder,
        path: PathBuf,
    },
    /// A configured folder was refused as unsafe.
    FolderUnsafe {
        folder: ProfileFolder,
        reason: String,
    },
    SettingsUnreadable {
        reason: String,
    },
}

/// What the resolver reports next to a resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ResolutionWarning {
    Configuration(ConfigurationWarning),
    /// The user preferred a candidate that was not used.
    PreferredProfileBypassed {
        preferred: CandidateIdentity,
        why: PreferenceBypassReason,
    },
    /// Several executables share the chosen profile and are equivalent.
    EquivalentExecutables {
        others: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PreferenceBypassReason {
    /// The pinned identity matches nothing that is installed now.
    NoLongerPresent,
    /// It is present but unusable.
    Unusable(Vec<UnusableReason>),
}

/// The state of the user's preference at resolution time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PreferenceState {
    /// AUTO and nothing remembered.
    NoPreference,
    /// A preferred candidate was used.
    PreferredHonoured,
    PreferredBypassed(PreferenceBypassReason),
    /// Forced; used exactly.
    Forced,
}

/// Why this candidate was chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum SelectionReason {
    /// Nothing else was viable.
    OnlyViableCandidate,
    /// Beat every viable alternative; decided at this factor.
    StrongestEvidence(RankFactor),
    /// The user's preferred candidate, and it is usable.
    PreferredAndUsable,
    /// The user pinned it.
    ForcedByUser,
    /// Equivalent executables of one profile; the first by path was used.
    EquivalentExecutableOfSameProfile,
}

/// The ordered questions the resolver asks, most decisive first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum RankFactor {
    SetupState,
    UseHistory,
    GameSpecificConfig,
    PortableMarker,
    DesktopLauncher,
    ValidBiosConfiguration,
    UserOverride,
}

impl RankFactor {
    pub const ORDER: [RankFactor; 7] = [
        Self::SetupState,
        Self::UseHistory,
        Self::GameSpecificConfig,
        Self::PortableMarker,
        Self::DesktopLauncher,
        Self::ValidBiosConfiguration,
        Self::UserOverride,
    ];
}

/// Per-factor scores of one candidate. Compared lexicographically in
/// [`RankFactor::ORDER`]; there is no weighted sum to second-guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CandidateRank(pub [u8; 7]);

impl CandidateRank {
    pub fn of(candidate: &EmulatorProfileCandidate) -> Self {
        let has = |kind: EvidenceKind| {
            candidate
                .evidence
                .iter()
                .any(|item| item.kind == kind && item.effect == EvidenceEffect::Supports)
        };
        Self([
            candidate.readiness.setup.rank(),
            u8::from(has(EvidenceKind::UseHistory)),
            u8::from(has(EvidenceKind::GameSpecificConfig)),
            u8::from(has(EvidenceKind::PortableMarker)),
            u8::from(has(EvidenceKind::DesktopLauncher)),
            u8::from(has(EvidenceKind::ValidBiosConfiguration)),
            u8::from(has(EvidenceKind::UserOverrideNamesIt)),
        ])
    }

    /// The first factor (most decisive first) on which `self` differs.
    pub fn first_difference(&self, other: &Self) -> Option<RankFactor> {
        RankFactor::ORDER
            .iter()
            .zip(self.0.iter().zip(other.0.iter()))
            .find(|(_, (left, right))| left != right)
            .map(|(factor, _)| *factor)
    }
}

/// Emulator-specific resolved facts (so consumers never parse config files).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EmulatorDetails {
    DuckStation(DuckStationDetails),
    Ppsspp(PpssppDetails),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DuckStationDetails {
    /// `None` when `[Folders]` was refused as unsafe or settings unreadable.
    pub folders: Option<crate::patch_manager::DuckStationFolders>,
    /// The configured BIOS search directory text, exactly as written.
    pub bios_search_directory_setting: Option<String>,
    pub portable_marker: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PpssppDetails {
    pub memstick: PathBuf,
}

/// A discovered executable/profile pair with everything known about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmulatorProfileCandidate {
    pub identity: CandidateIdentity,
    pub installation: LaunchInstallation,
    pub layout: ProfileLayout,
    pub main_config: PathBuf,
    pub folders: BTreeMap<ProfileFolder, ResolvedFolder>,
    pub readiness: ProfileReadiness,
    pub evidence: Vec<SelectionEvidence>,
    pub unusable: Vec<UnusableReason>,
    pub warnings: Vec<ConfigurationWarning>,
    pub details: Option<EmulatorDetails>,
}

impl EmulatorProfileCandidate {
    pub fn is_usable(&self) -> bool {
        self.unusable.is_empty()
    }
}

/// The single answer every consumer asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedEmulatorProfile {
    pub emulator: InventoryEmulator,
    pub identity: CandidateIdentity,
    pub executable: PathBuf,
    pub installation: LaunchInstallation,
    pub layout: ProfileLayout,
    pub profile_root: PathBuf,
    pub main_config: PathBuf,
    pub folders: BTreeMap<ProfileFolder, ResolvedFolder>,
    pub readiness: ProfileReadiness,
    pub warnings: Vec<ResolutionWarning>,
    pub evidence: Vec<SelectionEvidence>,
    pub mode: SelectionModeKind,
    pub reason: SelectionReason,
    pub preference: PreferenceState,
    pub details: Option<EmulatorDetails>,
}

/// Where a candidate ended up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Standing {
    Selected,
    /// Usable but outranked; `lost_on` is the first factor where it falls
    /// behind the selection.
    Alternative {
        lost_on: RankFactor,
    },
    /// Usable and *stronger* than the selection on `stronger_on`, but the
    /// user's preference or pin chose the other one.
    NotPreferred {
        stronger_on: RankFactor,
    },
    /// Usable and tied with the leader (reported with `Ambiguous`).
    Tied,
    Unusable(Vec<UnusableReason>),
}

/// WHY NOT THE OTHERS: one per candidate considered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateAssessment {
    pub identity: CandidateIdentity,
    pub layout: ProfileLayout,
    pub standing: Standing,
    pub rank: CandidateRank,
    pub evidence: Vec<SelectionEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolution {
    pub profile: ResolvedEmulatorProfile,
    pub assessments: Vec<CandidateAssessment>,
}

/// A refusal under FORCED mode. The resolver never substitutes another
/// candidate, and never repairs anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ForcedRefusal {
    /// The pinned executable/profile is not installed any more.
    ForcedProfileUnavailable {
        pinned: CandidateIdentity,
        reasons: Vec<UnusableReason>,
    },
    /// Present but cannot work (wrong pairing, unreadable configuration).
    ForcedProfileInvalid {
        pinned: CandidateIdentity,
        reasons: Vec<UnusableReason>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ResolutionOutcome {
    Resolved(Box<Resolution>),
    /// Evidence cannot separate these usable candidates; show the choice.
    Ambiguous {
        choices: Vec<CandidateAssessment>,
        assessments: Vec<CandidateAssessment>,
    },
    /// Nothing is installed.
    NoInstallation {
        assessments: Vec<CandidateAssessment>,
    },
    /// Something is installed but no profile is usable.
    NoUsableProfile {
        assessments: Vec<CandidateAssessment>,
    },
    Refused(ForcedRefusal),
}

impl ResolutionOutcome {
    pub fn resolved(&self) -> Option<&Resolution> {
        match self {
            Self::Resolved(resolution) => Some(resolution),
            _ => None,
        }
    }
}
