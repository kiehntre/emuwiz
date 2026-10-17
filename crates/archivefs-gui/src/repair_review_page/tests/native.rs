use super::*;

fn settle_input(page: &mut RepairReviewPageState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while page.input_busy() {
        page.poll_input();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn native_plan_load_is_preview_only_and_reloading_invalidates_confirmation() {
    let dir = TestDir::new("native-load");
    let path = dir.path().join("plan.json");
    let plan = fixture_plan();
    let bytes = serde_json::to_vec(&plan).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let mut page = RepairReviewPageState::default();
    page.background_ui = true;
    page.load_plan(path.clone());
    assert!(page.input_busy());
    assert!(page.plan.is_none());
    settle_input(&mut page);
    assert!(page.plan.is_some());
    assert!(!page.is_apply_running());
    assert!(page.apply_result.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    page.select_all(&build_rows(page.plan.as_ref().unwrap(), None));
    page.open_apply_confirmation();
    assert!(page.apply_confirm.is_some());
    page.load_plan(path.clone());
    assert!(page.apply_confirm.is_none());
    settle_input(&mut page);
    assert!(page.selected.is_empty());
    std::fs::write(&path, b"not a plan").unwrap();
    page.load_plan(path);
    settle_input(&mut page);
    assert!(page.error.as_ref().unwrap().contains("parse"));
    assert!(
        page.plan.is_some(),
        "old preview stays labeled as previous evidence"
    );
    assert!(page.apply_result.is_none());
}

#[test]
fn native_apply_disconnect_is_unknown_and_invalidates_preview() {
    let (sender, messages) = std::sync::mpsc::channel();
    drop(sender);
    let mut page = RepairReviewPageState::default();
    page.background_ui = true;
    page.plan = Some(fixture_plan());
    page.apply_job = Some(RepairApplyJob { messages });
    page.apply_running = true;
    assert!(page.poll_apply());
    assert!(page.plan_stale);
    assert!(!page.can_apply());
    assert!(page.apply_result.is_none());
    assert_eq!(
        page.apply_failure.as_ref().unwrap().label,
        "Repair outcome unknown"
    );
    assert!(
        page.apply_failure
            .as_ref()
            .unwrap()
            .detail
            .contains("Repair History")
    );
}
