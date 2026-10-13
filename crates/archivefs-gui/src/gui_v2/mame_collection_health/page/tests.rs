use super::*;
use crate::dat_catalogue_picker::CatalogueInventorySnapshot;
use archivefs_core::dat::managed_sources::ManagedDatSources;
use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
use std::fs;

pub(in crate::gui_v2) struct Fixture {
    pub dir: tempfile::TempDir,
    pub request: Request,
}
impl Fixture {
    pub(in crate::gui_v2) fn new(machines: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalogue.xml");
        fs::write(&path, format!("<mame build=\"0.280\">{machines}</mame>")).unwrap();
        let root = dir.path().join("collection");
        fs::create_dir(&root).unwrap();
        let mut registry = DatSourceRegistry::new();
        registry
            .add(DatSourceEntry::new(
                "synthetic".into(),
                "Synthetic MAME arcade".into(),
                path.clone(),
                DatSourceKind::File,
            ))
            .unwrap();
        Self {
            request: Request {
                id: 1,
                reference: CatalogueRef::local("synthetic"),
                snapshot: CatalogueInventorySnapshot {
                    local_registry: registry,
                    managed_sources: ManagedDatSources::new(),
                    managed_root: dir.path().join("managed"),
                },
                catalogue_path: path,
                root,
                version_output: Some("MAME 0.280".into()),
            },
            dir,
        }
    }
    pub(in crate::gui_v2) fn rom(&self, set: &str, bytes: &[u8]) {
        fs::create_dir_all(self.request.root.join(set)).unwrap();
        fs::write(self.request.root.join(set).join("main.bin"), bytes).unwrap();
    }
    pub(in crate::gui_v2) fn report(&self) -> Report {
        worker::run(self.request.clone(), &AtomicBool::new(false), &mut |_| {}).unwrap()
    }
}
pub(in crate::gui_v2) const GOOD: &str = r#"<machine name="g"><description>Test Game</description><rom name="main.bin" size="11" sha1="2aae6c35c94fcfb415dbe95f408b9ce91ee846ed" crc="0d4a1185"/></machine>"#;

#[test]
fn populated_worker_report_preserves_actual_sources_hashes_and_receipt() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    let before = fs::read(&f.request.catalogue_path).unwrap();
    let report = f.report();
    assert_eq!(report.analysis.passing_sets, 1);
    assert!(report.inspection.inventory.inspected_completely);
    assert_eq!(
        report.analysis.set_results[0].evidence.requirements[0].content,
        archivefs_core::mame_collection_analyser::MameMemberEvidence::Verified
    );
    assert!(report.catalogue.recorded_author.is_none());
    assert_eq!(report.catalogue.sha256.len(), 64);
    let json: serde_json::Value = serde_json::from_str(&report.json).unwrap();
    assert_eq!(json["catalogue"]["source_id"], "synthetic");
    assert_eq!(
        json["inspection"]["inventory"]["inspected_completely"],
        true
    );
    assert_eq!(fs::read(&f.request.catalogue_path).unwrap(), before);
    assert_eq!(
        fs::read(f.request.root.join("g/main.bin")).unwrap(),
        b"hello world"
    );
    assert_eq!(fs::read_dir(f.dir.path()).unwrap().count(), 2);
}
#[test]
fn conflicting_sha1_overrides_matching_crc_in_real_worker() {
    let f =
        Fixture::new(&GOOD.replace("2aae6c35c94fcfb415dbe95f408b9ce91ee846ed", &"a".repeat(40)));
    f.rom("g", b"hello world");
    let report = f.report();
    assert_eq!(report.analysis.failing_sets, 1);
    assert_eq!(
        report.analysis.set_results[0].evidence.requirements[0].content,
        archivefs_core::mame_collection_analyser::MameMemberEvidence::Contradictory
    );
}
#[test]
fn missing_bios_and_device_dependencies_are_evidence_not_green_counts() {
    let xml = format!(
        r#"<machine name="bios" isbios="yes"><rom name="bios.bin" size="11" sha1="2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"/></machine><machine name="dev" isdevice="yes"><rom name="device.bin" size="11" sha1="2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"/></machine>{}"#,
        GOOD.replace(
            "<machine name=\"g\">",
            "<machine name=\"g\" romof=\"bios\"><device_ref name=\"dev\"/>"
        )
    );
    let f = Fixture::new(&xml);
    f.rom("g", b"hello world");
    let report = f.report();
    let g = report
        .analysis
        .set_results
        .iter()
        .find(|s| s.name == "g")
        .unwrap();
    assert_ne!(
        g.health,
        archivefs_core::mame_collection_analyser::MameSetHealth::Good
    );
    assert!(
        g.evidence
            .requirements
            .iter()
            .any(|r| r.declaring_set == "bios")
    );
    assert!(
        g.evidence
            .requirements
            .iter()
            .any(|r| r.declaring_set == "dev")
    );
    assert_eq!(report.context["g"].rom_of.as_deref(), Some("bios"));
    assert_eq!(report.context["g"].device_references, ["dev"]);
}
#[test]
fn misleading_chd_filename_is_unverified_and_absence_is_missing() {
    let f = Fixture::new(&GOOD.replace(
        "</machine>",
        &format!(
            "<disk name=\"disk\" sha1=\"{}\"/></machine>",
            "a".repeat(40)
        ),
    ));
    f.rom("g", b"hello world");
    let missing = f.report();
    assert_eq!(missing.analysis.chd_coverage.sets_missing_chd, 1);
    fs::write(f.request.root.join("g/disk.chd"), b"not a CHD").unwrap();
    let present = f.report();
    assert_eq!(present.analysis.passing_sets, 0);
    assert_eq!(present.analysis.chd_coverage.sets_unverified_chd, 1);
    assert!(
        present.inspection.inventory.sets[0]
            .members
            .iter()
            .find(|m| m.name == "disk.chd")
            .unwrap()
            .sha1
            .is_none()
    );
}
#[test]
fn malformed_software_list_and_stale_catalogue_fail_closed() {
    let f = Fixture::new(GOOD);
    for body in [
        "<mame><machine broken",
        "<softwarelist name=\"list\"><software name=\"g\"/></softwarelist>",
        "<datafile><game name=\"g\"><rom name=\"main.bin\"/></game></datafile>",
    ] {
        fs::write(&f.request.catalogue_path, body).unwrap();
        assert!(worker::run(f.request.clone(), &AtomicBool::new(false), &mut |_| {}).is_err());
    }
}
#[test]
fn cancellation_and_changed_source_during_worker_analysis_never_report_good() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    let cancel = AtomicBool::new(false);
    let report = worker::run(f.request.clone(), &cancel, &mut |_| {
        cancel.store(true, Ordering::Relaxed);
    })
    .unwrap();
    assert!(report.inspection.cancelled);
    assert_eq!(report.analysis.passing_sets, 0);
    let file = f.request.root.join("g/main.bin");
    let report = worker::run(f.request.clone(), &AtomicBool::new(false), &mut |p| {
        if p.current.to_string_lossy().contains("Calculating health") {
            fs::write(&file, b"changed file").unwrap();
        }
    })
    .unwrap();
    assert!(!report.inspection.inventory.inspected_completely);
    assert_eq!(report.analysis.passing_sets, 0);
}
#[test]
fn background_completion_is_bound_to_requested_selection_and_job_id() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    let mut page = HealthPage {
        selected: Some(f.request.reference.clone()),
        root: f.request.root.to_string_lossy().into(),
        version_output: "MAME 0.280".into(),
        ..Default::default()
    };
    let (sender, receiver) = mpsc::channel();
    page.job = Some(Job {
        request: f.request.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        receiver,
    });
    sender
        .send(Message::Finished(999, Err("cross-ID error".into())))
        .unwrap();
    page.poll();
    assert!(page.job.is_some());
    assert!(page.message.is_none());
    let mut wrong = f.report();
    wrong.job_id = 2;
    sender.send(Message::Finished(1, Ok(wrong))).unwrap();
    page.poll();
    assert!(page.report.is_none());
    assert!(page.message.unwrap().contains("does not match"));
}

#[test]
fn late_cancellation_cannot_adopt_a_prepared_good_report() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    let receipt = f.report();
    let mut page = HealthPage::default();
    page.selected = Some(f.request.reference.clone());
    page.root = f.request.root.to_string_lossy().into();
    page.version_output = "MAME 0.280".into();
    let (sender, receiver) = mpsc::channel();
    page.job = Some(Job {
        request: f.request.clone(),
        cancel: Arc::new(AtomicBool::new(true)),
        receiver,
    });
    sender.send(Message::Finished(1, Ok(receipt))).unwrap();
    page.poll();
    assert!(
        page.report.is_none(),
        "late Cancel adopted a completed Good receipt"
    );
    assert!(page.message.as_ref().unwrap().contains("cancelled"));
}
#[test]
fn repeated_requests_do_not_start_an_overlapping_worker_or_reuse_old_success() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    let mut page = HealthPage {
        report: Some(f.report().into()),
        ..Default::default()
    };
    page.invalidate_inputs();
    assert!(page.report.is_none());
    let (_sender, receiver) = mpsc::channel();
    page.job = Some(Job {
        request: f.request,
        cancel: Arc::new(AtomicBool::new(false)),
        receiver,
    });
    page.start();
    assert_eq!(page.job.as_ref().unwrap().request.id, 1);
}

#[test]
fn refused_inspection_does_not_keep_an_old_success_report() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    let mut page = HealthPage::default();
    page.report = Some(f.report().into());
    page.start();
    assert!(
        page.report.is_none(),
        "failed inspection retained the previous green report"
    );
    assert!(
        page.message
            .as_ref()
            .unwrap()
            .contains("Choose a MAME arcade catalogue")
    );
    assert!(page.job.is_none());
}
#[test]
fn unsupported_inspection_and_empty_collection_are_not_verified_complete() {
    let f = Fixture::new(GOOD);
    let empty = f.report();
    assert_eq!(empty.analysis.discovered_sets, 0);
    assert_eq!(empty.analysis.passing_sets, 0);
    fs::write(f.request.root.join("bad.7z"), b"unsupported").unwrap();
    let partial = f.report();
    assert!(!partial.inspection.inventory.inspected_completely);
    assert_eq!(partial.analysis.passing_sets, 0);
}

#[test]
fn changed_sources_after_completion_withdraw_the_displayed_report() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    for catalogue in [false, true] {
        let mut page = HealthPage {
            report: Some(f.report().into()),
            ..Default::default()
        };
        let path = if catalogue {
            &f.request.catalogue_path
        } else {
            &f.request.root.join("g/main.bin")
        };
        let bytes = fs::read(path).unwrap();
        fs::write(path, b"changed since report completed").unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while page.report.is_some() && std::time::Instant::now() < end {
            page.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(
            page.report.is_none(),
            "stale verified report remained visible"
        );
        assert!(page.message.as_ref().unwrap().contains("Sources changed"));
        assert_eq!(fs::read(path).unwrap(), b"changed since report completed");
        fs::write(path, bytes).unwrap();
    }
}

#[test]
fn background_cancel_completion_preserves_sources_and_clears_old_success() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    let before = fs::read(&f.request.catalogue_path).unwrap();
    let mut page = HealthPage::default();
    page.selected = Some(f.request.reference.clone());
    page.root = f.request.root.to_string_lossy().into();
    page.version_output = "MAME 0.280".into();
    page.report = Some(f.report().into());
    page.picker.ensure_loaded_with(f.request.snapshot.clone());
    let end = Instant::now() + Duration::from_secs(3);
    while page.picker.loading && Instant::now() < end {
        page.picker.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
    page.start();
    assert!(page.job.is_some());
    assert!(page.report.is_none());
    page.job
        .as_ref()
        .unwrap()
        .cancel
        .store(true, Ordering::Relaxed);
    while page.job.is_some() && Instant::now() < end {
        page.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(page.job.is_none());
    assert!(
        page.report
            .as_ref()
            .is_none_or(|r| r.analysis.passing_sets == 0)
    );
    assert!(
        page.message
            .as_ref()
            .is_some_and(|m| m.contains("cancel") || m.contains("incomplete"))
    );
    assert_eq!(fs::read(&f.request.catalogue_path).unwrap(), before);
    assert_eq!(
        fs::read(f.request.root.join("g/main.bin")).unwrap(),
        b"hello world"
    );
}

#[test]
fn export_serialization_is_bounded_and_keeps_complete_small_receipts() {
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    let report = f.report();
    assert!(worker::bounded_json(&report, 64).is_err());
    assert!(!report.json.is_empty());
    let parsed: serde_json::Value = serde_json::from_str(&report.json).unwrap();
    assert_eq!(
        parsed["context"]["g"]["rom_declarations"][0]["sha1"],
        "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
    );
}

#[test]
fn version_input_needs_mame_product_evidence_and_keeps_the_supplied_text() {
    use archivefs_core::mame_collection_analyser::MameVersionCompatibility as V;
    let f = Fixture::new(GOOD);
    f.rom("g", b"hello world");
    for output in ["0.280", "DuckStation 0.280", "é0.280", "MAME 0.280 0.279"] {
        let mut request = f.request.clone();
        request.version_output = Some(output.into());
        let report = worker::run(request, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(
            report.analysis.version_compatibility,
            V::ExactSourceVersionUnknown,
            "unidentified or invalid product: {output}"
        );
        assert_eq!(report.analysis.emulator_version.as_deref(), Some(output));
    }
    for (output, expected) in [
        ("MAME 0.280", V::CatalogueVersionMatch),
        ("0.280 (mame0280)", V::CatalogueVersionMatch),
        ("MAME 0.279", V::CatalogueVersionMismatch),
    ] {
        let mut request = f.request.clone();
        request.version_output = Some(output.into());
        let report = worker::run(request, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(report.analysis.version_compatibility, expected);
    }
}

#[test]
fn duplicate_machine_metadata_is_not_hidden_and_untrusted_text_is_inert() {
    let f = Fixture::new(&format!("{GOOD}{GOOD}"));
    f.rom("g", b"hello world");
    let report = f.report();
    assert!(report.context["g"].duplicate_machine_declarations);
    assert_eq!(report.context["g"].rom_declarations.len(), 2);
    assert_eq!(report.analysis.passing_sets, 0);
    let value = super::super::view::text("catalogue\nGood 999\r\t\0\u{2028}é");
    assert!(!value.chars().any(|c| c.is_control() || c == '\u{2028}'));
    assert!(value.contains("\\nGood 999"));
    assert!(value.ends_with('é'));
    assert!(super::super::view::text(&"é".repeat(5000)).len() < 1200);
}

#[test]
fn real_inventory_preserves_split_and_merged_clone_semantics() {
    let parent = GOOD.replace("name=\"g\"", "name=\"parent\"");
    let split_clone = GOOD
        .replace(
            "<machine name=\"g\">",
            "<machine name=\"clone\" cloneof=\"parent\" romof=\"parent\">",
        )
        .replace(
            "name=\"main.bin\"",
            "name=\"borrowed.bin\" merge=\"main.bin\"",
        );
    let f = Fixture::new(&format!("{parent}{split_clone}"));
    f.rom("parent", b"hello world");
    fs::create_dir(f.request.root.join("clone")).unwrap();
    let report = f.report();
    assert_eq!(
        report
            .analysis
            .set_results
            .iter()
            .find(|s| s.name == "clone")
            .unwrap()
            .health,
        archivefs_core::mame_collection_analyser::MameSetHealth::Good
    );
    let merged_clone = GOOD
        .replace(
            "<machine name=\"g\">",
            "<machine name=\"clone\" cloneof=\"parent\">",
        )
        .replace("name=\"main.bin\"", "name=\"unique.bin\"");
    let f = Fixture::new(&format!("{parent}{merged_clone}"));
    f.rom("parent", b"hello world");
    fs::create_dir(f.request.root.join("parent/clone")).unwrap();
    fs::write(
        f.request.root.join("parent/clone/unique.bin"),
        b"hello world",
    )
    .unwrap();
    let report = f.report();
    let clone = report
        .analysis
        .set_results
        .iter()
        .find(|s| s.name == "clone")
        .unwrap();
    assert_eq!(
        clone.health,
        archivefs_core::mame_collection_analyser::MameSetHealth::Good
    );
    assert!(clone.evidence.requirements.iter().all(
        |r| r.content == archivefs_core::mame_collection_analyser::MameMemberEvidence::Verified
    ));
}
