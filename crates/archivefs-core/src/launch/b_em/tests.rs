use super::*;
use crate::launch::planning::ResolvedIdentity;
use crate::launch::readiness::LaunchReadiness;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{PermissionsExt, symlink};
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    profile: BEmProfile,
    media: BEmMedia,
}

/// A structurally valid 40-track single-sided DFS image (the same shape main's
/// own DFS parser tests use): catalogue sectors, one entry, matching geometry.
fn ssd_bytes() -> Vec<u8> {
    let total_sectors: u16 = 400;
    let mut image = vec![0_u8; usize::from(total_sectors) * 256];
    image[0..8].copy_from_slice(b"EXAMPLE ");
    image[256..260].fill(b' ');
    image[260] = 0x12; // valid BCD catalogue cycle number
    image[261] = 8; // one eight-byte entry
    image[262] = ((total_sectors >> 8) & 3) as u8;
    image[263] = total_sectors as u8;
    image[8..15].copy_from_slice(b"!BOOT  ");
    image[15] = b'$' | 0x80;
    let details = 256 + 8;
    image[details..details + 2].copy_from_slice(&0x1900_u16.to_le_bytes());
    image[details + 2..details + 4].copy_from_slice(&0x1900_u16.to_le_bytes());
    image[details + 4..details + 6].copy_from_slice(&3_u16.to_le_bytes());
    image[details + 6] = 0;
    image[details + 7] = 2;
    image
}

fn uef_bytes() -> Vec<u8> {
    let mut bytes = b"UEF File!\0\x0a\x00".to_vec();
    bytes.extend(vec![0u8; 64]);
    bytes
}

impl Fixture {
    fn new(format: BEmMediaFormat) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("b-em");
        std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bytes = if format == BEmMediaFormat::Uef {
            uef_bytes()
        } else {
            ssd_bytes()
        };
        let path = temp.path().join(match format {
            BEmMediaFormat::Uef => "Tape name with spaces ünï.uef",
            BEmMediaFormat::Dsd => "disc.dsd",
            _ => "Dísc – 日本.ssd",
        });
        std::fs::write(&path, &bytes).unwrap();
        Self {
            media: BEmMedia {
                path,
                format,
                identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                    platform_id: PLATFORM_ID.into(),
                    game_key: "synthetic".into(),
                }),
                platform_evidence: LocalEvidenceStrength::Verified,
                verified_source_sha256: Sha256::digest(&bytes).into(),
            },
            profile: BEmProfile {
                executable: exe,
                machine: Some(BEmMachine::ModelB),
                disposable_session_acknowledged: true,
            },
            temp,
        }
    }
}

#[test]
fn classification_is_preview_only_and_offers_no_launch() {
    assert_eq!(
        CLASSIFICATION,
        NativeAdapterClassification::PreviewReadinessOnly
    );
    let f = Fixture::new(BEmMediaFormat::Uef);
    let preview = preview_b_em(&f.profile, &f.media).unwrap();
    assert_eq!(
        preview.classification(),
        NativeAdapterClassification::PreviewReadinessOnly
    );
    assert_eq!(preview.readiness(), LaunchReadiness::Blocked);
    assert!(!preview.is_launchable());
    assert!(preview.unproven().iter().any(|r| r.contains("-cfg")));
    assert!(preview.unproven().iter().any(|r| r.contains("CMOS")));
    assert!(preview.is_fresh());
}

#[test]
fn ssd_dsd_and_uef_validate_with_spaces_and_unicode_paths() {
    for format in [BEmMediaFormat::Ssd, BEmMediaFormat::Uef] {
        let f = Fixture::new(format);
        let preview = preview_b_em(&f.profile, &f.media).unwrap();
        assert_eq!(preview.bound_sources().count(), 1);
        assert_eq!(preview.platform_id(), "BBC Micro");
    }
}

#[test]
fn unsupported_media_is_refused_by_name() {
    for format in [
        BEmMediaFormat::Csw,
        BEmMediaFormat::Adf,
        BEmMediaFormat::Snapshot,
        BEmMediaFormat::Unknown,
    ] {
        let f = Fixture::new(format);
        assert!(matches!(
            preview_b_em(&f.profile, &f.media),
            Err(PreviewError::UnsupportedMedia(_))
        ));
    }
}

#[test]
fn a_wrongly_labelled_or_malformed_container_is_refused() {
    // A UEF label on SSD bytes, and an SSD label on arbitrary bytes.
    let mut f = Fixture::new(BEmMediaFormat::Uef);
    std::fs::write(&f.media.path, ssd_bytes()).unwrap();
    f.media.verified_source_sha256 = Sha256::digest(ssd_bytes()).into();
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
    let mut f = Fixture::new(BEmMediaFormat::Ssd);
    let junk = vec![0x5a; 4096];
    std::fs::write(&f.media.path, &junk).unwrap();
    f.media.verified_source_sha256 = Sha256::digest(&junk).into();
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
}

#[test]
fn identity_machine_and_session_gates() {
    let mut f = Fixture::new(BEmMediaFormat::Uef);
    f.media.platform_evidence = LocalEvidenceStrength::Weak;
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    let mut f = Fixture::new(BEmMediaFormat::Uef);
    f.media.verified_source_sha256 = [1; 32];
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    let mut f = Fixture::new(BEmMediaFormat::Uef);
    f.media.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "Acorn Electron".into(),
        game_key: "x".into(),
    });
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    let mut f = Fixture::new(BEmMediaFormat::Uef);
    f.profile.machine = None;
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::MachineRequired)
    ));
    let mut f = Fixture::new(BEmMediaFormat::Uef);
    f.profile.disposable_session_acknowledged = false;
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::DisposableSessionRequired)
    ));
}

#[test]
fn executable_policy_missing_symlink_and_drift() {
    let mut f = Fixture::new(BEmMediaFormat::Uef);
    let real = f.profile.executable.clone();
    let link = f.temp.path().join("link-b-em");
    symlink(&real, &link).unwrap();
    f.profile.executable = link;
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::ExecutableUnsafe)
    ));
    f.profile.executable = f.temp.path().join("missing");
    assert!(matches!(
        preview_b_em(&f.profile, &f.media),
        Err(PreviewError::ExecutableMissing)
    ));
    // Drift after preview makes it stale.
    let f = Fixture::new(BEmMediaFormat::Uef);
    let preview = preview_b_em(&f.profile, &f.media).unwrap();
    std::fs::write(&f.profile.executable, b"#!/bin/sh\nexit 3\n").unwrap();
    assert!(!preview.is_fresh());
}

#[test]
fn source_change_after_preview_is_stale_and_the_source_is_never_modified() {
    let f = Fixture::new(BEmMediaFormat::Ssd);
    let before = std::fs::read(&f.media.path).unwrap();
    let preview = preview_b_em(&f.profile, &f.media).unwrap();
    assert!(preview.is_fresh());
    assert_eq!(std::fs::read(&f.media.path).unwrap(), before);
    std::fs::write(&f.media.path, b"changed").unwrap();
    assert!(!preview.is_fresh());
}

#[test]
fn discovery_reports_symlinks_and_never_runs_candidates() {
    let f = Fixture::new(BEmMediaFormat::Uef);
    let bin = f.temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    symlink(&f.profile.executable, bin.join("b-em")).unwrap();
    let path = std::env::join_paths([&bin, f.temp.path()]).unwrap();
    let found = discover_executables(&[], Some(&path));
    assert_eq!(found.executables, vec![f.profile.executable.clone()]);
    assert_eq!(found.symlink_refusals.len(), 1);
}
