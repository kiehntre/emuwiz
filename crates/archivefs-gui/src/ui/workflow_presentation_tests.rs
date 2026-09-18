use super::*;

fn visible_text(output: &egui::FullOutput) -> String {
    fn visit(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(value) => {
                text.push_str(value.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in &output.shapes {
        visit(&shape.shape, &mut text);
    }
    text
}

#[test]
fn stage2_header_picker_and_advanced_details_render_without_side_effects() {
    let context = egui::Context::default();
    let fixture = tempfile::tempdir().unwrap();
    let mut draft = fixture.path().display().to_string();
    let before = draft.clone();
    let mut advanced_rendered = false;
    let output = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            workflow_header(
                ui,
                "BIOS / Firmware",
                "Prepare firmware required by your emulators.",
            );
            folder_picker(ui, "BIOS folder", &mut draft);
            let _ = action_button(ui, "Inspect BIOS Folder", ActionStyle::Primary, true);
            technical_details(ui, "stage2-collapsed", |ui| {
                advanced_rendered = true;
                ui.label("raw backend details");
            });
        });
    });
    let text = visible_text(&output);
    for label in [
        "BIOS / Firmware",
        "Prepare firmware",
        "Browse…",
        "Inspect BIOS Folder",
        "Technical details",
    ] {
        assert!(text.contains(label), "missing {label}: {text}");
    }
    assert!(!advanced_rendered);
    assert!(!text.contains("raw backend details"));
    assert_eq!(draft, before);
    assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 0);
}

#[test]
fn stage2_cards_have_equal_width_height_and_aligned_controls() {
    let context = egui::Context::default();
    let mut cards = Vec::new();
    let mut buttons = Vec::new();
    let _ = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                for description in [
                    "Short",
                    "A longer description that wraps onto another line in the same width",
                ] {
                    ui.allocate_ui_with_layout(
                        egui::vec2(260.0, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            aligned_card(ui, 260.0, 146.0, |ui| {
                                let top = ui.cursor().top();
                                ui.label(description);
                                ui.add_space((top + 112.0 - ui.cursor().top()).max(0.0));
                                buttons.push(ui.button("Choose").rect);
                                cards.push(ui.min_rect());
                            });
                        },
                    );
                }
            });
        });
    });
    assert!((cards[0].width() - cards[1].width()).abs() < 1.0);
    assert!((cards[0].height() - cards[1].height()).abs() < 1.0);
    assert!((cards[0].top() - cards[1].top()).abs() < 1.0);
    assert!((buttons[0].top() - buttons[1].top()).abs() < 1.0);
}

#[test]
fn stage2_bios_landing_is_safe_without_inspection_or_apply() {
    let context = egui::Context::default();
    let mut state = crate::bios_projection_page::BiosProjectionPageState::default();
    let output = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
    });
    let text = visible_text(&output);
    assert_eq!(text.matches("Browse…").count(), 2);
    assert!(text.contains("Inspect BIOS Folder"));
    assert!(!state.take_doctor_refresh_request());
}

#[test]
fn stage2_activity_hides_raw_ids_and_keeps_history() {
    use crate::activity_history::*;
    let context = egui::Context::default();
    let mut history = OperationHistory::default();
    let message = "action=repair.x finding=bios.missing internal journal data";
    history.record(HistoryEntry::new(
        ActivityAction::DoctorRepair,
        None,
        ActivityOutcome::Failed,
        message,
    ));
    let mut expanded = false;
    let mut app = crate::tests::app_for_operation_tests();
    let output = context.run(Default::default(), |ctx| {
        let _ = show_activity_panel(ctx, &mut history, &mut expanded, &mut app.clipboard);
    });
    let text = visible_text(&output);
    assert!(!text.contains("action="));
    assert!(!text.contains("finding="));
    assert_eq!(history.entries().next().unwrap().message, message);
    expanded = true;
    let output = context.run(Default::default(), |ctx| {
        let _ = show_activity_panel(ctx, &mut history, &mut expanded, &mut app.clipboard);
    });
    assert!(visible_text(&output).contains(message));
}

#[test]
fn stage2_dat_workflow_has_distinct_verify_and_import_without_writes() {
    use crate::dat_sources_page::*;
    let fixture = tempfile::tempdir().unwrap();
    let config = fixture.path().join("sources.toml");
    let state = DatSourcesPageState::load_with_transaction_dir(
        config.clone(),
        vec![],
        archivefs_core::safe_read::TrustedRoots::none(),
        fixture.path().join("journal"),
    );
    let view = state.view();
    let mut ui_state = DatSourcesPageUi::default();
    let context = egui::Context::default();
    let output = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(show_dat_sources_page(ui, &view, &mut ui_state).is_none());
        });
    });
    let text = visible_text(&output);
    for label in [
        "DATs & Verification",
        "Check your games",
        "Verify Games…",
        "Import DAT…",
        "Exact",
        "Probable",
        "Ambiguous",
        "No match",
    ] {
        assert!(text.contains(label), "missing {label}");
    }
    assert!(!config.exists());
    assert!(!ui_state.open_catalogue_picker);
}

#[test]
fn stage2_saves_landing_exposes_card_chooser_without_inspecting() {
    use crate::pcsx2_page::*;
    let context = egui::Context::default();
    let mut vault = Pcsx2SaveVaultState::default();
    let output = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(
                show_save_vault_landing(ui, false, None, &Pcsx2StatusState::Idle, &mut vault)
                    .is_none()
            );
        });
    });
    let text = visible_text(&output);
    assert!(text.contains("PS2 Save Vault"));
    assert!(text.contains("Choose another memory-card image"));
    assert!(!text.contains("Filesystem: PS2"));
    assert!(vault.manual_loading.is_none());
}
