use super::*;
use crate::launch::planning::ResolvedIdentity;
use crate::launch::readiness::LaunchReadiness;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{PermissionsExt, symlink};
use tempfile::TempDir;

fn tap(name: &[u8], start: u16, data: &[u8]) -> Vec<u8> {
    let end = start as usize + data.len() - 1;
    let mut b = vec![
        0x16,
        0x16,
        0x16,
        0x24,
        0,
        0,
        0x80,
        0xc7,
        (end >> 8) as u8,
        end as u8,
        (start >> 8) as u8,
        start as u8,
        0,
    ];
    b.extend(name);
    b.push(0);
    b.extend(data);
    b
}

fn crc(b: &[u8]) -> u16 {
    let mut remainder = 0xffffu32;
    for v in b {
        remainder ^= (*v as u32) << 8;
        for _ in 0..8 {
            remainder <<= 1;
            if remainder & 0x10000 != 0 {
                remainder ^= 0x11021;
            }
        }
    }
    remainder as u16
}
fn record(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend(bytes);
    out.extend(crc(bytes).to_be_bytes());
}
/// A one-side MFM_DISK geometry-1 image with valid ID/data CRCs.
fn mfm(tracks: usize, sectors: usize) -> Vec<u8> {
    let mut b = vec![0; 256];
    b[..8].copy_from_slice(b"MFM_DISK");
    b[8..12].copy_from_slice(&1u32.to_le_bytes());
    b[12..16].copy_from_slice(&(tracks as u32).to_le_bytes());
    b[16..20].copy_from_slice(&1u32.to_le_bytes());
    for cylinder in 0..tracks {
        let mut track = vec![0x4e; 60];
        for sector in 1..=sectors {
            track.extend([0; 12]);
            record(
                &mut track,
                &[0xa1, 0xa1, 0xa1, 0xfe, cylinder as u8, 0, sector as u8, 1],
            );
            track.extend([0x22; 22]);
            track.extend([0; 12]);
            let mut data = vec![0xa1, 0xa1, 0xa1, 0xfb];
            data.extend([0x35; 256]);
            record(&mut track, &data);
            track.extend(vec![0x4e; 38]);
        }
        track.resize(6400, 0x4e);
        b.extend(track);
    }
    b
}

struct Fixture {
    temp: TempDir,
    profile: OricutronProfile,
    media: OricutronMedia,
}

impl Fixture {
    fn with(bytes: Vec<u8>, name: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("oricutron");
        std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = temp.path().join(name);
        std::fs::write(&path, &bytes).unwrap();
        let observation = observe_oric_media(&bytes).unwrap();
        Self {
            media: OricutronMedia {
                path,
                observation,
                sha256: Sha256::digest(&bytes).into(),
                evidence: LocalEvidenceStrength::Verified,
                release_identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                    platform_id: PLATFORM_ID.into(),
                    game_key: "synthetic".into(),
                }),
            },
            profile: OricutronProfile {
                executable: exe,
                machine: Some(OricMachine::Atmos),
                disposable_session_acknowledged: true,
            },
            temp,
        }
    }
    fn tap() -> Self {
        Self::with(
            tap(b"HELLO", 0x500, &[1, 2, 3, 4]),
            "Tape with spaces ünï.tap",
        )
    }
    fn dsk() -> Self {
        Self::with(mfm(2, 17), "Dísk – 日本.dsk")
    }
}

#[test]
fn classification_is_preview_only_and_names_the_exe_directory_finding() {
    assert_eq!(
        CLASSIFICATION,
        NativeAdapterClassification::PreviewReadinessOnly
    );
    let f = Fixture::tap();
    let preview = preview_oricutron(&f.profile, &f.media).unwrap();
    assert_eq!(preview.readiness(), LaunchReadiness::Blocked);
    assert!(!preview.is_launchable());
    assert!(
        preview
            .unproven()
            .iter()
            .any(|r| r.contains("executable's own directory"))
    );
    assert!(
        preview
            .unproven()
            .iter()
            .any(|r| r.contains("scratch copy of the installation"))
    );
    assert!(preview.is_fresh());
}

#[test]
fn tap_and_the_reviewed_dsk_subset_validate_with_spaces_and_unicode() {
    for f in [Fixture::tap(), Fixture::dsk()] {
        let preview = preview_oricutron(&f.profile, &f.media).unwrap();
        assert_eq!(preview.scratch_plan().sources().count(), 1);
    }
    assert_eq!(Fixture::tap().media.format(), OricMediaFormat::Tap);
    assert_eq!(Fixture::dsk().media.format(), OricMediaFormat::Dsk);
}

#[test]
fn telestrat_cannot_be_selected_and_a_machine_is_required() {
    // Exhaustive: adding a variant (for example Telestrat) breaks this match
    // and must be a deliberate review of every use.
    for machine in [OricMachine::Oric1, OricMachine::Atmos] {
        match machine {
            OricMachine::Oric1 | OricMachine::Atmos => {}
        }
    }
    let mut f = Fixture::tap();
    f.profile.machine = None;
    assert!(matches!(
        preview_oricutron(&f.profile, &f.media),
        Err(PreviewError::MachineRequired)
    ));
}

#[test]
fn an_observation_that_does_not_match_the_bytes_is_refused() {
    // Valid observation for tape A, but the bound file is tape B (and its hash
    // is updated, so only the observation comparison can catch it).
    let mut f = Fixture::tap();
    let other = tap(b"OTHER", 0x600, &[9, 9, 9]);
    std::fs::write(&f.media.path, &other).unwrap();
    f.media.sha256 = Sha256::digest(&other).into();
    assert!(matches!(
        preview_oricutron(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
    // Garbage bytes under a real observation.
    let mut f = Fixture::dsk();
    let junk = vec![0x11; 4096];
    std::fs::write(&f.media.path, &junk).unwrap();
    f.media.sha256 = Sha256::digest(&junk).into();
    assert!(preview_oricutron(&f.profile, &f.media).is_err());
}

#[test]
fn weak_evidence_wrong_hash_and_session_gates() {
    let mut f = Fixture::tap();
    f.media.evidence = LocalEvidenceStrength::Weak;
    assert!(matches!(
        preview_oricutron(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    let mut f = Fixture::tap();
    f.media.sha256 = [7; 32];
    assert!(matches!(
        preview_oricutron(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    let mut f = Fixture::tap();
    f.profile.disposable_session_acknowledged = false;
    assert!(matches!(
        preview_oricutron(&f.profile, &f.media),
        Err(PreviewError::DisposableSessionRequired)
    ));
}

#[test]
fn executable_policy_staleness_and_immutability() {
    let mut f = Fixture::tap();
    let real = f.profile.executable.clone();
    let link = f.temp.path().join("link");
    symlink(&real, &link).unwrap();
    f.profile.executable = link;
    assert!(matches!(
        preview_oricutron(&f.profile, &f.media),
        Err(PreviewError::ExecutableUnsafe)
    ));
    f.profile.executable = f.temp.path().join("missing");
    assert!(matches!(
        preview_oricutron(&f.profile, &f.media),
        Err(PreviewError::ExecutableMissing)
    ));
    f.profile.executable = real;
    let before = std::fs::read(&f.media.path).unwrap();
    let preview = preview_oricutron(&f.profile, &f.media).unwrap();
    assert_eq!(std::fs::read(&f.media.path).unwrap(), before);
    assert!(preview.is_fresh());
    std::fs::write(&f.profile.executable, b"#!/bin/sh\nexit 2\n").unwrap();
    assert!(!preview.is_fresh());
}

#[test]
fn discovery_reports_a_path_symlink_with_its_target() {
    let f = Fixture::tap();
    let bin = f.temp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    symlink(&f.profile.executable, bin.join("oricutron")).unwrap();
    let path = std::env::join_paths([&bin]).unwrap();
    let found = discover_executables(&[], Some(&path));
    assert!(found.executables.is_empty());
    assert_eq!(found.symlink_refusals.len(), 1);
    assert!(found.symlink_refusals[0].eligible_target.is_some());
}
