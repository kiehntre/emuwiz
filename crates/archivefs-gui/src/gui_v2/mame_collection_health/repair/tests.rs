use super::super::page::tests::{Fixture, GOOD};
use super::*;
use archivefs_core::dat::rename_apply::{
    EntryState, RollbackResult, TransactionState,
    journal::{list_journals, read_journal, write_journal},
    reconcile_recovery, rollback_transaction_confined,
};

fn fixture() -> Fixture {
    let f = Fixture::new(GOOD);
    f.rom("donor", b"hello world");
    std::fs::create_dir(f.request.root.join("g")).unwrap();
    f
}
fn options(f: &Fixture) -> MameInternalRepairApplyOptions {
    MameInternalRepairApplyOptions {
        journal_dir: Some(f.dir.path().join("journals")),
        ..Default::default()
    }
}
#[test]
fn blue_review_apply_reload_undo_uses_existing_journal() {
    let f = fixture();
    let report = f.report();
    let reviewed = report.repair.as_ref().expect("supported exploded repair");
    assert_eq!(reviewed.apply.operations.len(), 1);
    assert_eq!(
        reviewed.apply.operations[0].destination_path,
        f.request.root.join("g/main.bin")
    );
    let opts = options(&f);
    let notice = execute(reviewed, &opts, &mut |_| {}).unwrap();
    assert!(notice.starts_with("Applied transaction"), "{notice}");
    assert_eq!(
        std::fs::read(f.request.root.join("g/main.bin")).unwrap(),
        b"hello world"
    );
    let (records, warnings) = list_journals(opts.journal_dir.as_ref().unwrap());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(records.len(), 1);
    let path = archivefs_core::dat::rename_apply::journal_path(
        opts.journal_dir.as_ref().unwrap(),
        &records[0].transaction_id,
    )
    .unwrap();
    let mut tx = read_journal(&path).unwrap();
    assert_eq!(tx.unknown["mame_catalogue_sha256"], report.catalogue.sha256);
    assert_eq!(tx.unknown["mame_reviewed_snapshot_digest"], reviewed.digest);
    let rows = crate::gui_v2::history_view::build_entries(
        &crate::gui_v2::history_view::HistorySources {
            quarantine: &[],
            playing_library: &[],
            organisation: std::slice::from_ref(&tx),
            mame: &[],
            evidence: &[],
        },
        None,
    );
    assert_eq!(
        rows[0].source,
        crate::gui_v2::history_view::Source::MameInternalRepair
    );
    assert_eq!(rows[0].family, crate::gui_v2::history_view::Family::Mame);
    assert!(crate::gui_v2::history_view::check_undo_safety(&rows[0]).safe());
    let result = rollback_transaction_confined(
        &mut tx,
        opts.journal_dir.as_ref().unwrap(),
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([&f.request.root]),
    )
    .unwrap();
    assert_eq!(result.result, RollbackResult::FullyRolledBack);
    assert!(!f.request.root.join("g/main.bin").exists());
    assert_eq!(
        std::fs::read(f.request.root.join("donor/main.bin")).unwrap(),
        b"hello world"
    );
}
#[test]
fn interrupted_apply_and_undo_recover_from_exact_journalled_slots() {
    let f = fixture();
    let report = f.report();
    let opts = options(&f);
    execute(report.repair.as_ref().unwrap(), &opts, &mut |_| {}).unwrap();
    let (mut records, _) = list_journals(opts.journal_dir.as_ref().unwrap());
    let mut tx = records.remove(0);
    // Deterministic crash window: filesystem publication completed; final
    // Applied checkpoint absent. The created-object proof was checkpointed.
    tx.state = TransactionState::Applying;
    tx.entries[0].state = EntryState::Applying;
    write_journal(opts.journal_dir.as_ref().unwrap(), &tx).unwrap();
    reconcile_recovery(&mut tx, opts.journal_dir.as_ref().unwrap()).unwrap();
    assert_eq!(tx.state, TransactionState::Applied);
    let outcome = rollback_transaction_confined(
        &mut tx,
        opts.journal_dir.as_ref().unwrap(),
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([&f.request.root]),
    )
    .unwrap();
    assert_eq!(outcome.result, RollbackResult::FullyRolledBack);
    tx.state = TransactionState::RollingBack;
    tx.entries[0].state = EntryState::RollingBack;
    write_journal(opts.journal_dir.as_ref().unwrap(), &tx).unwrap();
    reconcile_recovery(&mut tx, opts.journal_dir.as_ref().unwrap()).unwrap();
    assert_eq!(tx.entries[0].state, EntryState::RolledBack);
    assert!(!f.request.root.join("g/main.bin").exists());
    let after = f.report();
    assert!(after.inspection.inventory.inspected_completely);
    assert!(
        after
            .inspection
            .inventory
            .sets
            .iter()
            .flat_map(|s| &s.members)
            .all(|m| !m.name.contains(".emuwiz-copy-"))
    );
    assert!(
        after
            .repair
            .as_ref()
            .unwrap()
            .plan
            .requirements
            .iter()
            .flat_map(|r| &r.exact_matching_sources)
            .all(|s| !s.member.contains(".emuwiz-copy-"))
    );
}

#[test]
fn stale_catalogue_files_and_new_destinations_reject_before_journal() {
    for change in 0..4 {
        let f = fixture();
        let report = f.report();
        match change {
            0 => std::fs::write(
                &f.request.catalogue_path,
                format!("<mame build=\"0.281\">{GOOD}</mame>"),
            )
            .unwrap(),
            1 => std::fs::write(f.request.root.join("donor/main.bin"), b"changed rom").unwrap(),
            2 => std::fs::write(f.request.root.join("g/main.bin"), b"foreign rom").unwrap(),
            _ => std::fs::create_dir(f.request.root.join("new-set")).unwrap(),
        }
        let opts = options(&f);
        assert!(execute(report.repair.as_ref().unwrap(), &opts, &mut |_| {}).is_err());
        assert!(!opts.journal_dir.unwrap().exists());
    }
}
#[test]
fn exact_copy_inode_is_required_for_undo_and_recovery() {
    let f = fixture();
    let report = f.report();
    let opts = options(&f);
    execute(report.repair.as_ref().unwrap(), &opts, &mut |_| {}).unwrap();
    let (records, warnings) = list_journals(opts.journal_dir.as_ref().unwrap());
    assert!(warnings.is_empty(), "{warnings:?}");
    let path = archivefs_core::dat::rename_apply::journal_path(
        opts.journal_dir.as_ref().unwrap(),
        &records[0].transaction_id,
    )
    .unwrap();
    let mut tx = read_journal(&path).unwrap();
    let dest = f.request.root.join("g/main.bin");
    std::fs::rename(&dest, f.request.root.join("retained-original")).unwrap();
    std::fs::write(&dest, b"hello world").unwrap();
    let outcome = rollback_transaction_confined(
        &mut tx,
        opts.journal_dir.as_ref().unwrap(),
        &AtomicBool::new(false),
        &TrustedRoots::from_paths([&f.request.root]),
    )
    .unwrap();
    assert_ne!(outcome.result, RollbackResult::FullyRolledBack);
    assert_eq!(std::fs::read(&dest).unwrap(), b"hello world");
    tx.state = TransactionState::Applying;
    tx.entries[0].state = EntryState::Applying;
    write_journal(opts.journal_dir.as_ref().unwrap(), &tx).unwrap();
    let _recovery = reconcile_recovery(&mut tx, opts.journal_dir.as_ref().unwrap()).unwrap();
    assert_ne!(tx.state, TransactionState::Applied);
}
#[test]
fn ambiguous_dependencies_and_chd_are_never_apply_authority() {
    let f = Fixture::new(&GOOD.replace(
        "<machine name=\"g\">",
        "<machine name=\"g\"><device_ref name=\"missing-device\"/>",
    ));
    f.rom("donor", b"hello world");
    std::fs::create_dir(f.request.root.join("g")).unwrap();
    let report = f.report();
    assert!(report.repair.is_none());
    let f = Fixture::new(&GOOD.replace(
        "<rom name=\"main.bin\" size=\"11\"",
        "<disk name=\"main.bin\"",
    ));
    f.rom("donor", b"hello world");
    std::fs::create_dir(f.request.root.join("g")).unwrap();
    let report = f.report();
    if let Some(r) = report.repair {
        assert!(r.apply.transaction.is_none());
    }
}

#[test]
fn actual_blue_apply_button_runs_transaction_and_exposes_history_handoff() {
    use eframe::egui;
    let f = fixture();
    let report = f.report();
    let context = egui::Context::default();
    let mut state = State {
        journal_dir: options(&f).journal_dir,
        ..Default::default()
    };
    fn render(
        context: &egui::Context,
        state: &mut State,
        report: &Report,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 2200.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| state.show(ui, Some(report)));
            },
        )
    }
    fn text_rect(out: &egui::FullOutput, wanted: &str) -> egui::Rect {
        out.shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == wanted => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Missing actual control {wanted}"))
    }
    // With no confirmation, a click on the rendered Apply control must not run.
    let out = render(&context, &mut state, &report, vec![]);
    let button = text_rect(&out, "Apply internal repairs");
    for pressed in [true, false] {
        render(
            &context,
            &mut state,
            &report,
            vec![
                egui::Event::PointerMoved(button.center()),
                egui::Event::PointerButton {
                    pos: button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(!state.busy());
    assert!(!f.request.root.join("g/main.bin").exists());
    state.confirmation = "APPLY".into();
    let out = render(&context, &mut state, &report, vec![]);
    let button = text_rect(&out, "Apply internal repairs");
    for pressed in [true, false] {
        render(
            &context,
            &mut state,
            &report,
            vec![
                egui::Event::PointerMoved(button.center()),
                egui::Event::PointerButton {
                    pos: button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(state.busy());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.busy() && std::time::Instant::now() < deadline {
        state.poll();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!state.busy());
    assert!(
        state
            .notice
            .as_deref()
            .unwrap()
            .starts_with("Applied transaction"),
        "{:?}",
        state.notice
    );
    assert_eq!(
        std::fs::read(f.request.root.join("g/main.bin")).unwrap(),
        b"hello world"
    );
    let out = render(&context, &mut state, &report, vec![]);
    let button = text_rect(&out, "Open History & Undo / recovery");
    for pressed in [true, false] {
        render(
            &context,
            &mut state,
            &report,
            vec![
                egui::Event::PointerMoved(button.center()),
                egui::Event::PointerButton {
                    pos: button.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(state.open_history);
}

#[test]
fn duplicate_observed_containers_and_supplied_release_mismatch_block_apply() {
    let f = fixture();
    zip::ZipWriter::new(std::fs::File::create(f.request.root.join("g.zip")).unwrap())
        .finish()
        .unwrap();
    let report = f.report();
    assert!(report.repair.is_none());
    assert!(
        report
            .repair_unavailable
            .unwrap()
            .contains("multiple collection containers")
    );
    let mut f = fixture();
    f.request.version_output = Some("MAME 0.279".into());
    let report = f.report();
    assert!(report.repair.is_none());
    assert!(report.repair_unavailable.unwrap().contains("version"));
}

#[test]
fn exact_preview_escapes_control_characters_in_affected_paths() {
    use eframe::egui;
    let f = fixture();
    // An actual fixture filename, including newline and bidi control.
    let hostile = f.request.root.join("donor/line\n\u{202e}.bin");
    std::fs::rename(f.request.root.join("donor/main.bin"), &hostile).unwrap();
    let report = f.report();
    assert_eq!(
        report.repair.as_ref().unwrap().apply.operations[0].selected_source,
        hostile
    );
    let context = egui::Context::default();
    let mut state = State::default();
    let out = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 2200.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui, Some(&report)));
        },
    );
    let labels: Vec<_> = out
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some(t.galley.text()),
            _ => None,
        })
        .collect();
    assert!(labels.iter().all(|s| !s.contains('\u{202e}')));
    assert!(
        labels
            .iter()
            .any(|s| s.contains("\\u{202e}") && s.contains("\\n")),
        "{labels:?}"
    );
}

#[test]
fn blue_repair_undo_runs_through_the_native_history_controller() {
    let f = fixture();
    let report = f.report();
    let opts = options(&f);
    execute(report.repair.as_ref().unwrap(), &opts, &mut |_| {}).unwrap();
    let mut history = crate::repair_history_page::RepairHistoryPageState::load_with_journal_dir(
        opts.journal_dir.unwrap(),
    );
    let id = history.transactions[0].transaction_id.clone();
    assert!(history.can_undo(&id));
    let description = crate::repair_history_page::presentation::undo_confirmation_description(
        history.transaction_by_id(&id),
    );
    assert!(description.contains("private recovery staging"));
    assert!(description.contains("Original source files stay in place"));
    history.open_undo_confirmation(&id);
    assert_eq!(history.undo_confirm.as_ref().unwrap().transaction_id, id);
    history.confirm_undo();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while history.is_undo_running() && std::time::Instant::now() < deadline {
        history.poll_undo();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!history.is_undo_running());
    assert!(history.undo_error.is_none(), "{:?}", history.undo_error);
    assert_eq!(
        history.undo_outcome.as_ref().unwrap().result,
        RollbackResult::FullyRolledBack
    );
    assert!(!f.request.root.join("g/main.bin").exists());
    assert_eq!(
        std::fs::read(f.request.root.join("donor/main.bin")).unwrap(),
        b"hello world"
    );
    assert_eq!(
        history.transaction_by_id(&id).unwrap().state,
        TransactionState::RolledBack
    );
}

mod mame_check {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    /// Writes an executable script and waits until it can be spawned, so a
    /// parallel test's fork cannot make the first real run fail with ETXTBSY.
    fn script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\ncase \"$1\" in --probe) exit 0;; esac\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..200 {
            match std::process::Command::new(&path).arg("--probe").status() {
                Ok(_) => break,
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        path
    }
    fn check(executable: Option<std::path::PathBuf>, ms: u64) -> MameCheck {
        MameCheck {
            executable,
            timeout: Duration::from_millis(ms),
        }
    }
    fn run(f: &Fixture, c: &MameCheck) -> Outcome {
        let report = f.report();
        execute_checked(report.repair.as_ref().unwrap(), &options(f), c, &mut |_| {}).unwrap()
    }
    const GOOD_MAME: &str = "for a; do last=$a; done; echo \"romset $last is good\"";

    #[test]
    fn mame_pass_is_reported_as_rom_verification_only() {
        let f = fixture();
        let exe = script(f.dir.path(), "mame-good", GOOD_MAME);
        let out = run(&f, &check(Some(exe), 5000));
        assert_eq!(out.verifications.len(), 1);
        let v = &out.verifications[0].verification;
        assert_eq!(v.state, MameRepairVerificationState::VerifiedComplete);
        assert_eq!(v.destination_hashes_verified.len(), 1);
        assert!(
            out.notice.starts_with("Applied transaction"),
            "{}",
            out.notice
        );
        assert!(out.notice.contains("destination file hashes verified"));
        assert!(out.notice.contains("not that a game plays"));
        let text = describe(v);
        assert!(
            text.contains("does not show the game starts, plays"),
            "{text}"
        );
        assert_eq!(
            std::fs::read(f.request.root.join("g/main.bin")).unwrap(),
            b"hello world"
        );
    }

    #[test]
    fn mame_failure_is_not_shown_as_verified() {
        let f = fixture();
        let exe = script(f.dir.path(), "mame-bad", "echo 'romset g is bad'; exit 1");
        let out = run(&f, &check(Some(exe), 5000));
        let v = &out.verifications[0].verification;
        assert_eq!(v.state, MameRepairVerificationState::VerificationFailed);
        assert!(describe(v).contains("not shown to verify"));
        assert!(!out.notice.contains("1 of 1 set(s) verified complete"));
        // The file copy and its journal are unaffected by a failed check.
        assert!(f.request.root.join("g/main.bin").exists());
    }

    #[test]
    fn missing_or_unstartable_mame_is_unavailable_not_success() {
        let f = fixture();
        let out = run(&f, &check(None, 1000));
        let v = &out.verifications[0].verification;
        assert_eq!(
            v.state,
            MameRepairVerificationState::VerificationUnavailable
        );
        assert!(describe(v).contains("was not run"));
        assert!(out.notice.contains("0 of 1 set(s) verified complete"));
        let f = fixture();
        let out = run(&f, &check(Some("/nonexistent/mame".into()), 1000));
        let v = &out.verifications[0].verification;
        assert_ne!(v.state, MameRepairVerificationState::VerifiedComplete);
        assert_eq!(
            v.targeted_mame.as_ref().unwrap().state,
            MameTargetedVerificationState::Unavailable
        );
        assert!(describe(v).contains("could not be started"));
        assert!(out.notice.contains("0 of 1 set(s) verified complete"));
    }

    #[test]
    fn configured_mame_is_validated_before_it_runs() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture();
        let (exe, marker) = marker_script(&f);
        let link = f.dir.path().join("mame-link");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        let not_exec = f.dir.path().join("mame-plain");
        std::fs::write(&not_exec, "#!/bin/sh\ntouch marker\n").unwrap();
        std::fs::set_permissions(&not_exec, std::fs::Permissions::from_mode(0o644)).unwrap();
        let cases = [
            link,
            not_exec,
            f.dir.path().to_path_buf(),
            std::path::PathBuf::from("mame"),
        ];
        for bad in cases {
            let f = fixture();
            let out = run(&f, &check(Some(bad.clone()), 2000));
            let v = &out.verifications[0].verification;
            assert_eq!(
                v.state,
                MameRepairVerificationState::VerificationUnavailable,
                "{bad:?}"
            );
        }
        assert!(!marker.exists());
        assert_eq!(validated_executable(Some(&exe)), Some(exe));
    }

    #[test]
    fn mame_timeout_is_bounded_and_fails() {
        let f = fixture();
        let exe = script(f.dir.path(), "mame-slow", "exec sleep 30");
        let started = std::time::Instant::now();
        let out = run(&f, &check(Some(exe), 300));
        assert!(started.elapsed() < Duration::from_secs(10));
        let v = &out.verifications[0].verification;
        assert_eq!(v.state, MameRepairVerificationState::VerificationFailed);
        assert_eq!(
            v.targeted_mame.as_ref().unwrap().state,
            MameTargetedVerificationState::Timeout
        );
        assert!(describe(v).contains("stopped"));
    }

    fn applied(
        f: &Fixture,
    ) -> (
        std::sync::Arc<Reviewed>,
        MameInternalRepairApplyPlan,
        MameInternalRepairApplyResult,
    ) {
        let report = f.report();
        let reviewed = report.repair.clone().unwrap();
        let mut apply = reviewed.apply.clone();
        let result =
            apply_mame_internal_repair_plan(&mut apply, &options(f), &AtomicBool::new(false))
                .unwrap();
        (reviewed, apply, result)
    }
    fn marker_script(f: &Fixture) -> (std::path::PathBuf, std::path::PathBuf) {
        let marker = f.dir.path().join("mame-ran");
        let exe = script(
            f.dir.path(),
            "mame-marker",
            &format!("touch '{}'; {GOOD_MAME}", marker.display()),
        );
        (exe, marker)
    }

    #[test]
    fn stale_evidence_never_starts_mame() {
        let f = fixture();
        let (reviewed, apply, result) = applied(&f);
        let (exe, marker) = marker_script(&f);
        let basis = &reviewed.verification_basis[0];
        // Changed destination bytes.
        std::fs::write(
            f.request.root.join("g/main.bin"),
            b"someone else wrote this",
        )
        .unwrap();
        let c = check(Some(exe.clone()), 5000);
        let v = verify_set(
            basis,
            &apply,
            &result,
            (&reviewed.catalogue_digest, &reviewed.plan),
            &c,
        );
        assert_eq!(v.state, MameRepairVerificationState::StaleEvidence);
        // Changed catalogue.
        let v = verify_set(
            basis,
            &apply,
            &result,
            ("different-catalogue", &reviewed.plan),
            &c,
        );
        assert_eq!(v.state, MameRepairVerificationState::StaleEvidence);
        assert!(!marker.exists(), "MAME must not run on stale evidence");
        assert!(describe(&v).contains("evidence changed"));
    }

    #[test]
    fn rolled_back_or_partial_transaction_never_starts_mame() {
        let f = fixture();
        let (reviewed, apply, mut result) = applied(&f);
        let (exe, marker) = marker_script(&f);
        let c = check(Some(exe), 5000);
        for state in [
            TransactionState::RolledBack,
            TransactionState::Applying,
            TransactionState::ApplyFailed,
        ] {
            result.outcome.transaction.state = state;
            let v = verify_set(
                &reviewed.verification_basis[0],
                &apply,
                &result,
                (&reviewed.catalogue_digest, &reviewed.plan),
                &c,
            );
            assert_eq!(
                v.state,
                MameRepairVerificationState::StaleEvidence,
                "{state:?}"
            );
        }
        assert!(
            !marker.exists(),
            "MAME must not run unless the transaction is Applied"
        );
    }

    #[test]
    fn failed_apply_runs_no_mame_check() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture();
        let (exe, marker) = marker_script(&f);
        let report = f.report();
        let dir = f.request.root.join("g");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = execute_checked(
            report.repair.as_ref().unwrap(),
            &options(&f),
            &check(Some(exe), 5000),
            &mut |_| {},
        );
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        if let Ok(out) = result {
            assert!(out.verifications.is_empty(), "{}", out.notice);
        }
        assert!(!marker.exists());
    }

    #[test]
    fn basis_is_captured_at_planning_time() {
        let f = fixture();
        let report = f.report();
        let reviewed = report.repair.as_ref().unwrap();
        let basis = &reviewed.verification_basis;
        assert_eq!(basis.len(), 1);
        assert_eq!(basis[0].set, "g");
        assert_eq!(basis[0].identity.catalogue_sha256, report.catalogue.sha256);
        assert_eq!(basis[0].identity.evidence_generation, reviewed.digest);
        assert_eq!(basis[0].identity.arcade_root, reviewed.plan.collection_root);
        assert!(!basis[0].identity.dependency_topology_digest.is_empty());
    }
}
