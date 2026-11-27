use super::*;
use crate::dat::catalogue_selection::{CatalogueInventoryInputs, CatalogueRef, resolve_catalogue};
use crate::dat::limits::DatLimits;
use crate::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
use crate::database::PlatformCorrectionGroup;
use sha1::{Digest, Sha1};
use std::os::unix::ffi::OsStrExt;

const NES_HEADER: &str = "Nintendo - Nintendo Entertainment System";

struct Fixture {
    dir: tempfile::TempDir,
    db: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("roms").join("SNES");
        std::fs::create_dir_all(&root).unwrap();
        let db = dir.path().join("library.sqlite3");
        crate::Database::open_or_create(&db).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')",
            [root.as_os_str().as_bytes()],
        )
        .unwrap();
        Self { dir, db, root }
    }

    /// A game file guessed onto `platform` by `source`.
    fn game(
        &self,
        id: i64,
        name: &str,
        body: &[u8],
        platform: &str,
        source: &str,
    ) -> UnmatchedGame {
        let path = self.root.join(name);
        std::fs::write(&path, body).unwrap();
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.execute(
            "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) \
             VALUES(?1,1,?2,?3,?2,'direct_game_image',?4,?4,'now','now','now','now')",
            rusqlite::params![id, name.as_bytes(), path.as_os_str().as_bytes(), name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) VALUES(?1,?2,?3,1,'now')",
            rusqlite::params![id, platform, source],
        )
        .unwrap();
        UnmatchedGame {
            path,
            platform: platform.to_string(),
        }
    }

    /// A trusted NES catalogue listing `games` (name, file, bytes).
    fn nes_catalogue(&self, games: &[(&str, &str, &[u8])]) -> DiscoveryCatalogue {
        let dat = self.dir.path().join("nes.dat");
        let mut body = format!(
            "<datafile><header><name>{NES_HEADER}</name><description>{NES_HEADER}</description><version>20260101</version><author>No-Intro</author></header>"
        );
        for (game, file, bytes) in games {
            let sha1: String = Sha1::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            body.push_str(&format!(
                "<game name=\"{game}\"><rom name=\"{file}\" size=\"{}\" sha1=\"{sha1}\"/></game>",
                bytes.len()
            ));
        }
        body.push_str("</datafile>");
        std::fs::write(&dat, body).unwrap();
        let mut registry = DatSourceRegistry::default();
        let mut entry = DatSourceEntry::new(
            "nes".into(),
            "No-Intro NES".into(),
            dat,
            DatSourceKind::File,
        );
        entry.platform = Some("NES".into());
        entry.platform_chosen_by_person = true;
        registry.add(entry).unwrap();
        let resolved = resolve_catalogue(
            &CatalogueRef::local("nes"),
            CatalogueInventoryInputs {
                local_registry: &registry,
                managed_sources: &Default::default(),
                managed_root: self.dir.path(),
                limits: DatLimits::default(),
            },
        )
        .unwrap();
        DiscoveryCatalogue {
            resolved,
            platform: crate::canonical_platform_for_alias("NES")
                .unwrap()
                .to_string(),
        }
    }

    fn discover(
        &self,
        games: &[UnmatchedGame],
        catalogues: &[DiscoveryCatalogue],
    ) -> DiscoveryOutcome {
        let work = self.dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        discover_other_system_matches(
            &DiscoveryRequest {
                database: &self.db,
                games,
                catalogues,
                trusted: &TrustedRoots::none(),
                generation: 1,
                work_dir: &work,
            },
            &AtomicBool::new(false),
            &|_| {},
        )
        .unwrap()
    }

    fn stored(&self) -> (i64, i64, Vec<(String, String)>) {
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        let mut stmt = conn
            .prepare("SELECT platform, source FROM platform_assignments ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        (
            count("SELECT count(*) FROM library_dat_identities"),
            count("SELECT count(*) FROM platform_assignments"),
            rows,
        )
    }
}

/// The real case: NES games in the SNES folder are found by the NES
/// catalogue and previewed as Ready corrections. Nothing is saved.
#[test]
fn nes_games_in_the_snes_folder_are_found_by_the_nes_catalogue() {
    let fx = Fixture::new();
    let one = fx.game(1, "Game One.nes", b"nes game one", "SNES", "folder_alias");
    let two = fx.game(
        2,
        "Game Two.nes",
        b"nes game two",
        "SNES",
        "registry_detector",
    );
    let unknown = fx.game(
        3,
        "Homebrew.nes",
        b"not in any catalogue",
        "SNES",
        "folder_alias",
    );
    let catalogue = fx.nes_catalogue(&[
        ("Game One (USA)", "Game One.nes", b"nes game one"),
        ("Game Two (Europe)", "Game Two.nes", b"nes game two"),
    ]);
    let before = fx.stored();
    let found = fx.discover(&[one, two, unknown], &[catalogue]);
    assert_eq!(found.catalogues_checked, 1);
    assert!(found.skipped.is_empty(), "{:?}", found.skipped);
    assert_eq!(found.previews.len(), 1);
    let preview = &found.previews[0];
    assert_eq!(preview.preselected(), vec![1, 2]);
    for candidate in &preview.candidates {
        assert_eq!(candidate.current.platform, "SNES");
        assert_eq!(candidate.group, PlatformCorrectionGroup::Ready);
        assert!(
            candidate.algorithm.contains("SHA-1"),
            "{}",
            candidate.algorithm
        );
    }
    assert!(
        preview.catalogue.sha256.is_some(),
        "Apply can re-check the catalogue"
    );
    assert_eq!(fx.stored(), before, "discovery saves nothing");
}

/// A game chosen by hand is found too, but only for review.
#[test]
fn a_chosen_system_found_elsewhere_is_only_for_review() {
    let fx = Fixture::new();
    let chosen = fx.game(1, "Game One.nes", b"nes game one", "SNES", "manual");
    let catalogue = fx.nes_catalogue(&[("Game One (USA)", "Game One.nes", b"nes game one")]);
    let found = fx.discover(&[chosen], &[catalogue]);
    assert_eq!(found.previews.len(), 1);
    assert_eq!(
        found.previews[0].candidates[0].group,
        PlatformCorrectionGroup::NeedsReview
    );
    assert!(found.previews[0].preselected().is_empty());
}

/// A catalogue is offered only files of a format it lists, and never the
/// games of its own system.
#[test]
fn only_other_systems_games_in_listed_formats_are_checked() {
    let fx = Fixture::new();
    let snes_file = fx.game(1, "Real.sfc", b"nes game one", "SNES", "folder_alias");
    let on_nes = fx.game(2, "Game One.nes", b"nes game one", "NES", "folder_alias");
    let catalogue = fx.nes_catalogue(&[("Game One (USA)", "Game One.nes", b"nes game one")]);
    let found = fx.discover(&[snes_file, on_nes], &[catalogue]);
    assert!(found.previews.is_empty(), "{:?}", found.previews);
    assert_eq!(
        found.catalogues_checked, 0,
        "no file to check, so no audit ran"
    );
}

/// Cancelling stops discovery and changes nothing.
#[test]
fn a_cancelled_discovery_changes_nothing() {
    let fx = Fixture::new();
    let one = fx.game(1, "Game One.nes", b"nes game one", "SNES", "folder_alias");
    let catalogue = fx.nes_catalogue(&[("Game One (USA)", "Game One.nes", b"nes game one")]);
    let before = fx.stored();
    let work = fx.dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let error = discover_other_system_matches(
        &DiscoveryRequest {
            database: &fx.db,
            games: &[one],
            catalogues: &[catalogue],
            trusted: &TrustedRoots::none(),
            generation: 1,
            work_dir: &work,
        },
        &AtomicBool::new(true),
        &|_| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert_eq!(fx.stored(), before);
}

/// Only enabled catalogues of other systems are chosen.
#[test]
fn only_other_systems_enabled_catalogues_are_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let mut toml = String::new();
    for (id, header, platform, enabled) in [
        ("nes", NES_HEADER, "NES", true),
        (
            "snes",
            "Nintendo - Super Nintendo Entertainment System",
            "SNES",
            true,
        ),
        ("gb", "Nintendo - Game Boy", "Game Boy", false),
    ] {
        let dat = dir.path().join(format!("{id}.dat"));
        std::fs::write(
            &dat,
            format!("<datafile><header><name>{header}</name><description>{header}</description><version>1</version><author>No-Intro</author></header><game name=\"G\"><rom name=\"g.bin\" size=\"1\" sha1=\"3841bbf35b3033e9f06b30c4d6f9d9127a8288b3\"/></game></datafile>"),
        )
        .unwrap();
        toml.push_str(&format!(
            "[[sources]]\nid = \"{id}\"\ndisplay_name = \"{id}\"\npath = \"{}\"\nkind = \"file\"\nenabled = {enabled}\nplatform = \"{platform}\"\nhealth_state = \"valid\"\n\n",
            dat.display()
        ));
    }
    let config: crate::dat::sources::DatSourcesConfig = toml::from_str(&toml).unwrap();
    let (registry, problems) = DatSourceRegistry::from_config(&config);
    assert!(problems.is_empty(), "{problems:?}");
    let inputs = CatalogueInventoryInputs {
        local_registry: &registry,
        managed_sources: &Default::default(),
        managed_root: dir.path(),
        limits: DatLimits::default(),
    };
    let (chosen, skipped) = other_system_catalogues(inputs, "SNES");
    assert!(skipped.is_empty(), "{skipped:?}");
    let names: Vec<_> = chosen
        .iter()
        .map(|c| c.resolved.source_id.as_str())
        .collect();
    assert_eq!(names, vec!["nes"]);
}
