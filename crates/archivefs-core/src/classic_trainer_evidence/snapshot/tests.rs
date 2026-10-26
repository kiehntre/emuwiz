use super::*;
use crate::dat::limits::DatLimits;
use crate::dat::sources::{
    DatSourceKind,
    audit_cache::AuditCacheConfig,
    audit_run::{DatAuditRequest, run_dat_audit_with_cache},
};
use std::sync::atomic::AtomicBool;

struct World {
    _temp: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
    bytes: Vec<u8>,
}
impl World {
    fn new() -> Self {
        Self::build(false)
    }
    fn build(ambiguous: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let games = root.join("games/ZX Spectrum");
        std::fs::create_dir_all(&games).unwrap();
        let path = games.join("fixture.z80");
        let mut bytes = vec![0; 49182];
        bytes[7] = 128;
        bytes[8] = 254;
        bytes[9] = 255;
        bytes[30 + 16384] = 0x76;
        std::fs::write(&path, &bytes).unwrap();
        let config = Config {
            source_folders: vec![root.join("games")],
            mount_root: root.join("mount"),
            ratarmount_bin: "ratarmount".into(),
            master_rom_root: None,
        };
        std::fs::write(
            root.join("config.toml"),
            format!(
                "source_folders = [\"{}\"]\nmount_root = \"{}\"\nratarmount_bin = \"ratarmount\"\n",
                config.source_folders[0].display(),
                config.mount_root.display()
            ),
        )
        .unwrap();
        let mut db = Database::open_or_create(root.join("library.db")).unwrap();
        crate::scan_and_persist(&mut db, &config, "snapshot-test").unwrap();
        let id = db.find_archive_id_by_absolute_path(&path).unwrap().unwrap();
        db.set_manual_platform_for_archives(&[id], "ZX Spectrum")
            .unwrap();
        let digest = sha1::Sha1::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        std::fs::write(root.join("fixture.dat"),format!(r#"<datafile><header><name>Sinclair ZX Spectrum - Games - [Z80]</name><version>20261025</version></header><game name="Fixture"><rom name="fixture.z80" size="49182" sha1="{digest}"/></game></datafile>"#)).unwrap();
        if ambiguous {
            let dat = std::fs::read_to_string(root.join("fixture.dat")).unwrap();
            let second = format!(
                r#"<game name="Other release"><rom name="other.z80" size="49182" sha1="{digest}"/></game>"#
            );
            std::fs::write(
                root.join("fixture.dat"),
                dat.replace("</datafile>", &format!("{second}</datafile>")),
            )
            .unwrap();
        }
        let outcome = run_dat_audit_with_cache(
            &DatAuditRequest {
                source_id: "zx-fixture".into(),
                source_display_name: "ZX fixture".into(),
                dat_path: root.join("fixture.dat"),
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
        std::fs::write(root.join("dat-sources.toml"),format!("[[sources]]\nid = \"zx-fixture\"\ndisplay_name = \"ZX fixture\"\npath = \"{}\"\nkind = \"file\"\nenabled = true\n",root.join("fixture.dat").display())).unwrap();
        Self {
            _temp: temp,
            root,
            path,
            bytes,
        }
    }
    fn command(&self) -> crate::launch::retroarch_command::RetroArchCommand {
        use crate::emulator_environment::retroarch::{ProfileKind, ProfileRef, ProfileScope};
        use crate::launch::retroarch_command::{RetroArchCommand, RetroArchCommandSelection};
        let executable = self.root.join("frontend");
        std::fs::copy("/usr/bin/true", &executable).unwrap();
        let core = self.root.join("core.so");
        std::fs::write(&core, b"original fixture core").unwrap();
        RetroArchCommand {
            saturn_core_selection: None,
            snapshot_evidence: Some(self.load().unwrap()),
            executable,
            arguments: vec![],
            working_directory: None,
            selection: RetroArchCommandSelection {
                profile: ProfileRef {
                    profile_kind: ProfileKind::Native,
                    scope: ProfileScope::User,
                },
                core_stem: "fuse".into(),
                platform_id: "ZX Spectrum".into(),
                core_library: core,
                content_path: self.path.clone(),
            },
        }
    }
    fn load(&self) -> Result<SnapshotEvidence, String> {
        SnapshotEvidence::load_at(
            &self.path,
            &self.root.join("library.db"),
            &self.root.join("config.toml"),
            &self.root.join("dat-sources.toml"),
        )
    }
}
#[test]
fn verified_snapshot_has_typed_identity_and_revalidates_without_a_trainer() {
    let w = World::new();
    let proof = w.load().unwrap();
    assert_eq!(
        proof.identity(),
        CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "ZX Spectrum".into(),
            game_key: hex_sha256(&w.bytes)
        })
    );
    proof.revalidate().unwrap();
    assert_eq!(std::fs::read(&w.path).unwrap(), w.bytes);
}
#[test]
fn stale_withdrawn_changed_and_unsupported_sources_refuse() {
    for mutation in 0..8 {
        let w = World::new();
        let proof = w.load().unwrap();
        match mutation {
            0 => std::fs::write(w.root.join("dat-sources.toml"), "sources = []\n").unwrap(),
            1 => std::fs::write(w.root.join("fixture.dat"), "changed catalogue").unwrap(),
            2 => {
                let mut bytes = w.bytes.clone();
                bytes[40000] ^= 1;
                std::fs::write(&w.path, bytes).unwrap();
            }
            3 => {
                let mut bytes = w.bytes.clone();
                bytes[12] |= 32;
                std::fs::write(&w.path, bytes).unwrap();
            }
            4 => {
                let mut bytes = w.bytes.clone();
                bytes[7] = 0;
                std::fs::write(&w.path, bytes).unwrap();
            }
            5 => std::fs::write(&w.path, &w.bytes[..100]).unwrap(),
            6 => {
                let text = std::fs::read_to_string(w.root.join("config.toml")).unwrap();
                std::fs::write(
                    w.root.join("config.toml"),
                    text.replace(
                        &format!("\"{}\"", w.root.join("games").display()),
                        "\"/missing/withdrawn\"",
                    ),
                )
                .unwrap();
            }
            _ => {
                let mut db = Database::open_or_create(w.root.join("library.db")).unwrap();
                db.mark_library_dat_identities_stale_for_source("zx-fixture")
                    .unwrap();
            }
        }
        assert!(proof.revalidate().is_err(), "mutation {mutation}");
        assert!(w.load().is_err(), "mutation {mutation}");
    }
}
#[test]
fn filename_and_pin_without_gold_verification_never_mint_authority() {
    let w = World::new();
    let mut db = Database::open_or_create(w.root.join("library.db")).unwrap();
    db.mark_library_dat_identities_stale_for_source("zx-fixture")
        .unwrap();
    std::fs::write(
        w.path.with_extension("pok.target"),
        format!("game-sha256 {}\n", hex_sha256(&w.bytes)),
    )
    .unwrap();
    assert!(w.load().is_err());
}
#[test]
fn unsafe_file_and_platform_drift_refuse() {
    for mutation in 0..3 {
        let w = World::new();
        let proof = w.load().unwrap();
        match mutation {
            0 => {
                let original = w.root.join("original.z80");
                std::fs::rename(&w.path, &original).unwrap();
                std::os::unix::fs::symlink(original, &w.path).unwrap();
            }
            1 => {
                std::fs::hard_link(&w.path, w.root.join("linked.z80")).unwrap();
            }
            _ => {
                let mut db = Database::open_or_create(w.root.join("library.db")).unwrap();
                let id = db
                    .find_archive_id_by_absolute_path(&w.path)
                    .unwrap()
                    .unwrap();
                db.set_manual_platform_for_archives(&[id], "SNES").unwrap();
            }
        }
        assert!(proof.revalidate().is_err());
    }
}

#[test]
fn final_spawn_refuses_changed_or_missing_snapshot_authority() {
    let w = World::new();
    let mut command = w.command();
    std::fs::write(w.root.join("dat-sources.toml"), "sources = []\n").unwrap();
    assert!(crate::launch::spawn_retroarch(command.clone()).is_err());
    command.snapshot_evidence = None;
    assert!(crate::launch::spawn_retroarch(command).is_err());
}

#[test]
fn ambiguous_catalogue_matches_never_mint_authority() {
    let w = World::build(true);
    assert!(w.load().is_err());
}

#[test]
fn config_withdrawal_during_catalogue_resolution_is_refused() {
    let w = World::new();
    let result = SnapshotEvidence::load_at_inner(
        &w.path,
        &w.root.join("library.db"),
        &w.root.join("config.toml"),
        &w.root.join("dat-sources.toml"),
        || {
            let config = std::fs::read_to_string(w.root.join("config.toml")).unwrap();
            std::fs::write(
                w.root.join("config.toml"),
                config.replace(
                    &format!("\"{}\"", w.root.join("games").display()),
                    "\"/missing/withdrawn\"",
                ),
            )
            .unwrap();
        },
    );
    assert!(result.is_err());
}
#[test]
fn final_runtime_sources_run_after_identity_and_guards_run_after_sources() {
    let w = World::new();
    let command = w.command();
    let called = std::cell::Cell::new(false);
    let outcome = crate::launch::execution::spawn_retroarch_guarded(command, || {
        called.set(true);
        std::fs::write(w.root.join("dat-sources.toml"), "sources = []\n").unwrap();
        Ok(())
    });
    assert!(
        called.get(),
        "final source checks must follow successful identity resolution"
    );
    assert!(
        outcome.is_err(),
        "source withdrawal during final checks must prevent spawn"
    );
}

#[test]
fn snapshot_backend_changes_during_final_verification_refuse_even_without_cheats() {
    use std::os::unix::fs::PermissionsExt;
    for mutation in 0..4 {
        let w = World::new();
        let command = w.command();
        let frontend = command.executable.clone();
        let core = command.selection.core_library.clone();
        let result = crate::launch::execution::spawn_retroarch_guarded(command, || {
            match mutation {
                0 => {
                    let replacement = w.root.join("replacement");
                    std::fs::copy("/usr/bin/false", &replacement).unwrap();
                    std::fs::rename(replacement, &frontend).unwrap();
                }
                1 => std::fs::write(&core, b"changed fixture core!").unwrap(),
                2 => std::fs::set_permissions(&frontend, std::fs::Permissions::from_mode(0o600))
                    .unwrap(),
                _ => {
                    std::fs::remove_file(&core).unwrap();
                    std::os::unix::fs::symlink(w.root.join("fixture.dat"), &core).unwrap();
                }
            }
            Ok(())
        });
        assert!(
            result.is_err(),
            "backend mutation {mutation} must prevent spawn"
        );
    }
}
