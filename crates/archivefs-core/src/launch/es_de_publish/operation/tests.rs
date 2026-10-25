use super::*;
use crate::dat::identity::{DatPlatformConfidence, DatPlatformIdentity};
use crate::dat::rename_apply::{
    executor::HardConflictMode, journal::write_journal, preflight::DirectoryPolicy,
};
use crate::emulator_environment::{
    HostReadOnlyFilesystem,
    es_de::{DiscoveryEnvironment, ExplicitRoot, discover_es_de_environment},
};
use crate::playing_library::{
    CandidateEvidenceSummary, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPlan, PlayingLibraryPolicy, RetroDeckProjectionPlan, RetroDeckVisibility,
    build_retrodeck_projection, build_retrodeck_projection_transaction,
};

const XML: &str = "<?xml version=\"1.0\"?>\r\n<gameList unusual=\"Ω\">\r\n<!-- preserve this -->\r\n<folder><path>./old</path><name>Original</name><custom a=\"1\"/></folder>\r\n</gameList>\r\n";
struct Fixture {
    _temp: tempfile::TempDir,
    source: PathBuf,
    journal: PathBuf,
    projection: RetroDeckProjectionPlan,
    originals: Vec<(PathBuf, Vec<u8>)>,
}
impl Fixture {
    fn new(
        platform: &str,
        system: &str,
        launcher: &str,
        companions: &[(&str, &str)],
        existing: bool,
    ) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let source = root.join("Source Ω with spaces");
        let dest = root.join("Retro Deck 雪");
        let home = root.join("ES-DE");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(home.join("custom_systems")).unwrap();
        fs::write(home.join("custom_systems/es_systems.xml"),format!("<systemList><system><name>{system}</name><fullname>Fixture</fullname><path>{}/roms/{system}</path><extension>.gb .gba .cue .bin .gdi .raw .m3u</extension><platform>{system}</platform><theme>{system}</theme></system></systemList>",dest.display())).unwrap();
        let profile = discover_es_de_environment(
            &HostReadOnlyFilesystem,
            &DiscoveryEnvironment {
                home: Some(root.into()),
                path: Some("".into()),
                explicit_root: Some(ExplicitRoot {
                    home_directory: home,
                    executable_path: None,
                }),
                explicit_bundled_systems_files: vec![],
                appimage_search_roots: vec![],
                explicit_appimages: vec![],
                explicit_portables: vec![],
            },
        )
        .unwrap()
        .profiles
        .into_iter()
        .find(|p| !p.system_data.is_empty())
        .unwrap();
        let gamelist = PathBuf::from(
            &profile
                .system_data
                .iter()
                .find(|s| s.system_name == system)
                .unwrap()
                .gamelist_file
                .path
                .display,
        );
        if existing {
            fs::create_dir_all(gamelist.parent().unwrap()).unwrap();
            fs::write(&gamelist, XML).unwrap();
        }
        let mut originals = vec![];
        for (name, bytes) in std::iter::once((
            launcher,
            match launcher.rsplit('.').next().unwrap() {
                "cue" => {
                    "FILE \"Track 01 Ω.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\n"
                }
                "gdi" => "2\n1 0 4 2352 \"Track 01 Ω.bin\" 0\n2 45000 0 2352 \"Track 02.raw\" 0\n",
                "m3u" => "Disc 1.cue\nDisc 2.cue\n",
                _ => "synthetic fixture",
            },
        ))
        .chain(companions.iter().copied())
        {
            let path = source.join(name);
            fs::write(&path, bytes).unwrap();
            originals.push((path, bytes.as_bytes().to_vec()));
        }
        let operation = |name: &str| LinkedLibraryOperation {
            source_path: source.join(name),
            destination_path: root.join("playing").join(name),
        };
        let game = ElectedGame {
            dat_entry_name: "Canonical Ω Release".into(),
            family_root_name: "Canonical".into(),
            explanation: ElectionExplanation {
                steps: vec![],
                rejected: vec![],
                winner_evidence: CandidateEvidenceSummary::unknown(),
            },
            launcher_operation: operation(launcher),
            companion_operations: companions.iter().map(|(n, _)| operation(n)).collect(),
        };
        let plan = PlayingLibraryPlan {
            destination_root: root.join("playing"),
            policy: PlayingLibraryPolicy::default(),
            archives_examined: 1,
            families_examined: 1,
            operations: game.all_operations().cloned().collect(),
            elected_games: vec![game],
            unresolved_groups: vec![],
            exclusions: vec![],
            singleton_families: 1,
            conflicts: vec![],
            rejected_launchers: vec![],
        };
        let identity = DatPlatformIdentity::Resolved {
            platform: platform.into(),
            machine_key: None,
            confidence: DatPlatformConfidence::Strong,
            evidence: vec![],
        };
        let projection = build_retrodeck_projection(
            &plan,
            &identity,
            dest.clone(),
            RetroDeckVisibility::verified_same_path_bind(source.clone(), dest).unwrap(),
            &profile,
        )
        .unwrap();
        let journal = root.join("journal");
        fs::create_dir(&journal).unwrap();
        Self {
            _temp: temp,
            source,
            journal,
            projection,
            originals,
        }
    }
    fn apply(&self) -> Result<OperationOutcome, String> {
        self.apply_with_cancel(false)
    }
    fn apply_with_cancel(&self, cancelled: bool) -> Result<OperationOutcome, String> {
        let mut tx = build_retrodeck_projection_transaction(&self.projection, 7)?;
        fs::create_dir_all(&self.projection.retrodeck_rom_root).unwrap();
        prepare(&self.journal, &mut tx).unwrap();
        apply(
            &self.projection.es_de_publication,
            &mut ApplyExecution {
                approved_paths: tx
                    .entries
                    .iter()
                    .map(|e| e.source_path.to_string_lossy().into_owned())
                    .collect(),
                transaction: &mut tx,
                current_generation: 7,
                trusted: TrustedRoots::from_paths([
                    self.source.clone(),
                    self.projection.retrodeck_rom_root.clone(),
                ]),
                journal_dir: self.journal.clone(),
                hard_conflict_mode: HardConflictMode::AbortAll,
                cancel: &AtomicBool::new(cancelled),
                directory_policy: DirectoryPolicy::SameFilesystem,
                allow_symlink_source: false,
            },
        )
    }
    fn unchanged(&self) {
        for (p, bytes) in &self.originals {
            assert_eq!(fs::read(p).unwrap(), *bytes);
        }
    }
    fn no_links(&self) {
        for op in &self.projection.playing_library_plan.operations {
            assert!(fs::symlink_metadata(&op.destination_path).is_err());
        }
    }
}
#[test]
fn retrodeck_direct_media_mappings_all_links_one_launcher_xml_preservation_and_exact_undo() {
    for (platform, system, launcher, companions) in [
        ("Game Boy", "gb", "Game Ω.gb", vec![]),
        ("Game Boy Advance", "gba", "Game Ω.gba", vec![]),
        (
            "PSX",
            "psx",
            "Game Ω.cue",
            vec![("Track 01 Ω.bin", "binary track")],
        ),
        (
            "Dreamcast",
            "dreamcast",
            "Game Ω.gdi",
            vec![
                ("Track 01 Ω.bin", "data track"),
                ("Track 02.raw", "audio track"),
            ],
        ),
        (
            "PSX",
            "psx",
            "Game Ω.m3u",
            vec![
                (
                    "Disc 1.cue",
                    "FILE \"Disc 1.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\n",
                ),
                ("Disc 1.bin", "disc one"),
                (
                    "Disc 2.cue",
                    "FILE \"Disc 2.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\n",
                ),
                ("Disc 2.bin", "disc two"),
            ],
        ),
    ] {
        let f = Fixture::new(platform, system, launcher, &companions, true);
        assert_eq!(f.projection.es_de_system, system);
        assert_eq!(f.projection.games.len(), 1);
        assert_eq!(f.projection.total_files, 1 + companions.len());
        assert_eq!(f.projection.es_de_publication.added.len(), 1);
        let out = f.apply().unwrap();
        assert_eq!(out.phase, Phase::Completed, "{:?}", out.detail);
        for op in &f.projection.playing_library_plan.operations {
            assert_eq!(fs::read_link(&op.destination_path).unwrap(), op.source_path);
        }
        let published = fs::read_to_string(&f.projection.es_de_publication.gamelist_path).unwrap();
        assert_eq!(published.matches("<game>").count(), 1);
        let start = published
            .find("    <game>")
            .unwrap_or_else(|| published.find("<game>").unwrap());
        let end = published[start..].find("</game>").unwrap() + start + "</game>".len();
        // Compare exact original prefix/suffix via the native insertion span.
        let close = XML.find("</gameList>").unwrap();
        assert!(published.starts_with(&XML[..close]));
        assert!(published.ends_with(&XML[close..]));
        assert!(end > start);
        let mut replanned = f.projection.playing_library_plan.clone();
        // Re-publication sees the exact existing launcher and adds no duplicates.
        let profile_path = f._temp.path().join("ES-DE");
        let profile = discover_es_de_environment(
            &HostReadOnlyFilesystem,
            &DiscoveryEnvironment {
                home: Some(f._temp.path().into()),
                path: Some("".into()),
                explicit_root: Some(ExplicitRoot {
                    home_directory: profile_path,
                    executable_path: None,
                }),
                explicit_bundled_systems_files: vec![],
                appimage_search_roots: vec![],
                explicit_appimages: vec![],
                explicit_portables: vec![],
            },
        )
        .unwrap()
        .profiles
        .into_iter()
        .find(|p| !p.system_data.is_empty())
        .unwrap();
        let again = plan_es_de_gamelist_publication(&replanned, platform, &profile).unwrap();
        assert!(again.is_unchanged());
        apply_es_de_gamelist_publication(&again).unwrap();
        assert_eq!(fs::read_to_string(&again.gamelist_path).unwrap(), published);
        let discovery = discover(&f.journal);
        assert!(discovery.1.is_empty());
        assert_eq!(discovery.0.len(), 1);
        // Generic child Undo cannot split this combined operation, even if markers are stripped.
        let mut child = out.transaction.clone();
        child.unknown.clear();
        assert!(
            crate::dat::rename_apply::rollback::rollback_transaction(
                &mut child,
                &f.journal,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        let restored = restore(
            &f.journal,
            &out.transaction.transaction_id,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(restored.phase, Phase::Restored);
        assert_eq!(fs::read(&again.gamelist_path).unwrap(), XML.as_bytes());
        f.no_links();
        f.unchanged();
        replanned.operations.clear();
    }
}
#[test]
fn retrodeck_direct_new_gamelist_removed_and_interrupted_recovery_available() {
    let f = Fixture::new("Game Boy Advance", "gba", "Game.gba", &[], false);
    OPERATION_FAULT.with(|fault| fault.set(Some("before_completion")));
    assert!(f.apply().is_err());
    OPERATION_FAULT.with(|fault| fault.set(None));
    let (ops, errors) = discover(&f.journal);
    assert!(errors.is_empty());
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].phase, Phase::MetadataPublishing);
    let out = restore(
        &f.journal,
        &ops[0].transaction.transaction_id,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(out.phase, Phase::Restored);
    assert!(!f.projection.es_de_publication.gamelist_path.exists());
    f.no_links();
    f.unchanged();
}
#[test]
fn retrodeck_direct_unverified_visibility_and_destination_collision_fail_without_mutation() {
    let mut f = Fixture::new("Game Boy", "gb", "Game.gb", &[], true);
    f.projection.visibility = RetroDeckVisibility::unverified(None, None, None, None);
    assert!(build_retrodeck_projection_transaction(&f.projection, 7).is_err());
    f.no_links();
    f.projection.visibility = RetroDeckVisibility::verified_same_path_bind(
        f.source.clone(),
        f.projection.destination_root.clone(),
    )
    .unwrap();
    let destination = &f.projection.games[0].launcher.destination_path;
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(destination, b"collision").unwrap();
    assert!(f.apply().is_err());
    assert_eq!(fs::read(destination).unwrap(), b"collision");
    assert_eq!(
        fs::read(&f.projection.es_de_publication.gamelist_path).unwrap(),
        XML.as_bytes()
    );
    f.unchanged();
}
#[test]
fn retrodeck_direct_link_failure_keeps_xml_and_esde_failure_removes_links() {
    let f = Fixture::new(
        "PSX",
        "psx",
        "Game.cue",
        &[("Track 01 Ω.bin", "binary")],
        true,
    );
    let out = f.apply_with_cancel(true).unwrap();
    assert_eq!(out.phase, Phase::Restored);
    f.no_links();
    assert_eq!(
        fs::read(&f.projection.es_de_publication.gamelist_path).unwrap(),
        XML.as_bytes()
    );
    f.unchanged();
    let f = Fixture::new(
        "PSX",
        "psx",
        "Game.cue",
        &[("Track 01 Ω.bin", "binary")],
        true,
    );
    super::super::PUBLICATION_FAULT.with(|fault| {
        fault.replace(Some("before_write"));
    });
    let out = f.apply().unwrap();
    super::super::PUBLICATION_FAULT.with(|fault| {
        fault.replace(None);
    });
    assert_eq!(out.phase, Phase::Restored, "{:?}", out.detail);
    f.no_links();
    assert_eq!(
        fs::read(&f.projection.es_de_publication.gamelist_path).unwrap(),
        XML.as_bytes()
    );
    f.unchanged();
}

#[test]
fn retrodeck_direct_filename_only_and_unknown_platforms_are_refused() {
    use crate::dat::identity::{
        DatPlatformEvidence, DatPlatformEvidenceKind, resolve_dat_platform_identity,
    };
    let f = Fixture::new("Game Boy", "gb", "Game.gb", &[], true);
    let weak = resolve_dat_platform_identity(vec![DatPlatformEvidence {
        platform: "Game Boy".into(),
        machine_key: None,
        kind: DatPlatformEvidenceKind::FilenameCorroboration,
        confidence: DatPlatformConfidence::Weak,
        detail: "Game Boy.dat".into(),
    }]);
    assert_eq!(weak, DatPlatformIdentity::Unknown);
    // Real temporary systems XML is reused, never an installed ES-DE profile.
    let home = f._temp.path().join("ES-DE");
    let profile = discover_es_de_environment(
        &HostReadOnlyFilesystem,
        &DiscoveryEnvironment {
            home: Some(f._temp.path().into()),
            path: Some("".into()),
            explicit_root: Some(ExplicitRoot {
                home_directory: home,
                executable_path: None,
            }),
            explicit_bundled_systems_files: vec![],
            appimage_search_roots: vec![],
            explicit_appimages: vec![],
            explicit_portables: vec![],
        },
    )
    .unwrap()
    .profiles
    .into_iter()
    .find(|p| !p.system_data.is_empty())
    .unwrap();
    for identity in [
        weak,
        DatPlatformIdentity::Resolved {
            platform: "Unknown Console".into(),
            machine_key: None,
            confidence: DatPlatformConfidence::Strong,
            evidence: vec![],
        },
    ] {
        assert!(
            build_retrodeck_projection(
                &f.projection.playing_library_plan,
                &identity,
                f.projection.destination_root.clone(),
                f.projection.visibility.clone(),
                &profile
            )
            .is_err()
        );
    }
    f.no_links();
    f.unchanged();
}

#[test]
fn retrodeck_direct_missing_or_corrupt_parent_never_allows_independent_child_undo() {
    let f = Fixture::new("Game Boy", "gb", "Game.gb", &[], true);
    let out = f.apply().unwrap();
    let id = &out.transaction.transaction_id;
    let (receipt, state, _) = paths(&f.journal, id).unwrap();
    let before = fs::read(&f.projection.es_de_publication.gamelist_path).unwrap();
    let mut child = out.transaction.clone();
    fs::remove_file(&receipt).unwrap();
    assert!(require_independent(&f.journal, &child).is_err());
    assert!(restore(&f.journal, id, &AtomicBool::new(false)).is_err());
    child.unknown.clear(); // parent state still establishes ownership
    assert!(require_independent(&f.journal, &child).is_err());
    fs::write(&receipt, b"corrupt parent").unwrap();
    fs::remove_file(&state).unwrap();
    assert!(require_independent(&f.journal, &child).is_err());
    assert!(!discover(&f.journal).1.is_empty());
    assert_eq!(
        fs::read(&f.projection.es_de_publication.gamelist_path).unwrap(),
        before
    );
    f.unchanged();
}

#[test]
fn retrodeck_direct_changed_metadata_or_link_blocks_undo_preserves_evidence() {
    let f = Fixture::new("Game Boy Advance", "gba", "Game.gba", &[], true);
    let out = f.apply().unwrap();
    let metadata = &f.projection.es_de_publication.gamelist_path;
    fs::write(metadata, b"<gameList><!-- external edit --></gameList>").unwrap();
    let result = restore(
        &f.journal,
        &out.transaction.transaction_id,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.phase, Phase::RecoveryRequired);
    assert!(f.projection.games[0].launcher.destination_path.is_symlink());
    assert_eq!(
        fs::read(metadata).unwrap(),
        b"<gameList><!-- external edit --></gameList>"
    );
    assert_eq!(discover(&f.journal).0.len(), 1);
    f.unchanged();
}

#[cfg(unix)]
#[test]
fn retrodeck_direct_real_link_io_refusal_leaves_gamelist_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("Game Boy Advance", "gba", "Game.gba", &[], true);
    let output_root = f.projection.retrodeck_rom_root.clone();
    let hook_root = output_root.clone();
    OPERATION_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |stage, _| {
            if stage == "before_links" {
                fs::set_permissions(&hook_root, fs::Permissions::from_mode(0o555)).unwrap();
            }
        }))
    });
    let result = f.apply();
    OPERATION_HOOK.with(|hook| *hook.borrow_mut() = None);
    fs::set_permissions(&output_root, fs::Permissions::from_mode(0o755)).unwrap();
    let out = result.unwrap();
    assert_eq!(out.phase, Phase::Restored, "{:?}", out.detail);
    assert!(
        out.transaction
            .entries
            .iter()
            .all(|e| e.applied_at_unix.is_none())
    );
    f.no_links();
    f.unchanged();
    assert_eq!(
        fs::read(&f.projection.es_de_publication.gamelist_path).unwrap(),
        XML.as_bytes()
    );
}

#[test]
fn retrodeck_direct_prepared_intent_blocks_independent_recovery_before_parent_creation() {
    let f = Fixture::new("Game Boy", "gb", "Game.gb", &[], true);
    let mut child = build_retrodeck_projection_transaction(&f.projection, 7).unwrap();
    prepare(&f.journal, &mut child).unwrap();
    assert!(require_independent(&f.journal, &child).is_err());
    assert!(
        crate::dat::rename_apply::reconcile::reconcile_recovery(&mut child, &f.journal).is_err()
    );
    assert!(
        crate::dat::rename_apply::rollback::rollback_transaction(
            &mut child,
            &f.journal,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(prepare(&f.journal, &mut child).is_err());
    f.no_links();
    f.unchanged();
    assert_eq!(
        fs::read(&f.projection.es_de_publication.gamelist_path).unwrap(),
        XML.as_bytes()
    );
}
#[test]
fn retrodeck_direct_contradictory_restored_child_never_claims_restore_success() {
    let f = Fixture::new("Game Boy", "gb", "Game.gb", &[], true);
    let out = f.apply().unwrap();
    let id = out.transaction.transaction_id.clone();
    let (receipt, _, _) = load(&f.journal, &id).unwrap();
    let mut child = out.transaction;
    child.state = TransactionState::RolledBack;
    write_journal(&f.journal, &child).unwrap();
    checkpoint(&f.journal, &receipt, Phase::Restored, None).unwrap();
    assert_eq!(
        inspect(&f.journal, &child.transaction_id).unwrap().phase,
        Phase::RecoveryRequired
    );
    assert!(f.projection.games[0].launcher.destination_path.is_symlink());
    f.unchanged();
}
