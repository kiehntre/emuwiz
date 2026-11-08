//! Disposable-fixture journeys: identify -> plan -> approve -> move -> undo.
use super::*;
use crate::dat::rom_organisation::{OrganisationStatus, rollback_organisation_transaction};
use rusqlite::Connection;

fn finding(path: &Path, tier: Tier, detected: &str) -> Finding {
    Finding {
        archive_id: 0,
        title: "Game".into(),
        path: path.to_path_buf(),
        folder: path.parent().unwrap().to_path_buf(),
        assigned: "SNES".into(),
        assigned_source: Some("folder_alias".into()),
        tier,
        detected: detected.into(),
        lines: vec!["evidence".into()],
        has_unh: false,
    }
}

struct Library {
    _temp: tempfile::TempDir,
    root: PathBuf,
    db: crate::Database,
    journal: PathBuf,
}

fn library(files: &[&str]) -> Library {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    for f in files {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, format!("bytes of {f}")).unwrap();
    }
    let mut db = crate::Database::open_or_create(temp.path().join("lib.sqlite3")).unwrap();
    db.register_source_folders(std::slice::from_ref(&root))
        .unwrap();
    let folder = db.list_source_folders().unwrap()[0].id;
    for f in files {
        let a = crate::Archive::from_path_in_root(&root.join(f), &root).unwrap();
        db.upsert_archive(folder, &root, &a).unwrap();
    }
    let journal = temp.path().join("journal");
    Library {
        _temp: temp,
        root,
        db,
        journal,
    }
}

fn platform_of(db: &crate::Database, path: &Path) -> Option<String> {
    let id = db.find_archive_id_by_absolute_path(path).unwrap()?;
    Connection::open(db.path())
        .unwrap()
        .query_row(
            "SELECT platform FROM platform_assignments WHERE archive_id=?1 AND is_current=1",
            [id],
            |r| r.get(0),
        )
        .ok()
}

#[test]
fn mixed_library_journey_moves_confirmed_games_and_leaves_the_rest() {
    let lib = library(&[
        "snes/Dat Game.zip",
        "snes/Homebrew.zip",
        "snes/Maybe.zip",
        "snes/Mixed.zip",
        "mixed/Mystery.zip",
    ]);
    let s = |f: &str| lib.root.join(f);
    let findings = [
        finding(&s("snes/Dat Game.zip"), Tier::Confirmed, "NES"),
        finding(&s("snes/Homebrew.zip"), Tier::PlatformConfirmed, "NES"),
        finding(&s("snes/Maybe.zip"), Tier::Probable, "NES"),
        finding(&s("snes/Mixed.zip"), Tier::Ambiguous, "NES"),
    ];
    let refs: Vec<&Finding> = findings.iter().collect();
    let unknown = [UnknownGame {
        archive_id: 0,
        path: s("mixed/Mystery.zip"),
    }];
    let mut plans = plan_organisation(&refs, &unknown, std::slice::from_ref(&lib.root), 1);
    assert_eq!(plans.len(), 1);
    let summary = summarise(&plans);
    assert_eq!(
        (summary.to_move, summary.to_unknown, summary.left_alone),
        (2, 1, 2)
    );
    // Nothing moved by planning.
    assert!(s("snes/Dat Game.zip").exists());
    assert!(!lib.root.join("Nintendo Entertainment System").exists());

    prepare(&mut plans[0], &lib.db).unwrap();
    let approved: BTreeSet<String> = plans[0]
        .plan
        .suggested()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect();
    let outcome = apply(
        &plans[0],
        &approved,
        &lib.db,
        &lib.journal,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(outcome.summary.applied, 3);

    let folder = crate::platform::canonical_layout_folder("NES").unwrap();
    let moved = lib.root.join(&folder).join("Dat Game.zip");
    assert_eq!(
        std::fs::read(&moved).unwrap(),
        b"bytes of snes/Dat Game.zip"
    );
    assert!(lib.root.join(&folder).join("Homebrew.zip").exists());
    assert!(lib.root.join("Unnamed/Unknown/Mystery.zip").exists());
    // Probable and Ambiguous stay exactly where they were.
    assert!(s("snes/Maybe.zip").exists() && s("snes/Mixed.zip").exists());
    assert_eq!(platform_of(&lib.db, &moved).as_deref(), Some("NES"));
    assert!(
        lib.db
            .find_archive_id_by_absolute_path(&s("snes/Dat Game.zip"))
            .unwrap()
            .is_none()
    );

    let mut tx = outcome.transaction;
    rollback_organisation_transaction(&mut tx, &lib.journal, &AtomicBool::new(false), &lib.root)
        .unwrap();
    assert!(
        s("snes/Dat Game.zip").exists()
            && s("snes/Homebrew.zip").exists()
            && s("mixed/Mystery.zip").exists()
    );
    assert!(!moved.exists());
    assert_ne!(
        platform_of(&lib.db, &s("snes/Dat Game.zip")).as_deref(),
        Some("NES")
    );
}

#[test]
fn an_existing_destination_is_a_collision_and_is_never_overwritten() {
    let lib = library(&["snes/Game.zip", "nes/Game.zip"]);
    let f = finding(&lib.root.join("snes/Game.zip"), Tier::Confirmed, "NES");
    let plans = plan_organisation(&[&f], &[], std::slice::from_ref(&lib.root), 1);
    let entry = &plans[0].plan.entries[0];
    assert_eq!(entry.status, OrganisationStatus::Conflict);
    assert_eq!(summarise(&plans).collisions, 1);
    assert_eq!(
        std::fs::read(lib.root.join("nes/Game.zip")).unwrap(),
        b"bytes of nes/Game.zip"
    );
}

#[test]
fn games_outside_registered_roots_are_left_alone_with_a_reason() {
    let lib = library(&["snes/Game.zip"]);
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("Game.zip");
    std::fs::write(&path, b"x").unwrap();
    let inside = finding(&lib.root.join("snes/Game.zip"), Tier::Confirmed, "NES");
    let stray = finding(&path, Tier::Confirmed, "NES");
    let plans = plan_organisation(&[&inside, &stray], &[], std::slice::from_ref(&lib.root), 1);
    assert!(
        plans[0]
            .left_alone
            .iter()
            .any(|(p, why)| p == &path && why.contains("registered source folder"))
    );
}
