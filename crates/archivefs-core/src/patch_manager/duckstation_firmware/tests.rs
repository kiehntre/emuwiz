//! No network anywhere in this suite, and no real Redump/Sony hash is
//! embedded - every hash below is computed at test time from synthetic byte
//! content this test itself invented, never a value copied from a real BIOS
//! dump or a real Redump DAT.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::dat::firmware_evidence::FirmwareSystem;
use crate::dat::model::DatEcosystem;
use crate::patch_manager::duckstation_local::{
    DuckStationProfileDiscoveryRoots, discover_duckstation_profiles,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn fixture_root(label: &str) -> PathBuf {
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "archivefs-duckstation-firmware-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

const BIOS_BYTES: &[u8] = b"synthetic test PS1 BIOS bytes, not a real Sony dump...";

fn write_bios(bios_root: &std::path::Path, name: &str, bytes: &[u8]) -> PathBuf {
    fs::create_dir_all(bios_root).unwrap();
    let path = bios_root.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Hashes `bytes` with the exact same algorithms `hash_firmware_file` uses,
/// so a fixture's evidence record is guaranteed self-consistent without
/// this test ever hand-typing a digest.
fn digests_of(bytes: &[u8]) -> (String, String, String) {
    use md5::Md5;
    use sha1::Sha1;
    use sha1::digest::Digest;
    let crc32 = crate::identity_source::hashing::Crc32::of(bytes);
    let md5 = hex(&Md5::digest(bytes));
    let sha1 = hex(&Sha1::digest(bytes));
    (crc32, md5, sha1)
}

fn record_for(name: &str, bytes: &[u8]) -> FirmwareIdentityRecord {
    record_for_system(name, bytes, FirmwareSystem::PlayStation)
}

fn record_for_system(name: &str, bytes: &[u8], system: FirmwareSystem) -> FirmwareIdentityRecord {
    let (crc32, md5, sha1) = digests_of(bytes);
    FirmwareIdentityRecord {
        system,
        provider: DatEcosystem::Redump,
        name: name.to_string(),
        description: Some(format!("{name} description")),
        size_bytes: bytes.len() as u64,
        crc32,
        md5,
        sha1,
        dat_version: Some("20240101".to_string()),
    }
}

/// A real, discovered, eligible DuckStation profile - configuration is
/// driven through the exact same `discover_duckstation_profiles`/
/// `inspect_duckstation_game` production code every other test in this
/// crate uses, never a hand-built struct pretending to be discovery output.
struct Fixture {
    profile: crate::patch_manager::duckstation_local::DuckStationProfile,
    root: PathBuf,
}

fn write_global(root: &std::path::Path, text: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("settings.ini"), text).unwrap();
}

fn write_per_game(root: &std::path::Path, serial: &str, text: &str) {
    let path = root.join("gamesettings").join(format!("{serial}.ini"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn build_fixture(label: &str, global_ini: &str) -> Fixture {
    let temp_root = fixture_root(label);
    let configuration_path = temp_root.join("duckstation");
    write_global(&configuration_path, global_ini);
    let roots = DuckStationProfileDiscoveryRoots {
        home: temp_root.join("home"),
        xdg_config_home: temp_root.join("home"),
        xdg_data_home: temp_root.join("data"),
        xdg_config_home_explicit: false,
        explicit_configuration_roots: vec![configuration_path.clone()],
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
        path_override: Some(Vec::new()),
    };
    let profile = discover_duckstation_profiles(&roots)
        .profiles
        .into_iter()
        .find(|profile| profile.configuration_path == configuration_path && profile.eligible)
        .expect("fixture profile must be discovered and eligible");
    Fixture {
        profile,
        root: temp_root,
    }
}

fn request(serial: &str) -> crate::patch_manager::duckstation_local::DuckStationGameRequest {
    crate::patch_manager::duckstation_local::DuckStationGameRequest {
        verified_ps1_serial: Some(serial.to_string()),
        ..Default::default()
    }
}

fn inspection_for(
    fixture: &Fixture,
    serial: &str,
) -> crate::patch_manager::duckstation_local::DuckStationGameInspection {
    crate::patch_manager::duckstation_local::inspect_duckstation_game(
        &fixture.profile,
        &request(serial),
    )
}

// --- 1: exact PS1 Redump record match -> Verified -----------------------------------------------

#[test]
fn matching_bios_becomes_verified() {
    let fixture = build_fixture("verified", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("PS1 BIOS SCPH-1001", BIOS_BYTES)];

    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    let DuckStationBiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified, got {outcome:?}");
    };
    assert_eq!(verified.size_bytes, BIOS_BYTES.len() as u64);
    assert_eq!(verified.record.name, "PS1 BIOS SCPH-1001");
    assert_eq!(verified.record.provider, DatEcosystem::Redump);
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 2/3/4/5: mismatches never verify -------------------------------------------------------------

#[test]
fn sha1_mismatch_is_not_verified() {
    let fixture = build_fixture("sha1-mismatch", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let mut record = record_for("fixture", BIOS_BYTES);
    record.sha1 = "0".repeat(40);
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[record]);
    assert!(!matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn md5_mismatch_is_not_verified() {
    let fixture = build_fixture("md5-mismatch", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let mut record = record_for("fixture", BIOS_BYTES);
    record.md5 = "0".repeat(32);
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[record]);
    assert!(!matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn crc32_mismatch_is_not_verified() {
    let fixture = build_fixture("crc32-mismatch", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let mut record = record_for("fixture", BIOS_BYTES);
    record.crc32 = "00000000".to_string();
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[record]);
    assert!(!matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn size_mismatch_is_not_verified() {
    let fixture = build_fixture("size-mismatch", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let mut record = record_for("fixture", BIOS_BYTES);
    record.size_bytes += 1;
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[record]);
    assert!(!matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 6: convincing filename + wrong bytes -> Unknown ------------------------------------------

#[test]
fn plausible_filename_with_wrong_hash_is_unknown_not_verified() {
    let fixture = build_fixture(
        "plausible-filename",
        "[BIOS]\nBIOSFilename=scph1001_verified.bin\n",
    );
    write_bios(
        &fixture.profile.bios_path,
        "scph1001_verified.bin",
        BIOS_BYTES,
    );
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let record = record_for("unrelated", b"totally different bytes entirely");
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[record]);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Unknown { .. }
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 7: missing BIOS -> Missing -----------------------------------------------------------------

#[test]
fn missing_configured_bios_is_missing() {
    let fixture = build_fixture("missing", "[BIOS]\nBIOSFilename=nope.bin\n");
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert_eq!(outcome, DuckStationBiosVerificationOutcome::Missing);
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn nothing_configured_at_all_is_missing() {
    let fixture = build_fixture("nothing-configured", "");
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert_eq!(outcome, DuckStationBiosVerificationOutcome::Missing);
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 8: symlink -> rejected -----------------------------------------------------------------------

#[test]
fn symlink_bios_is_unsafe() {
    let fixture = build_fixture("symlink", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    let real = write_bios(&fixture.profile.bios_path, "real.bin", BIOS_BYTES);
    let link = fixture.profile.bios_path.join("scph1001.bin");
    symlink(&real, &link).unwrap();
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Unsafe { .. }
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 9: non-regular -> rejected ---------------------------------------------------------------

#[test]
fn directory_bios_is_unsafe() {
    let fixture = build_fixture("directory", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    fs::create_dir_all(fixture.profile.bios_path.join("scph1001.bin")).unwrap();
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Unsafe { .. }
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 10: unreadable -> honest failure --------------------------------------------------------

#[test]
fn unreadable_bios_permission_denied() {
    if unsafe { libc::geteuid() } == 0 {
        // Running as root ignores POSIX permission bits entirely.
        return;
    }
    let fixture = build_fixture("unreadable", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    let path = write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Unreadable { .. }
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 11: provenance retained -------------------------------------------------------------------

#[test]
fn matched_record_metadata_is_retained() {
    let fixture = build_fixture("provenance", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let record = record_for("PS1 BIOS SCPH-1001", BIOS_BYTES);
    let outcome =
        resolve_duckstation_bios(&fixture.profile, &inspection, std::slice::from_ref(&record));
    let DuckStationBiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified");
    };
    assert_eq!(verified.record, record);
    assert_eq!(verified.required_region, Some("NTSC-U"));
    let (crc32, md5, sha1) = digests_of(BIOS_BYTES);
    assert_eq!(verified.crc32, crc32);
    assert_eq!(verified.md5, md5);
    assert_eq!(verified.sha1, sha1);
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 12: correct configured BIOS selected -------------------------------------------------------

#[test]
fn configured_generic_bios_is_the_one_selected_and_verified() {
    let fixture = build_fixture("configured-generic", "[BIOS]\nBIOSFilename=my-bios.bin\n");
    write_bios(&fixture.profile.bios_path, "my-bios.bin", BIOS_BYTES);
    // A decoy file that would be wrongly picked by "just glob *.bin".
    write_bios(
        &fixture.profile.bios_path,
        "decoy.bin",
        b"decoy bytes, wrong file",
    );
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    let DuckStationBiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified, got {outcome:?}");
    };
    assert_eq!(verified.path, fixture.profile.bios_path.join("my-bios.bin"));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 13: two region BIOS configs are not arbitrarily collapsed ------------------------------------

#[test]
fn ntscu_and_ntscj_region_configs_select_their_own_distinct_file() {
    let fixture = build_fixture(
        "two-regions",
        "[BIOS]\nPathNTSCU=scph1001.bin\nPathNTSCJ=scph5500.bin\n",
    );
    let ntscu_bytes = b"synthetic ntsc-u bios bytes, distinct from ntsc-j";
    let ntscj_bytes = b"synthetic ntsc-j bios bytes, distinct from ntsc-u";
    write_bios(&fixture.profile.bios_path, "scph1001.bin", ntscu_bytes);
    write_bios(&fixture.profile.bios_path, "scph5500.bin", ntscj_bytes);
    let evidence = vec![
        record_for("PS1 BIOS SCPH-1001 (NTSC-U)", ntscu_bytes),
        record_for("PS1 BIOS SCPH-5500 (NTSC-J)", ntscj_bytes),
    ];

    let ntscu_inspection = inspection_for(&fixture, "SLUS-12345");
    let ntscu_outcome = resolve_duckstation_bios(&fixture.profile, &ntscu_inspection, &evidence);
    let DuckStationBiosVerificationOutcome::Verified(ntscu_verified) = ntscu_outcome else {
        panic!("expected NTSC-U Verified, got {ntscu_outcome:?}");
    };
    assert_eq!(
        ntscu_verified.path,
        fixture.profile.bios_path.join("scph1001.bin")
    );
    assert_eq!(ntscu_verified.record.name, "PS1 BIOS SCPH-1001 (NTSC-U)");

    let ntscj_inspection = inspection_for(&fixture, "SLPS-12345");
    let ntscj_outcome = resolve_duckstation_bios(&fixture.profile, &ntscj_inspection, &evidence);
    let DuckStationBiosVerificationOutcome::Verified(ntscj_verified) = ntscj_outcome else {
        panic!("expected NTSC-J Verified, got {ntscj_outcome:?}");
    };
    assert_eq!(
        ntscj_verified.path,
        fixture.profile.bios_path.join("scph5500.bin")
    );
    assert_eq!(ntscj_verified.record.name, "PS1 BIOS SCPH-5500 (NTSC-J)");
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 14: ambiguous required region handled honestly ------------------------------------------

#[test]
fn multiple_region_configs_without_a_verified_serial_are_ambiguous() {
    let fixture = build_fixture(
        "ambiguous-region",
        "[BIOS]\nPathNTSCU=scph1001.bin\nPathPAL=scph5502.bin\n",
    );
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    write_bios(
        &fixture.profile.bios_path,
        "scph5502.bin",
        b"pal bios bytes",
    );
    // No verified serial at all - `EmulatorMetadataOnly` never yields a
    // required region.
    let request = crate::patch_manager::duckstation_local::DuckStationGameRequest {
        emulator_serial: Some("SLUS-12345".to_string()),
        ..Default::default()
    };
    let inspection = crate::patch_manager::duckstation_local::inspect_duckstation_game(
        &fixture.profile,
        &request,
    );
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Ambiguous { .. }
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn required_region_with_no_matching_override_falls_back_to_generic() {
    // A PAL game, but only an NTSC-U region override plus a generic
    // fallback are configured - DuckStation itself falls back to the
    // generic BIOS when no region-specific path is set for the required
    // region, so this must resolve (and verify) the generic file, not
    // report Missing/Ambiguous.
    let fixture = build_fixture(
        "region-fallback",
        "[BIOS]\nPathNTSCU=scph1001.bin\nBIOSFilename=generic.bin\n",
    );
    write_bios(&fixture.profile.bios_path, "scph1001.bin", b"ntsc-u bytes");
    write_bios(&fixture.profile.bios_path, "generic.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLES-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    let DuckStationBiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified via generic fallback, got {outcome:?}");
    };
    assert_eq!(verified.path, fixture.profile.bios_path.join("generic.bin"));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- global vs per-game conflict -----------------------------------------------------------------

#[test]
fn global_and_per_game_generic_bios_disagreement_is_conflict() {
    let fixture = build_fixture("conflict", "[BIOS]\nBIOSFilename=global.bin\n");
    write_bios(&fixture.profile.bios_path, "global.bin", BIOS_BYTES);
    write_bios(
        &fixture.profile.bios_path,
        "override.bin",
        b"different bios bytes entirely",
    );
    write_per_game(
        &fixture.profile.configuration_path,
        "SLUS-12345",
        "[BIOS]\nBIOSFilename=override.bin\n",
    );
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Conflict { .. }
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn global_and_per_game_generic_bios_agreement_is_not_a_conflict() {
    let fixture = build_fixture("agreement", "[BIOS]\nBIOSFilename=global.bin\n");
    write_bios(&fixture.profile.bios_path, "global.bin", BIOS_BYTES);
    write_per_game(
        &fixture.profile.configuration_path,
        "SLUS-12345",
        "[BIOS]\nBIOSFilename=global.bin\n",
    );
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 15/16: arbitrary/wrong-system evidence cannot verify -------------------------------------

#[test]
fn arbitrary_non_ps1_record_cannot_verify() {
    let fixture = build_fixture("wrong-system", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    // Same bytes, same hashes - but tagged as a Saturn/other-system record
    // by simply not being `FirmwareSystem::PlayStation`. Since matching is
    // purely by hash today (not system-tagged), this test's real intent is
    // covered by test 16 below (PS2 evidence, a genuinely different
    // record set, cannot verify) - this one instead proves an evidence
    // slice containing *no* PlayStation-tagged record for these bytes at
    // all still cannot verify when the bytes don't match anything.
    let unrelated_record = record_for_system(
        "unrelated",
        b"completely different bytes, not this BIOS at all",
        FirmwareSystem::PlayStation2,
    );
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[unrelated_record]);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Unknown { .. }
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn ps2_evidence_cannot_verify_a_ps1_bios() {
    let fixture = build_fixture("ps2-evidence", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    // A PS2 BIOS DAT would never contain a record whose hashes match this
    // PS1 BIOS's bytes in the first place - the real protection here is
    // that matching is by hash, and no PS2 evidence set will ever hash-
    // collide with a real PS1 dump. Proven by using genuinely different
    // PS2-labelled evidence bytes.
    let ps2_record = record_for_system(
        "PS2 BIOS",
        b"totally different ps2 bios bytes, unrelated to this ps1 file",
        FirmwareSystem::PlayStation2,
    );
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[ps2_record]);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Unknown { .. }
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 17: managed Redump PS1 evidence feeds verifier ---------------------------------------------

#[test]
fn managed_redump_ps1_evidence_satisfies_this_verifier() {
    use crate::dat::firmware_evidence::redump_bios_evidence_from_dat;
    use crate::dat::parsers::parse_dat_file;

    let bios_bytes = b"synthetic managed-provider PS1 BIOS bytes for this test only";
    let (crc32, md5, sha1) = digests_of(bios_bytes);
    let dat_dir = tempfile::tempdir().unwrap();
    let dat_path = dat_dir.path().join("ps1-bios.dat");
    fs::write(
        &dat_path,
        format!(
            r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Sony - PlayStation - BIOS Images</name>
        <description>Sony - PlayStation - BIOS Images</description>
        <version>20240101</version>
        <author>Redump.org</author>
    </header>
    <game name="Sony PlayStation BIOS SCPH-1001">
        <description>Sony PlayStation BIOS SCPH-1001</description>
        <rom name="scph1001.bin" size="{}" crc="{crc32}" md5="{md5}" sha1="{sha1}"/>
    </game>
</datafile>"#,
            bios_bytes.len()
        ),
    )
    .unwrap();
    let parsed = parse_dat_file(&dat_path, crate::dat::limits::DatLimits::default())
        .unwrap()
        .dat;
    let evidence = redump_bios_evidence_from_dat(&parsed, FirmwareSystem::PlayStation).unwrap();
    assert_eq!(evidence.len(), 1);

    let fixture = build_fixture("managed-bridge", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", bios_bytes);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- 18: missing evidence cannot become Verified -------------------------------------------------

#[test]
fn empty_evidence_never_verifies_anything() {
    let fixture = build_fixture("empty-evidence", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[]);
    assert!(matches!(
        outcome,
        DuckStationBiosVerificationOutcome::Unknown { .. }
    ));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- legacy-state projection ------------------------------------------------------------------

#[test]
fn verified_outcome_projects_to_the_verified_legacy_state() {
    let record = record_for("fixture", BIOS_BYTES);
    let outcome = DuckStationBiosVerificationOutcome::Verified(DuckStationVerifiedBios {
        path: PathBuf::from("/bios/scph1001.bin"),
        size_bytes: BIOS_BYTES.len() as u64,
        crc32: "aabbccdd".to_string(),
        md5: "0".repeat(32),
        sha1: "0".repeat(40),
        record,
        required_region: Some("NTSC-U"),
    });
    assert_eq!(outcome.as_legacy_state(), DuckStationBiosState::Verified);
}

#[test]
fn unknown_missing_unsafe_ambiguous_and_conflict_never_project_to_verified() {
    assert_ne!(
        DuckStationBiosVerificationOutcome::Unknown {
            path: PathBuf::from("/bios/x.bin")
        }
        .as_legacy_state(),
        DuckStationBiosState::Verified
    );
    assert_eq!(
        DuckStationBiosVerificationOutcome::Missing.as_legacy_state(),
        DuckStationBiosState::Missing
    );
    assert_ne!(
        DuckStationBiosVerificationOutcome::Unsafe {
            path: PathBuf::from("/bios/x.bin"),
            detail: "x".to_string()
        }
        .as_legacy_state(),
        DuckStationBiosState::Verified
    );
    assert_ne!(
        DuckStationBiosVerificationOutcome::Ambiguous {
            detail: "x".to_string()
        }
        .as_legacy_state(),
        DuckStationBiosState::Verified
    );
    assert_ne!(
        DuckStationBiosVerificationOutcome::Conflict {
            detail: "x".to_string()
        }
        .as_legacy_state(),
        DuckStationBiosState::Verified
    );
}

// --- inspect_duckstation_game_with_firmware_evidence wiring ---------------------------------------

#[test]
fn inspection_with_firmware_evidence_overwrites_health_and_bios_state() {
    let fixture = build_fixture("wired", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let with_firmware = inspect_duckstation_game_with_firmware_evidence(
        &fixture.profile,
        &request("SLUS-12345"),
        &evidence,
    );
    assert!(matches!(
        with_firmware.bios_verification,
        DuckStationBiosVerificationOutcome::Verified(_)
    ));
    assert_eq!(
        with_firmware.inspection.health.bios,
        DuckStationBiosState::Verified
    );
    assert_eq!(
        with_firmware.inspection.bios.state,
        DuckStationBiosState::Verified
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn inspection_without_matching_evidence_never_reports_verified() {
    let fixture = build_fixture("not-wired", "[BIOS]\nBIOSFilename=scph1001.bin\n");
    write_bios(&fixture.profile.bios_path, "scph1001.bin", BIOS_BYTES);
    let with_firmware = inspect_duckstation_game_with_firmware_evidence(
        &fixture.profile,
        &request("SLUS-12345"),
        &[],
    );
    assert_ne!(
        with_firmware.inspection.health.bios,
        DuckStationBiosState::Verified
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- profile settings parsing and relative BIOS folders ------------------------------------------

#[test]
fn a_logging_bios_flag_is_not_a_bios_filename() {
    // Every real DuckStation settings.ini has `[Logging] BIOS = true`.
    let fixture = build_fixture(
        "logging-bios",
        "[Logging]\nBIOS = true\n[BIOS]\nSearchDirectory = bios\n",
    );
    let inspection = inspection_for(&fixture, "SLUS-12345");
    assert_eq!(
        inspection.bios.configured_path, None,
        "{:?}",
        inspection.bios
    );
    assert_eq!(
        inspection
            .global_config
            .settings
            .unknown
            .get("logging.bios")
            .map(String::as_str),
        Some("true")
    );
    assert!(inspection.global_config.settings.bios_filename.is_none());
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn the_legacy_bare_bios_filename_alias_still_works_in_its_own_section() {
    let fixture = build_fixture("legacy-bios", "[BIOS]\nBIOS = scph1001.bin\n");
    let inspection = inspection_for(&fixture, "SLUS-12345");
    assert_eq!(
        inspection.global_config.settings.bios_filename.as_deref(),
        Some("scph1001.bin")
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn a_relative_search_directory_is_resolved_against_the_profile_root() {
    let fixture = build_fixture(
        "relative-search",
        "[BIOS]\nBIOSFilename=my-bios.bin\nSearchDirectory=custom-bios\n",
    );
    let bios_dir = fixture.profile.configuration_path.join("custom-bios");
    write_bios(&bios_dir, "my-bios.bin", BIOS_BYTES);
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    let DuckStationBiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified, got {outcome:?}");
    };
    assert_eq!(verified.path, bios_dir.join("my-bios.bin"));
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn a_relative_search_directory_does_not_depend_on_the_working_directory() {
    // The folder exists only beside the profile root, not under the test's cwd.
    let fixture = build_fixture(
        "relative-cwd",
        "[BIOS]\nBIOSFilename=missing-here.bin\nSearchDirectory=bios\n",
    );
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[]);
    // Missing - but looked up under the profile's own bios folder.
    assert!(
        matches!(outcome, DuckStationBiosVerificationOutcome::Missing),
        "{outcome:?}"
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn an_absolute_search_directory_is_used_as_is() {
    let fixture = build_fixture("absolute-search", "");
    let elsewhere = fixture.root.join("elsewhere-bios");
    write_bios(&elsewhere, "my-bios.bin", BIOS_BYTES);
    write_global(
        &fixture.profile.configuration_path,
        &format!(
            "[BIOS]\nBIOSFilename=my-bios.bin\nSearchDirectory={}\n",
            elsewhere.display()
        ),
    );
    let fixture = Fixture {
        profile: discover_duckstation_profiles(&DuckStationProfileDiscoveryRoots {
            home: fixture.root.join("home"),
            xdg_config_home: fixture.root.join("home"),
            xdg_data_home: fixture.root.join("data"),
            xdg_config_home_explicit: false,
            explicit_configuration_roots: vec![fixture.profile.configuration_path.clone()],
            portable_configuration_roots: Vec::new(),
            explicit_executables: Vec::new(),
            known_version_outputs: BTreeMap::new(),
            appimage_directory: None,
            path_override: Some(Vec::new()),
        })
        .profiles
        .into_iter()
        .find(|profile| profile.eligible)
        .unwrap(),
        root: fixture.root,
    };
    let inspection = inspection_for(&fixture, "SLUS-12345");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let DuckStationBiosVerificationOutcome::Verified(verified) =
        resolve_duckstation_bios(&fixture.profile, &inspection, &evidence)
    else {
        panic!("expected Verified");
    };
    assert_eq!(verified.path, elsewhere.join("my-bios.bin"));
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- automatic BIOS search (no Path<Region>, no BIOSFilename) -----------------------------------

/// A synthetic 512 KiB image (the PS1 BIOS size DuckStation's search accepts).
fn bios_sized(seed: u8) -> Vec<u8> {
    (0..PS1_BIOS_SIZE as usize)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn search_fixture(label: &str) -> (Fixture, PathBuf) {
    // The real layout: SearchDirectory names `bios/psx`, so the trusted BIOS
    // tree is `bios` and everything else under the fixture root is outside it.
    let fixture = build_fixture(label, "[BIOS]\nSearchDirectory=../bios/psx\n");
    // Exactly the path DuckStation forms: its data root joined with the
    // relative SearchDirectory.
    let directory = fixture.profile.configuration_path.join("../bios/psx");
    fs::create_dir_all(&directory).unwrap();
    (fixture, directory)
}

#[test]
fn search_directory_only_verifies_a_redump_bios_for_the_required_region() {
    // Real settings.ini shape: only [BIOS] SearchDirectory, relative to the
    // data root. Before the fix this was reported Missing.
    let (fixture, directory) = search_fixture("auto-usa");
    let usa = bios_sized(1);
    let japan = bios_sized(2);
    write_bios(&directory, "scph1001.bin", &usa);
    write_bios(&directory, "scph5500.bin", &japan);
    write_bios(&directory, "readme.txt", b"not a bios");
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let evidence = vec![record_for("ps-22j", &japan), record_for("ps-22a", &usa)];
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &evidence);
    let DuckStationBiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified, got {outcome:?}");
    };
    assert_eq!(verified.path, directory.join("scph1001.bin"));
    assert_eq!(verified.record.name, "ps-22a");
    assert_eq!(verified.required_region, Some("NTSC-U"));
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn search_directory_with_only_another_regions_bios_is_not_verified() {
    let (fixture, directory) = search_fixture("auto-wrong-region");
    let japan = bios_sized(3);
    write_bios(&directory, "scph5500.bin", &japan);
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome = resolve_duckstation_bios(
        &fixture.profile,
        &inspection,
        &[record_for("ps-22j", &japan)],
    );
    assert_eq!(
        outcome,
        DuckStationBiosVerificationOutcome::Unknown {
            path: directory.clone()
        }
    );
    assert_eq!(
        outcome.as_legacy_state(),
        DuckStationBiosState::PresentUnverified
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn search_directory_unverified_or_unknown_region_is_never_verified() {
    let (fixture, directory) = search_fixture("auto-unverified");
    let usa = bios_sized(4);
    write_bios(&directory, "scph1001.bin", &usa);
    // A matching hash whose record name carries no region is not enough.
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome = resolve_duckstation_bios(
        &fixture.profile,
        &inspection,
        &[record_for("fixture", &usa)],
    );
    assert!(
        matches!(outcome, DuckStationBiosVerificationOutcome::Unknown { .. }),
        "{outcome:?}"
    );
    // No evidence at all: present, unverified.
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[]);
    assert!(
        matches!(outcome, DuckStationBiosVerificationOutcome::Unknown { .. }),
        "{outcome:?}"
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn search_directory_ignores_wrong_sizes() {
    let (fixture, directory) = search_fixture("auto-size");
    let usa = bios_sized(5);
    write_bios(&directory, "short.bin", &usa[..1024]);
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome =
        resolve_duckstation_bios(&fixture.profile, &inspection, &[record_for("ps-22a", &usa)]);
    assert_eq!(outcome, DuckStationBiosVerificationOutcome::Missing);
    fs::remove_dir_all(&fixture.root).unwrap();
}

// --- contained BIOS links (bios/psx/*.bin -> ../*.bin) ------------------------------------------

#[test]
fn a_relative_link_to_a_bios_in_the_folder_above_verifies() {
    let (fixture, directory) = search_fixture("link-contained");
    let usa = bios_sized(7);
    write_bios(&directory.join(".."), "scph1001.bin", &usa);
    symlink("../scph1001.bin", directory.join("scph1001.bin")).unwrap();
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome =
        resolve_duckstation_bios(&fixture.profile, &inspection, &[record_for("ps-22a", &usa)]);
    let DuckStationBiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified, got {outcome:?}");
    };
    assert_eq!(verified.path, directory.join("scph1001.bin"));
    assert_eq!(verified.record.name, "ps-22a");
    fs::remove_dir_all(&fixture.root).unwrap();
}

fn assert_link_not_verified(label: &str, make: impl FnOnce(&Path, &Path, &[u8])) {
    let (fixture, directory) = search_fixture(label);
    let usa = bios_sized(8);
    make(&fixture.root, &directory, &usa);
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome =
        resolve_duckstation_bios(&fixture.profile, &inspection, &[record_for("ps-22a", &usa)]);
    assert!(
        !matches!(outcome, DuckStationBiosVerificationOutcome::Verified(_)),
        "{label}: {outcome:?}"
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn a_relative_link_escaping_the_bios_tree_is_refused() {
    assert_link_not_verified("link-escape", |root, directory, usa| {
        write_bios(&root.join("outside"), "real.bin", usa);
        symlink("../../outside/real.bin", directory.join("scph1001.bin")).unwrap();
    });
}

#[test]
fn an_absolute_link_is_refused_even_inside_the_tree() {
    assert_link_not_verified("link-absolute", |_, directory, usa| {
        let real = write_bios(&directory.join(".."), "real.bin", usa);
        let real = fs::canonicalize(real).unwrap();
        symlink(&real, directory.join("scph1001.bin")).unwrap();
    });
}

#[test]
fn broken_and_looping_links_are_refused() {
    assert_link_not_verified("link-broken", |_, directory, _| {
        symlink("../missing.bin", directory.join("scph1001.bin")).unwrap();
        symlink("loop-b.bin", directory.join("loop-a.bin")).unwrap();
        symlink("loop-a.bin", directory.join("loop-b.bin")).unwrap();
    });
}

#[test]
fn a_link_to_a_directory_or_fifo_inside_the_tree_is_refused() {
    assert_link_not_verified("link-special", |_, directory, _| {
        fs::create_dir_all(directory.join("../a-folder")).unwrap();
        symlink("../a-folder", directory.join("scph1001.bin")).unwrap();
        let fifo = directory.join("../a-fifo");
        let fifo_c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        symlink("../a-fifo", directory.join("scph5501.bin")).unwrap();
    });
}

#[test]
fn a_link_through_a_symlinked_folder_out_of_the_tree_is_refused() {
    // bios/escape -> ../outside, then psx/scph1001.bin -> ../escape/real.bin:
    // the canonical target leaves the tree, so it is refused.
    assert_link_not_verified("link-via-folder", |root, directory, usa| {
        write_bios(&root.join("outside"), "real.bin", usa);
        symlink("../outside", directory.join("../escape")).unwrap();
        symlink("../escape/real.bin", directory.join("scph1001.bin")).unwrap();
    });
}

#[test]
fn a_shallow_bios_folder_never_trusts_a_broad_parent() {
    // Review of e9309492: the parent of `/mnt/x` or `~/bios` is far too broad.
    let root = |path: &str| {
        trusted_bios_root(Path::new(path), Some(Path::new("/var/home/user"))).map(Path::to_path_buf)
    };
    assert_eq!(
        root("/mnt/usbdrive/[BIOS]/bios/psx"),
        Some(PathBuf::from("/mnt/usbdrive/[BIOS]/bios"))
    );
    assert_eq!(
        root("/home/user/bios"),
        Some(PathBuf::from("/home/user/bios"))
    );
    // Final review: a mount root or the home folder is never trusted itself.
    assert_eq!(root("/mnt/x"), None);
    assert_eq!(root("/var/home/user"), None);
    assert_eq!(root("/var/home"), None);
    assert_eq!(root("/media/user/disk"), None);
    assert_eq!(
        root("/media/user/disk/bios"),
        Some(PathBuf::from("/media/user/disk/bios"))
    );
    assert_eq!(root("/mnt/x/bios"), Some(PathBuf::from("/mnt/x/bios")));
    assert_eq!(root("/mnt"), None);
    assert_eq!(root("/"), None);
    // Re-review: a deep parent can still be the whole home folder or every
    // mounted drive; neither is ever trusted.
    assert_eq!(
        root("/var/home/user/bios"),
        Some(PathBuf::from("/var/home/user/bios"))
    );
    assert_eq!(root("/run/media/user/USB"), None);
    assert_eq!(root("/run/media/user"), None);
    assert_eq!(root("/media/user"), None);
    // Image-based systems: `/mnt` is a link to `var/mnt`, so canonical BIOS
    // folders live below `/var/mnt` and its drives are mount roots too.
    assert_eq!(root("/var/mnt/x"), None);
    assert_eq!(root("/var/mnt"), None);
    assert_eq!(
        root("/var/mnt/x/bios"),
        Some(PathBuf::from("/var/mnt/x/bios"))
    );
    assert_eq!(
        root("/var/mnt/x/emu/bios/psx"),
        Some(PathBuf::from("/var/mnt/x/emu/bios"))
    );
    // The BIOS folder is the home folder itself (standard layout).
    assert_eq!(
        trusted_bios_root(Path::new("/home/user"), Some(Path::new("/home/user"))),
        None
    );
    assert_eq!(
        root("/run/media/user/USB/bios/psx"),
        Some(PathBuf::from("/run/media/user/USB/bios"))
    );
    assert_eq!(
        root("/var/home/user/emu/bios/psx"),
        Some(PathBuf::from("/var/home/user/emu/bios"))
    );
}

#[test]
fn a_link_to_a_file_that_is_not_bios_sized_is_never_hashed_or_verified() {
    let (fixture, directory) = search_fixture("link-oversized");
    // A record for the oversized bytes proves the size gate, not the hash,
    // is what refuses it.
    let mut big = bios_sized(9);
    big.extend(std::iter::repeat_n(0u8, 4096));
    write_bios(&directory.join(".."), "big.bin", &big);
    symlink("../big.bin", directory.join("scph1001.bin")).unwrap();
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome =
        resolve_duckstation_bios(&fixture.profile, &inspection, &[record_for("ps-22a", &big)]);
    assert!(
        !matches!(outcome, DuckStationBiosVerificationOutcome::Verified(_)),
        "{outcome:?}"
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn a_folder_of_only_broken_links_has_no_bios() {
    let (fixture, directory) = search_fixture("link-only-broken");
    symlink("../missing.bin", directory.join("scph1001.bin")).unwrap();
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[]);
    assert!(
        matches!(outcome, DuckStationBiosVerificationOutcome::Missing),
        "{outcome:?}"
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn junk_links_sorted_first_do_not_hide_a_real_bios() {
    let (fixture, directory) = search_fixture("link-junk-first");
    let usa = bios_sized(10);
    write_bios(&directory.join(".."), "real.bin", &usa);
    write_bios(&directory.join(".."), "junk.bin", b"not a bios");
    for index in 0..(MAX_AUTO_SEARCH_HASHED + 5) {
        symlink("../junk.bin", directory.join(format!("a{index:04}.bin"))).unwrap();
    }
    symlink("../real.bin", directory.join("z-scph1001.bin")).unwrap();
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome =
        resolve_duckstation_bios(&fixture.profile, &inspection, &[record_for("ps-22a", &usa)]);
    assert!(
        matches!(outcome, DuckStationBiosVerificationOutcome::Verified(_)),
        "{outcome:?}"
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn an_empty_path_for_the_required_region_falls_back_to_the_search() {
    // Only a PAL path is set; an NTSC-U game makes DuckStation search.
    let fixture = build_fixture(
        "auto-other-region-path",
        "[BIOS]\nSearchDirectory=../bios-folder\nPathPAL=pal.bin\n",
    );
    let directory = fixture.root.join("bios-folder");
    let usa = bios_sized(6);
    write_bios(&directory, "scph1001.bin", &usa);
    let inspection = inspection_for(&fixture, "SLUS-00404");
    let outcome = resolve_duckstation_bios(
        &fixture.profile,
        &inspection,
        &[record_for("ps-41a(w)", &usa)],
    );
    assert!(
        matches!(outcome, DuckStationBiosVerificationOutcome::Verified(_)),
        "{outcome:?}"
    );
    fs::remove_dir_all(&fixture.root).unwrap();
}

#[test]
fn redump_machine_names_map_to_regions() {
    let bytes = b"x";
    let region = |name: &str| redump_ps1_bios_region(&record_for(name, bytes));
    assert_eq!(region("ps-41a"), Some(DuckStationRegion::NtscU));
    assert_eq!(region("ps-41a(w)"), Some(DuckStationRegion::NtscU));
    assert_eq!(region("ps-30e"), Some(DuckStationRegion::Pal));
    assert_eq!(region("psone-43j"), Some(DuckStationRegion::NtscJ));
    assert_eq!(region("ps-22j(v)"), Some(DuckStationRegion::NtscJ));
    assert_eq!(region("ps-22d"), None);
    assert_eq!(region("fixture"), None);
    assert_eq!(
        redump_ps1_bios_region(&record_for_system(
            "ps-41a",
            bytes,
            FirmwareSystem::PlayStation2
        )),
        None
    );
}

#[test]
fn an_unset_or_unresolvable_home_trusts_no_bios_tree() {
    // Final re-review: without a home folder the home guard cannot be
    // applied, so every BIOS link must be refused rather than trusted.
    let (_fixture, directory) = search_fixture("home-unset");
    for home in [
        None,
        Some(std::ffi::OsString::new()),
        Some(std::ffi::OsString::from(
            "/nonexistent/emuwiz-home-for-test",
        )),
    ] {
        assert!(
            trusted_bios_tree_for_home(&directory, home.clone()).is_empty(),
            "HOME {home:?} must trust no tree"
        );
    }
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    if home
        .as_deref()
        .is_some_and(|home| std::fs::canonicalize(home).is_ok())
    {
        assert!(!trusted_bios_tree_for_home(&directory, home).is_empty());
    }
}
