//! Disposable Job #37 collection, using the supported scan and DAT-audit APIs.
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::sources::audit_cache::AuditCacheConfig;
use archivefs_core::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use archivefs_core::dat::sources::{
    DatSourceEntry, DatSourceKind, DatSourceRegistry, load_dat_sources_config_from,
    save_dat_sources_config_to, validate_dat_source,
};
use archivefs_core::identity_source::hashing::hash_file_reporting;
use archivefs_core::safe_read::TrustedRoots;
use archivefs_core::verified_game_fixture::{VerifiedGameFixture, synthetic_rom};
use archivefs_core::{Database, scan_source_folder_at};
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("supply a fresh absolute root")?,
    );
    if !root.is_absolute() {
        return Err("root must be absolute".into());
    }
    for (key, value) in VerifiedGameFixture::environment_for(&root) {
        // SAFETY: before threads and before any production call.
        unsafe { std::env::set_var(key, value) };
    }
    if std::env::args().any(|argument| argument == "--validate-only" || argument == "--refresh") {
        if fs::read_to_string(root.join(".job37-disposable"))? != "job37-synthetic-only\n" {
            return Err("not an owned Job 37 collection".into());
        }
        let path = root.join("config/dat_sources.toml");
        let (mut registry, _) =
            DatSourceRegistry::from_config(&load_dat_sources_config_from(&path)?);
        let ids: Vec<_> = registry
            .entries()
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        for id in ids {
            let entry = registry.get_mut(&id).ok_or("missing fixture catalogue")?;
            let (report, _) = validate_dat_source(entry, DatLimits::default());
            entry.health = report.to_health(&entry.path, entry.kind);
        }
        save_dat_sources_config_to(path, &registry.to_config())?;
        if std::env::args().any(|argument| argument == "--refresh") {
            scan_source_folder_at(
                &root.join("config/config.toml"),
                &root.join("data/library.sqlite3"),
                &root.join("games"),
                "job37-refresh",
            )?;
        }
        return Ok(());
    }
    let fixture = VerifiedGameFixture::build_at(&root)?;
    fs::write(root.join(".job37-disposable"), "job37-synthetic-only\n")?;
    let variants = [
        ("Aurora Quest (Europe) (Rev 1)", None),
        (
            "Aurora Quest (USA) (Rev 1)",
            Some("Aurora Quest (Europe) (Rev 1)"),
        ),
        (
            "Aurora Quest (USA) (Rev 2)",
            Some("Aurora Quest (Europe) (Rev 1)"),
        ),
        (
            "Aurora Quest (Japan)",
            Some("Aurora Quest (Europe) (Rev 1)"),
        ),
        ("Branch Adventure (USA)", None),
        (
            "Branch Adventure Special (USA)",
            Some("Branch Adventure (USA)"),
        ),
        ("Disconnected Game (USA)", None),
    ];
    let mut dat = String::from(
        "<datafile><header><name>Nintendo - Game Boy Advance</name><homepage>No-Intro</homepage></header>",
    );
    for (index, (title, parent)) in variants.iter().enumerate() {
        let filename = format!("{title}.gba");
        let path = fixture.games_dir.join(&filename);
        fs::write(&path, synthetic_rom(100 + index as u32))?;
        let hash = hash_file_reporting(&path, &TrustedRoots::none(), None, &|_| {})
            .map_err(|e| format!("{e:?}"))?;
        let relationship = parent
            .map(|p| format!(" cloneof=\"{p}\""))
            .unwrap_or_default();
        dat.push_str(&format!("<game name=\"{title}\"{relationship}><rom name=\"{filename}\" size=\"{}\" sha1=\"{}\"/></game>", hash.bytes_hashed, hash.sha1));
    }
    dat.push_str("</datafile>");
    let dat_path = root.join("catalogue/job37-gba.dat");
    fs::write(&dat_path, dat)?;
    // Filename similarity alone must never promote this to verified.
    fs::write(
        fixture.games_dir.join("Aurora Quest Unknown Dump.gba"),
        synthetic_rom(999),
    )?;
    fs::create_dir(fixture.games_dir.join("gb"))?;
    fs::write(
        fixture.games_dir.join("gb/Unverified Pocket.gb"),
        vec![0x42; 32768],
    )?;
    fs::create_dir(fixture.games_dir.join("psx"))?;
    for disc in 1..=2 {
        fs::write(
            fixture
                .games_dir
                .join(format!("psx/Orbit Story (USA) (Disc {disc} of 2).bin")),
            vec![disc; 2352 * 20],
        )?;
        fs::write(
            fixture
                .games_dir
                .join(format!("psx/Orbit Story (USA) (Disc {disc} of 2).cue")),
            format!(
                "FILE \"Orbit Story (USA) (Disc {disc} of 2).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n"
            ),
        )?;
    }
    fs::write(
        fixture.games_dir.join("psx/Orbit Story (USA).m3u"),
        "Orbit Story (USA) (Disc 1 of 2).cue\nOrbit Story (USA) (Disc 2 of 2).cue\n",
    )?;
    fs::write(
        fixture.games_dir.join("psx/Missing Companion.cue"),
        "FILE \"absent.bin\" BINARY\n TRACK 01 MODE2/2352\n INDEX 01 00:00:00\n",
    )?;
    let optical_dat = root.join("catalogue/job37-psx.dat");
    let mut optical = String::from(
        "<datafile><header><name>Sony - PlayStation</name><homepage>redump.org</homepage></header>",
    );
    for disc in 1..=2 {
        let title = format!("Orbit Story (USA) (Disc {disc} of 2)");
        let filename = format!("{title}.bin");
        let hash = hash_file_reporting(
            &fixture.games_dir.join("psx").join(&filename),
            &TrustedRoots::none(),
            None,
            &|_| {},
        )
        .map_err(|e| format!("{e:?}"))?;
        optical.push_str(&format!(
            "<game name=\"{title}\"><rom name=\"{filename}\" size=\"{}\" sha1=\"{}\"/></game>",
            hash.bytes_hashed, hash.sha1
        ));
    }
    optical.push_str("</datafile>");
    fs::write(&optical_dat, optical)?;
    scan_source_folder_at(
        &fixture.config_path,
        &fixture.database_path,
        &fixture.games_dir,
        "job37",
    )?;
    let mut registry = DatSourceRegistry::new();
    for (id, name, path) in [
        (
            "emuwiz-fixture-no-intro",
            "Fixture No-Intro",
            fixture.dat_path.clone(),
        ),
        ("job37", "Job 37 GBA", dat_path.clone()),
        ("job37-psx", "Job 37 PlayStation", optical_dat.clone()),
    ] {
        let mut entry = DatSourceEntry::new(id.into(), name.into(), path, DatSourceKind::File);
        let (report, _) = validate_dat_source(&entry, DatLimits::default());
        entry.health = report.to_health(&entry.path, entry.kind);
        registry.add(entry)?;
    }
    save_dat_sources_config_to(
        fixture.config_dir.join("dat_sources.toml"),
        &registry.to_config(),
    )?;
    for (source_id, source_display_name, dat_path) in [
        ("job37", "Job 37 GBA", dat_path),
        ("job37-psx", "Job 37 PlayStation", optical_dat),
    ] {
        let audit = run_dat_audit_with_cache(
            &DatAuditRequest {
                source_id: source_id.into(),
                source_display_name: source_display_name.into(),
                dat_path,
                dat_kind: DatSourceKind::File,
                scan_root: fixture.games_dir.clone(),
                limits: DatLimits::default(),
                policy: None,
                platform: None,
            },
            &TrustedRoots::none(),
            &AtomicBool::new(false),
            &|_| {},
            AuditCacheConfig::Disabled,
        )?;
        let mut db = Database::open_or_create(&fixture.database_path)?;
        db.persist_library_dat_identities_from_audit(&audit)?;
        db.close()?;
    }
    // Simulate a disconnected file only after supported discovery recorded it.
    fs::remove_file(fixture.games_dir.join("Disconnected Game (USA).gba"))?;
    scan_source_folder_at(
        &fixture.config_path,
        &fixture.database_path,
        &fixture.games_dir,
        "job37-missing-file",
    )?;
    let lines: Vec<_> = fixture
        .child_environment()
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    fs::write(root.join("profile.env"), lines.join("\n"))?;
    fs::write(
        fixture.config_dir.join("gui-v2.json"),
        "{\"route\":\"BrowsePlay\",\"welcome_dismissed\":true}",
    )?;
    println!("{}", root.display());
    Ok(())
}
