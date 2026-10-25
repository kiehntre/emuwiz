use super::*;
use crate::dat::catalogue_selection::{CatalogueInventoryInputs, CatalogueRef, resolve_catalogue};
use crate::dat::sources::{DatSourceEntry, DatSourceRegistry};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;

struct Fixture {
    dir: tempfile::TempDir,
    db: PathBuf,
    dat: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("roms");
        std::fs::create_dir(&root).unwrap();
        let db = dir.path().join("library.sqlite3");
        Database::open_or_create(&db).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')", [root.as_os_str().as_bytes()]).unwrap();
        let dat = dir.path().join("spectrum.dat");
        let mut f = Self { dir, db, dat, root };
        f.write_dat("Sinclair ZX Spectrum - Games - [TAP]");
        f
    }
    fn write_dat(&mut self, header: &str) {
        let hash = "3841bbf35b3033e9f06b30c4d6f9d9127a8288b3";
        std::fs::write(&self.dat, format!("<datafile><header><name>{header}</name><description>{header}</description><version>synthetic-1</version><author>TOSEC</author></header><game name=\"Synthetic\"><rom name=\"Synthetic.tap\" size=\"23\" sha1=\"{hash}\"/></game></datafile>")).unwrap();
    }
    fn add(&self, id: i64, name: &str, body: Option<&[u8]>, platform: Option<&str>) {
        let path = self.root.join(name);
        if let Some(body) = body {
            std::fs::write(&path, body).unwrap();
        }
        let conn = rusqlite::Connection::open(&self.db).unwrap();
        conn.execute("INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,first_seen_at,last_seen_at,created_at,updated_at) VALUES(?1,1,?2,?3,?2,'direct_game_image',?4,?4,'now','now','now','now')", rusqlite::params![id,name.as_bytes(),path.as_os_str().as_bytes(),name]).unwrap();
        if let Some(platform) = platform {
            conn.execute("INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) VALUES(?1,?2,'manual',1,'now')", rusqlite::params![id,platform]).unwrap();
        }
    }
    fn resolved(&self) -> ResolvedCatalogue {
        // Use the real resolver and an unbound imported catalogue.
        let mut registry = DatSourceRegistry::default();
        registry
            .add(DatSourceEntry::new(
                "synthetic".into(),
                "Synthetic TOSEC".into(),
                self.dat.clone(),
                DatSourceKind::File,
            ))
            .unwrap();
        resolve_catalogue(
            &CatalogueRef::local("synthetic"),
            CatalogueInventoryInputs {
                local_registry: &registry,
                managed_sources: &Default::default(),
                managed_root: self.dir.path(),
                limits: DatLimits::default(),
            },
        )
        .unwrap()
    }
    fn preview(&self) -> VerificationPreview {
        preview_library_verification(self.resolved(), self.db.clone(), "ZX Spectrum".into(), None)
            .unwrap()
    }
    fn run(&self) -> VerificationTotals {
        self.preview()
            .execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
            .unwrap()
    }
    fn zip(&self, name: &str, member: &str, body: &[u8]) {
        let file = std::fs::File::create(self.root.join(name)).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(member, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(body).unwrap();
        zip.finish().unwrap();
    }
}
#[test]
fn spectrum_exact_and_genuine_negative_are_content_verified() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    f.add(2, "b.tap", Some(b"different tape"), Some("ZX Spectrum"));
    let t = f.run();
    assert_eq!(
        (t.verified, t.no_match, t.not_checked, t.saved),
        (1, 1, 0, 2)
    );
}
#[test]
fn preview_excludes_cross_platform_unknown_missing_and_other_representations() {
    let f = Fixture::new();
    for (id, name, platform) in [
        (1, "a.tap", Some("ZX Spectrum")),
        (2, "b.tap", Some("Game Boy")),
        (3, "c.tap", None),
        (4, "d.tzx", Some("ZX Spectrum")),
    ] {
        f.add(id, name, Some(b"matching synthetic tape"), platform);
    }
    f.add(5, "missing.tap", None, Some("ZX Spectrum"));
    let p = f.preview();
    assert_eq!(
        (
            p.selected_games,
            p.eligible_games,
            p.exclusions.len(),
            p.wrong_system,
            p.unknown_system
        ),
        (3, 1, 2, 1, 1)
    );
    let t = p
        .execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
        .unwrap();
    assert_eq!((t.verified, t.not_checked, t.saved), (1, 2, 1));
    let db = Database::open_read_only(&f.db).unwrap();
    for id in [2, 3, 4, 5] {
        assert!(db.library_dat_identities_for_item(id).unwrap().is_empty());
    }
}
#[test]
fn archive_matching_corrupt_and_wrong_representation() {
    let f = Fixture::new();
    for (id, name) in [
        (1, "good.zip"),
        (2, "bad.zip"),
        (3, "other.zip"),
        (4, "negative.zip"),
    ] {
        f.add(id, name, Some(b"corrupt"), Some("ZX Spectrum"));
    }
    f.zip("good.zip", "game.tap", b"matching synthetic tape");
    f.zip("other.zip", "game.tzx", b"another representation");
    f.zip("negative.zip", "game.tap", b"genuine negative");
    let t = f.run();
    assert_eq!((t.verified, t.no_match, t.not_checked), (1, 1, 2));
    assert_eq!(t.unchecked.len(), 2);
    assert!(
        t.unchecked
            .iter()
            .any(|item| item.reason.contains("unreadable"))
    );
    assert_eq!(t.catalogue_sha256, digest(&f.dat).unwrap());
    let db = Database::open_read_only(&f.db).unwrap();
    assert!(
        db.library_dat_identities_for_item(1).unwrap()[0]
            .archive_member
            .is_some()
    );
}
#[test]
fn changed_dat_cannot_use_stale_approval() {
    let mut f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let p = f.preview();
    f.write_dat("Nintendo - Game Boy");
    assert!(
        p.execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
            .is_err()
    );
    assert!(
        Database::open_read_only(&f.db)
            .unwrap()
            .library_dat_identities_for_item(1)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn changed_membership_cannot_use_stale_approval() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let p = f.preview();
    f.add(2, "b.tap", Some(b"extra"), Some("ZX Spectrum"));
    assert!(
        p.execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
            .is_err()
    );
}
#[test]
fn changed_rom_cannot_use_stale_approval() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let p = f.preview();
    std::fs::write(f.root.join("a.tap"), b"changed").unwrap();
    assert!(
        p.execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
            .is_err()
    );
}
#[test]
fn changed_dat_during_hashing_prevents_writes() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let p = f.preview();
    let result = p.execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {
        std::fs::write(&f.dat, b"changed").unwrap();
    });
    assert!(result.is_err());
    assert!(
        Database::open_read_only(&f.db)
            .unwrap()
            .library_dat_identities_for_item(1)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn cancellation_prevents_writes() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    assert!(
        f.preview()
            .execute(&TrustedRoots::none(), &AtomicBool::new(true), &|_| {})
            .is_err()
    );
}
#[test]
fn wrong_catalogue_platform_fails_closed() {
    let f = Fixture::new();
    assert!(
        preview_library_verification(f.resolved(), f.db.clone(), "Game Boy".into(), None).is_err()
    );
}
#[test]
fn unavailable_database_never_falls_back_to_folder_walk() {
    let f = Fixture::new();
    assert!(
        preview_library_verification(
            f.resolved(),
            f.dir.path().join("missing.sqlite3"),
            "ZX Spectrum".into(),
            None
        )
        .is_err()
    );
}
#[test]
fn collection_bounds_the_population() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let p = preview_library_verification(
        f.resolved(),
        f.db.clone(),
        "ZX Spectrum".into(),
        Some(f.root.join("subfolder")),
    )
    .unwrap();
    assert_eq!(
        (p.selected_games, p.eligible_games, p.outside_collection),
        (0, 0, 1)
    );
}
#[test]
fn duplicate_paths_are_excluded_even_across_platforms() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    f.add(2, "shadow.tap", None, Some("Game Boy"));
    rusqlite::Connection::open(&f.db)
        .unwrap()
        .execute(
            "UPDATE archives SET absolute_path_cached=?1 WHERE id=2",
            [f.root.join("a.tap").as_os_str().as_bytes()],
        )
        .unwrap();
    let p = f.preview();
    assert_eq!(p.eligible_games, 0);
    assert!(p.exclusions[0].reason.contains("Ambiguous"));
}

#[test]
fn corrupt_archive_preserves_prior_verified_member_evidence() {
    let f = Fixture::new();
    f.add(1, "a.zip", Some(b"placeholder"), Some("ZX Spectrum"));
    f.zip("a.zip", "game.tap", b"matching synthetic tape");
    assert_eq!(f.run().verified, 1);
    let before = Database::open_read_only(&f.db)
        .unwrap()
        .library_dat_identities_for_item(1)
        .unwrap();
    std::fs::write(f.root.join("a.zip"), b"broken ZIP").unwrap();
    let t = f.run();
    assert_eq!(
        (t.verified, t.no_match, t.not_checked, t.protected_prior),
        (0, 0, 1, 1)
    );
    let after = Database::open_read_only(&f.db)
        .unwrap()
        .library_dat_identities_for_item(1)
        .unwrap();
    assert_eq!(before, after);
}
#[test]
fn incomplete_nested_archive_is_not_a_definitive_negative() {
    let f = Fixture::new();
    f.add(1, "a.zip", Some(b"placeholder"), Some("ZX Spectrum"));
    f.zip("a.zip", "nested.zip", b"not inspected recursively");
    let t = f.run();
    assert_eq!((t.verified, t.no_match, t.not_checked), (0, 0, 1));
}
#[test]
fn membership_change_during_hashing_prevents_transactional_save() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let p = f.preview();
    let changed = AtomicBool::new(false);
    let result = p.execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {
        if !changed.swap(true, Ordering::Relaxed) {
            let conn = rusqlite::Connection::open(&f.db).unwrap();
            conn.execute(
                "UPDATE platform_assignments SET platform='Game Boy' WHERE archive_id=1",
                [],
            )
            .unwrap();
        }
    });
    assert!(result.is_err());
    assert!(
        Database::open_read_only(&f.db)
            .unwrap()
            .library_dat_identities_for_item(1)
            .unwrap()
            .is_empty()
    );
}
fn other_tables(db: &Path) -> BTreeMap<String, Vec<String>> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let names:Vec<String>=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT IN ('library_dat_identities', 'dat_expected_entries', 'dat_expected_inventory_meta') ORDER BY name").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
                .unwrap();
            let n = stmt.column_count();
            let mut data: Vec<String> = stmt
                .query_map([], |row| {
                    Ok(format!(
                        "{:?}",
                        (0..n)
                            .map(|i| row.get::<_, rusqlite::types::Value>(i).unwrap())
                            .collect::<Vec<_>>()
                    ))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            data.sort();
            (name, data)
        })
        .collect()
}
#[test]
fn verification_changes_no_unrelated_database_tables_or_roms() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let before = other_tables(&f.db);
    let dat = std::fs::read(&f.dat).unwrap();
    f.run();
    assert_eq!(before, other_tables(&f.db));
    assert_eq!(dat, std::fs::read(&f.dat).unwrap());
    assert_eq!(
        std::fs::read(f.root.join("a.tap")).unwrap(),
        b"matching synthetic tape"
    );
}

#[test]
fn corrupt_zip_member_counts_one_error_and_cannot_be_negative() {
    let f = Fixture::new();
    f.add(1, "crc.zip", Some(b"placeholder"), Some("ZX Spectrum"));
    let file = std::fs::File::create(f.root.join("crc.zip")).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "game.tap",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(b"matching synthetic tape").unwrap();
    zip.finish().unwrap();
    let mut bytes = std::fs::read(f.root.join("crc.zip")).unwrap();
    let pos = bytes
        .windows(23)
        .position(|b| b == b"matching synthetic tape")
        .unwrap();
    bytes[pos] ^= 1;
    std::fs::write(f.root.join("crc.zip"), bytes).unwrap();
    let t = f.run();
    assert_eq!(
        (t.verified, t.no_match, t.not_checked, t.errors),
        (0, 0, 1, 1)
    );
}
#[test]
fn unreadable_permissions_are_excluded_in_preview() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.add(
        1,
        "locked.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    std::fs::set_permissions(
        f.root.join("locked.tap"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let p = f.preview();
    assert_eq!(p.eligible_games, 0);
    assert!(p.exclusions[0].reason.contains("Unreadable"));
}
#[test]
fn saved_evidence_is_bound_to_the_confirmed_catalogue_hash() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let p = f.preview();
    let expected = p.sha256.clone();
    p.execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
        .unwrap();
    let saved = Database::open_read_only(&f.db)
        .unwrap()
        .library_dat_identities_for_item(1)
        .unwrap();
    assert_eq!(
        saved[0].source.catalogue_sha256.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(saved[0].source.dat_path, f.dat.to_string_lossy());
    let database = Database::open_read_only(&f.db).unwrap();
    assert_eq!(
        database.expected_dat_inventory_sha256("synthetic").unwrap(),
        Some(expected)
    );
    assert_eq!(database.expected_dat_entry_count("synthetic").unwrap(), 1);
}

#[test]
fn incomplete_catalogue_hash_coverage_withholds_negatives_but_keeps_exact_matches() {
    let f = Fixture::new();
    f.add(
        1,
        "exact.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    f.add(
        2,
        "unknown.tap",
        Some(b"cannot rule out unsupported hash entry"),
        Some("ZX Spectrum"),
    );
    let text = std::fs::read_to_string(&f.dat).unwrap().replace(
        "</datafile>",
        &format!(
            "<game name=\"SHA256 only\"><rom name=\"other.tap\" sha256=\"{}\"/></game></datafile>",
            "0".repeat(64)
        ),
    );
    std::fs::write(&f.dat, text).unwrap();
    let p = f.preview();
    assert!(p.formats.contains("tap"));
    assert!(!p.negative_formats.contains("tap"));
    let t = p
        .execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
        .unwrap();
    assert_eq!(
        (t.verified, t.no_match, t.not_checked, t.saved),
        (1, 0, 1, 1)
    );
}

#[test]
fn exact_hash_collision_is_ambiguous_and_counted_once() {
    let f = Fixture::new();
    f.add(
        1,
        "exact.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let text = std::fs::read_to_string(&f.dat).unwrap();
    let start = text.find("<game ").unwrap();
    let game = text[start..text.find("</datafile>").unwrap()].replace("Synthetic", "Twin");
    std::fs::write(
        &f.dat,
        text.replace("</datafile>", &format!("{game}</datafile>")),
    )
    .unwrap();
    let t = f.run();
    assert_eq!(
        (t.verified, t.no_match, t.ambiguous, t.not_checked, t.saved),
        (0, 0, 1, 0, 1)
    );
}
#[test]
fn complete_negative_zip_with_ancillary_content_remains_a_genuine_negative() {
    let f = Fixture::new();
    f.add(1, "a.zip", Some(b"placeholder"), Some("ZX Spectrum"));
    let file = std::fs::File::create(f.root.join("a.zip")).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    for (name, body) in [
        ("game.tap", b"genuine negative".as_slice()),
        ("readme.txt", b"ancillary text".as_slice()),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
    let t = f.run();
    assert_eq!((t.verified, t.no_match, t.not_checked), (0, 1, 0));
}

#[test]
fn ambiguous_archive_member_is_not_a_definitive_negative_for_its_parent() {
    let f = Fixture::new();
    f.add(1, "a.zip", Some(b"placeholder"), Some("ZX Spectrum"));
    f.zip("a.zip", "game.tap", b"matching synthetic tape");
    let text = std::fs::read_to_string(&f.dat).unwrap();
    let start = text.find("<game ").unwrap();
    let game = text[start..text.find("</datafile>").unwrap()].replace("Synthetic", "Twin");
    std::fs::write(
        &f.dat,
        text.replace("</datafile>", &format!("{game}</datafile>")),
    )
    .unwrap();
    let t = f.run();
    assert_eq!((t.verified, t.no_match, t.not_checked), (0, 0, 1));
}
#[test]
fn partial_multirom_archive_is_not_a_definitive_negative_for_its_parent() {
    let f = Fixture::new();
    f.add(1, "a.zip", Some(b"placeholder"), Some("ZX Spectrum"));
    f.zip("a.zip", "game.tap", b"matching synthetic tape");
    let text = std::fs::read_to_string(&f.dat).unwrap().replace(
        "</game>",
        &format!(
            "<rom name=\"missing.tap\" sha1=\"{}\"/></game>",
            "1".repeat(40)
        ),
    );
    std::fs::write(&f.dat, text).unwrap();
    let t = f.run();
    assert_eq!((t.verified, t.no_match, t.not_checked), (0, 0, 1));
}

#[test]
fn scoped_persistence_refuses_an_outcome_from_different_catalogue_bytes() {
    let f = Fixture::new();
    f.add(
        1,
        "a.tap",
        Some(b"matching synthetic tape"),
        Some("ZX Spectrum"),
    );
    let preview = f.preview();
    let mut request = preview.request.clone();
    // A preview uses '/' only with explicit targets. Bound this synthetic
    // fixture's root as well, so even a target-selection regression stays local.
    request.scan_root = f.root.clone();
    let targets = AuditTargets::Catalogue(CatalogueAuditTargets {
        files: preview.scope.eligible.keys().cloned().collect(),
        catalogue_rows: preview.eligible_games,
        ..Default::default()
    });
    let mut outcome = run_dat_audit_with_targets(
        &request,
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
        &targets,
    )
    .unwrap();
    outcome.catalogue_sha256 = Some("f".repeat(64));
    let mut database = Database::open_or_create(&f.db).unwrap();
    assert!(
        database
            .persist_scoped_library_dat_verification(&outcome, &preview.scope)
            .is_err()
    );
    assert!(
        database
            .library_dat_identities_for_item(1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(database.expected_dat_entry_count("synthetic").unwrap(), 0);
}

/// A CUE disc is one library row; its track files are not rows. Confirming a
/// verification must still hash the tracks so the whole disc can be judged,
/// without counting or saving the tracks as games of their own.
#[test]
fn a_cue_disc_is_verified_with_its_untracked_track_files() {
    let (name_1, name_2) = ("Track 01 ; $.bin", "Track 02 audio.bin");
    let (f, cue, [track_1, track_2]) = cue_fixture([name_1, name_2]);
    let preview = |f: &Fixture| {
        preview_library_verification(f.resolved(), f.db.clone(), "PSX".into(), None).unwrap()
    };
    let run = |preview: VerificationPreview| {
        preview.execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
    };
    let archive_count = |f: &Fixture| -> i64 {
        rusqlite::Connection::open(&f.db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM archives", [], |row| row.get(0))
            .unwrap()
    };

    // The preview accounts for the whole disc's bytes, still one game.
    assert!(cue_launch_content(&f).has_runnable_path());
    let p = preview(&f);
    assert_eq!(p.eligible_games, 1);
    assert_eq!(
        p.bytes,
        (cue.len() + track_1.len() + track_2.len()) as u64,
        "the sheet and both track files"
    );
    let totals = run(p).unwrap();
    assert_eq!(
        (
            totals.verified,
            totals.no_match,
            totals.ambiguous,
            totals.saved
        ),
        (1, 0, 0, 1),
        "{totals:?}"
    );
    assert_eq!(archive_count(&f), 1, "tracks are never catalogued as games");

    // A track changed after Preview invalidates the approval.
    let stale = preview(&f);
    std::fs::write(f.root.join(name_2), vec![9u8; 2352 * 2]).unwrap();
    let refused = run(stale).unwrap_err();
    assert!(refused.contains("changed after preview"), "{refused}");

    // A wrong track after a fresh preview is not Verified, and the stored
    // reason names the file instead of hiding it.
    let totals = run(preview(&f)).unwrap();
    assert_eq!(totals.verified, 0, "{totals:?}");
    // Structurally valid media remains launchable even when its hashes differ.
    assert!(cue_launch_content(&f).has_runnable_path());
    let facts: Vec<u8> = rusqlite::Connection::open(&f.db)
        .unwrap()
        .query_row(
            "SELECT facts_json FROM library_dat_identities WHERE archive_id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let facts = String::from_utf8_lossy(&facts);
    assert!(
        !facts.contains("verified_single_match"),
        "a changed track must not leave the disc verified"
    );
    assert!(facts.contains(name_2), "{facts}");
    assert!(
        facts.contains("differing or unrecognised on disc"),
        "{facts}"
    );
    assert!(archive_count(&f) == 1);
}

fn cue_fixture(names: [&str; 2]) -> (Fixture, String, [Vec<u8>; 2]) {
    use sha1::{Digest, Sha1};
    let sha1 = |bytes: &[u8]| -> String {
        Sha1::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let f = Fixture::new();
    let [name_1, name_2] = names;
    let cue = format!(
        "FILE \"{name_1}\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\nFILE \"{name_2}\" BINARY\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n"
    );
    let (track_1, track_2) = (vec![1u8; 2352 * 3], vec![2u8; 2352 * 2]);
    std::fs::write(
        &f.dat,
        format!(
            "<datafile><header><name>Sony - PlayStation</name><description>Sony - PlayStation</description><version>synthetic-1</version><author>redump.org</author></header><game name=\"Synthetic Disc (USA)\"><rom name=\"Synthetic Disc.cue\" size=\"{}\" sha1=\"{}\"/><rom name=\"{name_1}\" size=\"{}\" sha1=\"{}\"/><rom name=\"{name_2}\" size=\"{}\" sha1=\"{}\"/></game></datafile>",
            cue.len(),
            sha1(cue.as_bytes()),
            track_1.len(),
            sha1(&track_1),
            track_2.len(),
            sha1(&track_2),
        ),
    )
    .unwrap();
    f.add(1, "Synthetic Disc.cue", Some(cue.as_bytes()), Some("PSX"));
    for (name, bytes) in [(name_1, &track_1), (name_2, &track_2)] {
        std::fs::create_dir_all(f.root.join(name).parent().unwrap()).unwrap();
        std::fs::write(f.root.join(name), bytes).unwrap();
    }
    (f, cue, [track_1, track_2])
}

fn cue_preview(f: &Fixture) -> VerificationPreview {
    preview_library_verification(f.resolved(), f.db.clone(), "PSX".into(), None).unwrap()
}
fn cue_run(f: &Fixture) -> VerificationTotals {
    cue_preview(f)
        .execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
        .unwrap()
}
fn cue_identity(
    f: &Fixture,
) -> super::super::library_identity_summary::PersistedLibraryDatIdentity {
    Database::open_read_only(&f.db)
        .unwrap()
        .library_dat_identity_for_item(1, "synthetic")
        .unwrap()
        .unwrap()
}

#[test]
fn cue_verification_hashes_only_required_files_with_plain_and_tricky_names() {
    for names in [
        ["Track01.bin", "Track02.bin"],
        ["Track 01 ; $.bin", "音声 ' (02) [x].BIN"],
        ["./Track01.bin", "./sub dir/Track02.bin"],
    ] {
        let (f, _, _) = cue_fixture(names);
        std::fs::write(f.root.join("unrelated.bin"), b"not a disc member").unwrap();
        let p = cue_preview(&f);
        assert_eq!(p.targets.files.len(), 3);
        assert_eq!(p.targets.companion_files, 2);
        let hashed = std::sync::Mutex::new(Vec::new());
        let t = p
            .execute(
                &TrustedRoots::none(),
                &AtomicBool::new(false),
                &|progress| {
                    if let DatAuditProgress::Hashing { file_name, .. } = progress {
                        hashed.lock().unwrap().push(file_name);
                    }
                },
            )
            .unwrap();
        assert_eq!((t.verified, t.saved, t.errors), (1, 1, 0), "{t:?}");
        assert_eq!(hashed.lock().unwrap().len(), 3);
        assert!(
            !hashed
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == "unrelated.bin")
        );
        assert_eq!(cue_identity(&f).disc_members.len(), 3);
        assert_eq!((cue_run(&f).verified, cue_run(&f).saved), (1, 1));
    }
}

#[test]
fn native_cue_verification_survives_unchanged_production_rescans() {
    let (f, _, _) = cue_fixture(["Track01.bin", "Track02.bin"]);
    assert_eq!(cue_run(&f).verified, 1);
    let config = crate::Config {
        source_folders: vec![f.root.clone()],
        mount_root: f.dir.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    let mut database = Database::open_or_create(&f.db).unwrap();
    for _ in 0..2 {
        crate::scan_and_persist(&mut database, &config, "native-cue-rescan").unwrap();
        assert_eq!(database.load_archives().unwrap().len(), 1);
        assert_eq!(
            database.library_dat_states().unwrap().get(&1),
            Some(&crate::LibraryDatState::Verified)
        );
        assert!(cue_identity(&f).disc_members_unchanged());
    }
    assert_eq!(cue_run(&f).verified, 1);
}

#[test]
fn missing_or_unsafe_cue_companions_cannot_verify_the_disc() {
    use std::os::unix::fs::symlink;
    for defect in [
        "missing",
        "directory",
        "escape-symlink",
        "parent",
        "absolute",
    ] {
        let (f, cue, tracks) = cue_fixture(["Track01.bin", "Track02.bin"]);
        let track = f.root.join("Track02.bin");
        std::fs::remove_file(&track).unwrap();
        let outside = f.dir.path().join("outside.bin");
        std::fs::write(&outside, &tracks[1]).unwrap();
        match defect {
            "directory" => std::fs::create_dir(&track).unwrap(),
            "escape-symlink" => symlink(&outside, &track).unwrap(),
            "parent" | "absolute" => {
                let reference = if defect == "parent" {
                    "../outside.bin".into()
                } else {
                    outside.to_string_lossy().into_owned()
                };
                let changed = cue.replace("Track02.bin", &reference);
                std::fs::write(f.root.join("Synthetic Disc.cue"), &changed).unwrap();
                // Keep the sheet itself an exact hash match: only the unsafe
                // companion reference may prevent whole-disc verification.
                let record = |body: &str| {
                    format!(
                        "size=\"{}\" sha1=\"{}\"",
                        body.len(),
                        hex(&sha1::Sha1::digest(body.as_bytes()))
                    )
                };
                let dat = std::fs::read_to_string(&f.dat).unwrap();
                std::fs::write(&f.dat, dat.replace(&record(&cue), &record(&changed))).unwrap();
            }
            _ => {}
        }
        let p = cue_preview(&f);
        assert!(!p.targets.files.contains(&outside));
        let t = cue_run(&f);
        assert_eq!(t.verified, 0, "{defect}: {t:?}");
        assert!(
            !matches!(
                cue_identity(&f).verification_state,
                DatVerificationState::VerifiedSingleMatch { .. }
            ),
            "{defect}"
        );
    }
}

#[test]
fn cue_companion_changes_before_or_during_hashing_refuse_the_save() {
    for change in ["remove", "replace-symlink", "during-hashing"] {
        let (f, _, tracks) = cue_fixture(["Track01.bin", "Track02.bin"]);
        let p = cue_preview(&f);
        let track = f.root.join("Track02.bin");
        match change {
            "remove" => std::fs::remove_file(&track).unwrap(),
            "replace-symlink" => {
                let outside = f.dir.path().join("outside.bin");
                std::fs::write(&outside, &tracks[1]).unwrap();
                std::fs::remove_file(&track).unwrap();
                std::os::unix::fs::symlink(outside, &track).unwrap();
            }
            _ => {}
        }
        let error = p
            .execute(
                &TrustedRoots::none(),
                &AtomicBool::new(false),
                &|progress| {
                    if change == "during-hashing"
                        && matches!(progress, DatAuditProgress::Comparing { .. })
                    {
                        std::fs::write(&track, vec![9u8; tracks[1].len()]).unwrap();
                    }
                },
            )
            .unwrap_err();
        assert!(error.contains("changed after preview"), "{change}: {error}");
        assert!(
            Database::open_read_only(&f.db)
                .unwrap()
                .library_dat_identities_for_item(1)
                .unwrap()
                .is_empty()
        );
    }
}

fn cue_launch_content(f: &Fixture) -> crate::launch::planning::LaunchContentRef {
    let path = f.root.join("Synthetic Disc.cue");
    let archive = crate::Archive {
        identity: crate::ArchiveIdentity::from_path(
            &path,
            f.root.clone(),
            Some(&std::fs::metadata(&path).unwrap()),
        ),
        path,
        kind: crate::ArchiveKind::DirectGameImage,
        health: crate::ArchiveHealth::Pending,
    };
    let record = crate::ArchiveRecord::new(
        crate::MountPlan::new(archive, f.dir.path().join("mount")),
        crate::MountState::NotMountable,
        crate::ArchiveMetadata::empty(),
        crate::ArchiveHealth::Pending,
    );
    crate::launch::evidence_bridge::launch_content_ref_from_archive_record(&record, None)
}
