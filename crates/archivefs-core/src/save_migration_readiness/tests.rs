use std::fs;
use std::path::PathBuf;

use super::*;

fn raw(size: u64) -> SaveMigrationRepresentation {
    SaveMigrationRepresentation::RawFile { size_bytes: size }
}

fn src(
    platform: &str,
    format: SaveMigrationFormat,
    save_type: SaveArtifactType,
    representation: SaveMigrationRepresentation,
) -> SaveMigrationSource {
    SaveMigrationSource {
        platform: Some(platform.into()),
        game_identity: Some("game-1".into()),
        region: Some("US".into()),
        emulator: Some("RetroArch".into()),
        core: Some("snes9x".into()),
        save_type,
        format,
        representation,
        identity_confidence: SaveMigrationIdentityConfidence::Exact,
        filename: Some("game.srm".into()),
        container_context: None,
        evidence_stale: false,
    }
}

fn tgt(
    platform: &str,
    format: SaveMigrationFormat,
    save_type: SaveArtifactType,
    representation: SaveMigrationRepresentation,
) -> SaveMigrationTarget {
    SaveMigrationTarget {
        platform: Some(platform.into()),
        game_identity: Some("game-1".into()),
        region: Some("US".into()),
        emulator: Some("Snes9x".into()),
        core: None,
        save_type,
        format,
        representation,
        container_context: None,
        proven_conversion_paths: Vec::new(),
    }
}

fn env() -> SaveTargetEnvironment {
    SaveTargetEnvironment {
        emulator: Some("Snes9x".into()),
        emulator_version: None,
        core: None,
        profile: Some("default".into()),
        version_matters: false,
    }
}

fn request(source: SaveMigrationSource, target: SaveMigrationTarget) -> SaveMigrationRequest {
    SaveMigrationRequest {
        source,
        target,
        target_environment: env(),
        source_path: Some(PathBuf::from("/saves/game.srm")),
        source_stamp: None,
        restore_refusals: None,
    }
}

fn snes(size: u64) -> (SaveMigrationSource, SaveMigrationTarget) {
    (
        src(
            "SNES",
            SaveMigrationFormat::SnesSram,
            SaveArtifactType::Sram,
            raw(size),
        ),
        tgt(
            "SNES",
            SaveMigrationFormat::SnesSram,
            SaveArtifactType::Sram,
            raw(size),
        ),
    )
}

fn observed_request(dir: &tempfile::TempDir) -> SaveMigrationRequest {
    let path = dir.path().join("game.srm");
    fs::write(&path, [1u8; 16]).unwrap();
    let (source, target) = snes(16);
    let mut request = request(source, target);
    request.source_path = Some(path.clone());
    request.source_stamp = Some(SaveSourceStamp::capture(&path));
    request
}

#[test]
fn exact_sram_compatibility_is_separate_from_restore_authority() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = observed_request(&dir);
    request.restore_refusals = Some(vec![]);
    let result = plan_save_migration_for_target(request);
    assert_eq!(result.outcome, SaveMigrationOutcome::DirectlyCompatible);
    assert_eq!(
        result.restore.writer,
        SaveRestoreWriterCapability::SingleFileTransactional(GenericSingleFileSaveFamily::Sram)
    );
    assert!(!result.restore.existing_restore_apply_ready);
    assert!(!result.automatic_migration_available);
    assert!(result.read_only);
}

#[test]
fn every_supplied_restore_refusal_is_retained() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = observed_request(&dir);
    let refusals = vec![
        SaveRestoreRefusal::EmulatorRunning,
        SaveRestoreRefusal::DestinationChanged,
        SaveRestoreRefusal::SnapshotHashChanged,
        SaveRestoreRefusal::SharedContainerRestoreNotSupported,
    ];
    request.restore_refusals = Some(refusals.clone());
    let result = plan_save_migration_for_target(request);
    assert_eq!(result.outcome, SaveMigrationOutcome::DirectlyCompatible);
    assert_eq!(result.restore.restore_refusals, Some(refusals));
    assert!(!result.restore.existing_restore_apply_ready);
}

#[test]
fn missing_or_filename_identity_and_missing_target_evidence_stay_unknown() {
    let dir = tempfile::tempdir().unwrap();
    for missing in [
        "filename", "source", "target", "version", "platform", "stamp",
    ] {
        let mut request = observed_request(&dir);
        match missing {
            "filename" => {
                request.source.identity_confidence = SaveMigrationIdentityConfidence::FilenameOnly
            }
            "source" => request.source.game_identity = None,
            "target" => request.target.game_identity = None,
            "version" => request.target_environment.version_matters = true,
            "platform" => request.target.platform = None,
            _ => request.source_stamp = None,
        }
        let result = plan_save_migration_for_target(request);
        assert_eq!(
            result.outcome,
            SaveMigrationOutcome::NeedsEvidence,
            "{missing}"
        );
        assert!(!result.automatic_migration_available);
    }
}

#[test]
fn wrong_game_and_platform_refuse_even_for_identical_bytes() {
    let dir = tempfile::tempdir().unwrap();
    for platform in [true, false] {
        let mut request = observed_request(&dir);
        if platform {
            request.target.platform = Some("GBA".into());
        } else {
            request.target.game_identity = Some("different-game".into());
        }
        let result = plan_save_migration_for_target(request);
        assert_eq!(result.outcome, SaveMigrationOutcome::KnownIncompatible);
    }
}

#[test]
fn source_digest_and_opened_identity_detect_same_size_edits_and_replacements() {
    let dir = tempfile::tempdir().unwrap();
    for replace in [false, true] {
        let request = observed_request(&dir);
        let path = request.source_path.as_ref().unwrap();
        let time = fs::metadata(path).unwrap().modified().unwrap();
        if replace {
            let other = dir.path().join("replacement");
            fs::write(&other, [1u8; 16]).unwrap();
            fs::rename(other, path).unwrap();
        } else {
            fs::write(path, [2u8; 16]).unwrap();
        }
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
        let result = plan_save_migration_for_target(request);
        assert_eq!(result.outcome, SaveMigrationOutcome::NeedsEvidence);
        assert!(
            result
                .reasons
                .contains(&SaveMigrationReason::SourceChangedSinceCapture)
        );
    }
}

#[test]
fn stamps_cannot_be_substituted_between_paths() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = observed_request(&dir);
    request.source_path = Some(dir.path().join("unrelated"));
    assert_eq!(
        plan_save_migration_for_target(request).outcome,
        SaveMigrationOutcome::NeedsEvidence
    );
}

#[test]
fn shared_cards_never_become_raw_single_file_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = observed_request(&dir);
    request.source.format = SaveMigrationFormat::Ps1MemoryCard;
    request.source.save_type = SaveArtifactType::PsMemoryCard;
    request.source.representation = SaveMigrationRepresentation::MemoryCardImage {
        size_bytes: 16,
        container: "mcr".into(),
    };
    request.target.format = request.source.format.clone();
    request.target.save_type = request.source.save_type;
    request.target.representation = request.source.representation.clone();
    let result = plan_save_migration_for_target(request);
    assert_eq!(result.container, SaveContainerSemantics::SharedContainer);
    assert_ne!(result.outcome, SaveMigrationOutcome::DirectlyCompatible);
    assert_eq!(
        result.restore.writer,
        SaveRestoreWriterCapability::PreviewOnly
    );
}

#[test]
fn incomplete_psp_sets_and_unavailable_converters_never_offer_migration() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = observed_request(&dir);
    request.source.format = SaveMigrationFormat::PspSavedata;
    request.source.representation = SaveMigrationRepresentation::Directory { file_count: None };
    request.target.format = request.source.format.clone();
    request.target.representation = request.source.representation.clone();
    let result = plan_save_migration_for_target(request);
    assert_ne!(result.outcome, SaveMigrationOutcome::DirectlyCompatible);
    assert!(!result.automatic_migration_available);
    let mut request = observed_request(&dir);
    request.target.format = SaveMigrationFormat::MisterRawSave;
    let result = plan_save_migration_for_target(request);
    assert_ne!(result.outcome, SaveMigrationOutcome::DirectlyCompatible);
    assert!(!result.automatic_migration_available);
}

#[test]
fn source_observation_and_planning_write_nothing_and_refuse_special_files() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let request = observed_request(&dir);
    let path = request.source_path.clone().unwrap();
    let before = fs::read(&path).unwrap();
    let stamp = request.source_stamp.as_ref().unwrap();
    assert!(
        stamp.matches_inventory(
            &path,
            16,
            Some(
                &Sha256::digest(&before)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )
        )
    );
    assert!(!stamp.matches_inventory(&path, 16, Some("wrong")));
    plan_save_migration_for_target(request);
    assert_eq!(fs::read(&path).unwrap(), before);
    let link = dir.path().join("link");
    symlink(&path, &link).unwrap();
    assert_eq!(
        SaveSourceStamp::capture(&link).freshness(),
        SaveSourceFreshness::Stale
    );
    assert_eq!(
        SaveSourceStamp::capture(dir.path()).freshness(),
        SaveSourceFreshness::Stale
    );
}
