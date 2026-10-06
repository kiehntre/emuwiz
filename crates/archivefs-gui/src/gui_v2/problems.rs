//! Native GUI-v2 problem projection.
//!
//! This module is deliberately a presentation adapter. It does not scan the
//! filesystem, infer identities, or invent repair actions. Findings come from
//! the saved library state and the existing exact-duplicate proof.

use super::library::{DuplicateReport, Game, Library, UNKNOWN_PLATFORM};
use super::routes::{Route, Section};
use archivefs_core::game_identity::{IdentityKind, IdentityStatus};
use archivefs_core::identity_attention::{
    ChoiceReason, IdentityAttention, IdentityFacts, InformationalReason, classify_identity,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd)]
pub(super) enum Severity {
    NeedsAttention,
    Warning,
    Informational,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub(super) enum ProblemState {
    Current,
    NeedsEvidence,
    Informational,
}

impl ProblemState {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::NeedsEvidence => "Needs evidence",
            Self::Informational => "Informational",
        }
    }

    pub(super) fn is_actionable(self) -> bool {
        !matches!(self, Self::Informational)
    }
}

/// Every kind of finding GUI-v2 can show. Adding a kind forces the three
/// exhaustive matches below (`is_actionable`, `ALL` through the coverage test,
/// and the constructors) to say whether the person gets a next step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum ProblemKind {
    /// The file is gone and its folder is still there.
    FileMissing,
    /// The file is gone and so is its folder (a drive that is not connected).
    FileFolderUnavailable,
    /// The file is there but its saved health says it is not usable.
    FileUnhealthy,
    /// A MAME set (judged as a whole set) with a file problem.
    FileMame,
    IdentityConflict,
    IdentityAmbiguous,
    IdentityMame,
    /// Games with no system assigned.
    NoSystem,
    /// A reference database exists for the system but is not installed.
    DatSetupRequired,
    /// A reference database is installed and these games are not matched yet.
    DatMatchAvailable,
    NoReferenceSource,
    MatchedByReference,
    SpecialRelease,
    DuplicateGroup,
}

impl ProblemKind {
    pub(super) const ALL: [ProblemKind; 14] = [
        Self::FileMissing,
        Self::FileFolderUnavailable,
        Self::FileUnhealthy,
        Self::FileMame,
        Self::IdentityConflict,
        Self::IdentityAmbiguous,
        Self::IdentityMame,
        Self::NoSystem,
        Self::DatSetupRequired,
        Self::DatMatchAvailable,
        Self::NoReferenceSource,
        Self::MatchedByReference,
        Self::SpecialRelease,
        Self::DuplicateGroup,
    ];

    /// Informational kinds need nothing from the person and get no button.
    pub(super) fn is_actionable(self) -> bool {
        match self {
            Self::FileMissing
            | Self::FileFolderUnavailable
            | Self::FileUnhealthy
            | Self::FileMame
            | Self::IdentityConflict
            | Self::IdentityAmbiguous
            | Self::IdentityMame
            | Self::NoSystem
            | Self::DatSetupRequired
            | Self::DatMatchAvailable
            | Self::DuplicateGroup => true,
            Self::NoReferenceSource | Self::MatchedByReference | Self::SpecialRelease => false,
        }
    }
}

/// What the destination should open on, carried with the navigation so the
/// person does not have to find the thing again. It only ever selects
/// something that already exists; opening it never starts work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ProblemContext {
    None,
    /// Games list on one system (the library's own label for it).
    GamesSystem(String),
    /// Check Games on one platform.
    CheckPlatform(String),
    /// Duplicates page showing one exact-duplicate group (by SHA-256).
    DuplicateGroup(String),
    /// Problems page positioned on the Missing games review.
    MissingReview,
}

/// One next step for a finding: what the button says, where it goes, and what
/// it opens on. Navigation only; nothing here changes a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProblemAction {
    pub(super) label: String,
    pub(super) route: Route,
    pub(super) context: ProblemContext,
}

impl ProblemAction {
    fn new(label: impl Into<String>, route: Route) -> Self {
        Self {
            label: label.into(),
            route,
            context: ProblemContext::None,
        }
    }

    fn with(mut self, context: ProblemContext) -> Self {
        self.context = context;
        self
    }

    fn show_game(game_id: i64) -> Self {
        Self::new("Show game", Route::Game(game_id))
    }

    fn review_mame() -> Self {
        Self::new("Review in MAME", Route::MameWorkflow)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ProblemFilter {
    #[default]
    Actionable,
    All,
}

impl ProblemFilter {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Actionable => "Actionable now",
            Self::All => "All current findings",
        }
    }

    pub(super) fn accepts(self, problem: &Problem) -> bool {
        match self {
            Self::Actionable => problem.state.is_actionable(),
            Self::All => true,
        }
    }
}

/// True only when the game's own identity evidence carries a verified exact
/// MAME machine name (from a checksum-pinned MAME DAT). The platform label,
/// file name, or folder never counts.
pub(super) fn proves_mame(game: &Game) -> bool {
    game.archive.identity_report.as_ref().is_some_and(|report| {
        report.evidence.iter().any(|evidence| {
            evidence.kind == IdentityKind::MameMachineName
                && evidence.status == IdentityStatus::Verified
                && evidence.value.is_some()
        })
    })
}

const MAME_ACTION: &str = "Review this set in the MAME workflow. MAME sets are judged as complete sets, so individual files are not renamed or repaired from this page.";

impl Severity {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NeedsAttention => "Needs attention",
            Self::Warning => "Warnings",
            Self::Informational => "Informational",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub(super) enum Category {
    Files,
    Duplicates,
    Identity,
    Verification,
}

impl Category {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Files => "Broken or missing files",
            Self::Duplicates => "Duplicate games",
            Self::Identity => "Metadata and identity",
            Self::Verification => "Verification results",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Problem {
    pub(super) id: String,
    pub(super) game_id: Option<i64>,
    pub(super) title: String,
    pub(super) category: Category,
    pub(super) severity: Severity,
    pub(super) state: ProblemState,
    pub(super) kind: ProblemKind,
    /// The one obvious next step. `None` only for informational findings.
    pub(super) primary: Option<ProblemAction>,
    pub(super) secondary: Option<ProblemAction>,
    pub(super) affected: String,
    pub(super) location: String,
    pub(super) why: String,
    pub(super) action: String,
    pub(super) safety: String,
    pub(super) undo: String,
    pub(super) technical: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ProblemSummary {
    pub(super) problems: Vec<Problem>,
    pub(super) category_indices: BTreeMap<Category, Vec<usize>>,
}

impl ProblemSummary {
    pub(super) fn from_library(library: &Library, duplicates: Option<&DuplicateReport>) -> Self {
        let mut problems = Vec::new();
        let mut groups: BTreeMap<IdentityGroupKey, IdentityGroup> = BTreeMap::new();
        for game in &library.games {
            if game.archive.last_verified_missing_at.is_some()
                || !path_is_present(game)
                || matches!(
                    game.archive.last_known_health.as_str(),
                    "missing" | "corrupt" | "damaged" | "error"
                )
            {
                problems.push(file_problem(game));
            } else if game.dat_exact.is_some() {
                // An exact authoritative DAT match is verified automatically.
            } else {
                let facts = IdentityFacts {
                    platform: (game.platform != UNKNOWN_PLATFORM).then_some(game.platform.as_str()),
                    relative_path: &game.archive.relative_path,
                    report: game.archive.identity_report.as_ref(),
                    matched_by_reference_data: library
                        .identity_context
                        .matched
                        .contains(&game.archive.id),
                };
                match classify_identity(&facts, library.identity_context.inventory.as_ref()) {
                    IdentityAttention::Identified => {}
                    IdentityAttention::NeedsChoice(
                        reason @ (ChoiceReason::Conflict | ChoiceReason::Ambiguous),
                    ) => problems.push(identity_choice_problem(game, reason)),
                    other => {
                        let key = IdentityGroupKey::of(other, &game.platform);
                        let group = groups.entry(key).or_default();
                        group.count += 1;
                        if group.samples.len() < 5 {
                            group.samples.push(game.title.clone());
                        }
                    }
                }
            }
        }
        problems.extend(
            groups
                .into_iter()
                .map(|(key, group)| identity_group_problem(&key, &group)),
        );
        if let Some(report) = duplicates {
            for group in &report.groups {
                problems.push(Problem {
                    id: format!("duplicate-{}-{}", group.exact_index, group.sha256),
                    game_id: None,
                    title: format!("{} identical copies were found", group.members.len()),
                    category: Category::Duplicates,
                    severity: Severity::Warning,
                    state: ProblemState::Current,
                    kind: ProblemKind::DuplicateGroup,
                    primary: Some(
                        ProblemAction::new("Review duplicates", Route::Section(Section::Duplicates))
                            .with(ProblemContext::DuplicateGroup(group.sha256.clone())),
                    ),
                    secondary: None,
                    affected: group.members.iter().map(|member| member.title.as_str()).collect::<Vec<_>>().join(", "),
                    location: "Current duplicate candidates".into(),
                    why: "Keeping multiple byte-for-byte copies makes it harder to know which file to use and wastes space.".into(),
                    action: "Review the duplicate group. EmuWiz will not remove anything from this page.".into(),
                    safety: "Read-only until an existing duplicate-quarantine plan is explicitly reviewed and approved.".into(),
                    undo: "Quarantine undo is available only where the existing repair transaction proves it is safe.".into(),
                    technical: format!("SHA-256 {} · {} bytes · {} files examined", group.sha256, group.size_bytes, report.files_examined),
                });
            }
        }
        problems.sort_by(|a, b| {
            (a.severity, &a.category, &a.title, &a.id).cmp(&(
                b.severity,
                &b.category,
                &b.title,
                &b.id,
            ))
        });
        let mut category_indices = BTreeMap::new();
        for (index, problem) in problems.iter().enumerate() {
            category_indices
                .entry(problem.category)
                .or_insert_with(Vec::new)
                .push(index);
        }
        Self {
            problems,
            category_indices,
        }
    }

    pub(super) fn count(&self, severity: Severity) -> usize {
        self.problems
            .iter()
            .filter(|problem| problem.severity == severity)
            .count()
    }

    /// The one "needs attention" number: Home and Problems & Repair both show it.
    pub(super) fn attention_count(&self) -> usize {
        self.count(Severity::NeedsAttention)
    }

    pub(super) fn actionable_count(&self) -> usize {
        self.problems
            .iter()
            .filter(|problem| problem.state.is_actionable())
            .count()
    }
}

/// Whether the recorded object exists. An arcade set is a folder, not a file.
fn path_is_present(game: &Game) -> bool {
    if game.archive.archive_kind == "arcade_set_directory" {
        game.archive.absolute_path.is_dir()
    } else {
        game.archive.absolute_path.is_file()
    }
}

fn file_problem(game: &Game) -> Problem {
    let present = path_is_present(game);
    let mame = proves_mame(game);
    // A missing file whose folder is also gone usually means a drive that is not
    // connected; a missing file in a folder that is there is a file that moved.
    let folder_reachable = game
        .archive
        .absolute_path
        .parent()
        .is_some_and(std::path::Path::is_dir);
    let kind = if mame {
        ProblemKind::FileMame
    } else if present {
        ProblemKind::FileUnhealthy
    } else if folder_reachable {
        ProblemKind::FileMissing
    } else {
        ProblemKind::FileFolderUnavailable
    };
    let id = game.archive.id;
    let (primary, action) = match kind {
        ProblemKind::FileMame => (ProblemAction::review_mame(), MAME_ACTION.to_string()),
        ProblemKind::FileUnhealthy => (
            ProblemAction::new(
                "Check this game",
                Route::Task {
                    section: Section::Check,
                    game: id,
                },
            )
            .with(ProblemContext::CheckPlatform(game.platform.clone())),
            "Check this game again to see whether the file can be read.".to_string(),
        ),
        ProblemKind::FileFolderUnavailable => (
            ProblemAction::new("Review game folders", Route::Section(Section::Sources)),
            "The folder this game lives in is not available. Make sure the drive is connected, then review your game folders.".to_string(),
        ),
        _ => (
            ProblemAction::new("Review missing games", Route::Section(Section::Problems))
                .with(ProblemContext::MissingReview),
            "Review the missing games. EmuWiz can forget entries you confirm are gone, and you can undo that.".to_string(),
        ),
    };
    Problem {
        id: format!("missing-{id}"),
        game_id: Some(id),
        title: format!("{} is missing or has a saved health problem", game.title),
        category: Category::Files,
        severity: Severity::NeedsAttention,
        state: ProblemState::Current,
        kind,
        primary: Some(primary),
        secondary: Some(ProblemAction::show_game(id)),
        affected: format!("{} · {}", game.platform, game.title),
        location: if present {
            format!("Current path: {}", game.archive.absolute_path.display())
        } else {
            format!("Last recorded path: {}", game.archive.absolute_path.display())
        },
        why: "EmuWiz cannot safely verify or prepare this game until the recorded file is available and readable.".into(),
        action,
        safety: "Read-only. Browsing and verification do not rename, move, delete, or repair the source file.".into(),
        undo: "No file change was made, so there is nothing to undo.".into(),
        technical: format!(
            "Catalogue id {} · source folder {} · recorded path {} · health {}",
            game.archive.id,
            game.archive.source_folder_id,
            game.archive.absolute_path.display(),
            game.archive.last_known_health
        ),
    }
}

/// A game whose evidence conflicts or is ambiguous: a person has to choose.
fn identity_choice_problem(game: &Game, reason: ChoiceReason) -> Problem {
    let (title, why, action, kind, label) = match reason {
        ChoiceReason::Conflict => (
            format!("{} has conflicting identification evidence", game.title),
            "Two trusted sources disagree about which game this is, so EmuWiz will not pick one for you.",
            "Review the evidence in the game details and decide which one is right.",
            ProblemKind::IdentityConflict,
            "Review evidence",
        ),
        _ => (
            format!("{} has more than one possible match", game.title),
            "EmuWiz found several possible matches and will not guess between them.",
            "Choose the correct match in the game details.",
            ProblemKind::IdentityAmbiguous,
            "Review matches",
        ),
    };
    let id = game.archive.id;
    let mame = proves_mame(game);
    Problem {
        id: format!("identity-{id}"),
        game_id: Some(id),
        title,
        category: Category::Identity,
        severity: Severity::NeedsAttention,
        state: ProblemState::NeedsEvidence,
        kind: if mame {
            ProblemKind::IdentityMame
        } else {
            kind
        },
        primary: Some(if mame {
            ProblemAction::review_mame()
        } else {
            ProblemAction::new(label, Route::ReviewIdentity(id))
        }),
        secondary: (!mame && game.platform != UNKNOWN_PLATFORM).then(|| {
            ProblemAction::new(
                format!("Check {} games", game.platform),
                Route::Section(Section::Check),
            )
            .with(ProblemContext::CheckPlatform(game.platform.clone()))
        }),
        affected: format!("{} · {}", game.platform, game.title),
        location: format!("Current path: {}", game.archive.absolute_path.display()),
        why: why.into(),
        action: if mame {
            MAME_ACTION.into()
        } else {
            action.into()
        },
        safety: "Read-only. EmuWiz will not turn a filename hint into a verified identity.".into(),
        undo: "No file change was made, so there is nothing to undo.".into(),
        technical: format!(
            "Catalogue id {} · identity evidence {reason:?}",
            game.archive.id
        ),
    }
}

/// Games that share one unconfirmed-identity situation are one finding, not one
/// finding per game.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct IdentityGroupKey {
    kind: u8,
    reason: String,
    platform: String,
}

#[derive(Default)]
struct IdentityGroup {
    count: usize,
    samples: Vec<String>,
}

impl IdentityGroupKey {
    fn of(attention: IdentityAttention, platform: &str) -> Self {
        let (kind, reason, platform) = match attention {
            IdentityAttention::SetupRequired(ecosystem) => {
                (1, ecosystem.label().to_string(), platform)
            }
            IdentityAttention::ActionAvailable => (2, String::new(), platform),
            IdentityAttention::NeedsChoice(_) => (0, String::new(), ""),
            IdentityAttention::Informational(InformationalReason::NoReferenceSource) => {
                (3, String::new(), platform)
            }
            IdentityAttention::Informational(InformationalReason::MatchedByReferenceData) => {
                (4, String::new(), "")
            }
            IdentityAttention::Informational(InformationalReason::SpecialRelease) => {
                (5, String::new(), "")
            }
            IdentityAttention::Identified => (9, String::new(), ""),
        };
        Self {
            kind,
            reason,
            platform: platform.to_string(),
        }
    }
}

fn identity_group_problem(key: &IdentityGroupKey, group: &IdentityGroup) -> Problem {
    let n = group.count;
    let games = if n == 1 { "game" } else { "games" };
    let have = if n == 1 { "has" } else { "have" };
    let platform = key.platform.clone();
    let (title, why, action, kind, severity, state, primary) = match key.kind {
        0 => (
            format!("{n} {games} {have} no system assigned"),
            "EmuWiz cannot launch or identify a game until it knows which system it belongs to.".to_string(),
            "Show these games. Choosing a system for them is not available in this window yet.".to_string(),
            ProblemKind::NoSystem,
            Severity::Warning,
            ProblemState::NeedsEvidence,
            Some(
                ProblemAction::new("Show games without a system", Route::Section(Section::Games))
                    .with(ProblemContext::GamesSystem(UNKNOWN_PLATFORM.to_string())),
            ),
        ),
        1 => (
            format!("{}: identification data is not set up yet", key.platform),
            format!("EmuWiz knows an identification database for this system ({}) but none is installed. {n} {games} can still be played; identification is optional.", key.reason),
            "Set up identification data for this system.".to_string(),
            ProblemKind::DatSetupRequired,
            Severity::Warning,
            ProblemState::NeedsEvidence,
            Some(ProblemAction::new("Set up identification data", Route::Section(Section::Dat))),
        ),
        2 => (
            format!("{}: {n} {games} {have} not been matched to identification data yet", key.platform),
            "These games can still be played. Matching them gives EmuWiz stronger proof of exactly which release each one is.".to_string(),
            "Open Check Games for this system to match its games.".to_string(),
            ProblemKind::DatMatchAvailable,
            Severity::Warning,
            ProblemState::NeedsEvidence,
            Some(
                ProblemAction::new("Review matches", Route::Section(Section::Check))
                    .with(ProblemContext::CheckPlatform(platform)),
            ),
        ),
        3 => (
            format!("{}: no identification database is available", key.platform),
            format!("EmuWiz does not know a reference database for this system, so there is nothing to match {n} {games} against. This is not a problem."),
            "No action is needed.".to_string(),
            ProblemKind::NoReferenceSource,
            Severity::Informational,
            ProblemState::Informational,
            None,
        ),
        4 => (
            format!("{n} arcade {games} matched the reference data"),
            "These sets were checked against the MAME reference data. Nothing is wrong.".to_string(),
            "No action is needed.".to_string(),
            ProblemKind::MatchedByReference,
            Severity::Informational,
            ProblemState::Informational,
            None,
        ),
        _ => (
            format!("{n} special {} (homebrew, prototypes, hacks, translations)", if n == 1 { "release" } else { "releases" }),
            "Normal identification databases do not describe these releases, so they stay unmatched. They can still be played.".to_string(),
            "No action is needed.".to_string(),
            ProblemKind::SpecialRelease,
            Severity::Informational,
            ProblemState::Informational,
            None,
        ),
    };
    Problem {
        id: format!("identity-group-{}-{}-{}", key.kind, key.reason, key.platform),
        game_id: None,
        title,
        category: Category::Identity,
        severity,
        state,
        kind,
        primary,
        secondary: None,
        affected: if key.platform.is_empty() { "Several systems".into() } else { key.platform.clone() },
        location: format!("{n} {games} in your library"),
        why,
        action,
        safety: "Read-only. EmuWiz will not turn a filename hint into a verified identity, and nothing is renamed or moved.".into(),
        undo: "No file change was made, so there is nothing to undo.".into(),
        technical: format!("{n} games · examples: {}", group.samples.join(", ")),
    }
}

#[cfg(test)]
pub(in crate::gui_v2) mod tests {
    use super::*;
    use archivefs_core::PersistedArchive;
    use archivefs_core::identity_attention::ReferenceInventory;

    fn game(id: i64, title: &str, identified: bool, missing: bool) -> Game {
        let mut archive = PersistedArchive {
            id,
            source_folder_id: 1,
            relative_path: format!("{title}.zip").into(),
            absolute_path: "/path/that/does/not/exist.zip".into(),
            archive_kind: "zip".into(),
            display_name: title.into(),
            normalized_name: title.into(),
            size_bytes: Some(1),
            modified_time_unix_seconds: Some(1),
            platform: Some("Arcade".into()),
            platform_source: Some("test".into()),
            last_known_health: "pending".into(),
            last_seen_at: "now".into(),
            last_verified_missing_at: None,
            identity_report: None,
        };
        if !missing {
            archive.absolute_path = std::env::current_exe().unwrap();
        }
        Game {
            archive,
            title: title.into(),
            platform: "Arcade".into(),
            identified,
            dat_exact: None,
            attention: missing,
            screenscraper: None,
            search: title.to_lowercase(),
        }
    }

    #[test]
    fn projection_uses_plain_english_and_severity_counts() {
        let library = Library::new(Vec::new());
        let mut library = library;
        library.games = vec![
            game(1, "Pac-Man", false, false),
            game(2, "Missing", true, true),
        ];
        let summary = ProblemSummary::from_library(&library, None);
        assert_eq!(summary.count(Severity::NeedsAttention), 1);
        assert_eq!(summary.count(Severity::Warning), 1);
        assert!(
            summary
                .problems
                .iter()
                .any(|p| p.title.contains("has not been matched"))
        );
        assert!(summary.problems.iter().all(|p| !p.title.contains("::")));
    }

    #[test]
    fn duplicate_projection_is_review_only_and_deterministic() {
        let library = Library::new(Vec::new());
        let report = DuplicateReport {
            files_examined: 2,
            exact_groups: Vec::new(),
            groups: vec![super::super::library::DuplicateGroup {
                exact_index: 0,
                kind: "Exact duplicates".into(),
                sha256: "abc".into(),
                size_bytes: 4,
                members: vec![
                    super::super::library::DuplicateMember {
                        path: "/a".into(),
                        title: "A".into(),
                        platform: "Arcade".into(),
                        size_bytes: 4,
                        evidence: "hash".into(),
                    },
                    super::super::library::DuplicateMember {
                        path: "/b".into(),
                        title: "B".into(),
                        platform: "Arcade".into(),
                        size_bytes: 4,
                        evidence: "hash".into(),
                    },
                ],
            }],
        };
        let first = ProblemSummary::from_library(&library, Some(&report));
        let second = ProblemSummary::from_library(&library, Some(&report));
        assert_eq!(first, second);
        assert_eq!(first.problems[0].category, Category::Duplicates);
        assert!(first.problems[0].action.contains("Review"));
        assert_eq!(first.problems[0].state, ProblemState::Current);
        let action = first.problems[0].primary.as_ref().unwrap();
        assert_eq!(action.label, "Review duplicates");
        assert_eq!(action.route, Route::Section(Section::Duplicates));
        assert_eq!(action.context, ProblemContext::DuplicateGroup("abc".into()));
    }

    #[test]
    fn identity_findings_require_evidence_before_rename() {
        let mut library = Library::new(Vec::new());
        library.games = vec![game(3, "Unknown", false, false)];
        let summary = ProblemSummary::from_library(&library, None);
        let problem = &summary.problems[0];
        assert_eq!(problem.state, ProblemState::NeedsEvidence);
        let action = problem.primary.as_ref().unwrap();
        assert_eq!(action.route, Route::Section(Section::Check));
        assert_eq!(
            action.context,
            ProblemContext::CheckPlatform("Arcade".into())
        );
        assert!(!problem.action.to_lowercase().contains("rename"));
        assert!(problem.safety.contains("filename hint"));
    }

    #[test]
    fn missing_file_uses_last_recorded_path_instead_of_claiming_current_path() {
        let mut library = Library::new(Vec::new());
        library.games = vec![game(4, "Missing", true, true)];
        let summary = ProblemSummary::from_library(&library, None);
        assert!(
            summary.problems[0]
                .location
                .starts_with("Last recorded path:")
        );
        assert!(
            !summary.problems[0]
                .affected
                .contains("/path/that/does/not/exist.zip")
        );
    }

    #[test]
    fn problem_filter_defaults_to_actionable_and_can_show_all_findings() {
        assert_eq!(ProblemFilter::default(), ProblemFilter::Actionable);
        assert!(ProblemFilter::Actionable.accepts(&Problem {
            id: "id".into(),
            title: "Needs review".into(),
            category: Category::Identity,
            severity: Severity::Warning,
            state: ProblemState::NeedsEvidence,
            kind: ProblemKind::DatMatchAvailable,
            primary: Some(ProblemAction::new(
                "Review matches",
                Route::Section(Section::Check)
            )),
            secondary: None,
            game_id: None,
            affected: "Arcade · Game".into(),
            location: "Current path: /game.zip".into(),
            why: "evidence".into(),
            action: "Review".into(),
            safety: "read-only".into(),
            undo: "none".into(),
            technical: "id".into(),
        }));
    }

    fn mame_game(id: i64, title: &str, missing: bool, verified: bool) -> Game {
        use archivefs_core::game_identity::*;
        let mut game = game(id, title, verified, missing);
        game.archive.identity_report = Some(GameIdentityReport {
            archive_path: game.archive.absolute_path.clone(),
            platform: IdentityPlatform::Arcade,
            format: IdentityImageFormat::LooseCartridgeRom,
            evidence: vec![IdentityEvidence {
                kind: IdentityKind::MameMachineName,
                status: if verified {
                    IdentityStatus::Verified
                } else {
                    IdentityStatus::Candidate
                },
                value: Some("pacman".into()),
                confidence: IdentityConfidence::ExactBytes,
                provenance: IdentityProvenance {
                    archive_path: game.archive.absolute_path.clone(),
                    member_path: None,
                    member_index: None,
                    method: "fixture".into(),
                },
                diagnostic: "fixture".into(),
            }],
            warnings: vec![],
            bytes_read: 1,
            archive_members_inspected: 0,
            metadata_paths_inspected: 0,
            nested_container_depth: 0,
            complete: true,
        });
        game
    }

    fn summary_for(games: Vec<Game>) -> ProblemSummary {
        let mut library = Library::new(Vec::new());
        library.games = games;
        ProblemSummary::from_library(&library, None)
    }

    #[test]
    fn proven_mame_problem_opens_the_existing_mame_workflow() {
        let summary = summary_for(vec![mame_game(10, "pacman", true, true)]);
        let problem = &summary.problems[0];
        let action = problem.primary.as_ref().unwrap();
        assert_eq!(problem.kind, ProblemKind::FileMame);
        assert_eq!(action.label, "Review in MAME");
        assert_eq!(action.route, Route::MameWorkflow);
        // the game is retained for the separate Game Details link only;
        // MameWorkflow carries no set/game payload, so none is invented
        assert_eq!(problem.game_id, Some(10));
    }

    #[test]
    fn mame_problem_offers_no_generic_rename_or_repair() {
        let summary = summary_for(vec![mame_game(10, "pacman", true, true)]);
        let action = summary.problems[0].action.to_lowercase();
        assert!(action.contains("mame workflow"));
        assert!(action.contains("not renamed or repaired"));
        let action = summary.problems[0].primary.as_ref().unwrap();
        assert_eq!(action.route, Route::MameWorkflow);
        assert_ne!(action.label, "Review missing games");
    }

    #[test]
    fn non_mame_and_unproven_problems_keep_existing_destinations() {
        // Arcade platform label alone, a candidate-only MAME name, and no
        // report at all are all not proof.
        let summary = summary_for(vec![
            game(1, "Plain arcade missing", true, true),
            mame_game(2, "candidate", true, false),
            game(3, "Plain unknown", false, false),
        ]);
        for problem in &summary.problems {
            assert!(
                problem
                    .primary
                    .as_ref()
                    .is_none_or(|a| a.route != Route::MameWorkflow),
                "{}",
                problem.id
            );
        }
        let by_id = |id: &str| summary.problems.iter().find(|p| p.id == id).unwrap();
        // The test files do not exist and neither does their folder.
        for id in ["missing-1", "missing-2"] {
            let action = by_id(id).primary.as_ref().unwrap();
            assert_eq!(action.label, "Review game folders");
            assert_eq!(action.route, Route::Section(Section::Sources));
        }
        let matching = by_id("identity-group-2--Arcade").primary.as_ref().unwrap();
        assert_eq!(matching.route, Route::Section(Section::Check));
    }

    #[test]
    fn filter_and_search_do_not_change_mame_classification() {
        let summary = summary_for(vec![mame_game(10, "pacman", true, true)]);
        let before = summary.problems[0].primary.clone();
        for filter in [ProblemFilter::Actionable, ProblemFilter::All] {
            let _ = filter.accepts(&summary.problems[0]);
        }
        let query = "pacman";
        let _visible = summary.problems[0].title.to_lowercase().contains(query);
        assert_eq!(summary.problems[0].primary, before);
        assert_eq!(before.unwrap().route, Route::MameWorkflow);
    }

    #[test]
    fn proves_mame_needs_verified_machine_name_only() {
        assert!(proves_mame(&mame_game(1, "a", false, true)));
        assert!(!proves_mame(&mame_game(2, "b", false, false)));
        assert!(!proves_mame(&game(3, "c", true, false)));
    }

    fn identity_summary(
        games: Vec<Game>,
        inventory: Option<ReferenceInventory>,
        matched: &[i64],
    ) -> ProblemSummary {
        let mut library = Library::new(Vec::new());
        library.games = games;
        library.identity_context = super::super::library::IdentityContext {
            inventory,
            matched: matched.iter().copied().collect(),
        };
        ProblemSummary::from_library(&library, None)
    }

    fn platform_game(id: i64, title: &str, platform: &str) -> Game {
        let mut game = game(id, title, false, false);
        game.platform = platform.into();
        game
    }

    #[test]
    fn ambiguous_evidence_is_a_per_game_choice_and_fails_closed() {
        let mut game = mame_game(5, "twin", false, false);
        game.archive.identity_report.as_mut().unwrap().evidence[0].status =
            archivefs_core::game_identity::IdentityStatus::Ambiguous;
        // even a reference-data match must not hide an ambiguous game
        let summary = identity_summary(vec![game], None, &[5]);
        assert_eq!(summary.problems.len(), 1);
        let problem = &summary.problems[0];
        assert_eq!(problem.severity, Severity::NeedsAttention);
        assert!(problem.title.contains("more than one possible match"));
        assert!(!problem.technical.contains("Identified"));
    }

    #[test]
    fn informational_identity_rows_do_not_count_as_attention() {
        let summary = identity_summary(
            vec![
                platform_game(1, "Mystery", "MSX"),
                platform_game(2, "Cool Demo (Homebrew)", "NES"),
                platform_game(3, "pacman", "Arcade"),
            ],
            None,
            &[3],
        );
        assert_eq!(summary.problems.len(), 3);
        assert_eq!(summary.actionable_count(), 0);
        assert_eq!(summary.count(Severity::NeedsAttention), 0);
        assert_eq!(summary.count(Severity::Informational), 3);
    }

    #[test]
    fn unmatched_games_are_one_actionable_group_per_system() {
        let summary = identity_summary(
            vec![
                platform_game(1, "A", "NES"),
                platform_game(2, "B", "NES"),
                platform_game(3, "C", "SNES"),
            ],
            None,
            &[],
        );
        assert_eq!(summary.problems.len(), 2);
        assert!(summary.problems.iter().all(|p| {
            p.state.is_actionable()
                && p.severity == Severity::Warning
                && p.kind == ProblemKind::DatMatchAvailable
                && p.primary
                    .as_ref()
                    .is_some_and(|a| a.route == Route::Section(Section::Check))
        }));
        assert!(
            summary
                .problems
                .iter()
                .any(|p| p.title.starts_with("NES: 2 games"))
        );
    }

    #[test]
    fn games_without_a_system_ask_for_a_system_choice() {
        let summary = identity_summary(
            vec![platform_game(
                1,
                "Who knows",
                super::super::library::UNKNOWN_PLATFORM,
            )],
            None,
            &[],
        );
        let action = summary.problems[0].primary.as_ref().unwrap();
        assert_eq!(summary.problems[0].kind, ProblemKind::NoSystem);
        assert_eq!(action.route, Route::Section(Section::Games));
        assert_eq!(
            action.context,
            ProblemContext::GamesSystem(super::super::library::UNKNOWN_PLATFORM.into())
        );
        assert!(summary.problems[0].title.contains("no system assigned"));
    }

    #[test]
    fn missing_identification_data_is_setup_only_when_proven_absent() {
        let (platform, _) = ("NES", ());
        let nothing = ReferenceInventory {
            platforms: Default::default(),
            ecosystems: Vec::new(),
            has_unattributed: false,
        };
        let proven = identity_summary(vec![platform_game(1, "A", platform)], Some(nothing), &[]);
        assert!(proven.problems[0].title.contains("not set up yet"));
        // an unattributed catalogue might cover it: only an ordinary action
        let unclear = ReferenceInventory {
            platforms: Default::default(),
            ecosystems: Vec::new(),
            has_unattributed: true,
        };
        let unproven = identity_summary(vec![platform_game(1, "A", platform)], Some(unclear), &[]);
        assert!(
            unproven.problems[0].title.contains("have not been matched")
                || unproven.problems[0].title.contains("has not been matched")
        );
    }

    #[test]
    fn nothing_is_promoted_to_verified_by_projection() {
        let games = vec![
            platform_game(1, "A", "NES"),
            platform_game(2, "pacman", "Arcade"),
        ];
        let mut library = Library::new(Vec::new());
        library.games = games;
        library.identity_context.matched.insert(2);
        let _ = ProblemSummary::from_library(&library, None);
        assert!(library.games.iter().all(|g| !g.identified));
    }

    #[test]
    fn home_and_problems_share_one_attention_number() {
        let mut library = Library::new(Vec::new());
        library.games = vec![
            game(1, "Missing", true, true),
            platform_game(2, "A", "NES"),
            platform_game(3, "Cool (Homebrew)", "NES"),
        ];
        let summary = ProblemSummary::from_library(&library, None);
        let listed = summary
            .problems
            .iter()
            .filter(|p| p.severity == Severity::NeedsAttention)
            .count();
        assert_eq!(summary.attention_count(), listed);
        assert_eq!(listed, 1);
    }

    // ---- action coverage -------------------------------------------------

    /// One real finding of the given kind, built by the same constructors the
    /// summary uses.
    pub(in crate::gui_v2) fn sample_problem(kind: ProblemKind) -> Problem {
        let dir = tempfile::tempdir().unwrap();
        let group = |key_kind: u8, platform: &str| {
            identity_group_problem(
                &IdentityGroupKey {
                    kind: key_kind,
                    reason: "No-Intro".into(),
                    platform: platform.into(),
                },
                &IdentityGroup {
                    count: 3,
                    samples: vec!["A".into()],
                },
            )
        };
        let mut missing_in_reachable_folder = game(1, "Gone", true, true);
        missing_in_reachable_folder.archive.absolute_path = dir.path().join("gone.zip");
        let mut unhealthy = game(2, "Broken", true, false);
        unhealthy.archive.last_known_health = "corrupt".into();
        let plain = game(3, "Plain", true, false);
        match kind {
            ProblemKind::FileMissing => file_problem(&missing_in_reachable_folder),
            ProblemKind::FileFolderUnavailable => file_problem(&game(4, "Away", true, true)),
            ProblemKind::FileUnhealthy => file_problem(&unhealthy),
            ProblemKind::FileMame => file_problem(&mame_game(5, "mame", true, true)),
            ProblemKind::IdentityConflict => {
                identity_choice_problem(&plain, ChoiceReason::Conflict)
            }
            ProblemKind::IdentityAmbiguous => {
                identity_choice_problem(&plain, ChoiceReason::Ambiguous)
            }
            ProblemKind::IdentityMame => {
                identity_choice_problem(&mame_game(6, "mame", false, true), ChoiceReason::Conflict)
            }
            ProblemKind::NoSystem => group(0, UNKNOWN_PLATFORM),
            ProblemKind::DatSetupRequired => group(1, "NES"),
            ProblemKind::DatMatchAvailable => group(2, "NES"),
            ProblemKind::NoReferenceSource => group(3, "Obscure"),
            ProblemKind::MatchedByReference => group(4, ""),
            ProblemKind::SpecialRelease => group(5, ""),
            ProblemKind::DuplicateGroup => {
                let report = DuplicateReport {
                    files_examined: 2,
                    exact_groups: Vec::new(),
                    groups: vec![super::super::library::DuplicateGroup {
                        exact_index: 0,
                        kind: "Exact duplicates".into(),
                        sha256: "abc".into(),
                        size_bytes: 4,
                        members: Vec::new(),
                    }],
                };
                ProblemSummary::from_library(&Library::new(Vec::new()), Some(&report))
                    .problems
                    .remove(0)
            }
        }
    }

    const VAGUE_LABELS: &[&str] = &[
        "Fix",
        "Resolve",
        "Continue",
        "Repair",
        "Advanced",
        "Open specialist interface",
        "OK",
    ];

    #[test]
    fn every_problem_kind_has_a_truthful_action_or_is_informational() {
        let mut seen = std::collections::BTreeSet::new();
        for kind in ProblemKind::ALL {
            let problem = sample_problem(kind);
            assert_eq!(problem.kind, kind, "{kind:?} built a different kind");
            seen.insert(kind);
            assert_eq!(
                problem.state.is_actionable(),
                kind.is_actionable(),
                "{kind:?}: state and kind disagree about needing action"
            );
            if kind.is_actionable() {
                let primary = problem
                    .primary
                    .as_ref()
                    .unwrap_or_else(|| panic!("{kind:?} is actionable but has no action"));
                for action in std::iter::once(primary).chain(problem.secondary.as_ref()) {
                    assert!(!action.label.trim().is_empty(), "{kind:?}");
                    assert!(
                        !VAGUE_LABELS.contains(&action.label.as_str()),
                        "{kind:?}: `{}` does not say what will happen",
                        action.label
                    );
                    // No escape into the legacy window: a game-scoped Task route is
                    // only valid for the sections that have a native page.
                    if let Route::Task { section, .. } = &action.route {
                        assert!(
                            matches!(section, Section::Check | Section::Problems),
                            "{kind:?}: Task route into {section:?} would fall back to the legacy handoff"
                        );
                    }
                }
                assert_ne!(problem.severity, Severity::Informational, "{kind:?}");
            } else {
                assert!(problem.primary.is_none(), "{kind:?} must not have a button");
                assert!(problem.secondary.is_none(), "{kind:?}");
                assert_eq!(problem.severity, Severity::Informational, "{kind:?}");
                assert_eq!(problem.state, ProblemState::Informational, "{kind:?}");
            }
        }
        assert_eq!(seen.len(), ProblemKind::ALL.len());
    }

    #[test]
    fn the_primary_actions_land_on_the_documented_destinations_with_context() {
        let action = |kind| sample_problem(kind).primary.unwrap();
        let a = action(ProblemKind::FileMissing);
        assert_eq!(
            (a.label.as_str(), &a.route, &a.context),
            (
                "Review missing games",
                &Route::Section(Section::Problems),
                &ProblemContext::MissingReview
            )
        );
        let a = action(ProblemKind::FileFolderUnavailable);
        assert_eq!(
            (a.label.as_str(), &a.route),
            ("Review game folders", &Route::Section(Section::Sources))
        );
        let a = action(ProblemKind::FileUnhealthy);
        assert_eq!(a.label, "Check this game");
        assert!(matches!(
            a.route,
            Route::Task {
                section: Section::Check,
                game: 2
            }
        ));
        assert_eq!(a.context, ProblemContext::CheckPlatform("Arcade".into()));
        let a = action(ProblemKind::IdentityConflict);
        assert_eq!(
            (a.label.as_str(), &a.route),
            ("Review evidence", &Route::ReviewIdentity(3))
        );
        let a = action(ProblemKind::IdentityAmbiguous);
        assert_eq!(
            (a.label.as_str(), &a.route),
            ("Review matches", &Route::ReviewIdentity(3))
        );
        let a = action(ProblemKind::NoSystem);
        assert_eq!(a.route, Route::Section(Section::Games));
        assert_eq!(
            a.context,
            ProblemContext::GamesSystem(UNKNOWN_PLATFORM.into())
        );
        let a = action(ProblemKind::DatSetupRequired);
        assert_eq!(
            (a.label.as_str(), &a.route),
            ("Set up identification data", &Route::Section(Section::Dat))
        );
        let a = action(ProblemKind::DatMatchAvailable);
        assert_eq!(
            (a.label.as_str(), &a.route),
            ("Review matches", &Route::Section(Section::Check))
        );
        assert_eq!(a.context, ProblemContext::CheckPlatform("NES".into()));
        let a = action(ProblemKind::DuplicateGroup);
        assert_eq!(a.context, ProblemContext::DuplicateGroup("abc".into()));
        for kind in [ProblemKind::FileMame, ProblemKind::IdentityMame] {
            assert_eq!(action(kind).route, Route::MameWorkflow, "{kind:?}");
        }
    }

    #[test]
    fn a_missing_file_always_also_offers_the_game_itself() {
        for kind in [
            ProblemKind::FileMissing,
            ProblemKind::FileFolderUnavailable,
            ProblemKind::FileUnhealthy,
            ProblemKind::FileMame,
        ] {
            let secondary = sample_problem(kind).secondary.unwrap();
            assert_eq!(secondary.label, "Show game", "{kind:?}");
            assert!(matches!(secondary.route, Route::Game(_)), "{kind:?}");
        }
    }
}

/// Test seam: the route a per-game identity-choice finding sends the person to.
#[cfg(test)]
pub(super) fn tests_support_identity_choice(game: &Game) -> Route {
    identity_choice_problem(game, ChoiceReason::Ambiguous)
        .primary
        .map(|action| action.route)
        .unwrap()
}
