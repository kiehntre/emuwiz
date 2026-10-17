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

struct Library {
    fixture: Fixture,
    page: DatSourcesPageState,
    games: PathBuf,
    database_path: PathBuf,
}

const DAT_HEADER: &str = r#"<?xml version="1.0"?><datafile><header><name>Sony - PlayStation</name><version>{version}</version><author>redump.org</author></header>"#;

fn redump_dat(version: &str) -> String {
    let body: String = ENTRIES
        .iter()
        .map(|(entry, sha1)| {
            format!(
                r#"<game name="{entry}"><rom name="{entry}.bin" size="{}" sha1="{sha1}"/></game>"#,
                format!("bytes of {entry}").len()
            )
        })
        .collect();
    format!(
        "{}{body}</datafile>",
        DAT_HEADER.replace("{version}", version)
    )
}

impl Library {
    /// Scan, then add and save the DAT. Nothing validated or audited yet.
    fn added() -> Self {
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
        let dat = fixture.write("redump.dat", &redump_dat("2026-10-01"));
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
        Self {
            fixture,
            page,
            games,
            database_path,
        }
    }

    /// Scan, add the DAT, Save, Validate, Audit - the user's own steps.
    fn audited() -> Self {
        let mut library = Self::added();
        library.validate();
        library.audit();
        library
    }

    fn validate(&mut self) {
        self.page.apply(DatSourcesPageAction::Validate {
            id: "redump".into(),
        });
        run_to_completion(&mut self.page);
    }

    fn audit(&mut self) -> DatSaveResult {
        self.page.apply(DatSourcesPageAction::Audit {
            id: "redump".into(),
            scan_root: self.fixture.root.join("games"),
        });
        run_to_completion(&mut self.page);
        self.page
            .view()
            .save_results
            .into_iter()
            .find(|r| r.operation == DatSaveOperation::Audit && r.source_id == "redump")
            .unwrap()
    }

    fn readiness(&self) -> Option<String> {
        self.page
            .view()
            .rows
            .into_iter()
            .find(|row| row.id == "redump")
            .unwrap()
            .audit_readiness
    }

    fn disc(&self, name: &str) -> PathBuf {
        self.games.join(name)
    }

    /// Reload the library exactly as the app does.
    fn snapshot(&self) -> crate::database_load::CachedLibrarySnapshot {
        let reopened = archivefs_core::Database::open_read_only(&self.database_path).unwrap();
        crate::database_load::load_snapshot_from(
            &reopened,
            &self.database_path,
            &self.fixture.config_path,
        )
        .unwrap_or_else(|_| panic!("library snapshot"))
    }

    /// Whether Game Details would show disc 1 in a verified order.
    fn order_verified(&self) -> bool {
        let snapshot = self.snapshot();
        let sets = sets_from(&snapshot.media_topology, &snapshot.archives);
        MultiDiscLaunchContext::for_content(
            &sets,
            &self.disc("orbit (Disc 1).iso"),
            &snapshot.archives,
        )
        .is_some_and(|context| {
            guidance(
                context.sets.first().unwrap(),
                &self.disc("orbit (Disc 1).iso"),
                None,
            )
            .order_verified
        })
    }
}

#[test]
fn verified_dat_order_reaches_game_details_and_preview_without_database_edits() {
    let library = Library::audited();
    let Library { page, games, .. } = &library;
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

    let snapshot = library.snapshot();
    let sets = sets_from(&snapshot.media_topology, &snapshot.archives);
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
fn saved_order_is_retired_when_the_source_is_removed_from_the_dat_list() {
    let mut library = Library::audited();
    assert!(library.order_verified());
    library.page.apply(DatSourcesPageAction::Remove {
        id: "redump".into(),
    });
    library.page.apply(DatSourcesPageAction::Save);
    assert!(!library.order_verified());
}

#[test]
fn a_newer_catalogue_validated_without_a_re_audit_retires_the_saved_order() {
    let mut library = Library::audited();
    assert!(library.order_verified());
    // Same path, newer revision, validated but not audited again.
    std::fs::write(
        library.fixture.root.join("redump.dat"),
        redump_dat("2026-11-01"),
    )
    .unwrap();
    library.page.apply(DatSourcesPageAction::Validate {
        id: "redump".into(),
    });
    run_to_completion(&mut library.page);
    assert!(!library.order_verified());
}

#[test]
fn a_later_contrary_identity_retires_the_saved_order() {
    let library = Library::audited();
    assert!(library.order_verified());
    // Another source verifies disc 1's exact bytes as a different entry; it is
    // saved directly, so no disc-order refresh runs after it.
    let other = library.fixture.write(
        "other.dat",
        &format!(
            r#"<?xml version="1.0"?><datafile><header><name>Other</name><version>7</version></header><game name="Orbit Quest (Europe) (Disc 1)"><rom name="x.bin" size="{}" sha1="{}"/></game></datafile>"#,
            format!("bytes of {}", ENTRIES[0].0).len(),
            ENTRIES[0].1
        ),
    );
    let audit = archivefs_core::dat::sources::audit_run::run_dat_audit_with_cache(
        &archivefs_core::dat::sources::audit_run::DatAuditRequest {
            source_id: "other".into(),
            source_display_name: "Other".into(),
            dat_path: other,
            dat_kind: DatSourceKind::File,
            scan_root: library.fixture.root.join("games"),
            limits: DatLimits::default(),
            policy: None,
            platform: None,
        },
        &TrustedRoots::none(),
        &std::sync::atomic::AtomicBool::new(false),
        &|_| {},
        archivefs_core::dat::sources::audit_cache::AuditCacheConfig::Disabled,
    )
    .unwrap();
    archivefs_core::Database::open_or_create(&library.database_path)
        .unwrap()
        .persist_library_dat_identities_from_audit(&audit)
        .unwrap();
    // The other source must be configured for its identity to be current.
    let mut config = std::fs::read_to_string(&library.fixture.config_path).unwrap();
    config.push_str(&format!(
        "\n[[sources]]\nid = \"other\"\ndisplay_name = \"Other\"\npath = \"{}\"\nkind = \"file\"\n",
        library.fixture.root.join("other.dat").display()
    ));
    std::fs::write(&library.fixture.config_path, config).unwrap();
    assert!(!library.order_verified());
}

#[test]
fn audit_before_validation_says_validate_first_and_the_next_steps_are_pointed_out() {
    let mut library = Library::added();
    assert!(
        library
            .readiness()
            .is_some_and(|r| r.starts_with("Not validated yet")),
        "{:?}",
        library.readiness()
    );
    let premature = library.audit();
    assert_eq!(premature.outcome, DatSaveOutcome::SuccessWithWarnings);
    assert!(
        premature
            .technical_details
            .iter()
            .any(|d| d.contains("has not been validated") && d.contains("Validate")),
        "{:?}",
        premature.technical_details
    );
    assert!(
        !premature
            .technical_details
            .iter()
            .any(|d| d.contains("nothing to order"))
    );
    assert!(!library.order_verified());

    library.validate();
    assert!(
        library
            .readiness()
            .is_some_and(|r| r.starts_with("Validated. Next: Audit")),
        "{:?}",
        library.readiness()
    );
    let audited = library.audit();
    assert!(
        audited
            .technical_details
            .iter()
            .any(|d| d.contains("1 game(s) saved")),
        "{:?}",
        audited.technical_details
    );
    assert_eq!(library.readiness(), None);
    assert!(library.order_verified());
}

#[test]
fn a_changed_dat_is_flagged_and_auditing_it_without_validating_saves_no_order() {
    let mut library = Library::audited();
    assert!(library.order_verified());
    std::fs::write(
        library.fixture.root.join("redump.dat"),
        redump_dat("2026-11-01"),
    )
    .unwrap();
    assert!(
        library
            .readiness()
            .is_some_and(|r| r.contains("changed since it was validated")),
        "{:?}",
        library.readiness()
    );
    let audit = library.audit();
    assert!(
        audit
            .technical_details
            .iter()
            .any(|d| d.contains("validated for a different version")),
        "{:?}",
        audit.technical_details
    );
    assert!(!library.order_verified());
}
