use super::*;
use archivefs_core::dat::updates::{
    ManagedDatHttpRequest, ManagedDatHttpResponse, ManagedDatSnapshot, ManagedDatTransport,
    ManagedDatTransportError, managed_dat_state_path, save_managed_dat_state,
};
use std::cell::Cell;

fn descriptor(name: &str) -> ManagedDatSourceDescriptor {
    ManagedDatSourceDescriptor::mame_software_list(name)
        .unwrap()
        .with_update_policy(ManagedDatUpdatePolicy::Manual)
}

fn page(root: &Path) -> DatSourcesPageState {
    let journal = root.join("journal");
    std::fs::create_dir_all(&journal).unwrap();
    DatSourcesPageState::load_with_transaction_dir(
        root.join("dat_sources.toml"),
        Vec::new(),
        TrustedRoots::none(),
        journal,
    )
}

fn install(root: &Path, descriptor: &ManagedDatSourceDescriptor) -> (String, PathBuf) {
    let bytes = b"<softwarelist name=\"gamecom\"><software name=\"test\"/></softwarelist>";
    let hash: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let objects = root
        .join(descriptor.source_id().storage_relative_path())
        .join("objects");
    std::fs::create_dir_all(&objects).unwrap();
    let object = objects.join(&hash);
    std::fs::write(&object, bytes).unwrap();
    let mut state =
        ManagedDatState::new(descriptor, ManagedDatSnapshot::new(hash.clone()).unwrap()).unwrap();
    state.upstream_revision = Some("a".repeat(40));
    state.validation_summary = Some("Existing verification evidence".into());
    save_managed_dat_state(root, &state).unwrap();
    (hash, object)
}

struct MockTransport {
    calls: Cell<usize>,
    status: u16,
    body: Vec<u8>,
}
impl ManagedDatTransport for MockTransport {
    fn get(
        &self,
        request: &ManagedDatHttpRequest,
        _: u64,
        destination: &mut dyn std::io::Write,
    ) -> Result<ManagedDatHttpResponse, ManagedDatTransportError> {
        self.calls.set(self.calls.get() + 1);
        assert!(request.url.starts_with("https://api.github.com/"));
        destination.write_all(&self.body).unwrap();
        Ok(ManagedDatHttpResponse {
            status: self.status,
            content_length: Some(self.body.len() as u64),
            etag: None,
            last_modified: None,
            retry_after_seconds: Some(120),
            downloaded_bytes: self.body.len() as u64,
        })
    }
}

#[test]
fn existing_provider_checks_preserve_catalogue_database_snapshot_and_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("managed");
    let descriptor = descriptor("gamecom");
    let (hash, object) = install(&root, &descriptor);
    let database_path = temp.path().join("catalogue.sqlite");
    drop(archivefs_core::Database::open_or_create(&database_path).unwrap());
    let database_before = std::fs::read(&database_path).unwrap();
    let object_before = std::fs::read(&object).unwrap();
    let before = load_managed_dat_state(&root, &descriptor).unwrap();
    for (revision, expected) in [("a".repeat(40), true), ("b".repeat(40), false)] {
        let transport = MockTransport {
            calls: Cell::new(0),
            status: 200,
            body: format!("{{\"sha\":\"{revision}\"}}").into_bytes(),
        };
        let outcome = checked_provider(&root, &descriptor, &hash, &transport).unwrap();
        assert_eq!(
            matches!(outcome, ManagedDatUpdateOutcome::UpToDate { .. }),
            expected
        );
        assert_eq!(transport.calls.get(), 1);
        assert_eq!(std::fs::read(&database_path).unwrap(), database_before);
        assert_eq!(std::fs::read(&object).unwrap(), object_before);
        let after = load_managed_dat_state(&root, &descriptor).unwrap();
        assert_eq!(after.source_id, before.source_id);
        assert_eq!(after.current_snapshot, before.current_snapshot);
        assert_eq!(after.sha256, before.sha256);
        assert_eq!(after.validation_summary, before.validation_summary);
        assert_eq!(after.upstream_revision, before.upstream_revision);
        assert_eq!(after.previous_snapshot, before.previous_snapshot);
    }
}

#[test]
fn corrupt_missing_and_malformed_metadata_fail_before_network() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("managed");
    let descriptor = descriptor("gamecom");
    let (hash, object) = install(&root, &descriptor);
    let transport = MockTransport {
        calls: Cell::new(0),
        status: 304,
        body: Vec::new(),
    };
    std::fs::write(&object, "corrupt bytes").unwrap();
    assert!(checked_provider(&root, &descriptor, &hash, &transport).is_err());
    std::fs::remove_file(&object).unwrap();
    assert!(checked_provider(&root, &descriptor, &hash, &transport).is_err());
    let state_path = managed_dat_state_path(&root, descriptor.source_id()).unwrap();
    std::fs::write(&state_path, "{broken").unwrap();
    assert!(checked_provider(&root, &descriptor, &hash, &transport).is_err());
    std::fs::remove_file(&state_path).unwrap();
    assert!(checked_provider(&root, &descriptor, &hash, &transport).is_err());
    assert_eq!(transport.calls.get(), 0);
}

#[test]
fn malformed_remote_response_fails_without_replacing_catalogue() {
    let temp = tempfile::tempdir().unwrap();
    let descriptor = descriptor("gamecom");
    let (hash, object) = install(temp.path(), &descriptor);
    let before = std::fs::read(&object).unwrap();
    let transport = MockTransport {
        calls: Cell::new(0),
        status: 200,
        body: b"{broken".to_vec(),
    };
    let outcome = checked_provider(temp.path(), &descriptor, &hash, &transport).unwrap();
    assert!(matches!(outcome, ManagedDatUpdateOutcome::Failed { .. }));
    assert_eq!(std::fs::read(object).unwrap(), before);
}

#[test]
fn rate_limit_blocks_same_service_across_datasets_and_runs_but_not_other_providers() {
    let tasks = vec![
        (descriptor("gamecom"), "a".repeat(64)),
        (descriptor("nes"), "b".repeat(64)),
        (
            ManagedDatSourceDescriptor::redump_bios(RedumpBiosSystem::PlayStation).unwrap(),
            "c".repeat(64),
        ),
    ];
    let mut calls = Vec::new();
    let batch = run_checks(
        tasks.clone(),
        BTreeMap::new(),
        || 100,
        |descriptor, _| {
            calls.push(service(descriptor.source_id().provider));
            Ok(
                if descriptor.source_id().provider == ManagedDatProvider::MameSoftwareList {
                    ManagedDatUpdateOutcome::RateLimited {
                        retry_after_seconds: Some(120),
                    }
                } else {
                    ManagedDatUpdateOutcome::UpToDate {
                        upstream_revision: Some("known".into()),
                    }
                },
            )
        },
        || panic!("cooldown must not sleep"),
    );
    assert_eq!(calls, vec!["github", "redump"]);
    assert_eq!(batch.checks.len(), 3);
    assert_eq!(batch.cooldowns["github"], 220);
    let again = run_checks(
        vec![tasks[0].clone()],
        batch.cooldowns,
        || 200,
        |_, _| panic!("cooldown must prevent network"),
        || panic!(),
    );
    assert!(matches!(
        again.checks[0].1.result,
        CheckResult::Failed { .. }
    ));
}

#[test]
fn offline_and_provider_failure_are_isolated_and_checks_are_serially_paced() {
    let mut called = 0;
    let mut paced = 0;
    let tasks = vec![
        (descriptor("gamecom"), "a".repeat(64)),
        (descriptor("nes"), "b".repeat(64)),
        (descriptor("snes"), "c".repeat(64)),
    ];
    let batch = run_checks(
        tasks,
        BTreeMap::new(),
        || 100,
        |_, _| {
            called += 1;
            match called {
                1 => Ok(ManagedDatUpdateOutcome::Offline),
                2 => Err(archivefs_core::ArchiveFsError::Config(
                    "provider failed".into(),
                )),
                _ => Ok(ManagedDatUpdateOutcome::UpToDate {
                    upstream_revision: Some("known".into()),
                }),
            }
        },
        || paced += 1,
    );
    assert_eq!(called, 3);
    assert_eq!(paced, 2);
    assert!(matches!(
        batch.checks[0].1.result,
        CheckResult::Failed { .. }
    ));
    assert!(matches!(
        batch.checks[1].1.result,
        CheckResult::Failed { .. }
    ));
    assert_eq!(batch.checks[2].1.result, CheckResult::Current);
}

#[test]
fn browsing_blue_dat_page_does_not_start_checks_or_modify_catalogue() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    page.add_managed_mame_software_list("gamecom".into());
    install(&page.managed_root, &descriptor("gamecom"));
    let database_path = temp.path().join("catalogue.sqlite");
    drop(archivefs_core::Database::open_or_create(&database_path).unwrap());
    let before = std::fs::read(&database_path).unwrap();
    let view = page.view();
    assert_eq!(view.update_health.len(), 1);
    assert_eq!(
        view.update_health[0].assessment.health,
        archivefs_core::dat::update_health::Health::Unknown
    );
    let mut ui_state = DatSourcesPageUi::default();
    let context = egui::Context::default();
    let _ = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(show_dat_sources_page(ui, &view, &mut ui_state).is_none());
        });
    });
    assert!(page.update_health.job.is_none());
    assert!(page.managed_job.is_none());
    assert_eq!(std::fs::read(database_path).unwrap(), before);
}

#[test]
fn duplicate_catalogue_identity_is_a_conflict_not_an_update_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    for (id, filename) in [("one", "one.dat"), ("two", "two.dat")] {
        let path = temp.path().join(filename);
        std::fs::write(&path, "<datafile><header><name>Same identity</name><version>20261017</version></header><game name=\"test\"><rom name=\"a\" size=\"1\" crc=\"12345678\"/></game></datafile>").unwrap();
        let entry = DatSourceEntry::new(id.into(), id.into(), path, DatSourceKind::File);
        let report = validate_dat_source(&entry, DatLimits::default());
        page.validations.insert(id.into(), report.0);
        page.draft.add(entry).unwrap();
    }
    let view = page.view();
    assert_eq!(view.update_health.len(), 2);
    assert!(
        view.update_health
            .iter()
            .all(|row| row.plan == PlanDisposition::Conflict)
    );
}

#[test]
fn failed_provider_can_be_rechecked_and_success_remains_hash_bound() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    page.add_managed_mame_software_list("gamecom".into());
    let descriptor = descriptor("gamecom");
    let (hash, _) = install(&page.managed_root, &descriptor);
    let check = Check {
        installed_sha256: hash,
        checked_at: now_unix_seconds(),
        available_version: None,
        result: CheckResult::Failed {
            detail: "offline".into(),
        },
    };
    let (sender, receiver) = sync_channel(1);
    sender
        .send(BatchResult {
            checks: vec![(descriptor.source_id().clone(), check)],
            cooldowns: BTreeMap::new(),
        })
        .unwrap();
    page.update_health.job = Some(receiver);
    assert!(page.poll_health_checks());
    let view = page.view();
    assert!(supports_check(&view.managed_rows[0]));
    assert_eq!(view.update_health[0].plan, PlanDisposition::CheckFirst);
    assert_eq!(
        view.update_health[0].assessment.health,
        archivefs_core::dat::update_health::Health::CheckFailed
    );
}

#[test]
fn persisted_check_time_is_visible_without_inventing_currentness() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    page.add_managed_mame_software_list("gamecom".into());
    let descriptor = descriptor("gamecom");
    install(&page.managed_root, &descriptor);
    let mut state = load_managed_dat_state(&page.managed_root, &descriptor).unwrap();
    state.last_checked_at_unix_seconds = Some(100);
    save_managed_dat_state(&page.managed_root, &state).unwrap();
    let view = page.view();
    assert_eq!(
        view.update_health[0].last_checked_display,
        Some(format_unix_timestamp(100))
    );
    assert_eq!(
        view.update_health[0].assessment.health,
        archivefs_core::dat::update_health::Health::Unknown
    );
}

#[test]
fn cached_no_intro_versions_are_visible_without_turning_active_into_current() {
    use archivefs_core::identity_source::no_intro::{
        NoIntroPackMemberEvidence, NoIntroPackSnapshot, NoIntroPackStatus, NoIntroVariant,
        report_no_intro_lifecycle,
    };
    let temp = tempfile::tempdir().unwrap();
    let snapshot = NoIntroPackSnapshot {
        pack_sha256: "b".repeat(64),
        snapshot_sha256: "c".repeat(64),
        imported_at_unix_seconds: 100,
        import_order: 1,
        status: NoIntroPackStatus::Current,
        members: vec![NoIntroPackMemberEvidence {
            source_member_name: "Game Boy.dat".into(),
            system_name: "Game Boy".into(),
            variant: NoIntroVariant::Headerless,
            artifact_sha256: "a".repeat(64),
            upstream_version: Some("20261017".into()),
        }],
        coverage: Vec::new(),
        predecessor: None,
        successor: None,
        rejected: Vec::new(),
    };
    let status = report_no_intro_lifecycle(&[snapshot]);
    let path = temp
        .path()
        .join("snapshots")
        .join("c".repeat(64))
        .join("dats/0.dat");
    let (version, hash, _) = cached_no_intro_metadata(&status, temp.path(), &path).unwrap();
    assert_eq!(version.as_deref(), Some("20261017"));
    assert_eq!(hash.as_deref(), Some("a".repeat(64).as_str()));
    assert_eq!(
        assess_health(hash.as_deref(), &CheckHistory::default(), 101).health,
        archivefs_core::dat::update_health::Health::Unknown
    );
    assert!(
        cached_no_intro_metadata(&status, temp.path(), &temp.path().join("unrelated.dat"))
            .is_none()
    );
}

#[test]
fn tosec_release_metadata_only_describes_owned_materialized_members() {
    let temp = tempfile::tempdir().unwrap();
    let mut page = page(temp.path());
    page.tosec_managed_view.active_sha256 = Some("a".repeat(64));
    page.tosec_managed_view.active_release = Some("TOSEC release".into());
    page.tosec_managed_view.active_source_path = Some(temp.path().join("original-import"));
    let mut entry = DatSourceEntry::new(
        "tosec".into(),
        "TOSEC".into(),
        temp.path().join("original-import/game.dat"),
        DatSourceKind::File,
    );
    entry.ownership = archivefs_core::dat::sources::DatSourceOwnership::ImportedTosecReleasePack {
        pack_id: "pack".into(),
        relative_path: PathBuf::from("game.dat"),
    };
    assert_eq!(page.cached_pack_metadata(&entry).0, None);
    entry.path = page
        .managed_root
        .join("tosec-snapshots/materialized")
        .join("a".repeat(64))
        .join("game.dat");
    assert_eq!(
        page.cached_pack_metadata(&entry).0.as_deref(),
        Some("TOSEC release")
    );
    entry.ownership = archivefs_core::dat::sources::DatSourceOwnership::UserLocal;
    assert_eq!(page.cached_pack_metadata(&entry).0, None);
}
