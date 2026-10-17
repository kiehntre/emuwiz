use archivefs_core::ingestion::{
    bounded_import::{
        BoundedImportPlan, apply_bounded_import, begin_bounded_preview, prepare_bounded_import,
    },
    preview::{MediaDisposition, SpectrumCatalogue, preview_media},
};
use archivefs_core::{Config, Database};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
fn snapshot(tag: u8) -> Vec<u8> {
    let mut b = vec![0; 49182];
    b[7] = 128;
    b[27] = 255;
    b[28] = 255;
    b[29] = 1;
    b[100] = tag;
    b
}
fn sna(tag: u8) -> Vec<u8> {
    let mut b = vec![0; 49179];
    b[24] = 192;
    b[25] = 1;
    b[26] = 31;
    b[100] = tag;
    b
}
fn config(root: &Path) -> Config {
    Config {
        source_folders: vec![root.into()],
        mount_root: PathBuf::new(),
        ratarmount_bin: String::new(),
        master_rom_root: None,
    }
}
struct Fixture {
    _t: tempfile::TempDir,
    root: PathBuf,
    parent: PathBuf,
    db: PathBuf,
    cfg: PathBuf,
    dat: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("games");
        let root = parent.join("zxs");
        std::fs::create_dir_all(&root).unwrap();
        let db = t.path().join("copy.sqlite3");
        let cfg = t.path().join("config.toml");
        let dat = t.path().join("catalogue.dat");
        std::fs::write(root.join("existing.z80"), snapshot(1)).unwrap();
        let mut d = Database::open_or_create(&db).unwrap();
        archivefs_core::scan_and_persist(&mut d, &config(&parent), "synthetic-seed").unwrap();
        archivefs_core::save_source_folder_configs_to(
            &cfg,
            &[archivefs_core::SourceFolderConfig {
                path: parent.clone(),
                enabled: true,
                created_at: None,
            }],
            &t.path().join("mount"),
            "",
            None,
        )
        .unwrap();
        Self {
            _t: t,
            root,
            parent,
            db,
            cfg,
            dat,
        }
    }
    fn catalogue(&self, files: &[(&str, &str)]) -> SpectrumCatalogue {
        let mut xml = String::from(
            "<datafile><header><name>Sinclair ZX Spectrum - Games - [Z80]</name></header>",
        );
        for (file, game) in files {
            let h = archivefs_core::identity_source::hashing::hash_file(
                &self.root.join(file),
                &archivefs_core::safe_read::TrustedRoots::none(),
                None,
            )
            .unwrap();
            xml.push_str(&format!("<game name=\"{game}\"><rom name=\"unrelated\" size=\"{}\" crc=\"{}\" md5=\"{}\" sha1=\"{}\"/></game>",h.bytes_hashed,h.crc32,h.md5,h.sha1));
        }
        xml.push_str("</datafile>");
        std::fs::write(&self.dat, xml).unwrap();
        SpectrumCatalogue::from_parsed(
            &archivefs_core::dat::parsers::parse_dat_file(&self.dat, Default::default())
                .unwrap()
                .dat,
        )
        .unwrap()
    }
    fn plan(&self, files: &[(&str, &str)]) -> BoundedImportPlan {
        let cat = self.catalogue(files);
        let d = Database::open_read_only(&self.db).unwrap();
        let context = begin_bounded_preview(&d, &self.cfg).unwrap();
        let p = preview_media(
            &config(&self.root),
            &[],
            &d.load_archives().unwrap(),
            &[cat],
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap();
        prepare_bounded_import(&p, context).unwrap()
    }
    fn rows(&self) -> Vec<archivefs_core::PersistedArchive> {
        Database::open_read_only(&self.db)
            .unwrap()
            .load_archives()
            .unwrap()
    }
}
#[test]
fn bounded_allowlist_only_preserves_parts_releases_and_existing_evidence() {
    let f = Fixture::new();
    std::fs::write(f.root.join("alternate.Z80"), snapshot(2)).unwrap();
    std::fs::write(f.root.join("part.SNA"), sna(3)).unwrap();
    let old = f.rows();
    let plan = f.plan(&[
        ("alternate.Z80", "Existing alternate release"),
        ("part.SNA", "Multipart (Disk 1 of 2)"),
    ]);
    assert_eq!(plan.projected_insertions(), 2);
    // A new path appearing after review must never enter the Apply walk.
    std::fs::write(f.root.join("unreviewed.z80"), snapshot(4)).unwrap();
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(r.inserted.len(), 2);
    let rows = f.rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows.iter().find(|a| a.id == old[0].id), Some(&old[0]));
    assert!(
        !rows
            .iter()
            .any(|a| a.absolute_path.ends_with("unreviewed.z80"))
    );
    assert_eq!(
        std::fs::read(f.root.join("alternate.Z80")).unwrap(),
        snapshot(2)
    );
    assert_eq!(std::fs::read(f.root.join("part.SNA")).unwrap(), sna(3));
    assert!(
        apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {})
            .unwrap_err()
            .contains("records changed")
    );
}
#[test]
fn bounded_excludes_refused_unsupported_metadata_and_ambiguous() {
    let f = Fixture::new();
    std::fs::write(f.root.join("ambiguous.z80"), snapshot(9)).unwrap();
    for name in [
        "broken.z80",
        "broken.sna",
        "broken.trd",
        "broken.scl",
        "unsupported.sp",
        "notes.txt",
    ] {
        std::fs::write(f.root.join(name), [255; 16]).unwrap();
    }
    let plan = f.plan(&[
        ("ambiguous.z80", "First identity"),
        ("ambiguous.z80", "Second identity"),
    ]);
    assert_eq!(plan.candidate_count(), 1);
    assert_eq!(plan.projected_insertions(), 0);
    assert_eq!(plan.exclusions().len(), 6);
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert!(r.inserted.is_empty());
    assert_eq!(r.skipped.len(), 7);
    assert!(r.skipped.iter().any(|(_, r)| r.contains("Ambiguous")));
    assert_eq!(f.rows().len(), 1);
}
#[test]
fn bounded_skips_changed_missing_unreadable_and_symlink_candidates() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let files = [
        "changed.z80",
        "missing.z80",
        "protected.z80",
        "unsafe.z80",
        "good.z80",
    ];
    for (n, name) in files.iter().enumerate() {
        std::fs::write(f.root.join(name), snapshot(n as u8 + 2)).unwrap();
    }
    let names: Vec<_> = files.iter().map(|s| (*s, *s)).collect();
    let plan = f.plan(&names);
    std::fs::write(f.root.join(files[0]), snapshot(99)).unwrap();
    std::fs::remove_file(f.root.join(files[1])).unwrap();
    std::fs::set_permissions(f.root.join(files[2]), std::fs::Permissions::from_mode(0)).unwrap();
    std::fs::remove_file(f.root.join(files[3])).unwrap();
    symlink(f.root.join(files[4]), f.root.join(files[3])).unwrap();
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(r.inserted.len(), 1);
    assert_eq!(r.skipped.len(), 4);
    assert!(r.inserted[0].0.ends_with("good.z80"));
    assert_eq!(f.rows().len(), 2);
}
#[test]
fn bounded_cancel_and_database_error_roll_back_entire_batch() {
    for inject_error in [false, true] {
        let f = Fixture::new();
        std::fs::write(f.root.join("a.z80"), snapshot(2)).unwrap();
        std::fs::write(f.root.join("b.z80"), snapshot(3)).unwrap();
        let plan = f.plan(&[("a.z80", "A"), ("b.z80", "B")]);
        let before = f.rows();
        if inject_error {
            let sql = rusqlite::Connection::open(&f.db).unwrap();
            sql.execute_batch("CREATE TRIGGER synthetic_refuse_second BEFORE INSERT ON archives WHEN cast(NEW.file_name_cached AS TEXT)='b.z80' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
        }
        let cancel = AtomicBool::new(false);
        let result = apply_bounded_import(&plan, &cancel, &mut |n, total| {
            if !inject_error && n == total {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(result.is_err());
        assert_eq!(f.rows(), before);
    }
}
#[test]
fn bounded_refuses_stale_catalogue_alias_configuration_and_replaced_storage() {
    for change in ["catalogue", "alias", "config", "storage", "database"] {
        let f = Fixture::new();
        std::fs::write(f.root.join("a.z80"), snapshot(2)).unwrap();
        let plan = f.plan(&[("a.z80", "A")]);
        let before = f.rows();
        match change {
            "catalogue" => std::fs::write(&f.dat, "changed").unwrap(),
            "alias" => {
                Database::open_or_create(&f.db)
                    .unwrap()
                    .add_platform_alias("zxs", "ZX Spectrum")
                    .unwrap();
            }
            "config" => {
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&f.cfg)
                    .unwrap()
                    .write_all(b"\n# changed after review\n")
                    .unwrap();
            }
            "storage" => {
                std::fs::rename(&f.parent, f.parent.with_extension("old")).unwrap();
                std::fs::create_dir_all(&f.root).unwrap();
            }
            "database" => {
                let replacement = f.db.with_extension("replacement");
                Database::open_or_create(&replacement).unwrap();
                std::fs::rename(&replacement, &f.db).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).is_err(),
            "{change}"
        );
        if change != "database" {
            assert_eq!(f.rows(), before);
        }
    }
}
#[test]
fn bounded_new_preview_on_replacement_storage_still_requires_reviewed_rebind() {
    let f = Fixture::new();
    std::fs::rename(&f.parent, f.parent.with_extension("old")).unwrap();
    std::fs::create_dir_all(&f.root).unwrap();
    std::fs::write(f.root.join("a.z80"), snapshot(2)).unwrap();
    let cat = f.catalogue(&[("a.z80", "A")]);
    let d = Database::open_read_only(&f.db).unwrap();
    let context = begin_bounded_preview(&d, &f.cfg).unwrap();
    let p = preview_media(
        &config(&f.root),
        &[],
        &d.load_archives().unwrap(),
        &[cat],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    assert!(
        prepare_bounded_import(&p, context)
            .unwrap_err()
            .contains("binding")
    );
}
#[test]
fn bounded_presentation_edits_cannot_expand_allowlist() {
    let f = Fixture::new();
    std::fs::write(f.root.join("a.z80"), snapshot(2)).unwrap();
    std::fs::write(f.root.join("unsupported.sp"), snapshot(3)).unwrap();
    let cat = f.catalogue(&[("a.z80", "A")]);
    let d = Database::open_read_only(&f.db).unwrap();
    let context = begin_bounded_preview(&d, &f.cfg).unwrap();
    let mut p = preview_media(
        &config(&f.root),
        &[],
        &d.load_archives().unwrap(),
        &[cat],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    let original = prepare_bounded_import(&p, context.clone()).unwrap();
    let original_approval = original.approval_sha256();
    let original_exclusions = original.exclusions_sha256();
    for i in &mut p.items {
        i.disposition = MediaDisposition::Eligible;
        i.path = f.root.join("unsupported.sp");
    }
    p.roots = vec![f.parent.clone()];
    let plan = prepare_bounded_import(&p, context).unwrap();
    assert_eq!(plan.approval_sha256(), original_approval);
    assert_eq!(plan.exclusions_sha256(), original_exclusions);
    assert_eq!(original_approval.len(), 64);
    assert_eq!(original_exclusions.len(), 64);
    assert_ne!(original_approval, original_exclusions);
    assert_eq!(plan.candidate_count(), 1);
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(r.inserted.len(), 1);
    assert!(r.inserted[0].0.ends_with("a.z80"));
}
#[test]
fn bounded_archive_member_copy_is_never_in_allowlist() {
    use std::io::Write;
    let f = Fixture::new();
    let bytes = snapshot(8);
    let mut z = zip::ZipWriter::new(std::fs::File::create(f.root.join("indexed.zip")).unwrap());
    z.start_file("renamed.z80", zip::write::SimpleFileOptions::default())
        .unwrap();
    z.write_all(&bytes).unwrap();
    z.finish().unwrap();
    let mut db = Database::open_or_create(&f.db).unwrap();
    archivefs_core::scan_and_persist(&mut db, &config(&f.parent), "synthetic-archive").unwrap();
    drop(db);
    std::fs::write(f.root.join("copy.z80"), &bytes).unwrap();
    std::fs::write(f.root.join("new.z80"), snapshot(7)).unwrap();
    let plan = f.plan(&[("copy.z80", "Represented"), ("new.z80", "New")]);
    assert_eq!(plan.candidate_count(), 1);
    assert_eq!(plan.exclusions().len(), 1);
    assert!(plan.exclusions()[0].1.contains("SHA-256"));
    let result = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(result.inserted.len(), 1);
    assert!(result.inserted[0].0.ends_with("new.z80"));
    assert!(
        !f.rows()
            .iter()
            .any(|r| r.absolute_path.ends_with("copy.z80"))
    );
}
#[test]
fn bounded_saved_scope_is_captured_before_preview() {
    let f = Fixture::new();
    std::fs::write(f.root.join("new.z80"), snapshot(2)).unwrap();
    let cat = f.catalogue(&[("new.z80", "New")]);
    let d = Database::open_read_only(&f.db).unwrap();
    let context = begin_bounded_preview(&d, &f.cfg).unwrap();
    let p = preview_media(
        &config(&f.root),
        &[],
        &d.load_archives().unwrap(),
        &[cat],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    Database::open_or_create(&f.db)
        .unwrap()
        .add_platform_alias("zxs", "ZX Spectrum")
        .unwrap();
    assert!(
        prepare_bounded_import(&p, context)
            .unwrap_err()
            .contains("changed")
    );
}
#[test]
fn bounded_parent_source_swap_before_commit_rolls_back_even_when_child_inode_survives() {
    let f = Fixture::new();
    std::fs::write(f.root.join("new.z80"), snapshot(2)).unwrap();
    let plan = f.plan(&[("new.z80", "New")]);
    let before = f.rows();
    let old = f.parent.with_extension("old");
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |n, total| {
        if n == total {
            std::fs::rename(&f.parent, &old).unwrap();
            std::fs::create_dir(&f.parent).unwrap();
            std::fs::rename(old.join("zxs"), &f.root).unwrap();
        }
    });
    assert!(r.unwrap_err().contains("parent source changed"));
    assert_eq!(f.rows(), before);
}
#[test]
fn bounded_disks_and_szx_are_inserted_without_conversion() {
    let f = Fixture::new();
    let mut trd = vec![0; 655360];
    trd[0x8e2] = 1;
    trd[0x8e3] = 0x16;
    trd[0x8e5..0x8e7].copy_from_slice(&2544u16.to_le_bytes());
    trd[0x8e7] = 0x10;
    let mut scl = b"SINCLAIR\x01".to_vec();
    scl.extend_from_slice(&[0; 14]);
    scl[22] = 1;
    scl.extend_from_slice(&[0; 256]);
    scl.extend_from_slice(&[0; 4]);
    let fixtures = [
        ("disk.TRD", trd),
        ("disk.SCL", scl),
        ("state.SZX", b"ZXST\x01\x04\x01\0CRTR\0\0\0\0".to_vec()),
    ];
    for (n, b) in &fixtures {
        std::fs::write(f.root.join(n), b).unwrap();
    }
    let plan = f.plan(&[
        ("disk.TRD", "TRD game"),
        ("disk.SCL", "SCL game"),
        ("state.SZX", "SZX game"),
    ]);
    assert_eq!(plan.projected_insertions(), 3);
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(r.inserted.len(), 3);
    for (n, b) in fixtures {
        assert_eq!(std::fs::read(f.root.join(n)).unwrap(), b);
    }
}
#[test]
fn bounded_preserves_manual_platform_dat_blob_and_saved_scan_history() {
    let f = Fixture::new();
    let id = f.rows()[0].id;
    Database::open_or_create(&f.db)
        .unwrap()
        .set_manual_platform(id, "GameCube")
        .unwrap();
    let sql = rusqlite::Connection::open(&f.db).unwrap();
    sql.execute("INSERT INTO library_dat_identities(archive_id,dat_source_id,verification_state,completeness,audited_at,facts_json,created_at,updated_at) VALUES(?1,'preservation-fixture','Verified','complete','fixture',?2,'fixture','fixture')",rusqlite::params![id,b"opaque-existing-evidence".as_slice()]).unwrap();
    let before = f.rows();
    let scans: i64 = sql
        .query_row("SELECT count(*) FROM scan_runs", [], |r| r.get(0))
        .unwrap();
    std::fs::write(f.root.join("new.z80"), snapshot(2)).unwrap();
    let plan = f.plan(&[("new.z80", "New")]);
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(r.inserted.len(), 1);
    assert_eq!(f.rows().iter().find(|r| r.id == id), Some(&before[0]));
    let blob: Vec<u8> = sql
        .query_row(
            "SELECT facts_json FROM library_dat_identities WHERE archive_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(blob, b"opaque-existing-evidence");
    assert_eq!(
        sql.query_row::<i64, _, _>("SELECT count(*) FROM library_dat_identities", [], |r| r
            .get(0))
            .unwrap(),
        1
    );
    assert_eq!(
        sql.query_row::<i64, _, _>("SELECT count(*) FROM scan_runs", [], |r| r.get(0))
            .unwrap(),
        scans
    );
}
#[test]
fn bounded_subfolder_uses_configured_parent_for_existing_custom_alias_precedence() {
    let f = Fixture::new();
    let mut db = Database::open_or_create(&f.db).unwrap();
    db.add_platform_alias("zxs", "ZX Spectrum").unwrap();
    drop(db);
    std::fs::write(f.root.join("new.sna"), sna(2)).unwrap();
    let plan = f.plan(&[("new.sna", "New")]);
    let r = apply_bounded_import(&plan, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
    assert_eq!(r.inserted.len(), 1);
    let expected_db = f.db.with_extension("expected.sqlite3");
    let mut normal = Database::open_or_create(expected_db).unwrap();
    normal.add_platform_alias("zxs", "ZX Spectrum").unwrap();
    archivefs_core::scan_and_persist(&mut normal, &config(&f.parent), "synthetic-comparison")
        .unwrap();
    let expected = normal
        .load_archives()
        .unwrap()
        .into_iter()
        .find(|a| a.absolute_path.ends_with("new.sna"))
        .unwrap();
    let actual = f
        .rows()
        .into_iter()
        .find(|a| a.absolute_path.ends_with("new.sna"))
        .unwrap();
    assert_eq!(actual.platform, expected.platform);
    assert_eq!(actual.platform_source, expected.platform_source);
    assert_eq!(actual.source_folder_id, f.rows()[0].source_folder_id);
}
