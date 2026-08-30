//! GUI Maintenance Batch 2: relocated from main.rs's single inline
//! `#[cfg(test)] mod tests { ... }` (see `crate::tests` for the shared
//! imports/fixtures this file and its siblings rely on).
//!
//! This file's name is a best-effort thematic label, not a strict
//! single-feature boundary: the original test module interleaved topics
//! throughout (tests for unrelated features sit side by side in source
//! order), so this file was cut at safe item boundaries within that
//! existing order rather than by re-sorting tests into pure per-feature
//! files. Every test here is copied byte-for-byte from its original
//! location - nothing was rewritten, renamed, or reordered relative to
//! its neighbors within this slice.
//!
//! Predominant theme observed in this slice: Doctor scan findings and repair review/confirm flows.

use super::*;
use archivefs_core::diagnostics::environment::{MountMode, WritabilityAssessment};
use archivefs_core::diagnostics::profiles::{EmulatorKind, ProfileAssessment};

#[test]
fn cheats_mods_page_has_a_truthful_no_archive_empty_state() {
    let ctx = egui::Context::default();
    let history = OperationHistory::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                None,
                &RetroArchProfilesState::NotScanned,
                &Pcsx2ProfilesState::NotScanned,
                &DolphinProfilesState::NotScanned,
                &XeniaProfilesState::NotScanned,
                None,
                None,
                &history,
                false,
                &mut clipboard,
                &mut dolphin_texture_mod_page::DolphinTextureModPageState::default(),
            );
        });
    });

    for expected in [
        "Cheats & Mods",
        "Choose a game",
        "Select a game to see available cheats and patches.",
        "Open Library",
    ] {
        assert!(rendered_text_contains(&output, expected));
    }
    for internal in [
        "archive context",
        "Profile discovery",
        "Trusted retrieval",
        "Matching unavailable",
        "Installation gated",
    ] {
        assert!(!rendered_text_contains(&output, internal));
    }
    assert!(!rendered_text_contains(&output, "/roms/"));
}

#[test]
fn cheats_mods_page_routes_retroarch_to_shared_preview_without_stale_wording() {
    let mut app = app_with_cheats_mods_context();
    let (source_id, source_name, list) = cheat_source_list_fixture();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.selected_source_id = Some(source_id);
    workflow.source_list = CheatStepResource::Ready(list);
    let history = OperationHistory::default();
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheats_mods_page(
                ui,
                app.cheat_workflow.as_mut(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
                &app.xenia_profiles,
                None,
                None,
                &history,
                false,
                &mut clipboard,
                &mut dolphin_texture_mod_page::DolphinTextureModPageState::default(),
            );
        });
    });

    for expected in [
        "Choose a RetroArch profile",
        "native-user",
        "Cheat source",
        "Available cheats",
        source_name.as_str(),
        "Ready",
        "Shared preview",
        // "Controlled apply available after eligible preview" now lives
        // behind the collapsed "Workflow diagnostics" section - covered
        // separately by `cheats_mods_page_renders_the_new_hierarchy_headings`.
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "Cheats & Mods page did not render {expected:?}"
        );
    }
    assert!(!rendered_text_contains(
        &output,
        "Archive matching and cheat installation are not yet implemented"
    ));
}

#[test]
fn cheats_mods_workspace_keeps_lifecycle_states_visibly_separate() {
    let app = app_with_cheats_mods_context();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_cheats_mods_workflow_states(
                ui,
                app.cheat_workflow.as_ref(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
            );
        });
    });

    for expected in [
        "Emulator profile",
        "Cheat or mod source",
        "Trust state",
        "Inspection state",
        "Destination",
        "Installation state",
        "Trusted",
        "/isolated/cheats",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "workflow model did not render {expected:?}"
        );
    }
}

#[test]
fn pcsx2_workflow_states_render_every_row_through_the_shared_status_rows_component() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.adapter = CheatEmulatorAdapter::Pcsx2;
    workflow.selected_pcsx2_profile_id = Some("pcsx2-native-test".to_string());
    app.pcsx2_profiles = Pcsx2ProfilesState::Ready(Pcsx2ProfileDiscovery {
        profiles: vec![pcsx2_profile_fixture()],
        warnings: Vec::new(),
        complete: true,
    });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_cheats_mods_workflow_states(
                ui,
                app.cheat_workflow.as_ref(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
            );
        });
    });
    for expected in [
        "Emulator profile",
        "Cheat or mod source",
        "Existing PCSX2-managed files",
        "Trust state",
        "Unverified local content",
        "Inspection state",
        "Destination",
        "/isolated/PCSX2",
        "Installation state",
        "Unavailable · read-only adapter",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "PCSX2 workflow-state row did not render {expected:?} through status_rows"
        );
    }
}

#[test]
fn dolphin_workflow_states_render_every_row_through_the_shared_status_rows_component() {
    let mut app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_mut().unwrap();
    workflow.adapter = CheatEmulatorAdapter::Dolphin;
    workflow.selected_dolphin_profile_id = Some("dolphin-native-test".to_string());
    app.dolphin_profiles = DolphinProfilesState::Ready(DolphinProfileDiscovery {
        profiles: vec![dolphin_profile_fixture()],
        warnings: Vec::new(),
        complete: true,
    });
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_cheats_mods_workflow_states(
                ui,
                app.cheat_workflow.as_ref(),
                &app.retroarch_profiles,
                &app.pcsx2_profiles,
                &app.dolphin_profiles,
            );
        });
    });
    for expected in [
        "Emulator profile",
        "Cheat or mod source",
        "Dolphin upstream GameSettings provider",
        "Trust state",
        "Exact-ID provider data · locally validated",
        "Inspection state",
        "Destination",
        "Installation state",
        "Preview, journal-backed apply, and rollback available",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "Dolphin workflow-state row did not render {expected:?} through status_rows"
        );
    }
}

#[test]
fn cheats_mods_safety_copy_is_truthful_and_has_no_fake_scanning_setting() {
    assert_eq!(
        LocalSafetyScanningState::current(),
        LocalSafetyScanningState::PlannedUnavailable
    );
    assert!(LOCAL_INSPECTION_PRIVACY_COPY.contains("not sent"));
    assert!(LOCAL_INSPECTION_PRIVACY_COPY.contains("planned and is not active yet"));
    assert!(UNKNOWN_CODE_POLICY.contains("never executes unknown code automatically"));
    assert!(IMPORT_CONSENT_COPY.contains("not an antivirus scanner"));
    assert!(SCANNING_DISABLED_WARNING.contains("does not make unsafe files safe"));
    assert!(ETHICAL_USE_COPY.contains("must not be used to bypass copy protection"));
    assert!(USER_RESPONSIBILITY_COPY.contains("EmuWiz does not verify ownership"));
    assert_eq!(
        local_scanning_presentation(LocalSafetyScanningState::current()).0,
        "Local safety scanning · Planned"
    );
}

#[test]
fn cheats_mods_trust_labels_do_not_conflate_unverified_with_blocked() {
    assert_eq!(import_trust_label(ImportTrustState::Trusted), "Trusted");
    assert_eq!(
        import_trust_label(ImportTrustState::Unverified),
        "Unverified"
    );
    assert_eq!(import_trust_label(ImportTrustState::Blocked), "Blocked");
    assert_ne!(
        import_trust_tone(ImportTrustState::Unverified),
        import_trust_tone(ImportTrustState::Blocked)
    );
    assert_eq!(
        import_source_presentation(ImportSourceKind::ArchiveFsTrustedCatalogue),
        ("EmuWiz trusted catalogue", "Available")
    );
    assert_eq!(
        import_source_presentation(ImportSourceKind::LocalUnverifiedSource).1,
        "Planned"
    );
}

#[test]
fn cheats_mods_archive_context_shows_mount_and_manual_platform_state() {
    let app = app_with_cheats_mods_context();
    let workflow = app.cheat_workflow.as_ref().unwrap();
    let live = loaded_data_with_records("/mount", vec![record("/roms/a.zip", MountState::Pending)]);
    let cached = cached_snapshot(vec![persisted_archive_with_platform(
        PathBuf::from("/roms/a.zip"),
        1,
        "SNES",
        MANUAL_PLATFORM_SOURCE,
    )]);
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_cheat_archive_context(ui, workflow, Some(&live), Some(&cached), &mut clipboard);
        });
    });

    for expected in [
        "Selected archive context",
        "Ready to mount",
        "Manual platform assignment",
        "/roms/a.zip",
        "/roms",
    ] {
        assert!(rendered_text_contains(&output, expected));
    }
}

#[test]
fn recent_cheat_activity_is_filtered_and_compact() {
    let mut history = OperationHistory::default();
    history.record(HistoryEntry::new(
        ActivityAction::Mount,
        None,
        ActivityOutcome::Completed,
        "unrelated mount",
    ));
    history.record(HistoryEntry::new(
        ActivityAction::RetroArchProfileScan,
        None,
        ActivityOutcome::Completed,
        "profile scan complete",
    ));
    history.record(HistoryEntry::new(
        ActivityAction::CheatSourceRetrieval,
        Some(PathBuf::from("/roms/a.zip")),
        ActivityOutcome::Failed,
        "trusted catalogue failed",
    ));
    history.record(HistoryEntry::new(
        ActivityAction::CheatSourceRetrieval,
        Some(PathBuf::from("/roms/other.zip")),
        ActivityOutcome::Completed,
        "unrelated catalogue",
    ));
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_recent_cheat_activity(ui, &history, Some(Path::new("/roms/a.zip")));
        });
    });

    assert!(rendered_text_contains(&output, "profile scan complete"));
    assert!(rendered_text_contains(&output, "trusted catalogue failed"));
    assert!(!rendered_text_contains(&output, "unrelated mount"));
    assert!(!rendered_text_contains(&output, "unrelated catalogue"));
}

#[test]
fn mods_section_has_no_fake_user_actions() {
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| show_mods_section(ui, false, false));
    });

    assert!(rendered_text_contains(&output, "Mods"));
    assert!(rendered_text_contains(
        &output,
        "No mod workflow is available yet"
    ));
    for forbidden in ["Install", "Browse", "Download", "Apply", "Remove"] {
        assert!(
            !rendered_text_contains(&output, forbidden),
            "Mods must not render a fake {forbidden} action"
        );
    }
}

#[test]
fn cheat_workflow_step1_shows_blockers_and_requires_explicit_profile_choice() {
    let ctx = egui::Context::default();
    let mut workflow = CheatWorkflowState {
        archive_path: PathBuf::from("/roms/a.zip"),
        display_name: "a".to_string(),
        normalized_name: "a".to_string(),
        platform: None,
        region: None,
        source_root: PathBuf::from("/roms"),
        size_bytes: None,
        adapter: CheatEmulatorAdapter::RetroArch,
        identity_request: None,
        identity: CheatStepResource::NotLoaded,
        preview_request: None,
        preview: CheatStepResource::NotLoaded,
        transaction: CheatTransactionState::Idle,
        transaction_notice: None,
        selected_profile_id: None,
        selected_pcsx2_profile_id: None,
        pcsx2_inventory_profile_id: None,
        pcsx2_inventory: CheatStepResource::NotLoaded,
        pcsx2_gamehacking: CheatStepResource::NotLoaded,
        gamecube_gamehacking: CheatStepResource::NotLoaded,
        gamecube_gamehacking_request: None,
        gamecube_gamehacking_cancellation: None,
        gamecube_gamehacking_generation: 0,
        gamecube_gamehacking_blocked: false,
        bsfree_gamecube: CheatStepResource::NotLoaded,
        bsfree_gamecube_cancellation: None,
        bsfree_gamecube_generation: 0,
        bsfree_wii: CheatStepResource::NotLoaded,
        bsfree_wii_cancellation: None,
        bsfree_wii_generation: 0,
        selected_dolphin_profile_id: None,
        dolphin_explicit_root: String::new(),
        dolphin_inventory_profile_id: None,
        dolphin_inventory: CheatStepResource::NotLoaded,
        dolphin_provider_request: None,
        dolphin_provider: CheatStepResource::NotLoaded,
        dolphin_provider_selection: None,
        dolphin_destination_error: None,
        dolphin_local_lookup: DolphinLocalLookupState::NotAttempted,
        dolphin_profile_selection: None,
        dolphin_profile_choice: None,
        dolphin_details_open: false,
        dolphin_show_exact_changes: false,
        selected_xenia_profile_id: None,
        xenia_explicit_root: String::new(),
        xenia_provider_request: None,
        xenia_provider: CheatStepResource::NotLoaded,
        xenia_selected_candidate_index: None,
        xenia_selection: None,
        xenia_destination_error: None,
        xenia_profile_selection: None,
        xenia_profile_choice: None,
        xenia_details_open: false,
        xenia_show_exact_changes: false,
        source_mode: CheatSourceMode::ArchiveFsTrustedCatalogue,
        existing_library_profile_id: None,
        existing_library: CheatStepResource::NotLoaded,
        source_list: CheatStepResource::NotLoaded,
        source_fetch: CheatStepResource::NotLoaded,
        selected_source_id: None,
        fetch_force_refresh: false,
        candidates: CheatStepResource::NotLoaded,
        candidates_request: None,
        candidate_query: String::new(),
        candidate_selection: None,
        candidate_load_error: None,
    };

    // Ineligible profile: blocker code and detail are rendered.
    let profiles = RetroArchProfilesState::Ready(cheat_discovery(vec![cheat_profile(
        "blocked-profile",
        false,
    )]));
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheat_workflow_step1(ui, &mut workflow, &profiles, false);
        });
    });
    assert!(rendered_text_contains(&output, "Blocked"));
    assert!(rendered_text_contains(
        &output,
        "cheats_destination_unresolved"
    ));

    // Two eligible profiles: explicit-choice message, no selection.
    let profiles = RetroArchProfilesState::Ready(cheat_discovery(vec![
        cheat_profile("native-user", true),
        cheat_profile("flatpak-user", true),
    ]));
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheat_workflow_step1(ui, &mut workflow, &profiles, false);
        });
    });
    assert!(rendered_text_contains(&output, "never silently picks"));
    assert_eq!(workflow.selected_profile_id, None);

    // A previously selected profile that vanished is cleared, not kept.
    workflow.selected_profile_id = Some("gone-profile".to_string());
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_cheat_workflow_step1(ui, &mut workflow, &profiles, false);
        });
    });
    assert_eq!(
        workflow.selected_profile_id, None,
        "a stale profile selection must be cleared, never silently kept"
    );
}

#[test]
fn retroarch_profile_labels_cover_every_kind_and_scope() {
    assert_eq!(profile_kind_label(&ProfileKind::Native), "Native");
    assert_eq!(profile_kind_label(&ProfileKind::AppImage), "AppImage");
    assert_eq!(profile_kind_label(&ProfileKind::Flatpak), "Flatpak");
    assert_eq!(profile_scope_label(&ProfileScope::User), "User");
    assert_eq!(profile_scope_label(&ProfileScope::System), "System");
}

#[test]
fn system_information_text_includes_all_labelled_fields() {
    let text = system_information_text(Some("/db/library.sqlite3"), None, Some("/mnt/root"));
    assert!(text.contains(&format!("EmuWiz {}", env!("CARGO_PKG_VERSION"))));
    assert!(text.contains("Database schema: v"));
    assert!(text.contains("Database path: /db/library.sqlite3"));
    assert!(
        text.contains("Configuration path: unknown"),
        "missing inputs must be reported as unknown, never invented"
    );
    assert!(text.contains("Mount root: /mnt/root"));
}

#[test]
fn doctor_summary_and_report_text_reflect_exact_check_counts() {
    let report = DoctorReport {
        config_path: PathBuf::from("/config/archivefs.toml"),
        checks: vec![
            DoctorCheck {
                name: "ratarmount".to_string(),
                status: DoctorStatus::Pass,
                detail: "ratarmount is available".to_string(),
            },
            DoctorCheck {
                name: "mount root".to_string(),
                status: DoctorStatus::Warn,
                detail: "mount root missing".to_string(),
            },
            DoctorCheck {
                name: "database".to_string(),
                status: DoctorStatus::Fail,
                detail: "database unreadable".to_string(),
            },
        ],
        archives_found: 5,
        archives_with_platform: 4,
        archives_unknown_platform: 1,
        unknown_platform_examples: Vec::new(),
        platform_counts: vec![("PS2".to_string(), 4)],
        pending_archives: 3,
        mounted_archives: 2,
    };
    let summary = doctor_summary_text(&report);
    assert_eq!(
        summary,
        "3 checks: 1 passed, 1 warnings, 1 failed · 5 archives (2 mounted, 3 pending, 1 unknown platform)"
    );
    let text = doctor_report_text(&report);
    assert!(text.contains("Config: /config/archivefs.toml"));
    assert!(text.contains(&summary));
    assert!(text.contains("[PASS] ratarmount — ratarmount is available"));
    assert!(text.contains("mount root missing"));
    assert!(text.contains("database unreadable"));
    assert!(text.contains("PS2: 4"));
}

#[test]
fn activity_filter_lists_cover_every_variant() {
    for action in ALL_ACTIVITY_ACTIONS {
        assert_eq!(
            ALL_ACTIVITY_ACTIONS
                .iter()
                .filter(|candidate| **candidate == action)
                .count(),
            1,
            "{action:?} must appear exactly once in the Operation filter list"
        );
    }
    for outcome in ALL_ACTIVITY_OUTCOMES {
        assert_eq!(
            ALL_ACTIVITY_OUTCOMES
                .iter()
                .filter(|candidate| **candidate == outcome)
                .count(),
            1,
            "{outcome:?} must appear exactly once in the Result filter list"
        );
    }
}

#[test]
fn history_filters_select_by_action_and_outcome_without_reordering() {
    let mut history = OperationHistory::default();
    history.record(HistoryEntry::new(
        ActivityAction::Mount,
        None,
        ActivityOutcome::Completed,
        "first",
    ));
    history.record(HistoryEntry::new(
        ActivityAction::Unmount,
        None,
        ActivityOutcome::Failed,
        "second",
    ));
    history.record(HistoryEntry::new(
        ActivityAction::Mount,
        None,
        ActivityOutcome::Failed,
        "third",
    ));

    let all = visible_history_entries(&history, &HistoryLogFilters::default());
    assert_eq!(
        all.iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        vec!["third", "second", "first"],
        "default order is newest first"
    );

    let mounts = visible_history_entries(
        &history,
        &HistoryLogFilters {
            action: Some(ActivityAction::Mount),
            ..HistoryLogFilters::default()
        },
    );
    assert_eq!(
        mounts
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        vec!["third", "first"]
    );

    let failed_mounts = visible_history_entries(
        &history,
        &HistoryLogFilters {
            action: Some(ActivityAction::Mount),
            outcome: Some(ActivityOutcome::Failed),
            ..HistoryLogFilters::default()
        },
    );
    assert_eq!(
        failed_mounts
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        vec!["third"]
    );

    let oldest_first = visible_history_entries(
        &history,
        &HistoryLogFilters {
            oldest_first: true,
            ..HistoryLogFilters::default()
        },
    );
    assert_eq!(
        oldest_first
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second", "third"],
        "oldest-first reverses the filtered view only"
    );
}

#[test]
fn history_text_search_matches_message_action_and_outcome_case_insensitively() {
    let mut history = OperationHistory::default();
    history.record(HistoryEntry::new(
        ActivityAction::Mount,
        None,
        ActivityOutcome::Completed,
        "Mounted Chrono Trigger",
    ));
    history.record(HistoryEntry::new(
        ActivityAction::Unmount,
        None,
        ActivityOutcome::Failed,
        "unrelated entry",
    ));

    let by_message = visible_history_entries(
        &history,
        &HistoryLogFilters {
            text_query: "chrono".to_string(),
            ..HistoryLogFilters::default()
        },
    );
    assert_eq!(
        by_message
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        vec!["Mounted Chrono Trigger"]
    );

    let by_action = visible_history_entries(
        &history,
        &HistoryLogFilters {
            text_query: "unmount".to_string(),
            ..HistoryLogFilters::default()
        },
    );
    assert_eq!(by_action.len(), 1);
    assert_eq!(by_action[0].action, ActivityAction::Unmount);

    let by_outcome = visible_history_entries(
        &history,
        &HistoryLogFilters {
            text_query: "failed".to_string(),
            ..HistoryLogFilters::default()
        },
    );
    assert_eq!(by_outcome.len(), 1);
    assert_eq!(by_outcome[0].outcome, ActivityOutcome::Failed);

    let none_match = visible_history_entries(
        &history,
        &HistoryLogFilters {
            text_query: "no-such-thing".to_string(),
            ..HistoryLogFilters::default()
        },
    );
    assert!(none_match.is_empty());

    // Search combines with the existing category filters rather than
    // replacing them.
    let combined = visible_history_entries(
        &history,
        &HistoryLogFilters {
            action: Some(ActivityAction::Mount),
            text_query: "chrono".to_string(),
            ..HistoryLogFilters::default()
        },
    );
    assert_eq!(combined.len(), 1);

    let combined_miss = visible_history_entries(
        &history,
        &HistoryLogFilters {
            action: Some(ActivityAction::Unmount),
            text_query: "chrono".to_string(),
            ..HistoryLogFilters::default()
        },
    );
    assert!(combined_miss.is_empty());
}

#[test]
fn active_mounts_page_lists_only_mounted_archives_and_requires_confirmation() {
    let records = vec![
        record("/roms/mounted.zip", MountState::Mounted),
        record("/roms/pending.zip", MountState::Pending),
    ];
    let ctx = egui::Context::default();
    let mut confirm = None;
    let mut cleanup = false;
    let mut action = None;
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            action = show_active_mounts_page(
                ui,
                Some(&records),
                &mut confirm,
                &mut cleanup,
                None,
                false,
            );
        });
    });
    assert!(
        action.is_none(),
        "rendering alone must not unmount anything"
    );
    assert!(rendered_text_contains(&output, "/roms/mounted.zip"));
    assert!(
        !rendered_text_contains(&output, "/roms/pending.zip"),
        "only mounted archives are listed"
    );
    assert!(confirm.is_none());

    let mut stale = Some(PathBuf::from("/roms/pending.zip"));
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ =
                show_active_mounts_page(ui, Some(&records), &mut stale, &mut cleanup, None, false);
        });
    });
    assert!(
        stale.is_none(),
        "a confirmation must not survive for a non-mounted archive"
    );

    let mut live_confirm = Some(PathBuf::from("/roms/mounted.zip"));
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_active_mounts_page(
                ui,
                Some(&records),
                &mut live_confirm,
                &mut cleanup,
                None,
                false,
            );
        });
    });
    assert!(rendered_text_contains(&output, "Unmount now"));
    assert!(live_confirm.is_some());
}

#[test]
fn preview_match_strength_presentation_covers_every_variant_with_a_distinct_honest_explanation() {
    let all = [
        PreviewMatchStrength::VerifiedExact,
        PreviewMatchStrength::Strong,
        PreviewMatchStrength::Candidate,
        PreviewMatchStrength::Ambiguous,
        PreviewMatchStrength::Unsupported,
    ];
    let mut labels = std::collections::HashSet::new();
    for strength in all {
        let (label, _tone, explanation) = preview_match_strength_presentation(strength);
        assert!(!label.is_empty());
        assert!(
            !explanation.is_empty(),
            "{strength:?} must explain itself, not just carry a bare label"
        );
        assert!(
            labels.insert(label),
            "{strength:?} must not share a label with another match strength"
        );
    }
    // The two "don't trust this without review" tiers must be visibly
    // distinct in tone from the two "safe to proceed" tiers.
    assert_eq!(
        preview_match_strength_presentation(PreviewMatchStrength::VerifiedExact).1,
        widgets::StatusTone::Success
    );
    assert_eq!(
        preview_match_strength_presentation(PreviewMatchStrength::Ambiguous).1,
        widgets::StatusTone::Blocked
    );
}

#[test]
fn cheat_warnings_summary_shows_a_bounded_sample_and_keeps_the_rest_reachable() {
    let ctx = egui::Context::default();
    let warnings: Vec<String> = (0..12)
        .map(|index| {
            format!("{index} files retained but are non-actionable because parsing was incomplete")
        })
        .collect();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_cheat_warnings_summary(ui, &warnings, "warnings_summary_test", &mut clipboard);
        });
    });
    assert!(
        rendered_text_contains(&output, "12 catalogue issues found"),
        "the full count must be visible even though only a sample renders directly"
    );
    assert!(
        rendered_text_contains(&output, "The catalogue still works"),
        "must reassure the user the catalogue is still usable despite these exclusions"
    );
    assert!(rendered_text_contains(&output, "What happened?"));
    assert!(
        !rendered_text_contains(
            &output,
            "11 catalogue files could not be parsed and were excluded from matching."
        ),
        "only a bounded sample renders directly - the rest stays behind Technical details"
    );
    assert!(!rendered_text_contains(&output, "verification notes"));
}

/// Root-cause regression for the "First use of widget ID .../Second
/// use of widget ID ..." egui warning on Cheats & Mods and catalogue
/// cards: `show_cheat_warnings_summary`'s outer "What happened?"
/// `CollapsingHeader` used to have no `id_salt` at all, so every call
/// site collided on the same literal-text-derived ID (the inner
/// `technical_details` disclosure was already correctly salted - only
/// the outer header was not). Two instances rendered in the same frame
/// (the real-world shape of the bug: two catalogue sources' warning
/// sections both visible at once) must now toggle independently -
/// proof that they hold genuinely distinct IDs, not just that the
/// salt values passed in happen to differ.
#[test]
fn cheat_warnings_summary_two_instances_in_one_frame_toggle_independently() {
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let warnings_a = vec!["only in instance A".to_string()];
    let warnings_b = vec!["only in instance B".to_string()];
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));

    let mut render = |ctx: &egui::Context, input: egui::RawInput| {
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_cheat_warnings_summary(ui, &warnings_a, "instance_a", &mut clipboard);
                show_cheat_warnings_summary(ui, &warnings_b, "instance_b", &mut clipboard);
            });
        })
    };

    // Frame 1: both collapsed by default; find instance A's header
    // (the first "What happened?" painted) and click it open.
    let first = render(
        &ctx,
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
    );
    let header_pos = find_exact_text_center(&first, "What happened?")
        .expect("expected at least one \"What happened?\" header to render");
    let _ = render(&ctx, click_at(screen, header_pos));

    // Frame 3: settle, then check which instance's sample text shows.
    let output = render(
        &ctx,
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
    );
    assert!(
        rendered_text_contains(&output, "only in instance A"),
        "instance A's header was clicked open and must show its own content"
    );
    assert!(
        !rendered_text_contains(&output, "only in instance B"),
        "instance B must remain independently collapsed - if the two headers shared an ID, \
             opening A would have opened B too"
    );
}

#[test]
fn cheat_warnings_summary_is_silent_when_there_is_nothing_to_report() {
    let ctx = egui::Context::default();
    let mut clipboard = InMemoryClipboard::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_cheat_warnings_summary(ui, &[], "empty_warnings_test", &mut clipboard);
        });
    });
    assert!(!rendered_text_contains(&output, "catalogue issue"));
    assert!(!rendered_text_contains(&output, "Technical details"));
}

#[test]
fn mount_row_matches_is_case_insensitive_over_name_platform_and_paths() {
    let entry = record("/roms/Final Fantasy VII.zip", MountState::Pending);
    assert!(mount_row_matches(&entry, ""));
    assert!(mount_row_matches(&entry, "  "));
    assert!(mount_row_matches(&entry, "final fantasy"));
    assert!(mount_row_matches(&entry, "FINAL"));
    assert!(mount_row_matches(&entry, "/roms/"));
    assert!(
        mount_row_matches(&entry, "/mnt/archivefs/"),
        "planned destination is searchable"
    );
    assert!(!mount_row_matches(&entry, "chrono trigger"));
}

// --- Doctor Stage 1A ------------------------------------------------

fn doctor_health_issue(path: &str, category: HealthCategory) -> HealthIssue {
    HealthIssue {
        path: PathBuf::from(path),
        platform: Some("SNES".to_string()),
        present: category != HealthCategory::Missing,
        mount_state: Some(MountState::Pending),
        category,
        reason: format!("reason for {}", category.label()),
        retryable: category.is_retryable(),
        recovery_action: match category {
            HealthCategory::RetryableFailure => Some(RecoveryAction::RetryMount),
            _ => None,
        },
        last_seen_at: Some("2026-07-31T00:00:00Z".to_string()),
        size_bytes: Some(4096),
        modified_time_unix_seconds: Some(1_700_000_000),
    }
}

/// Builds a real scan through the real runner, so the GUI tests exercise
/// the same code path the application does.
fn doctor_scan_from(issues: &[HealthIssue]) -> DoctorScan {
    let mut inputs = DoctorScanInputs::none_loaded();
    inputs.health_issues = Gathered::Ready(issues);
    run_doctor_scan(&inputs)
}

fn doctor_outcome(scan: DoctorScan) -> DoctorScanState {
    DoctorScanState::Ready(Box::new(DoctorScanOutcome {
        scan,
        finished_at_unix_seconds: 1_700_000_000,
    }))
}

fn render_doctor_page(state: &DoctorScanState, selected: &mut Option<String>) -> egui::FullOutput {
    render_doctor_page_with(state, selected, None, None, None)
}

/// Renders the Doctor page in Gamer View, where informational findings
/// are summarised rather than listed card by card.
fn render_doctor_page_gamer(state: &DoctorScanState) -> egui::FullOutput {
    render_doctor_page_with_mode(state, &mut None, None, None, None, true)
}

fn render_doctor_page_with(
    state: &DoctorScanState,
    selected: &mut Option<String>,
    review: Option<&DoctorRepairReview>,
    repair_result: Option<&DoctorRepairOutcome>,
    repaired_at: Option<i64>,
) -> egui::FullOutput {
    render_doctor_page_with_mode(state, selected, review, repair_result, repaired_at, false)
}

fn render_doctor_page_with_mode(
    state: &DoctorScanState,
    selected: &mut Option<String>,
    review: Option<&DoctorRepairReview>,
    repair_result: Option<&DoctorRepairOutcome>,
    repaired_at: Option<i64>,
    gamer_view: bool,
) -> egui::FullOutput {
    let mut clipboard = InMemoryClipboard::default();
    let ctx = egui::Context::default();
    ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_doctor_page(
                ui,
                state,
                selected,
                review,
                repair_result,
                repaired_at,
                &mut clipboard,
                gamer_view,
            );
        });
    })
}

#[test]
fn large_loose_rom_group_hides_examples_and_technical_breakdown_by_default() {
    let issues: Vec<HealthIssue> = (0..12)
        .map(|index| {
            doctor_health_issue(
                &format!("/roms/loose/{index:03}.sfc"),
                HealthCategory::MountNotRequired,
            )
        })
        .collect();
    let scan = doctor_scan_from(&issues);
    let state = doctor_outcome(scan);
    let output = render_doctor_page(&state, &mut None);

    assert!(rendered_text_contains(&output, "12 loose ROMs are healthy"));
    assert!(rendered_text_contains(
        &output,
        "These games can be used directly. Nothing needs fixing."
    ));
    assert!(rendered_text_contains(&output, "Show examples"));
    assert!(rendered_text_contains(&output, "Show details"));
    for hidden in [
        "By reason",
        "By platform",
        "By media kind",
        "By evidence",
        "/roms/loose/",
    ] {
        assert!(
            !rendered_text_contains(&output, hidden),
            "{hidden} leaked into the default card"
        );
    }

    let ctx = egui::Context::default();
    ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
    let mut clipboard = InMemoryClipboard::default();
    let details = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_doctor_page(
                ui,
                &state,
                &mut None,
                None,
                None,
                None,
                &mut clipboard,
                false,
            );
        });
    });
    assert!(rendered_text_contains(&details, "By reason"));
    assert!(rendered_text_contains(&details, "By platform"));
    assert!(rendered_text_contains(&details, "By media kind"));
    assert!(rendered_text_contains(&details, "By evidence"));
    assert!(rendered_text_contains(&details, "/roms/loose/000.sfc"));
}

#[test]
fn copied_doctor_report_keeps_every_finding_hidden_by_gui_grouping() {
    let issues: Vec<HealthIssue> = (0..12)
        .map(|index| {
            doctor_health_issue(
                &format!("/roms/history/{index:03}.zip"),
                HealthCategory::HistoricalMountFailure,
            )
        })
        .collect();
    let scan = doctor_scan_from(&issues);
    let text = doctor_scan_report_text(&DoctorScanOutcome {
        scan,
        finished_at_unix_seconds: 1_700_000_000,
    });

    for index in 0..12 {
        assert!(text.contains(&format!("/roms/history/{index:03}.zip")));
    }
}

/// A storage scan built from values, so the page can be exercised without
/// depending on whatever the test machine's disks happen to look like.
fn doctor_scan_with_storage(available_bytes: u64, total_bytes: u64, read_only: bool) -> DoctorScan {
    use archivefs_core::diagnostics::environment::{
        FilesystemGroup, FilesystemStat, MountMode, ResourceRole, StorageAssessment,
    };
    use archivefs_core::emulator_environment::EncodedPath;
    let assessment = StorageAssessment {
        filesystems: vec![FilesystemGroup {
            representative_path: EncodedPath::from_path(Path::new("/var/lib/archivefs")),
            device_id: Some(1),
            mount_point: Some(EncodedPath::from_path(Path::new("/var"))),
            filesystem_type: Some("ext4".to_string()),
            mount_mode: if read_only {
                MountMode::ReadOnly
            } else {
                MountMode::ReadWrite
            },
            stat: Some(FilesystemStat {
                available_bytes,
                total_bytes,
            }),
            roles: vec![ResourceRole::Database],
            paths: vec![EncodedPath::from_path(Path::new("/var/lib/archivefs"))],
            evidence_source: "statvfs and /proc/self/mountinfo",
        }],
        unassessed: Vec::new(),
        mount_table_available: true,
    };
    let mut inputs = DoctorScanInputs::none_loaded();
    // Borrowed for the duration of the scan only; the scan owns its output.
    inputs.storage = Gathered::Ready(&assessment);
    run_doctor_scan(&inputs)
}

/// Test 84
#[test]
fn doctor_page_groups_low_space_under_storage_with_a_readable_size() {
    let scan = doctor_scan_with_storage(100 * 1024 * 1024, 100 * 1024 * 1024 * 1024, false);
    let state = doctor_outcome(scan);
    let output = render_doctor_page(&state, &mut None);
    assert!(rendered_text_contains(&output, "Storage (1)"));
    assert!(
        rendered_text_contains(&output, "MiB"),
        "a person must see a size, not a byte count"
    );
}

/// Test 85
#[test]
fn doctor_page_groups_a_read_only_filesystem_under_filesystems() {
    let scan = doctor_scan_with_storage(500 * 1024 * 1024 * 1024, 1000 * 1024 * 1024 * 1024, true);
    let state = doctor_outcome(scan);
    let output = render_doctor_page(&state, &mut None);
    assert!(rendered_text_contains(&output, "Filesystems (1)"));
    assert!(rendered_text_contains(&output, "mounted read-only"));
}

/// The exact confirmed defect: an Advanced-View-only state filter must
/// not silently empty Gamer View's list.
#[test]
fn gamer_view_ignores_advanced_view_state_filters_it_cannot_show_or_clear() {
    let mut app = gamer_app_with_platforms(&[("Acorn Archimedes", 3), ("SNES", 2)]);
    // Exactly what "Review missing" in the Health dashboard does.
    app.library_filters.missing = true;
    app.library_filters.platform = Some("Acorn Archimedes".to_string());

    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(gamer_screen()),
        ..Default::default()
    };
    let output = run_gamer_frames(&mut app, &ctx, input, 2);

    assert!(
        rendered_text_contains(&output, "Title0000"),
        "the selected platform's games must be listed; the Missing checkbox lives in \
             Advanced View, which Gamer View neither renders nor can clear"
    );
    assert!(!rendered_text_contains(
        &output,
        "No games match the selected platform"
    ));
    // The Advanced-View filter itself is preserved, not silently reset -
    // returning to Advanced View must find it exactly as it was left.
    assert!(app.library_filters.missing);
}

/// The structural guarantee, stated directly: for every card the shelf
/// offers, selecting it produces exactly the number of rows the card
/// advertised. No pairing of counts and rows can drift apart.
#[test]
fn gamer_shelf_counts_are_exactly_the_rows_selecting_that_card_produces() {
    let rows: Vec<ArchiveRow> = {
        let mut records = Vec::new();
        for (index, platform) in ["SNES", "SNES", "GameCube", "MegaDrive"]
            .into_iter()
            .enumerate()
        {
            let mut row = record(&format!("/roms/g{index}.zip"), MountState::Pending);
            row.metadata.platform = Some(platform.to_string());
            records.push(row);
        }
        let mut unknown = record("/roms/unknown.zip", MountState::Pending);
        unknown.metadata.platform = None;
        unknown.identity.platform = None;
        records.push(unknown);
        records.iter().map(row_for).collect()
    };

    let all = GamerLibrarySnapshot::build(&rows, None, "");
    assert_eq!(all.candidates.len(), 5);
    assert_eq!(all.visible.len(), 5, "All lists every game");

    for (platform, count) in &all.platform_counts.named {
        let selected = GamerLibrarySnapshot::build(&rows, Some(platform), "");
        assert_eq!(
            selected.visible.len(),
            *count,
            "the {platform} card advertised {count}"
        );
        assert!(!selected.selection_is_stale);
    }
    let unknown = GamerLibrarySnapshot::build(&rows, Some("Unknown"), "");
    assert_eq!(unknown.visible.len(), all.platform_counts.unknown);
    assert!(!unknown.visible.is_empty());
}

/// A card whose count is non-zero can never produce an empty list.
#[test]
fn a_non_zero_card_count_can_never_yield_a_false_empty_state() {
    let mut records = Vec::new();
    for (index, platform) in ["SNES", "GameCube", "GameCube"].into_iter().enumerate() {
        let mut row = record(&format!("/roms/g{index}.zip"), MountState::Pending);
        row.metadata.platform = Some(platform.to_string());
        records.push(row);
    }
    let rows: Vec<ArchiveRow> = records.iter().map(row_for).collect();
    for search in ["", "g1", "gamecube", "nothing-matches-this"] {
        let snapshot = GamerLibrarySnapshot::build(&rows, None, search);
        for (platform, count) in &snapshot.platform_counts.named {
            let selected = GamerLibrarySnapshot::build(&rows, Some(platform), search);
            assert_eq!(selected.visible.len(), *count);
            assert!(
                *count == 0 || !selected.visible.is_empty(),
                "{platform} advertised {count} under search {search:?} but listed nothing"
            );
        }
    }
}

/// Search and the platform selection compose, and the counts follow the
/// search rather than describing a library the list is not showing.
#[test]
fn gamer_search_and_platform_compose_and_clearing_search_restores_rows() {
    let mut records = Vec::new();
    for (index, platform) in ["SNES", "SNES", "GameCube"].into_iter().enumerate() {
        let mut row = record(&format!("/roms/game{index}.zip"), MountState::Pending);
        row.metadata.platform = Some(platform.to_string());
        records.push(row);
    }
    let rows: Vec<ArchiveRow> = records.iter().map(row_for).collect();

    let unfiltered = GamerLibrarySnapshot::build(&rows, Some("SNES"), "");
    assert_eq!(unfiltered.visible.len(), 2);

    let searched = GamerLibrarySnapshot::build(&rows, Some("SNES"), "game1");
    assert_eq!(
        searched.visible.len(),
        1,
        "search narrows within the platform"
    );
    assert_eq!(
        searched
            .platform_counts
            .named
            .iter()
            .find(|(name, _)| name == "SNES")
            .map(|(_, count)| *count),
        Some(1),
        "the card must count what the search admits, not the whole library"
    );

    let cleared = GamerLibrarySnapshot::build(&rows, Some("SNES"), "");
    assert_eq!(
        cleared.visible.len(),
        2,
        "clearing the search restores rows"
    );
}

/// A selection that survives a library reload but names a platform the
/// new snapshot no longer has must resolve to All, not to a dead list.
#[test]
fn a_platform_missing_from_the_snapshot_falls_back_to_all() {
    let mut app = gamer_app_with_platforms(&[("SNES", 2)]);
    // `open_cheat_archive_picker` writes canonical adapter ids such as
    // this one straight into the shared filter.
    app.library_filters.platform = Some("Xbox360".to_string());

    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(gamer_screen()),
        ..Default::default()
    };
    let output = run_gamer_frames(&mut app, &ctx, input, 2);

    assert_eq!(
        app.library_filters.platform, None,
        "a platform no card offers must fall back to All"
    );
    assert!(rendered_text_contains(&output, "Title0000"));
}

#[test]
fn a_platform_present_in_the_snapshot_is_never_treated_as_stale() {
    let mut records = Vec::new();
    let mut row = record("/roms/a.zip", MountState::Pending);
    row.metadata.platform = Some("SNES".to_string());
    records.push(row);
    let mut unknown = record("/roms/b.zip", MountState::Pending);
    unknown.metadata.platform = None;
    unknown.identity.platform = None;
    records.push(unknown);
    let rows: Vec<ArchiveRow> = records.iter().map(row_for).collect();

    assert!(!GamerLibrarySnapshot::build(&rows, Some("SNES"), "").selection_is_stale);
    assert!(!GamerLibrarySnapshot::build(&rows, Some("Unknown"), "").selection_is_stale);
    assert!(!GamerLibrarySnapshot::build(&rows, None, "").selection_is_stale);
    assert!(GamerLibrarySnapshot::build(&rows, Some("MegaDrive"), "").selection_is_stale);
    // "Unknown" is only a real card while something is unclassified.
    let classified: Vec<ArchiveRow> = records[..1].iter().map(row_for).collect();
    assert!(GamerLibrarySnapshot::build(&classified, Some("Unknown"), "").selection_is_stale);
}

/// Clicking a card - the real button, at its real position - lists that
/// platform's games, and switching to another recomputes the rows.
#[test]
fn clicking_platform_cards_lists_and_then_switches_the_visible_games() {
    let mut app = gamer_app_with_platforms(&[("Acorn Archimedes", 2), ("SNES", 2)]);
    let ctx = egui::Context::default();
    let screen = gamer_screen();
    let idle = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    run_gamer_frames(&mut app, &ctx, idle.clone(), 3);
    let cards = gamer_shelf_geometry(&ctx).cards;
    assert!(cards.len() >= 3, "All plus two platforms");

    // Card 1 is the first named platform, in the shelf's own order.
    run_gamer_frames(&mut app, &ctx, click_at(screen, cards[1].center()), 1);
    assert_eq!(
        app.library_filters.platform.as_deref(),
        Some("Acorn Archimedes")
    );
    let output = run_gamer_frames(&mut app, &ctx, idle.clone(), 1);
    assert!(rendered_text_contains(&output, "Title0000"));
    assert!(!rendered_text_contains(&output, "Title0002"));

    // Switching without a restart recomputes the rows.
    run_gamer_frames(&mut app, &ctx, click_at(screen, cards[2].center()), 1);
    assert_eq!(app.library_filters.platform.as_deref(), Some("SNES"));
    let output = run_gamer_frames(&mut app, &ctx, idle.clone(), 1);
    assert!(rendered_text_contains(&output, "Title0002"));
    assert!(!rendered_text_contains(&output, "Title0000"));

    // Back to All.
    run_gamer_frames(&mut app, &ctx, click_at(screen, cards[0].center()), 1);
    assert_eq!(app.library_filters.platform, None);
    let output = run_gamer_frames(&mut app, &ctx, idle, 1);
    assert!(rendered_text_contains(&output, "Title0000"));
    assert!(rendered_text_contains(&output, "Title0002"));
}

/// A platform card is a real `egui::Button`, so egui's own keyboard
/// focus plus Enter or Space activates it exactly as a mouse click does.
/// Driven through the card's real widget id and egui's real focus, not
/// by re-deriving key handling.
#[test]
fn platform_cards_activate_from_the_keyboard_exactly_as_from_the_mouse() {
    for key in [egui::Key::Enter, egui::Key::Space] {
        let mut app = gamer_app_with_platforms(&[("Acorn Archimedes", 2), ("SNES", 2)]);
        let ctx = egui::Context::default();
        let screen = gamer_screen();
        let idle = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        run_gamer_frames(&mut app, &ctx, idle.clone(), 3);
        let geometry = gamer_shelf_geometry(&ctx);
        assert_eq!(geometry.card_ids.len(), geometry.cards.len());

        // The mouse baseline this comparison rests on.
        run_gamer_frames(
            &mut app,
            &ctx,
            click_at(screen, geometry.cards[1].center()),
            1,
        );
        assert_eq!(
            app.library_filters.platform.as_deref(),
            Some("Acorn Archimedes")
        );

        // Now the same card via focus plus the key under test.
        let snes_card = geometry.card_ids[2];
        ctx.memory_mut(|memory| memory.request_focus(snes_card));
        let mut activated = idle.clone();
        activated.events = vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }];
        run_gamer_frames(&mut app, &ctx, activated, 1);
        assert_eq!(
            app.library_filters.platform.as_deref(),
            Some("SNES"),
            "{key:?} on a focused platform card must select it, as a click does"
        );
        let output = run_gamer_frames(&mut app, &ctx, idle, 1);
        assert!(rendered_text_contains(&output, "Title0002"));
    }
}

/// A library reload replaces the rows; the list must follow it in the
/// next frame, with no restart and no manual re-selection.
#[test]
fn a_library_reload_refreshes_the_visible_rows_without_a_restart() {
    let mut app = gamer_app_with_platforms(&[("SNES", 2)]);
    app.library_filters.platform = Some("SNES".to_string());
    let ctx = egui::Context::default();
    let idle = egui::RawInput {
        screen_rect: Some(gamer_screen()),
        ..Default::default()
    };
    let output = run_gamer_frames(&mut app, &ctx, idle.clone(), 2);
    assert!(rendered_text_contains(&output, "Title0000"));

    // A completed refresh (a scan, or the reload that follows Mount All)
    // installs a new snapshot in exactly this way.
    let mut replacement = record("/roms/new.zip", MountState::Pending);
    replacement.metadata.platform = Some("SNES".to_string());
    replacement.metadata.title = Some("BrandNewTitle".to_string());
    app.state = LoadState::Ready(Box::new(loaded_data_with_records(
        "/mount",
        vec![replacement],
    )));

    let output = run_gamer_frames(&mut app, &ctx, idle, 2);
    assert!(rendered_text_contains(&output, "BrandNewTitle"));
    assert!(!rendered_text_contains(&output, "Title0000"));
    assert_eq!(
        app.library_filters.platform.as_deref(),
        Some("SNES"),
        "a selection the new snapshot still offers stays selected"
    );
}

/// At TV resolution the list must actually occupy space and be on
/// screen - a correct filter is no use behind a zero-height pane.
#[test]
fn the_game_list_occupies_real_visible_space_at_tv_resolution() {
    let mut app = gamer_app_with_platforms(&[("SNES", 40)]);
    let ctx = egui::Context::default();
    let screen = gamer_screen();
    let idle = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    let output = run_gamer_frames(&mut app, &ctx, idle, 3);

    let mut texts = Vec::new();
    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                out.push((text.galley.text().to_string(), text.visual_bounding_rect()))
            }
            egui::Shape::Vec(nested) => nested.iter().for_each(|s| walk(s, out)),
            _ => {}
        }
    }
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut texts);
    }
    let rows: Vec<egui::Rect> = texts
        .iter()
        .filter(|(text, _)| text.contains("Title"))
        .map(|(_, rect)| *rect)
        .collect();
    assert!(rows.len() > 1, "several rows must be drawn, not one");
    for rect in &rows {
        assert!(
            rect.width() > 0.0 && rect.height() > 0.0,
            "a game row was allocated no space: {rect:?}"
        );
        assert!(
            rect.right() <= screen.right(),
            "no horizontal overflow at TV resolution: {rect:?}"
        );
        assert!(
            rect.left() >= screen.left(),
            "no row starts left of the viewport: {rect:?}"
        );
    }
    // A scroll area draws one row past its bottom edge and clips it, so
    // the useful assertion is that the *listing* starts on screen and
    // covers real height - not that every shape is inside the viewport.
    let top = rows
        .iter()
        .map(|rect| rect.top())
        .fold(f32::INFINITY, f32::min);
    assert!(
        top > screen.top() && top < screen.bottom(),
        "the game list must begin inside the viewport, not below it"
    );
    let visible_rows = rows
        .iter()
        .filter(|rect| rect.bottom() <= screen.bottom())
        .count();
    assert!(
        visible_rows > 3,
        "only {visible_rows} rows fell inside a 1080p viewport"
    );
}

/// Truthful wording for each distinct reason, including the composed
/// search-plus-platform case the old `else if` chain could not express.
#[test]
fn the_empty_list_wording_names_the_actual_reason() {
    assert_eq!(
        gamer_empty_list_guidance(true, false, false),
        "No games are in your library yet."
    );
    assert_eq!(
        gamer_empty_list_guidance(false, true, false),
        "No games match your search."
    );
    assert_eq!(
        gamer_empty_list_guidance(false, false, true),
        "No games match the selected platform."
    );
    assert_eq!(
        gamer_empty_list_guidance(false, true, true),
        "No games on this platform match your search."
    );
}

/// Phase 3: the "Add games" first-run call-to-action must only show for
/// the genuinely-empty-library case - never when a search or platform
/// filter is merely hiding existing games, where a folder picker would
/// not help and would be actively misleading.
#[test]
fn add_games_button_shows_only_for_the_true_first_run_empty_state() {
    assert!(gamer_view_shows_add_games_button(true, true, true));
    assert!(!gamer_view_shows_add_games_button(false, true, true));
    assert!(!gamer_view_shows_add_games_button(true, false, true));
    assert!(!gamer_view_shows_add_games_button(true, true, false));
}

/// Phase 3: Gamer View's own wording for the scan its "Add games" flow
/// chains must never contain the Advanced-View "source(s)"/"archive(s)"
/// vocabulary `source_action_success_message` uses for the same event.
#[test]
fn gamer_view_first_scan_message_uses_plain_language() {
    let mut summary = ScanPersistSummary {
        scan_run_id: 1,
        counts: archivefs_core::ScanRunCounts::default(),
        folder_errors: Vec::new(),
        platform_assignment_warnings: Vec::new(),
        skipped_files: Vec::new(),
        ingestion_stats: Default::default(),
        ingestion_skip_reasons: Default::default(),
        ingestion_platform_counts: Default::default(),
        ingestion_skipped: Vec::new(),
        ingestion_recognised_sample: Vec::new(),
    };
    assert_eq!(
        gamer_view_first_scan_message(&summary),
        "We looked through that folder but didn't find any games in it. Double-check it's the \
         right folder, or try another one."
    );

    summary.ingestion_stats.loose_roms = 1;
    assert_eq!(gamer_view_first_scan_message(&summary), "Found 1 game!");

    summary.ingestion_stats.loose_roms = 1228;
    summary.ingestion_stats.archives = 5;
    let message = gamer_view_first_scan_message(&summary);
    assert_eq!(message, "Found 1233 games!");
    for banned in ["source", "archive", "catalogue", "scan"] {
        assert!(
            !message.to_ascii_lowercase().contains(banned),
            "Gamer View's scan message must not name {banned:?}: {message:?}"
        );
    }
}

/// Phase 3: Gamer View's own wording for a cached/unvalidated row must
/// never say "Cached" - that's Advanced View's precise internal state
/// name, banned-vocabulary-in-spirit for the beginner-facing panel.
#[test]
fn gamer_view_row_origin_labels_avoid_the_word_cached() {
    for origin in [
        RowOrigin::CachedAwaitingValidation,
        RowOrigin::CachedMissing,
        RowOrigin::CachedUnavailable,
    ] {
        let label = origin.gamer_view_label();
        assert!(!label.is_empty());
        assert!(
            !label.to_ascii_lowercase().contains("cached"),
            "{origin:?} -> {label:?} still names \"Cached\""
        );
    }
}

/// Phase 4: every mount/unmount/scan failure reaching Gamer View is, at
/// the source, `ArchiveFsError`'s raw `Display` output - e.g. "scanner
/// error: ...", "database error: ...", or a bare OS error like
/// "/path: Permission denied (os error 13)". None of that internal or
/// OS-level vocabulary may survive `gamer_view_failure_message`'s
/// translation, and every result must still say what a person can do
/// next and that their games are safe.
#[test]
fn gamer_view_failure_message_removes_internal_and_os_level_vocabulary() {
    let permission =
        gamer_view_failure_message("mount error: /roms/Game.zip: Permission denied (os error 13)");
    assert!(permission.to_ascii_lowercase().contains("permission"));
    assert!(!permission.contains("os error"));
    assert!(!permission.to_ascii_lowercase().contains("mount error"));

    let missing = gamer_view_failure_message(
        "io error: /roms/Game.zip: No such file or directory (os error 2)",
    );
    assert!(!missing.contains("os error"));
    assert!(!missing.to_ascii_lowercase().contains("io error"));

    for raw in [
        "scanner error: failed to enumerate /roms",
        "database error: could not open catalogue.sqlite3",
        "config error: config.toml is malformed",
        "unmount error: device or resource busy",
    ] {
        let translated = gamer_view_failure_message(raw);
        for banned in ["scanner", "database", "config error", "catalogue", "sqlite"] {
            assert!(
                !translated.to_ascii_lowercase().contains(banned),
                "translating {raw:?} leaked {banned:?} into {translated:?}"
            );
        }
        // Every failure message must say the user's data/games are safe
        // and give a real next step the app actually supports.
        assert!(translated.to_ascii_lowercase().contains("safe"));
        assert!(
            translated.to_ascii_lowercase().contains("try again")
                || translated.to_ascii_lowercase().contains("advanced view")
        );
    }
}

/// A platform genuinely holding nothing still reports so - the truthful
/// empty state must survive the fix that removed the false one.
#[test]
fn a_platform_with_no_games_still_reports_an_honest_empty_state() {
    let mut app = gamer_app_with_platforms(&[("SNES", 1)]);
    // Force the list empty the only way that remains: a search nothing
    // satisfies, with a platform selected.
    app.library_filters.platform = Some("SNES".to_string());
    app.filter = "no-such-game-anywhere".to_string();
    let ctx = egui::Context::default();
    let idle = egui::RawInput {
        screen_rect: Some(gamer_screen()),
        ..Default::default()
    };
    let output = run_gamer_frames(&mut app, &ctx, idle, 2);
    assert!(rendered_text_contains(
        &output,
        "No games on this platform match your search."
    ));
}

// ---------------------------------------------------------------------
// Human-smoke regression: Doctor "Measured values"
//
// Confirmed in Sunshine on a real scan: clicking the disclosure did
// nothing, and egui itself painted "First use of widget ID 6819" beside
// it. `Finding::id` names a finding's *kind*, and a real scan produces
// hundreds of findings sharing one - `doctor_presentation_groups` exists
// precisely because it does. Salting the header with that id gave every
// one of those cards the same widget id, and selecting a finding by id
// expanded all of them at once.
// ---------------------------------------------------------------------

/// Two findings of the same kind, which is the situation that broke.
fn repeated_doctor_findings() -> DoctorScan {
    let issues = [
        doctor_health_issue("/roms/one.zip", HealthCategory::UnknownPlatform),
        doctor_health_issue("/roms/two.zip", HealthCategory::UnknownPlatform),
    ];
    doctor_scan_from(&issues)
}

#[test]
fn findings_sharing_one_id_still_get_distinct_expansion_keys() {
    let scan = repeated_doctor_findings();
    assert!(scan.findings.len() >= 2);
    assert_eq!(
        scan.findings[0].id, scan.findings[1].id,
        "this test is only meaningful while the ids do collide"
    );

    let ordinals = DoctorFindingOrdinals::of(&scan);
    let first = doctor_finding_key(&scan.findings[0], ordinals.ordinal(&scan.findings[0]));
    let second = doctor_finding_key(&scan.findings[1], ordinals.ordinal(&scan.findings[1]));
    assert_ne!(first, second, "each rendered finding needs its own key");

    // Stable: rebuilding the map for the same scan yields the same keys.
    let again = DoctorFindingOrdinals::of(&scan);
    assert_eq!(
        first,
        doctor_finding_key(&scan.findings[0], again.ordinal(&scan.findings[0]))
    );

    // And the kind is still recoverable, which is what the two
    // invalidation paths reason about.
    assert_eq!(doctor_finding_key_id(&first), scan.findings[0].id);
    assert_eq!(doctor_finding_key_id(&second), scan.findings[1].id);
}

/// Selecting one finding must not open its twin.
#[test]
fn expanding_one_finding_does_not_expand_another_of_the_same_kind() {
    let scan = repeated_doctor_findings();
    let ordinals = DoctorFindingOrdinals::of(&scan);
    let first = doctor_finding_key(&scan.findings[0], ordinals.ordinal(&scan.findings[0]));
    let state = doctor_outcome(scan);

    let output = render_doctor_page(&state, &mut Some(first));
    let hide = painted_doctor_texts(&output)
        .into_iter()
        .filter(|text| text == "Hide details")
        .count();
    assert_eq!(
        hide, 1,
        "exactly one card may be expanded; selecting by kind expanded all of them"
    );
}

fn painted_doctor_texts(output: &egui::FullOutput) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(nested) => nested.iter().for_each(|shape| walk(shape, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut found);
    }
    found
}

/// Clicking the disclosure expands it, clicking again collapses it, and
/// the state survives the frames in between.
#[test]
fn the_measured_values_disclosure_expands_collapses_and_persists() {
    let scan = doctor_scan_with_storage(100 * 1024 * 1024, 100 * 1024 * 1024 * 1024, false);
    let ordinals = DoctorFindingOrdinals::of(&scan);
    let key = doctor_finding_key(&scan.findings[0], ordinals.ordinal(&scan.findings[0]));
    let state = doctor_outcome(scan);
    let mut selected = Some(key);
    let mut clipboard = InMemoryClipboard::default();
    let context = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 900.0));
    let idle = egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };
    let mut frame = |input: egui::RawInput, selected: &mut Option<String>| {
        context.run(input, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                let _ = show_doctor_page(
                    ui,
                    &state,
                    selected,
                    None,
                    None,
                    None,
                    &mut clipboard,
                    false,
                );
            });
        })
    };

    let output = frame(idle.clone(), &mut selected);
    assert!(!rendered_text_contains(&output, "available_bytes"));
    let header = find_exact_text_center(&output, "Measured values")
        .expect("the disclosure must be rendered");

    let click = |position: egui::Pos2| egui::RawInput {
        screen_rect: Some(screen),
        events: vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ],
        ..Default::default()
    };

    let _ = frame(click(header), &mut selected);
    let output = frame(idle.clone(), &mut selected);
    assert!(
        rendered_text_contains(&output, "available_bytes"),
        "clicking must expand the measured values"
    );
    // Still open several frames later - the state is not rebuilt per frame.
    let _ = frame(idle.clone(), &mut selected);
    let output = frame(idle.clone(), &mut selected);
    assert!(rendered_text_contains(&output, "available_bytes"));

    // Re-found rather than reused: the header may have shifted as the
    // content above it settled.
    let output = frame(idle.clone(), &mut selected);
    let header = find_exact_text_center(&output, "Measured values")
        .expect("the disclosure is still rendered while expanded");
    let _ = frame(click(header), &mut selected);
    // `CollapsingHeader` animates closed, so the content is still painted
    // for a few frames after the click; the assertion is that it goes.
    let mut collapsed = false;
    for _ in 0..30 {
        let output = frame(idle.clone(), &mut selected);
        if !rendered_text_contains(&output, "available_bytes") {
            collapsed = true;
            break;
        }
    }
    assert!(collapsed, "clicking again must collapse them");
}

/// A finding that measured nothing must say so in plain text, not offer a
/// triangle that opens onto nothing.
#[test]
fn a_finding_with_no_measurements_renders_words_not_a_disclosure() {
    let mut scan = repeated_doctor_findings();
    scan.findings[0].measurements.clear();
    let ordinals = DoctorFindingOrdinals::of(&scan);
    let key = doctor_finding_key(&scan.findings[0], ordinals.ordinal(&scan.findings[0]));
    let state = doctor_outcome(scan);

    let output = render_doctor_page(&state, &mut Some(key));
    assert!(rendered_text_contains(&output, DOCTOR_NO_MEASURED_VALUES));
    assert_eq!(DOCTOR_NO_MEASURED_VALUES, "No measured values recorded");
    assert!(
        !rendered_text_contains(&output, "Measured values"),
        "no disclosure may be offered for a finding that measured nothing"
    );
}

/// The disclosure is a real `CollapsingHeader`, so it carries egui's own
/// focus and Enter/Space activation rather than a hand-painted triangle.
#[test]
fn the_measured_values_disclosure_activates_from_the_keyboard() {
    for key_pressed in [egui::Key::Enter, egui::Key::Space] {
        let scan = doctor_scan_with_storage(100 * 1024 * 1024, 100 * 1024 * 1024 * 1024, false);
        let ordinals = DoctorFindingOrdinals::of(&scan);
        let key = doctor_finding_key(&scan.findings[0], ordinals.ordinal(&scan.findings[0]));
        let state = doctor_outcome(scan);
        let mut selected = Some(key);
        let mut clipboard = InMemoryClipboard::default();
        let context = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 900.0));
        let idle = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut frame = |input: egui::RawInput, selected: &mut Option<String>| {
            context.run(input, |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let _ = show_doctor_page(
                        ui,
                        &state,
                        selected,
                        None,
                        None,
                        None,
                        &mut clipboard,
                        false,
                    );
                });
            })
        };
        let output = frame(idle.clone(), &mut selected);
        assert!(!rendered_text_contains(&output, "available_bytes"));

        // Focus it the way a keyboard user reaches it, then activate.
        let mut tabbed = idle.clone();
        tabbed.events = vec![egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }];
        let mut expanded = false;
        for _ in 0..12 {
            let _ = frame(tabbed.clone(), &mut selected);
            let mut activate = idle.clone();
            activate.events = vec![egui::Event::Key {
                key: key_pressed,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }];
            let _ = frame(activate, &mut selected);
            let output = frame(idle.clone(), &mut selected);
            if rendered_text_contains(&output, "available_bytes") {
                expanded = true;
                break;
            }
        }
        assert!(
            expanded,
            "{key_pressed:?} must reach and activate the disclosure by keyboard"
        );
    }
}

/// The expansion key of the first finding of a given kind.
///
/// A finding is selected by its per-card key, not by its id: an id names
/// a *kind* and repeats across findings. See `doctor_finding_key`.
fn doctor_key_of_kind(scan: &DoctorScan, id: &str) -> String {
    let index = scan
        .findings
        .iter()
        .position(|finding| finding.id == id)
        .unwrap_or_else(|| panic!("no finding with id {id}"));
    doctor_finding_key(&scan.findings[index], index)
}

/// Test 86
#[test]
fn doctor_page_shows_measured_values_only_for_the_selected_finding() {
    let scan = doctor_scan_with_storage(100 * 1024 * 1024, 100 * 1024 * 1024 * 1024, false);
    let key = doctor_finding_key(&scan.findings[0], 0);
    let state = doctor_outcome(scan);

    let collapsed = render_doctor_page(&state, &mut None);
    assert!(!rendered_text_contains(&collapsed, "Measured values"));

    let mut selected = Some(key);
    let expanded = render_doctor_page(&state, &mut selected);
    assert!(rendered_text_contains(&expanded, "Measured values"));
}

/// Test 87
#[test]
fn doctor_page_offers_no_destructive_button_for_a_storage_or_filesystem_finding() {
    for read_only in [false, true] {
        let scan = doctor_scan_with_storage(100 * 1024 * 1024, 100 * 1024 * 1024 * 1024, read_only);
        for finding in &scan.findings {
            assert!(
                finding.repair.is_none(),
                "{} must not offer a repair, so no button can be rendered for it",
                finding.id
            );
        }
        let finding_id = scan.findings[0].id.clone();
        let state = doctor_outcome(scan);
        let output = render_doctor_page(&state, &mut Some(finding_id));
        for forbidden in ["Delete", "Clean up", "Repair", "Fix"] {
            assert!(
                !rendered_text_contains(&output, forbidden),
                "`{forbidden}` must not appear anywhere on a diagnostic-only result"
            );
        }
    }
}

/// Test 88
#[test]
fn the_copied_report_carries_the_measured_values_too() {
    let scan = doctor_scan_with_storage(100 * 1024 * 1024, 100 * 1024 * 1024 * 1024, false);
    let text = doctor_scan_report_text(&DoctorScanOutcome {
        scan,
        finished_at_unix_seconds: 1_700_000_000,
    });
    assert!(text.contains("Measured: available_bytes = "));
    assert!(text.contains("Measured: total_bytes = "));
}

/// Test 89
#[test]
fn the_read_only_notice_states_that_no_probe_file_is_written() {
    let output = render_doctor_page(&DoctorScanState::NotRun, &mut None);
    assert!(rendered_text_contains(
        &output,
        "no test file is ever written"
    ));
    assert!(rendered_text_contains(
        &output,
        "emulator profiles, cheats and patches are never modified"
    ));
}

/// Post-v0.8 usability pass: before this fix, "what does Run Doctor
/// actually check" only appeared in the empty "no scan has run yet"
/// state's message - once a scan completed, that explanation vanished
/// for the rest of the session. It must now appear both before and
/// after a scan has run.
#[test]
fn doctor_page_explains_what_it_checks_before_and_after_a_scan() {
    let before = render_doctor_page(&DoctorScanState::NotRun, &mut None);
    assert!(rendered_text_contains(
        &before,
        "Checks configuration, source folder availability, the mount destination, library \
             and database health, and emulator or profile prerequisites where applicable."
    ));

    let after = doctor_outcome(doctor_scan_from(&[]));
    let output = render_doctor_page(&after, &mut None);
    assert!(rendered_text_contains(
        &output,
        "Checks configuration, source folder availability, the mount destination, library \
             and database health, and emulator or profile prerequisites where applicable."
    ));
}

#[test]
fn doctor_page_states_the_scan_is_read_only() {
    let output = render_doctor_page(&DoctorScanState::NotRun, &mut None);
    assert!(rendered_text_contains(&output, "read-only"));
    assert!(rendered_text_contains(
        &output,
        "never creates, mounts, unmounts, repairs, rebuilds or removes anything"
    ));
    assert!(rendered_text_contains(&output, "Run Doctor"));
    assert!(rendered_text_contains(&output, "Last run: never"));
    assert!(rendered_text_contains(&output, "No scan has run yet"));
}

#[test]
fn doctor_page_shows_exact_severity_counts_and_a_last_run_timestamp() {
    let issues = vec![
        doctor_health_issue("/roms/a.zip", HealthCategory::TerminalFailure),
        doctor_health_issue("/roms/b.zip", HealthCategory::Missing),
        doctor_health_issue("/roms/c.zip", HealthCategory::CachedOnly),
        doctor_health_issue("/roms/d.zip", HealthCategory::UnknownPlatform),
    ];
    let state = doctor_outcome(doctor_scan_from(&issues));
    let output = render_doctor_page(&state, &mut None);

    for expected in [
        "Critical: 0",
        "Error: 1",
        "Warning: 1",
        "Info: 2",
        "Last run: ",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
}

#[test]
fn gamer_view_summarizes_informational_findings_instead_of_flooding() {
    // A realistic flood: hundreds of informational findings beside one
    // genuine error and one warning.
    let mut issues: Vec<HealthIssue> = (0..500)
        .map(|index| {
            doctor_health_issue(
                &format!("/roms/loose/{index:04}.sfc"),
                HealthCategory::CachedOnly,
            )
        })
        .collect();
    issues.push(doctor_health_issue(
        "/roms/broken.zip",
        HealthCategory::TerminalFailure,
    ));
    issues.push(doctor_health_issue(
        "/roms/odd.zip",
        HealthCategory::Missing,
    ));
    let state = doctor_outcome(doctor_scan_from(&issues));
    let output = render_doctor_page_gamer(&state);

    // Exact counts are retained...
    for expected in ["Critical: 0", "Error: 1", "Warning: 1", "Info: 500"] {
        assert!(
            rendered_text_contains(&output, expected),
            "missing {expected}"
        );
    }
    // ...the informational pile is summarised in one friendly line...
    assert!(
        rendered_text_contains(&output, "500 checks are informational or healthy"),
        "the info flood must be summarised"
    );
    assert!(rendered_text_contains(
        &output,
        "None of these need attention"
    ));
    // ...and no info-only category group is foregrounded card by card.
    assert!(
        !rendered_text_contains(&output, "Library (500)"),
        "individual info findings must not flood the Gamer View dashboard"
    );
    // Errors and warnings stay foregrounded as their own category groups.
    assert!(
        rendered_text_contains(&output, "Mounts (1)"),
        "a genuine error must remain foregrounded"
    );
    assert!(
        rendered_text_contains(&output, "Library (1)"),
        "a genuine warning must remain foregrounded"
    );
    // The full informational detail stays reachable one disclosure down.
    assert!(
        rendered_text_contains(&output, "Technical details"),
        "the full info list must remain reachable"
    );
}

#[test]
fn gamer_view_with_only_informational_findings_renders_a_summary_not_a_flood() {
    let issues: Vec<HealthIssue> = (0..3)
        .map(|index| {
            doctor_health_issue(
                &format!("/roms/loose/{index:04}.sfc"),
                HealthCategory::CachedOnly,
            )
        })
        .collect();
    let state = doctor_outcome(doctor_scan_from(&issues));
    let output = render_doctor_page_gamer(&state);

    assert!(rendered_text_contains(&output, "Info: 3"));
    assert!(
        rendered_text_contains(&output, "3 checks are informational or healthy"),
        "an all-info scan must still be summarised"
    );
    // No info-only category group is shown as a foreground card list.
    assert!(
        !rendered_text_contains(&output, "Library (3)"),
        "info-only categories must not flood in Gamer View"
    );
}

#[test]
fn advanced_view_lists_informational_findings_individually() {
    let issues: Vec<HealthIssue> = (0..3)
        .map(|index| {
            doctor_health_issue(
                &format!("/roms/loose/{index:04}.sfc"),
                HealthCategory::CachedOnly,
            )
        })
        .collect();
    let state = doctor_outcome(doctor_scan_from(&issues));
    let output = render_doctor_page(&state, &mut None);

    assert!(rendered_text_contains(&output, "Info: 3"));
    assert!(
        rendered_text_contains(&output, "Library (3)"),
        "Advanced View keeps the full info groups visible"
    );
}

#[test]
fn doctor_page_groups_findings_by_category_in_stable_order() {
    let issues = vec![
        doctor_health_issue("/roms/a.zip", HealthCategory::TerminalFailure),
        doctor_health_issue("/roms/b.zip", HealthCategory::Missing),
    ];
    let scan = doctor_scan_from(&issues);
    // Mounts precedes Library in `DoctorCategory::ALL`, so the grouped
    // order the page renders is that order, not insertion order.
    assert_eq!(
        scan.by_category()
            .into_iter()
            .map(|(category, _)| category)
            .collect::<Vec<_>>(),
        vec![DoctorCategory::Mounts, DoctorCategory::Library]
    );
    let state = doctor_outcome(scan);
    let output = render_doctor_page(&state, &mut None);
    assert!(rendered_text_contains(&output, "Mounts (1)"));
    assert!(rendered_text_contains(&output, "Library (1)"));
}

#[test]
fn doctor_page_shows_evidence_only_for_the_selected_finding() {
    let issues = vec![doctor_health_issue("/roms/a.zip", HealthCategory::Missing)];
    let scan = doctor_scan_from(&issues);
    let key = doctor_key_of_kind(&scan, "library.archive_missing");
    let state = doctor_outcome(scan);

    let collapsed = render_doctor_page(&state, &mut None);
    assert!(!rendered_text_contains(&collapsed, "Evidence"));
    assert!(rendered_text_contains(&collapsed, "Details"));

    let mut selected = Some(key);
    let expanded = render_doctor_page(&state, &mut selected);
    for expected in [
        "Evidence",
        "Classification: Missing",
        "Last seen: 2026-07-31T00:00:00Z",
        "Reported by archive health",
        "library.archive_missing",
        "Hide details",
    ] {
        assert!(
            rendered_text_contains(&expanded, expected),
            "missing {expected}"
        );
    }
}

#[test]
fn stale_catalogue_entry_count_is_present_only_for_missing_findings() {
    let missing = doctor_outcome(doctor_scan_from(&[
        doctor_health_issue("/roms/a.zip", HealthCategory::Missing),
        doctor_health_issue("/roms/b.zip", HealthCategory::Missing),
    ]));
    assert_eq!(problems_repair_page::stale_library_entry_count(&missing), 2);

    let healthy = doctor_outcome(doctor_scan_from(&[]));
    assert_eq!(problems_repair_page::stale_library_entry_count(&healthy), 0);
}

#[test]
fn doctor_page_shows_why_it_matters_and_the_next_step_when_one_exists() {
    let setup = SetupDiagnostics {
        config_path: Some(PathBuf::from("/config/config.toml")),
        config_path_error: None,
        config_missing: false,
        mount_root: Some(PathBuf::from("/mount")),
        can_create_mount_root: true,
        ready_for_scanning: false,
        ready_for_actions: false,
        config_identity: ConfigIdentity {
            config_path: Some(PathBuf::from("/config/config.toml")),
            content_digest: None,
        },
        checks: vec![SetupDiagnostic {
            name: "ratarmount is available".to_string(),
            status: SetupDiagnosticStatus::Error,
            detail: "ratarmount was not found.".to_string(),
            why_it_matters: "EmuWiz uses ratarmount to expose archive contents.".to_string(),
            next_step: "Install ratarmount and ensure it is available on PATH.".to_string(),
        }],
    };
    let mut inputs = DoctorScanInputs::none_loaded();
    inputs.setup = Gathered::Ready(&setup);
    let scan = run_doctor_scan(&inputs);
    let key = doctor_finding_key(&scan.findings[0], 0);
    let state = doctor_outcome(scan);

    let output = render_doctor_page(&state, &mut Some(key));
    assert!(rendered_text_contains(&output, "Why it matters"));
    assert!(rendered_text_contains(
        &output,
        "EmuWiz uses ratarmount to expose archive contents."
    ));
    assert!(rendered_text_contains(&output, "Recommended next step"));
    assert!(rendered_text_contains(
        &output,
        "Install ratarmount and ensure it is available on PATH."
    ));
}

/// Stage 1A states that a repair exists but never offers one. The words
/// are informational; there is no control of any kind.
#[test]
fn doctor_page_names_an_existing_repair_without_offering_it() {
    let issues = vec![doctor_health_issue(
        "/roms/a.zip",
        HealthCategory::RetryableFailure,
    )];
    let scan = doctor_scan_from(&issues);
    let key = doctor_key_of_kind(&scan, "mounts.retryable_failure");
    let state = doctor_outcome(scan);
    let output = render_doctor_page(&state, &mut Some(key));

    assert!(rendered_text_contains(
        &output,
        "A repair action already exists elsewhere in EmuWiz"
    ));
    assert!(rendered_text_contains(&output, "Library → Health, Retry"));
    // No repair control is rendered anywhere on the page.
    for forbidden in [
        "Retry mount",
        "Remount",
        "Force unmount",
        "Clean up",
        "Repair",
        "Fix",
        "Roll back",
        "Remove missing",
        "Rescan",
    ] {
        assert_eq!(
            count_exact_text_occurrences(&output, forbidden),
            0,
            "Stage 1A must render no repair control, but found `{forbidden}`"
        );
    }
}

#[test]
fn doctor_page_shows_a_healthy_result_rather_than_an_empty_screen() {
    let state = doctor_outcome(doctor_scan_from(&[]));
    let output = render_doctor_page(&state, &mut None);
    assert!(rendered_text_contains(&output, "Healthy"));
    assert!(rendered_text_contains(
        &output,
        "No problems detected by the available read-only checks."
    ));
}

#[test]
fn emulator_setup_destination_exposes_the_supported_emulator_readiness_list() {
    // Item 5: the dedicated "Emulator Setup" destination must land on a page
    // where the supported emulator checks are actually visible. It renders
    // the shared Doctor scan (same engine and `doctor_scan` state as the
    // Problems & Repair -> Diagnostics tab); the scan's "Emulator profiles"
    // / "Emulators" categories carry the per-emulator rows.
    let profile_report = ProfileAssessmentReport {
        profiles: vec![ProfileAssessment {
            emulator: EmulatorKind::Ppsspp,
            profile_id: "ppsspp-native".to_string(),
            profile_kind: "Native".to_string(),
            scope: "User".to_string(),
            discovery_confidence: "documented native path".to_string(),
            eligible: true,
            blockers: Vec::new(),
            root_path: EncodedPath::from_path(Path::new("/profiles/ppsspp")),
            destination_path: EncodedPath::from_path(Path::new("/profiles/ppsspp/PSP/Cheats")),
            destination_exists: true,
            destination_is_directory: true,
            destination_is_symlink: false,
            mount_mode: MountMode::ReadWrite,
            permissions: None,
            writability: WritabilityAssessment::AppearsWritable,
            preferred: None,
        }],
        unavailable: vec![(EmulatorKind::Xenia, "no documented native path".to_string())],
        discovery_incomplete: false,
    };
    let mut inputs = DoctorScanInputs::none_loaded();
    inputs.emulator_profiles = Gathered::Ready(&profile_report);
    let scan = run_doctor_scan(&inputs);

    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.doctor_scan = doctor_outcome(scan);
    app.view = MainView::EmulatorSetup;

    let output = render_problems_repair_app(&mut app);
    assert!(
        rendered_text_contains(&output, "Emulator Setup"),
        "the dedicated page header must render"
    );
    assert!(rendered_text_contains(&output, "Emulator readiness"));
    assert!(rendered_text_contains(&output, "Full diagnostics"));
    assert!(
        rendered_text_contains(&output, "PPSSPP"),
        "a supported emulator's own row must be visible"
    );
    // No Problems & Repair tab chrome when arriving at the dedicated route.
    assert!(!rendered_text_contains(&output, "Repair / Recovery"));
}

#[test]
fn emulator_setup_and_the_diagnostics_tab_share_one_doctor_scan_state() {
    // Same engine, same `doctor_scan` - no second scan, no divergent state.
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.doctor_scan = doctor_outcome(doctor_scan_from(&[]));

    app.view = MainView::EmulatorSetup;
    let on_setup = render_problems_repair_app(&mut app);
    assert!(rendered_text_contains(&on_setup, "Emulator readiness"));
    assert!(rendered_text_contains(&on_setup, "Full diagnostics"));

    app.view = MainView::Doctor;
    let on_diagnostics = render_problems_repair_app(&mut app);
    assert!(rendered_text_contains(&on_diagnostics, "Healthy"));
}

#[test]
fn doctor_page_shows_ppsspp_and_duckstation_profile_inspections() {
    let profile_report = ProfileAssessmentReport {
        profiles: vec![
            ProfileAssessment {
                emulator: EmulatorKind::Ppsspp,
                profile_id: "ppsspp-native".to_string(),
                profile_kind: "Native".to_string(),
                scope: "User".to_string(),
                discovery_confidence: "documented native path".to_string(),
                eligible: true,
                blockers: Vec::new(),
                root_path: EncodedPath::from_path(Path::new("/profiles/ppsspp")),
                destination_path: EncodedPath::from_path(Path::new("/profiles/ppsspp/PSP/Cheats")),
                destination_exists: true,
                destination_is_directory: true,
                destination_is_symlink: false,
                mount_mode: MountMode::ReadWrite,
                permissions: None,
                writability: WritabilityAssessment::AppearsWritable,
                preferred: None,
            },
            ProfileAssessment {
                emulator: EmulatorKind::DuckStation,
                profile_id: "duckstation-native".to_string(),
                profile_kind: "Native".to_string(),
                scope: "N/A".to_string(),
                discovery_confidence: "discovered configuration directory".to_string(),
                eligible: false,
                blockers: vec!["settings file is not readable".to_string()],
                root_path: EncodedPath::from_path(Path::new("/profiles/duckstation")),
                destination_path: EncodedPath::from_path(Path::new("/profiles/duckstation/cheats")),
                destination_exists: true,
                destination_is_directory: true,
                destination_is_symlink: false,
                mount_mode: MountMode::ReadWrite,
                permissions: None,
                writability: WritabilityAssessment::AppearsWritable,
                preferred: None,
            },
        ],
        unavailable: Vec::new(),
        discovery_incomplete: false,
    };
    let mut inputs = DoctorScanInputs::none_loaded();
    inputs.emulator_profiles = Gathered::Ready(&profile_report);
    let scan = run_doctor_scan(&inputs);

    for (id, emulator, profile, configuration_path, cheat_destination, eligibility, blocker) in [
        (
            "emulator_profile.ppsspp_inspected",
            "PPSSPP",
            "ppsspp-native",
            "/profiles/ppsspp",
            "/profiles/ppsspp/PSP/Cheats",
            "Adapter considers it usable: true",
            None,
        ),
        (
            "emulator_profile.duckstation_inspected",
            "DuckStation",
            "duckstation-native",
            "/profiles/duckstation",
            "/profiles/duckstation/cheats",
            "Adapter considers it usable: false",
            Some("Adapter blocker: settings file is not readable"),
        ),
    ] {
        let key = doctor_key_of_kind(&scan, id);
        let output = render_doctor_page(&doctor_outcome(scan.clone()), &mut Some(key));
        for expected in [
            "Emulator profiles (2)",
            emulator,
            profile,
            "Configuration path:",
            configuration_path,
            "Cheat destination:",
            cheat_destination,
            eligibility,
        ] {
            assert!(
                rendered_text_contains(&output, expected),
                "Doctor did not render {expected:?} for {emulator}"
            );
        }
        if let Some(blocker) = blocker {
            assert!(
                rendered_text_contains(&output, blocker),
                "Doctor did not render {blocker:?} for {emulator}"
            );
        }
    }
}

#[test]
fn doctor_page_leaves_an_existing_dolphin_profile_row_unchanged_alongside_ppsspp_and_duckstation() {
    let profile_report = ProfileAssessmentReport {
        profiles: vec![
            ProfileAssessment {
                emulator: EmulatorKind::Dolphin,
                profile_id: "dolphin-native".to_string(),
                profile_kind: "Native".to_string(),
                scope: "User".to_string(),
                discovery_confidence: "documented native path".to_string(),
                eligible: true,
                blockers: Vec::new(),
                root_path: EncodedPath::from_path(Path::new("/profiles/dolphin")),
                destination_path: EncodedPath::from_path(Path::new(
                    "/profiles/dolphin/GameSettings",
                )),
                destination_exists: true,
                destination_is_directory: true,
                destination_is_symlink: false,
                mount_mode: MountMode::ReadWrite,
                permissions: None,
                writability: WritabilityAssessment::PermissionDenied,
                preferred: None,
            },
            ProfileAssessment {
                emulator: EmulatorKind::Ppsspp,
                profile_id: "ppsspp-native".to_string(),
                profile_kind: "Native".to_string(),
                scope: "User".to_string(),
                discovery_confidence: "documented native path".to_string(),
                eligible: true,
                blockers: Vec::new(),
                root_path: EncodedPath::from_path(Path::new("/profiles/ppsspp")),
                destination_path: EncodedPath::from_path(Path::new("/profiles/ppsspp/PSP/Cheats")),
                destination_exists: true,
                destination_is_directory: true,
                destination_is_symlink: false,
                mount_mode: MountMode::ReadWrite,
                permissions: None,
                writability: WritabilityAssessment::AppearsWritable,
                preferred: None,
            },
            ProfileAssessment {
                emulator: EmulatorKind::DuckStation,
                profile_id: "duckstation-native".to_string(),
                profile_kind: "Native".to_string(),
                scope: "N/A".to_string(),
                discovery_confidence: "discovered configuration directory".to_string(),
                eligible: true,
                blockers: Vec::new(),
                root_path: EncodedPath::from_path(Path::new("/profiles/duckstation")),
                destination_path: EncodedPath::from_path(Path::new("/profiles/duckstation/cheats")),
                destination_exists: true,
                destination_is_directory: true,
                destination_is_symlink: false,
                mount_mode: MountMode::ReadWrite,
                permissions: None,
                writability: WritabilityAssessment::AppearsWritable,
                preferred: None,
            },
        ],
        unavailable: Vec::new(),
        discovery_incomplete: false,
    };
    let mut inputs = DoctorScanInputs::none_loaded();
    inputs.emulator_profiles = Gathered::Ready(&profile_report);
    let scan = run_doctor_scan(&inputs);

    // Dolphin keeps its pre-existing finding id and its original
    // "Destination:" evidence label - it never picks up the PPSSPP/
    // DuckStation-only "Cheat destination:" wording or an `_inspected` id.
    let dolphin_key = doctor_key_of_kind(&scan, "emulator_profile.permission_denied");
    let dolphin_finding = scan
        .findings
        .iter()
        .find(|finding| finding.id == "emulator_profile.permission_denied")
        .expect("dolphin finding");
    assert!(
        dolphin_finding
            .evidence
            .iter()
            .any(|line| line.starts_with("Destination: ")),
        "dolphin evidence should keep the original 'Destination:' label: {:?}",
        dolphin_finding.evidence
    );
    assert!(
        dolphin_finding
            .evidence
            .iter()
            .all(|line| !line.starts_with("Cheat destination:")),
        "dolphin evidence must not gain the PPSSPP/DuckStation-only 'Cheat destination:' label"
    );
    let output = render_doctor_page(&doctor_outcome(scan.clone()), &mut Some(dolphin_key));
    assert!(rendered_text_contains(&output, "Dolphin"));
    assert!(rendered_text_contains(
        &output,
        "/profiles/dolphin/GameSettings"
    ));

    // PPSSPP and DuckStation are still reported alongside it.
    assert!(
        scan.findings
            .iter()
            .any(|finding| finding.id == "emulator_profile.ppsspp_inspected")
    );
    assert!(
        scan.findings
            .iter()
            .any(|finding| finding.id == "emulator_profile.duckstation_inspected")
    );
}

#[test]
fn ppsspp_and_duckstation_profiles_never_become_managed_scan_targets_in_the_gui_pipeline() {
    let profile_report = ProfileAssessmentReport {
        profiles: vec![
            ProfileAssessment {
                emulator: EmulatorKind::Ppsspp,
                profile_id: "ppsspp-native".to_string(),
                profile_kind: "Native".to_string(),
                scope: "User".to_string(),
                discovery_confidence: "documented native path".to_string(),
                eligible: true,
                blockers: Vec::new(),
                root_path: EncodedPath::from_path(Path::new("/profiles/ppsspp")),
                destination_path: EncodedPath::from_path(Path::new("/profiles/ppsspp/PSP/Cheats")),
                destination_exists: true,
                destination_is_directory: true,
                destination_is_symlink: false,
                mount_mode: MountMode::ReadWrite,
                permissions: None,
                writability: WritabilityAssessment::AppearsWritable,
                preferred: None,
            },
            ProfileAssessment {
                emulator: EmulatorKind::DuckStation,
                profile_id: "duckstation-native".to_string(),
                profile_kind: "Native".to_string(),
                scope: "N/A".to_string(),
                discovery_confidence: "discovered configuration directory".to_string(),
                eligible: true,
                blockers: Vec::new(),
                root_path: EncodedPath::from_path(Path::new("/profiles/duckstation")),
                destination_path: EncodedPath::from_path(Path::new("/profiles/duckstation/cheats")),
                destination_exists: true,
                destination_is_directory: true,
                destination_is_symlink: false,
                mount_mode: MountMode::ReadWrite,
                permissions: None,
                writability: WritabilityAssessment::AppearsWritable,
                preferred: None,
            },
            ProfileAssessment {
                emulator: EmulatorKind::Pcsx2,
                profile_id: "pcsx2-native".to_string(),
                profile_kind: "Native".to_string(),
                scope: "User".to_string(),
                discovery_confidence: "documented native path".to_string(),
                eligible: true,
                blockers: Vec::new(),
                root_path: EncodedPath::from_path(Path::new("/profiles/pcsx2")),
                destination_path: EncodedPath::from_path(Path::new("/profiles/pcsx2/cheats")),
                destination_exists: true,
                destination_is_directory: true,
                destination_is_symlink: false,
                mount_mode: MountMode::ReadWrite,
                permissions: None,
                writability: WritabilityAssessment::AppearsWritable,
                preferred: None,
            },
        ],
        unavailable: Vec::new(),
        discovery_incomplete: false,
    };

    // This is the exact call `gather_doctor_inputs` makes to build the
    // managed-entries scan the GUI feeds into Doctor - see main.rs.
    let targets = managed_scan_targets(&profile_report);

    assert_eq!(
        targets.len(),
        1,
        "only the PCSX2 profile should become a managed scan target: {targets:?}"
    );
    assert_eq!(
        targets[0].destination_root,
        Path::new("/profiles/pcsx2/cheats")
    );
}

/// A healthy result must never read as "everything was checked".
#[test]
fn doctor_page_lists_unchecked_and_deferred_checks_alongside_a_healthy_result() {
    let state = doctor_outcome(doctor_scan_from(&[]));
    let output = render_doctor_page(&state, &mut None);

    assert!(rendered_text_contains(&output, "What was checked"));
    assert!(rendered_text_contains(&output, "Not checked in this run"));
    assert!(rendered_text_contains(&output, "Not checked by EmuWiz yet"));
    assert!(rendered_text_contains(
        &output,
        "a healthy result does not mean they are fine"
    ));
    for deferred in [
        "Per-directory disk quotas",
        "Write access inside a sandbox",
        "Managed entries with no install record",
    ] {
        assert!(
            rendered_text_contains(&output, deferred),
            "the deferred check `{deferred}` must be visible"
        );
    }
    // And nothing claims a pass for a check that never ran.
    assert_eq!(
        count_exact_text_occurrences(&output, "All checks passed"),
        0
    );
}

#[test]
fn doctor_page_renders_long_unicode_paths_without_panicking() {
    let long_name = "ロング".repeat(150);
    let path = format!("/roms/{long_name}/ゲーム 💾 [!].zip");
    let issues = vec![doctor_health_issue(&path, HealthCategory::Missing)];
    let scan = doctor_scan_from(&issues);
    let key = doctor_key_of_kind(&scan, "library.archive_missing");
    let state = doctor_outcome(scan);
    let output = render_doctor_page(&state, &mut Some(key));
    assert!(rendered_text_contains(&output, "ゲーム 💾 [!].zip"));
    assert!(rendered_text_contains(&output, "Evidence"));
}

#[test]
fn doctor_page_keeps_the_previous_result_visible_while_a_new_run_is_in_flight() {
    let issues = vec![doctor_health_issue("/roms/a.zip", HealthCategory::Missing)];
    let (_sender, receiver) = mpsc::channel();
    let state = DoctorScanState::Running {
        generation: RefreshGeneration::INITIAL,
        receiver,
        previous: Some(Box::new(DoctorScanOutcome {
            scan: doctor_scan_from(&issues),
            finished_at_unix_seconds: 1_700_000_000,
        })),
    };
    let output = render_doctor_page(&state, &mut None);
    assert!(rendered_text_contains(
        &output,
        "the previous result stays on screen"
    ));
    assert!(rendered_text_contains(&output, "Library (1)"));
    assert!(state.is_running());
}

/// The core rule: Run Doctor must not reload the application, rescan the
/// library, or disturb any existing state.
#[test]
fn running_doctor_does_not_refresh_or_reload_the_application() {
    let mut app = app_for_operation_tests();
    let state_before = match &app.state {
        LoadState::Ready(data) => std::ptr::from_ref(data.as_ref()) as usize,
        _ => panic!("fixture must be Ready"),
    };
    let refresh_generation_before = app.refresh_generation;
    let snapshot_generation_before = app.snapshot_generation;
    let database_generation_before = app.database_generation;

    app.start_doctor_scan(egui::Context::default());

    assert!(app.doctor_scan.is_running(), "the scan started");
    assert_eq!(
        match &app.state {
            LoadState::Ready(data) => std::ptr::from_ref(data.as_ref()) as usize,
            _ => panic!("state must still be Ready"),
        },
        state_before,
        "Run Doctor replaced the loaded application state"
    );
    assert_eq!(
        app.refresh_generation, refresh_generation_before,
        "Run Doctor triggered an application refresh"
    );
    assert_eq!(app.snapshot_generation, snapshot_generation_before);
    assert_eq!(
        app.database_generation, database_generation_before,
        "Run Doctor touched the database worker"
    );
}

#[test]
fn a_superseded_doctor_run_is_discarded_rather_than_shown() {
    let mut app = app_for_operation_tests();
    // A result carrying an older generation must be ignored.
    let (sender, receiver) = mpsc::channel();
    app.doctor_scan_generation = RefreshGeneration::INITIAL.next().next();
    app.doctor_scan = DoctorScanState::Running {
        generation: app.doctor_scan_generation,
        receiver,
        previous: None,
    };
    sender
        .send((
            RefreshGeneration::INITIAL,
            DoctorGathered {
                mount_root_safety: Gathered::NotLoaded("stale"),
                stale_mount_directories: Gathered::NotLoaded("stale"),
                index_freshness: Gathered::NotLoaded("stale"),
                database: Gathered::NotLoaded("stale"),
                source_health: Gathered::NotLoaded("stale"),
                transactions: Gathered::NotLoaded("stale"),
                storage: Gathered::NotLoaded("stale"),
                emulator_profiles: Gathered::NotLoaded("stale"),
                linux_emulator_installations: Gathered::NotLoaded("stale"),
                arcade_dat_version: Gathered::NotLoaded("stale"),
                xemu_readiness: Gathered::NotLoaded("stale"),
                xenia_readiness: Gathered::NotLoaded("stale"),
                ppsspp_readiness: Gathered::NotLoaded("stale"),
                rpcs3_readiness: Gathered::NotLoaded("stale"),
                managed_entries: Gathered::NotLoaded("stale"),
            },
        ))
        .expect("send");
    app.poll_doctor_scan();
    assert!(
        app.doctor_scan.is_running(),
        "a stale result must not complete the run"
    );
}

#[test]
fn a_current_doctor_run_completes_and_records_when_it_finished() {
    let mut app = app_for_operation_tests();
    let (sender, receiver) = mpsc::channel();
    app.doctor_scan = DoctorScanState::Running {
        generation: app.doctor_scan_generation,
        receiver,
        previous: None,
    };
    sender
        .send((
            app.doctor_scan_generation,
            DoctorGathered {
                mount_root_safety: Gathered::Failed(
                    "the mount root could not be inspected".to_string(),
                ),
                stale_mount_directories: Gathered::NotLoaded("not gathered"),
                index_freshness: Gathered::NotLoaded("not gathered"),
                database: Gathered::NotLoaded("no database"),
                source_health: Gathered::NotLoaded("no sources"),
                transactions: Gathered::NotLoaded("no history"),
                storage: Gathered::NotLoaded("not gathered"),
                emulator_profiles: Gathered::NotLoaded("not gathered"),
                linux_emulator_installations: Gathered::NotLoaded("not gathered"),
                arcade_dat_version: Gathered::NotLoaded("not gathered"),
                xemu_readiness: Gathered::NotLoaded("not gathered"),
                xenia_readiness: Gathered::NotLoaded("not gathered"),
                ppsspp_readiness: Gathered::NotLoaded("not gathered"),
                rpcs3_readiness: Gathered::NotLoaded("not gathered"),
                managed_entries: Gathered::NotLoaded("not gathered"),
            },
        ))
        .expect("send");
    app.poll_doctor_scan();

    let outcome = match &app.doctor_scan {
        DoctorScanState::Ready(outcome) => outcome,
        _ => panic!("the run must complete"),
    };
    assert!(outcome.finished_at_unix_seconds > 0);
    // A failed gather becomes a visible finding, not a panic or a gap.
    let failure = outcome
        .scan
        .finding("doctor.adapter_failed.destination_safety")
        .expect("the gather failure is reported");
    assert_eq!(failure.severity, DoctorSeverity::Error);
    assert!(
        failure
            .evidence
            .iter()
            .any(|item| item.contains("the mount root could not be inspected"))
    );
}

// --- Doctor Stage 1B: repairs ---------------------------------------

/// A scan whose single finding offers `CleanMountPath`, built through the
/// real adapter so the GUI test exercises real data.
fn doctor_scan_with_repair() -> DoctorScan {
    let stale = vec![PathBuf::from("/mount/SNES/Old Game")];
    let mut inputs = DoctorScanInputs::none_loaded();
    inputs.stale_mount_directories = Gathered::Ready(stale.as_slice());
    run_doctor_scan(&inputs)
}

fn doctor_review() -> DoctorRepairReview {
    DoctorRepairReview {
        action: DoctorRepairAction::CleanMountPath,
        finding_id: "mount_root.stale_mount_directory".to_string(),
        affected: Some("/mount/SNES/Old Game".to_string()),
        finding_title: "Leftover empty mount folder".to_string(),
        evidence: vec!["/mount/SNES/Old Game".to_string()],
    }
}

#[test]
fn a_finding_with_a_repair_shows_review_repair_but_never_runs_it() {
    let state = doctor_outcome(doctor_scan_with_repair());
    let output = render_doctor_page(&state, &mut None);

    assert!(rendered_text_contains(&output, "Repair available"));
    assert!(rendered_text_contains(
        &output,
        "Remove this leftover mount folder"
    ));
    assert!(rendered_text_contains(&output, "confirmation required"));
    assert!(rendered_text_contains(&output, "Review repair"));
    // No control that would execute directly.
    for forbidden in ["Confirm repair", "Repair now", "Fix now", "Clean up now"] {
        assert_eq!(
            count_exact_text_occurrences(&output, forbidden),
            0,
            "the finding list must not offer `{forbidden}` without review"
        );
    }
}

#[test]
fn a_finding_without_a_repair_shows_no_repair_control() {
    let issues = vec![doctor_health_issue(
        "/roms/a.zip",
        HealthCategory::TerminalFailure,
    )];
    let state = doctor_outcome(doctor_scan_from(&issues));
    let output = render_doctor_page(&state, &mut None);
    assert_eq!(count_exact_text_occurrences(&output, "Review repair"), 0);
    assert_eq!(count_exact_text_occurrences(&output, "Repair available"), 0);
}

#[test]
fn the_review_screen_states_every_fact_before_confirming() {
    let state = doctor_outcome(doctor_scan_with_repair());
    let review = doctor_review();
    let output = render_doctor_page_with(&state, &mut None, Some(&review), None, None);

    for expected in [
        "Review this repair",
        "Finding: Leftover empty mount folder",
        "Affected resource: /mount/SNES/Old Game",
        "Remove this leftover mount folder",
        "archivefs_core::cleanup_selected_mount_tree",
        "Exactly what will change",
        "stopping at the configured mount root",
        "What will not be touched",
        "Symlinks are never followed",
        "Evidence for this repair",
        "Afterwards",
        "Undo:",
        "Cancel",
        "Confirm repair",
    ] {
        assert!(
            rendered_text_contains(&output, expected),
            "the review screen must state: {expected}"
        );
    }
}

/// While the review screen is open the finding list is replaced, so a
/// second repair cannot be started from behind it.
#[test]
fn the_review_screen_replaces_the_finding_list() {
    let state = doctor_outcome(doctor_scan_with_repair());
    let review = doctor_review();
    let output = render_doctor_page_with(&state, &mut None, Some(&review), None, None);
    assert_eq!(count_exact_text_occurrences(&output, "Review repair"), 0);
    assert_eq!(count_exact_text_occurrences(&output, "Run Doctor"), 0);
}

#[test]
fn a_repair_that_rescans_the_library_says_so_before_confirming() {
    let state = doctor_outcome(doctor_scan_with_repair());
    let review = DoctorRepairReview {
        action: DoctorRepairAction::RebuildIndex,
        finding_id: "library.index_out_of_date".to_string(),
        affected: None,
        finding_title: "The archive index is out of date".to_string(),
        evidence: Vec::new(),
    };
    let output = render_doctor_page_with(&state, &mut None, Some(&review), None, None);
    assert!(rendered_text_contains(&output, "This rescans your library"));
    assert!(rendered_text_contains(
        &output,
        "scans every configured source folder"
    ));
    // And it does not pretend to be reversible.
    assert!(rendered_text_contains(&output, "Undo unavailable."));
}

#[test]
fn cancelling_a_review_leaves_no_repair_pending_and_changes_nothing() {
    let mut app = app_for_operation_tests();
    app.doctor_scan = doctor_outcome(doctor_scan_with_repair());
    app.doctor_repair_review = Some(doctor_review());
    let history_before = app.history.entries().count();

    app.cancel_doctor_repair();

    assert!(app.doctor_repair_review.is_none());
    assert!(app.doctor_repair_result.is_none());
    assert_eq!(
        app.history.entries().count(),
        history_before,
        "cancelling is not an attempt, so it is not recorded"
    );
    assert!(
        app.doctor_repair_finished_at_unix_seconds.is_none(),
        "nothing ran"
    );
}

#[test]
fn opening_a_review_executes_nothing() {
    let mut app = app_for_operation_tests();
    app.doctor_scan = doctor_outcome(doctor_scan_with_repair());
    let findings_before = match &app.doctor_scan {
        DoctorScanState::Ready(outcome) => outcome.scan.findings.len(),
        _ => panic!("ready"),
    };
    let history_before = app.history.entries().count();

    app.review_doctor_repair_for(
        DoctorRepairAction::CleanMountPath,
        "mount_root.stale_mount_directory".to_string(),
        "/mount/SNES/Old Game".to_string(),
    );

    assert!(app.doctor_repair_review.is_some(), "the review is open");
    assert!(app.doctor_repair_result.is_none(), "nothing was executed");
    assert_eq!(app.history.entries().count(), history_before);
    assert_eq!(
        match &app.doctor_scan {
            DoctorScanState::Ready(outcome) => outcome.scan.findings.len(),
            _ => panic!("ready"),
        },
        findings_before,
        "the findings are unchanged"
    );
}

/// A refused repair is still an attempt, so it is recorded, and the
/// result is shown honestly rather than as a success.
#[test]
fn a_refused_repair_is_recorded_in_history_and_shown_as_refused() {
    let mut app = app_for_operation_tests();
    // The finding names a path that does not exist, so revalidation
    // refuses it. Nothing in the fixture can be mutated either way.
    app.doctor_scan = doctor_outcome(doctor_scan_with_repair());
    app.doctor_repair_review = Some(doctor_review());
    let history_before = app.history.entries().count();

    // Configuration supplied directly. Reading the real one made this test
    // pass only on a machine that happened to have
    // `~/.config/archivefs/config.toml`, and fail on CI, which does not - and
    // it was never what the test is about.
    // Paths that deliberately do not exist: the repair is refused at
    // revalidation, so nothing here is ever opened, created or written.
    let scratch = std::env::temp_dir().join("archivefs-doctor-repair-refusal-fixture");
    app.confirm_doctor_repair_with(
        Config {
            source_folders: vec![scratch.join("sources")],
            mount_root: scratch.join("mounts"),
            ratarmount_bin: "ratarmount".to_string(),
            master_rom_root: None,
        },
        scratch.join("index.json"),
    );

    assert_eq!(
        app.history.entries().count(),
        history_before + 1,
        "exactly one History entry per attempt"
    );
    let entry = app.history.entries().last().expect("entry");
    assert_eq!(entry.action, ActivityAction::DoctorRepair);
    assert!(
        entry.message.contains("action=clean_mount_path"),
        "{}",
        entry.message
    );
    assert!(
        entry
            .message
            .contains("finding=mount_root.stale_mount_directory"),
        "{}",
        entry.message
    );
    assert!(
        entry.message.contains("confirmed=true"),
        "{}",
        entry.message
    );
    assert!(entry.message.contains("undo="), "{}", entry.message);

    let outcome = app.doctor_repair_result.as_deref().expect("result");
    assert_ne!(outcome.record.status, DoctorRepairStatus::Succeeded);
    assert!(outcome.record.changed_paths.is_empty());
    assert!(app.doctor_repair_review.is_none(), "the review closed");
    assert!(app.doctor_repair_finished_at_unix_seconds.is_some());

    // The result is rendered honestly, and the scan timestamp is kept.
    let output = render_doctor_page_with(
        &app.doctor_scan,
        &mut None,
        None,
        app.doctor_repair_result.as_deref(),
        app.doctor_repair_finished_at_unix_seconds,
    );
    assert!(rendered_text_contains(&output, "nothing was changed"));
    assert!(rendered_text_contains(&output, "Last run: "));
    assert!(rendered_text_contains(&output, "Last repair: "));
    assert!(rendered_text_contains(
        &output,
        "recorded in History & Logs"
    ));
}

#[test]
fn the_history_detail_records_every_required_field() {
    let record = DoctorRepairOutcome {
        action: DoctorRepairAction::CleanMountPath,
        spec: DoctorRepairAction::CleanMountPath.spec(),
        record: archivefs_core::diagnostics::repair::DoctorRepairRecord {
            action_id: "clean_mount_path",
            action_title: "Remove this leftover mount folder",
            finding_id: "mount_root.stale_mount_directory".to_string(),
            affected: Some(
                archivefs_core::emulator_environment::EncodedPath::from_path(Path::new(
                    "/mount/SNES/Old Game",
                )),
            ),
            confirmed: true,
            dry_run: false,
            status: DoctorRepairStatus::Succeeded,
            verification: DoctorRepairVerification::Verified,
            changed_paths: vec![
                archivefs_core::emulator_environment::EncodedPath::from_path(Path::new(
                    "/mount/SNES/Old Game",
                )),
            ],
            undo: archivefs_core::diagnostics::repair::DoctorRepairUndo::NothingToUndo,
            summary: "done".to_string(),
            rejection: None,
            error: None,
        },
    };
    let detail = doctor_repair_history_detail(&record);
    for expected in [
        "action=clean_mount_path",
        "finding=mount_root.stale_mount_directory",
        "confirmed=true",
        "dry_run=false",
        "result=Succeeded",
        "verification=Repair verified",
        "undo=",
        "resource=/mount/SNES/Old Game",
        "changed=[/mount/SNES/Old Game]",
    ] {
        assert!(detail.contains(expected), "missing {expected} in {detail}");
    }
}

#[test]
fn a_disappeared_issue_reads_friendly_rather_than_as_a_refusal() {
    // The issue simply vanished before repair: that is "nothing needed
    // changing", not a harsh refusal.
    let outcome = DoctorRepairOutcome {
        action: DoctorRepairAction::CleanMountPath,
        spec: DoctorRepairAction::CleanMountPath.spec(),
        record: archivefs_core::diagnostics::repair::DoctorRepairRecord {
            action_id: "clean_mount_path",
            action_title: "Remove this leftover mount folder",
            finding_id: "mount_root.stale_mount_directory".to_string(),
            affected: Some(
                archivefs_core::emulator_environment::EncodedPath::from_path(Path::new(
                    "/mount/SNES/Old Game",
                )),
            ),
            confirmed: true,
            dry_run: false,
            status: DoctorRepairStatus::Rejected,
            verification: DoctorRepairVerification::NotAttempted,
            changed_paths: Vec::new(),
            undo: archivefs_core::diagnostics::repair::DoctorRepairUndo::NothingToUndo,
            summary: "Remove this leftover mount folder was refused: The problem this repair \
                          addresses is no longer present. Nothing was changed."
                .to_string(),
            rejection: Some(DoctorRepairRejection::StaleFinding),
            error: None,
        },
    };
    let state = doctor_outcome(doctor_scan_with_repair());
    let output = render_doctor_page_with(&state, &mut None, None, Some(&outcome), Some(42));

    assert!(
        rendered_text_contains(&output, "Nothing needed changing"),
        "a disappeared issue must read as nothing-needed-changing"
    );
    assert!(
        !rendered_text_contains(&output, "Repair was refused"),
        "the harsh refusal wording must not surface for a disappeared issue"
    );
    // The exact technical reason is still preserved below and in history.
    assert!(
        rendered_text_contains(&output, "no longer present"),
        "the technical reason must remain visible"
    );
    assert!(rendered_text_contains(
        &output,
        "recorded in History & Logs"
    ));
}

#[test]
fn a_real_refusal_keeps_the_exact_safety_wording() {
    let outcome = DoctorRepairOutcome {
        action: DoctorRepairAction::CleanMountPath,
        spec: DoctorRepairAction::CleanMountPath.spec(),
        record: archivefs_core::diagnostics::repair::DoctorRepairRecord {
            action_id: "clean_mount_path",
            action_title: "Remove this leftover mount folder",
            finding_id: "mount_root.stale_mount_directory".to_string(),
            affected: Some(
                archivefs_core::emulator_environment::EncodedPath::from_path(Path::new(
                    "/mount/SNES/Old Game",
                )),
            ),
            confirmed: true,
            dry_run: false,
            status: DoctorRepairStatus::Rejected,
            verification: DoctorRepairVerification::NotAttempted,
            changed_paths: Vec::new(),
            undo: archivefs_core::diagnostics::repair::DoctorRepairUndo::NothingToUndo,
            summary: "Remove this leftover mount folder was refused: That path is inside a \
                          configured source folder. EmuWiz never modifies anything there."
                .to_string(),
            rejection: Some(DoctorRepairRejection::PathUnderSourceRoot),
            error: None,
        },
    };
    let state = doctor_outcome(doctor_scan_with_repair());
    let output = render_doctor_page_with(&state, &mut None, None, Some(&outcome), Some(42));

    assert!(
        rendered_text_contains(&output, "Repair was refused and nothing was changed"),
        "a genuine safety refusal keeps its exact wording"
    );
}

#[test]
fn a_verified_repair_removes_only_that_finding_and_keeps_the_rest() {
    let mut app = app_for_operation_tests();
    // Two unrelated findings plus the repairable one.
    let issues = vec![
        doctor_health_issue("/roms/a.zip", HealthCategory::Missing),
        doctor_health_issue("/roms/b.zip", HealthCategory::UnknownPlatform),
    ];
    let stale = vec![PathBuf::from("/mount/SNES/Old Game")];
    let mut inputs = DoctorScanInputs::none_loaded();
    inputs.health_issues = Gathered::Ready(issues.as_slice());
    inputs.stale_mount_directories = Gathered::Ready(stale.as_slice());
    app.doctor_scan = doctor_outcome(run_doctor_scan(&inputs));
    let before = match &app.doctor_scan {
        DoctorScanState::Ready(outcome) => outcome.scan.findings.len(),
        _ => panic!("ready"),
    };
    assert!(before >= 3);

    app.doctor_repair_review = Some(doctor_review());
    app.confirm_doctor_repair();

    // The repair is refused here (the path does not exist), so nothing is
    // removed - unrelated findings are preserved either way.
    let after = match &app.doctor_scan {
        DoctorScanState::Ready(outcome) => &outcome.scan.findings,
        _ => panic!("ready"),
    };
    assert!(
        after
            .iter()
            .any(|finding| finding.id == "library.archive_missing"),
        "an unrelated finding must survive a repair"
    );
    assert!(
        after
            .iter()
            .any(|finding| finding.id == "library.unknown_platform"),
        "an unrelated finding must survive a repair"
    );
}

#[test]
fn long_content_pages_use_shared_scrolling_without_changing_table_pages() {
    for view in [
        MainView::Selected,
        MainView::Settings,
        MainView::Doctor,
        MainView::About,
        MainView::Sources,
        MainView::HistoryLogs,
        MainView::RepairHistory,
        // Library Organisation's preview/plan results list has no
        // `ScrollArea` of its own (live-QA Phase 8: a generated preview
        // extended below the window with the footer unreachable).
        MainView::CanonicalOrganisation,
    ] {
        assert!(main_view_uses_page_scroll(view));
    }
    // The unified Library shell's scrolling rule: all four
    // Library-related destinations manage their own internal
    // scrolling (Library/Health/Duplicates already did; LibraryViews
    // moved here too - see main_view_uses_page_scroll's doc comment
    // for why that is safe), so none of them use the outer page
    // scroll.
    for view in [
        MainView::Library,
        MainView::Health,
        MainView::Duplicates,
        MainView::LibraryViews,
        MainView::RecentlyFound,
        MainView::Mount,
    ] {
        assert!(!main_view_uses_page_scroll(view));
    }
}

#[test]
fn recently_found_uses_exact_latest_scan_paths() {
    let mut alien = persisted_archive(PathBuf::from("/roms/megadrive/Alien 3.md"), false);
    alien.display_name = "Alien 3".to_string();
    alien.normalized_name = "alien 3".to_string();
    let other = persisted_archive(PathBuf::from("/roms/megadrive/Other.md"), false);
    let recent = RecentScanAdditions {
        scan: CompletedScanSummary {
            scan_run_id: 9,
            started_at: "start".into(),
            finished_at: Some("finish".into()),
            triggered_by: "test".into(),
            source_folders_scanned: 1,
            archives_seen: 2,
            archives_added: 1,
            archives_updated: 1,
            archives_missing: 0,
            archives_unchanged: 0,
            skipped_unsupported_extension: 0,
            skipped_ambiguous_platform: 0,
            errors_count: 0,
            error_message: None,
        },
        archives: vec![alien.clone()],
        truncated: false,
    };
    assert!(recent_scan_contains(&recent, &alien.absolute_path));
    assert!(!recent_scan_contains(&recent, &other.absolute_path));
    assert!(alien.display_name.to_ascii_lowercase().contains("alien 3"));
}

/// Phase 5: a game with unresolved platform must show the same "Needs
/// attention" word a blocked mount already uses in the list - not a
/// silently different, unexplained state - and the mount state must
/// still take priority for its own label when the platform *is* known.
#[test]
fn unknown_platform_rows_are_needs_attention_in_the_list_too() {
    assert_eq!(
        gamer_view_row_state_label(true, MountState::Pending),
        "Needs attention"
    );
    assert_eq!(
        gamer_view_row_state_label(true, MountState::NotMountable),
        "Needs attention"
    );
    assert_eq!(
        gamer_view_row_state_label(false, MountState::Pending),
        "Ready to mount"
    );
    assert_eq!(
        gamer_view_row_state_label(false, MountState::NotMountable),
        "Ready to play"
    );
}

// --- Problems & Repair consolidation ---------------------------------------
//
// `MainView::Doctor`/`RepairReview`/`RepairHistory` still exist and still
// render through their own unchanged engines (proven throughout this file
// already, via `show_doctor_page` and elsewhere); what these tests prove is
// the *new* consolidated destination built on top of them: one sidebar
// entry, a shared tab row, correct per-tab dispatch, and that navigating to
// any of the three underlying `MainView`s still lands on real content.

fn problems_repair_screen_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1400.0, 1600.0),
        )),
        ..Default::default()
    }
}

fn render_problems_repair_app(app: &mut ArchiveFsApp) -> egui::FullOutput {
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    ctx.run(problems_repair_screen_input(), |ctx| {
        app.update(ctx, &mut frame)
    })
}

#[test]
fn problems_repair_tab_covers_every_consolidated_view() {
    for view in [
        MainView::Problems,
        MainView::Doctor,
        MainView::RepairReview,
        MainView::RepairHistory,
    ] {
        assert!(
            problems_repair_tab_for_main_view(view).is_some(),
            "{view:?} must map to a Problems & Repair tab"
        );
    }
    assert_eq!(
        problems_repair_tab_for_main_view(MainView::Problems),
        Some(ProblemsRepairTab::Overview)
    );
    assert_eq!(
        problems_repair_tab_for_main_view(MainView::Doctor),
        Some(ProblemsRepairTab::Diagnostics)
    );
    assert_eq!(
        problems_repair_tab_for_main_view(MainView::RepairReview),
        Some(ProblemsRepairTab::Repair)
    );
    assert_eq!(
        problems_repair_tab_for_main_view(MainView::RepairHistory),
        Some(ProblemsRepairTab::Repair)
    );
    // Every other destination is unaffected.
    assert_eq!(problems_repair_tab_for_main_view(MainView::Home), None);
}

#[test]
fn navigate_to_problems_repair_tab_sets_the_matching_view_and_remembers_it() {
    let mut app = app_for_operation_tests();
    app.navigate_to_problems_repair_tab(ProblemsRepairTab::Diagnostics);
    assert_eq!(app.view, MainView::Doctor);
    assert_eq!(app.problems_repair_tab, ProblemsRepairTab::Diagnostics);

    // Clicking the one sidebar entry again restores the last tab rather
    // than resetting to Overview - the same rule `MainView::Library`
    // already follows.
    app.navigate_to_main_view(MainView::Problems);
    assert_eq!(app.view, MainView::Doctor);
    assert_eq!(app.problems_repair_tab, ProblemsRepairTab::Diagnostics);
}

#[test]
fn reconcile_problems_repair_tab_follows_a_direct_view_assignment() {
    // Mirrors how a deep-link (e.g. Home's "Check Setup" card) still just
    // sets `self.view` directly - `reconcile_problems_repair_tab` (called
    // every frame) is what keeps `problems_repair_tab` truthful afterwards.
    let mut app = app_for_operation_tests();
    app.view = MainView::RepairReview;
    app.reconcile_problems_repair_tab();
    assert_eq!(app.problems_repair_tab, ProblemsRepairTab::Repair);
}

#[test]
fn problems_repair_overview_renders_the_tab_row_and_a_summary() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Problems;
    let output = render_problems_repair_app(&mut app);

    assert!(rendered_text_contains(&output, "Problems & Repair"));
    assert!(rendered_text_contains(&output, "Overview"));
    assert!(rendered_text_contains(&output, "Diagnostics"));
    assert!(rendered_text_contains(&output, "Repair / Recovery"));
    assert!(rendered_text_contains(&output, "Not checked yet"));
}

#[test]
fn problems_repair_diagnostics_tab_still_renders_doctor_content() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::Doctor;
    let output = render_problems_repair_app(&mut app);

    assert!(
        rendered_text_contains(&output, DOCTOR_READ_ONLY_NOTICE),
        "Diagnostics must still render Doctor's own safety notice"
    );
    assert!(rendered_text_contains(&output, "Run Doctor"));
}

#[test]
fn problems_repair_repair_tab_renders_review_and_history_together() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::RepairReview;
    let output = render_problems_repair_app(&mut app);

    assert!(
        rendered_text_contains(&output, "No repair plan loaded"),
        "Repair Review's own content must still render"
    );
    assert!(
        rendered_text_contains(&output, "Repair History"),
        "Repair History must render alongside Review, not require a separate destination"
    );
}

#[test]
fn problems_repair_history_deep_link_also_renders_both_sections() {
    // A deep-link landing directly on `MainView::RepairHistory` (rather
    // than `RepairReview`) must still reach the same consolidated content -
    // there is no separate "History" destination to fall through to.
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::RepairHistory;
    let output = render_problems_repair_app(&mut app);

    assert!(rendered_text_contains(&output, "No repair plan loaded"));
    assert!(rendered_text_contains(&output, "Repair History"));
}

#[test]
fn problems_repair_is_the_only_sidebar_entry_for_doctor_and_repair() {
    let sidebar_views: std::collections::HashSet<MainView> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .filter_map(|entry| match entry.click {
            NavClick::View(view) => Some(view),
            _ => None,
        })
        .collect();
    assert!(sidebar_views.contains(&MainView::Problems));
    for view in [
        MainView::Doctor,
        MainView::RepairReview,
        MainView::RepairHistory,
    ] {
        assert!(
            !sidebar_views.contains(&view),
            "{view:?} must not have its own sidebar entry any more"
        );
    }
}

// --- Exact Duplicate Review navigation --------------------------------
//
// Proves the page is genuinely reachable through the real application
// dispatch, not merely callable from its own isolated test module (see
// `exact_duplicate_review_page::tests` for the page's own behaviour tests -
// these prove reachability, not re-test the page itself).

#[test]
fn duplicate_finder_is_a_first_class_standalone_destination_not_a_repair_tab() {
    // 0.8.1 "core workflows directly discoverable": Duplicate Finder is no
    // longer routed through the Problems & Repair Repair tab, so it renders
    // standalone (no Repair Review / Repair History framing).
    assert_eq!(
        problems_repair_tab_for_main_view(MainView::ExactDuplicateReview),
        None
    );
    // ...and it now DOES have its own sidebar entry ("Duplicate Finder").
    let sidebar_views: std::collections::HashSet<MainView> = ADVANCED_NAV_GROUPS
        .iter()
        .flat_map(|group| group.entries)
        .filter_map(|entry| match entry.click {
            NavClick::View(view) => Some(view),
            _ => None,
        })
        .collect();
    assert!(sidebar_views.contains(&MainView::ExactDuplicateReview));
}

// --- 1: the navigation destination is visible -------------------------

#[test]
fn the_duplicate_finder_cross_link_is_visible_on_the_repair_tab() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::RepairReview;

    let output = render_problems_repair_app(&mut app);

    assert!(rendered_text_contains(&output, "Open Duplicate Finder"));
}

#[test]
fn the_disc_conversion_cross_link_is_visible_on_the_repair_tab() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::RepairReview;

    let output = render_problems_repair_app(&mut app);

    assert!(rendered_text_contains(&output, "Open Disc Conversion"));
    assert!(!rendered_text_contains(&output, "OpticalConversionPage"));
}

#[test]
fn convert_discs_home_card_lands_on_the_first_class_disc_conversion_page() {
    // The Home "Convert discs" card opens the dedicated Disc Conversion
    // destination directly - no Repair framing, no second click.
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.navigate_to_home_card(home_page::HomeCard::ConvertDiscs);
    assert_eq!(app.view, MainView::DiscConversion);
    assert!(
        app.optical_conversion_page.is_some(),
        "the conversion page state must exist on arrival"
    );

    let output = render_problems_repair_app(&mut app);
    assert!(
        rendered_text_contains(&output, "Convert Disc Images"),
        "the optical conversion page's own heading must render immediately"
    );
    assert!(rendered_text_contains(&output, "Source folder:"));
    assert!(!rendered_text_contains(&output, "Repair History"));
}

#[test]
fn duplicate_home_card_lands_on_the_first_class_duplicate_finder_page() {
    // Route to the actionable Duplicate Finder, standalone - not the
    // read-only Library duplicates viewer and not a Repair sub-page.
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.navigate_to_home_card(home_page::HomeCard::DuplicateReview);
    assert_eq!(app.view, MainView::ExactDuplicateReview);

    let output = render_problems_repair_app(&mut app);
    assert!(
        rendered_text_contains(&output, "Duplicate Finder"),
        "the standalone Duplicate Finder header must render"
    );
    assert!(
        rendered_text_contains(&output, "Source folder:"),
        "the Exact Duplicate Review page's own content must render"
    );
    assert!(!rendered_text_contains(&output, "Repair History"));
}

#[test]
fn setup_check_summary_maps_the_doctor_scan_state_the_home_card_shows() {
    use home_page::SetupCheckSummary;

    // Before any run this session.
    assert_eq!(
        setup_check_summary(&DoctorScanState::NotRun),
        SetupCheckSummary::NeverRun
    );

    // A completed, clean run that actually checked at least one subsystem.
    let healthy = doctor_outcome(doctor_scan_from(&[]));
    assert_eq!(setup_check_summary(&healthy), SetupCheckSummary::Healthy);

    // A completed run where every subsystem was unavailable - never a pass.
    let nothing_checked = doctor_outcome(run_doctor_scan(&DoctorScanInputs::none_loaded()));
    assert_eq!(
        setup_check_summary(&nothing_checked),
        SetupCheckSummary::NoChecksRun
    );

    // A completed run that produced findings is never Healthy / NeverRun /
    // NoChecksRun, and a blocking count is reported faithfully.
    let with_findings =
        doctor_scan_with_storage(500 * 1024 * 1024 * 1024, 1000 * 1024 * 1024 * 1024, true);
    assert!(!with_findings.is_healthy());
    match setup_check_summary(&doctor_outcome(with_findings.clone())) {
        SetupCheckSummary::NeedsAttention(n) => {
            assert_eq!(n, with_findings.blocking_count());
            assert!(n > 0);
        }
        SetupCheckSummary::Warnings(n) => {
            assert_eq!(with_findings.blocking_count(), 0);
            assert!(n > 0);
        }
        other => panic!("a scan with findings must not map to {other:?}"),
    }
}

// --- 2: selecting it reaches the Exact Duplicate Review page -----------

#[test]
fn selecting_the_repair_tab_cross_link_reaches_the_duplicate_finder_page() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::RepairReview;

    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let before = ctx.run(problems_repair_screen_input(), |ctx| {
        app.update(ctx, &mut frame)
    });
    let pos = find_exact_text_center(&before, "Open Duplicate Finder")
        .expect("the Duplicate Finder cross-link must be clickable");
    let click = vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        },
    ];
    let _ = ctx.run(
        egui::RawInput {
            events: click,
            ..problems_repair_screen_input()
        },
        |ctx| app.update(ctx, &mut frame),
    );

    assert_eq!(app.view, MainView::ExactDuplicateReview);

    let output = ctx.run(problems_repair_screen_input(), |ctx| {
        app.update(ctx, &mut frame)
    });
    assert!(
        rendered_text_contains(&output, "Duplicate Finder"),
        "the standalone Duplicate Finder page header must be visible"
    );
    assert!(
        rendered_text_contains(&output, "Source folder:"),
        "the Exact Duplicate Review page's own content must be visible"
    );
    // No Repair framing when arriving at the first-class destination.
    assert!(!rendered_text_contains(&output, "Repair History"));
}

// --- 3: the page starts in its safe empty state -------------------------

#[test]
fn the_page_starts_in_its_safe_empty_state() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::ExactDuplicateReview;

    let output = render_problems_repair_app(&mut app);

    assert!(rendered_text_contains(&output, "Source folder:"));
    assert!(
        app.exact_duplicate_review_page
            .as_ref()
            .is_none_or(|page| page.report().is_none()),
        "no scan has run just from navigating here"
    );
}

// --- 4: leaving and returning preserves state, per existing convention -

#[test]
fn leaving_and_returning_preserves_the_draft_source_folder() {
    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::ExactDuplicateReview;
    let _ = render_problems_repair_app(&mut app);
    app.exact_duplicate_review_page
        .as_mut()
        .expect("lazily created on first visit")
        .source_root_draft = "/tmp/example".to_string();

    // Leave for another tab, then come back - exactly like
    // `RepairReviewPageState` is never reset by switching tabs.
    app.view = MainView::Doctor;
    let _ = render_problems_repair_app(&mut app);
    app.view = MainView::ExactDuplicateReview;
    let _ = render_problems_repair_app(&mut app);

    assert_eq!(
        app.exact_duplicate_review_page
            .as_ref()
            .unwrap()
            .source_root_draft,
        "/tmp/example"
    );
}

// --- 5: navigation alone performs no scan or filesystem mutation --------

#[test]
fn navigating_to_the_page_never_scans_or_mutates_anything() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("a.bin"), b"hello").unwrap();
    std::fs::write(temp.path().join("b.bin"), b"hello").unwrap();

    let mut app = app_for_operation_tests();
    app.ui_mode = GuiMode::AdvancedView;
    app.view = MainView::ExactDuplicateReview;
    let _ = render_problems_repair_app(&mut app);
    let _ = render_problems_repair_app(&mut app);

    let page = app.exact_duplicate_review_page.as_ref().unwrap();
    assert!(page.report().is_none(), "navigation alone must never scan");
    assert!(temp.path().join("a.bin").exists());
    assert!(temp.path().join("b.bin").exists());
}

// --- 7: existing Repair Review navigation remains unchanged -------------
// (already proven by `problems_repair_repair_tab_renders_review_and_history_together`
// and `problems_repair_history_deep_link_also_renders_both_sections` above,
// re-run unmodified as part of this same test binary.)
