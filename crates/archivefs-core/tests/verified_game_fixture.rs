//! The shared verified-game fixture goes through the production pipeline
//! only, is verified for real reasons, survives a restart, and never leaves
//! its private root.

archivefs_core::install_test_environment!();

use archivefs_core::dat::library_identity_summary::DatVerificationState;
use archivefs_core::verified_game_fixture::{
    DAT_SOURCE_ID, GAME_NAME, PLATFORM, VerifiedGameFixture, synthetic_rom,
};
use archivefs_core::{Database, load_source_folder_configs_from};
use std::path::Path;

#[test]
fn the_game_is_verified_by_the_production_pipeline_and_persisted() {
    let fixture = VerifiedGameFixture::build().unwrap();
    let row = fixture
        .persisted_identity()
        .unwrap()
        .expect("a stored DAT identity row");
    assert!(
        matches!(row.verification_state, DatVerificationState::VerifiedSingleMatch { ref algorithm } if algorithm != "crc32"),
        "{:?}",
        row.verification_state
    );
    assert_eq!(
        row.canonical.canonical_dat_name.as_deref(),
        Some(GAME_NAME),
        "{:?}",
        row.canonical
    );
    assert_eq!(row.source.source_id, DAT_SOURCE_ID);
    // The scan itself assigned the platform; no manual assignment.
    let database = Database::open_or_create(&fixture.database_path).unwrap();
    let archive = database
        .load_archives()
        .unwrap()
        .into_iter()
        .find(|archive| archive.absolute_path == fixture.rom_path)
        .expect("the cartridge is in the library");
    assert_eq!(archive.platform.as_deref(), Some(PLATFORM), "{archive:?}");
    // Registration went through config.toml, as the app writes it.
    let sources = load_source_folder_configs_from(&fixture.config_path).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].path, fixture.games_dir);
}

#[test]
fn verification_survives_reopening_the_profile_as_a_restart_would() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().join("profile");
    VerifiedGameFixture::build_at(&root).unwrap();
    // A second handle sees only what the database and config files say.
    let reopened = VerifiedGameFixture::reopen(&root).unwrap();
    assert!(reopened.is_verified().unwrap());
    assert!(reopened.audit.is_none());
    assert_eq!(
        reopened.archive_id().unwrap(),
        VerifiedGameFixture::reopen(&root)
            .unwrap()
            .archive_id()
            .unwrap()
    );
}

#[test]
fn a_catalogue_that_lists_a_different_rom_does_not_verify_the_game() {
    let root = tempfile::tempdir().unwrap();
    let fixture =
        VerifiedGameFixture::build_at_with_mismatched_catalogue(&root.path().join("p")).unwrap();
    assert!(!fixture.is_verified().unwrap());
    assert_ne!(synthetic_rom(0x1EE7), synthetic_rom(0x5EED));
}

#[test]
fn the_fixture_is_disposable_and_stays_inside_its_root() {
    let root;
    {
        let fixture = VerifiedGameFixture::build().unwrap();
        root = fixture.root().to_path_buf();
        for (key, value) in fixture.child_environment() {
            assert!(
                Path::new(&value).starts_with(&root),
                "{key}={value} escapes the root"
            );
        }
        for path in [
            &fixture.config_path,
            &fixture.database_path,
            &fixture.dat_path,
            &fixture.rom_path,
        ] {
            assert!(path.starts_with(&root), "{}", path.display());
        }
        assert!(fixture.config_dir.join("dat_sources.toml").is_file());
    }
    assert!(!root.exists(), "a temporary fixture is removed on drop");
}

#[test]
fn a_non_empty_root_is_refused() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("precious"), b"user data").unwrap();
    let error = match VerifiedGameFixture::build_at(root.path()) {
        Ok(_) => panic!("a non-empty root must be refused"),
        Err(error) => error,
    };
    assert!(error.contains("non-empty"), "{error}");
    assert_eq!(
        std::fs::read(root.path().join("precious")).unwrap(),
        b"user data"
    );
}

#[test]
fn the_fake_retroarch_records_its_arguments_and_the_stub_core_names_itself() {
    let fixture = VerifiedGameFixture::build().unwrap();
    let fake = fixture.fake_retroarch("mGBA").unwrap();
    assert!(fake.recorded_arguments().is_none());
    let status = std::process::Command::new(&fake.executable)
        .args([
            "--config",
            fake.config.to_str().unwrap(),
            "-L",
            fake.core.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let recorded = fake.recorded_arguments().unwrap();
    assert_eq!(recorded[0], "--config");
    assert!(
        fake.record_dir.join("profile").is_dir(),
        "the profile pointed at was copied"
    );
    let bytes = std::fs::read(&fake.core).unwrap();
    assert!(
        bytes.windows(4).any(|w| w == b"mGBA"),
        "the stub core carries its library name"
    );
}
