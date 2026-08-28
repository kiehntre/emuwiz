//! Read-only projection of an elected Playing Library into a RomM layout.
//!
//! This consumes the existing 1G1R plan and never scans or re-elects.  A
//! strong DAT platform identity is required; platform names, extensions and
//! source directories are never used to choose a RomM folder.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::dat::identity::{DatPlatformConfidence, DatPlatformIdentity};
use crate::platform_evidence_fusion::romm_platform_mapping::production_romm_slug;

use super::apply_adapter::build_playing_library_transaction;
use super::{ElectedGame, LinkedLibraryOperation, PlayingLibraryPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommProjectedGame {
    pub dat_entry_name: String,
    pub launcher: LinkedLibraryOperation,
    pub companions: Vec<LinkedLibraryOperation>,
}

/// Evidence that absolute symlink targets created for RomM are resolvable by
/// the RomM process. A host-only path is never sufficient: Docker does not
/// rewrite symlink targets when a bind mount is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RommVisibility {
    Unverified {
        host_root: Option<PathBuf>,
        romm_root: Option<PathBuf>,
    },
    VerifiedVisible {
        host_root: PathBuf,
        romm_root: PathBuf,
    },
}

impl RommVisibility {
    pub fn unverified(host_root: Option<PathBuf>, romm_root: Option<PathBuf>) -> Self {
        Self::Unverified {
            host_root,
            romm_root,
        }
    }

    /// The only currently supported verified contract: a reviewed same-path
    /// bind, such as host `/mnt` being visible as `/mnt` in the container.
    pub fn verified_same_path_bind(root: PathBuf) -> Result<Self, String> {
        if !root.is_absolute() {
            return Err("RomM visibility root must be absolute".to_string());
        }
        Ok(Self::VerifiedVisible {
            host_root: root.clone(),
            romm_root: root,
        })
    }

    pub fn is_verified(&self) -> bool {
        matches!(self, Self::VerifiedVisible { .. })
    }

    fn source_is_visible(&self, source: &Path) -> bool {
        match self {
            Self::VerifiedVisible {
                host_root,
                romm_root,
            } => host_root == romm_root && source.starts_with(host_root),
            Self::Unverified { .. } => false,
        }
    }

    pub fn description(&self) -> &'static str {
        if self.is_verified() {
            "VerifiedVisible (same-path bind)"
        } else {
            "Unverified — apply blocked"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RommLibraryProjectionPlan {
    pub destination_root: PathBuf,
    pub romm_root: PathBuf,
    pub canonical_platform_id: String,
    pub romm_platform_slug: String,
    pub games: Vec<RommProjectedGame>,
    pub excluded_elections: usize,
    pub unresolved_elections: usize,
    pub rejected_launchers: usize,
    pub total_files: usize,
    pub visibility: RommVisibility,
    /// A normal PlayingLibraryPlan with projected destinations, deliberately
    /// retained so the existing journaled symlink transaction builder can be
    /// reused unchanged.
    pub playing_library_plan: PlayingLibraryPlan,
}

fn project_operation(
    operation: &LinkedLibraryOperation,
    source_root: &Path,
    romm_root: &Path,
) -> Result<LinkedLibraryOperation, String> {
    let relative = operation
        .destination_path
        .strip_prefix(source_root)
        .map_err(|_| "playing-library destination escaped its configured root".to_string())?;
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        || relative.as_os_str().is_empty()
    {
        return Err("playing-library destination is not a safe relative path".to_string());
    }
    Ok(LinkedLibraryOperation {
        source_path: operation.source_path.clone(),
        destination_path: romm_root.join(relative),
    })
}

fn project_game(
    game: &ElectedGame,
    source_root: &Path,
    romm_root: &Path,
) -> Result<RommProjectedGame, String> {
    Ok(RommProjectedGame {
        dat_entry_name: game.dat_entry_name.clone(),
        launcher: project_operation(&game.launcher_operation, source_root, romm_root)?,
        companions: game
            .companion_operations
            .iter()
            .map(|operation| project_operation(operation, source_root, romm_root))
            .collect::<Result<_, _>>()?,
    })
}

/// Builds a RomM projection from an existing, already-elected plan.
/// `identity` must be a strong identity from the parsed catalogue, not a
/// filename or path hint.  The reviewed static mapping is used deliberately;
/// ambiguous/unmapped platforms fail closed.
pub fn build_romm_projection(
    plan: &PlayingLibraryPlan,
    identity: &DatPlatformIdentity,
    destination_root: PathBuf,
) -> Result<RommLibraryProjectionPlan, String> {
    build_romm_projection_with_visibility(
        plan,
        identity,
        destination_root,
        RommVisibility::unverified(None, None),
    )
}

pub fn build_romm_projection_with_visibility(
    plan: &PlayingLibraryPlan,
    identity: &DatPlatformIdentity,
    destination_root: PathBuf,
    visibility: RommVisibility,
) -> Result<RommLibraryProjectionPlan, String> {
    if !destination_root.is_absolute() {
        return Err("the RomM destination root must be an absolute path".to_string());
    }
    if !plan.conflicts.is_empty() {
        return Err(format!(
            "{} Playing Library destination conflict(s) must be resolved before RomM projection",
            plan.conflicts.len()
        ));
    }
    let (canonical_platform_id, confidence) = match identity {
        DatPlatformIdentity::Resolved {
            platform,
            confidence,
            ..
        } => (platform.clone(), *confidence),
        DatPlatformIdentity::Unknown => {
            return Err("RomM projection needs a verified platform identity".to_string());
        }
        DatPlatformIdentity::Ambiguous { .. } => {
            return Err("RomM projection refused an ambiguous platform identity".to_string());
        }
    };
    if confidence != DatPlatformConfidence::Strong {
        return Err("RomM projection needs strong platform evidence".to_string());
    }
    let slug = production_romm_slug(&canonical_platform_id, &Default::default(), None)
        .ok_or_else(|| format!("no reviewed RomM slug is mapped for {canonical_platform_id}"))?;
    let romm_root = destination_root.join("roms").join(&slug);

    let games = plan
        .elected_games
        .iter()
        .map(|game| project_game(game, &plan.destination_root, &romm_root))
        .collect::<Result<Vec<_>, _>>()?;
    let mut destinations = BTreeSet::new();
    let mut projected_games = Vec::with_capacity(games.len());
    let mut operations = Vec::new();
    for game in games {
        let all = std::iter::once(game.launcher.clone())
            .chain(game.companions.iter().cloned())
            .collect::<Vec<_>>();
        for operation in &all {
            if !destinations.insert(operation.destination_path.clone()) {
                return Err(format!(
                    "two elected releases require the same RomM destination: {}",
                    operation.destination_path.display()
                ));
            }
        }
        operations.extend(all);
        projected_games.push(game);
    }

    let mut projected_plan = plan.clone();
    projected_plan.destination_root = romm_root.clone();
    projected_plan.operations = operations;
    projected_plan.elected_games = projected_games
        .iter()
        .zip(plan.elected_games.iter())
        .map(|(game, original)| ElectedGame {
            dat_entry_name: game.dat_entry_name.clone(),
            family_root_name: original.family_root_name.clone(),
            explanation: original.explanation.clone(),
            launcher_operation: game.launcher.clone(),
            companion_operations: game.companions.clone(),
        })
        .collect();

    Ok(RommLibraryProjectionPlan {
        destination_root,
        romm_root,
        canonical_platform_id,
        romm_platform_slug: slug,
        total_files: projected_plan.operations.len(),
        visibility,
        games: projected_games,
        excluded_elections: plan.exclusions.len(),
        unresolved_elections: plan.unresolved_groups.len(),
        rejected_launchers: plan.rejected_launchers.len(),
        playing_library_plan: projected_plan,
    })
}

pub fn build_romm_projection_transaction(
    projection: &RommLibraryProjectionPlan,
    generation: u64,
) -> Result<crate::dat::rename_apply::model::RenameTransaction, String> {
    if !projection.visibility.is_verified() {
        return Err(
            "RomM apply is blocked: the source link targets are not verified visible to RomM"
                .to_string(),
        );
    }
    if !projection
        .playing_library_plan
        .operations
        .iter()
        .all(|operation| {
            projection
                .visibility
                .source_is_visible(&operation.source_path)
        })
    {
        return Err(
            "RomM apply is blocked: at least one source link target is outside the verified RomM-visible root"
                .to_string(),
        );
    }
    build_playing_library_transaction(&projection.playing_library_plan, generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::identity::DatPlatformConfidence;
    use crate::dat::rename_apply::executor::{ApplyExecution, HardConflictMode, apply_transaction};
    use crate::dat::rename_apply::journal::write_journal;
    use crate::dat::rename_apply::preflight::DirectoryPolicy;
    use crate::dat::rename_apply::rollback::rollback_transaction;
    use crate::playing_library::{
        CandidateEvidenceSummary, ElectionExplanation, PlayingLibraryPolicy,
    };
    use crate::safe_read::TrustedRoots;

    fn plan(root: &Path, source: &Path) -> PlayingLibraryPlan {
        let op = LinkedLibraryOperation {
            source_path: source.to_path_buf(),
            destination_path: root.join("game.cue"),
        };
        PlayingLibraryPlan {
            destination_root: root.to_path_buf(),
            policy: PlayingLibraryPolicy::default(),
            archives_examined: 1,
            families_examined: 1,
            elected_games: vec![ElectedGame {
                dat_entry_name: "Game".into(),
                family_root_name: "Game".into(),
                explanation: ElectionExplanation {
                    steps: vec![],
                    rejected: vec![],
                    winner_evidence: CandidateEvidenceSummary::unknown(),
                },
                launcher_operation: op,
                companion_operations: vec![],
            }],
            unresolved_groups: vec![],
            exclusions: vec![],
            singleton_families: 1,
            conflicts: vec![],
            operations: vec![],
            rejected_launchers: vec![],
        }
    }

    fn multi_file_plan(
        root: &Path,
        launcher_name: &str,
        companion_names: &[&str],
    ) -> PlayingLibraryPlan {
        let launcher = root.join(launcher_name);
        let mut result = plan(root, &launcher);
        result.elected_games[0].launcher_operation.destination_path = launcher;
        for name in companion_names {
            let operation = LinkedLibraryOperation {
                source_path: root.join(name),
                destination_path: root.join(name),
            };
            result.elected_games[0].companion_operations.push(operation);
        }
        result
    }

    #[test]
    fn verified_gba_projects_to_reviewed_slug() {
        let root = Path::new("/playing");
        let projection = build_romm_projection(
            &plan(root, Path::new("/source/game.cue")),
            &DatPlatformIdentity::Resolved {
                platform: "Game Boy Advance".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            PathBuf::from("/romm"),
        )
        .unwrap();
        assert_eq!(projection.romm_root, PathBuf::from("/romm/roms/gba"));
        assert_eq!(projection.games.len(), 1);
    }

    #[test]
    fn verified_game_boy_projects_to_reviewed_slug() {
        let projection = build_romm_projection(
            &plan(Path::new("/playing"), Path::new("/source/game.gb")),
            &DatPlatformIdentity::Resolved {
                platform: "Game Boy".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            PathBuf::from("/romm"),
        )
        .unwrap();
        assert_eq!(projection.romm_platform_slug, "gb");
    }

    #[test]
    fn verified_game_boy_color_projects_to_reviewed_slug() {
        let projection = build_romm_projection(
            &plan(Path::new("/playing"), Path::new("/source/game.gbc")),
            &DatPlatformIdentity::Resolved {
                platform: "Game Boy Color".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            PathBuf::from("/romm"),
        )
        .unwrap();
        assert_eq!(projection.romm_platform_slug, "gbc");
    }

    #[test]
    fn unknown_platform_fails_closed() {
        let result = build_romm_projection(
            &plan(Path::new("/p"), Path::new("/s/a")),
            &DatPlatformIdentity::Unknown,
            PathBuf::from("/r"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn source_conflicts_are_not_resurrected_by_projection() {
        let mut plan = plan(Path::new("/playing"), Path::new("/source/game.zip"));
        plan.conflicts
            .push(crate::playing_library::DestinationConflict {
                destination_basename: "game.zip".into(),
                contenders: vec!["one".into(), "two".into()],
                destinations: vec![],
            });
        let result = build_romm_projection(
            &plan,
            &DatPlatformIdentity::Resolved {
                platform: "Game Boy Advance".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            PathBuf::from("/romm"),
        );
        assert!(result.unwrap_err().contains("conflict"));
    }

    #[test]
    fn filename_only_platform_evidence_cannot_authorize_a_slug() {
        let result = build_romm_projection(
            &plan(Path::new("/playing"), Path::new("/source/game.gba")),
            &DatPlatformIdentity::Unknown,
            PathBuf::from("/romm"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn direct_projection_keeps_gdi_tracks_as_one_game() {
        let projection = build_romm_projection(
            &multi_file_plan(
                Path::new("/playing"),
                "game.gdi",
                &["track01.bin", "track02.raw"],
            ),
            &DatPlatformIdentity::Resolved {
                platform: "Dreamcast".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            PathBuf::from("/romm"),
        )
        .unwrap();
        assert_eq!(projection.games.len(), 1);
        assert_eq!(
            projection.games[0]
                .launcher
                .destination_path
                .extension()
                .unwrap(),
            "gdi"
        );
        assert_eq!(projection.games[0].companions.len(), 2);
    }

    #[test]
    fn direct_projection_keeps_m3u_canonical_and_disc_files_together() {
        let projection = build_romm_projection(
            &multi_file_plan(
                Path::new("/playing"),
                "game.m3u",
                &["disc1.cue", "disc2.cue", "disc1.bin"],
            ),
            &DatPlatformIdentity::Resolved {
                platform: "PSX".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            PathBuf::from("/romm"),
        )
        .unwrap();
        assert_eq!(projection.games.len(), 1);
        assert_eq!(
            projection.games[0]
                .launcher
                .destination_path
                .extension()
                .unwrap(),
            "m3u"
        );
        assert_eq!(projection.games[0].companions.len(), 3);
    }

    #[test]
    fn excluded_duplicate_candidates_do_not_create_extra_games() {
        let mut source_plan = plan(Path::new("/playing"), Path::new("/source/game.gba"));
        source_plan
            .exclusions
            .push(crate::playing_library::ExcludedCandidate {
                dat_entry_name: "same game duplicate".into(),
                source_path: PathBuf::from("/source/duplicate.gba"),
                excluded_classes: vec!["exact duplicate excluded by election".into()],
            });
        let projection = build_romm_projection(
            &source_plan,
            &DatPlatformIdentity::Resolved {
                platform: "Game Boy Advance".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            PathBuf::from("/romm"),
        )
        .unwrap();
        assert_eq!(projection.games.len(), 1);
        assert_eq!(projection.excluded_elections, 1);
    }

    #[test]
    fn duplicate_projected_destinations_fail_closed() {
        let root = Path::new("/playing");
        let mut source_plan = plan(root, Path::new("/source/one.gba"));
        source_plan.elected_games.push(ElectedGame {
            dat_entry_name: "Second".into(),
            family_root_name: "Second".into(),
            explanation: source_plan.elected_games[0].explanation.clone(),
            launcher_operation: LinkedLibraryOperation {
                source_path: PathBuf::from("/source/two.gba"),
                destination_path: root.join("game.cue"),
            },
            companion_operations: vec![],
        });
        let result = build_romm_projection(
            &source_plan,
            &DatPlatformIdentity::Resolved {
                platform: "Game Boy Advance".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            PathBuf::from("/romm"),
        );
        assert!(result.unwrap_err().contains("same RomM destination"));
    }

    #[test]
    fn unverified_visibility_blocks_apply_but_verified_same_path_allows_it() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("game.gba");
        std::fs::write(&source, b"game").unwrap();
        let mut source_plan = plan(temp.path(), &source);
        source_plan.elected_games[0]
            .launcher_operation
            .destination_path = temp.path().join("game.gba");
        let unverified = build_romm_projection(
            &source_plan,
            &strong("Game Boy Advance"),
            temp.path().join("romm"),
        )
        .unwrap();
        assert!(build_romm_projection_transaction(&unverified, 1).is_err());
        let verified = build_romm_projection_with_visibility(
            &source_plan,
            &strong("Game Boy Advance"),
            temp.path().join("romm"),
            RommVisibility::verified_same_path_bind(temp.path().to_path_buf()).unwrap(),
        )
        .unwrap();
        assert!(build_romm_projection_transaction(&verified, 1).is_ok());
    }

    fn strong(platform: &str) -> DatPlatformIdentity {
        DatPlatformIdentity::Resolved {
            platform: platform.into(),
            machine_key: None,
            confidence: DatPlatformConfidence::Strong,
            evidence: vec![],
        }
    }

    #[test]
    fn cue_launcher_and_companion_remain_one_projected_game() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("disc.bin");
        std::fs::write(&source, b"track").unwrap();
        let plan_root = temp.path().join("playing");
        let mut plan = plan(&plan_root, &temp.path().join("disc.cue"));
        plan.elected_games[0]
            .companion_operations
            .push(LinkedLibraryOperation {
                source_path: source.clone(),
                destination_path: plan_root.join("disc.bin"),
            });
        let projection = build_romm_projection(
            &plan,
            &DatPlatformIdentity::Resolved {
                platform: "PSX".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            temp.path().join("romm"),
        )
        .unwrap();
        assert_eq!(projection.games.len(), 1);
        assert_eq!(projection.total_files, 2);
        assert_eq!(projection.playing_library_plan.operations.len(), 2);
        assert!(
            projection.games[0]
                .launcher
                .destination_path
                .ends_with("game.cue")
        );
        assert!(
            projection.games[0].companions[0]
                .destination_path
                .ends_with("disc.bin")
        );
    }

    #[test]
    fn projected_multi_file_release_uses_shared_apply_and_rollback() {
        let temp = tempfile::tempdir().unwrap();
        let cue = temp.path().join("game.cue");
        let bin = temp.path().join("disc.bin");
        std::fs::write(&cue, b"FILE \"disc.bin\" BINARY\n").unwrap();
        std::fs::write(&bin, b"track").unwrap();
        let playing_root = temp.path().join("playing");
        let mut source_plan = plan(&playing_root, &cue);
        source_plan.elected_games[0]
            .companion_operations
            .push(LinkedLibraryOperation {
                source_path: bin.clone(),
                destination_path: playing_root.join("disc.bin"),
            });
        let projection = build_romm_projection_with_visibility(
            &source_plan,
            &DatPlatformIdentity::Resolved {
                platform: "PSX".into(),
                machine_key: None,
                confidence: DatPlatformConfidence::Strong,
                evidence: vec![],
            },
            temp.path().join("romm"),
            RommVisibility::verified_same_path_bind(temp.path().to_path_buf()).unwrap(),
        )
        .unwrap();
        let mut transaction = build_romm_projection_transaction(&projection, 1).unwrap();
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        std::fs::create_dir_all(&projection.romm_root).unwrap();
        write_journal(&journal_dir, &transaction).unwrap();
        let approved_paths = transaction
            .entries
            .iter()
            .map(|entry| entry.source_path.to_string_lossy().into_owned())
            .collect();
        apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths,
            current_generation: 1,
            trusted: TrustedRoots::from_paths([cue.parent().unwrap(), &projection.romm_root]),
            journal_dir: journal_dir.clone(),
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &std::sync::atomic::AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .unwrap();
        assert!(projection.romm_root.join("game.cue").is_symlink());
        assert!(projection.romm_root.join("disc.bin").is_symlink());
        assert_eq!(std::fs::read(&cue).unwrap(), b"FILE \"disc.bin\" BINARY\n");
        assert_eq!(std::fs::read(&bin).unwrap(), b"track");
        rollback_transaction(
            &mut transaction,
            &journal_dir,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert!(!projection.romm_root.join("game.cue").exists());
        assert!(!projection.romm_root.join("disc.bin").exists());
    }

    #[test]
    fn existing_destination_blocks_projection_apply_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("game.gba");
        std::fs::write(&source, b"game").unwrap();
        let projection = build_romm_projection_with_visibility(
            &plan(temp.path(), &source),
            &strong("Game Boy Advance"),
            temp.path().join("romm"),
            RommVisibility::verified_same_path_bind(temp.path().to_path_buf()).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(&projection.romm_root).unwrap();
        let blocked = projection.romm_root.join("game.cue");
        std::fs::write(&blocked, b"unrelated").unwrap();
        let mut transaction = build_romm_projection_transaction(&projection, 1).unwrap();
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        write_journal(&journal_dir, &transaction).unwrap();
        let approved_paths = transaction
            .entries
            .iter()
            .map(|entry| entry.source_path.to_string_lossy().into_owned())
            .collect();
        assert!(
            apply_transaction(&mut ApplyExecution {
                transaction: &mut transaction,
                approved_paths,
                current_generation: 1,
                trusted: TrustedRoots::from_paths([temp.path()]),
                journal_dir,
                hard_conflict_mode: HardConflictMode::AbortAll,
                cancel: &std::sync::atomic::AtomicBool::new(false),
                directory_policy: DirectoryPolicy::SameFilesystem,
                allow_symlink_source: false,
            })
            .is_err()
        );
        assert_eq!(std::fs::read(&blocked).unwrap(), b"unrelated");
        assert!(!source.is_symlink());
    }

    #[test]
    fn identical_projection_reapply_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("game.gba");
        std::fs::write(&source, b"game").unwrap();
        let projection = build_romm_projection_with_visibility(
            &plan(temp.path(), &source),
            &strong("Game Boy Advance"),
            temp.path().join("romm"),
            RommVisibility::verified_same_path_bind(temp.path().to_path_buf()).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(&projection.romm_root).unwrap();
        let journal_dir = temp.path().join("journal");
        std::fs::create_dir_all(&journal_dir).unwrap();
        for generation in [1, 2] {
            let mut transaction =
                build_romm_projection_transaction(&projection, generation).unwrap();
            write_journal(&journal_dir, &transaction).unwrap();
            let approved_paths = transaction
                .entries
                .iter()
                .map(|entry| entry.source_path.to_string_lossy().into_owned())
                .collect();
            let outcome = apply_transaction(&mut ApplyExecution {
                transaction: &mut transaction,
                approved_paths,
                current_generation: generation,
                trusted: TrustedRoots::from_paths([temp.path()]),
                journal_dir: journal_dir.clone(),
                hard_conflict_mode: HardConflictMode::AbortAll,
                cancel: &std::sync::atomic::AtomicBool::new(false),
                directory_policy: DirectoryPolicy::SameFilesystem,
                allow_symlink_source: false,
            })
            .unwrap();
            assert_eq!(outcome.summary.applied, 1);
        }
        assert!(projection.romm_root.join("game.cue").is_symlink());
        assert_eq!(std::fs::read(&source).unwrap(), b"game");
    }
}
