//! Exercise the real worker without constructing an app, changing environment
//! variables, contacting RomM, or reading the user's identity store.

use super::*;
use archivefs_core::identity_source::cache::{
    CACHE_FORMAT_VERSION, IdentityCache, IdentityCacheLocation, publish_cache,
};
use archivefs_core::identity_source::model::{
    ExternalHash, ExternalIdentityRecord, ExternalVerification, HashAlgorithm, IdentityProvider,
};
use archivefs_core::identity_source::settings::{ProviderSettings, SettingsLocation};
use archivefs_core::identity_source::verification::VerificationStore;
use std::collections::BTreeMap;
use std::fs;

const SERVER: &str = "http://127.0.0.1:9";
const ABC_MD5: &str = "900150983cd24fb0d6963f7d28e17f72";

struct Fixture {
    directory: tempfile::TempDir,
    identity_root: PathBuf,
    library: PathBuf,
    media: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let identity_root = directory.path().join("identity");
        let library = directory.path().join("library");
        fs::create_dir(&library).unwrap();
        let media = library.join("synthetic.gb");
        fs::write(&media, b"abc").unwrap();
        let fixture = Self {
            directory,
            identity_root,
            library,
            media,
        };
        let mut settings = ProviderSettings::default();
        settings.source.url = SERVER.to_string();
        // Cache-only branches must work even when online preflight would fail.
        settings.source.token_path = Some(fixture.directory.path().join("missing-token"));
        fixture.settings().save(&settings).unwrap();
        fixture.publish(ABC_MD5, ExternalVerification::StrongExternal);
        fixture
    }

    fn settings(&self) -> SettingsLocation {
        SettingsLocation::new(&self.identity_root, IdentityProvider::Romm)
    }

    fn verifications(&self) -> VerificationStore {
        VerificationStore::new(&self.identity_root, IdentityProvider::Romm)
    }

    fn publish(&self, md5: &str, verification: ExternalVerification) {
        let record = ExternalIdentityRecord {
            provider: IdentityProvider::Romm,
            server_id: SERVER.to_string(),
            provider_platform_id: Some("7".into()),
            provider_game_id: "synthetic-1".into(),
            provider_file_id: None,
            provider_path: "gb/synthetic.gb".into(),
            archivefs_path: Some(self.media.clone()),
            title: Some("Synthetic fixture".into()),
            platform_candidate: Some("Game Boy".into()),
            provider_platform_name: Some("gb".into()),
            regions: Vec::new(),
            revision: None,
            hashes: vec![ExternalHash::parse(HashAlgorithm::Md5, md5).unwrap()],
            file_size_bytes: Some(3),
            metadata_provider_ids: Vec::new(),
            artwork: None,
            related_files: Vec::new(),
            sibling_game_ids: Vec::new(),
            imported_at_unix_seconds: 1,
            provider_updated_at: None,
            verification,
            conflicts: Vec::new(),
            evidence: vec!["Synthetic provider provenance".into()],
            synopsis: None,
            genres: Vec::new(),
            players: None,
            rating: None,
            release_year: None,
            howlongtobeat: None,
        };
        publish_cache(
            &IdentityCacheLocation::new(&self.identity_root, IdentityProvider::Romm),
            &IdentityCache {
                format_version: CACHE_FORMAT_VERSION,
                provider: IdentityProvider::Romm,
                server_id: SERVER.into(),
                server_version: None,
                source_fingerprint: "synthetic".into(),
                imported_at_unix_seconds: 1,
                platforms: Vec::new(),
                records: vec![record],
                rejected_hashes: Vec::new(),
                unknown_platforms: Vec::new(),
                server_reported_total: Some(1),
            },
        )
        .unwrap();
    }

    fn run(&self, operation: RommOperation) -> Result<RommOperationOutcome, String> {
        self.run_with(operation, false, Ok(vec![self.library.clone()]))
    }

    fn run_with(
        &self,
        operation: RommOperation,
        cancelled: bool,
        roots: Result<Vec<PathBuf>, String>,
    ) -> Result<RommOperationOutcome, String> {
        run_romm_operation_at(
            &self.identity_root,
            &operation,
            &roots,
            None,
            17,
            &Arc::new(AtomicBool::new(cancelled)),
            &|_| {},
        )
    }

    fn verify_request(&self) -> RommOperation {
        RommOperation::VerifyLocalFile {
            local_path: self.media.clone(),
            romm_game_id: "synthetic-1".into(),
            local_platform: Box::default(),
            chosen_game_id: None,
        }
    }

    fn files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    collect(&entry.path(), files);
                } else {
                    files.insert(entry.path(), fs::read(entry.path()).unwrap());
                }
            }
        }
        let mut files = BTreeMap::new();
        collect(self.directory.path(), &mut files);
        files
    }
}

#[test]
fn missing_store_status_is_read_only_and_not_configured() {
    let directory = tempfile::tempdir().unwrap();
    let snapshot = load_romm_snapshot_at(directory.path()).unwrap();
    assert!(matches!(
        snapshot.status.state,
        archivefs_core::identity_source::status::ProviderState::NotConfigured
    ));
    assert_eq!(snapshot.status.records_imported, 0);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn snapshot_reads_persisted_counts_without_a_usable_token_or_writes() {
    let fixture = Fixture::new();
    let mut settings = fixture.settings().load().unwrap();
    settings.source.enabled = true;
    fixture.settings().save(&settings).unwrap();
    let before = fixture.files();
    let snapshot = load_romm_snapshot_at(&fixture.identity_root).unwrap();
    assert_eq!(snapshot.status.records_imported, 1);
    assert_eq!(snapshot.cache_format_version, Some(CACHE_FORMAT_VERSION));
    assert!(!snapshot.token_available);
    assert!(snapshot.token_problem.is_some());
    assert_eq!(fixture.files(), before);
}

#[test]
fn browsing_uses_cache_despite_missing_token_and_failed_source_configuration() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let outcome = fixture
        .run_with(
            RommOperation::LoadRecords {
                filters: Box::default(),
                offset: 0,
                limit: 10,
            },
            false,
            Err("configuration unavailable".into()),
        )
        .unwrap();
    let RommOperationOutcome::Records(page) = outcome else {
        panic!("expected records")
    };
    assert_eq!(page.total_in_cache, 1);
    assert_eq!(page.rows.len(), 1);
    assert_eq!(fixture.files(), before);
}

#[test]
fn detail_retains_provider_provenance_without_network_or_writes() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let outcome = fixture
        .run(RommOperation::LoadRecordDetail {
            romm_game_id: "synthetic-1".into(),
        })
        .unwrap();
    let RommOperationOutcome::RecordDetail(detail) = outcome else {
        panic!("expected detail")
    };
    let detail = detail.as_ref().as_ref().expect("cached record");
    assert!(
        detail
            .evidence
            .iter()
            .any(|line| line.contains("Synthetic provider provenance"))
    );
    assert_eq!(fixture.files(), before);
}

#[test]
fn resolve_selected_game_does_not_hash_or_persist_verification() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let outcome = fixture
        .run(RommOperation::ResolveGame {
            local_path: fixture.media.clone(),
            local_platform: Box::default(),
            chosen_game_id: None,
        })
        .unwrap();
    let RommOperationOutcome::GameIdentity(panel) = outcome else {
        panic!("expected game")
    };
    assert_eq!(panel.local_path, fixture.media);
    assert!(!fixture.verifications().path().exists());
    assert_eq!(fixture.files(), before);
}

#[test]
fn explicit_verification_persists_matching_hashes_without_modifying_media() {
    let fixture = Fixture::new();
    let outcome = fixture.run(fixture.verify_request()).unwrap();
    let RommOperationOutcome::Verified(result) = outcome else {
        panic!("expected verification")
    };
    assert!(result.all_agree);
    assert!(!result.any_disagree);
    assert_eq!(result.bytes_hashed, 3);
    assert_eq!(
        result.stored_at.as_deref(),
        Some(fixture.verifications().path().as_path())
    );
    assert!(fixture.verifications().path().is_file());
    assert_eq!(fs::read(&fixture.media).unwrap(), b"abc");
    let next = fixture.run(fixture.verify_request()).unwrap();
    let RommOperationOutcome::Verified(next) = next else {
        panic!("expected verification")
    };
    assert_eq!(next.verdict_before, result.verdict_after);
}

#[test]
fn explicit_verification_preserves_disagreement_in_the_store_and_result() {
    let fixture = Fixture::new();
    fixture.publish(
        "00000000000000000000000000000000",
        ExternalVerification::StrongExternal,
    );
    let outcome = fixture.run(fixture.verify_request()).unwrap();
    let RommOperationOutcome::Verified(result) = outcome else {
        panic!("expected verification")
    };
    assert!(!result.all_agree);
    assert!(result.any_disagree);
    assert_eq!(result.comparisons.len(), 1);
    assert_eq!(result.comparisons[0].local, ABC_MD5);
    assert!(fixture.verifications().path().is_file());
    assert_eq!(fs::read(&fixture.media).unwrap(), b"abc");
}

#[test]
fn stale_record_binding_is_refused_before_hashing_or_writing() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let mut request = fixture.verify_request();
    if let RommOperation::VerifyLocalFile { romm_game_id, .. } = &mut request {
        *romm_game_id = "another-record".into();
    }
    assert!(fixture.run(request).unwrap_err().contains("no longer maps"));
    assert_eq!(fixture.files(), before);
}

#[test]
fn verification_refuses_a_file_outside_the_supplied_roots() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let error = fixture
        .run_with(fixture.verify_request(), false, Ok(Vec::new()))
        .unwrap_err();
    assert!(error.contains("not inside a configured source folder"));
    assert_eq!(fixture.files(), before);
}

#[cfg(unix)]
#[test]
fn verification_refuses_a_symlink_escaping_the_source_roots() {
    let fixture = Fixture::new();
    let link = fixture.library.join("escape.gb");
    std::os::unix::fs::symlink(fixture.settings().config_path(), &link).unwrap();
    assert!(
        confine_to_source_roots(&link, &[fixture.library.clone()])
            .unwrap_err()
            .contains("leads out")
    );
    assert!(!fixture.verifications().path().exists());
}

#[test]
fn cancelled_verification_does_not_publish_partial_hashes() {
    let fixture = Fixture::new();
    let before = fixture.files();
    assert!(
        fixture
            .run_with(
                fixture.verify_request(),
                true,
                Ok(vec![fixture.library.clone()])
            )
            .is_err()
    );
    assert_eq!(fixture.files(), before);
}

#[test]
fn cancelled_stale_summary_returns_no_partial_result_and_does_not_write() {
    let fixture = Fixture::new();
    fixture.publish(ABC_MD5, ExternalVerification::Stale);
    let before = fixture.files();
    let error = fixture
        .run_with(
            RommOperation::StaleSummary,
            true,
            Ok(vec![fixture.library.clone()]),
        )
        .unwrap_err();
    assert!(error.contains("cancelled"));
    assert_eq!(fixture.files(), before);
}

#[test]
fn unchanged_save_does_not_revalidate_token_or_rewrite_configuration() {
    let fixture = Fixture::new();
    let settings = fixture.settings().load().unwrap();
    let before = fixture.files();
    let outcome = fixture
        .run(RommOperation::SaveConfiguration(Box::new(settings.clone())))
        .unwrap();
    let RommOperationOutcome::Saved(saved) = outcome else {
        panic!("expected no-op save")
    };
    assert_eq!(*saved, settings);
    assert_eq!(fixture.files(), before);
}

#[test]
fn rejected_configuration_save_preserves_previous_files() {
    let fixture = Fixture::new();
    let mut settings = fixture.settings().load().unwrap();
    settings.source.url.clear();
    let before = fixture.files();
    assert!(
        fixture
            .run(RommOperation::SaveConfiguration(Box::new(settings)))
            .unwrap_err()
            .contains("address is required")
    );
    assert_eq!(fixture.files(), before);
}

#[test]
fn explicit_enable_persists_without_requiring_network_or_token() {
    let fixture = Fixture::new();
    assert!(!fixture.settings().load().unwrap().source.enabled);
    assert!(matches!(
        fixture.run(RommOperation::SetEnabled(true)).unwrap(),
        RommOperationOutcome::Enabled(true)
    ));
    assert!(fixture.settings().load().unwrap().source.enabled);
    assert_eq!(fs::read(&fixture.media).unwrap(), b"abc");
}

#[test]
fn missing_artwork_returns_feature_outcomes_without_network_or_writes() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let cover = fixture
        .run(RommOperation::LoadCover {
            local_path: fixture.media.clone(),
            romm_game_id: "synthetic-1".into(),
        })
        .unwrap();
    let RommOperationOutcome::Cover(cover) = cover else {
        panic!("expected cover")
    };
    assert!(matches!(
        cover.state,
        crate::romm_game::CoverState::Unavailable(_)
    ));
    let screenshot = fixture
        .run(RommOperation::LoadScreenshot {
            local_path: fixture.media.clone(),
            romm_game_id: "synthetic-1".into(),
        })
        .unwrap();
    let RommOperationOutcome::Screenshot(screenshot) = screenshot else {
        panic!("expected screenshot")
    };
    assert!(matches!(
        screenshot.state,
        crate::romm_game::CoverState::Failed(_)
    ));
    assert_eq!(fixture.files(), before);
}

#[test]
fn missing_manual_is_refused_without_opening_anything_or_writing() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let error = fixture
        .run(RommOperation::OpenManual {
            local_path: fixture.media.clone(),
            romm_game_id: "synthetic-1".into(),
        })
        .unwrap_err();
    assert!(error.contains("No manual is available"));
    assert_eq!(fixture.files(), before);
}

#[test]
fn network_operation_still_requires_a_token_and_keeps_existing_cache_on_refusal() {
    let fixture = Fixture::new();
    let before = fixture.files();
    assert!(fixture.run(RommOperation::Refresh).is_err());
    assert_eq!(fixture.files(), before);
}
