//! Typed, fail-closed composition of explicitly selected cheats into one launch.
//!
//! This is the `launch` half of the per-launch cheat design
//! (`docs/design/CHEAT_PER_LAUNCH_WORKSPACE_V1.md`). It is a pure planner: it
//! reads no files, spawns nothing and writes nothing. It turns
//!
//! ```text
//! candidates (what is available) + selections (what the user chose) + target facts
//! ```
//!
//! into a [`CheatLaunchPlan`] that is either `NoCheatsSelected` (the launch is
//! unchanged), `Ready`, or `Blocked` with structured reasons. Materialising the
//! plan, spawning, verifying and cleaning up belong to a later executor.
//!
//! Rules enforced here:
//! * **Availability is not enablement.** Only a [`CheatLaunchSelection`]
//!   enters a plan; nothing about a candidate (exact, bundled, installed,
//!   applicable...) can select it.
//! * **Conflicts are never guessed.** Several implementations of one logical
//!   cheat block until one is chosen explicitly; only the chosen one is used.
//! * **Applicability is a gate.** Wrong game/region/revision, malformed and
//!   unsupported block; weak matches need an explicit review acknowledgement
//!   and are never promoted to exact.
//! * **Sources are immutable.** Source ROMs and cheat files appear only as
//!   read-only or must-remain-unchanged references; the emulator is handed a
//!   generated derivative in launch-owned scratch.
//! * **Real state stays real.** Saves and save states are passthrough
//!   grants, never copies; the real emulator config is a protected reference,
//!   never a writable base.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::resource_grants::{
    LaunchAccessScope, LaunchProjectionMethod, LaunchResourceAccess, LaunchResourceGrant,
    LaunchResourceGrantSet, LaunchResourceLifetime, LaunchResourceRole,
};
use super::retroarch_command::RetroArchCommand;
use super::retroarch_resource_projection::approved_retroarch_launch_root;
use crate::patch_manager::{
    CheatApplicabilityReport, CheatApplicabilityState, CheatApplySupport,
    CheatDerivativeEntryRecord, CheatDerivativeError, CheatDerivativeInput,
    CheatLaunchDerivativeKind, CheatRouteTarget, CheatSourceReference, ChtEntry,
    render_retroarch_selected_derivative,
};

// ---------------------------------------------------------------------
// Capability
// ---------------------------------------------------------------------

/// How an emulator can consume cheats for one launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatLaunchMode {
    Unsupported,
    /// Cheats can only be installed persistently (journaled shared install).
    /// This is not launch-scoped and is never presented as such.
    PersistentInstallOnly,
    LaunchScopedConfig,
    LaunchScopedScript,
    LaunchScopedMemoryCommands,
    GuiOnly,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheatLaunchFormat {
    RetroArchCht,
    Other { name: String },
}

/// Whether the safety-relevant behaviour was observed, and on what.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistenceProof {
    Unproven,
    ProvenByHarness {
        emulator_version: String,
        evidence: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatEmulatorCapability {
    pub adapter_id: String,
    pub mode: CheatLaunchMode,
    /// The launch mode is only usable when the workspace honours state fences.
    pub requires_state_fencing: bool,
    pub formats: Vec<CheatLaunchFormat>,
    /// Whether this crate can compose a launch for the adapter today.
    pub composer_implemented: bool,
    pub proof: PersistenceProof,
}

/// Minimal capability facts. Only RetroArch has a composer; everything the
/// existing route table can install persistently is `PersistentInstallOnly`
/// (not launch-scoped); nothing else is claimed. Integration seam: the
/// canonical capability registry proposed in the design replaces this.
#[must_use]
pub fn cheat_launch_capability(adapter_id: &str) -> CheatEmulatorCapability {
    let id = adapter_id.trim().to_ascii_lowercase();
    let base = |mode, requires_state_fencing, formats, composer_implemented, proof| {
        CheatEmulatorCapability {
            adapter_id: id.clone(),
            mode,
            requires_state_fencing,
            formats,
            composer_implemented,
            proof,
        }
    };
    match id.as_str() {
        "" => base(
            CheatLaunchMode::Unknown,
            false,
            vec![],
            false,
            PersistenceProof::Unproven,
        ),
        "retroarch" => base(
            CheatLaunchMode::LaunchScopedConfig,
            true,
            vec![CheatLaunchFormat::RetroArchCht],
            true,
            PersistenceProof::ProvenByHarness {
                emulator_version: "RetroArch 1.22.2 (Flatpak)".into(),
                evidence: "docs/research/RETROARCH_APPENDCONFIG_PERSISTENCE_TEST.md: \
                           appendconfig values leak into the base config; \
                           config_save_on_exit=false suppressed the automatic rewrite \
                           (one core, no content). Overrides and explicit saves not covered."
                    .into(),
            },
        ),
        // A launch-scoped trainer exists for ScummVM, but composing it through
        // this planner is not implemented, so it must not be reported ready.
        "scummvm" => base(
            CheatLaunchMode::LaunchScopedConfig,
            true,
            vec![],
            false,
            PersistenceProof::Unproven,
        ),
        other => {
            match crate::patch_manager::cheat_apply_support(&CheatRouteTarget::standalone(other)) {
                CheatApplySupport::Supported => base(
                    CheatLaunchMode::PersistentInstallOnly,
                    false,
                    vec![],
                    false,
                    PersistenceProof::Unproven,
                ),
                CheatApplySupport::InventoryOnly | CheatApplySupport::Unsupported => base(
                    CheatLaunchMode::Unsupported,
                    false,
                    vec![],
                    false,
                    PersistenceProof::Unproven,
                ),
            }
        }
    }
}

// ---------------------------------------------------------------------
// Applicability gate
// ---------------------------------------------------------------------
//
// There is exactly one applicability model: `patch_manager::cheat_applicability`.
// A variant carries the state `assess_cheat_applicability` produced for the
// selected game; this module only decides what each state means for a launch.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicabilityVerdict {
    Allowed,
    /// Needs an explicit acknowledgement; never becomes an exact match.
    ReviewRequired,
    Blocked,
}

/// Launch policy over all canonical applicability findings, then the summary.
/// Hard refusals cannot be acknowledged away. Only an exact game
/// match (hash/verified identifier) or a fully ready assessment launches
/// without review. A title that merely looks similar, even with a verified
/// platform, is never enough on its own.
#[must_use]
pub fn applicability_verdict(report: &CheatApplicabilityReport) -> ApplicabilityVerdict {
    if report
        .blockers
        .iter()
        .chain(&report.warnings)
        .any(|issue| issue.is_hard_refusal())
    {
        return ApplicabilityVerdict::Blocked;
    }
    let state = report.state;
    use CheatApplicabilityState as S;
    match state {
        S::Ready | S::ExactGameMatch => ApplicabilityVerdict::Allowed,
        S::StrongMatch
        | S::PossibleMatch
        | S::NeedsReview
        | S::MissingRequiredEvidence
        | S::ConflictingVariants => ApplicabilityVerdict::ReviewRequired,
        S::WrongRegion
        | S::WrongRevision
        | S::DifferentGame
        | S::UnsupportedFormat
        | S::UnsupportedEmulator
        | S::Malformed => ApplicabilityVerdict::Blocked,
    }
}

// ---------------------------------------------------------------------
// Inputs: availability vs selection
// ---------------------------------------------------------------------

/// One concrete implementation of a logical cheat.
#[derive(Clone, Debug)]
pub struct CheatVariant {
    pub variant_id: String,
    pub source: CheatSourceReference,
    pub format: CheatLaunchFormat,
    pub applicability: CheatApplicabilityReport,
    /// Parsed entry for RetroArch `.cht` variants. `None` is malformed.
    pub entry: Option<ChtEntry>,
}

/// What is *available*. Being here never enables anything.
#[derive(Clone, Debug)]
pub struct CheatCandidate {
    pub logical_id: String,
    pub title: String,
    pub variants: Vec<CheatVariant>,
    /// Set from reconciliation data that reports unresolved implementations
    /// even when only one variant is listed (Batch 2 seam).
    pub unresolved_conflict: bool,
}

impl CheatCandidate {
    /// Typed replacement for the hand-set conflict flag: true when the
    /// canonical reconciliation group says the implementations differ and a
    /// deliberate choice is required (any review-requiring duplicate kind, or a
    /// same-title/different-code or unproven relationship). Identical and
    /// corroborating duplicates never force a choice.
    #[must_use]
    pub fn requires_choice(group: &crate::patch_manager::CheatReconciliationGroup) -> bool {
        use crate::patch_manager::CheatRelationship as R;
        group
            .classifications
            .iter()
            .any(|kind| kind.requires_review())
            || matches!(
                group.relationship,
                R::SameTitleDifferentCode | R::RelatedUnproven
            )
    }
}

/// What the user *chose*. The only way a cheat enters a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheatLaunchSelection {
    pub logical_id: String,
    /// Required whenever the cheat has several implementations or an
    /// unresolved conflict.
    pub variant_id: Option<String>,
    /// The user reviewed a weak/unevaluated applicability state.
    pub review_acknowledged: bool,
}

#[derive(Clone, Debug)]
pub struct CheatLaunchTarget {
    pub adapter_id: String,
    pub game_identity: String,
    pub identity_verified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetroArchProfileIsolation {
    /// No isolation is available.
    None,
    /// `--config` only. Verified insufficient: auxiliary state (core options,
    /// caches) still lands in the real profile.
    ConfigFileOnly,
    /// The whole writable RetroArch profile is redirected to launch scratch.
    DisposableProfile,
}

/// A per-core/per-content override that will be consulted for this launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetroArchOverrideFinding {
    pub path: PathBuf,
    pub keys: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RetroArchLaunchFacts {
    /// Approved scratch root for this launch. `None` means scratch could not
    /// be established.
    pub launch_root: Option<PathBuf>,
    /// The user's real `retroarch.cfg`; protected, never a writable base.
    pub real_config_path: Option<PathBuf>,
    pub real_save_directory: Option<PathBuf>,
    pub real_state_directory: Option<PathBuf>,
    pub content_path: PathBuf,
    /// The core's `library_name`, used for the cheat file's directory.
    pub core_library_name: String,
    /// A system directory already owned by the BIOS projection, if any.
    pub system_directory: Option<PathBuf>,
    pub profile_isolation: RetroArchProfileIsolation,
    pub effective_overrides: Vec<RetroArchOverrideFinding>,
}

#[derive(Clone, Debug)]
pub struct CheatLaunchRequest {
    pub launch_id: String,
    pub target: CheatLaunchTarget,
    pub candidates: Vec<CheatCandidate>,
    pub selections: Vec<CheatLaunchSelection>,
    pub retroarch: Option<RetroArchLaunchFacts>,
}

// ---------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheatLaunchPlanStatus {
    /// Nothing was selected: the launch is exactly what it would have been.
    NoCheatsSelected,
    Ready,
    Blocked,
}

/// Why a selection or the whole plan cannot proceed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum CheatLaunchBlockReason {
    EmptyLaunchId,
    DuplicateSelection,
    UnknownCheat,
    NoImplementations,
    UnknownVariant {
        variant_id: String,
    },
    /// Several implementations (or reconciliation reports a conflict) and no
    /// explicit choice was made. Nothing is picked automatically.
    UnresolvedConflict {
        variants: Vec<String>,
    },
    Malformed {
        detail: String,
    },
    Applicability {
        state: CheatApplicabilityState,
    },
    ReviewRequired {
        state: CheatApplicabilityState,
    },
    FormatUnsupported {
        format: CheatLaunchFormat,
    },
    UnsupportedTarget {
        adapter_id: String,
        mode: CheatLaunchMode,
    },
    NoComposer {
        adapter_id: String,
    },
    FactsMissing,
    ScratchUnavailable,
    ProtectedConfigUnclear,
    SavePathInvalid {
        which: String,
    },
    ContentPathInvalid,
    ProfileIsolationInsufficient {
        provided: RetroArchProfileIsolation,
    },
    OverrideDefeatsSetting {
        path: PathBuf,
        key: String,
    },
    DerivativeFailed {
        detail: String,
    },
    GrantsInvalid {
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedCheat {
    pub logical_id: String,
    pub variant_id: Option<String>,
    pub reason: CheatLaunchBlockReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedCheat {
    pub logical_id: String,
    pub variant_id: String,
    pub title: String,
    pub source: CheatSourceReference,
    pub format: CheatLaunchFormat,
    pub applicability: CheatApplicabilityState,
    pub review_acknowledged: bool,
}

/// A generated derivative and where it will live (launch-owned scratch).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedDerivative {
    pub kind: CheatLaunchDerivativeKind,
    pub destination: PathBuf,
    pub sha256: String,
    pub bytes: Vec<u8>,
    pub entries: Vec<CheatDerivativeEntryRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetroArchCheatLaunchSettings {
    pub base_config_path: PathBuf,
    pub base_config_contents: String,
    pub cheats_directory: PathBuf,
    /// Arguments to append to the existing argv (`--config <scratch base>`).
    pub extra_arguments: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchStateClass {
    ReadOnlySource,
    RealUserState,
    EphemeralConfig,
    EphemeralRuntime,
    ProtectedConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateExpectation {
    MustRemainUnchanged,
    /// Normal saves and states: change is expected and never an error.
    MayChange,
    MustNotExistAfter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FingerprintKind {
    /// Length + modified time. Cheap; for large media.
    FileIdentity,
    /// Content hash. Small files only (see [`MAX_HASHED_FILE_BYTES`]).
    Sha256,
    /// Named `key = "value"` entries only; for configs an emulator rewrites.
    KeyProbe {
        keys: Vec<String>,
    },
    /// Still exists and was not emptied. Never hashes a directory.
    ExistsNotTruncated,
    NotPresent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationSeverity {
    Info,
    Warning,
    LaunchAffecting,
    Corruption,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchStateExpectation {
    pub path: PathBuf,
    pub class: LaunchStateClass,
    pub expectation: StateExpectation,
    pub fingerprint: FingerprintKind,
    pub severity: ViolationSeverity,
}

/// A persistent write the plan knowingly performs. Empty by default: a
/// launch-scoped plan makes no persistent writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedPersistentWrite {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheatLaunchPlan {
    pub launch_id: String,
    pub adapter_id: String,
    pub game_identity: String,
    pub status: CheatLaunchPlanStatus,
    pub capability: CheatEmulatorCapability,
    pub selected: Vec<PlannedCheat>,
    pub blocked: Vec<BlockedCheat>,
    /// Plan-level blocks not attributable to one cheat (ownership, scratch...).
    pub plan_blocks: Vec<CheatLaunchBlockReason>,
    pub derivative: Option<PlannedDerivative>,
    pub grants: LaunchResourceGrantSet,
    pub expectations: Vec<LaunchStateExpectation>,
    pub retroarch: Option<RetroArchCheatLaunchSettings>,
    pub persistent_writes: Vec<ExpectedPersistentWrite>,
    /// Real emulator configuration files the plan would mutate. Always empty.
    pub global_config_mutations: Vec<PathBuf>,
}

impl CheatLaunchPlan {
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.status == CheatLaunchPlanStatus::Ready
    }

    fn empty(request: &CheatLaunchRequest, capability: CheatEmulatorCapability) -> Self {
        Self {
            launch_id: request.launch_id.clone(),
            adapter_id: capability.adapter_id.clone(),
            game_identity: request.target.game_identity.clone(),
            status: CheatLaunchPlanStatus::NoCheatsSelected,
            capability,
            selected: Vec::new(),
            blocked: Vec::new(),
            plan_blocks: Vec::new(),
            derivative: None,
            grants: LaunchResourceGrantSet::default(),
            expectations: Vec::new(),
            retroarch: None,
            persistent_writes: Vec::new(),
            global_config_mutations: Vec::new(),
        }
    }
}

/// Keys the generated RetroArch launch config must own. A user override that
/// sets any of them would defeat the launch's safety guarantees.
pub const RETROARCH_MANDATORY_KEYS: &[&str] = &[
    "config_save_on_exit",
    "savefile_directory",
    "savestate_directory",
    "cheat_database_path",
    "apply_cheats_after_load",
    "auto_overrides_enable",
];

/// Largest file hashed for a state expectation.
pub const MAX_HASHED_FILE_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------------
// Planner
// ---------------------------------------------------------------------

/// Plans one launch's cheats. Pure and deterministic.
#[must_use]
pub fn plan_cheat_launch(request: &CheatLaunchRequest) -> CheatLaunchPlan {
    let capability = cheat_launch_capability(&request.target.adapter_id);
    let mut plan = CheatLaunchPlan::empty(request, capability.clone());

    if request.selections.is_empty() {
        return plan;
    }
    plan.status = CheatLaunchPlanStatus::Blocked;
    if request.launch_id.trim().is_empty() {
        plan.plan_blocks.push(CheatLaunchBlockReason::EmptyLaunchId);
        return plan;
    }

    let mut ordered: Vec<&CheatLaunchSelection> = request.selections.iter().collect();
    ordered.sort_by(|a, b| a.logical_id.cmp(&b.logical_id));

    let target_usable = matches!(capability.mode, CheatLaunchMode::LaunchScopedConfig)
        && capability.composer_implemented;
    let mut seen = BTreeSet::new();
    let mut inputs: Vec<(PlannedCheat, ChtEntry)> = Vec::new();

    for selection in ordered {
        let block = |plan: &mut CheatLaunchPlan, variant: Option<String>, reason| {
            plan.blocked.push(BlockedCheat {
                logical_id: selection.logical_id.clone(),
                variant_id: variant,
                reason,
            });
        };
        if !seen.insert(selection.logical_id.clone()) {
            block(&mut plan, None, CheatLaunchBlockReason::DuplicateSelection);
            continue;
        }
        if !target_usable {
            let reason = if capability.composer_implemented {
                CheatLaunchBlockReason::UnsupportedTarget {
                    adapter_id: capability.adapter_id.clone(),
                    mode: capability.mode,
                }
            } else if matches!(capability.mode, CheatLaunchMode::LaunchScopedConfig) {
                CheatLaunchBlockReason::NoComposer {
                    adapter_id: capability.adapter_id.clone(),
                }
            } else {
                CheatLaunchBlockReason::UnsupportedTarget {
                    adapter_id: capability.adapter_id.clone(),
                    mode: capability.mode,
                }
            };
            block(&mut plan, selection.variant_id.clone(), reason);
            continue;
        }
        match resolve_selection(request, selection, &capability) {
            Ok((planned, entry)) => inputs.push((planned, entry)),
            Err((variant, reason)) => block(&mut plan, variant, reason),
        }
    }

    plan.selected = inputs.iter().map(|(planned, _)| planned.clone()).collect();

    if target_usable {
        match request.retroarch.as_ref() {
            None => plan.plan_blocks.push(CheatLaunchBlockReason::FactsMissing),
            Some(facts) => check_retroarch_facts(facts, &mut plan.plan_blocks),
        }
    }
    if !plan.blocked.is_empty() || !plan.plan_blocks.is_empty() {
        return plan;
    }
    let Some(facts) = request.retroarch.as_ref() else {
        return plan;
    };

    match compose_retroarch(request, facts, &inputs) {
        Ok(composition) => {
            plan.derivative = Some(composition.derivative);
            plan.grants = composition.grants;
            plan.expectations = composition.expectations;
            plan.retroarch = Some(composition.settings);
            plan.status = CheatLaunchPlanStatus::Ready;
        }
        Err(reason) => plan.plan_blocks.push(reason),
    }
    plan
}

type SelectionError = (Option<String>, CheatLaunchBlockReason);

fn resolve_selection(
    request: &CheatLaunchRequest,
    selection: &CheatLaunchSelection,
    capability: &CheatEmulatorCapability,
) -> Result<(PlannedCheat, ChtEntry), SelectionError> {
    let Some(candidate) = request
        .candidates
        .iter()
        .find(|candidate| candidate.logical_id == selection.logical_id)
    else {
        return Err((None, CheatLaunchBlockReason::UnknownCheat));
    };
    if candidate.variants.is_empty() {
        return Err((None, CheatLaunchBlockReason::NoImplementations));
    }
    let variant = match selection.variant_id.as_deref() {
        Some(wanted) => candidate
            .variants
            .iter()
            .find(|variant| variant.variant_id == wanted)
            .ok_or_else(|| {
                (
                    Some(wanted.to_string()),
                    CheatLaunchBlockReason::UnknownVariant {
                        variant_id: wanted.to_string(),
                    },
                )
            })?,
        None => {
            if candidate.variants.len() > 1 || candidate.unresolved_conflict {
                let mut variants: Vec<String> = candidate
                    .variants
                    .iter()
                    .map(|variant| variant.variant_id.clone())
                    .collect();
                variants.sort();
                return Err((
                    None,
                    CheatLaunchBlockReason::UnresolvedConflict { variants },
                ));
            }
            &candidate.variants[0]
        }
    };
    let id = Some(variant.variant_id.clone());
    if !capability.formats.contains(&variant.format) {
        return Err((
            id,
            CheatLaunchBlockReason::FormatUnsupported {
                format: variant.format.clone(),
            },
        ));
    }
    let Some(entry) = variant.entry.as_ref().filter(|entry| entry.is_selectable()) else {
        return Err((
            id,
            CheatLaunchBlockReason::Malformed {
                detail: "the entry has no usable code or has a blocking warning".into(),
            },
        ));
    };
    match applicability_verdict(&variant.applicability) {
        ApplicabilityVerdict::Blocked => {
            return Err((
                id,
                CheatLaunchBlockReason::Applicability {
                    state: variant.applicability.state,
                },
            ));
        }
        ApplicabilityVerdict::ReviewRequired if !selection.review_acknowledged => {
            return Err((
                id,
                CheatLaunchBlockReason::ReviewRequired {
                    state: variant.applicability.state,
                },
            ));
        }
        _ => {}
    }
    Ok((
        PlannedCheat {
            logical_id: candidate.logical_id.clone(),
            variant_id: variant.variant_id.clone(),
            title: candidate.title.clone(),
            source: variant.source.clone(),
            format: variant.format.clone(),
            applicability: variant.applicability.state,
            review_acknowledged: selection.review_acknowledged,
        },
        entry.clone(),
    ))
}

fn safe_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn check_retroarch_facts(facts: &RetroArchLaunchFacts, blocks: &mut Vec<CheatLaunchBlockReason>) {
    let approved = approved_retroarch_launch_root();
    match facts.launch_root.as_deref() {
        Some(root) if safe_absolute(root) && root != approved && root.starts_with(&approved) => {}
        _ => blocks.push(CheatLaunchBlockReason::ScratchUnavailable),
    }
    match facts.real_config_path.as_deref() {
        Some(path) if safe_absolute(path) && path.file_name().is_some() => {
            if facts
                .launch_root
                .as_deref()
                .is_some_and(|root| path.starts_with(root))
            {
                blocks.push(CheatLaunchBlockReason::ProtectedConfigUnclear);
            }
        }
        _ => blocks.push(CheatLaunchBlockReason::ProtectedConfigUnclear),
    }
    for (which, path) in [
        ("save", facts.real_save_directory.as_deref()),
        ("state", facts.real_state_directory.as_deref()),
    ] {
        let valid = path.is_some_and(|path| {
            safe_absolute(path)
                && !facts
                    .launch_root
                    .as_deref()
                    .is_some_and(|root| path.starts_with(root))
        });
        if !valid {
            blocks.push(CheatLaunchBlockReason::SavePathInvalid {
                which: which.into(),
            });
        }
    }
    if !safe_absolute(&facts.content_path) || facts.content_path.file_stem().is_none() {
        blocks.push(CheatLaunchBlockReason::ContentPathInvalid);
    }
    if facts.profile_isolation != RetroArchProfileIsolation::DisposableProfile {
        blocks.push(CheatLaunchBlockReason::ProfileIsolationInsufficient {
            provided: facts.profile_isolation,
        });
    }
    for finding in &facts.effective_overrides {
        for key in &finding.keys {
            if RETROARCH_MANDATORY_KEYS.contains(&key.as_str()) {
                blocks.push(CheatLaunchBlockReason::OverrideDefeatsSetting {
                    path: finding.path.clone(),
                    key: key.clone(),
                });
            }
        }
    }
}

struct RetroArchComposition {
    derivative: PlannedDerivative,
    grants: LaunchResourceGrantSet,
    expectations: Vec<LaunchStateExpectation>,
    settings: RetroArchCheatLaunchSettings,
}

fn compose_retroarch(
    request: &CheatLaunchRequest,
    facts: &RetroArchLaunchFacts,
    inputs: &[(PlannedCheat, ChtEntry)],
) -> Result<RetroArchComposition, CheatLaunchBlockReason> {
    let root = facts
        .launch_root
        .as_ref()
        .ok_or(CheatLaunchBlockReason::ScratchUnavailable)?;
    let (Some(real_config), Some(save_dir), Some(state_dir)) = (
        facts.real_config_path.as_ref(),
        facts.real_save_directory.as_ref(),
        facts.real_state_directory.as_ref(),
    ) else {
        return Err(CheatLaunchBlockReason::FactsMissing);
    };
    let content_basename = facts
        .content_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or(CheatLaunchBlockReason::ContentPathInvalid)?;

    let derivative_inputs: Vec<CheatDerivativeInput<'_>> = inputs
        .iter()
        .map(|(planned, entry)| CheatDerivativeInput {
            logical_id: &planned.logical_id,
            variant_id: &planned.variant_id,
            source: &planned.source,
            entry,
        })
        .collect();
    let derivative = render_retroarch_selected_derivative(
        &facts.core_library_name,
        content_basename,
        &derivative_inputs,
    )
    .map_err(
        |error: CheatDerivativeError| CheatLaunchBlockReason::DerivativeFailed {
            detail: format!("{error:?}"),
        },
    )?;

    let ra = root.join("retroarch");
    let profile = ra.join("profile");
    let base_config = profile.join("retroarch.cfg");
    let cheats_dir = ra.join("cheats");
    let cheat_file = cheats_dir.join(&derivative.relative_name);
    let system_dir = facts
        .system_directory
        .clone()
        .unwrap_or_else(|| profile.join("system"));

    let path_text = |path: &Path| path.to_string_lossy().into_owned();
    let config: Vec<(&str, String)> = vec![
        ("config_save_on_exit", "false".into()),
        ("auto_overrides_enable", "false".into()),
        ("savefile_directory", path_text(save_dir)),
        ("savestate_directory", path_text(state_dir)),
        ("system_directory", path_text(&system_dir)),
        ("cheat_database_path", path_text(&cheats_dir)),
        ("apply_cheats_after_load", "true".into()),
        (
            "core_options_path",
            path_text(&profile.join("core_options.opt")),
        ),
        ("rgui_config_directory", path_text(&profile.join("config"))),
        (
            "content_history_path",
            path_text(&profile.join("content_history.lpl")),
        ),
        ("playlist_directory", path_text(&profile.join("playlists"))),
        ("cache_directory", path_text(&profile.join("cache"))),
        ("log_dir", path_text(&profile.join("logs"))),
    ];
    let mut contents = String::new();
    for (key, value) in &config {
        contents.push_str(&format!("{key} = \"{value}\"\n"));
    }

    let launch_id = request.launch_id.clone();
    let grant = |role,
                 source: Option<&Path>,
                 presented: &Path,
                 access,
                 projection,
                 lifetime,
                 scope,
                 reason: &str| {
        LaunchResourceGrant {
            launch_id: launch_id.clone(),
            role,
            source_path: source.map(Path::to_path_buf),
            presented_path: Some(presented.to_path_buf()),
            access,
            projection,
            lifetime,
            scope,
            provenance: "cheat_launch_plan".into(),
            reason: reason.into(),
        }
    };
    let mut grants = LaunchResourceGrantSet::default();
    let list = [
        grant(
            LaunchResourceRole::GameMedia,
            Some(&facts.content_path),
            &facts.content_path,
            LaunchResourceAccess::ReadOnly,
            LaunchProjectionMethod::DirectPath,
            LaunchResourceLifetime::LaunchOnly,
            LaunchAccessScope::StrictMinimal,
            "content is read-only source media",
        ),
        grant(
            LaunchResourceRole::SaveData,
            Some(save_dir),
            save_dir,
            LaunchResourceAccess::ReadWrite,
            LaunchProjectionMethod::DirectPath,
            LaunchResourceLifetime::Persistent,
            LaunchAccessScope::NarrowDirectory,
            "real saves pass through; never copied",
        ),
        grant(
            LaunchResourceRole::SaveData,
            Some(state_dir),
            state_dir,
            LaunchResourceAccess::ReadWrite,
            LaunchProjectionMethod::DirectPath,
            LaunchResourceLifetime::Persistent,
            LaunchAccessScope::NarrowDirectory,
            "real save states pass through; never copied",
        ),
        grant(
            LaunchResourceRole::TemporaryRuntime,
            None,
            &profile,
            LaunchResourceAccess::CreateOnly,
            LaunchProjectionMethod::GeneratedDirectory,
            LaunchResourceLifetime::LaunchOnly,
            LaunchAccessScope::NarrowDirectory,
            "disposable RetroArch profile (auxiliary writable state)",
        ),
        grant(
            LaunchResourceRole::Config,
            None,
            &base_config,
            LaunchResourceAccess::CreateOnly,
            LaunchProjectionMethod::GeneratedFile,
            LaunchResourceLifetime::LaunchOnly,
            LaunchAccessScope::StrictMinimal,
            "generated launch base config; the real retroarch.cfg stays protected",
        ),
        grant(
            LaunchResourceRole::CheatMaterial,
            None,
            &cheats_dir,
            LaunchResourceAccess::CreateOnly,
            LaunchProjectionMethod::GeneratedDirectory,
            LaunchResourceLifetime::LaunchOnly,
            LaunchAccessScope::NarrowDirectory,
            "launch-owned cheat directory",
        ),
        grant(
            LaunchResourceRole::CheatMaterial,
            None,
            &cheat_file,
            LaunchResourceAccess::CreateOnly,
            LaunchProjectionMethod::GeneratedFile,
            LaunchResourceLifetime::LaunchOnly,
            LaunchAccessScope::StrictMinimal,
            "selected-only derivative generated from source cheats",
        ),
    ];
    for item in list {
        grants
            .try_insert(item)
            .map_err(|error| CheatLaunchBlockReason::GrantsInvalid {
                detail: format!("{error:?}"),
            })?;
    }
    if facts.system_directory.is_none() {
        grants
            .try_insert(grant(
                LaunchResourceRole::TemporaryRuntime,
                None,
                &system_dir,
                LaunchResourceAccess::CreateOnly,
                LaunchProjectionMethod::GeneratedDirectory,
                LaunchResourceLifetime::LaunchOnly,
                LaunchAccessScope::NarrowDirectory,
                "scratch system directory (no BIOS linked by this plan)",
            ))
            .map_err(|error| CheatLaunchBlockReason::GrantsInvalid {
                detail: format!("{error:?}"),
            })?;
    }
    grants
        .validate()
        .map_err(|error| CheatLaunchBlockReason::GrantsInvalid {
            detail: format!("{error:?}"),
        })?;

    let mut expectations = vec![
        LaunchStateExpectation {
            path: facts.content_path.clone(),
            class: LaunchStateClass::ReadOnlySource,
            expectation: StateExpectation::MustRemainUnchanged,
            fingerprint: FingerprintKind::FileIdentity,
            severity: ViolationSeverity::Corruption,
        },
        LaunchStateExpectation {
            path: real_config.clone(),
            class: LaunchStateClass::ProtectedConfig,
            expectation: StateExpectation::MustRemainUnchanged,
            fingerprint: FingerprintKind::Sha256,
            severity: ViolationSeverity::Warning,
        },
        LaunchStateExpectation {
            path: real_config.clone(),
            class: LaunchStateClass::ProtectedConfig,
            expectation: StateExpectation::MustRemainUnchanged,
            fingerprint: FingerprintKind::KeyProbe {
                keys: RETROARCH_MANDATORY_KEYS
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect(),
            },
            severity: ViolationSeverity::LaunchAffecting,
        },
    ];
    let source_files: BTreeSet<&PathBuf> = inputs
        .iter()
        .filter_map(|(planned, _)| planned.source.source_path.as_ref())
        .collect();
    for path in source_files {
        expectations.push(LaunchStateExpectation {
            path: path.clone(),
            class: LaunchStateClass::ReadOnlySource,
            expectation: StateExpectation::MustRemainUnchanged,
            fingerprint: FingerprintKind::Sha256,
            severity: ViolationSeverity::Warning,
        });
    }
    for dir in [save_dir, state_dir] {
        expectations.push(LaunchStateExpectation {
            path: dir.clone(),
            class: LaunchStateClass::RealUserState,
            expectation: StateExpectation::MayChange,
            fingerprint: FingerprintKind::ExistsNotTruncated,
            severity: ViolationSeverity::Corruption,
        });
    }
    for (path, class) in [
        (&cheat_file, LaunchStateClass::EphemeralConfig),
        (&base_config, LaunchStateClass::EphemeralConfig),
        (root, LaunchStateClass::EphemeralRuntime),
    ] {
        expectations.push(LaunchStateExpectation {
            path: path.clone(),
            class,
            expectation: StateExpectation::MustNotExistAfter,
            fingerprint: FingerprintKind::NotPresent,
            severity: ViolationSeverity::Warning,
        });
    }
    // Deterministic, de-duplicated order.
    expectations.sort_by(|a, b| {
        (&a.path, format!("{:?}", a.fingerprint)).cmp(&(&b.path, format!("{:?}", b.fingerprint)))
    });
    expectations.dedup();

    Ok(RetroArchComposition {
        derivative: PlannedDerivative {
            kind: derivative.kind,
            destination: cheat_file,
            sha256: derivative.sha256,
            bytes: derivative.bytes,
            entries: derivative.entries,
        },
        grants,
        expectations,
        settings: RetroArchCheatLaunchSettings {
            base_config_path: base_config.clone(),
            base_config_contents: contents,
            cheats_directory: cheats_dir,
            extra_arguments: vec!["--config".into(), path_text(&base_config)],
        },
    })
}

/// Why a plan cannot be applied to a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheatLaunchCommandError {
    PlanBlocked,
    MissingSettings,
}

/// Applies a plan to an already-built RetroArch command. With no cheats
/// selected the command is returned unchanged; a blocked plan never yields a
/// command.
pub fn command_with_cheat_launch_plan(
    command: &RetroArchCommand,
    plan: &CheatLaunchPlan,
) -> Result<RetroArchCommand, CheatLaunchCommandError> {
    match plan.status {
        CheatLaunchPlanStatus::NoCheatsSelected => Ok(command.clone()),
        CheatLaunchPlanStatus::Blocked => Err(CheatLaunchCommandError::PlanBlocked),
        CheatLaunchPlanStatus::Ready => {
            let settings = plan
                .retroarch
                .as_ref()
                .ok_or(CheatLaunchCommandError::MissingSettings)?;
            let mut command = command.clone();
            command
                .arguments
                .extend(settings.extra_arguments.iter().map(OsString::from));
            Ok(command)
        }
    }
}

// ---------------------------------------------------------------------
// Targeted baseline capture and verification (no directory hashing)
// ---------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateBaseline {
    pub path: PathBuf,
    pub exists: bool,
    pub len: Option<u64>,
    pub modified_nanos: Option<u128>,
    pub sha256: Option<String>,
    pub keys: BTreeMap<String, Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateOutcome {
    Unchanged,
    /// Allowed change (saves) or nothing to compare.
    Changed {
        detail: String,
    },
    Violation {
        severity: ViolationSeverity,
        detail: String,
    },
}

fn hash_small_file(path: &Path, len: u64) -> Option<String> {
    if len > MAX_HASHED_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let mut digest = Sha256::new();
    digest.update(&bytes);
    Some(
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn read_keys(path: &Path, keys: &[String]) -> BTreeMap<String, Option<String>> {
    let mut found: BTreeMap<String, Option<String>> =
        keys.iter().map(|key| (key.clone(), None)).collect();
    let Ok(text) = std::fs::read_to_string(path) else {
        return found;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if let Some(slot) = found.get_mut(key) {
            *slot = Some(value.trim().trim_matches('"').to_string());
        }
    }
    found
}

/// Captures only what the expectation needs. Never walks a directory.
#[must_use]
pub fn capture_baseline(expectation: &LaunchStateExpectation) -> StateBaseline {
    let metadata = std::fs::symlink_metadata(&expectation.path).ok();
    let len = metadata.as_ref().map(std::fs::Metadata::len);
    let modified_nanos = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    let is_file = metadata.as_ref().is_some_and(std::fs::Metadata::is_file);
    let sha256 = match (&expectation.fingerprint, len) {
        (FingerprintKind::Sha256, Some(len)) if is_file => hash_small_file(&expectation.path, len),
        _ => None,
    };
    let keys = match &expectation.fingerprint {
        FingerprintKind::KeyProbe { keys } => read_keys(&expectation.path, keys),
        _ => BTreeMap::new(),
    };
    StateBaseline {
        path: expectation.path.clone(),
        exists: metadata.is_some(),
        len,
        modified_nanos,
        sha256,
        keys,
    }
}

/// Compares the current state with a baseline captured before launch.
#[must_use]
pub fn verify_expectation(
    expectation: &LaunchStateExpectation,
    baseline: &StateBaseline,
) -> StateOutcome {
    let now = capture_baseline(expectation);
    let violation = |detail: &str| StateOutcome::Violation {
        severity: expectation.severity,
        detail: detail.into(),
    };
    match expectation.expectation {
        StateExpectation::MustNotExistAfter => {
            if now.exists {
                violation("scratch path was left behind")
            } else {
                StateOutcome::Unchanged
            }
        }
        StateExpectation::MayChange => match expectation.fingerprint {
            FingerprintKind::ExistsNotTruncated => {
                if baseline.exists && !now.exists {
                    violation("real user state disappeared")
                } else if baseline.exists
                    && baseline
                        .len
                        .zip(now.len)
                        .is_some_and(|(before, after)| after == 0 && before > 0)
                {
                    violation("real user state was emptied")
                } else if (baseline.len, baseline.modified_nanos) != (now.len, now.modified_nanos) {
                    StateOutcome::Changed {
                        detail: "changed (expected for user state)".into(),
                    }
                } else {
                    StateOutcome::Unchanged
                }
            }
            _ => StateOutcome::Unchanged,
        },
        StateExpectation::MustRemainUnchanged => {
            if baseline.exists != now.exists {
                return violation("existence changed");
            }
            match &expectation.fingerprint {
                FingerprintKind::FileIdentity => {
                    if (baseline.len, baseline.modified_nanos) == (now.len, now.modified_nanos) {
                        StateOutcome::Unchanged
                    } else {
                        violation("length or modified time changed")
                    }
                }
                FingerprintKind::Sha256 => match (&baseline.sha256, &now.sha256) {
                    (Some(before), Some(after)) if before == after => StateOutcome::Unchanged,
                    (Some(_), Some(_)) => violation("content hash changed"),
                    _ if (baseline.len, baseline.modified_nanos)
                        == (now.len, now.modified_nanos) =>
                    {
                        StateOutcome::Unchanged
                    }
                    _ => violation("file too large or unreadable to hash and its identity changed"),
                },
                FingerprintKind::KeyProbe { .. } => {
                    if baseline.keys == now.keys {
                        StateOutcome::Unchanged
                    } else {
                        violation("a protected key changed")
                    }
                }
                FingerprintKind::ExistsNotTruncated | FingerprintKind::NotPresent => {
                    StateOutcome::Unchanged
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
