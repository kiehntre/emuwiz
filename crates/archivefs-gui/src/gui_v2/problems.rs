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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProblemDestination {
    CheckGames,
    Games,
    Duplicates,
    /// Identification data (DAT) sources and matching.
    Dat,
    /// Findings that existing typed evidence proves are MAME set findings.
    /// MAME sets are judged as complete sets, so these go to the dedicated
    /// MAME workflow rather than a generic rename/repair page.
    Mame,
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

impl ProblemDestination {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::CheckGames => "Open Check Games",
            Self::Games => "Review Games",
            Self::Duplicates => "Review Duplicates",
            Self::Dat => "Open identification data",
            Self::Mame => "Review in MAME",
        }
    }

    /// The existing route this destination opens. `MameWorkflow` is a global
    /// route with no set/game payload, so no context is invented for it.
    pub(super) fn route(self) -> Route {
        match self {
            Self::CheckGames => Route::Section(Section::Check),
            Self::Games => Route::Section(Section::Games),
            Self::Duplicates => Route::Section(Section::Duplicates),
            Self::Dat => Route::Section(Section::Dat),
            Self::Mame => Route::MameWorkflow,
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
    pub(super) destination: ProblemDestination,
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
                    destination: ProblemDestination::Duplicates,
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
    let path_is_current = path_is_present(game);
    Problem {
        id: format!("missing-{}", game.archive.id),
        game_id: Some(game.archive.id),
        title: format!("{} is missing or has a saved health problem", game.title),
        category: Category::Files,
        severity: Severity::NeedsAttention,
        state: ProblemState::Current,
        destination: if proves_mame(game) {
            ProblemDestination::Mame
        } else {
            ProblemDestination::Games
        },
        affected: format!("{} · {}", game.platform, game.title),
        location: if path_is_current {
            format!("Current path: {}", game.archive.absolute_path.display())
        } else {
            format!("Last recorded path: {}", game.archive.absolute_path.display())
        },
        why: "EmuWiz cannot safely verify or prepare this game until the recorded file is available and readable.".into(),
        action: if proves_mame(game) {
            MAME_ACTION.into()
        } else {
            "Review the game and its folder, then run verification again.".into()
        },
        safety: "Read-only. Browsing and verification do not rename, move, delete, or repair the source file.".into(),
        undo: "No file change was made, so there is nothing to undo.".into(),
        technical: format!(
            "Catalogue id {} · recorded path {} · health {}",
            game.archive.id,
            game.archive.absolute_path.display(),
            game.archive.last_known_health
        ),
    }
}

/// A game whose evidence conflicts or is ambiguous: a person has to choose.
fn identity_choice_problem(game: &Game, reason: ChoiceReason) -> Problem {
    let (title, why, action) = match reason {
        ChoiceReason::Conflict => (
            format!("{} has conflicting identification evidence", game.title),
            "Two trusted sources disagree about which game this is, so EmuWiz will not pick one for you.",
            "Review the evidence in the game details and decide which one is right.",
        ),
        _ => (
            format!("{} has more than one possible match", game.title),
            "EmuWiz found several possible matches and will not guess between them.",
            "Choose the correct match in the game details.",
        ),
    };
    Problem {
        id: format!("identity-{}", game.archive.id),
        game_id: Some(game.archive.id),
        title,
        category: Category::Identity,
        severity: Severity::NeedsAttention,
        state: ProblemState::NeedsEvidence,
        destination: if proves_mame(game) {
            ProblemDestination::Mame
        } else {
            ProblemDestination::CheckGames
        },
        affected: format!("{} · {}", game.platform, game.title),
        location: format!("Current path: {}", game.archive.absolute_path.display()),
        why: why.into(),
        action: action.into(),
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
    let (title, why, action, destination, severity, state) = match key.kind {
        0 => (
            format!("{n} {games} {have} no system assigned"),
            "EmuWiz cannot launch or identify a game until it knows which system it belongs to.".to_string(),
            "Choose a system for these games.".to_string(),
            ProblemDestination::Games,
            Severity::Warning,
            ProblemState::NeedsEvidence,
        ),
        1 => (
            format!("{}: identification data is not set up yet", key.platform),
            format!("EmuWiz knows an identification database for this system ({}) but none is installed. {n} {games} can still be played; identification is optional.", key.reason),
            "Set up identification data for this system.".to_string(),
            ProblemDestination::Dat,
            Severity::Warning,
            ProblemState::NeedsEvidence,
        ),
        2 => (
            format!("{}: {n} {games} {have} not been matched to identification data yet", key.platform),
            "These games can still be played. Matching them gives EmuWiz stronger proof of exactly which release each one is.".to_string(),
            "Open identification data and match this system's games.".to_string(),
            ProblemDestination::Dat,
            Severity::Warning,
            ProblemState::NeedsEvidence,
        ),
        3 => (
            format!("{}: no identification database is available", key.platform),
            format!("EmuWiz does not know a reference database for this system, so there is nothing to match {n} {games} against. This is not a problem."),
            "No action is needed.".to_string(),
            ProblemDestination::Games,
            Severity::Informational,
            ProblemState::Informational,
        ),
        4 => (
            format!("{n} arcade {games} matched the reference data"),
            "These sets were checked against the MAME reference data. Nothing is wrong.".to_string(),
            "No action is needed.".to_string(),
            ProblemDestination::Mame,
            Severity::Informational,
            ProblemState::Informational,
        ),
        _ => (
            format!("{n} special {} (homebrew, prototypes, hacks, translations)", if n == 1 { "release" } else { "releases" }),
            "Normal identification databases do not describe these releases, so they stay unmatched. They can still be played.".to_string(),
            "No action is needed.".to_string(),
            ProblemDestination::Games,
            Severity::Informational,
            ProblemState::Informational,
        ),
    };
    Problem {
        id: format!("identity-group-{}-{}-{}", key.kind, key.reason, key.platform),
        game_id: None,
        title,
        category: Category::Identity,
        severity,
        state,
        destination,
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
mod tests {
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
        assert_eq!(
            first.problems[0].destination,
            ProblemDestination::Duplicates
        );
    }

    #[test]
    fn identity_findings_require_evidence_before_rename() {
        let mut library = Library::new(Vec::new());
        library.games = vec![game(3, "Unknown", false, false)];
        let summary = ProblemSummary::from_library(&library, None);
        let problem = &summary.problems[0];
        assert_eq!(problem.state, ProblemState::NeedsEvidence);
        assert_eq!(problem.destination, ProblemDestination::Dat);
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
            destination: ProblemDestination::CheckGames,
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
        assert_eq!(problem.destination, ProblemDestination::Mame);
        assert_eq!(problem.destination.label(), "Review in MAME");
        assert_eq!(problem.destination.route(), Route::MameWorkflow);
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
        assert_ne!(summary.problems[0].destination, ProblemDestination::Games);
        assert_ne!(summary.problems[0].destination.label(), "Review Games");
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
            assert_ne!(
                problem.destination,
                ProblemDestination::Mame,
                "{}",
                problem.id
            );
        }
        let by_id = |id: &str| summary.problems.iter().find(|p| p.id == id).unwrap();
        assert_eq!(by_id("missing-1").destination, ProblemDestination::Games);
        assert_eq!(by_id("missing-2").destination, ProblemDestination::Games);
        assert_eq!(
            by_id("identity-group-2--Arcade").destination,
            ProblemDestination::Dat
        );
        assert_eq!(
            ProblemDestination::Games.route(),
            Route::Section(Section::Games)
        );
    }

    #[test]
    fn filter_and_search_do_not_change_mame_classification() {
        let summary = summary_for(vec![mame_game(10, "pacman", true, true)]);
        let before = summary.problems[0].destination;
        for filter in [ProblemFilter::Actionable, ProblemFilter::All] {
            let _ = filter.accepts(&summary.problems[0]);
        }
        let query = "pacman";
        let _visible = summary.problems[0].title.to_lowercase().contains(query);
        assert_eq!(summary.problems[0].destination, before);
        assert_eq!(before, ProblemDestination::Mame);
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
                platform_game(1, "Mystery", "Acorn Electron"),
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
        assert!(summary.problems.iter().all(|p| p.state.is_actionable()
            && p.severity == Severity::Warning
            && p.destination == ProblemDestination::Dat));
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
        assert_eq!(summary.problems[0].destination, ProblemDestination::Games);
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
}
