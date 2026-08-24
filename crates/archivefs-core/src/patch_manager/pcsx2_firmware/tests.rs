//! No network anywhere in this suite, and no real Redump hash is embedded -
//! every hash below is computed at test time from synthetic byte content
//! this test itself invented, never a value copied from a real BIOS dump or
//! a real Redump DAT.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::dat::firmware_evidence::FirmwareSystem;
use crate::dat::model::DatEcosystem;
use crate::patch_manager::pcsx2_local::Pcsx2Settings;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn fixture_root(label: &str) -> PathBuf {
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "archivefs-pcsx2-firmware-{label}-{}-{sequence}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

const BIOS_BYTES: &[u8] = b"synthetic test BIOS bytes, not a real dump, repeated to pad size...";

fn write_bios(bios_root: &std::path::Path, name: &str, bytes: &[u8]) -> PathBuf {
    fs::create_dir_all(bios_root).unwrap();
    let path = bios_root.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

/// Hashes `bytes` with the exact same algorithms `hash_bios_file` uses, so
/// a fixture's evidence record is guaranteed self-consistent without this
/// test ever hand-typing a digest.
fn digests_of(bytes: &[u8]) -> (String, String, String) {
    use md5::Md5;
    use sha1::Sha1;
    use sha1::digest::Digest;
    let crc32 = Crc32::of(bytes);
    let md5 = hex(&Md5::digest(bytes));
    let sha1 = hex(&Sha1::digest(bytes));
    (crc32, md5, sha1)
}

fn record_for(name: &str, bytes: &[u8]) -> FirmwareIdentityRecord {
    let (crc32, md5, sha1) = digests_of(bytes);
    FirmwareIdentityRecord {
        system: FirmwareSystem::PlayStation2,
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

fn empty_global_config() -> Pcsx2Config {
    Pcsx2Config {
        path: PathBuf::from("/nonexistent/PCSX2.ini"),
        exists: false,
        readable: false,
        settings: Pcsx2Settings::default(),
        warnings: Vec::new(),
    }
}

fn configured_global_config(bios_filename: &str) -> Pcsx2Config {
    let mut unknown = BTreeMap::new();
    unknown.insert("Filenames/BIOS".to_string(), bios_filename.to_string());
    // `Pcsx2Settings::controller_sections` is private to `pcsx2_local`, so
    // this fills it via `Default` through a mutation rather than naming it
    // in a struct-update literal (which the compiler checks privacy on just
    // as strictly as a named field).
    let mut settings = Pcsx2Settings::default();
    settings.unknown = unknown;
    Pcsx2Config {
        path: PathBuf::from("/nonexistent/PCSX2.ini"),
        exists: true,
        readable: true,
        settings,
        warnings: Vec::new(),
    }
}

// --- 1: known valid hash -> Verified -----------------------------------------------------------

#[test]
fn matching_bios_becomes_verified() {
    let root = fixture_root("verified");
    let bios_root = root.join("bios");
    let path = write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    let evidence = vec![record_for("Sony PlayStation 2 BIOS v02.20", BIOS_BYTES)];

    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &evidence);
    let Pcsx2BiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified, got {outcome:?}");
    };
    assert_eq!(verified.path, path);
    assert_eq!(verified.size_bytes, BIOS_BYTES.len() as u64);
    assert_eq!(verified.record.name, "Sony PlayStation 2 BIOS v02.20");
    assert_eq!(verified.record.provider, DatEcosystem::Redump);
    fs::remove_dir_all(root).unwrap();
}

// --- 2/3/4/5: mismatches never verify -----------------------------------------------------------

#[test]
fn sha1_mismatch_is_not_verified() {
    let root = fixture_root("sha1-mismatch");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    let mut record = record_for("fixture", BIOS_BYTES);
    record.sha1 = "0".repeat(40);
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &[record]);
    assert!(!matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn md5_mismatch_is_not_verified() {
    let root = fixture_root("md5-mismatch");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    let mut record = record_for("fixture", BIOS_BYTES);
    record.md5 = "0".repeat(32);
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &[record]);
    assert!(!matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn crc32_mismatch_is_not_verified() {
    let root = fixture_root("crc32-mismatch");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    let mut record = record_for("fixture", BIOS_BYTES);
    record.crc32 = "00000000".to_string();
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &[record]);
    assert!(!matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn size_mismatch_is_not_verified() {
    let root = fixture_root("size-mismatch");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    let mut record = record_for("fixture", BIOS_BYTES);
    record.size_bytes += 1;
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &[record]);
    assert!(!matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

// --- 6: correct-looking filename with wrong hashes -> Unknown -----------------------------------

#[test]
fn plausible_filename_with_wrong_hash_is_unknown_not_verified() {
    let root = fixture_root("plausible-filename");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "SCPH-70012_verified.bin", BIOS_BYTES);
    // Evidence for a completely different BIOS - the filename alone must
    // never bridge the gap.
    let record = record_for("unrelated", b"totally different bytes");
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &[record]);
    assert_eq!(
        outcome,
        Pcsx2BiosVerificationOutcome::Unknown {
            path: bios_root.join("SCPH-70012_verified.bin")
        }
    );
    fs::remove_dir_all(root).unwrap();
}

// --- 7: symlink BIOS -> blocked -----------------------------------------------------------------

#[test]
fn symlink_bios_is_unsafe() {
    let root = fixture_root("symlink");
    let bios_root = root.join("bios");
    let real = write_bios(&bios_root, "real.bin", BIOS_BYTES);
    let link = bios_root.join("scph-70012.bin");
    symlink(&real, &link).unwrap();
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_pcsx2_bios(
        &bios_root,
        &configured_global_config("scph-70012.bin"),
        &evidence,
    );
    assert!(matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Unsafe { .. }
    ));
    fs::remove_dir_all(root).unwrap();
}

// --- 8: directory/non-regular BIOS -> blocked ----------------------------------------------------

#[test]
fn directory_bios_is_unsafe() {
    let root = fixture_root("directory");
    let bios_root = root.join("bios");
    fs::create_dir_all(bios_root.join("scph-70012.bin")).unwrap();
    let outcome = resolve_pcsx2_bios(&bios_root, &configured_global_config("scph-70012.bin"), &[]);
    assert!(matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Unsafe { .. }
    ));
    fs::remove_dir_all(root).unwrap();
}

// --- 9: missing BIOS -> Missing -------------------------------------------------------------------

#[test]
fn missing_bios_is_missing() {
    let root = fixture_root("missing");
    let bios_root = root.join("bios");
    fs::create_dir_all(&bios_root).unwrap();
    let outcome = resolve_pcsx2_bios(&bios_root, &configured_global_config("scph-70012.bin"), &[]);
    assert_eq!(outcome, Pcsx2BiosVerificationOutcome::Missing);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_bios_directory_is_missing() {
    let root = fixture_root("missing-dir");
    let bios_root = root.join("bios");
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &[]);
    assert_eq!(outcome, Pcsx2BiosVerificationOutcome::Missing);
    fs::remove_dir_all(root).unwrap();
}

// --- 10: unreadable BIOS -----------------------------------------------------------------------

#[test]
fn unreadable_bios_permission_denied() {
    let root = fixture_root("unreadable");
    let bios_root = root.join("bios");
    let path = write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let outcome = resolve_pcsx2_bios(&bios_root, &configured_global_config("scph-70012.bin"), &[]);
    // Running as root can bypass the permission bit entirely, in which case
    // this legitimately succeeds in reading the file - only assert the
    // refusal when it actually happened.
    if !nix_running_as_root() {
        assert!(matches!(
            outcome,
            Pcsx2BiosVerificationOutcome::Unreadable { .. }
        ));
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn nix_running_as_root() -> bool {
    // SAFETY: `geteuid` takes no arguments and has no failure mode.
    unsafe { libc::geteuid() == 0 }
}

// --- 11: correct Redump record metadata retained ------------------------------------------------

#[test]
fn matched_record_metadata_is_retained() {
    let root = fixture_root("metadata");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    let mut record = record_for(
        "Sony PlayStation 2 BIOS v02.20(10/02/2005) Console",
        BIOS_BYTES,
    );
    record.description = Some("Sony PlayStation 2 BIOS v02.20(10/02/2005) Console".to_string());
    record.dat_version = Some("20240315".to_string());
    let outcome = resolve_pcsx2_bios(
        &bios_root,
        &configured_global_config("scph-70012.bin"),
        &[record.clone()],
    );
    let Pcsx2BiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified");
    };
    assert_eq!(verified.record, record);
    fs::remove_dir_all(root).unwrap();
}

// --- 8/9 from the task list: multiple candidates -------------------------------------------------

#[test]
fn one_verified_among_several_candidates_is_selected() {
    let root = fixture_root("multi-one-verified");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "a.bin", b"junk file, never matches anything");
    let path = write_bios(&bios_root, "b.bin", BIOS_BYTES);
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &evidence);
    let Pcsx2BiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified, got {outcome:?}");
    };
    assert_eq!(verified.path, path);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn multiple_conflicting_verified_candidates_are_ambiguous() {
    let root = fixture_root("multi-ambiguous");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "a.bin", BIOS_BYTES);
    write_bios(
        &bios_root,
        "b.bin",
        b"a different, also-catalogued BIOS dump entirely",
    );
    let evidence = vec![
        record_for("fixture-a", BIOS_BYTES),
        record_for(
            "fixture-b",
            b"a different, also-catalogued BIOS dump entirely",
        ),
    ];
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &evidence);
    assert!(matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Ambiguous { .. }
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn no_candidate_verifies_among_several_stays_unknown() {
    let root = fixture_root("multi-none-verified");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "a.bin", b"unrelated bytes one");
    write_bios(&bios_root, "b.bin", b"unrelated bytes two, longer content");
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &[]);
    assert!(matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Unknown { .. }
    ));
    fs::remove_dir_all(root).unwrap();
}

// --- configuration-selected BIOS bypasses candidate ambiguity entirely --------------------------

#[test]
fn configured_selection_resolves_even_with_other_candidates_present() {
    let root = fixture_root("configured-selection");
    let bios_root = root.join("bios");
    let selected = write_bios(&bios_root, "selected.bin", BIOS_BYTES);
    write_bios(&bios_root, "other.bin", b"a different unrelated dump");
    let evidence = vec![record_for("fixture", BIOS_BYTES)];
    let outcome = resolve_pcsx2_bios(
        &bios_root,
        &configured_global_config("selected.bin"),
        &evidence,
    );
    let Pcsx2BiosVerificationOutcome::Verified(verified) = outcome else {
        panic!("expected Verified, got {outcome:?}");
    };
    assert_eq!(verified.path, selected);
    fs::remove_dir_all(root).unwrap();
}

// --- 17: multiple identical matching records handled deterministically --------------------------

#[test]
fn identical_duplicate_records_pick_the_same_one_deterministically() {
    let root = fixture_root("duplicate-records");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    let first = record_for("aaa-first-alphabetically", BIOS_BYTES);
    let second = record_for("zzz-second-alphabetically", BIOS_BYTES);
    let evidence_forward = vec![first.clone(), second.clone()];
    let evidence_reversed = vec![second, first];
    let a = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &evidence_forward);
    let b = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &evidence_reversed);
    assert_eq!(
        a, b,
        "record order in the evidence slice must not change the outcome"
    );
    let Pcsx2BiosVerificationOutcome::Verified(verified) = a else {
        panic!("expected Verified");
    };
    assert_eq!(verified.record.name, "aaa-first-alphabetically");
    fs::remove_dir_all(root).unwrap();
}

// --- no network / no embedded records -------------------------------------------------------------

#[test]
fn empty_evidence_never_verifies_anything() {
    let root = fixture_root("empty-evidence");
    let bios_root = root.join("bios");
    write_bios(&bios_root, "scph-70012.bin", BIOS_BYTES);
    let outcome = resolve_pcsx2_bios(&bios_root, &empty_global_config(), &[]);
    assert!(!matches!(
        outcome,
        Pcsx2BiosVerificationOutcome::Verified(_)
    ));
    fs::remove_dir_all(root).unwrap();
}

// --- readiness projection ------------------------------------------------------------------------

#[test]
fn verified_outcome_projects_to_the_verified_legacy_state() {
    let record = record_for("fixture", BIOS_BYTES);
    let outcome = Pcsx2BiosVerificationOutcome::Verified(Pcsx2VerifiedBios {
        path: PathBuf::from("/bios/scph-70012.bin"),
        size_bytes: BIOS_BYTES.len() as u64,
        crc32: record.crc32.clone(),
        md5: record.md5.clone(),
        sha1: record.sha1.clone(),
        record,
    });
    assert_eq!(outcome.as_legacy_state(), Pcsx2BiosVerification::Verified);
}

#[test]
fn unknown_ambiguous_and_unsafe_never_project_to_verified() {
    assert_ne!(
        Pcsx2BiosVerificationOutcome::Unknown {
            path: PathBuf::from("/bios/x.bin")
        }
        .as_legacy_state(),
        Pcsx2BiosVerification::Verified
    );
    assert_ne!(
        Pcsx2BiosVerificationOutcome::Ambiguous {
            detail: "x".to_string()
        }
        .as_legacy_state(),
        Pcsx2BiosVerification::Verified
    );
    assert_ne!(
        Pcsx2BiosVerificationOutcome::Unsafe {
            path: PathBuf::from("/bios/x.bin"),
            detail: "x".to_string()
        }
        .as_legacy_state(),
        Pcsx2BiosVerification::Verified
    );
    assert_ne!(
        Pcsx2BiosVerificationOutcome::Missing.as_legacy_state(),
        Pcsx2BiosVerification::Verified
    );
}

/// Proves the managed Redump provider bridge end to end: a synthetic
/// "downloaded" Redump PS2 BIOS DAT is parsed with the real DAT parser and
/// turned into [`FirmwareIdentityRecord`]s via
/// `crate::dat::firmware_evidence::redump_bios_evidence_from_dat` - the
/// exact function `update_redump_bios` (in `crate::dat::updates`) calls
/// before promoting a managed snapshot - and that evidence genuinely
/// verifies a real on-disk BIOS file through the unchanged
/// [`resolve_pcsx2_bios`] entry point. No network, no embedded real Redump
/// hash, no PCSX2-specific code touched to make this work: the existing
/// generic `&[FirmwareIdentityRecord]` parameter is all that was ever
/// needed.
#[test]
fn managed_redump_ps2_evidence_satisfies_the_existing_pcsx2_bios_matcher() {
    use crate::dat::firmware_evidence::redump_bios_evidence_from_dat;
    use crate::dat::parsers::parse_dat_file;

    let bios_bytes = b"synthetic managed-provider BIOS bytes for this test only";
    let (crc32, md5, sha1) = digests_of(bios_bytes);
    let dat_dir = tempfile::tempdir().unwrap();
    let dat_path = dat_dir.path().join("ps2-bios.dat");
    fs::write(
        &dat_path,
        format!(
            r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Sony - PlayStation 2 - BIOS Images</name>
        <description>Sony - PlayStation 2 - BIOS Images</description>
        <version>20240101</version>
        <author>Redump.org</author>
    </header>
    <game name="Sony PlayStation 2 BIOS v02.20 Console">
        <description>Sony PlayStation 2 BIOS v02.20 Console</description>
        <rom name="scph-70012.bin" size="{}" crc="{crc32}" md5="{md5}" sha1="{sha1}"/>
    </game>
</datafile>"#,
            bios_bytes.len()
        ),
    )
    .unwrap();
    let parsed = parse_dat_file(&dat_path, crate::dat::limits::DatLimits::default())
        .unwrap()
        .dat;
    let evidence = redump_bios_evidence_from_dat(&parsed, FirmwareSystem::PlayStation2).unwrap();
    assert_eq!(evidence.len(), 1);

    let root = fixture_root("managed-bridge");
    write_bios(&root, "scph-70012.bin", bios_bytes);
    let outcome = resolve_pcsx2_bios(&root, &empty_global_config(), &evidence);
    assert!(matches!(outcome, Pcsx2BiosVerificationOutcome::Verified(_)));
}
