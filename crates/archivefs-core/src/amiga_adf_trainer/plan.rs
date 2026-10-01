//! Assessment and planning for non-WHDLoad Amiga trainers.
//!
//! Amiga-specific gates run first (mechanism, exact identity, platform, exact
//! image, disk and set targeting, alternate media). Entries that pass are then
//! handed to the *canonical* cheat machinery: `reconcile_cheats_for_game` for
//! duplicates and conflicts, `resolve_reviewed_cheat_plan` for explicit user
//! choices, and `assess_cheat_applicability` for identity/region/revision.
//! There is no Amiga-specific duplicate engine and no parallel identity model.
//!
//! A plan is data. It never opens the image for writing, spawns a process, or
//! claims a cheat works: the best a verified memory-write trainer reaches today
//! is [`AmigaTrainerStatus::RequiresEmulatorRuntime`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::import::AmigaTrainerImport;
use super::media::{AdfMedia, AlternateMedia};
use super::model::*;
use crate::game_identity::{IdentityEvidence, IdentityKind, IdentityStatus};
use crate::media_set::{MediaSet, MediaSetState, OrdinalUnit};
use crate::patch_manager::{
    CheatApplicability, CheatApplicabilityInput, CheatApplicabilityIssue, CheatApplicabilityState,
    CheatDocument, CheatDuplicateKind, CheatGameAssociation, CheatOperation, CheatParseEvidence,
    CheatPlatform, CheatReconciliationEntry, CheatReconciliationOutcome, CheatRecordProvenance,
    CheatReleaseEvidence, CheatReviewChoice, CheatSelectedGame, CheatSourceFormat,
    ResolvedCheatPlanRequest, assess_cheat_applicability, reconcile_cheats_for_game,
    resolve_reviewed_cheat_plan,
};

/// Everything known about the image the trainer would be prepared for.
pub struct AmigaTrainerContext<'a> {
    pub media: &'a AdfMedia,
    /// Identity evidence. Defaults to what [`AdfMedia`] supports; callers (and
    /// tests) may supply different evidence, and it is checked against the bytes.
    pub facts: Vec<IdentityEvidence>,
    /// The canonical media set this image belongs to, when one was resolved.
    pub media_set: Option<&'a MediaSet>,
}

impl<'a> AmigaTrainerContext<'a> {
    #[must_use]
    pub fn for_media(media: &'a AdfMedia) -> Self {
        Self {
            media,
            facts: media.identity_facts(),
            media_set: None,
        }
    }

    #[must_use]
    pub fn with_media_set(mut self, set: &'a MediaSet) -> Self {
        self.media_set = Some(set);
        self
    }
}

/// Why a trainer is not (yet) usable. Hard blockers mean it does not apply to
/// this target; soft blockers mean evidence is missing or a choice is needed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum AmigaTrainerBlocker {
    Mechanism(AmigaTrainerMechanism),
    PlatformMismatch { claimed: String },
    MediaNotTargeted,
    AlternateMedia(AlternateMedia),
    WrongDisk { wanted: u16, found: u16 },
    ReleaseMismatch,
    RegionMismatch,
    RevisionMismatch,
    CanonicalRefusal(CheatApplicabilityIssue),
    // Soft: identity not established.
    IdentityNotVerified,
    IdentityEmpty,
    IdentityMismatch,
    IdentityConflicting,
    // Soft: targeting evidence missing.
    DiskUnconfirmed,
    SetRequired,
    SetIncomplete,
    SetOrderUnverified,
    SetAmbiguous,
    SetUnverified,
    ReleaseEvidenceUnknown(&'static str),
    TooManyForGame,
    // Soft: duplicate/conflict handling.
    ConflictNeedsChoice,
    NotChosen,
}

impl AmigaTrainerBlocker {
    #[must_use]
    pub const fn is_hard(&self) -> bool {
        matches!(
            self,
            Self::Mechanism(_)
                | Self::PlatformMismatch { .. }
                | Self::MediaNotTargeted
                | Self::AlternateMedia(_)
                | Self::WrongDisk { .. }
                | Self::ReleaseMismatch
                | Self::RegionMismatch
                | Self::RevisionMismatch
                | Self::CanonicalRefusal(_)
        )
    }
}

/// What the explicit-choice step did with a trainer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum AmigaTrainerSelection {
    NotEvaluated,
    Selected,
    /// Identical to another trainer that is selected; provenance is kept.
    DuplicateOf(usize),
    NeedsChoice,
    NotChosen,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AmigaTrainerAssessment {
    pub index: usize,
    pub title: String,
    pub mechanism: AmigaTrainerMechanism,
    pub status: AmigaTrainerStatus,
    pub blockers: Vec<AmigaTrainerBlocker>,
    pub selection: AmigaTrainerSelection,
    /// The canonical applicability state, when the trainer reached that stage.
    pub canonical_state: Option<CheatApplicabilityState>,
}

impl AmigaTrainerAssessment {
    /// True only when a complete, verified plan exists for this trainer.
    #[must_use]
    pub fn can_plan(&self) -> bool {
        matches!(
            self.status,
            AmigaTrainerStatus::RequiresEmulatorRuntime | AmigaTrainerStatus::Preparable
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AmigaConflictGroup {
    /// Index in the canonical reconciliation result; the key for a user choice.
    pub group_index: usize,
    pub trainers: Vec<usize>,
    pub kinds: Vec<CheatDuplicateKind>,
}

/// The original image is only ever read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SourceMediaPolicy {
    ReadOnlyOriginal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AmigaTrainerPlan {
    pub media_path: PathBuf,
    pub media_sha256: String,
    /// The disk number the canonical media set assigns this image, if known.
    pub disk_ordinal: Option<u16>,
    pub set_state: Option<MediaSetState>,
    pub source_policy: SourceMediaPolicy,
    /// No current mechanism needs writable media; one that does is Unsupported.
    pub requires_scratch_copy: bool,
    pub assessments: Vec<AmigaTrainerAssessment>,
    pub conflict_groups: Vec<AmigaConflictGroup>,
    /// Trainers with a complete plan and an explicit or automatic selection.
    pub selected: Vec<usize>,
    pub runtime: [AmigaRuntimeOption; 3],
    /// Trainers beyond the per-game limit that were not evaluated further.
    pub over_limit: usize,
}

impl AmigaTrainerPlan {
    #[must_use]
    pub fn count(&self, status: AmigaTrainerStatus) -> usize {
        self.assessments
            .iter()
            .filter(|a| a.status == status)
            .count()
    }

    #[must_use]
    pub fn assessment(&self, index: usize) -> Option<&AmigaTrainerAssessment> {
        self.assessments.iter().find(|a| a.index == index)
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn claimed_platform_is_amiga(platform: &str) -> bool {
    crate::canonical_platform_for_alias(platform) == Some("Amiga")
}

/// Identity gate: a verified, concrete, non-empty exact-image hash that equals
/// the bytes actually measured, plus a verified Amiga platform. Names, titles and
/// volume labels never enter into it.
fn identity_blockers(ctx: &AmigaTrainerContext<'_>) -> Vec<AmigaTrainerBlocker> {
    let mut out = Vec::new();
    let hashes: Vec<&IdentityEvidence> = ctx
        .facts
        .iter()
        .filter(|f| f.kind == IdentityKind::LooseRomSha256)
        .collect();
    if hashes.iter().any(|f| f.status == IdentityStatus::Ambiguous) {
        out.push(AmigaTrainerBlocker::IdentityConflicting);
    }
    let verified: Vec<&&IdentityEvidence> = hashes
        .iter()
        .filter(|f| f.status == IdentityStatus::Verified)
        .collect();
    if verified.is_empty() {
        if !out.contains(&AmigaTrainerBlocker::IdentityConflicting) {
            out.push(AmigaTrainerBlocker::IdentityNotVerified);
        }
    } else {
        let mut distinct: Vec<String> = Vec::new();
        for f in &verified {
            match f.value.as_deref().map(str::trim) {
                None | Some("") => out.push(AmigaTrainerBlocker::IdentityEmpty),
                Some(v) if !is_sha256(v) => out.push(AmigaTrainerBlocker::IdentityMismatch),
                Some(v) => {
                    let v = v.to_ascii_lowercase();
                    if !distinct.contains(&v) {
                        distinct.push(v);
                    }
                }
            }
        }
        if distinct.len() > 1 {
            out.push(AmigaTrainerBlocker::IdentityConflicting);
        } else if let Some(only) = distinct.first()
            && only != &ctx.media.sha256
        {
            out.push(AmigaTrainerBlocker::IdentityMismatch);
        }
    }
    let platform_verified = ctx.facts.iter().any(|f| {
        f.kind == IdentityKind::Platform
            && f.status == IdentityStatus::Verified
            && f.value.as_deref().is_some_and(claimed_platform_is_amiga)
    });
    if !platform_verified {
        out.push(AmigaTrainerBlocker::IdentityNotVerified);
    }
    out.sort_by_key(|b| format!("{b:?}"));
    out.dedup();
    out
}

fn member_of<'a>(ctx: &'a AmigaTrainerContext<'_>) -> Option<&'a crate::media_set::MediaSetMember> {
    ctx.media_set?.members.iter().find(|m| {
        m.representations
            .iter()
            .any(|r| r.record.source.path == ctx.media.path)
    })
}

fn disk_ordinal(ctx: &AmigaTrainerContext<'_>) -> Option<u16> {
    member_of(ctx)?
        .ordinal
        .as_ref()
        .filter(|o| o.unit == OrdinalUnit::Disk)
        .map(|o| o.number)
}

/// Targeting gates for scope, disk and set. Never infers a disk from a name.
fn targeting_blockers(t: &AmigaTrainer, ctx: &AmigaTrainerContext<'_>) -> Vec<AmigaTrainerBlocker> {
    use AmigaTrainerBlocker as B;
    let mut out = Vec::new();
    let Some(matched) = t.target.media.iter().find(|m| m.sha256 == ctx.media.sha256) else {
        return vec![B::MediaNotTargeted];
    };
    let set = ctx.media_set;
    if let (Some(set), Some(key)) = (set, &t.target.release)
        && &set.identity.key != key
    {
        out.push(B::ReleaseMismatch);
    }
    if let Some(set) = set
        && set
            .platform
            .as_deref()
            .is_some_and(|p| !claimed_platform_is_amiga(p))
    {
        out.push(B::PlatformMismatch {
            claimed: set.platform.clone().unwrap_or_default(),
        });
    }
    match t.target.scope {
        AmigaTrainerScope::Disk(wanted) => {
            if let Some(found) = matched.disk
                && found != wanted
            {
                out.push(B::WrongDisk { wanted, found });
            }
            match disk_ordinal(ctx) {
                Some(found) if found != wanted => out.push(B::WrongDisk { wanted, found }),
                Some(_) => {}
                None if matched.disk == Some(wanted) => {}
                None => out.push(B::DiskUnconfirmed),
            }
        }
        AmigaTrainerScope::WholeTitle => match set {
            None => out.push(B::SetRequired),
            Some(set) => {
                match set.state {
                    MediaSetState::CompleteSet => {}
                    MediaSetState::IncompleteSet => out.push(B::SetIncomplete),
                    MediaSetState::ConflictingSet => out.push(B::SetOrderUnverified),
                    MediaSetState::AmbiguousSet => out.push(B::SetAmbiguous),
                    MediaSetState::UnverifiedSet | MediaSetState::UnsupportedSet => {
                        out.push(B::SetUnverified);
                    }
                }
                if !set.identity.verified || member_of(ctx).is_none() {
                    out.push(B::SetUnverified);
                }
            }
        },
        AmigaTrainerScope::Revision => {
            if set.is_none() {
                out.push(B::ReleaseEvidenceUnknown("revision"));
            }
        }
    }
    out.sort_by_key(|b| format!("{b:?}"));
    out.dedup();
    out
}

fn selected_game(ctx: &AmigaTrainerContext<'_>) -> CheatSelectedGame {
    // Never the title or file name: identity comes from evidence only.
    let mut game = CheatSelectedGame::from_evidence(&ctx.facts);
    game.title = None;
    game.filename = None;
    if let Some(set) = ctx.media_set {
        let trusted = set.identity.verified
            && !matches!(
                set.state,
                MediaSetState::ConflictingSet | MediaSetState::AmbiguousSet
            );
        let status = if trusted {
            IdentityStatus::Verified
        } else {
            IdentityStatus::Candidate
        };
        if let Some(region) = &set.variant.region {
            game.region = Some(CheatReleaseEvidence {
                value: region.clone(),
                status,
            });
        }
        if let Some(revision) = &set.variant.revision {
            game.revision = Some(CheatReleaseEvidence {
                value: revision.clone(),
                status,
            });
        }
    }
    game
}

/// The trainer as a canonical reconciliation entry. Platform, operations,
/// provenance and the exact-image claim all use the canonical vocabulary.
fn canonical_entry(
    t: &AmigaTrainer,
    media: &AdfMedia,
    import: &AmigaTrainerImport,
) -> CheatReconciliationEntry {
    let operations = t
        .writes
        .iter()
        .map(|w| match w.width {
            AmigaMemoryWidth::Byte => CheatOperation::Write8 {
                address: u64::from(w.address),
                value: w.value as u8,
            },
            AmigaMemoryWidth::Word => CheatOperation::Write16 {
                address: u64::from(w.address),
                value: w.value as u16,
            },
            AmigaMemoryWidth::Long => CheatOperation::Write32 {
                address: u64::from(w.address),
                value: w.value,
            },
        })
        .collect();
    let mut record = CheatRecordProvenance::local_with_sha256(
        Path::new(&import.source_name),
        &import.source_sha256,
        "amiga_adf_trainer",
    );
    record.record_index = u32::try_from(t.index).ok();
    record.original_description = Some(t.title.clone());
    let format = CheatSourceFormat::Other("EmuWiz Amiga ADF trainer".into());
    CheatReconciliationEntry {
        game_identity: format!("amiga-adf:sha256:{}", media.sha256),
        identity_verified: true,
        applicability: CheatApplicability {
            region: t.target.region.clone(),
            revision: t.target.revision.clone(),
            verified_binary_identity: Some(media.sha256.clone()),
            ..Default::default()
        },
        source_path: Some(import.source_name.clone()),
        source_index: u32::try_from(t.index).ok(),
        source_fields: Vec::new(),
        title: t.title.clone(),
        source: t.source.name.clone(),
        source_format: format.clone(),
        document: CheatDocument {
            source_evidence: vec![record],
            title: t.title.clone(),
            platform: CheatPlatform::Other("Amiga".into()),
            source_format: format,
            operations,
            issues: Vec::new(),
            provenance: vec![t.source.name.clone()],
        },
        raw_code: None,
        provenance: vec![t.source.name.clone(), import.source_sha256.clone()],
    }
}

fn finish(blockers: &[AmigaTrainerBlocker]) -> AmigaTrainerStatus {
    if blockers.iter().any(AmigaTrainerBlocker::is_hard) {
        AmigaTrainerStatus::Unsupported
    } else if !blockers.is_empty() {
        AmigaTrainerStatus::PreviewOnly
    } else {
        AmigaTrainerStatus::RequiresEmulatorRuntime
    }
}

/// Build the plan for one image. `choices` are explicit user decisions for
/// conflicting trainers, keyed by the canonical group index in
/// [`AmigaTrainerPlan::conflict_groups`]; without one, nothing in a conflict
/// is selected.
#[must_use]
pub fn plan_amiga_adf_trainers(
    import: &AmigaTrainerImport,
    ctx: &AmigaTrainerContext<'_>,
    choices: &BTreeMap<usize, CheatReviewChoice>,
) -> AmigaTrainerPlan {
    let identity = identity_blockers(ctx);
    let mut assessments: Vec<AmigaTrainerAssessment> = Vec::new();
    let mut candidates: Vec<usize> = Vec::new(); // positions in `assessments`
    let mut over_limit = 0usize;
    let mut considered_for_game = 0usize;

    for t in &import.trainers {
        let mut blockers: Vec<AmigaTrainerBlocker> = Vec::new();
        if t.mechanism.unsupported_reason().is_some() {
            blockers.push(AmigaTrainerBlocker::Mechanism(t.mechanism));
        } else {
            if !claimed_platform_is_amiga(&t.platform) {
                blockers.push(AmigaTrainerBlocker::PlatformMismatch {
                    claimed: t.platform.clone(),
                });
            }
            if ctx.media.alternate.is_alternate() {
                blockers.push(AmigaTrainerBlocker::AlternateMedia(ctx.media.alternate));
            }
            blockers.extend(identity.iter().cloned());
            let targeting = targeting_blockers(t, ctx);
            let targets_this_image = !targeting.contains(&AmigaTrainerBlocker::MediaNotTargeted);
            blockers.extend(targeting);
            if targets_this_image {
                considered_for_game += 1;
                if considered_for_game > MAX_TRAINERS_PER_GAME {
                    over_limit += 1;
                    blockers.push(AmigaTrainerBlocker::TooManyForGame);
                }
            }
        }
        blockers.sort_by_key(|b| format!("{b:?}"));
        blockers.dedup();
        let clean = blockers.is_empty();
        assessments.push(AmigaTrainerAssessment {
            index: t.index,
            title: t.title.clone(),
            mechanism: t.mechanism,
            status: finish(&blockers),
            blockers,
            selection: AmigaTrainerSelection::NotEvaluated,
            canonical_state: None,
        });
        if clean {
            candidates.push(assessments.len() - 1);
        }
    }

    // Canonical stage: only trainers that passed every Amiga gate.
    let mut conflict_groups = Vec::new();
    let mut selected = Vec::new();
    if !candidates.is_empty() {
        let by_index: BTreeMap<usize, &AmigaTrainer> =
            import.trainers.iter().map(|t| (t.index, t)).collect();
        let entries: Vec<CheatReconciliationEntry> = candidates
            .iter()
            .map(|&p| canonical_entry(by_index[&assessments[p].index], ctx.media, import))
            .collect();
        let game = selected_game(ctx);
        if let CheatReconciliationOutcome::Ready(result) =
            reconcile_cheats_for_game(entries.clone())
        {
            for (slot, &p) in candidates.iter().enumerate() {
                let trainer = by_index[&assessments[p].index];
                let mut association = CheatGameAssociation::from_entry(&entries[slot]);
                association.platform = Some(trainer.platform.clone());
                let report = assess_cheat_applicability(&CheatApplicabilityInput {
                    game: game.clone(),
                    association,
                    document: entries[slot].document.clone(),
                    parsing: CheatParseEvidence::Valid,
                    native_cht: None,
                    route: None,
                    reconciliation: Some(result.clone()),
                });
                let a = &mut assessments[p];
                a.canonical_state = Some(report.state);
                for issue in &report.blockers {
                    match issue {
                        CheatApplicabilityIssue::WrongRegion => {
                            a.blockers.push(AmigaTrainerBlocker::RegionMismatch);
                        }
                        CheatApplicabilityIssue::WrongRevision => {
                            a.blockers.push(AmigaTrainerBlocker::RevisionMismatch);
                        }
                        CheatApplicabilityIssue::DifferentGame => {
                            a.blockers.push(AmigaTrainerBlocker::PlatformMismatch {
                                claimed: trainer.platform.clone(),
                            });
                        }
                        CheatApplicabilityIssue::ConflictingIdentity => {
                            a.blockers.push(AmigaTrainerBlocker::IdentityConflicting);
                        }
                        CheatApplicabilityIssue::Malformed
                        | CheatApplicabilityIssue::MissingCode => {
                            a.blockers
                                .push(AmigaTrainerBlocker::CanonicalRefusal(*issue));
                        }
                        // Capability-unknown, parsing-unknown and conflict findings are
                        // handled by the runtime classification and the choice step.
                        _ => {}
                    }
                }
                // Unknown region/revision only matters when the trainer *claims* one:
                // the exact-image hash already pins an unqualified trainer.
                for issue in &report.warnings {
                    match issue {
                        CheatApplicabilityIssue::RegionUnknown
                            if trainer.target.region.is_some() =>
                        {
                            a.blockers
                                .push(AmigaTrainerBlocker::ReleaseEvidenceUnknown("region"))
                        }
                        CheatApplicabilityIssue::RevisionUnknown
                            if trainer.target.revision.is_some() =>
                        {
                            a.blockers
                                .push(AmigaTrainerBlocker::ReleaseEvidenceUnknown("revision"))
                        }
                        _ => {}
                    }
                }
                a.blockers.sort_by_key(|b| format!("{b:?}"));
                a.blockers.dedup();
                a.status = finish(&a.blockers);
            }

            // Explicit choices through the canonical resolver.
            let request = ResolvedCheatPlanRequest {
                source_report_digest: import.source_sha256.clone(),
                emulator: "Amiga (a running emulator is required)".into(),
                profile: ctx.media.sha256.clone(),
                target_file: None,
                existing_file_digest: None,
                destination_changed: false,
            };
            let resolved = resolve_reviewed_cheat_plan(&result, choices, &request);
            let cand_index: Vec<usize> = candidates.iter().map(|&p| assessments[p].index).collect();
            let trainer_of = |entry: usize| cand_index[entry];
            for (group_index, group) in result.groups.iter().enumerate() {
                if group.classifications.iter().any(|k| k.requires_review()) {
                    conflict_groups.push(AmigaConflictGroup {
                        group_index,
                        trainers: group.entry_indices.iter().map(|&e| trainer_of(e)).collect(),
                        kinds: group.classifications.clone(),
                    });
                }
            }
            conflict_groups.truncate(MAX_REPORTED_ITEMS);
            let mut chosen = std::collections::BTreeSet::new();
            for entry in &resolved.selected_entries {
                let primary = trainer_of(entry.canonical_entry_index);
                chosen.insert(primary);
                for dup in &entry.duplicate_entry_indices {
                    if *dup != entry.canonical_entry_index {
                        chosen.insert(trainer_of(*dup));
                    }
                }
            }
            let in_conflict: std::collections::BTreeSet<usize> = conflict_groups
                .iter()
                .flat_map(|g| g.trainers.iter().copied())
                .collect();
            for &p in &candidates {
                let index = assessments[p].index;
                let a = &mut assessments[p];
                if a.status != AmigaTrainerStatus::RequiresEmulatorRuntime {
                    continue;
                }
                if in_conflict.contains(&index) {
                    if chosen.contains(&index) {
                        a.selection = AmigaTrainerSelection::Selected;
                    } else {
                        let decided = conflict_groups
                            .iter()
                            .filter(|g| g.trainers.contains(&index))
                            .any(|g| choices.contains_key(&g.group_index));
                        a.selection = if decided {
                            AmigaTrainerSelection::NotChosen
                        } else {
                            AmigaTrainerSelection::NeedsChoice
                        };
                        a.blockers.push(if decided {
                            AmigaTrainerBlocker::NotChosen
                        } else {
                            AmigaTrainerBlocker::ConflictNeedsChoice
                        });
                        a.status = AmigaTrainerStatus::PreviewOnly;
                    }
                } else if chosen.contains(&index) {
                    a.selection = AmigaTrainerSelection::Selected;
                }
            }
            // An identical duplicate stays visible, pointing at the one it matches.
            for entry in &resolved.selected_entries {
                let primary = trainer_of(entry.canonical_entry_index);
                for dup in &entry.duplicate_entry_indices {
                    let dup_index = trainer_of(*dup);
                    if dup_index != primary
                        && let Some(a) = assessments.iter_mut().find(|a| a.index == dup_index)
                        && a.status == AmigaTrainerStatus::RequiresEmulatorRuntime
                    {
                        a.selection = AmigaTrainerSelection::DuplicateOf(primary);
                    }
                }
            }
            for &p in &candidates {
                let a = &assessments[p];
                if a.selection == AmigaTrainerSelection::Selected && a.can_plan() {
                    selected.push(a.index);
                }
            }
        }
    }
    assessments.sort_by_key(|a| a.index);
    selected.sort_unstable();
    AmigaTrainerPlan {
        media_path: ctx.media.path.clone(),
        media_sha256: ctx.media.sha256.clone(),
        disk_ordinal: disk_ordinal(ctx),
        set_state: ctx.media_set.map(|s| s.state),
        source_policy: SourceMediaPolicy::ReadOnlyOriginal,
        requires_scratch_copy: false,
        assessments,
        conflict_groups,
        selected,
        runtime: amiga_runtime_options(),
        over_limit,
    }
}
