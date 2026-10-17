use super::*;
use archivefs_core::dat::rom_organisation::*;
use std::{collections::BTreeSet, path::PathBuf};
fn selection(count: usize) -> Selection {
    let entries = (0..count)
        .map(|n| OrganisationPlanEntry {
            source_path: PathBuf::from(format!("/collection/{n}.bin")),
            destination_path: PathBuf::from(format!("/collection/new{n}.bin")),
            platform: None,
            platform_display_name: String::new(),
            platform_source: String::new(),
            slug: None,
            layout_folder: None,
            mode: OrganisationMode::RenameInPlace,
            content_classification: None,
            original_metadata: Default::default(),
            status: OrganisationStatus::Suggested,
            reason: None,
        })
        .collect::<Vec<_>>();
    let approved = entries
        .iter()
        .map(|e| e.source_path.to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    Selection {
        plan: OrganisationPlan {
            master_root: "/collection".into(),
            mode: OrganisationMode::RenameInPlace,
            content_policy: Default::default(),
            classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.into(),
            generation: 1,
            entries,
        },
        approved,
    }
}
#[test]
fn r2_selection_refuses_zero_large_nested_and_link_batches() {
    assert!(selection(0).pairs().is_err());
    assert!(selection(9).pairs().is_err());
    let mut s = selection(1);
    s.plan.entries[0].destination_path = "/other/b.bin".into();
    assert!(s.pairs().is_err());
    s = selection(1);
    s.plan.mode = OrganisationMode::BuildLinkedLibrary;
    assert!(s.pairs().is_err());
}
#[test]
fn r2_confirmation_binding_changes_with_plan_and_approval_selection() {
    let a = selection(2);
    let mut b = a.clone();
    b.plan.entries[0].destination_path = "/collection/changed.bin".into();
    assert_ne!(a.fingerprint(), b.fingerprint());
    b = a.clone();
    b.approved.clear();
    assert_ne!(a.fingerprint(), b.fingerprint());
}
#[test]
fn r2_pair_projection_preserves_exact_selected_names() {
    let s = selection(2);
    let (root, pairs) = s.pairs().unwrap();
    assert_eq!(root, PathBuf::from("/collection"));
    assert_eq!(
        pairs,
        vec![
            ("0.bin".into(), "new0.bin".into()),
            ("1.bin".into(), "new1.bin".into())
        ]
    );
}

fn preview() -> Preview {
    Preview {
        selection: selection(1),
        plan: serde_json::json!({}),
        confirmation: "digest".into(),
        file: "/unused".into(),
    }
}
#[test]
fn unavailable_selection_invalidates_every_confirmation() {
    let mut s = State::default();
    s.observe_selection(&Ok(selection(1)));
    s.preview = Some(preview());
    s.review = Some(Review {
        action: "apply".into(),
        value: serde_json::json!({}),
        selection: Some(selection(1)),
    });
    s.setup_confirm = true;
    s.preview_confirm = true;
    s.action_confirm = true;
    s.reuse_confirm = true;
    s.observe_selection(&Err("Choose at most eight files".into()));
    assert!(s.preview.is_none() && s.review.is_none());
    assert!(!s.setup_confirm && !s.preview_confirm && !s.action_confirm && !s.reuse_confirm);
}
#[test]
fn worker_cannot_restore_a_stale_preview() {
    let mut s = State::default();
    s.observe_selection(&Ok(selection(1)));
    let (tx, rx) = mpsc::channel();
    s.job = Some(Job {
        receiver: rx,
        selection_tag: s.selection_tag.clone(),
    });
    s.observe_selection(&Ok(selection(2)));
    tx.send(Ok(ResultValue::Preview(preview()))).ok();
    s.poll();
    assert!(s.preview.is_none());
    assert!(s.message.contains("Selection changed"));
}
#[test]
fn unavailable_selection_rejects_inflight_enrollment_review_but_keeps_registration() {
    let mut s = State::default();
    s.observe_selection(&Ok(selection(1)));
    let (tx, rx) = mpsc::channel();
    s.job = Some(Job {
        receiver: rx,
        selection_tag: s.selection_tag.clone(),
    });
    s.observe_selection(&Err("Preview unavailable".into()));
    let i = Info {
        root: "/collection".into(),
        workspace: "/private".into(),
        state: "/private/state-0".into(),
        next_state: "/private/state-1".into(),
        signer: "/private/operator".into(),
        status: serde_json::json!({"state":"awaiting_apply","ownership":"owned"}),
    };
    tx.send(Ok(ResultValue::Review(
        Review {
            action: "apply".into(),
            value: serde_json::json!({}),
            selection: Some(selection(1)),
        },
        i,
    )))
    .ok();
    s.poll();
    assert!(s.info.is_some());
    assert!(s.review.is_none());
    assert!(!s.can_choose_standard());
}
#[test]
fn approved_ineligible_files_are_never_silently_omitted() {
    let mut s = selection(2);
    s.plan.entries[0].status = OrganisationStatus::Blocked;
    assert!(s.pairs().is_err());
}
#[test]
fn owner_copy_is_optional_truthful_and_measured() {
    assert_eq!(copy::STANDARD, "Standard Rename (recommended)");
    assert_eq!(copy::ADVANCED, "Advanced Protection");
    assert!(copy::COMPARISON.contains("original-file anchors"));
    assert!(copy::COMPARISON.contains("take longer"));
    for term in [
        "ext4",
        "4096",
        "one to eight",
        "direct regular",
        "RenameMove",
    ] {
        assert!(copy::SCOPE.contains(term));
    }
    for term in ["other programs", "complete authority-store rollback"] {
        assert!(copy::LIMITS.contains(term));
    }
    for term in [
        "Measured",
        "not guarantees",
        "two samples",
        "0.14",
        "0.88",
        "0.55",
        "4.00",
    ] {
        assert!(copy::BENCHMARKS.contains(term));
    }
    assert!(!copy::MR_WIZ.contains("unsafe"));
    assert!(!copy::MR_WIZ.contains("safer"));
}
#[test]
fn refused_status_never_labels_retained_projection_verified() {
    for state in ["review_required", "busy", "unknown"] {
        assert!(!view::verified_inventory(
            &serde_json::json!({"state":state})
        ));
    }
    assert!(view::verified_inventory(
        &serde_json::json!({"state":"settled"})
    ));
}

#[test]
fn optional_choice_renders_at_1024x600_and_tab_never_selects_advanced() {
    let ctx = egui::Context::default();
    let mut selected = false;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1024., 600.),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                choice(ui, &mut selected, false, true);
            });
        },
    );
    assert!(!selected);
    assert!(!output.shapes.is_empty());
}
#[test]
fn missing_operator_refuses_without_changing_engine_or_starting_work() {
    let t = tempfile::tempdir().unwrap();
    let helper = Helper::fixture(t.path().join("missing-cli"));
    let result = helper.run(vec!["blue-eligibility".into(), "/collection".into()]);
    assert!(result.unwrap_err().contains("will not be used instead"));
    assert!(std::fs::read_dir(t.path()).unwrap().next().is_none());
}
