//! Loose PlayStation CUE/BIN discs through the user's own steps, on synthetic
//! files and a private database: scan, add the DAT, Save, Validate, Audit,
//! reload, Game Details, Preview. Then one track changes.
use super::*;
use crate::multidisc_launch_view::tests::duckstation_candidate;
use crate::multidisc_launch_view::{MultiDiscLaunchContext, guidance, sets_from};
use archivefs_core::launch::MultiDiscLaunch;

const SECTOR: usize = 2352;

/// (entry, track-1 fill, sheet SHA-1, [track SHA-1s]) - SHA-1s precomputed
/// for the exact bytes `write_disc` produces.
const DISCS: [(&str, u8, &str, [&str; 2]); 2] = [
    (
        "Orbit Quest (USA) (Disc 1)",
        1,
        "c9f9bbfe0481613b94595770ac37156afdfcaba5",
        [
            "84250d15d6ba6ffd3fb3b50334633a97e4d3e9ba",
            "9120008bc9bd4f2fd95e6b660c25da0354165380",
        ],
    ),
    (
        "Orbit Quest (USA) (Disc 2)",
        2,
        "438ceaa428a131269dc094a4a88620b1233c1bc5",
        [
            "a8a7bc23a62c76c6e6f057cd42840e11bc9688a8",
            "c3627ed06f45e86bfdd55c8fa4022d3f244746e3",
        ],
    ),
];

fn sheet(entry: &str) -> String {
    format!(
        "FILE \"{entry} (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\nFILE \"{entry} (Track 2).bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:00:10\n"
    )
}

fn tracks(entry: &str, fill: u8) -> [(String, Vec<u8>); 2] {
    [
        (format!("{entry} (Track 1).bin"), vec![fill; SECTOR * 20]),
        (
            format!("{entry} (Track 2).bin"),
            vec![fill.wrapping_add(100); SECTOR * 30],
        ),
    ]
}

fn dat() -> String {
    let games: String = DISCS
        .iter()
        .map(|(entry, fill, cue_sha1, track_sha1)| {
            let mut roms = format!(
                r#"<rom name="{entry}.cue" size="{}" sha1="{cue_sha1}"/>"#,
                sheet(entry).len()
            );
            for ((name, bytes), sha1) in tracks(entry, *fill).iter().zip(track_sha1) {
                roms.push_str(&format!(
                    r#"<rom name="{name}" size="{}" sha1="{sha1}"/>"#,
                    bytes.len()
                ));
            }
            format!(r#"<game name="{entry}">{roms}</game>"#)
        })
        .collect();
    format!(
        r#"<?xml version="1.0"?><datafile><header><name>Sony - PlayStation</name><version>2026-10-01</version><author>redump.org</author></header>{games}</datafile>"#
    )
}

/// Two catalogue-verified CUE/BIN discs after the user's own steps.
fn audited() -> (Fixture, PathBuf, PathBuf) {
    audited_from(false)
}

fn audited_from(managed: bool) -> (Fixture, PathBuf, PathBuf) {
    let fixture = Fixture::new();
    let games = fixture.dir("games/psx");
    // Local sheet names deliberately disagree with the catalogue's order.
    for ((entry, fill, ..), local) in DISCS.iter().zip(["second", "first"]) {
        std::fs::write(games.join(format!("{local}.cue")), sheet(entry)).unwrap();
        for (name, bytes) in tracks(entry, *fill) {
            std::fs::write(games.join(name), bytes).unwrap();
        }
    }
    let database_path = fixture.root.join("catalogue.sqlite3");
    let mut database = archivefs_core::Database::open_or_create(&database_path).unwrap();
    archivefs_core::scan_and_persist(
        &mut database,
        &archivefs_core::Config {
            source_folders: vec![fixture.root.join("games")],
            mount_root: fixture.root.join("mount"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        },
        "test",
    )
    .unwrap();
    // One entry per disc; no track is an entry.
    let mut rows: Vec<_> = database
        .load_archives()
        .unwrap()
        .into_iter()
        .map(|a| a.relative_path)
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        [
            PathBuf::from("psx/first.cue"),
            PathBuf::from("psx/second.cue")
        ]
    );
    drop(database);

    let dat_path = fixture.write("redump.dat", &dat());
    let mut page = fixture.page_with_library(vec![fixture.root.join("games")]);
    page.database_path = Some(database_path.clone());
    if managed {
        let (id, sha) = install_managed(&fixture, &dat());
        page.managed_sources = load_managed_dat_sources_from(&page.managed_config_path).unwrap();
        page.apply(DatSourcesPageAction::VerifyCatalogue {
            reference: CatalogueRef::managed_current(id, sha),
            scan_root: fixture.root.join("games"),
        });
        run_to_completion(&mut page);
        assert!(page.audit_error.is_none(), "{:?}", page.audit_error);
    } else {
        page.apply(DatSourcesPageAction::AddFile { path: dat_path });
        page.apply(DatSourcesPageAction::Save);
        page.apply(DatSourcesPageAction::Validate {
            id: "redump".into(),
        });
        run_to_completion(&mut page);
        page.apply(DatSourcesPageAction::Audit {
            id: "redump".into(),
            scan_root: fixture.root.join("games"),
        });
        run_to_completion(&mut page);
    }

    (fixture, games, database_path)
}

fn load(fixture: &Fixture, database_path: &Path) -> crate::database_load::CachedLibrarySnapshot {
    let reopened = archivefs_core::Database::open_read_only(database_path).unwrap();
    crate::database_load::load_snapshot_with_catalogues(
        &reopened,
        database_path,
        &fixture.config_path,
        &catalogues(fixture),
    )
    .unwrap_or_else(|_| panic!("library snapshot"))
}

#[test]
fn verified_cue_bin_discs_reach_game_details_and_preview_and_a_changed_track_stops_them() {
    for managed in [false, true] {
        launch_and_changed_track(managed);
    }
}

fn launch_and_changed_track(managed: bool) {
    let (fixture, games, database_path) = audited_from(managed);
    let snapshot = || load(&fixture, &database_path);
    let first = games.join("first.cue");
    let second = games.join("second.cue");
    let loaded = snapshot();
    let sets = sets_from(&loaded.media_topology, &loaded.archives);
    let context = MultiDiscLaunchContext::for_content(&sets, &first, &loaded.archives)
        .expect("Game Details finds the verified set")
        .with_disc_members(&loaded.disc_members);
    let decision = context.decide(&duckstation_candidate(&first), None, false);
    let shown = guidance(context.sets.first().unwrap(), &first, Some(&decision));
    assert!(shown.order_verified, "{shown:?}");
    assert_eq!(shown.headline, "2-disc game · order verified");
    let order: Vec<_> = shown
        .discs
        .iter()
        .map(|d| d.path.clone().unwrap())
        .collect();
    assert_eq!(order, [second.clone(), first.clone()]);
    let MultiDiscLaunch::Playlist(playlist) = &decision else {
        panic!("{decision:?}")
    };
    assert_eq!(playlist.discs, [second.clone(), first.clone()]);

    // One track of disc 1 changes, same size: the saved order is no longer
    // used, and Preview no longer plans a playlist.
    let track = games.join("Orbit Quest (USA) (Disc 1) (Track 2).bin");
    let mut bytes = std::fs::read(&track).unwrap();
    bytes[0] ^= 0xff;
    std::fs::write(&track, bytes).unwrap();

    // Before any reload, with the snapshot that still says "verified":
    // Launch refuses the playlist Preview planned, and a fresh Preview no
    // longer plans one.
    let (handoff_sets, planned) = context.handoff(&decision).unwrap();
    let mut arguments = vec![
        std::ffi::OsString::from("-batch"),
        std::ffi::OsString::from("--"),
        first.clone().into_os_string(),
    ];
    let error = archivefs_core::launch::multidisc_launch::hand_over_playlist(
        &mut arguments,
        &first,
        &handoff_sets,
        &planned,
    )
    .unwrap_err();
    assert!(error.contains("changed"), "{error}");
    assert!(!planned.playlist_path.exists());
    match context.decide(&duckstation_candidate(&first), None, false) {
        MultiDiscLaunch::SingleDisc { reason } => {
            assert!(
                reason.contains("changed since the disc was verified"),
                "{reason}"
            )
        }
        other => panic!("{other:?}"),
    }
    let loaded = snapshot();
    assert!(
        loaded.media_topology.is_empty(),
        "{:?}",
        loaded.media_topology
    );
    let sets = sets_from(&loaded.media_topology, &loaded.archives);
    let still_playlist = MultiDiscLaunchContext::for_content(&sets, &first, &loaded.archives)
        .map(|context| context.decide(&duckstation_candidate(&first), None, false))
        .is_some_and(|decision| matches!(decision, MultiDiscLaunch::Playlist(_)));
    assert!(!still_playlist);
}

/// What a remount looks like to EmuWiz: the same bytes and modification time
/// under a new inode.
fn simulate_remount(path: &Path) {
    let modified = std::fs::metadata(path).unwrap().modified().unwrap();
    let staged = path.with_extension("remount");
    std::fs::copy(path, &staged).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&staged)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(&staged, path).unwrap();
}

#[test]
fn a_remounted_disc_asks_for_a_recheck_and_the_explicit_recheck_restores_it() {
    for managed in [false, true] {
        remounted_disc(managed);
    }
}

fn remounted_disc(managed: bool) {
    let (fixture, games, database_path) = audited_from(managed);
    let first = games.join("first.cue");
    simulate_remount(&first);
    for (name, _) in tracks(DISCS[1].0, DISCS[1].1) {
        simulate_remount(&games.join(name));
    }
    let loaded = load(&fixture, &database_path);
    let recheck = loaded
        .disc_recheck
        .get(&first)
        .expect("the remounted disc asks for a re-check")
        .clone();
    assert!(recheck.reason.contains("remounted"), "{}", recheck.reason);
    assert!(loaded.media_topology.is_empty(), "no order while uncertain");

    let message = crate::multidisc_launch_view::run_disc_recheck_with_catalogues(
        &database_path,
        &catalogues(&fixture),
        &recheck,
        &std::sync::atomic::AtomicBool::new(false),
    );
    assert!(message.starts_with("Re-checked: every file"), "{message}");

    let loaded = load(&fixture, &database_path);
    assert!(loaded.disc_recheck.is_empty());
    let sets = sets_from(&loaded.media_topology, &loaded.archives);
    let context = MultiDiscLaunchContext::for_content(&sets, &first, &loaded.archives)
        .unwrap()
        .with_disc_members(&loaded.disc_members);
    assert!(matches!(
        context.decide(&duckstation_candidate(&first), None, false),
        MultiDiscLaunch::Playlist(_)
    ));
}

fn catalogues(
    fixture: &Fixture,
) -> std::collections::HashMap<
    String,
    archivefs_core::dat::configured_catalogues::ConfiguredCatalogue,
> {
    let config = load_dat_sources_config_from(&fixture.config_path).unwrap_or_default();
    let (local, problems) = DatSourceRegistry::from_config(&config);
    assert!(problems.is_empty());
    let managed = load_managed_dat_sources_from(
        fixture
            .config_path
            .with_file_name("managed_dat_sources.toml"),
    )
    .unwrap();
    archivefs_core::dat::configured_catalogues::configured_catalogues(
        archivefs_core::dat::catalogue_selection::CatalogueInventoryInputs {
            local_registry: &local,
            managed_sources: &managed,
            managed_root: &fixture.config_path.with_file_name("managed-dats"),
            limits: DatLimits::default(),
        },
    )
}

fn install_managed(fixture: &Fixture, body: &str) -> (ManagedDatSourceId, String) {
    use sha2::{Digest, Sha256};
    let sha: String = Sha256::digest(body.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let descriptor = archivefs_core::dat::updates::ManagedDatSourceDescriptor::redump_games(
        RedumpGameSystem::PlayStation,
    )
    .unwrap()
    .with_update_policy(archivefs_core::dat::updates::ManagedDatUpdatePolicy::Manual);
    let state = ManagedDatState::new(&descriptor, ManagedDatSnapshot::new(&sha).unwrap()).unwrap();
    let root = fixture.config_path.with_file_name("managed-dats");
    let object = root
        .join(state.source_id.storage_relative_path())
        .join("objects")
        .join(&sha);
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, body).unwrap();
    save_managed_dat_state(&root, &state).unwrap();
    let mut managed = archivefs_core::dat::managed_sources::ManagedDatSources::new();
    managed
        .add_redump_games(
            RedumpGameSystem::PlayStation,
            archivefs_core::dat::updates::ManagedDatUpdatePolicy::Manual,
        )
        .unwrap();
    archivefs_core::dat::managed_sources::save_managed_dat_sources_to(
        fixture
            .config_path
            .with_file_name("managed_dat_sources.toml"),
        &managed,
    )
    .unwrap();
    (descriptor.source_id().clone(), sha)
}

#[test]
fn current_source_identity_and_inventory_survive_validation_for_both_stores() {
    for managed in [false, true] {
        let (fixture, games, database_path) = audited_from(managed);
        let rows = catalogues(&fixture);
        let source = rows.values().next().unwrap();
        let database = archivefs_core::Database::open_read_only(&database_path).unwrap();
        assert_eq!(
            database
                .expected_dat_entry_count(&source.source_id)
                .unwrap(),
            2
        );
        assert_eq!(
            database
                .expected_dat_inventory_sha256(&source.source_id)
                .unwrap(),
            source.sha256
        );
        let loaded = load(&fixture, &database_path);
        for archive in &loaded.archives {
            let summary = &loaded.dat_identities[&archive.id][0];
            assert!(summary.is_verified());
            assert_eq!(
                summary.provenance_freshness,
                archivefs_core::dat::library_identity_summary::DatProvenanceFreshness::Current
            );
            assert_eq!(summary.source.catalogue_sha256, source.sha256);
            assert_eq!(
                summary.source.source_revision.as_deref(),
                Some("2026-10-01")
            );
            assert_eq!(summary.source.author.as_deref(), Some("redump.org"));
        }
        let evidence = database.current_library_dat_evidence(&rows).unwrap();
        assert_eq!(evidence.exact.len(), 2);
        // Each exact match carries the audited hashes it was made from.
        for archive_id in evidence.exact.keys() {
            assert!(
                evidence.exact_hashes[archive_id].sha1.is_some()
                    || evidence.exact_hashes[archive_id].crc32.is_some()
            );
        }
        drop(database);
        if !managed {
            let mut page = fixture.page_with_library(vec![fixture.root.join("games")]);
            page.database_path = Some(database_path.clone());
            page.apply(DatSourcesPageAction::Validate {
                id: "redump".into(),
            });
            run_to_completion(&mut page);
            let loaded = load(&fixture, &database_path);
            assert_eq!(
                loaded.media_topology.len(),
                1,
                "validating identical bytes must keep order"
            );
        }
        assert!(
            load(&fixture, &database_path)
                .disc_members
                .contains_key(&games.join("first.cue"))
        );
    }
}

#[test]
fn disabled_removed_and_changed_catalogues_withhold_verified_order_and_recheck() {
    for managed in [false, true] {
        for change in [
            "disabled",
            "removed",
            "version",
            "same-version",
            "missing-object",
            "corrupt-object",
        ] {
            let (fixture, games, database_path) = audited_from(managed);
            let old = catalogues(&fixture).into_values().next().unwrap();
            let first = games.join("first.cue");
            simulate_remount(&first);
            let recheck = load(&fixture, &database_path).disc_recheck[&first].clone();
            let body = match change {
                "version" => dat().replace("2026-10-01", "2026-10-02"),
                "same-version" => {
                    dat().replace("</datafile>", "<!-- changed snapshot --></datafile>")
                }
                _ => dat(),
            };
            if managed {
                let config = fixture
                    .config_path
                    .with_file_name("managed_dat_sources.toml");
                match change {
                    "disabled" => {
                        let mut sources =
                            archivefs_core::dat::managed_sources::ManagedDatSources::new();
                        sources
                            .add_redump_games(
                                RedumpGameSystem::PlayStation,
                                archivefs_core::dat::updates::ManagedDatUpdatePolicy::Disabled,
                            )
                            .unwrap();
                        archivefs_core::dat::managed_sources::save_managed_dat_sources_to(
                            &config, &sources,
                        )
                        .unwrap();
                    }
                    "removed" => {
                        std::fs::remove_file(config).unwrap();
                    }
                    "missing-object" => {
                        std::fs::remove_file(&old.path).unwrap();
                    }
                    "corrupt-object" => {
                        std::fs::write(&old.path, "invalid DAT bytes").unwrap();
                    }
                    _ => {
                        install_managed(&fixture, &body);
                    }
                }
            } else {
                match change {
                    "disabled" => {
                        let mut config =
                            load_dat_sources_config_from(&fixture.config_path).unwrap();
                        config.sources.as_mut().unwrap()[0].enabled = Some(false);
                        archivefs_core::dat::sources::save_dat_sources_config_to(
                            &fixture.config_path,
                            &config,
                        )
                        .unwrap();
                    }
                    "removed" => {
                        std::fs::remove_file(&fixture.config_path).unwrap();
                    }
                    "missing-object" => {
                        std::fs::remove_file(&old.path).unwrap();
                    }
                    "corrupt-object" => {
                        std::fs::write(&old.path, "invalid DAT bytes").unwrap();
                    }
                    _ => {
                        std::fs::write(&old.path, body).unwrap();
                    }
                }
            }
            let loaded = load(&fixture, &database_path);
            assert!(loaded.media_topology.is_empty(), "{managed} {change}");
            let rows = catalogues(&fixture);
            let database = archivefs_core::Database::open_read_only(&database_path).unwrap();
            assert!(
                database
                    .current_library_dat_evidence(&rows)
                    .unwrap()
                    .exact
                    .is_empty(),
                "{managed} {change}"
            );
            let message = crate::multidisc_launch_view::run_disc_recheck_with_catalogues(
                &database_path,
                &rows,
                &recheck,
                &AtomicBool::new(false),
            );
            assert!(
                !message.starts_with("Re-checked: every file"),
                "{managed} {change}: {message}"
            );
        }
    }
}

#[test]
fn reauditing_changed_bytes_does_not_reuse_old_topology_or_cached_health() {
    for (managed, same_version) in [(false, false), (true, false), (false, true), (true, true)] {
        let (fixture, _, path) = audited_from(managed);
        let old = load(&fixture, &path).media_topology;
        let rows = catalogues(&fixture);
        let source = rows.values().next().unwrap();
        let body = if same_version {
            dat().replace("</datafile>", "<!-- new bytes --></datafile>")
        } else {
            dat().replace("2026-10-01", "2026-10-02")
        };
        if !managed {
            // Historical arcade health may survive replacing a source's DAT.
            // The current file digest, not that old health label, is authority.
            let mut config = load_dat_sources_config_from(&fixture.config_path).unwrap();
            config.sources.as_mut().unwrap()[0].health_arcade_catalogue_revisions =
                Some(vec!["fbneo=old-validation".into()]);
            archivefs_core::dat::sources::save_dat_sources_config_to(&fixture.config_path, &config)
                .unwrap();
        }
        if managed {
            install_managed(&fixture, &body);
        } else {
            std::fs::write(&source.path, body).unwrap();
        }
        let rows = catalogues(&fixture);
        let source = rows.values().next().unwrap();
        let audit = archivefs_core::dat::sources::audit_run::run_dat_audit_with_cache(
            &archivefs_core::dat::sources::audit_run::DatAuditRequest {
                source_id: source.source_id.clone(),
                source_display_name: source.display_name.clone(),
                dat_path: source.path.clone(),
                dat_kind: source.kind,
                scan_root: fixture.root.join("games"),
                limits: DatLimits::default(),
                policy: None,
                platform: None,
            },
            &TrustedRoots::none(),
            &AtomicBool::new(false),
            &|_| {},
            archivefs_core::dat::sources::audit_cache::AuditCacheConfig::Disabled,
        )
        .unwrap();
        let mut database = archivefs_core::Database::open_or_create(&path).unwrap();
        database
            .persist_library_dat_identities_from_audit(&audit)
            .unwrap();
        assert!(
            load(&fixture, &path).media_topology.is_empty(),
            "old order must not acquire new catalogue evidence"
        );
        database.refresh_dat_disc_order(2).unwrap();
        let new = load(&fixture, &path).media_topology;
        assert_eq!(new.len(), 1);
        assert_ne!(old[0].identity.key, new[0].identity.key);
    }
}

#[test]
fn conflicting_managed_and_local_entry_names_never_supply_playlist_order() {
    let (fixture, games, path) = audited();
    let body = dat()
        .replace(
            "<game name=\"Orbit Quest (USA)",
            "<game name=\"Other Quest (USA)",
        )
        .replace("2026-10-01", "2026-10-02");
    let (id, sha) = install_managed(&fixture, &body);
    let mut page = fixture.page_with_library(vec![fixture.root.join("games")]);
    page.database_path = Some(path.clone());
    page.apply(DatSourcesPageAction::VerifyCatalogue {
        reference: CatalogueRef::managed_current(id, sha),
        scan_root: fixture.root.join("games"),
    });
    run_to_completion(&mut page);
    assert!(page.audit_error.is_none(), "{:?}", page.audit_error);
    let loaded = load(&fixture, &path);
    assert!(
        loaded
            .dat_identities
            .values()
            .all(|summaries| summaries.len() == 2)
    );
    assert!(loaded.media_topology.is_empty());
    let sets = sets_from(&loaded.media_topology, &loaded.archives);
    assert!(
        !MultiDiscLaunchContext::for_content(&sets, &games.join("first.cue"), &loaded.archives)
            .is_some_and(|context| matches!(
                context.with_disc_members(&loaded.disc_members).decide(
                    &duckstation_candidate(&games.join("first.cue")),
                    None,
                    false
                ),
                MultiDiscLaunch::Playlist(_)
            ))
    );
}

#[test]
fn stale_marks_and_legacy_missing_digests_do_not_acquire_current_evidence() {
    for managed in [false, true] {
        for legacy in [false, true] {
            let (fixture, _, path) = audited_from(managed);
            let rows = catalogues(&fixture);
            let source = rows.values().next().unwrap();
            let mut database = archivefs_core::Database::open_or_create(&path).unwrap();
            if legacy {
                for archive in database.load_archives().unwrap() {
                    let mut identity = database
                        .library_dat_identity_for_item(archive.id, &source.source_id)
                        .unwrap()
                        .unwrap();
                    identity.source.catalogue_sha256 = None;
                    database
                        .persist_library_dat_identity(archive.id, &identity)
                        .unwrap();
                }
            } else {
                database
                    .mark_library_dat_identities_stale_for_source(&source.source_id)
                    .unwrap();
            }
            assert!(
                database
                    .current_library_dat_evidence(&rows)
                    .unwrap()
                    .exact
                    .is_empty()
            );
            assert!(load(&fixture, &path).media_topology.is_empty());
        }
    }
}

#[test]
fn local_ids_cannot_impersonate_managed_evidence_after_removal() {
    let (fixture, _, path) = audited_from(true);
    let source = catalogues(&fixture).into_values().next().unwrap();
    std::fs::remove_file(
        fixture
            .config_path
            .with_file_name("managed_dat_sources.toml"),
    )
    .unwrap();
    let entry = archivefs_core::dat::sources::DatSourceEntry::new(
        source.source_id.clone(),
        "local collision".into(),
        source.path,
        source.kind,
    );
    let mut registry = DatSourceRegistry::new();
    assert!(
        registry.add(entry).is_err(),
        "managed audit IDs are outside the local registry ID grammar"
    );
    archivefs_core::dat::sources::save_dat_sources_config_to(
        &fixture.config_path,
        &registry.to_config(),
    )
    .unwrap();
    assert!(catalogues(&fixture).is_empty());
    assert!(load(&fixture, &path).media_topology.is_empty());
}

#[test]
fn production_config_paths_support_local_and_managed_disc_rechecks() {
    const CHILD: &str = "EMUWIZ_MANAGED_DAT_DEFAULT_PATH_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "dat_sources_page::tests::cue_bin_order::production_config_paths_support_local_and_managed_disc_rechecks", "--nocapture"])
            .env(CHILD, "1")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // The repository constructor installs a new private HOME before main in
    // this child. No parallel test shares its default application directories.
    assert!(std::env::var("HOME").unwrap().contains("emuwiz-test-env/"));
    for managed in [false, true] {
        let (fixture, games, path) = audited_from(managed);
        if managed {
            let sources = load_managed_dat_sources_from(
                fixture
                    .config_path
                    .with_file_name("managed_dat_sources.toml"),
            )
            .unwrap();
            let descriptor = &sources.descriptors().unwrap()[0];
            let state = load_managed_dat_state(
                &fixture.config_path.with_file_name("managed-dats"),
                descriptor,
            )
            .unwrap();
            let root = archivefs_core::dat::updates::managed_dat_root().unwrap();
            let object = root
                .join(state.source_id.storage_relative_path())
                .join("objects")
                .join(&state.sha256);
            std::fs::create_dir_all(object.parent().unwrap()).unwrap();
            std::fs::write(object, dat()).unwrap();
            save_managed_dat_state(&root, &state).unwrap();
            archivefs_core::dat::managed_sources::save_managed_dat_sources_to(
                archivefs_core::dat::managed_sources::default_managed_dat_sources_config_path()
                    .unwrap(),
                &sources,
            )
            .unwrap();
        }
        let snapshot = || {
            let db = archivefs_core::Database::open_read_only(&path).unwrap();
            // Production supplies main config.toml here, rather than DAT Sources.
            crate::database_load::load_snapshot_from(
                &db,
                &path,
                &fixture.config_path.with_file_name("config.toml"),
            )
            .unwrap_or_else(|_| panic!("production snapshot"))
        };
        let loaded = snapshot();
        assert_eq!(loaded.media_topology.len(), 1);
        assert!(loaded.dat_identities.values().flatten().all(|summary| summary.is_verified()
            && summary.provenance_freshness == archivefs_core::dat::library_identity_summary::DatProvenanceFreshness::Current));
        let first = games.join("first.cue");
        simulate_remount(&first);
        let before = snapshot();
        assert!(before.media_topology.is_empty());
        let recheck = &before.disc_recheck[&first];
        let message = crate::multidisc_launch_view::run_disc_recheck(
            &path,
            &fixture.config_path,
            recheck,
            &AtomicBool::new(false),
        );
        assert!(message.starts_with("Re-checked: every file"), "{message}");
        let after = snapshot();
        assert_eq!(after.media_topology.len(), 1);
        assert!(after.disc_recheck.is_empty());
    }
}
