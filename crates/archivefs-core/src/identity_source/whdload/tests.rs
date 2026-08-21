use super::{
    exact_slave_match_observation, inspect_whdload_slave_file, parse_whdload_slave,
    structural_slave_observation,
};
use crate::platform_evidence_fusion::evidence_lineage::{
    ClaimType, EvidenceChannel, LineageRelation, Representation, SourceFamily,
};
use std::path::Path;
use tempfile::tempdir;

fn put16(v: &mut [u8], at: usize, n: u16) {
    v[at..at + 2].copy_from_slice(&n.to_be_bytes());
}
fn put32(v: &mut [u8], at: usize, n: u32) {
    v[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
fn slave(version: u16) -> Vec<u8> {
    let size: usize = match version {
        1..=3 => 30,
        4..=7 => 32,
        8..=9 => 36,
        10..=15 => 42,
        16 => 50,
        17..=19 => 52,
        20 => 54,
        _ => 54,
    };
    let mut code = vec![0; (size + 64).next_multiple_of(4)];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    put16(&mut code, 12, version);
    put16(&mut code, 14, 3);
    put32(&mut code, 16, 524288);
    put32(&mut code, 20, 1);
    put32(&mut code, 24, 2);
    if size >= 30 {
        put16(&mut code, 28, 0);
    }
    if size >= 36 {
        put16(&mut code, 34, size as u16);
    }
    if size >= 42 {
        put16(&mut code, 36, (size + 8) as u16);
        put16(&mut code, 38, (size + 16) as u16);
        put16(&mut code, 40, (size + 24) as u16);
    }
    if size >= 50 {
        put32(&mut code, 42, 512 * 1024);
        put16(&mut code, 46, 0x1234);
        put16(&mut code, 48, (size + 32) as u16);
    }
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
fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}
#[test]
fn valid_supported_slave_parses() {
    let p = parse_whdload_slave(&slave(20)).unwrap();
    assert_eq!(p.runtime_version, 20);
    assert_eq!(p.name.as_deref(), Some("Game"));
}
#[test]
fn every_size_boundary_is_supported() {
    for v in [1, 3, 4, 7, 8, 9, 10, 15, 16, 17, 19, 20] {
        assert_eq!(parse_whdload_slave(&slave(v)).unwrap().runtime_version, v)
    }
}
#[test]
fn future_version_fails_closed() {
    assert!(parse_whdload_slave(&slave(21)).is_err())
}
#[test]
fn hunk_header_required() {
    let mut b = slave(20);
    b[..4].copy_from_slice(&0_u32.to_be_bytes());
    assert!(parse_whdload_slave(&b).is_err())
}
#[test]
fn security_and_id_required() {
    let mut b = slave(20);
    b[32] = 0;
    assert!(parse_whdload_slave(&b).is_err());
    let mut b = slave(20);
    b[36] = 0;
    assert!(parse_whdload_slave(&b).is_err())
}
#[test]
fn truncated_and_bad_hunk_fail() {
    assert!(parse_whdload_slave(&[0; 3]).is_err());
    let mut b = slave(20);
    b.truncate(36);
    assert!(parse_whdload_slave(&b).is_err())
}
#[test]
fn rptr_bounds_and_unterminated_fail() {
    let mut b = slave(8);
    put16(&mut b, 32 + 34, 0xffff);
    assert!(parse_whdload_slave(&b).is_err());
    let mut b = slave(8);
    let start = 32 + 36;
    for x in &mut b[start..] {
        *x = b'A'
    }
    assert!(parse_whdload_slave(&b).is_err())
}
#[test]
fn kickstart_metadata_v16_plus() {
    let p = parse_whdload_slave(&slave(16)).unwrap();
    assert_eq!(p.kick_name.as_deref(), Some("Kick"));
    assert_eq!(p.kick_size, Some(512 * 1024));
    assert_eq!(p.kick_crc, Some(0x1234));
    assert_eq!(p.config.as_deref(), Some("Config"));
}
#[test]
fn artifact_hashes_and_evidence_are_whole_slave_only() {
    let d = tempdir().unwrap();
    let a = inspect_whdload_slave_file(&write(d.path(), "Anything_1649.hdf", &slave(20))).unwrap();
    assert_eq!(a.hashes.sha1.len(), 40);
    assert_eq!(a.hashes.sha256.len(), 64);
    let structural = structural_slave_observation(&a);
    assert_eq!(structural.platform_candidate.as_deref(), Some("Amiga"));
    assert_eq!(
        structural.provenance.representation,
        Representation::WHDLoadSlave
    );
    assert_eq!(structural.provenance.channel, EvidenceChannel::LocalWHDLoad);
    assert_eq!(structural.provenance.upstream_source, SourceFamily::WHDLoad);
    assert_eq!(structural.provenance.lineage, LineageRelation::Independent);
    assert_eq!(structural.provenance.upstream_version, None);
    let exact = exact_slave_match_observation(&a, Some("Game".into()));
    assert_eq!(exact.claim, ClaimType::ExactSlaveMatch);
}
#[test]
fn filename_never_changes_identity() {
    let d = tempdir().unwrap();
    let a = inspect_whdload_slave_file(&write(d.path(), "GoldenAxe_v1.4_0017.hdf", &slave(20)))
        .unwrap();
    assert!(
        structural_slave_observation(&a)
            .notes
            .unwrap()
            .contains("runtime")
    );
    assert_eq!(
        exact_slave_match_observation(&a, None).release_candidate,
        None
    )
}
