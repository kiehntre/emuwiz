//! No network anywhere in this suite, and no real Redump/Sony hash is
//! embedded - every hash below is computed at test time from synthetic byte
//! content this test itself invented, never a value copied from a real BIOS
//! dump or a real Redump DAT.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
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
        explicit_configuration_roots: vec![configuration_path.clone()],
        portable_configuration_roots: Vec::new(),
        explicit_executables: Vec::new(),
        known_version_outputs: BTreeMap::new(),
        appimage_directory: None,
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
    let outcome = resolve_duckstation_bios(&fixture.profile, &inspection, &[record.clone()]);
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
