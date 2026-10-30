//! The existing Blue Play action and production discovery/preflight, in child
//! processes with disposable profiles and recording emulators only.
use super::*;
use std::os::unix::fs::PermissionsExt;

const CHILD: &str = "gui_v2::retroarch_journey_tests::cartridge_cue::child_entry";
const KIND: &str = "EMUWIZ_THREE_LAUNCH_KIND";

fn gameboy_rom() -> Vec<u8> {
    let mut bytes = vec![0; 32768];
    bytes[0x100..0x104].copy_from_slice(&[0, 0xc3, 0x50, 1]);
    let logo = [
        0xce, 0xed, 0x66, 0x66, 0xcc, 0x0d, 0, 0x0b, 3, 0x73, 0, 0x83, 0, 0x0c, 0, 0x0d, 0, 8,
        0x11, 0x1f, 0x88, 0x89, 0, 0x0e, 0xdc, 0xcc, 0x6e, 0xe6, 0xdd, 0xdd, 0xd9, 0x99, 0xbb,
        0xbb, 0x67, 0x63, 0x6e, 0x0e, 0xec, 0xcc, 0xdd, 0xdc, 0x99, 0x9f, 0xbb, 0xb9, 0x33, 0x3e,
    ];
    bytes[0x104..0x134].copy_from_slice(&logo);
    bytes[0x134..0x140].copy_from_slice(b"EMUWIZ LOOP ");
    bytes[0x14d] = archivefs_core::gb_header_evidence::compute_header_checksum(&bytes).unwrap();
    bytes[0x150..0x152].copy_from_slice(&[0x18, 0xfe]);
    bytes
}

fn fixture(root: &Path, kind: &str) -> VerifiedGameFixture {
    let (name, bytes, platform, companions) = match kind {
        "gb" => ("Synthetic Pocket (USA).gb", gameboy_rom(), "Nintendo - Game Boy", Vec::new()),
        // A synthetic idle image, not a commercial game or a known-good dump.
        "vb" => ("Synthetic Virtual (USA).vb", [0, 0x68].repeat(256 * 1024), "Nintendo - Virtual Boy", Vec::new()),
        // A Super Magic Drive dump of the synthetic Mega Drive cartridge: an
        // extension the scanner re-inspects on every pass, so a scan that
        // cannot read the file is observed rather than served from cache.
        "smd" => ("Synthetic Drive (USA).smd", archivefs_core::synthetic_media::megadrive_smd(), "Sega - Mega Drive - Genesis", Vec::new()),
        "cue" => ("Synthetic Disc.cue", b"FILE \"Track 01 ; $.bin\" BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nFILE \"Track 02 audio.bin\" BINARY\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n".to_vec(), "Sony - PlayStation", vec![("Track 01 ; $.bin", ps1_raw_bin_bytes(&ps1_iso_bytes())), ("Track 02 audio.bin", vec![0; 2352 * 8])]),
        _ => panic!("unknown journey"),
    };
    let borrowed: Vec<_> = companions
        .iter()
        .map(|(name, data)| (*name, data.as_slice()))
        .collect();
    let f =
        VerifiedGameFixture::build_at_with_game(root, name, &bytes, platform, &borrowed).unwrap();
    let fake = f.fake_retroarch("mGBA").unwrap();
    // Replace only this fixture's generated GBA core with the actual route's
    // core identity and extensions. No installed emulator is discovered.
    if kind != "cue" {
        fs::remove_file(&fake.core).unwrap();
        fs::remove_file(fake.root.join("info/mgba.info")).unwrap();
        let (stem, library, system, extensions) = match kind {
            "gb" => ("gambatte", "Gambatte", "Nintendo - Game Boy", "gb|gbc"),
            "smd" => (
                "genesis_plus_gx",
                "Genesis Plus GX",
                "Sega - Mega Drive - Genesis",
                "md|smd|gen|bin",
            ),
            _ => (
                "mednafen_vb",
                "Beetle VB",
                "Nintendo - Virtual Boy",
                "vb|vboy",
            ),
        };
        let core = fake.cores_dir.join(format!("{stem}_libretro.so"));
        let source = core.with_extension("c");
        fs::write(&source, format!("struct info {{ const char *n,*v,*e; _Bool a,b; }}; void retro_get_system_info(struct info *i) {{ i->n=\"{library}\"; i->v=\"1\"; i->e=\"{extensions}\"; }}\n")).unwrap();
        assert!(
            Command::new("cc")
                .args(["-shared", "-fPIC", "-o"])
                .arg(&core)
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        fs::write(
            fake.root.join(format!("info/{stem}.info")),
            format!("systemname = \"{system}\"\nsupported_extensions = \"{extensions}\"\n"),
        )
        .unwrap();
    } else {
        let profile = fake.config_dir().join("duckstation");
        fs::create_dir_all(profile.join("bios")).unwrap();
        fs::write(profile.join("bios/synthetic.bin"), b"abc").unwrap();
        fs::write(profile.join("settings.ini"), "[Main]\nSetupWizardIncomplete = false\n[BIOS]\nSearchDirectory = bios\nBIOSFilename = synthetic.bin\n").unwrap();
        let bios = root.join("catalogue/synthetic-ps1-bios.dat");
        fs::write(&bios, "<datafile><header><name>Sony - PlayStation - BIOS Images</name><description>synthetic test catalogue, not real firmware</description><author>Redump</author><homepage>redump.org</homepage></header><game name=\"Synthetic BIOS\"><rom name=\"synthetic.bin\" size=\"3\" crc=\"352441c2\" md5=\"900150983cd24fb0d6963f7d28e17f72\" sha1=\"a9993e364706816aba3e25717850c26c9cd0d89d\"/></game></datafile>").unwrap();
        use archivefs_core::dat::sources::*;
        let path = f.config_dir.join("dat_sources.toml");
        let config = load_dat_sources_config_from(&path).unwrap();
        let (mut registry, problems) = DatSourceRegistry::from_config(&config);
        assert!(problems.is_empty());
        registry
            .add(DatSourceEntry::new(
                "synthetic-ps1-bios".into(),
                "Synthetic PS1 BIOS fixture".into(),
                bios,
                DatSourceKind::File,
            ))
            .unwrap();
        save_dat_sources_config_to(path, &registry.to_config()).unwrap();
    }
    let executable = fake.executable.parent().unwrap().join(if kind == "cue" {
        "duckstation-qt"
    } else {
        "retroarch"
    });
    fs::write(&executable, format!("#!/bin/sh\nrec=\"{root}/launch-record\"\n/bin/mkdir -p \"$rec\"\nprintf '%s\\n' \"$@\" > \"$rec/args.txt\"\nprintf '%s\\n' \"$$\" > \"$rec/pid\"\n/bin/sleep 1\ncode=0\nif [ -f \"{root}/exit-code\" ]; then read code < \"{root}/exit-code\"; fi\nif [ \"$code\" != 0 ]; then printf 'synthetic emulator failure\\n' >&2; fi\nexit \"$code\"\n", root=root.display())).unwrap();
    archivefs_core::verified_game_fixture::compile_native_test_executable(
        &executable,
        &fs::read_to_string(&executable).unwrap(),
    )
    .unwrap();
    f
}

fn run(kind: &str) {
    let owned = tempfile::tempdir().unwrap();
    let exported = std::env::var_os("EMUWIZ_THREE_LAUNCH_EXPORT").map(PathBuf::from);
    let base = exported
        .unwrap_or_else(|| owned.path().to_path_buf())
        .join(kind);
    let scenarios: &[&str] = if kind == "cue" {
        &[
            "plain",
            "failure",
            "stale",
            "missing",
            "ambiguous",
            "unsafe",
            "ordering",
        ]
    } else if kind == "smd" {
        &["plain", "stale-inspection"]
    } else {
        &["plain", "failure", "changed", "missing-core"]
    };
    let scenarios = if std::env::var_os("EMUWIZ_THREE_LAUNCH_BASELINE").is_some() {
        &["plain"][..]
    } else {
        scenarios
    };
    for scenario in scenarios {
        let root = base.join(scenario);
        let f = fixture(&root, kind);
        fs::write(root.join("retroarch/saves/preserved.srm"), b"existing save").unwrap();
        fs::write(
            root.join("retroarch/states/preserved.state"),
            b"existing state",
        )
        .unwrap();
        let config = root.join("retroarch/config");
        let bin = root.join("retroarch/bin");
        let before: Vec<_> = fs::read_dir(&f.games_dir)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                (p.clone(), fs::read(p).unwrap())
            })
            .collect();
        let protected = [
            root.join("retroarch/saves/preserved.srm"),
            root.join("retroarch/states/preserved.state"),
            root.join("retroarch/config/retroarch/retroarch.cfg"),
        ];
        let protected_before: Vec<_> = protected
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--nocapture", "--test-threads=1"])
            .env_clear()
            .envs(f.child_environment())
            .env("EMUWIZ_TEST_REAL_ENVIRONMENT", "1")
            // The full Blue fixture needs a normal application-sized stack;
            // libtest's child test thread defaults to only 2 MiB.
            .env("RUST_MIN_STACK", "8388608")
            .env("XDG_CONFIG_HOME", &config)
            .env("PATH", &bin)
            .env(ROOT, &root)
            .env(KIND, kind)
            .env(SCENARIO, scenario)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{kind}/{scenario}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{}", String::from_utf8_lossy(&output.stdout));
        for (path, bytes) in protected.iter().zip(protected_before) {
            assert_eq!(
                fs::read(path).unwrap(),
                bytes,
                "protected user file changed: {}",
                path.display()
            );
        }

        for (path, bytes) in &before {
            assert_eq!(
                fs::read(path).unwrap(),
                *bytes,
                "original changed: {}",
                path.display()
            );
        }
    }
}

#[test]
fn game_boy_blue_play_journey() {
    run("gb");
}
#[test]
fn virtual_boy_blue_play_journey() {
    run("vb");
}
#[test]
fn cue_bin_blue_play_journey() {
    run("cue");
}
/// A scan that cannot finish reading the cartridge withdraws Play in Blue
/// (the row keeps its platform and history, and says to rescan); a completed
/// rescan restores Play through the same readiness path.
#[test]
fn mega_drive_blue_play_journey_withdraws_readiness_after_an_incomplete_inspection() {
    run("smd");
}

/// Partial read -> stale evidence -> Play withdrawn -> completed retry ->
/// Play restored, through the real Blue Launch page. The fault is a file the
/// scan cannot open; the file itself is never changed.
fn stale_inspection_journey(f: &VerifiedGameFixture, root: &Path) {
    let row = || {
        archivefs_core::Database::open_read_only(&f.database_path)
            .unwrap()
            .load_archives()
            .unwrap()
            .into_iter()
            .find(|row| row.absolute_path == f.rom_path)
            .expect("the cartridge is catalogued")
    };
    let scan = |reason: &str| {
        archivefs_core::scan_source_folder_at(
            &f.config_path,
            &f.database_path,
            &f.games_dir,
            reason,
        )
        .unwrap()
    };
    let before = row();
    assert_eq!(
        before.platform_source.as_deref(),
        Some("registry_detector"),
        "{before:?}"
    );
    assert_ne!(
        before.last_known_health,
        archivefs_core::ArchiveHealth::RetryAvailable.to_string()
    );

    // Play is offered while the evidence is current.
    let mut blue = Blue::open_game(f);
    blue.wait_for_readiness();
    blue.guard_executables(root);
    drop(blue);

    // A scan that cannot read the file.
    fs::set_permissions(&f.rom_path, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&f.rom_path).is_ok() {
        // root can read anything; the refusal cannot be staged.
        fs::set_permissions(&f.rom_path, fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }
    let summary = scan("journey-fault");
    fs::set_permissions(&f.rom_path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        summary
            .platform_assignment_warnings
            .iter()
            .any(|(path, text)| path == &f.rom_path && text.contains("inspection incomplete")),
        "{:?}",
        summary.platform_assignment_warnings
    );
    let stale = row();
    assert_eq!(
        stale.last_known_health,
        archivefs_core::ArchiveHealth::RetryAvailable.to_string()
    );
    assert_eq!(
        stale.platform, before.platform,
        "the assignment is kept as history"
    );
    assert_eq!(stale.platform_source, before.platform_source);
    assert_eq!(stale.size_bytes, before.size_bytes);
    assert_eq!(
        stale.modified_time_unix_seconds,
        before.modified_time_unix_seconds
    );

    // The file reads fine again, but the catalogue's last inspection did not
    // finish: Blue must not offer Play from that evidence, and must say why
    // and what to do.
    let mut blue = Blue::open_game(f);
    blue.wait_until(|b| b.shown("Rescan to finish checking"), 25);
    assert!(
        !blue.saw(PLAY),
        "stale evidence authorised Play: {:?}",
        blue.seen
    );
    assert!(
        !blue
            .seen
            .iter()
            .any(|s| s.to_lowercase().contains("corrupt")),
        "{:?}",
        blue.seen
    );
    drop(blue);

    // The retry completes.
    let summary = scan("journey-retry");
    assert!(
        summary.platform_assignment_warnings.is_empty(),
        "{:?}",
        summary.platform_assignment_warnings
    );
    let restored = row();
    assert_ne!(
        restored.last_known_health,
        archivefs_core::ArchiveHealth::RetryAvailable.to_string()
    );
    assert_eq!(restored.platform, before.platform);
    let mut blue = Blue::open_game(f);
    blue.wait_for_readiness();
    blue.guard_executables(root);
    assert!(!blue.shown("Rescan to finish checking"), "{:?}", blue.last);
}

#[test]
fn child_entry() {
    let Some(root) = std::env::var_os(KIND).and_then(|_| std::env::var_os(ROOT)) else {
        return;
    };
    let root = PathBuf::from(root);
    let kind = std::env::var(KIND).unwrap();
    let scenario = std::env::var(SCENARIO).unwrap();
    let mut f = VerifiedGameFixture::reopen(&root).unwrap();
    f.games_dir = root.join(match kind.as_str() {
        "cue" => "games/psx",
        "gb" => "games/gb",
        "smd" => "games",
        _ => "games/vb",
    });
    f.rom_path = f.games_dir.join(match kind.as_str() {
        "gb" => "Synthetic Pocket (USA).gb",
        "vb" => "Synthetic Virtual (USA).vb",
        "smd" => "Synthetic Drive (USA).smd",
        _ => "Synthetic Disc.cue",
    });
    if scenario == "stale-inspection" {
        stale_inspection_journey(&f, &root);
        return;
    }
    let record = root.join("launch-record");
    if record.exists() {
        fs::remove_dir_all(&record).unwrap();
    }
    if scenario == "failure" {
        fs::write(root.join("exit-code"), "7\n").unwrap();
    } else if root.join("exit-code").exists() {
        fs::remove_file(root.join("exit-code")).unwrap();
    }
    let mut blue = if kind == "cue" {
        let report = archivefs_core::game_identity::inspect_catalogued_game_identity(
            &f.rom_path,
            Some("PSX"),
        );
        assert!(matches!(
            archivefs_core::launch::canonical_identity_from_game_report(&report).0,
            archivefs_core::launch::CanonicalIdentityStatus::Resolved(_)
        ));
        Blue::open_game(&f)
    } else {
        Blue::open(&f)
    };
    let play = if kind == "cue" {
        "Launch duckstation"
    } else {
        PLAY
    };
    blue.wait_until(|b| b.shown(play), 25);
    if kind != "cue" {
        blue.guard_executables(&root);
    }
    let mut restore = None;
    match scenario.as_str() {
        "changed" => {
            let old = fs::read(&f.rom_path).unwrap();
            let mut changed = old.clone();
            changed[1024] ^= 1;
            fs::write(&f.rom_path, changed).unwrap();
            restore = Some((f.rom_path.clone(), old));
        }
        "missing-core" => {
            let core = root.join(format!("retroarch/cores/{}_libretro.so", core_stem(&kind)));
            let old = fs::read(&core).unwrap();
            fs::remove_file(&core).unwrap();
            restore = Some((core, old));
        }
        "missing" => {
            let path = f.games_dir.join("Track 02 audio.bin");
            let old = fs::read(&path).unwrap();
            fs::remove_file(&path).unwrap();
            restore = Some((path, old));
        }
        "ambiguous" => {
            fs::copy(&f.rom_path, f.games_dir.join("Second.cue")).unwrap();
        }
        "unsafe" => {
            let path = f.games_dir.join("Track 02 audio.bin");
            let old = fs::read(&path).unwrap();
            fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(f.games_dir.join("Track 01 ; $.bin"), &path).unwrap();
            restore = Some((path, old));
        }
        "stale" => {
            let path = f.games_dir.join("Track 02 audio.bin");
            let old = fs::read(&path).unwrap();
            let stamp = fs::metadata(&path).unwrap().modified().unwrap();
            let mut changed = old.clone();
            changed[0] ^= 1;
            fs::write(&path, changed).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(stamp))
                .unwrap();
            restore = Some((path, old));
        }
        "ordering" => {
            let old = fs::read(&f.rom_path).unwrap();
            fs::write(
                &f.rom_path,
                String::from_utf8(old.clone())
                    .unwrap()
                    .replace("TRACK 02", "TRACK 01"),
            )
            .unwrap();
            restore = Some((f.rom_path.clone(), old));
        }
        _ => {}
    }
    let refusing = !matches!(scenario.as_str(), "plain" | "failure");
    if kind == "cue" && refusing && scenario != "stale" {
        blue.paint();
        blue.wait_until(|b| !b.shown(play), 10);
    } else {
        blue.click(play);
        if refusing {
            blue.wait_until(
                |b| {
                    b.shown("not started")
                        || b.shown("could not")
                        || b.shown("failed")
                        || record.join("args.txt").exists()
                },
                20,
            );
        } else {
            blue.wait_until(
                |b| {
                    record.join("pid").exists()
                        && b.shown(if kind == "cue" {
                            "DuckStation running"
                        } else {
                            "RetroArch running"
                        })
                },
                20,
            );
            let pid = fs::read_to_string(record.join("pid")).unwrap();
            assert!(
                blue.shown(&format!("PID {}", pid.trim())),
                "{:?}",
                blue.last
            );
            let args = blue.wait_for_exit(&record);
            assert_eq!(Path::new(args.last().unwrap()), f.rom_path);
            if kind != "cue" {
                assert_core(
                    &args,
                    &root.join(format!("retroarch/cores/{}_libretro.so", core_stem(&kind))),
                );
            } else {
                assert_eq!(
                    args,
                    [
                        "-batch".to_string(),
                        "--".to_string(),
                        f.rom_path.display().to_string()
                    ]
                );
            }
            if scenario == "failure" {
                assert!(
                    blue.seen
                        .iter()
                        .any(|s| s.contains("unexpectedly") || s.contains("failed")),
                    "{:?}",
                    blue.seen
                );
            }
        }
    }
    if refusing {
        assert!(!record.join("args.txt").exists());
    }
    if scenario == "ambiguous" {
        fs::remove_file(f.games_dir.join("Second.cue")).unwrap();
    }
    if kind == "cue" && scenario == "stale" {
        blue.click("Recheck CUE/BIN files");
        assert!(
            !record.join("args.txt").exists(),
            "rechecking is not permission to launch"
        );
        blue.wait_until(|b| b.shown(play), 10);
        blue.click(play);
        blue.wait_for_exit(&record);
    }
    if kind == "cue" && scenario == "plain" {
        let first = fs::read_to_string(record.join("pid")).unwrap();
        blue.click("Launch DuckStation again (recheck)");
        blue.wait_until(
            |b| {
                fs::read_to_string(record.join("pid"))
                    .is_ok_and(|pid| !pid.is_empty() && pid != first)
                    && b.shown("DuckStation running")
            },
            20,
        );
        blue.wait_until(
            |b| {
                !b.app
                    .native_workflows
                    .as_ref()
                    .unwrap()
                    .app
                    .launch_standalone
                    .is_active()
            },
            20,
        );
        assert!(blue.shown("DuckStation exited normally"), "{:?}", blue.last);
    }
    if let Some((path, bytes)) = restore {
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            fs::remove_file(&path).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }
    println!(
        "BLUE_LAUNCH {kind}/{scenario}: ok; displayed={:?}",
        blue.seen
    );
}

fn ps1_iso_bytes() -> Vec<u8> {
    const ISO_SECTOR_SIZE: usize = 2_048;
    const SECTORS: usize = 24;

    fn directory_record(name: &[u8], extent: u32, size: u32, directory: bool) -> Vec<u8> {
        let length = 33 + name.len() + usize::from(name.len().is_multiple_of(2));
        let mut record = vec![0_u8; length];
        record[0] = length as u8;
        record[2..6].copy_from_slice(&extent.to_le_bytes());
        record[6..10].copy_from_slice(&extent.to_be_bytes());
        record[10..14].copy_from_slice(&size.to_le_bytes());
        record[14..18].copy_from_slice(&size.to_be_bytes());
        record[25] = if directory { 2 } else { 0 };
        record[28..30].copy_from_slice(&1_u16.to_le_bytes());
        record[30..32].copy_from_slice(&1_u16.to_be_bytes());
        record[32] = name.len() as u8;
        record[33..33 + name.len()].copy_from_slice(name);
        record
    }

    let mut iso = vec![0_u8; SECTORS * ISO_SECTOR_SIZE];
    let pvd = 16 * ISO_SECTOR_SIZE;
    iso[pvd] = 1;
    iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
    iso[pvd + 6] = 1;
    let root = directory_record(&[0], 20, ISO_SECTOR_SIZE as u32, true);
    iso[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);
    let terminator = 17 * ISO_SECTOR_SIZE;
    iso[terminator] = 255;
    iso[terminator + 1..terminator + 6].copy_from_slice(b"CD001");
    iso[terminator + 6] = 1;

    let cnf = b"BOOT = cdrom:\\SLUS_123.45;1\r\nTCB = 4\r\n";
    let root_offset = 20 * ISO_SECTOR_SIZE;
    let cnf_record = directory_record(b"SYSTEM.CNF;1", 21, cnf.len() as u32, false);
    iso[root_offset..root_offset + cnf_record.len()].copy_from_slice(&cnf_record);

    let cnf_offset = 21 * ISO_SECTOR_SIZE;
    iso[cnf_offset..cnf_offset + cnf.len()].copy_from_slice(cnf);
    let executable_name = b"SLUS_123.45;1";
    let executable_record = directory_record(executable_name, 22, 12, false);
    let executable_record_offset = root_offset + cnf_record.len();
    iso[executable_record_offset..executable_record_offset + executable_record.len()]
        .copy_from_slice(&executable_record);
    let executable_offset = 22 * ISO_SECTOR_SIZE;
    iso[executable_offset..executable_offset + 12].copy_from_slice(b"PS-X EXE\0\0\0\0");
    iso
}

fn ps1_raw_bin_bytes(image: &[u8]) -> Vec<u8> {
    use archivefs_core::raw_cd_sector::{
        LOGICAL_BLOCK_BYTES, MODE1_USER_DATA_OFFSET, RAW_SECTOR_BYTES, SYNC_PATTERN,
    };

    image
        .chunks_exact(LOGICAL_BLOCK_BYTES)
        .flat_map(|logical| {
            let mut sector = [0_u8; RAW_SECTOR_BYTES];
            sector[..SYNC_PATTERN.len()].copy_from_slice(&SYNC_PATTERN);
            sector[15] = 1;
            sector[MODE1_USER_DATA_OFFSET..MODE1_USER_DATA_OFFSET + LOGICAL_BLOCK_BYTES]
                .copy_from_slice(logical);
            sector
        })
        .collect()
}

/// The stub core each cartridge journey installs.
fn core_stem(kind: &str) -> &'static str {
    match kind {
        "gb" => "gambatte",
        "smd" => "genesis_plus_gx",
        _ => "mednafen_vb",
    }
}
