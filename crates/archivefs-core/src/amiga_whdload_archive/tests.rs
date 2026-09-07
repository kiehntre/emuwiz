use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::*;
use crate::dat::archive::lha::LhaProvider;
use crate::dat::archive::limits::ArchiveLimits;
use crate::safe_read::TrustedRoots;

// --- LHA archive fixture builder ----------------------------------------
//
// Hand-built exactly like `dat::archive::lha`'s own test fixture (a real,
// minimal, stored/`-lh0-` LHA a genuine 7-Zip can list and extract), just
// generalised to hold more than one member so multi-slave tests do not
// need a shell archiver.

fn lha_entry(name: &str, payload: &[u8]) -> Vec<u8> {
    assert!(name.len() <= u8::MAX as usize);
    let header_size = name.len() + 23;
    assert!(header_size <= u8::MAX as usize);
    let mut bytes = vec![header_size as u8, 0];
    bytes.extend_from_slice(b"-lh0-");
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.push(0x20); // archive attribute
    bytes.push(0); // level 0
    bytes.push(name.len() as u8);
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(&lha_crc16(payload).to_le_bytes());
    bytes.push(0); // host OS
    bytes[1] = bytes[2..]
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
    bytes.extend_from_slice(payload);
    bytes
}

fn stored_lha(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, payload) in entries {
        bytes.extend(lha_entry(name, payload));
    }
    bytes.push(0); // end of archive
    bytes
}

fn lha_crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0_u16;
    for byte in bytes {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xa001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

// --- WHDLoad `.slave` fixture builder ------------------------------------
//
// A minimal, real, structurally valid runtime-v20 slave - the same
// technique `identity_source::whdload::tests::slave` uses.

fn put16(v: &mut [u8], at: usize, n: u16) {
    v[at..at + 2].copy_from_slice(&n.to_be_bytes());
}
fn put32(v: &mut [u8], at: usize, n: u32) {
    v[at..at + 4].copy_from_slice(&n.to_be_bytes());
}

fn valid_slave_bytes() -> Vec<u8> {
    let size: usize = 54; // version 20
    let mut code = vec![0; (size + 64).next_multiple_of(4)];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    put16(&mut code, 12, 20);
    put16(&mut code, 14, 3);
    put32(&mut code, 16, 524288);
    put32(&mut code, 20, 1);
    put32(&mut code, 24, 2);
    put16(&mut code, 28, 0);
    put16(&mut code, 34, size as u16);
    put16(&mut code, 36, (size + 8) as u16);
    put16(&mut code, 38, (size + 16) as u16);
    put16(&mut code, 40, (size + 24) as u16);
    put32(&mut code, 42, 512 * 1024);
    put16(&mut code, 46, 0x1234);
    put16(&mut code, 48, (size + 32) as u16);
    code[size..size + 5].copy_from_slice(b"Game\0");
    code[size + 8..size + 13].copy_from_slice(b"Copy\0");
    code[size + 16..size + 21].copy_from_slice(b"Info\0");
    code[size + 24..size + 29].copy_from_slice(b"Kick\0");
    code[size + 32..size + 39].copy_from_slice(b"Config\0");
    let mut out = Vec::new();
    for n in [
        0x3f3_u32,
        0,
        1,
        0,
        0,
        (code.len() / 4) as u32,
        0x3e9,
        (code.len() / 4) as u32,
    ] {
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend_from_slice(&code);
    out.extend_from_slice(&0x3f2_u32.to_be_bytes());
    out
}

/// Every real test in this module needs the same optional local backend
/// `dat::archive::lha` itself depends on; skip (not fail) when it is
/// unavailable, exactly like that module's own tests do.
fn provider() -> Option<LhaProvider> {
    LhaProvider::discover(Duration::from_secs(10)).ok()
}

fn write_archive(dir: &Path, name: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, stored_lha(entries)).unwrap();
    path
}

#[test]
fn valid_single_slave_is_the_one_candidate() {
    let Some(_provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(
        dir.path(),
        "game.lha",
        &[("Game/Game.Slave", &valid_slave_bytes())],
    );
    let cancel = AtomicBool::new(false);
    let result = discover_whdload_slaves_in_archive(&archive, &cancel).unwrap();
    assert_eq!(result.candidates.len(), 1);
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.candidates[0].member_path, "Game/Game.Slave");
    assert_eq!(result.candidates[0].artifact.parsed.runtime_version, 20);
}

#[test]
fn no_slave_member_yields_no_candidates() {
    let Some(_provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(dir.path(), "data.lha", &[("Readme.txt", b"just docs")]);
    let cancel = AtomicBool::new(false);
    let result = discover_whdload_slaves_in_archive(&archive, &cancel).unwrap();
    assert!(result.candidates.is_empty());
    assert!(result.diagnostics.is_empty());
    assert_eq!(result.total_members, 1);
}

#[test]
fn malformed_slave_becomes_a_diagnostic_not_a_candidate() {
    let Some(_provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(
        dir.path(),
        "bad.lha",
        &[("Game/Broken.slave", b"not a real slave")],
    );
    let cancel = AtomicBool::new(false);
    let result = discover_whdload_slaves_in_archive(&archive, &cancel).unwrap();
    assert!(result.candidates.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].member_path, "Game/Broken.slave");
}

#[test]
fn valid_and_malformed_slave_coexist() {
    let Some(_provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(
        dir.path(),
        "mixed.lha",
        &[
            ("Game/Game.Slave", &valid_slave_bytes()),
            ("Game/Broken.slave", b"garbage"),
        ],
    );
    let cancel = AtomicBool::new(false);
    let result = discover_whdload_slaves_in_archive(&archive, &cancel).unwrap();
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0].member_path, "Game/Game.Slave");
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].member_path, "Game/Broken.slave");
}

#[test]
fn multiple_valid_slaves_are_all_preserved_never_auto_picked() {
    let Some(_provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(
        dir.path(),
        "two.lha",
        &[
            ("Game_v1.Slave", &valid_slave_bytes()),
            ("Game_v2.Slave", &valid_slave_bytes()),
        ],
    );
    let cancel = AtomicBool::new(false);
    let result = discover_whdload_slaves_in_archive(&archive, &cancel).unwrap();
    assert_eq!(result.candidates.len(), 2);
    let names: std::collections::BTreeSet<_> = result
        .candidates
        .iter()
        .map(|candidate| candidate.member_path.as_str())
        .collect();
    assert_eq!(
        names,
        std::collections::BTreeSet::from(["Game_v1.Slave", "Game_v2.Slave"])
    );
}

#[test]
fn traversal_member_name_is_refused_not_extracted() {
    let Some(_provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(
        dir.path(),
        "evil.lha",
        &[("../evil.slave", &valid_slave_bytes())],
    );
    let cancel = AtomicBool::new(false);
    let result = discover_whdload_slaves_in_archive(&archive, &cancel).unwrap();
    assert!(
        result.candidates.is_empty(),
        "a traversal member path must never become a candidate"
    );
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].detail.contains("could not be read"));
}

#[test]
fn oversized_member_is_refused_before_extraction() {
    // Exercises the exact bounded-read primitive `discover_whdload_slaves_in_archive`
    // relies on (`LhaArchiveSource::read_member`'s own size check), with a
    // caller-supplied bound far smaller than the production
    // `MAX_INDIVIDUAL_SLAVE_BYTES` ceiling so the test fixture stays tiny.
    let Some(provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let payload = vec![0_u8; 4096];
    let archive = write_archive(dir.path(), "big.lha", &[("Game.Slave", &payload)]);
    let trusted = TrustedRoots::from_paths(std::iter::once(dir.path()));
    let source = provider
        .open(
            &archive,
            &trusted,
            ArchiveLimits::default(),
            Duration::from_secs(10),
        )
        .unwrap();
    let cancel = AtomicBool::new(false);
    let result = source.read_member("Game.Slave", 1024, &cancel);
    assert_eq!(
        result,
        Err(crate::dat::archive::lha::LhaError::RefusedLimits {
            reason: "member size"
        })
    );
}

#[test]
fn excessive_member_count_is_refused_before_listing_succeeds() {
    // Same mechanism `discover_whdload_slaves_in_archive` composes with
    // (`ArchiveLimits::max_members`, already covering the production
    // ceiling's own tests) - exercised with a deliberately tiny custom
    // limit so the fixture does not need thousands of entries.
    let Some(provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(
        dir.path(),
        "many.lha",
        &[("A.Slave", b"a"), ("B.Slave", b"b")],
    );
    let trusted = TrustedRoots::from_paths(std::iter::once(dir.path()));
    let tight_limits = ArchiveLimits {
        max_members: 1,
        ..ArchiveLimits::default()
    };
    let result = provider.open(&archive, &trusted, tight_limits, Duration::from_secs(10));
    assert_eq!(
        result.err(),
        Some(crate::dat::archive::lha::LhaError::RefusedLimits {
            reason: "member count"
        })
    );
}

#[test]
fn archive_filename_is_never_used_as_slave_identity() {
    let Some(_provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(
        dir.path(),
        "totally_unrelated_filename.lha",
        &[("Game/Game.Slave", &valid_slave_bytes())],
    );
    let cancel = AtomicBool::new(false);
    let result = discover_whdload_slaves_in_archive(&archive, &cancel).unwrap();
    assert_eq!(result.candidates.len(), 1);
    let candidate = &result.candidates[0];
    // The slave's own structural `name` field ("Game") never equals, and is
    // never derived from, the archive's own filename.
    assert_eq!(candidate.artifact.parsed.name.as_deref(), Some("Game"));
    assert_ne!(
        candidate.artifact.parsed.name.as_deref(),
        archive.file_stem().and_then(|s| s.to_str())
    );
    // The archive's own path is retained as container provenance only.
    assert_eq!(result.archive_path, archive);
}

#[test]
fn structural_evidence_reuses_the_existing_whdload_observation_helper() {
    let Some(_provider) = provider() else { return };
    let dir = tempfile::tempdir().unwrap();
    let archive = write_archive(
        dir.path(),
        "game.lha",
        &[("Game/Game.Slave", &valid_slave_bytes())],
    );
    let cancel = AtomicBool::new(false);
    let result = discover_whdload_slaves_in_archive(&archive, &cancel).unwrap();
    let observation = crate::identity_source::whdload::structural_slave_observation(
        &result.candidates[0].artifact,
    );
    assert_eq!(observation.platform_candidate.as_deref(), Some("Amiga"));
}

#[test]
fn backend_unavailable_is_reported_distinctly_from_a_bad_archive() {
    // Cannot force a real absence of 7-Zip in an environment where it is
    // installed; this just proves the two failure shapes stay distinct
    // types, which the discovery layer depends on to fail soft only for
    // `BackendUnavailable`.
    assert_ne!(
        WhdloadArchiveError::BackendUnavailable,
        WhdloadArchiveError::Open {
            detail: "x".to_string()
        }
    );
}
