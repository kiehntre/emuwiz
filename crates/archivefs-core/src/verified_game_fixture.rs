//! A disposable, self-contained EmuWiz profile holding one game that the
//! unmodified production pipeline has verified against a catalogue.
//!
//! Everything lives under one private root (its own config, data, XDG and
//! temp directories), so a test never reads or writes a real profile. The
//! game is a synthetic Game Boy Advance cartridge image whose bytes are generated
//! here; the matching catalogue is a Logiqx DAT whose hashes are computed from
//! those bytes with the production hasher. Verification goes through the same
//! calls Blue and the CLI use: `add_source_folder_at`, `scan_source_folder_at`,
//! the DAT source registry, `run_dat_audit_with_cache` and
//! `persist_library_dat_identities_from_audit`. No row is inserted by hand and
//! no safety check is bypassed, so a game that shows as "Verified" here was
//! verified for the same reasons a real one would be.
//!
//! The fixture also installs a recording fake `retroarch` and a compiled stub
//! libretro core (see [`FakeRetroArch`]) for launch-handoff tests. They prove
//! what EmuWiz hands to the emulator, never that a game or cheat runs.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use crate::dat::library_identity_summary::{DatVerificationState, PersistedLibraryDatIdentity};
use crate::dat::limits::DatLimits;
use crate::dat::sources::audit_cache::AuditCacheConfig;
use crate::dat::sources::audit_run::{DatAuditOutcome, DatAuditRequest, run_dat_audit_with_cache};
use crate::dat::sources::{
    DatSourceEntry, DatSourceKind, DatSourceRegistry, save_dat_sources_config_to,
};
use crate::identity_source::hashing::hash_file_reporting;
use crate::safe_read::TrustedRoots;
use crate::{Database, add_source_folder_at, scan_source_folder_at};

/// Identifier the fixture registers its catalogue under.
pub const DAT_SOURCE_ID: &str = "emuwiz-fixture-no-intro";
/// Catalogue entry name; also the game's display name in the Library.
pub const GAME_NAME: &str = "EmuWiz Synthetic Fixture (USA)";
/// File name of the synthetic cartridge inside the games folder.
pub const ROM_FILE_NAME: &str = "EmuWiz Synthetic Fixture (USA).gba";
/// Platform name used in the DAT header. A loose `.gba` file gets this
/// platform from the scan alone and no header skipping applies to it.
pub const DAT_PLATFORM: &str = "Nintendo - Game Boy Advance";
/// The canonical platform the scan assigns to the cartridge.
pub const PLATFORM: &str = "Game Boy Advance";
/// Size of the generated cartridge image.
pub const ROM_SIZE: usize = 64 * 1024;

/// Environment a child process (Blue, the CLI) needs so it resolves this
/// profile and nothing else.
pub const CHILD_ENVIRONMENT_KEYS: [&str; 9] = [
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    // An empty, private directory list: host desktop entries (for example
    // a real emulator's `.desktop` file under /usr/share) never reach
    // discovery inside the profile.
    "XDG_DATA_DIRS",
    "EMUWIZ_CONFIG_HOME",
    "EMUWIZ_DATA_HOME",
    "TMPDIR",
];

/// One verified game in a private profile. Dropping it removes the whole
/// root when the fixture created a temporary one.
pub struct VerifiedGameFixture {
    root: PathBuf,
    owned: Option<tempfile::TempDir>,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub config_path: PathBuf,
    pub database_path: PathBuf,
    pub games_dir: PathBuf,
    pub rom_path: PathBuf,
    pub dat_path: PathBuf,
    /// The audit that produced the stored row; absent after [`reopen`](Self::reopen).
    pub audit: Option<DatAuditOutcome>,
}

impl VerifiedGameFixture {
    /// Build in a fresh temporary root that is removed on drop.
    pub fn build() -> Result<Self, String> {
        let owned = tempfile::Builder::new()
            .prefix("emuwiz-verified-game-")
            .tempdir()
            .map_err(|error| format!("temporary root: {error}"))?;
        let root = owned.path().to_path_buf();
        let mut fixture = Self::build_at(&root)?;
        fixture.owned = Some(owned);
        Ok(fixture)
    }

    /// Build under `root`, which the caller owns; nothing outside it is read
    /// or written. `root` must be absent or empty, so a real profile can
    /// never be mistaken for a disposable one.
    pub fn build_at(root: &Path) -> Result<Self, String> {
        Self::build_with(root, |bytes| dat_for(bytes, &hashes_of(bytes)?))
    }

    /// Like [`build_at`](Self::build_at) but with a catalogue that lists a
    /// different ROM, so the game must come out *not* verified. Used to prove
    /// the pipeline is doing real matching.
    pub fn build_at_with_mismatched_catalogue(root: &Path) -> Result<Self, String> {
        Self::build_with(root, |_| {
            let other = synthetic_rom(0x5EED);
            dat_for(&other, &hashes_of(&other)?)
        })
    }

    /// Reuse the same scan, DAT verification and persisted-library pipeline
    /// for a synthetic game and its companion files. Test-support only.
    pub fn build_at_with_game(
        root: &Path,
        filename: &str,
        bytes: &[u8],
        dat_platform: &str,
        companions: &[(&str, &[u8])],
    ) -> Result<Self, String> {
        Self::build_profile(root, filename, bytes, companions, |data| {
            let mut dat = dat_for(data, &hashes_of(data)?)?
                .replace(ROM_FILE_NAME, filename)
                .replace(DAT_PLATFORM, dat_platform);
            if !companions.is_empty() {
                let mut roms = String::new();
                for (name, bytes) in companions {
                    let hashes = hashes_of(bytes)?;
                    roms.push_str(&format!(
                        "<rom name=\"{name}\" size=\"{}\" crc=\"{}\" md5=\"{}\" sha1=\"{}\"/>",
                        bytes.len(),
                        hashes.crc32,
                        hashes.md5,
                        hashes.sha1
                    ));
                }
                dat = dat.replace("</game>", &format!("{roms}</game>")).replace(
                    "<homepage>No-Intro</homepage>",
                    "<homepage>redump.org</homepage>",
                );
            }
            Ok(dat)
        })
    }

    fn build_with(
        root: &Path,
        catalogue: impl FnOnce(&[u8]) -> Result<String, String>,
    ) -> Result<Self, String> {
        Self::build_profile(root, ROM_FILE_NAME, &synthetic_rom(0x1EE7), &[], catalogue)
    }

    fn build_profile(
        root: &Path,
        filename: &str,
        rom_bytes: &[u8],
        companions: &[(&str, &[u8])],
        catalogue: impl FnOnce(&[u8]) -> Result<String, String>,
    ) -> Result<Self, String> {
        for name in std::iter::once(filename).chain(companions.iter().map(|(name, _)| *name)) {
            let mut components = Path::new(name).components();
            if !matches!(components.next(), Some(std::path::Component::Normal(_)))
                || components.next().is_some()
            {
                return Err("fixture filenames must be single safe path components".into());
            }
        }
        if root.exists() && fs::read_dir(root).map_err(fail)?.next().is_some() {
            return Err(format!("refusing a non-empty root: {}", root.display()));
        }
        let config_dir = root.join("config");
        let data_dir = root.join("data");
        let games_dir = root.join(if filename.ends_with(".cue") {
            "games/psx"
        } else if filename.ends_with(".gb") {
            "games/gb"
        } else if filename.ends_with(".vb") {
            "games/vb"
        } else {
            "games"
        });
        for dir in [
            &config_dir,
            &data_dir,
            &games_dir,
            &root.join("home"),
            &root.join("xdg-config"),
            &root.join("xdg-data"),
            &root.join("xdg-cache"),
            &root.join("xdg-state"),
            &root.join("xdg-data-dirs"),
            &root.join("tmp"),
            &root.join("mount"),
        ] {
            fs::create_dir_all(dir).map_err(fail)?;
        }
        let rom_path = games_dir.join(filename);
        fs::write(&rom_path, rom_bytes).map_err(fail)?;
        for (name, bytes) in companions {
            fs::write(games_dir.join(name), bytes).map_err(fail)?;
        }
        let dat_path = root.join("catalogue").join("fixture-no-intro.dat");
        fs::create_dir_all(dat_path.parent().unwrap()).map_err(fail)?;
        fs::write(&dat_path, catalogue(rom_bytes)?).map_err(fail)?;

        let config_path = config_dir.join("config.toml");
        let database_path = data_dir.join("library.sqlite3");
        // The mount root is a required config value; nothing is mounted.
        fs::write(
            &config_path,
            format!(
                "source_folders = []\nmount_root = \"{}\"\nratarmount_bin = \"ratarmount\"\n",
                root.join("mount").display()
            ),
        )
        .map_err(fail)?;

        // 1. Register the games folder and scan it (production calls).
        add_source_folder_at(&config_path, &database_path, &games_dir).map_err(fail)?;
        scan_source_folder_at(
            &config_path,
            &database_path,
            &games_dir,
            "verified-game-fixture",
        )
        .map_err(fail)?;

        // 2. Register the catalogue exactly as the DAT page saves one.
        let mut registry = DatSourceRegistry::new();
        registry
            .add(DatSourceEntry::new(
                DAT_SOURCE_ID.into(),
                "Fixture No-Intro".into(),
                dat_path.clone(),
                DatSourceKind::File,
            ))
            .map_err(fail)?;
        save_dat_sources_config_to(config_dir.join("dat_sources.toml"), &registry.to_config())
            .map_err(fail)?;

        // 3. Verify the folder against the catalogue and persist the result.
        let audit = run_dat_audit_with_cache(
            &DatAuditRequest {
                source_id: DAT_SOURCE_ID.into(),
                source_display_name: "Fixture No-Intro".into(),
                dat_path: dat_path.clone(),
                dat_kind: DatSourceKind::File,
                scan_root: games_dir.clone(),
                limits: DatLimits::default(),
                policy: None,
                platform: None,
            },
            &TrustedRoots::none(),
            &AtomicBool::new(false),
            &|_| {},
            AuditCacheConfig::Disabled,
        )
        .map_err(fail)?;
        let mut database = Database::open_or_create(&database_path).map_err(fail)?;
        database
            .persist_library_dat_identities_from_audit(&audit)
            .map_err(fail)?;
        database.close().map_err(fail)?;

        Ok(Self {
            root: root.to_path_buf(),
            owned: None,
            config_dir,
            data_dir,
            config_path,
            database_path,
            games_dir,
            rom_path,
            dat_path,
            audit: Some(audit),
        })
    }

    /// Reopen a root that [`build_at`](Self::build_at) produced earlier, for
    /// example after a real Blue process ran against it. Nothing is rebuilt.
    pub fn reopen(root: &Path) -> Result<Self, String> {
        let config_dir = root.join("config");
        let data_dir = root.join("data");
        let games_dir = root.join("games");
        let database_path = data_dir.join("library.sqlite3");
        if !database_path.is_file() {
            return Err(format!(
                "no fixture database at {}",
                database_path.display()
            ));
        }
        Ok(Self {
            root: root.to_path_buf(),
            owned: None,
            config_path: config_dir.join("config.toml"),
            config_dir,
            database_path,
            rom_path: games_dir.join(ROM_FILE_NAME),
            games_dir,
            data_dir,
            dat_path: root.join("catalogue").join("fixture-no-intro.dat"),
            audit: None,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Opens the database afresh (as a restarted application would) and
    /// returns the game's library row id.
    pub fn archive_id(&self) -> Result<i64, String> {
        Database::open_or_create(&self.database_path)
            .map_err(fail)?
            .find_archive_id_by_absolute_path(&self.rom_path)
            .map_err(fail)?
            .ok_or_else(|| "the scanned cartridge has no library row".to_string())
    }

    /// The stored verification row, read from a fresh database handle.
    pub fn persisted_identity(&self) -> Result<Option<PersistedLibraryDatIdentity>, String> {
        let id = self.archive_id()?;
        Database::open_or_create(&self.database_path)
            .map_err(fail)?
            .library_dat_identity_for_item(id, DAT_SOURCE_ID)
            .map_err(fail)
    }

    /// True only when the stored row says exactly one catalogue entry matched
    /// on a cryptographic hash.
    pub fn is_verified(&self) -> Result<bool, String> {
        Ok(matches!(
            self.persisted_identity()?.map(|row| row.verification_state),
            Some(DatVerificationState::VerifiedSingleMatch { .. })
        ))
    }

    /// Environment for a child process so it uses this profile only.
    pub fn child_environment(&self) -> Vec<(String, String)> {
        Self::environment_for(&self.root)
    }

    /// The same environment, derived from a root alone. A fixture builder
    /// that is not itself running under the test environment sets these
    /// before it calls any production code, so nothing resolves to the
    /// real user's configuration.
    pub fn environment_for(root: &Path) -> Vec<(String, String)> {
        let values = [
            root.join("home"),
            root.join("xdg-config"),
            root.join("xdg-data"),
            root.join("xdg-cache"),
            root.join("xdg-state"),
            root.join("xdg-data-dirs"),
            root.join("config"),
            root.join("data"),
            root.join("tmp"),
        ];
        CHILD_ENVIRONMENT_KEYS
            .iter()
            .zip(values)
            .map(|(key, value)| ((*key).to_string(), value.display().to_string()))
            .collect()
    }

    /// Install a recording fake RetroArch beside the profile.
    pub fn fake_retroarch(&self, library_name: &str) -> Result<FakeRetroArch, String> {
        FakeRetroArch::install(&self.root.join("retroarch"), library_name)
    }
}

/// A fake `retroarch` executable that records its arguments and the
/// temporary profile it was pointed at, plus a compiled stub core that
/// answers `retro_get_system_info` with a known library name. Enough for the
/// launch-handoff path; it starts no emulator.
pub struct FakeRetroArch {
    pub root: PathBuf,
    pub executable: PathBuf,
    pub config: PathBuf,
    pub core: PathBuf,
    pub cores_dir: PathBuf,
    pub record_dir: PathBuf,
}

impl FakeRetroArch {
    pub fn install(root: &Path, library_name: &str) -> Result<Self, String> {
        let record_dir = root.join("record");
        let cores_dir = root.join("cores");
        for dir in [
            "bin",
            "config/retroarch",
            "cores",
            "info",
            "saves",
            "states",
        ] {
            fs::create_dir_all(root.join(dir)).map_err(fail)?;
        }
        let executable = root.join("bin").join("retroarch");
        fs::write(
            &executable,
            format!(
                // Absolute tool paths: a test may put only this fake's own
                // directory on PATH, so that a host `retroarch` (for
                // example `/usr/bin/retroarch -> ../games/retroarch`) can
                // never be discovered beside it.
                "#!/bin/sh\nrec=\"{rec}\"\n/bin/mkdir -p \"$rec\"\nprintf '%s\\n' \"$@\" > \"$rec/args.txt\"\n\
                 while [ $# -gt 0 ]; do if [ \"$1\" = \"--config\" ]; then /bin/cp -r \"$(/usr/bin/dirname \"$(/usr/bin/dirname \"$2\")\")\" \"$rec/profile\"; fi; shift; done\nexit 0\n",
                rec = record_dir.display()
            ),
        )
        .map_err(fail)?;
        let body = fs::read_to_string(&executable).map_err(fail)?;
        compile_native_test_executable(&executable, &body)?;
        let config = root.join("config/retroarch/retroarch.cfg");
        fs::write(
            &config,
            format!(
                "libretro_directory = \"{}\"\nlibretro_info_path = \"{}\"\nvideo_driver = \"vulkan\"\ninput_player1_a = \"x\"\ncheat_database_path = \"{}\"\nconfig_save_on_exit = \"true\"\nsavefile_directory = \"{}\"\nsavestate_directory = \"{}\"\n",
                cores_dir.display(),
                root.join("info").display(),
                root.join("cheats").display(),
                root.join("saves").display(),
                root.join("states").display()
            ),
        )
        .map_err(fail)?;
        fs::write(root.join("info/mgba.info"), "systemname = \"gba\"\n").map_err(fail)?;
        let core = cores_dir.join("mgba_libretro.so");
        compile_stub_core(&core, library_name)?;
        Ok(Self {
            root: root.to_path_buf(),
            executable,
            config,
            core,
            cores_dir,
            record_dir,
        })
    }

    /// The XDG config root that holds `retroarch/retroarch.cfg`: point a
    /// child's `XDG_CONFIG_HOME` here so production discovery finds the fake.
    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    /// The argument list the fake received, one per line, if it ran.
    pub fn recorded_arguments(&self) -> Option<Vec<String>> {
        fs::read_to_string(self.record_dir.join("args.txt"))
            .ok()
            .map(|text| text.lines().map(str::to_string).collect())
    }
}

/// Compiles a minimal shared object exposing `retro_get_system_info`.
pub fn compile_stub_core(path: &Path, library_name: &str) -> Result<(), String> {
    let source = path.with_extension("c");
    fs::write(
        &source,
        format!(
            "struct info {{ const char *n, *v, *e; _Bool a, b; }};\nvoid retro_get_system_info(struct info *i) {{ i->n = \"{library_name}\"; i->v = \"1\"; i->e = \"gba\"; }}\n"
        ),
    )
    .map_err(fail)?;
    let status = Command::new("cc")
        .args(["-shared", "-fPIC", "-O0", "-o"])
        .arg(path)
        .arg(&source)
        .status()
        .map_err(|error| format!("a C compiler (cc) is needed for the stub core: {error}"))?;
    if !status.success() {
        return Err(format!("cc failed with {status}"));
    }
    Ok(())
}

/// Native ELF fixture for installation-discovery/launch tests. Test behaviour
/// remains in the fixture's shell body, but discovery sees an actual native
/// program, never a launcher script misclassified as native RetroArch.
pub fn compile_native_test_executable(path: &Path, body: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;
    let literal = serde_json::to_string(body).map_err(fail)?;
    let source = format!(
        "#include <unistd.h>\n#include <stdlib.h>\nint main(int argc, char **argv) {{ char **args = calloc(argc + 4, sizeof(char*)); if (!args) return 125; args[0] = \"sh\"; args[1] = \"-c\"; args[2] = {literal}; for (int i = 0; i < argc; i++) args[i + 3] = argv[i]; execv(\"/bin/sh\", args); return 126; }}\n"
    );
    let mut child = Command::new("cc")
        .args(["-x", "c", "-O0", "-o"])
        .arg(path)
        .arg("-")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(fail)?;
    child
        .stdin
        .take()
        .ok_or("cc stdin unavailable")?
        .write_all(source.as_bytes())
        .map_err(fail)?;
    let status = child.wait().map_err(fail)?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cc failed with {status}"))
    }
}

/// A deterministic 64 KiB Game Boy Advance cartridge image with a plausible
/// header (title, game code, maker code, the fixed 0x96 byte and a correct
/// header complement) over a fixed pseudo-random payload. The Nintendo logo
/// area is left as payload: nothing in the image is copyrighted and nothing
/// in it executes.
pub fn synthetic_rom(seed: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; ROM_SIZE];
    let mut state = seed.wrapping_mul(0x9E37_79B9) | 1;
    for byte in bytes.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = (state >> 24) as u8;
    }
    bytes[0xA0..0xAC].copy_from_slice(b"EMUWIZ FIXTR");
    bytes[0xAC..0xB0].copy_from_slice(b"AEWZ");
    bytes[0xB0..0xB2].copy_from_slice(b"01");
    bytes[0xB2] = 0x96;
    bytes[0xB3..0xBD].fill(0);
    let complement = bytes[0xA0..0xBD]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_sub(*byte))
        .wrapping_sub(0x19);
    bytes[0xBD] = complement;
    bytes[0xBE..0xC0].fill(0);
    bytes
}

struct Hashes {
    crc32: String,
    md5: String,
    sha1: String,
}

fn hashes_of(bytes: &[u8]) -> Result<Hashes, String> {
    let scratch = tempfile::Builder::new()
        .prefix("emuwiz-fixture-hash-")
        .tempdir()
        .map_err(fail)?;
    let path = scratch.path().join("rom.bin");
    fs::write(&path, bytes).map_err(fail)?;
    let hashes = hash_file_reporting(&path, &TrustedRoots::none(), None, &|_| {})
        .map_err(|refusal| format!("{refusal:?}"))?;
    Ok(Hashes {
        crc32: hashes.crc32,
        md5: hashes.md5,
        sha1: hashes.sha1,
    })
}

fn dat_for(bytes: &[u8], hashes: &Hashes) -> Result<String, String> {
    Ok(format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE datafile PUBLIC \"-//Logiqx//DTD ROM Management Datafile//EN\" \"http://www.logiqx.com/Dats/datafile.dtd\">\n\
         <datafile><header><name>{DAT_PLATFORM}</name><description>{DAT_PLATFORM} (EmuWiz fixture)</description>\
         <version>20261021-000000</version><author>EmuWiz fixture</author><homepage>No-Intro</homepage></header>\
         <game name=\"{GAME_NAME}\"><description>{GAME_NAME}</description>\
         <rom name=\"{ROM_FILE_NAME}\" size=\"{}\" crc=\"{}\" md5=\"{}\" sha1=\"{}\"/></game></datafile>\n",
        bytes.len(),
        hashes.crc32,
        hashes.md5,
        hashes.sha1
    ))
}

fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}
