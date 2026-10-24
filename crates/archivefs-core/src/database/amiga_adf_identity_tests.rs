//! Real path, synthetic data: a structurally valid ADF in an `amiga` games
//! folder -> `scan_and_persist` -> `run_dat_audit` (which also saves the
//! digest-bound entry list) -> `amiga_adf_launch_identity` -> the ADF launch
//! gate in front of the existing FS-UAE preflight. Nothing touches a real
//! library, emulator or home directory; FS-UAE is a fake script never run.
use super::*;
use crate::dat::amiga_adf_identity::AmigaAdfIdentityRefusal as R;
use crate::dat::catalogue_selection::CatalogueRef;
use crate::dat::configured_catalogues::ConfiguredCatalogue;
use crate::dat::limits::DatLimits;
use crate::dat::sources::DatSourceKind;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{DatAuditRequest, run_dat_audit_with_cache};
use crate::launch::process_spawn::CapturedFileIdentity;
use crate::launch::{
    FsUaeAdfLaunchError, FsUaeAdfLaunchRequest, FsUaeLaunchRequest, preflight_fsuae_adf_launch,
};
use crate::patch_manager::{AmigaEmulatorKind, AmigaProfileDiscoveryRoots};
use crate::safe_read::TrustedRoots;
use sha1::Sha1;
use sha2::Sha256;
use std::sync::atomic::AtomicBool;

const SOURCE: &str = "tosec-amiga";
const VERSION: &str = "2026-10-01";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn p32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A minimal structurally valid flat AmigaDOS floppy (see `amiga_disk`'s
/// own fixture); `volume` makes each disk's bytes distinct.
fn adf(volume: &str) -> Vec<u8> {
    const SECTORS: usize = 1760;
    const ROOT: usize = SECTORS / 2;
    let mut img = vec![0_u8; SECTORS * 512];
    img[..4].copy_from_slice(b"DOS\x00");
    p32(&mut img, 8, ROOT as u32);
    let mut root = [0_u8; 512];
    p32(&mut root, 0, 2);
    p32(&mut root, 12, 72);
    let name = volume.as_bytes();
    let len = name.len().min(30);
    root[0x1B0] = len as u8;
    root[0x1B1..0x1B1 + len].copy_from_slice(&name[..len]);
    p32(&mut root, 508, 1);
    let mut sum = 0_u32;
    for offset in (0..512).step_by(4) {
        if offset != 20 {
            sum = sum.wrapping_add(u32::from_be_bytes(
                root[offset..offset + 4].try_into().unwrap(),
            ));
        }
    }
    p32(&mut root, 20, (sum as i32).wrapping_neg() as u32);
    img[ROOT * 512..(ROOT + 1) * 512].copy_from_slice(&root);
    img
}

struct Lab {
    root: PathBuf,
    db: Database,
    catalogues: HashMap<String, ConfiguredCatalogue>,
}

/// The real TOSEC header of the Amiga Games [ADF] catalogue, as published.
const GAMES_ADF: &str = "Commodore Amiga - Games - [ADF]";
/// A TOSEC catalogue that is not a Games [ADF] set.
const APPLICATIONS: &str = "Commodore Amiga - Applications - [ADF]";

type Entries<'a> = &'a [(&'a str, &'a [(&'a str, &'a str)])];

/// `entries`: (DAT entry name, ROM file names with the volume each holds).
/// `files`: (folder, local file name, volume written into it).
fn lab(entries: Entries, files: &[(&str, &str, &str)]) -> Lab {
    lab_in(GAMES_ADF, entries, files)
}

fn lab_in(header: &str, entries: Entries, files: &[(&str, &str, &str)]) -> Lab {
    let root = tempfile::tempdir().unwrap().keep();
    for (folder, file, volume) in files {
        let dir = root.join("games").join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), adf(volume)).unwrap();
    }
    let body: String = entries
        .iter()
        .map(|(entry, roms)| {
            let roms: String = roms
                .iter()
                .map(|(rom, volume)| {
                    let bytes = adf(volume);
                    format!(
                        r#"<rom name="{rom}" size="{}" sha1="{}"/>"#,
                        bytes.len(),
                        hex(&Sha1::digest(&bytes))
                    )
                })
                .collect();
            format!(r#"<game name="{entry}"><description>{entry}</description>{roms}</game>"#)
        })
        .collect();
    let dat = root.join("tosec.dat");
    fs::write(
        &dat,
        format!(
            r#"<?xml version="1.0"?><datafile><header><name>{header}</name><description>{header} (TOSEC-v{VERSION})</description><category>TOSEC</category><version>{VERSION}</version><author>Crashdisk - mai - idoru</author><homepage>TOSEC</homepage><url>http://www.tosecdev.org</url></header>{body}</datafile>"#
        ),
    )
    .unwrap();
    let mut db = Database::open_or_create(root.join("library.sqlite3")).unwrap();
    let config = crate::Config {
        source_folders: vec![root.join("games")],
        mount_root: root.join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    crate::scan_and_persist(&mut db, &config, "test").unwrap();
    let outcome = run_dat_audit_with_cache(
        &DatAuditRequest {
            source_id: SOURCE.into(),
            source_display_name: "TOSEC Amiga".into(),
            dat_path: dat.clone(),
            dat_kind: DatSourceKind::File,
            scan_root: root.join("games"),
            limits: DatLimits::default(),
            policy: None,
            platform: None,
        },
        &TrustedRoots::none(),
        &AtomicBool::new(false),
        &|_| {},
        AuditCacheConfig::Disabled,
    )
    .unwrap();
    db.persist_library_dat_identities_from_audit(&outcome)
        .unwrap();
    let catalogues = HashMap::from([(SOURCE.to_string(), catalogue(&dat))]);
    Lab {
        root,
        db,
        catalogues,
    }
}

fn catalogue(dat: &Path) -> ConfiguredCatalogue {
    ConfiguredCatalogue {
        reference: CatalogueRef::Local {
            source_id: SOURCE.into(),
            member: None,
        },
        source_id: SOURCE.into(),
        display_name: "TOSEC Amiga".into(),
        path: dat.to_path_buf(),
        kind: DatSourceKind::File,
        sha256: Some(hex(&Sha256::digest(fs::read(dat).unwrap()))),
        revision: Some(VERSION.into()),
    }
}

impl Lab {
    fn path(&self, folder: &str, file: &str) -> PathBuf {
        self.root.join("games").join(folder).join(file)
    }

    fn identity(&self, folder: &str, file: &str) -> amiga_adf_identity::AmigaAdfIdentityDecision {
        self.db
            .amiga_adf_launch_identity(&self.path(folder, file), &self.catalogues)
            .unwrap()
    }
}

const SINGLE: &str = "Speedball (1988)(Image Works)(Disk 1 of 1)";

fn single() -> Lab {
    lab(
        &[(
            SINGLE,
            &[(
                "Speedball (1988)(Image Works)(Disk 1 of 1).adf",
                "SPEEDBALL",
            )],
        )],
        &[("amiga", "speedball.adf", "SPEEDBALL")],
    )
}

#[test]
fn a_verified_disk_1_of_1_adf_gets_a_hash_launch_identity() {
    let lab = single();
    let identity = lab.identity("amiga", "speedball.adf").expect("verified");
    assert_eq!(identity.entry_name, SINGLE);
    assert_eq!(identity.source_id, SOURCE);
    assert_eq!(identity.source_revision, VERSION);
    assert_eq!(
        identity.game_key,
        format!("adf-sha1:{}", hex(&Sha1::digest(adf("SPEEDBALL"))))
    );
}

#[test]
fn every_disk_of_a_tosec_set_is_refused_although_each_entry_holds_one_file() {
    let set: [(&str, &[(&str, &str)]); 3] = [
        (
            "Lotus (1990)(Gremlin)(Disk 1 of 3)",
            &[("Lotus (1990)(Gremlin)(Disk 1 of 3).adf", "LOTUS1")],
        ),
        (
            "Lotus (1990)(Gremlin)(Disk 2 of 3)",
            &[("Lotus (1990)(Gremlin)(Disk 2 of 3).adf", "LOTUS2")],
        ),
        (
            "Lotus (1990)(Gremlin)(Disk 3 of 3)",
            &[("Lotus (1990)(Gremlin)(Disk 3 of 3).adf", "LOTUS3")],
        ),
    ];
    let lab = lab(
        &set,
        &[
            ("amiga", "l1.adf", "LOTUS1"),
            ("amiga", "l2.adf", "LOTUS2"),
            ("amiga", "l3.adf", "LOTUS3"),
        ],
    );
    for (file, n) in [("l1.adf", 1), ("l2.adf", 2), ("l3.adf", 3)] {
        assert_eq!(
            lab.identity("amiga", file),
            Err(R::DiskNumbered(format!("Disk {n} of 3"))),
            "{file}"
        );
    }
}

#[test]
fn a_verified_disk_without_a_completeness_statement_stays_unknown() {
    let lab = lab_in(
        APPLICATIONS,
        &[(
            "Speedball (1988)(Image Works)",
            &[("Speedball (1988)(Image Works).adf", "SB")],
        )],
        // The local name claims completeness; only the catalogue counts.
        &[("amiga", "Speedball (Disk 1 of 1).adf", "SB")],
    );
    assert_eq!(
        lab.identity("amiga", "Speedball (Disk 1 of 1).adf"),
        Err(R::CompletenessUnknown)
    );
}

#[test]
fn a_filename_matching_an_entry_is_not_identity() {
    let lab = lab(
        &[(
            SINGLE,
            &[(
                "Speedball (1988)(Image Works)(Disk 1 of 1).adf",
                "SPEEDBALL",
            )],
        )],
        &[(
            "amiga",
            "Speedball (1988)(Image Works)(Disk 1 of 1).adf",
            "SOMETHING ELSE",
        )],
    );
    assert_eq!(
        lab.identity("amiga", "Speedball (1988)(Image Works)(Disk 1 of 1).adf"),
        Err(R::NoCurrentMatch)
    );
}

#[test]
fn numbered_siblings_and_multi_file_entries_are_refused() {
    let lab = lab(
        &[
            (
                "Game (1990)(Pub)(Disk 1 of 1)",
                &[("Game (1990)(Pub)(Disk 1 of 1).adf", "G1")],
            ),
            (
                "Game (1990)(Pub)(Disk 2 of 2)",
                &[("Game (1990)(Pub)(Disk 2 of 2).adf", "G2")],
            ),
            (
                "Pair (1991)(Pub)(Disk 1 of 1)",
                &[
                    ("Pair (1991)(Pub)(Disk 1 of 1).adf", "P1"),
                    ("Pair extra.adf", "P2"),
                ],
            ),
        ],
        &[("amiga", "g.adf", "G1"), ("amiga", "p.adf", "P1")],
    );
    assert_eq!(
        lab.identity("amiga", "g.adf"),
        Err(R::NumberedSiblings("Game (1990)(Pub)(Disk 2 of 2)".into()))
    );
    assert_eq!(lab.identity("amiga", "p.adf"), Err(R::NotSingleRom(2)));
}

#[test]
fn stale_unconfigured_or_changed_evidence_is_refused() {
    let mut lab = single();
    // The catalogue file now has different bytes than the one audited.
    let mut changed = lab.catalogues[SOURCE].clone();
    changed.sha256 = Some("0".repeat(64));
    lab.catalogues.insert(SOURCE.into(), changed);
    assert_eq!(
        lab.identity("amiga", "speedball.adf"),
        Err(R::CatalogueStale)
    );
    lab.catalogues.clear();
    assert_eq!(
        lab.identity("amiga", "speedball.adf"),
        Err(R::CatalogueStale)
    );

    let lab = single();
    let path = lab.path("amiga", "speedball.adf");
    let mut bytes = fs::read(&path).unwrap();
    bytes[600] ^= 0xff;
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(&path, bytes).unwrap();
    assert_eq!(lab.identity("amiga", "speedball.adf"), Err(R::FileChanged));
}

#[test]
fn non_adf_unknown_and_non_amiga_entries_are_refused() {
    let lab = lab(
        &[(
            SINGLE,
            &[(
                "Speedball (1988)(Image Works)(Disk 1 of 1).adf",
                "SPEEDBALL",
            )],
        )],
        &[
            ("amiga", "speedball.adf", "SPEEDBALL"),
            ("misc", "elsewhere.adf", "SPEEDBALL"),
        ],
    );
    assert_eq!(
        lab.db
            .amiga_adf_launch_identity(&lab.path("amiga", "speedball.dms"), &lab.catalogues)
            .unwrap(),
        Err(R::NotAdf)
    );
    assert_eq!(lab.identity("amiga", "absent.adf"), Err(R::NotInLibrary));
    // The scanner stored the platform these bytes prove (Amiga); a game
    // whose current assignment is another platform is refused.
    let id = lab
        .db
        .find_archive_id_by_absolute_path(&lab.path("misc", "elsewhere.adf"))
        .unwrap()
        .unwrap();
    lab.db
        .connection
        .execute(
            "UPDATE platform_assignments SET platform = 'Acorn Archimedes' WHERE archive_id = ?1 AND is_current = 1",
            [id],
        )
        .unwrap();
    assert_eq!(lab.identity("misc", "elsewhere.adf"), Err(R::NotAmiga));
}

// ---------------------------------------------------------------------------
// The launch gate in front of the existing FS-UAE preflight.
// ---------------------------------------------------------------------------

struct Emulator {
    roots: AmigaProfileDiscoveryRoots,
    executable: PathBuf,
    config: PathBuf,
}

fn emulator(lab: &Lab, conf: &str, kickstart: bool) -> Emulator {
    let base = lab.root.join("emu");
    let config_root = base.join("fs-uae-config");
    fs::create_dir_all(&config_root).unwrap();
    let executable = base.join("bin").join("fs-uae");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let rom = base.join("kick13.rom");
    if kickstart {
        fs::write(&rom, b"not a verified ROM").unwrap();
    }
    let config = config_root.join("fs-uae.conf");
    fs::write(
        &config,
        format!("{conf}\nkickstart_file = {}\n", rom.display()),
    )
    .unwrap();
    Emulator {
        roots: AmigaProfileDiscoveryRoots {
            home: base.join("home"),
            xdg_config_home: base.join("xdg-config"),
            xdg_data_home: base.join("xdg-data"),
            explicit_configuration_roots: vec![(AmigaEmulatorKind::FsUae, config_root)],
            portable_configuration_roots: Vec::new(),
            explicit_executables: vec![(AmigaEmulatorKind::FsUae, executable.clone())],
            known_version_outputs: Default::default(),
        },
        executable,
        config,
    }
}

fn capture(path: &Path) -> CapturedFileIdentity {
    CapturedFileIdentity::capture(&fs::symlink_metadata(path).unwrap())
}

fn request(lab: &Lab, emu: &Emulator, key: &str) -> FsUaeAdfLaunchRequest {
    request_for(lab, emu, key, "speedball.adf")
}

fn request_for(lab: &Lab, emu: &Emulator, key: &str, file: &str) -> FsUaeAdfLaunchRequest {
    let content = lab.path("amiga", file);
    let profile = crate::patch_manager::discover_amiga_profiles(&emu.roots)
        .profiles
        .into_iter()
        .find(|p| p.emulator == AmigaEmulatorKind::FsUae && p.eligible)
        .expect("FS-UAE profile");
    FsUaeAdfLaunchRequest {
        fsuae: FsUaeLaunchRequest {
            selected_content_path: content.clone(),
            expected_platform_id: "Amiga".into(),
            expected_game_key: key.into(),
            profile_id: profile.profile_id,
            expected_executable: emu.executable.clone(),
            content_identity: capture(&content),
            executable_identity: capture(&emu.executable),
            config_path: emu.config.clone(),
            config_identity: capture(&emu.config),
            caps_backend_available: false,
        },
        roots: emu.roots.clone(),
        database_path: lab.root.join("library.sqlite3"),
        dat_sources_config: lab.root.join("unused.toml"),
    }
}

#[test]
fn a_verified_single_disk_adf_reaches_the_fs_uae_command_with_a_positional_config() {
    let lab = single();
    let key = lab.identity("amiga", "speedball.adf").unwrap().game_key;
    let emu = emulator(&lab, "amiga_model = A500", true);
    let before = fs::read(lab.path("amiga", "speedball.adf")).unwrap();
    let command = preflight_fsuae_adf_launch(&request(&lab, &emu, &key), &lab.db, &lab.catalogues)
        .expect("launchable");
    assert_eq!(command.executable, emu.executable);
    let mut media = std::ffi::OsString::from("--floppy-drive-0=");
    media.push(lab.path("amiga", "speedball.adf"));
    assert_eq!(
        command.arguments,
        vec![emu.config.clone().into_os_string(), media]
    );
    assert_eq!(
        fs::read(lab.path("amiga", "speedball.adf")).unwrap(),
        before
    );
}

#[test]
fn the_launch_gate_refuses_missing_kickstart_wrong_machine_and_drifted_identity() {
    let lab = single();
    let key = lab.identity("amiga", "speedball.adf").unwrap().game_key;

    let emu = emulator(&lab, "amiga_model = A500", false);
    assert!(matches!(
        preflight_fsuae_adf_launch(&request(&lab, &emu, &key), &lab.db, &lab.catalogues),
        Err(FsUaeAdfLaunchError::KickstartNotReady(_))
    ));

    let emu = emulator(&lab, "amiga_model = CD32", true);
    assert!(matches!(
        preflight_fsuae_adf_launch(&request(&lab, &emu, &key), &lab.db, &lab.catalogues),
        Err(FsUaeAdfLaunchError::WrongMachine(model)) if model == "CD32"
    ));

    let emu = emulator(&lab, "amiga_model = A500", true);
    assert_eq!(
        preflight_fsuae_adf_launch(
            &request(&lab, &emu, "adf-sha1:other"),
            &lab.db,
            &lab.catalogues
        ),
        Err(FsUaeAdfLaunchError::IdentityChanged)
    );

    let mut stale = lab.catalogues.clone();
    stale.clear();
    assert_eq!(
        preflight_fsuae_adf_launch(&request(&lab, &emu, &key), &lab.db, &stale),
        Err(FsUaeAdfLaunchError::Identity(R::CatalogueStale))
    );
}

#[test]
fn the_launch_gate_refuses_an_unproven_disk_and_a_non_adf_request() {
    let lab = lab_in(
        APPLICATIONS,
        &[(
            "Speedball (1988)(Image Works)",
            &[("Speedball (1988)(Image Works).adf", "SPEEDBALL")],
        )],
        &[("amiga", "speedball.adf", "SPEEDBALL")],
    );
    let emu = emulator(&lab, "amiga_model = A500", true);
    assert_eq!(
        preflight_fsuae_adf_launch(
            &request(&lab, &emu, "adf-sha1:any"),
            &lab.db,
            &lab.catalogues
        ),
        Err(FsUaeAdfLaunchError::Identity(R::CompletenessUnknown))
    );
    let mut dms = request(&lab, &emu, "adf-sha1:any");
    dms.fsuae.selected_content_path.set_extension("dms");
    assert_eq!(
        preflight_fsuae_adf_launch(&dms, &lab.db, &lab.catalogues),
        Err(FsUaeAdfLaunchError::NotAdf)
    );
}

// ---------------------------------------------------------------------------
// Real TOSEC entry names (copied verbatim from "Commodore Amiga - Games -
// [ADF] (TOSEC-v2025-01-30_CM)"); the disk bytes are synthetic.
// ---------------------------------------------------------------------------

const TURBO: &str = "1000cc Turbo (1990)(Impressions)[cr CSL]";
const NAM_CRACK: &str = "'Nam 1965-1975 v1.0 (1991-03-18)(Domark)(M4)[cr QTX][h BTL]";
const NAM_DISK_2: &str = "'Nam 1965-1975 v1.0 (1991-03-18)(Domark)(M4)(Disk 2 of 2)";
const TANK_1: &str = "A-10 Tank Killer v1.0 (1990)(Dynamix)(Disk 1 of 2)";

fn real_names_lab(header: &str) -> Lab {
    let turbo = format!("{TURBO}.adf");
    let crack = format!("{NAM_CRACK}.adf");
    let disk2 = format!("{NAM_DISK_2}.adf");
    let tank = format!("{TANK_1}.adf");
    lab_in(
        header,
        &[
            (TURBO, &[(turbo.as_str(), "TURBO")]),
            (NAM_CRACK, &[(crack.as_str(), "NAMCRACK")]),
            (NAM_DISK_2, &[(disk2.as_str(), "NAM2")]),
            (TANK_1, &[(tank.as_str(), "TANK1")]),
        ],
        &[
            ("amiga", "turbo.adf", "TURBO"),
            ("amiga", "nam.adf", "NAMCRACK"),
            ("amiga", "tank1.adf", "TANK1"),
        ],
    )
}

#[test]
fn a_real_tosec_single_disk_entry_is_verified_by_hash_and_by_catalogue_convention() {
    use crate::dat::amiga_adf_identity::StandaloneEvidence;
    let lab = real_names_lab(GAMES_ADF);
    let identity = lab.identity("amiga", "turbo.adf").expect("single medium");
    assert_eq!(identity.entry_name, TURBO);
    assert_eq!(identity.standalone, StandaloneEvidence::TosecSingleMedium);
    // The hash and the completeness evidence stay separate facts.
    assert!(identity.game_key.starts_with("adf-sha1:"));

    assert_eq!(
        lab.identity("amiga", "nam.adf"),
        Err(R::NumberedSiblings(NAM_DISK_2.into()))
    );
    assert_eq!(
        lab.identity("amiga", "tank1.adf"),
        Err(R::DiskNumbered("Disk 1 of 2".into()))
    );

    // The same names in a non-Games TOSEC catalogue prove nothing.
    let lab = real_names_lab(APPLICATIONS);
    assert_eq!(
        lab.identity("amiga", "turbo.adf"),
        Err(R::CompletenessUnknown)
    );
}

#[test]
fn a_real_tosec_single_disk_adf_reaches_the_fs_uae_command() {
    let lab = real_names_lab(GAMES_ADF);
    let key = lab.identity("amiga", "turbo.adf").unwrap().game_key;
    let emu = emulator(&lab, "amiga_model = A500", true);
    let request = request_for(&lab, &emu, &key, "turbo.adf");
    let content = lab.path("amiga", "turbo.adf");
    let command = preflight_fsuae_adf_launch(&request, &lab.db, &lab.catalogues)
        .expect("a real TOSEC single-disk entry is launchable");
    let mut media = std::ffi::OsString::from("--floppy-drive-0=");
    media.push(&content);
    assert_eq!(
        command.arguments,
        vec![emu.config.clone().into_os_string(), media]
    );
}
