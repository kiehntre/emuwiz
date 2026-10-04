//! Native GUI-v2 problem projection.
//!
//! This module is deliberately a presentation adapter. It does not scan the
//! filesystem, infer identities, or invent repair actions. Findings come from
//! the saved library state and the existing exact-duplicate proof.

use super::library::{DuplicateReport, Game, Library};
use super::routes::{Route, Section};
use archivefs_core::game_identity::{IdentityKind, IdentityStatus};
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
        for game in &library.games {
            if game.archive.last_verified_missing_at.is_some()
                || !path_is_present(game)
                || matches!(
                    game.archive.last_known_health.as_str(),
                    "missing" | "corrupt" | "damaged" | "error"
                )
            {
                problems.push(file_problem(game));
            } else if !game.identified {
                problems.push(identity_problem(game));
            }
        }
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

fn identity_problem(game: &Game) -> Problem {
    Problem {
        id: format!("identity-{}", game.archive.id),
        game_id: Some(game.archive.id),
        title: format!("{} needs identity review", game.title),
        category: Category::Identity,
        severity: Severity::Warning,
        state: ProblemState::NeedsEvidence,
        destination: if proves_mame(game) {
            ProblemDestination::Mame
        } else {
            ProblemDestination::CheckGames
        },
        affected: format!("{} · {}", game.platform, game.title),
        location: format!("Current path: {}", game.archive.absolute_path.display()),
        why:
            "EmuWiz has not established enough trusted evidence to say exactly which game this is."
                .into(),
        action: if proves_mame(game) {
            MAME_ACTION.into()
        } else {
            "Open the game details or verification page to review available evidence.".into()
        },
        safety: "Read-only. EmuWiz will not turn a filename hint into a verified identity.".into(),
        undo: "No file change was made, so there is nothing to undo.".into(),
        technical: format!("Catalogue id {} · identified=false", game.archive.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::PersistedArchive;

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
                .any(|p| p.title.contains("needs identity review"))
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
        assert_eq!(problem.destination, ProblemDestination::CheckGames);
        assert!(!problem.action.to_lowercase().contains("rename"));
        assert!(problem.location.starts_with("Current path:"));
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
            by_id("identity-3").destination,
            ProblemDestination::CheckGames
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
}
