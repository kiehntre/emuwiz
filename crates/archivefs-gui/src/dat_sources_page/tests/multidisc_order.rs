//! The user's path with synthetic media and a private database: add a DAT,
//! Validate (saves the catalogue inventory), Audit (saves identities, then
//! disc order), reload the library, open Game Details, Preview. No real ROM,
//! DAT or library is touched.
use super::*;
use crate::multidisc_launch_view::tests::duckstation_candidate;
use crate::multidisc_launch_view::{
    MultiDiscLaunchContext, guidance, preview_arguments, sets_from,
};
use archivefs_core::launch::MultiDiscLaunch;
use std::ffi::OsString;

/// Catalogue entries and the SHA-1 of `"bytes of {entry}"`, the content
/// each synthetic disc holds.
const ENTRIES: [(&str, &str); 4] = [
    (
        "Orbit Quest (USA) (Disc 1)",
        "cfc49c66d460b7ac7117d6f76946003fac83653b",
    ),
    (
        "Orbit Quest (USA) (Disc 2)",
        "9e0ba03edd349d101345cd5b9c67b609f5037204",
    ),
    (
        "Orbit Quest (USA) (Disc 3)",
        "67e5904f0819958eb341b95b709d821eb3b839cd",
    ),
    (
        "Solo Game (USA)",
        "65fb3a7639c9ee23e743ccaebb0e569da37748e4",
    ),
];

#[test]
fn verified_dat_order_reaches_game_details_and_preview_without_database_edits() {
    let fixture = Fixture::new();
    let games = fixture.dir("games/psx");
    // Local names deliberately disagree with the catalogue's order.
    let files = [
        ("orbit (Disc 3).iso", ENTRIES[0].0),
        ("orbit (Disc 1).iso", ENTRIES[1].0),
        ("orbit (Disc 2).iso", ENTRIES[2].0),
        ("solo.iso", ENTRIES[3].0),
    ];
    for (file, entry) in files {
        std::fs::write(games.join(file), format!("bytes of {entry}")).unwrap();
    }
    let body: String = ENTRIES
        .iter()
        .map(|(entry, sha1)| {
            format!(
                r#"<game name="{entry}"><rom name="{entry}.bin" size="{}" sha1="{sha1}"/></game>"#,
                format!("bytes of {entry}").len()
            )
        })
        .collect();
    let dat = fixture.write(
        "redump.dat",
        &format!(
            r#"<?xml version="1.0"?><datafile><header><name>Sony - PlayStation</name><version>2026-10-01</version><author>redump.org</author></header>{body}</datafile>"#
        ),
    );
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
    drop(database);

    let mut page = fixture.page_with_library(vec![fixture.root.join("games")]);
    page.database_path = Some(database_path.clone());
    page.apply(DatSourcesPageAction::AddFile { path: dat });
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
    let saved = page
        .view()
        .save_results
        .into_iter()
        .find(|r| r.operation == DatSaveOperation::Audit)
        .unwrap();
    assert!(
        saved
            .technical_details
            .iter()
            .any(|d| d.contains("3 disc/disk/tape image(s) numbered")
                && d.contains("1 game(s) saved")),
        "{:?}",
        saved.technical_details
    );

    // Reload the library exactly as the app does.
    let reopened = archivefs_core::Database::open_read_only(&database_path).unwrap();
    let snapshot =
        crate::database_load::load_snapshot_from(&reopened, &database_path, &fixture.config_path)
            .unwrap_or_else(|_| panic!("library snapshot"));
    let sets = sets_from(
        &snapshot.media_topology,
        &snapshot.archives,
        &snapshot.dat_identities,
    );
    let disc = |name: &str| games.join(name);
    let context =
        MultiDiscLaunchContext::for_content(&sets, &disc("orbit (Disc 1).iso"), &snapshot.archives)
            .expect("Game Details finds the verified set");

    // Game Details: catalogue order, this entry, start disc.
    let candidate = duckstation_candidate(&disc("orbit (Disc 1).iso"));
    let decision = context.decide(&candidate, None, false);
    let shown = guidance(
        context.sets.first().unwrap(),
        &disc("orbit (Disc 1).iso"),
        Some(&decision),
    );
    assert!(shown.order_verified, "{shown:?}");
    assert_eq!(shown.headline, "3-disc game · order verified");
    let paths = shown
        .discs
        .iter()
        .map(|d| d.path.clone().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            disc("orbit (Disc 3).iso"),
            disc("orbit (Disc 1).iso"),
            disc("orbit (Disc 2).iso")
        ]
    );
    assert!(shown.discs[0].start && shown.discs[1].this_entry);
    assert_eq!(shown.next_discs, ["Disc 3"]);
    assert!(shown.launch.unwrap().contains("playlist of all 3 discs"));

    // Preview shows the exact argument Launch will pass.
    let MultiDiscLaunch::Playlist(playlist) = &decision else {
        panic!("{decision:?}")
    };
    let mut arguments = vec![
        OsString::from("-batch"),
        OsString::from("--"),
        disc("orbit (Disc 1).iso").into(),
    ];
    preview_arguments(&mut arguments, Some(&disc("orbit (Disc 1).iso")), &decision);
    assert_eq!(arguments[2], playlist.playlist_path.as_os_str());
    let (handoff_sets, handoff) = context.handoff(&decision).unwrap();
    assert_eq!(&handoff, playlist);
    assert_eq!(handoff_sets.len(), 1);

    // A single-disc game is unchanged.
    assert!(
        MultiDiscLaunchContext::for_content(&sets, &disc("solo.iso"), &snapshot.archives).is_none()
    );

    // A disc changed after the scan never reaches a playlist.
    std::fs::write(disc("orbit (Disc 2).iso"), "replaced disc").unwrap();
    match context.decide(&candidate, None, false) {
        MultiDiscLaunch::SingleDisc { reason } => {
            assert!(
                reason.contains("differs from the last library scan"),
                "{reason}"
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn saved_order_is_ignored_once_its_catalogue_source_is_no_longer_configured() {
    // Same evidence, but the source was removed from the DAT list: its
    // identities are no longer current, so the saved order is not used.
    let identities: crate::multidisc_launch_view::DatIdentities = Default::default();
    let root = crate::multidisc_launch_view::tests::temp_root("unbacked");
    let (_, paths) = crate::multidisc_launch_view::tests::verified_set(&root, 2);
    let archives = crate::multidisc_launch_view::tests::catalogue_archives(&paths);
    let mut evidence = archivefs_core::media_set::MediaEvidence::new(
        archivefs_core::media_set::EvidenceKind::TrustedDat,
        "redump",
    );
    evidence.provenance.version = Some("2026-10-01".into());
    let records = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let name = format!("Orbit (USA) (Disc {})", index + 1);
            archivefs_core::media_set::dat_disc_order::dat_disc_record(
                &archivefs_core::media_set::dat_disc_order::DatDiscCandidate {
                    path: path.clone(),
                    platform: "PSX".into(),
                    format: "cue".into(),
                    source_id: "redump".into(),
                    source_revision: "2026-10-01".into(),
                    catalogue_sha256: None,
                    canonical_name: name.clone(),
                    matched_algorithm: "SHA-1".into(),
                    matched_value: None,
                },
                archivefs_core::media_set::MediaFamily::Optical,
                &archivefs_core::media_set::dat_disc_order::dat_disc_token(&name).unwrap(),
                Some(2),
            )
        })
        .collect();
    let saved =
        archivefs_core::media_set::resolve_index(archivefs_core::media_set::index_media(records))
            .sets;
    assert_eq!(
        saved[0].state,
        archivefs_core::media_set::MediaSetState::CompleteSet
    );
    let sets = sets_from(&saved, &archives, &identities);
    assert!(
        sets.iter()
            .all(|set| !archivefs_core::media_set::dat_disc_order::is_dat_disc_order_set(set)),
        "{sets:?}"
    );
}
