//! Native GUI-v2 problem projection.
//!
//! This module is deliberately a presentation adapter. It does not scan the
//! filesystem, infer identities, or invent repair actions. Findings come from
//! the saved library state and the existing exact-duplicate proof.

use super::library::{DuplicateReport, Game, Library};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd)]
pub(super) enum Severity {
    NeedsAttention,
    Warning,
    Informational,
}

impl Severity {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NeedsAttention => "Needs attention",
            Self::Warning => "Warnings",
            Self::Informational => "Informational",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd)]
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
    pub(super) title: String,
    pub(super) category: Category,
    pub(super) severity: Severity,
    pub(super) affected: String,
    pub(super) why: String,
    pub(super) action: String,
    pub(super) safety: String,
    pub(super) undo: String,
    pub(super) technical: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ProblemSummary {
    pub(super) problems: Vec<Problem>,
}

impl ProblemSummary {
    pub(super) fn from_library(library: &Library, duplicates: Option<&DuplicateReport>) -> Self {
        let mut problems = Vec::new();
        for game in &library.games {
            if game.archive.last_verified_missing_at.is_some()
                || !game.archive.absolute_path.is_file()
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
            for (index, group) in report.groups.iter().enumerate() {
                problems.push(Problem {
                    id: format!("duplicate-{index}-{}", group.sha256),
                    title: format!("{} identical copies were found", group.members.len()),
                    category: Category::Duplicates,
                    severity: Severity::Warning,
                    affected: group.members.iter().map(|member| member.title.as_str()).collect::<Vec<_>>().join(", "),
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
        Self { problems }
    }

    pub(super) fn count(&self, severity: Severity) -> usize {
        self.problems
            .iter()
            .filter(|problem| problem.severity == severity)
            .count()
    }
}

fn file_problem(game: &Game) -> Problem {
    Problem {
        id: format!("missing-{}", game.archive.id),
        title: format!("{} is missing or has a saved health problem", game.title),
        category: Category::Files,
        severity: Severity::NeedsAttention,
        affected: format!("{} · {}", game.platform, game.archive.absolute_path.display()),
        why: "EmuWiz cannot safely verify or prepare this game until the recorded file is available and readable.".into(),
        action: "Review the game and its folder, then run verification again.".into(),
        safety: "Read-only. Browsing and verification do not rename, move, delete, or repair the source file.".into(),
        undo: "No file change was made, so there is nothing to undo.".into(),
        technical: format!("Catalogue id {} · health {}", game.archive.id, game.archive.last_known_health),
    }
}

fn identity_problem(game: &Game) -> Problem {
    Problem {
        id: format!("identity-{}", game.archive.id),
        title: format!("{} needs identity review", game.title),
        category: Category::Identity,
        severity: Severity::Warning,
        affected: format!(
            "{} · {}",
            game.platform,
            game.archive.absolute_path.display()
        ),
        why:
            "EmuWiz has not established enough trusted evidence to say exactly which game this is."
                .into(),
        action: "Open the game details or verification page to review available evidence.".into(),
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
            groups: vec![super::super::library::DuplicateGroup {
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
    }
}
