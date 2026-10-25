//! Safe projection of an elected Playing Library into a RetroDECK/ES-DE tree.
//!
//! This module consumes an existing [`PlayingLibraryPlan`]. It does not scan,
//! hash, group, elect, or infer platforms. RetroDECK is distributed as a
//! Flatpak, so absolute symlink targets require an explicit same-path bind
//! contract before apply is permitted.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::dat::identity::{DatPlatformConfidence, DatPlatformIdentity};
use crate::emulator_environment::es_de::EsDeProfile;
use crate::launch::es_de_export::es_de_system_for_platform;
use crate::launch::es_de_publish::{EsDeGamelistPublication, plan_es_de_gamelist_publication};

use super::apply_adapter::build_playing_library_transaction;
use super::{ElectedGame, LinkedLibraryOperation, PlayingLibraryPlan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetroDeckProjectedGame {
    pub dat_entry_name: String,
    pub launcher: LinkedLibraryOperation,
    pub companions: Vec<LinkedLibraryOperation>,
}

/// Explicit evidence that the host paths used by symlink targets are visible
/// to RetroDECK at the same absolute paths. No Docker/Flatpak probing is done
/// implicitly; a caller must provide this reviewed result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetroDeckVisibility {
    Unverified {
        host_source_root: Option<PathBuf>,
        sandbox_source_root: Option<PathBuf>,
        host_destination_root: Option<PathBuf>,
        sandbox_destination_root: Option<PathBuf>,
    },
    VerifiedVisible {
        host_source_root: PathBuf,
        sandbox_source_root: PathBuf,
        host_destination_root: PathBuf,
        sandbox_destination_root: PathBuf,
    },
}

impl RetroDeckVisibility {
    pub fn unverified(
        host_source_root: Option<PathBuf>,
        sandbox_source_root: Option<PathBuf>,
        host_destination_root: Option<PathBuf>,
        sandbox_destination_root: Option<PathBuf>,
    ) -> Self {
        Self::Unverified {
            host_source_root,
            sandbox_source_root,
            host_destination_root,
            sandbox_destination_root,
        }
    }

    /// Constructs the currently supported safe contract: both roots are
    /// visible at the same absolute path in the host and RetroDECK sandbox.
    pub fn verified_same_path_bind(
        source_root: PathBuf,
        destination_root: PathBuf,
    ) -> Result<Self, String> {
        if !source_root.is_absolute() || !destination_root.is_absolute() {
            return Err("RetroDECK visibility roots must be absolute".to_string());
        }
        Ok(Self::VerifiedVisible {
            host_source_root: source_root.clone(),
            sandbox_source_root: source_root,
            host_destination_root: destination_root.clone(),
            sandbox_destination_root: destination_root,
        })
    }

    pub fn is_verified(&self) -> bool {
        matches!(self, Self::VerifiedVisible { .. })
    }

    fn permits(&self, source: &Path, destination: &Path) -> bool {
        match self {
            Self::VerifiedVisible {
                host_source_root,
                sandbox_source_root,
                host_destination_root,
                sandbox_destination_root,
            } => {
                host_source_root == sandbox_source_root
                    && host_destination_root == sandbox_destination_root
                    && source.starts_with(host_source_root)
                    && destination.starts_with(host_destination_root)
            }
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
pub struct RetroDeckProjectionPlan {
    pub destination_root: PathBuf,
    pub retrodeck_rom_root: PathBuf,
    pub canonical_platform_id: String,
    pub es_de_system: &'static str,
    pub games: Vec<RetroDeckProjectedGame>,
    pub excluded_elections: usize,
    pub unresolved_elections: usize,
    pub rejected_launchers: usize,
    pub total_files: usize,
    pub visibility: RetroDeckVisibility,
    pub es_de_publication: EsDeGamelistPublication,
    pub playing_library_plan: PlayingLibraryPlan,
}

fn project_operation(
    operation: &LinkedLibraryOperation,
    source_root: &Path,
    destination_root: &Path,
) -> Result<LinkedLibraryOperation, String> {
    let relative = operation
        .destination_path
        .strip_prefix(source_root)
        .map_err(|_| "playing-library destination escaped its configured root".to_string())?;
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("playing-library destination is not a safe relative path".to_string());
    }
    Ok(LinkedLibraryOperation {
        source_path: operation.source_path.clone(),
        destination_path: destination_root.join(relative),
    })
}

fn project_game(
    game: &ElectedGame,
    source_root: &Path,
    destination_root: &Path,
) -> Result<RetroDeckProjectedGame, String> {
    Ok(RetroDeckProjectedGame {
        dat_entry_name: game.dat_entry_name.clone(),
        launcher: project_operation(&game.launcher_operation, source_root, destination_root)?,
        companions: game
            .companion_operations
            .iter()
            .map(|operation| project_operation(operation, source_root, destination_root))
            .collect::<Result<_, _>>()?,
    })
}

pub fn build_retrodeck_projection(
    plan: &PlayingLibraryPlan,
    identity: &DatPlatformIdentity,
    destination_root: PathBuf,
    visibility: RetroDeckVisibility,
    es_de_profile: &EsDeProfile,
) -> Result<RetroDeckProjectionPlan, String> {
    if !destination_root.is_absolute() {
        return Err("the RetroDECK destination root must be absolute".to_string());
    }
    if !plan.conflicts.is_empty() {
        return Err(format!(
            "{} Playing Library destination conflict(s) must be resolved before RetroDECK projection",
            plan.conflicts.len()
        ));
    }
    let canonical_platform_id = match identity {
        DatPlatformIdentity::Resolved {
            platform,
            confidence: DatPlatformConfidence::Strong,
            ..
        } => platform.clone(),
        DatPlatformIdentity::Resolved { .. } => {
            return Err("RetroDECK projection needs strong platform evidence".to_string());
        }
        DatPlatformIdentity::Unknown => {
            return Err("RetroDECK projection needs verified platform identity".to_string());
        }
        DatPlatformIdentity::Ambiguous { .. } => {
            return Err("RetroDECK projection refused an ambiguous platform identity".to_string());
        }
    };
    let mapping = es_de_system_for_platform(&canonical_platform_id)
        .ok_or_else(|| format!("no reviewed ES-DE system mapping for {canonical_platform_id}"))?;
    let retrodeck_rom_root = destination_root.join("roms").join(mapping.es_de_system);
    let games = plan
        .elected_games
        .iter()
        .map(|game| project_game(game, &plan.destination_root, &retrodeck_rom_root))
        .collect::<Result<Vec<_>, _>>()?;

    let mut destinations = BTreeSet::new();
    let mut operations = Vec::new();
    for game in &games {
        for operation in std::iter::once(&game.launcher).chain(game.companions.iter()) {
            if !destinations.insert(operation.destination_path.clone()) {
                return Err(format!(
                    "two elected releases require the same RetroDECK destination: {}",
                    operation.destination_path.display()
                ));
            }
            operations.push(operation.clone());
        }
    }
    let mut projected_plan = plan.clone();
    projected_plan.destination_root = retrodeck_rom_root.clone();
    projected_plan.operations = operations;
    projected_plan.elected_games = games
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
    let es_de_publication =
        plan_es_de_gamelist_publication(&projected_plan, &canonical_platform_id, es_de_profile)
            .map_err(|error| error.to_string())?;

    Ok(RetroDeckProjectionPlan {
        destination_root,
        retrodeck_rom_root,
        canonical_platform_id,
        es_de_system: mapping.es_de_system,
        games,
        excluded_elections: plan.exclusions.len(),
        unresolved_elections: plan.unresolved_groups.len(),
        rejected_launchers: plan.rejected_launchers.len(),
        total_files: projected_plan.operations.len(),
        visibility,
        es_de_publication,
        playing_library_plan: projected_plan,
    })
}

pub fn build_retrodeck_projection_transaction(
    projection: &RetroDeckProjectionPlan,
    generation: u64,
) -> Result<crate::dat::rename_apply::model::RenameTransaction, String> {
    if !projection.visibility.is_verified() {
        return Err(
            "RetroDECK apply is blocked: source and destination visibility is not verified"
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
                .permits(&operation.source_path, &operation.destination_path)
        })
    {
        return Err(
            "RetroDECK apply is blocked: a link target is outside the verified visible roots"
                .to_string(),
        );
    }
    let transaction =
        build_playing_library_transaction(&projection.playing_library_plan, generation)?;
    if transaction.entries.len() != projection.playing_library_plan.operations.len() {
        return Err(
            "RetroDECK apply is blocked: one or more launcher/companion files are missing or no longer regular files"
                .to_string(),
        );
    }
    Ok(transaction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::identity::DatPlatformConfidence;
    use crate::playing_library::{
        CandidateEvidenceSummary, ElectionExplanation, PlayingLibraryPolicy,
    };

    fn identity(platform: &str) -> DatPlatformIdentity {
        DatPlatformIdentity::Resolved {
            platform: platform.into(),
            machine_key: None,
            confidence: DatPlatformConfidence::Strong,
            evidence: vec![],
        }
    }

    fn plan(root: &Path, launcher: &str, companions: &[&str]) -> PlayingLibraryPlan {
        let launcher = root.join(launcher);
        let game = ElectedGame {
            dat_entry_name: "Game".into(),
            family_root_name: "Game".into(),
            explanation: ElectionExplanation {
                steps: vec![],
                rejected: vec![],
                winner_evidence: CandidateEvidenceSummary::unknown(),
            },
            launcher_operation: LinkedLibraryOperation {
                source_path: launcher.clone(),
                destination_path: root.join(launcher.file_name().unwrap()),
            },
            companion_operations: companions
                .iter()
                .map(|name| LinkedLibraryOperation {
                    source_path: root.join(name),
                    destination_path: root.join(name),
                })
                .collect(),
        };
        let operations = game.all_operations().cloned().collect();
        PlayingLibraryPlan {
            destination_root: root.to_path_buf(),
            policy: PlayingLibraryPolicy::default(),
            archives_examined: 1,
            families_examined: 1,
            elected_games: vec![game],
            unresolved_groups: vec![],
            exclusions: vec![],
            singleton_families: 1,
            conflicts: vec![],
            operations,
            rejected_launchers: vec![],
        }
    }

    fn profile(root: &Path) -> EsDeProfile {
        use crate::emulator_environment::es_de::{
            DiscoveryEnvironment, discover_es_de_environment,
        };
        let home = root.to_path_buf();
        std::fs::create_dir_all(home.join("ES-DE/custom_systems")).unwrap();
        let systems = r#"<systemList>
<system><name>psx</name><fullname>Sony PlayStation</fullname><path>/tmp/retrodeck-test-roms/psx</path><extension>.cue .bin</extension><platform>psx</platform><theme>psx</theme></system>
<system><name>ps2</name><fullname>Sony PlayStation 2</fullname><path>/tmp/retrodeck-test-roms/ps2</path><extension>.iso</extension><platform>ps2</platform><theme>ps2</theme></system>
<system><name>dreamcast</name><fullname>Sega Dreamcast</fullname><path>/tmp/retrodeck-test-roms/dreamcast</path><extension>.gdi .bin</extension><platform>dreamcast</platform><theme>dreamcast</theme></system>
<system><name>saturn</name><fullname>Sega Saturn</fullname><path>/tmp/retrodeck-test-roms/saturn</path><extension>.cue .bin</extension><platform>saturn</platform><theme>saturn</theme></system>
<system><name>segacd</name><fullname>Sega CD</fullname><path>/tmp/retrodeck-test-roms/segacd</path><extension>.cue .bin .m3u</extension><platform>segacd</platform><theme>segacd</theme></system>
<system><name>gb</name><fullname>Game Boy</fullname><path>/tmp/retrodeck-test-roms/gb</path><extension>.gb</extension><platform>gb</platform><theme>gb</theme></system>
<system><name>gba</name><fullname>Game Boy Advance</fullname><path>/tmp/retrodeck-test-roms/gba</path><extension>.gba</extension><platform>gba</platform><theme>gba</theme></system>
</systemList>"#;
        std::fs::write(home.join("ES-DE/custom_systems/es_systems.xml"), systems).unwrap();
        let report = discover_es_de_environment(
            &crate::emulator_environment::HostReadOnlyFilesystem,
            &DiscoveryEnvironment {
                home: Some(home.into_os_string()),
                path: Some("".into()),
                explicit_bundled_systems_files: vec![],
                appimage_search_roots: vec![],
                explicit_root: None,
                explicit_appimages: vec![],
                explicit_portables: vec![],
            },
        )
        .unwrap();
        report
            .profiles
            .into_iter()
            .find(|profile| !profile.system_data.is_empty())
            .expect("test ES-DE profile")
    }

    #[test]
    fn reviewed_game_boy_and_gba_mappings_are_consumed() {
        let root = Path::new("/playing");
        let fixture = tempfile::tempdir().unwrap();
        for (platform, expected) in [
            ("PS2", "ps2"),
            ("Saturn", "saturn"),
            ("Dreamcast", "dreamcast"),
            ("Sega CD", "segacd"),
        ] {
            let result = build_retrodeck_projection(
                &plan(root, "game.rom", &[]),
                &identity(platform),
                PathBuf::from("/retrodeck"),
                RetroDeckVisibility::unverified(None, None, None, None),
                &profile(fixture.path()),
            );
            assert_eq!(result.unwrap().es_de_system, expected);
        }
    }

    #[test]
    fn unknown_identity_and_unverified_visibility_fail_closed() {
        let fixture = tempfile::tempdir().unwrap();
        let result = build_retrodeck_projection(
            &plan(Path::new("/playing"), "game.gba", &[]),
            &DatPlatformIdentity::Unknown,
            PathBuf::from("/retrodeck"),
            RetroDeckVisibility::unverified(None, None, None, None),
            &profile(fixture.path()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn complete_disc_releases_keep_one_launcher_and_all_companions() {
        let fixture = tempfile::tempdir().unwrap();
        let esde = profile(fixture.path());
        for (platform, launcher, companions) in [
            ("PS2", "game.iso", vec![]),
            ("Saturn", "game.cue", vec!["track01.bin"]),
            ("Dreamcast", "game.gdi", vec!["track01.bin", "track02.raw"]),
            ("Sega CD", "game.m3u", vec!["disc1.cue", "disc2.cue"]),
        ] {
            let names: Vec<&str> = companions;
            let result = build_retrodeck_projection(
                &plan(Path::new("/playing"), launcher, &names),
                &identity(platform),
                PathBuf::from("/retrodeck"),
                RetroDeckVisibility::unverified(None, None, None, None),
                &esde,
            )
            .unwrap();
            assert_eq!(result.games.len(), 1);
            assert_eq!(result.games[0].companions.len(), names.len());
            assert_eq!(result.es_de_publication.added.len(), 1);
            assert_eq!(
                result.es_de_publication.added[0].destination_path,
                result.games[0].launcher.destination_path
            );
        }
    }

    #[test]
    fn visibility_is_required_for_transaction_but_preview_is_available() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("playing");
        let destination = fixture.path().join("retrodeck");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("game.cue"), b"cue").unwrap();
        let projection = build_retrodeck_projection(
            &plan(&source, "game.cue", &[]),
            &identity("PSX"),
            destination.clone(),
            RetroDeckVisibility::unverified(
                Some(source.clone()),
                None,
                Some(destination.clone()),
                None,
            ),
            &profile(fixture.path()),
        )
        .unwrap();
        assert!(!projection.visibility.is_verified());
        assert!(build_retrodeck_projection_transaction(&projection, 1).is_err());
        let verified = build_retrodeck_projection(
            &plan(&source, "game.cue", &[]),
            &identity("PSX"),
            destination.clone(),
            RetroDeckVisibility::verified_same_path_bind(source, destination).unwrap(),
            &profile(fixture.path()),
        )
        .unwrap();
        assert!(build_retrodeck_projection_transaction(&verified, 1).is_ok());
    }

    #[test]
    fn missing_companion_is_refused_before_retrodeck_mutation() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("game.gdi"), b"gdi").unwrap();
        let projection = build_retrodeck_projection(
            &plan(&source, "game.gdi", &["missing.bin"]),
            &identity("Dreamcast"),
            destination.clone(),
            RetroDeckVisibility::verified_same_path_bind(source, destination).unwrap(),
            &profile(fixture.path()),
        )
        .unwrap();
        assert!(build_retrodeck_projection_transaction(&projection, 1).is_err());
        assert!(!projection.retrodeck_rom_root.join("game.gdi").exists());
    }

    #[test]
    fn projected_parent_traversal_is_refused() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("game.iso"), b"iso").unwrap();
        let mut unsafe_plan = plan(&source, "game.iso", &[]);
        unsafe_plan.elected_games[0]
            .launcher_operation
            .destination_path = source.join("../outside/game.iso");
        unsafe_plan.operations = unsafe_plan.elected_games[0]
            .all_operations()
            .cloned()
            .collect();
        assert!(
            build_retrodeck_projection(
                &unsafe_plan,
                &identity("PS2"),
                fixture.path().join("destination"),
                RetroDeckVisibility::unverified(None, None, None, None),
                &profile(fixture.path()),
            )
            .is_err()
        );
    }

    #[test]
    fn real_projection_apply_publishes_one_launcher_and_rolls_back_everything() {
        use crate::dat::rename_apply::executor::{
            ApplyExecution, HardConflictMode, apply_transaction,
        };
        use crate::dat::rename_apply::journal::write_journal;
        use crate::dat::rename_apply::preflight::DirectoryPolicy;
        use crate::launch::es_de_publish::{
            apply_es_de_gamelist_publication, rollback_es_de_gamelist_publication,
        };
        use crate::safe_read::TrustedRoots;
        use std::sync::atomic::AtomicBool;

        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source with spaces");
        let destination = fixture.path().join("Retro Deck Ω");
        std::fs::create_dir_all(&source).unwrap();
        for name in ["game.cue", "track 01.bin", "track 02.bin"] {
            std::fs::write(source.join(name), name.as_bytes()).unwrap();
        }
        let esde = profile(fixture.path());
        let gamelist = PathBuf::from(
            &esde
                .system_data
                .iter()
                .find(|entry| entry.system_name == "psx")
                .unwrap()
                .gamelist_file
                .path
                .display,
        );
        std::fs::create_dir_all(gamelist.parent().unwrap()).unwrap();
        let original = "<?xml version=\"1.0\"?>\n<gameList>\n\t<!-- keep -->\n</gameList>\n";
        std::fs::write(&gamelist, original).unwrap();
        let projection = build_retrodeck_projection(
            &plan(&source, "game.cue", &["track 01.bin", "track 02.bin"]),
            &identity("PSX"),
            destination.clone(),
            RetroDeckVisibility::verified_same_path_bind(source.clone(), destination.clone())
                .unwrap(),
            &esde,
        )
        .unwrap();
        assert_eq!(projection.games.len(), 1);
        assert_eq!(projection.total_files, 3);
        assert_eq!(projection.es_de_publication.added.len(), 1);
        assert_eq!(projection.es_de_publication.gamelist_path, gamelist);
        let mut transaction = build_retrodeck_projection_transaction(&projection, 9).unwrap();
        std::fs::create_dir_all(&projection.retrodeck_rom_root).unwrap();
        let journal = fixture.path().join("journal");
        write_journal(&journal, &transaction).unwrap();
        let approved_paths = transaction
            .entries
            .iter()
            .map(|entry| entry.source_path.to_string_lossy().into_owned())
            .collect();
        let outcome = apply_transaction(&mut ApplyExecution {
            transaction: &mut transaction,
            approved_paths,
            current_generation: 9,
            trusted: TrustedRoots::from_paths([
                source.clone(),
                projection.retrodeck_rom_root.clone(),
            ]),
            journal_dir: journal,
            hard_conflict_mode: HardConflictMode::AbortAll,
            cancel: &AtomicBool::new(false),
            directory_policy: DirectoryPolicy::SameFilesystem,
            allow_symlink_source: false,
        })
        .unwrap();
        apply_es_de_gamelist_publication(&projection.es_de_publication).unwrap();
        let published = std::fs::read_to_string(&gamelist).unwrap();
        assert!(published.contains("game.cue"));
        assert!(published.contains("<!-- keep -->"));
        assert_eq!(published.matches("<game>").count(), 1);
        for operation in &projection.playing_library_plan.operations {
            assert!(operation.destination_path.is_symlink());
        }
        assert_eq!(std::fs::read(source.join("game.cue")).unwrap(), b"game.cue");
        rollback_es_de_gamelist_publication(&projection.es_de_publication).unwrap();
        let mut applied = outcome.transaction;
        crate::dat::rename_apply::rollback::rollback_transaction(
            &mut applied,
            &fixture.path().join("journal"),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&gamelist).unwrap(), original);
        assert!(!projection.retrodeck_rom_root.join("game.cue").exists());
    }

    #[test]
    fn projected_collisions_and_existing_targets_fail_before_mutation() {
        use crate::dat::rename_apply::executor::{
            ApplyExecution, HardConflictMode, apply_transaction,
        };
        use crate::dat::rename_apply::journal::write_journal;
        use crate::dat::rename_apply::preflight::DirectoryPolicy;
        use crate::safe_read::TrustedRoots;
        use std::sync::atomic::AtomicBool;
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("game.cue"), b"cue").unwrap();
        let esde = profile(fixture.path());
        let mut duplicate = plan(&source, "game.cue", &[]);
        duplicate
            .elected_games
            .push(duplicate.elected_games[0].clone());
        assert!(
            build_retrodeck_projection(
                &duplicate,
                &identity("PSX"),
                destination.clone(),
                RetroDeckVisibility::verified_same_path_bind(source.clone(), destination.clone())
                    .unwrap(),
                &esde
            )
            .is_err()
        );
        let projection = build_retrodeck_projection(
            &plan(&source, "game.cue", &[]),
            &identity("PSX"),
            destination.clone(),
            RetroDeckVisibility::verified_same_path_bind(source.clone(), destination.clone())
                .unwrap(),
            &esde,
        )
        .unwrap();
        let target = &projection.playing_library_plan.operations[0].destination_path;
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, b"unrelated").unwrap();
        let mut transaction = build_retrodeck_projection_transaction(&projection, 1).unwrap();
        let journal = fixture.path().join("journal");
        write_journal(&journal, &transaction).unwrap();
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
                trusted: TrustedRoots::from_paths([source, destination]),
                journal_dir: journal,
                hard_conflict_mode: HardConflictMode::AbortAll,
                cancel: &AtomicBool::new(false),
                directory_policy: DirectoryPolicy::SameFilesystem,
                allow_symlink_source: false
            })
            .is_err()
        );
        assert_eq!(std::fs::read(target).unwrap(), b"unrelated");
    }

    #[test]
    fn replan_after_publication_is_unchanged_and_keeps_existing_gamelist_bytes() {
        use crate::launch::es_de_publish::apply_es_de_gamelist_publication;
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let destination = fixture.path().join("destination");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("game.cue"), b"cue").unwrap();
        let esde = profile(fixture.path());
        let gamelist = PathBuf::from(
            &esde
                .system_data
                .iter()
                .find(|entry| entry.system_name == "psx")
                .unwrap()
                .gamelist_file
                .path
                .display,
        );
        std::fs::create_dir_all(gamelist.parent().unwrap()).unwrap();
        let original = "<gameList>\n  <game><path>/keep</path></game>\n</gameList>\n";
        std::fs::write(&gamelist, original).unwrap();
        let first = build_retrodeck_projection(
            &plan(&source, "game.cue", &[]),
            &identity("PSX"),
            destination.clone(),
            RetroDeckVisibility::unverified(None, None, None, None),
            &esde,
        )
        .unwrap();
        apply_es_de_gamelist_publication(&first.es_de_publication).unwrap();
        let second = build_retrodeck_projection(
            &plan(&source, "game.cue", &[]),
            &identity("PSX"),
            destination,
            RetroDeckVisibility::unverified(None, None, None, None),
            &esde,
        )
        .unwrap();
        assert!(second.es_de_publication.is_unchanged());
        let updated = std::fs::read_to_string(gamelist).unwrap();
        assert!(updated.contains("/keep"));
        assert!(updated.ends_with("</gameList>\n"));
    }
}
