use super::*;
use crate::launch::planning::ResolvedIdentity;
use crate::launch::readiness::LaunchReadiness;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{PermissionsExt, symlink};
use tempfile::TempDir;

/// The same minimal valid D88 shape main's own parser tests use.
fn d88_bytes() -> Vec<u8> {
    let header = 0x2b0usize;
    let mut image = vec![0u8; header + 16 + 128];
    image[..6].copy_from_slice(b"SYNTH ");
    image[0x1c..0x20].copy_from_slice(&(header as u32).to_le_bytes());
    let track = header;
    image[track + 2] = 1;
    image[track + 4..track + 6].copy_from_slice(&1u16.to_le_bytes());
    image[track + 14..track + 16].copy_from_slice(&128u16.to_le_bytes());
    image[track + 16..].fill(0xe5);
    image
}

struct Fixture {
    temp: TempDir,
    profile: Profile,
    media: Media,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("sdlnp21kai");
        std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bytes = d88_bytes();
        let path = temp.path().join("ディスク with spaces ünï.d88");
        std::fs::write(&path, &bytes).unwrap();
        Self {
            media: Media {
                path,
                format: MediaFormat::D88,
                identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                    platform_id: PLATFORM_ID.into(),
                    game_key: "synthetic".into(),
                }),
                platform_evidence: LocalEvidenceStrength::Verified,
                verified_source_sha256: Sha256::digest(&bytes).into(),
            },
            profile: Profile {
                executable: exe,
                machine: Some(MachineProfile::Pc9801),
                disposable_session_acknowledged: true,
            },
            temp,
        }
    }

    fn rewrite(&mut self, bytes: &[u8]) {
        std::fs::write(&self.media.path, bytes).unwrap();
        self.media.verified_source_sha256 = Sha256::digest(bytes).into();
    }
}

#[test]
fn classification_is_preview_only_with_the_unproven_state_named() {
    assert_eq!(
        CLASSIFICATION,
        NativeAdapterClassification::PreviewReadinessOnly
    );
    let f = Fixture::new();
    let preview = preview(&f.profile, &f.media).unwrap();
    assert_eq!(preview.readiness(), LaunchReadiness::Blocked);
    assert!(!preview.is_launchable());
    assert!(preview.unproven().iter().any(|r| r.contains("CMOS/NVRAM")));
    assert!(
        preview
            .unproven()
            .iter()
            .any(|r| r.contains("positional D88"))
    );
    assert!(preview.is_fresh());
}

#[test]
fn main_d88_validator_replaces_the_weaker_offset_check() {
    // The prototype accepted any header whose present track offsets were in
    // range. main's parser also walks the sector records, so an image whose
    // track table points at garbage is refused.
    let mut f = Fixture::new();
    let mut bad = d88_bytes();
    let track = 0x2b0;
    bad[track + 4..track + 6].copy_from_slice(&9999u16.to_le_bytes()); // sector count lies
    f.rewrite(&bad);
    assert!(matches!(
        preview(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
    // Truncated, empty and arbitrary bytes.
    for bytes in [
        vec![],
        vec![0u8; 100],
        vec![0x7f; 0x2b0 + 64],
        d88_bytes()[..0x2b0].to_vec(),
    ] {
        let mut f = Fixture::new();
        f.rewrite(&bytes);
        assert!(preview(&f.profile, &f.media).is_err(), "{}", bytes.len());
    }
}

#[test]
fn hdi_and_unknown_media_are_refused_by_name() {
    for format in [MediaFormat::Hdi, MediaFormat::Unknown] {
        let mut f = Fixture::new();
        f.media.format = format;
        assert!(matches!(
            preview(&f.profile, &f.media),
            Err(PreviewError::UnsupportedMedia(_))
        ));
    }
}

#[test]
fn identity_machine_and_session_gates() {
    let mut f = Fixture::new();
    f.media.platform_evidence = LocalEvidenceStrength::Weak;
    assert!(matches!(
        preview(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    let mut f = Fixture::new();
    f.media.verified_source_sha256 = [3; 32];
    assert!(matches!(
        preview(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    // D88 is shared with PC-88/FM Towns/X68000: a different platform id refuses.
    let mut f = Fixture::new();
    f.media.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: "X68000".into(),
        game_key: "x".into(),
    });
    assert!(matches!(
        preview(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    let mut f = Fixture::new();
    f.profile.machine = None;
    assert!(matches!(
        preview(&f.profile, &f.media),
        Err(PreviewError::MachineRequired)
    ));
    let mut f = Fixture::new();
    f.profile.disposable_session_acknowledged = false;
    assert!(matches!(
        preview(&f.profile, &f.media),
        Err(PreviewError::DisposableSessionRequired)
    ));
}

#[test]
fn executable_policy_and_staleness() {
    let mut f = Fixture::new();
    let link = f.temp.path().join("link");
    symlink(&f.profile.executable, &link).unwrap();
    let real = std::mem::replace(&mut f.profile.executable, link);
    assert!(matches!(
        preview(&f.profile, &f.media),
        Err(PreviewError::ExecutableUnsafe)
    ));
    f.profile.executable = f.temp.path().join("missing");
    assert!(matches!(
        preview(&f.profile, &f.media),
        Err(PreviewError::ExecutableMissing)
    ));
    f.profile.executable = real;
    let p = preview(&f.profile, &f.media).unwrap();
    assert!(p.is_fresh());
    // Source edited / replaced / removed: stale.
    let before = std::fs::read(&f.media.path).unwrap();
    std::fs::write(&f.media.path, b"edited").unwrap();
    assert!(!p.is_fresh());
    std::fs::write(&f.media.path, &before).unwrap();
    std::fs::remove_file(&f.media.path).unwrap();
    assert!(!p.is_fresh());
}

#[test]
fn discovery_uses_reviewed_frontend_names_only_and_reports_symlinks() {
    let f = Fixture::new();
    let bin = f.temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    // A bare "np2kai" that is a symlink is reported, not accepted.
    symlink(&f.profile.executable, bin.join("np2kai")).unwrap();
    let other = f.temp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::copy(&f.profile.executable, other.join("xnp21kai")).unwrap();
    std::fs::write(other.join("np2"), b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(other.join("np2"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths([&bin, &other]).unwrap();
    let found = discover_executables(&[], Some(&path));
    assert_eq!(found.executables, vec![other.join("xnp21kai")]);
    assert_eq!(found.symlink_refusals.len(), 1);
}

#[test]
fn the_source_disk_is_never_modified_by_planning() {
    let f = Fixture::new();
    let before = std::fs::read(&f.media.path).unwrap();
    let _ = preview(&f.profile, &f.media).unwrap();
    assert_eq!(std::fs::read(&f.media.path).unwrap(), before);
}
