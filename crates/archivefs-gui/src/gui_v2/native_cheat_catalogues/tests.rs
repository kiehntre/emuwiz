use super::*;
use crate::DolphinCatalogueStatusSnapshot;
use crate::sources_page::{CatalogueManagerState, DolphinCatalogueManagerState};
use archivefs_core::patch_manager::CheatSourceList;

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "emuwiz-cheat-catalogues-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn listing(root: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = walk(root);
    names.sort();
    names
}

fn walk(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        out.push(entry.path().display().to_string());
        if entry.path().is_dir() {
            out.extend(walk(&entry.path()));
        }
    }
    out
}

fn workflows(context: &egui::Context, root: &std::path::Path) -> NativeWorkflows {
    let mut workflows = NativeWorkflows::new(context.clone());
    workflows.app = crate::tests::app_with_cheats_mods_context();
    // Both cards already loaded: opening the tab must start no scan of the
    // real machine and no network access.
    workflows.app.catalogue_bsfree_ui.catalogue_manager =
        CatalogueManagerState::Ready(CheatSourceList {
            schema_version: 1,
            entries: vec![],
        });
    workflows.app.catalogue_bsfree_ui.dolphin_catalogue_manager =
        DolphinCatalogueManagerState::Ready(Box::new(DolphinCatalogueStatusSnapshot {
            catalogue: None,
            last_check_unix_seconds: None,
        }));
    workflows.cheat_catalogues.cache_root_override = Some(root.to_path_buf());
    workflows
}

fn render(context: &egui::Context, workflows: &mut NativeWorkflows) -> String {
    let output = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            workflows.show_cheat_catalogues(ui);
        });
    });
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait(workflows: &mut NativeWorkflows) {
    for _ in 0..200 {
        if workflows.cheat_catalogues.poll() || !workflows.cheat_catalogues.busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("cache job did not finish");
}

#[test]
fn databases_tab_shows_both_gold_cards_without_network_or_writes() {
    let context = egui::Context::default();
    let root = temp_root("cards");
    let mut workflows = workflows(&context, &root);
    let text = render(&context, &mut workflows);
    assert!(text.contains("Cheat catalogues"), "{text}");
    assert!(text.contains("RetroArch cheat catalogue"), "{text}");
    assert!(text.contains("Dolphin"), "{text}");
    // No retrieval was started by merely opening the tab.
    assert!(
        workflows
            .app
            .catalogue_bsfree_ui
            .catalogue_retrieval
            .is_none()
    );
    assert!(
        workflows
            .app
            .catalogue_bsfree_ui
            .dolphin_catalogue_retrieval
            .is_none()
    );
    assert!(listing(&root).is_empty());
}

#[test]
fn cache_tab_on_empty_cache_is_read_only_and_truthful() {
    let context = egui::Context::default();
    let root = temp_root("empty");
    let mut workflows = workflows(&context, &root);
    workflows.cheat_catalogues.tab = SubTab::Cache;
    let text = render(&context, &mut workflows);
    assert!(text.contains("List saved copies"), "{text}");
    assert!(text.contains("Preview clean-up"), "{text}");
    for job in [CacheJob::Inventory, CacheJob::Verify] {
        workflows.cheat_catalogues.start(&context, job);
        wait(&mut workflows);
    }
    assert!(listing(&root).is_empty(), "inventory/verify must not write");
}

#[test]
fn missing_cache_root_reports_an_error_without_creating_it() {
    let context = egui::Context::default();
    let root = temp_root("missing").join("not-there");
    let mut workflows = workflows(&context, &root);
    workflows.cheat_catalogues.tab = SubTab::Cache;
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Inventory);
    wait(&mut workflows);
    assert!(!root.exists(), "inventory must never create the cache root");
    let text = render(&context, &mut workflows);
    assert!(
        text.contains("No saved copies to show") || text.contains("No saved copies yet"),
        "{text}"
    );
}

#[test]
fn prune_preview_on_empty_cache_offers_nothing_to_remove() {
    let context = egui::Context::default();
    let root = temp_root("prune");
    let mut workflows = workflows(&context, &root);
    workflows.cheat_catalogues.tab = SubTab::Cache;
    let policy = workflows.cheat_catalogues.policy();
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Plan(policy));
    wait(&mut workflows);
    let text = render(&context, &mut workflows);
    if workflows.cheat_catalogues.plan.is_some() {
        assert!(text.contains("Nothing to remove"), "{text}");
        assert!(!text.contains("Remove 0 items"));
    }
    assert!(listing(&root).is_empty());
}

#[test]
fn keep_count_never_goes_below_the_core_minimum() {
    let mut state = State {
        keep: 0,
        ..State::default()
    };
    assert_eq!(state.policy().keep_newest_per_source, Some(MINIMUM_KEEP));
    state.include_staging = true;
    assert!(state.policy().include_abandoned_staging);
}

#[test]
fn only_one_cache_job_runs_at_a_time() {
    let context = egui::Context::default();
    let root = temp_root("single");
    let mut state = State {
        cache_root_override: Some(root),
        ..State::default()
    };
    let (_sender, receiver) = mpsc::channel();
    state.worker = Some(receiver);
    state.start(&context, CacheJob::Verify);
    // The pretend job is still the only worker; a second start is ignored.
    assert!(state.busy());
}

#[test]
fn activity_job_is_queued_then_finished_with_failure_text() {
    let mut activity = Activity::default();
    let mut slot = None;
    track_job(
        &mut activity,
        &mut slot,
        true,
        "Work",
        ("done".into(), None),
    );
    let id = slot.expect("queued");
    assert_eq!(activity.running(), 1);
    track_job(
        &mut activity,
        &mut slot,
        false,
        "Work",
        ("failed".into(), Some("boom".into())),
    );
    assert!(slot.is_none());
    assert_eq!(activity.running(), 0);
    assert_eq!(activity.failed(), 1, "job {id} should be recorded failed");
}

#[test]
fn tab_is_reachable_and_the_host_adds_no_second_downloader() {
    let mods = include_str!("../mods.rs");
    assert!(mods.contains("(Tab::Catalogues, \"Cheat catalogues\")"));
    let host = include_str!("../native_cheat_catalogues.rs");
    let production = host.split("#[cfg(test)]").next().unwrap();
    for forbidden in [
        "HttpsCheatSourceTransport",
        "fetch_retroarch_cheat_source",
        "fetch_dolphin_catalogue",
        "std::fs::remove",
        "Command::new",
    ] {
        assert!(
            !production.contains(forbidden),
            "host must reuse Core/Gold: {forbidden}"
        );
    }
}

#[test]
fn prune_result_clears_stale_review_and_reports_honestly() {
    let mut state = State {
        plan: Some(
            plan_retroarch_cheat_cache_prune(&temp_root("stale"), &CachePrunePolicy::default())
                .unwrap(),
        ),
        ..State::default()
    };
    state.apply(CacheOutcome::Prune(Ok(CachePruneExecutionResult {
        schema_version: 1,
        status: CachePruneExecutionStatus::PartialFailure,
        confirmed: true,
        entries: vec![],
        bytes_reclaimed: 2048,
        snapshots_deleted: 1,
        staging_entries_removed: 0,
    })));
    assert!(state.plan.is_none(), "a used preview must not be reusable");
    let (ok, message) = state.notice.unwrap();
    assert!(!ok, "partial failure must not be shown as success");
    assert!(
        message.contains("problems") && message.contains("2.0 KB"),
        "{message}"
    );
}
