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

#[test]
fn verified_cue_bin_discs_reach_game_details_and_preview_and_a_changed_track_stops_them() {
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

    let snapshot = || {
        let reopened = archivefs_core::Database::open_read_only(&database_path).unwrap();
        crate::database_load::load_snapshot_from(&reopened, &database_path, &fixture.config_path)
            .unwrap_or_else(|_| panic!("library snapshot"))
    };
    let first = games.join("first.cue");
    let second = games.join("second.cue");
    let loaded = snapshot();
    let sets = sets_from(&loaded.media_topology, &loaded.archives);
    let context = MultiDiscLaunchContext::for_content(&sets, &first, &loaded.archives)
        .expect("Game Details finds the verified set");
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
