use super::*;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
fn stx(path: &Path) {
    let mut b = vec![0u8; 80];
    b[..4].copy_from_slice(b"RSY\0");
    b[4..6].copy_from_slice(&3u16.to_le_bytes());
    b[10] = 1;
    b[16..20].copy_from_slice(&64u32.to_le_bytes());
    b[24..26].copy_from_slice(&9u16.to_le_bytes());
    b[28..30].copy_from_slice(&6250u16.to_le_bytes());
    fs::write(path, b).unwrap();
}
fn fixture() -> (tempfile::TempDir, Selection, HatariProfileDiscoveryRoots) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let disk = root.join("Orbit (Disk 1 of 2).stx");
    stx(&disk);
    let config = root.join("config/hatari/hatari.cfg");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        config.parent().unwrap().join("tos104.img"),
        b"synthetic unknown firmware",
    )
    .unwrap();
    fs::write(
        &config,
        b"[System]\nnModelType=0\n[ROM]\nszTosImageFileName=tos104.img\n",
    )
    .unwrap();
    let exe = root.join("hatari");
    fs::write(&exe, b"#!/bin/sh\nexit 99\n").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
    let roots = HatariProfileDiscoveryRoots {
        home: root.join("home"),
        xdg_config_home: root.join("config"),
        xdg_data_home: root.join("data"),
        explicit_config_roots: vec![],
        portable_config_roots: vec![],
        explicit_executables: vec![exe],
        known_version_outputs: Default::default(),
        appimage_directory: None,
        path_override: Some(vec![]),
    };
    let selection = Selection {
        path: disk,
        source_root: root.into(),
        platform: Some("AtariST".into()),
    };
    (temp, selection, roots)
}
#[test]
fn hatari_readiness_binds_media_profile_tos_and_preview_but_cannot_authorise_launch() {
    let (_temp, selection, roots) = fixture();
    let snapshot = gather(selection.clone(), roots);
    assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
    assert_eq!(snapshot.profiles.len(), 1);
    let p = &snapshot.profiles[0];
    assert_eq!(
        p.inspection.health.tos.health,
        patch_manager::HatariTosHealth::PresentUnverified,
        "tos104 filename is not trusted hash evidence"
    );
    assert!(p.inspection.selected_game.verified_title.is_none());
    let cmd = p.command.as_ref().unwrap();
    assert_eq!(cmd.arguments[0], "--configfile");
    assert_eq!(cmd.arguments[1], p.profile.config_path.as_os_str());
    assert_eq!(cmd.arguments[2], "--disk-a");
    assert_eq!(cmd.arguments[3], selection.path.as_os_str());
    assert!(
        snapshot
            .media
            .contains("membership, order and completeness are unverified")
    );
    let plan = snapshot.plan();
    assert_eq!(plan.summary.ready + plan.summary.ready_with_warnings, 0);
    assert!(plan.candidates.iter().all(|c| !c.blockers.is_empty()));
    let input = crate::launch_readiness_page::LaunchReadinessInput::AtariSt(snapshot);
    assert!(matches!(
        crate::launch_readiness_page::gamer_play_action(&input),
        crate::launch_readiness_page::GamerPlayAction::BlockedTyped(_)
    ));
}
#[test]
fn hatari_missing_firmware_and_malformed_or_unproven_media_remain_explicit() {
    let (temp, selection, roots) = fixture();
    fs::remove_file(temp.path().join("config/hatari/tos104.img")).unwrap();
    let snapshot = gather(selection.clone(), roots.clone());
    assert!(snapshot.profiles[0].command.is_none());
    assert!(
        snapshot.profiles[0]
            .refusal
            .as_ref()
            .unwrap()
            .contains("Tos")
            || snapshot.profiles[0]
                .refusal
                .as_ref()
                .unwrap()
                .contains("tos")
    );
    fs::write(&selection.path, b"garbage.stx").unwrap();
    assert!(
        gather(selection.clone(), roots.clone())
            .error
            .unwrap()
            .contains("did not validate")
    );
    let mut unsupported = selection.clone();
    unsupported.path = temp.path().join("disk.ipf");
    assert!(
        gather(unsupported, roots.clone())
            .error
            .unwrap()
            .contains("ST and STX only")
    );
    let mut conflicting = selection;
    conflicting.platform = Some("Amiga".into());
    assert!(
        gather(conflicting, roots)
            .error
            .unwrap()
            .contains("conflicts")
    );
}
#[test]
fn hatari_selection_change_discards_late_worker_result() {
    let (_temp, selection, roots) = fixture();
    let old = gather(selection.clone(), roots);
    let (tx, rx) = mpsc::channel();
    tx.send(old).unwrap();
    let other = Selection {
        path: selection.path.with_file_name("other.stx"),
        ..selection.clone()
    };
    let state = Arc::new(Mutex::new(Controller {
        selection: Some(selection.clone()),
        pending: Some((selection, rx)),
        ..Default::default()
    }));
    let value = Controller::snapshot(&state, other.clone());
    assert_eq!(value.selection.as_ref(), Some(&other));
    assert!(value.key.is_none());
}

#[test]
fn hatari_actual_blue_builder_and_gamer_summary_preserve_refusal() {
    let (_temp, selection, roots) = fixture();
    let snapshot = gather(selection.clone(), roots);
    let mut app = crate::tests::app_for_operation_tests();
    app.archive_context.focused = Some(selection.path.clone());
    app.selected_evidence_ui.selected_evidence =
        crate::selected_evidence_page::SelectedEvidenceState::Ready {
            generation: 0,
            hasheous: crate::selected_evidence_page::HasheousState::Idle,
            report: Box::new(
                crate::selected_evidence_page::gather_selected_evidence_fast(&selection.path, None)
                    .unwrap(),
            ),
        };
    app.emulator_readiness.hatari = Arc::new(Mutex::new(Controller {
        selection: Some(Selection {
            platform: None,
            ..selection.clone()
        }),
        current: snapshot,
        checked: Some(Instant::now()),
        ..Default::default()
    }));
    let input = app.build_launch_readiness_input(None);
    assert!(matches!(
        &input,
        crate::launch_readiness_page::LaunchReadinessInput::AtariSt(_)
    ));
    let summary = crate::gui_v2::launch_readiness_summary::project(
        &input,
        crate::gui_v2::launch_readiness_summary::ReadinessFreshness::Current,
    );
    assert!(summary.primary_action.is_none());
    assert!(summary.explanation.contains("not authorised"));
    assert!(
        summary
            .details
            .iter()
            .any(|d| d.contains("PresentUnverified"))
    );
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            if let crate::launch_readiness_page::LaunchReadinessInput::AtariSt(snapshot) = &input {
                snapshot.show(ui);
            }
        });
    });
}

#[test]
fn hatari_blue_builder_preserves_conflicting_and_other_platform_identity() {
    use archivefs_core::game_identity::*;
    let (_temp, selection, _roots) = fixture();
    let mut app = crate::tests::app_for_operation_tests();
    app.archive_context.focused = Some(selection.path.clone());
    for serials in [vec!["SLUS-98765"], vec!["SLUS-98765", "SLUS-12345"]] {
        let mut report =
            crate::selected_evidence_page::gather_selected_evidence_fast(&selection.path, None)
                .unwrap();
        report.game_identity_report.platform = IdentityPlatform::PlayStation2;
        report.game_identity_report.evidence = serials
            .iter()
            .map(|serial| IdentityEvidence {
                kind: IdentityKind::Ps2Serial,
                status: IdentityStatus::Verified,
                value: Some((*serial).into()),
                confidence: IdentityConfidence::ExactBytes,
                provenance: IdentityProvenance {
                    archive_path: selection.path.clone(),
                    member_path: None,
                    member_index: None,
                    method: "synthetic conflicting/stronger fact".into(),
                },
                diagnostic: "Guard fixture; no real title verification performed".into(),
            })
            .collect();
        app.selected_evidence_ui.selected_evidence =
            crate::selected_evidence_page::SelectedEvidenceState::Ready {
                generation: 0,
                report: Box::new(report),
                hasheous: crate::selected_evidence_page::HasheousState::Idle,
            };
        let input = app.build_launch_readiness_input(None);
        assert!(!matches!(
            &input,
            crate::launch_readiness_page::LaunchReadinessInput::AtariSt(_)
        ));
        if serials.len() > 1 {
            assert!(matches!(
                input,
                crate::launch_readiness_page::LaunchReadinessInput::IdentityConflicting
            ));
        }
    }
}
#[test]
fn hatari_controller_report_shows_joystick_modes_without_button_claims() {
    use crate::launch_readiness_page::controller_readiness::{ControllerState, hatari_report};
    let (temp, selection, roots) = fixture();
    let config = temp.path().join("config/hatari/hatari.cfg");
    let base = "[System]\nnModelType=0\n[ROM]\nszTosImageFileName=tos104.img\n";
    let report_for = |extra: &str| {
        let contents = format!("{base}{extra}");
        fs::write(&config, &contents).unwrap();
        let snapshot = gather(selection.clone(), roots.clone());
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            contents,
            "settings must not be written"
        );
        hatari_report(&snapshot.profiles[0].inspection.config)
    };

    let active = report_for("[Joystick0]\nnJoystickMode=0\n[Joystick1]\nnJoystickMode=1\n");
    assert_eq!(active.state, ControllerState::SettingsFound);
    assert!(
        active
            .detail
            .contains("Joystick 0: disabled · Joystick 1: real joystick"),
        "{}",
        active.detail
    );
    assert!(active.detail.contains("button assignments are not"));

    // Hatari's own default: every port disabled.
    let disabled = report_for("[Joystick0]\nnJoystickMode = 0\n[Joystick1]\nnJoystickMode = 0\n");
    assert_eq!(disabled.state, ControllerState::NoActiveSettings);

    assert_eq!(report_for("").state, ControllerState::Unknown);
}
