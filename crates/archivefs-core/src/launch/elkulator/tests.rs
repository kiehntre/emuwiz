use super::*;
use crate::disk_format::{DiskFormat, DiskFormatContext, inspect_disk_format};
use crate::launch::planning::ResolvedIdentity;
use crate::launch::readiness::LaunchReadiness;
use crate::safe_read::TrustedRoots;
use crate::tape_analysis::MAX_ANALYSIS_BYTES;
use flate2::{Compression, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    profile: ElkulatorProfile,
    media: ElkulatorMedia,
}

fn executable(path: &Path) {
    // An observable trap if discovery/preview ever starts executing binaries.
    let marker = path.with_extension("executed");
    std::fs::write(
        path,
        format!("#!/bin/sh\nprintf invoked > '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn ssd() -> Vec<u8> {
    let mut bytes = vec![0; 400 * 256];
    bytes[..8].copy_from_slice(b"ELECTRON");
    bytes[256..260].fill(b' ');
    bytes[260] = 0x12;
    bytes[261] = 8;
    bytes[262] = 1;
    bytes[263] = 144; // 400 sectors
    bytes[8..15].copy_from_slice(b"!BOOT  ");
    bytes[15] = b'$';
    bytes[264..266].copy_from_slice(&0x1900_u16.to_le_bytes());
    bytes[266..268].copy_from_slice(&0x1900_u16.to_le_bytes());
    bytes[268] = 3;
    bytes[271] = 2; // Three bytes in sector 2, beyond the catalogue.
    bytes[512..515].copy_from_slice(&[1, 2, 3]);
    bytes
}

fn dsd() -> Vec<u8> {
    let side = ssd();
    let mut bytes = Vec::with_capacity(side.len() * 2);
    for track in side.chunks_exact(10 * 256) {
        bytes.extend_from_slice(track);
        bytes.extend_from_slice(track);
    }
    bytes
}

fn uef() -> Vec<u8> {
    let mut bytes = b"UEF File!\0\x0a\x00".to_vec();
    bytes.extend_from_slice(&0x0100_u16.to_le_bytes());
    bytes.extend_from_slice(&5_u32.to_le_bytes());
    bytes.extend_from_slice(&[0xb0, 0x04, 1, 2, 3]);
    bytes
}

impl Fixture {
    fn new(format: ElkulatorMediaFormat) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join(ADAPTER_ID);
        executable(&exe);
        let (suffix, bytes) = match format {
            ElkulatorMediaFormat::Uef => ("uef", uef()),
            ElkulatorMediaFormat::Dsd => ("dsd", dsd()),
            _ => ("ssd", ssd()),
        };
        let path = temp
            .path()
            .join(format!("Electron – 日本 tape or disc.{suffix}"));
        std::fs::write(&path, &bytes).unwrap();
        Self {
            profile: ElkulatorProfile {
                executable: exe,
                disposable_session_acknowledged: true,
            },
            media: ElkulatorMedia {
                path,
                format,
                identity: CanonicalIdentityStatus::Resolved(ResolvedIdentity {
                    platform_id: PLATFORM_ID.into(),
                    game_key: "verified-synthetic-game".into(),
                }),
                platform_evidence: LocalEvidenceStrength::Verified,
                verified_source_sha256: Sha256::digest(&bytes).into(),
            },
            temp,
        }
    }

    fn replace(&mut self, bytes: &[u8]) {
        std::fs::write(&self.media.path, bytes).unwrap();
        self.media.verified_source_sha256 = Sha256::digest(bytes).into();
    }
}

#[test]
fn executable_discovery_is_bounded_read_only_and_deduplicated() {
    let f = Fixture::new(ElkulatorMediaFormat::Uef);
    let found = discover_executables(
        &[f.profile.executable.clone()],
        Some(f.temp.path().as_os_str()),
    );
    assert_eq!(found.executables, [f.profile.executable.clone()]);
    assert!(found.symlink_refusals.is_empty());
    assert!(!f.profile.executable.with_extension("executed").exists());
}

#[test]
fn executable_ambiguity_is_reported_without_picking_a_winner() {
    let f = Fixture::new(ElkulatorMediaFormat::Uef);
    let second_dir = f.temp.path().join("second");
    std::fs::create_dir(&second_dir).unwrap();
    let second = second_dir.join(ADAPTER_ID);
    executable(&second);
    let path = std::env::join_paths([second_dir.as_path(), f.temp.path()]).unwrap();
    let found = discover_executables(&[], Some(&path));
    let mut expected = vec![second.clone(), f.profile.executable.clone()];
    expected.sort();
    assert_eq!(found.executables, expected);
    assert!(!second.with_extension("executed").exists());
    assert!(!f.profile.executable.with_extension("executed").exists());
}

#[test]
fn missing_executable_is_refused() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    f.profile.executable = f.temp.path().join("missing");
    assert!(
        discover_executables(&[f.profile.executable.clone()], None)
            .executables
            .is_empty()
    );
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::ExecutableMissing)
    ));
}

#[test]
fn symlinked_executable_is_reported_and_refused() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    let real = f.profile.executable.clone();
    let link = f.temp.path().join("linked-elkulator");
    symlink(&real, &link).unwrap();
    let found = discover_executables(std::slice::from_ref(&link), None);
    assert!(found.executables.is_empty());
    assert_eq!(found.symlink_refusals.len(), 1);
    assert_eq!(
        found.symlink_refusals[0].eligible_target.as_ref(),
        Some(&real)
    );
    f.profile.executable = link;
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::ExecutableUnsafe)
    ));
}

#[test]
fn non_executable_or_relative_path_is_refused() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    std::fs::set_permissions(
        &f.profile.executable,
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::ExecutableUnsafe)
    ));
    f.profile.executable = PathBuf::from(ADAPTER_ID);
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::ExecutableUnsafe)
    ));
}

#[test]
fn resolved_electron_accepts_ssd_dsd_and_uef_for_preview_only() {
    for format in [
        ElkulatorMediaFormat::Ssd,
        ElkulatorMediaFormat::Dsd,
        ElkulatorMediaFormat::Uef,
    ] {
        let f = Fixture::new(format);
        let preview = preview_elkulator(&f.profile, &f.media).unwrap();
        assert_eq!(preview.adapter(), ADAPTER_ID);
        assert_eq!(preview.platform_id(), PLATFORM_ID);
        assert_eq!(preview.executable(), f.profile.executable);
        assert!(preview.is_fresh());
        assert_eq!(
            preview.classification(),
            NativeAdapterClassification::PreviewReadinessOnly
        );
        assert_eq!(preview.readiness(), LaunchReadiness::Blocked);
        assert!(!preview.is_launchable());
    }
}

#[test]
fn filename_and_family_structure_never_promote_unknown_or_conflicting_identity() {
    for format in [
        ElkulatorMediaFormat::Ssd,
        ElkulatorMediaFormat::Dsd,
        ElkulatorMediaFormat::Uef,
    ] {
        let mut f = Fixture::new(format);
        for identity in [
            CanonicalIdentityStatus::Unknown,
            CanonicalIdentityStatus::Conflicting,
        ] {
            f.media.identity = identity;
            assert!(matches!(
                preview_elkulator(&f.profile, &f.media),
                Err(PreviewError::IdentityNotVerified)
            ));
        }
    }
}

#[test]
fn structurally_valid_dfs_remains_ambiguous_without_electron_identity() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Ssd);
    let evidence = inspect_disk_format(
        &f.media.path,
        &TrustedRoots::none(),
        DiskFormatContext {
            folder_platform: None,
        },
        None,
    );
    assert_eq!(evidence.format, Some(DiskFormat::AcornDfsDisk));
    assert!(
        evidence
            .evidence
            .iter()
            .any(|line| line.contains("does not identify one machine"))
    );
    f.media.identity = CanonicalIdentityStatus::Unknown;
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
}

#[test]
fn bbc_micro_identity_is_refused_for_every_supported_format() {
    for format in [
        ElkulatorMediaFormat::Ssd,
        ElkulatorMediaFormat::Dsd,
        ElkulatorMediaFormat::Uef,
    ] {
        let mut f = Fixture::new(format);
        f.media.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
            platform_id: "BBC Micro".into(),
            game_key: "verified-bbc-game".into(),
        });
        assert!(matches!(
            preview_elkulator(&f.profile, &f.media),
            Err(PreviewError::IdentityNotVerified)
        ));
    }
}

#[test]
fn weak_evidence_and_empty_canonical_game_key_are_refused() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    f.media.platform_evidence = LocalEvidenceStrength::Weak;
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    f.media.platform_evidence = LocalEvidenceStrength::Verified;
    f.media.identity = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: PLATFORM_ID.into(),
        game_key: "  ".into(),
    });
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
}

#[test]
fn gzip_wrapped_uef_uses_the_existing_bounded_parser() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&uef()).unwrap();
    f.replace(&encoder.finish().unwrap());
    assert!(preview_elkulator(&f.profile, &f.media).unwrap().is_fresh());
}

#[test]
fn malformed_uef_header_chunk_and_gzip_are_refused() {
    let mut truncated_header = b"UEF File!\0\x0a\x00".to_vec();
    truncated_header.extend_from_slice(&[0, 1]);
    let mut truncated_body = uef();
    truncated_body.pop();
    for bytes in [
        b"not UEF".to_vec(),
        truncated_header,
        truncated_body,
        vec![0x1f, 0x8b, 0, 0],
    ] {
        let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
        f.replace(&bytes);
        assert!(matches!(
            preview_elkulator(&f.profile, &f.media),
            Err(PreviewError::UnsupportedMedia(_))
        ));
    }
}

#[test]
fn uef_input_and_decompression_limits_are_preserved() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    f.replace(&vec![0; MAX_ANALYSIS_BYTES + 1]);
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
    // Under the compressed-input limit, over the parser's decoded-output limit.
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&vec![0; 16 * 1024 * 1024 + 1]).unwrap();
    f.replace(&encoder.finish().unwrap());
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
}

#[test]
fn malformed_dfs_and_mismatched_ssd_dsd_declarations_are_refused() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Ssd);
    f.replace(&vec![0x5a; 4096]);
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
    let mut f = Fixture::new(ElkulatorMediaFormat::Ssd);
    f.media.format = ElkulatorMediaFormat::Dsd;
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
    let mut f = Fixture::new(ElkulatorMediaFormat::Dsd);
    f.media.format = ElkulatorMediaFormat::Ssd;
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::UnsupportedMedia(_))
    ));
}

#[test]
fn unaudited_and_unsupported_formats_are_refused() {
    for format in [
        ElkulatorMediaFormat::Csw,
        ElkulatorMediaFormat::Wav,
        ElkulatorMediaFormat::Rom,
        ElkulatorMediaFormat::Snapshot,
        ElkulatorMediaFormat::Adf,
        ElkulatorMediaFormat::Fdi,
        ElkulatorMediaFormat::Unknown,
    ] {
        let f = Fixture::new(format);
        assert!(matches!(
            preview_elkulator(&f.profile, &f.media),
            Err(PreviewError::UnsupportedMedia(_))
        ));
    }
}

#[test]
fn verified_source_hash_is_required_and_retained() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    let preview = preview_elkulator(&f.profile, &f.media).unwrap();
    let sources = preview.bound_sources().collect::<Vec<_>>();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].original_path, f.media.path);
    assert_eq!(sources[0].sha256, f.media.verified_source_sha256);
    f.media.verified_source_sha256 = [0; 32];
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
}

#[test]
fn source_drift_invalidates_a_preview_and_the_old_identity_hash() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    let preview = preview_elkulator(&f.profile, &f.media).unwrap();
    let mut bytes = uef();
    *bytes.last_mut().unwrap() = 42; // Still valid UEF, same length.
    std::fs::write(&f.media.path, &bytes).unwrap();
    assert!(!preview.is_fresh());
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::IdentityNotVerified)
    ));
    f.replace(&bytes);
    assert!(preview_elkulator(&f.profile, &f.media).unwrap().is_fresh());
}

#[test]
fn executable_drift_invalidates_preview() {
    let f = Fixture::new(ElkulatorMediaFormat::Uef);
    let preview = preview_elkulator(&f.profile, &f.media).unwrap();
    std::fs::write(&f.profile.executable, b"#!/bin/sh\nexit 77\n").unwrap();
    assert!(!preview.is_fresh());
}

#[test]
fn symlinked_source_is_refused() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    let link = f.temp.path().join("linked.uef");
    symlink(&f.media.path, &link).unwrap();
    f.media.path = link;
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::Sandbox(_))
    ));
}

#[test]
fn disposable_session_acknowledgement_is_required() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    f.profile.disposable_session_acknowledged = false;
    assert!(matches!(
        preview_elkulator(&f.profile, &f.media),
        Err(PreviewError::DisposableSessionRequired)
    ));
}

#[test]
fn config_containment_is_classified_as_unverified() {
    let f = Fixture::new(ElkulatorMediaFormat::Uef);
    let preview = preview_elkulator(&f.profile, &f.media).unwrap();
    assert_eq!(
        CLASSIFICATION,
        NativeAdapterClassification::PreviewReadinessOnly
    );
    assert!(
        preview
            .unproven()
            .iter()
            .any(|reason| reason.contains("HOME/XDG") && reason.contains("elk.cfg"))
    );
    assert!(!preview.is_launchable());
}

#[test]
fn disk_write_risk_requires_disposable_media_before_any_future_launch() {
    let f = Fixture::new(ElkulatorMediaFormat::Ssd);
    let preview = preview_elkulator(&f.profile, &f.media).unwrap();
    assert!(
        preview
            .unproven()
            .iter()
            .any(|reason| reason.contains("rb+") && reason.contains("no write-back"))
    );
    assert_eq!(preview.readiness(), LaunchReadiness::Blocked);
}

#[test]
fn preview_never_changes_media_config_or_creates_a_workspace_or_process() {
    for format in [
        ElkulatorMediaFormat::Ssd,
        ElkulatorMediaFormat::Dsd,
        ElkulatorMediaFormat::Uef,
    ] {
        let f = Fixture::new(format);
        let cfg = f.temp.path().join("elk.cfg");
        std::fs::write(&cfg, b"sentinel configuration\n").unwrap();
        let before = std::fs::read(&f.media.path).unwrap();
        let entries_before = std::fs::read_dir(f.temp.path()).unwrap().count();
        let preview = preview_elkulator(&f.profile, &f.media).unwrap();
        assert!(preview.is_fresh());
        assert_eq!(std::fs::read(&f.media.path).unwrap(), before);
        assert_eq!(std::fs::read(cfg).unwrap(), b"sentinel configuration\n");
        assert_eq!(
            std::fs::read_dir(f.temp.path()).unwrap().count(),
            entries_before
        );
        assert!(!f.profile.executable.with_extension("executed").exists());
    }
}

#[test]
fn media_inspection_works_without_an_emulator_and_keeps_hash_provenance() {
    for format in [
        ElkulatorMediaFormat::Ssd,
        ElkulatorMediaFormat::Dsd,
        ElkulatorMediaFormat::Uef,
    ] {
        let f = Fixture::new(format);
        std::fs::remove_file(&f.profile.executable).unwrap();
        let report = inspection::inspect_media(&f.media.path).unwrap();
        assert_eq!(report.format, format);
        assert_eq!(report.source.sha256, f.media.verified_source_sha256);
        assert!(!report.structure.incomplete);
        assert!(report.is_fresh());
    }
}

#[test]
fn incomplete_uef_is_not_reported_as_complete_or_playable() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    f.replace(b"UEF File!\0\x0a\x00");
    let report = inspection::inspect_media(&f.media.path).unwrap();
    assert!(report.structure.incomplete);
    assert!(!report.structure.warnings.is_empty());
    let mut bytes = uef();
    bytes.extend_from_slice(&0x0102u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    f.replace(&bytes);
    assert!(
        inspection::inspect_media(&f.media.path)
            .unwrap()
            .structure
            .incomplete
    );
}

#[test]
fn inspection_revalidates_same_size_content_changes_and_missing_files() {
    let f = Fixture::new(ElkulatorMediaFormat::Ssd);
    let report = inspection::inspect_media(&f.media.path).unwrap();
    let mut changed = ssd();
    changed[512] ^= 1;
    std::fs::write(&f.media.path, changed).unwrap();
    assert!(!report.is_fresh());
    std::fs::remove_file(&f.media.path).unwrap();
    assert!(!report.is_fresh());
    assert!(inspection::inspect_media(&f.media.path).is_err());
}

#[test]
fn inspection_refuses_malformed_unsupported_and_symlink_sources() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Ssd);
    f.replace(&[0; 100]);
    assert!(inspection::inspect_media(&f.media.path).is_err());
    let unknown = f.temp.path().join("writable.fdi");
    std::fs::write(&unknown, ssd()).unwrap();
    assert!(inspection::inspect_media(&unknown).is_err());
    let link = f.temp.path().join("alias.ssd");
    symlink(&f.media.path, &link).unwrap();
    assert!(inspection::inspect_media(&link).is_err());
}

#[test]
fn inspection_does_not_create_media_copies_or_change_configuration() {
    let f = Fixture::new(ElkulatorMediaFormat::Dsd);
    let cfg = f.temp.path().join("elk.cfg");
    std::fs::write(&cfg, b"sentinel configuration").unwrap();
    let before = std::fs::read(&f.media.path).unwrap();
    let count = std::fs::read_dir(f.temp.path()).unwrap().count();
    let report = inspection::inspect_media(&f.media.path).unwrap();
    assert!(report.is_fresh());
    assert_eq!(std::fs::read(&f.media.path).unwrap(), before);
    assert_eq!(std::fs::read(&cfg).unwrap(), b"sentinel configuration");
    assert_eq!(std::fs::read_dir(f.temp.path()).unwrap().count(), count);
    assert!(!f.profile.executable.with_extension("executed").exists());
}

#[test]
fn registry_preview_never_registers_an_elkulator_execution_target() {
    let info = super::super::native_support::native_adapter_info(ADAPTER_ID).unwrap();
    assert_eq!(
        info.classification,
        NativeAdapterClassification::PreviewReadinessOnly
    );
    assert_eq!(info.platform_id, "Acorn Electron");
    assert!(super::super::platform_map::platforms_for_standalone_adapter(ADAPTER_ID).is_empty());
}

#[test]
fn empty_uef_does_not_invent_chunk_evidence() {
    let mut f = Fixture::new(ElkulatorMediaFormat::Uef);
    f.replace(b"UEF File!\0\x0a\x00");
    let report = inspection::inspect_media(&f.media.path).unwrap();
    let details = report.structure.details.join("\n");
    assert!(report.structure.incomplete);
    assert!(details.contains("0 supported data entries"));
    assert!(
        !details.contains("1 chunks"),
        "A default logical segment is not an observed chunk: {details}"
    );
}
