use super::*;
use crate::DolphinCatalogueStatusSnapshot;
use crate::sources_page::{CatalogueManagerState, DolphinCatalogueManagerState};
use archivefs_core::patch_manager::CheatSourceList;
use archivefs_core::patch_manager::{
    CheatSourceFetchOptions, CheatSourceHttpResponse, CheatSourceTransferContext,
    CheatSourceTransport, fetch_retroarch_cheat_source,
};
use std::io::Write;

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
fn databases_tab_shows_both_gold_cards_and_starts_no_retrieval() {
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

/// Serves a synthetic libretro archive; the "commit" decides the snapshot
/// identity, so distinct commits create distinct saved copies. No network.
struct SyntheticTransport {
    commit: char,
}

impl CheatSourceTransport for SyntheticTransport {
    fn get(
        &self,
        url: &str,
        _maximum_bytes: u64,
        destination: &mut dyn Write,
        _context: CheatSourceTransferContext<'_>,
    ) -> Result<CheatSourceHttpResponse, CheatSourceError> {
        let (body, kind) =
            if url.contains("api.github.com/repos/libretro/libretro-database/commits/") {
                (
                    format!(r#"{{"sha":"{}"}}"#, self.commit.to_string().repeat(40)).into_bytes(),
                    "application/json",
                )
            } else {
                let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
                writer
                    .start_file(
                        "libretro-database-1.22.1/cht/Nintendo - SNES/Game.cht",
                        zip::write::SimpleFileOptions::default(),
                    )
                    .unwrap();
                writer
                    .write_all(
                        format!(
                            "cheats = 1\ncheat0_desc = \"Lives {}\"\ncheat0_code = \"00FF\"\n",
                            self.commit
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                (writer.finish().unwrap().into_inner(), "application/zip")
            };
        destination.write_all(&body).unwrap();
        Ok(CheatSourceHttpResponse {
            status: 200,
            content_type: Some(kind.into()),
            content_encoding: None,
            content_length: Some(body.len() as u64),
            location: None,
            etag: None,
            last_modified: None,
            downloaded_bytes: body.len() as u64,
            retry_after_seconds: None,
        })
    }
}

/// Creates `commits.len()` real saved copies in `root` through Core's own
/// fetch (synthetic transport), so the manifests and digests are genuine.
fn build_snapshots(root: &std::path::Path, commits: &[char]) {
    for commit in commits {
        fetch_retroarch_cheat_source(
            "libretro-buildbot-cheats",
            &CheatSourceFetchOptions {
                cache_root: root.to_path_buf(),
                force_refresh: true,
                offline: false,
                expected_sha256: None,
                max_download_bytes: Some(2 * 1024 * 1024),
                cancellation: None,
                progress: None,
            },
            &SyntheticTransport { commit: *commit },
        )
        .unwrap();
        // Distinct retrieval timestamps for deterministic age ordering.
        std::thread::sleep(std::time::Duration::from_millis(1100));
    }
}

/// Acceptance helper, not a regression test: populates the directory in
/// `EMUWIZ_FIXTURE_ROOT` with synthetic saved copies.
#[test]
#[ignore = "builds a synthetic cache for real-GUI acceptance"]
fn build_acceptance_cache() {
    let root = PathBuf::from(std::env::var("EMUWIZ_FIXTURE_ROOT").unwrap());
    build_snapshots(&root, &['1', '2', '3']);
}

#[test]
fn real_snapshots_inventory_verify_pin_and_confirmed_prune() {
    let context = egui::Context::default();
    let root = temp_root("real");
    build_snapshots(&root, &['1', '2', '3']);
    let mut workflows = workflows(&context, &root);
    workflows.cheat_catalogues.tab = SubTab::Cache;

    workflows.cheat_catalogues.start(&context, CacheJob::Verify);
    wait(&mut workflows);
    let report = workflows
        .cheat_catalogues
        .verification
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap();
    assert_eq!(report.entries.len(), 3);
    assert_eq!(report.invalid_count, 0);
    let oldest = report
        .entries
        .iter()
        .min_by_key(|entry| entry.retrieved_at_unix_seconds)
        .unwrap()
        .snapshot_id
        .clone()
        .unwrap();

    // Pin the oldest copy: preview must protect it, and pinning drops any
    // earlier preview so a stale plan cannot be confirmed.
    workflows.cheat_catalogues.keep = 2;
    let policy = workflows.cheat_catalogues.policy();
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Plan(policy));
    wait(&mut workflows);
    assert_eq!(
        workflows
            .cheat_catalogues
            .plan
            .as_ref()
            .unwrap()
            .candidate_count,
        1
    );
    workflows.cheat_catalogues.start(
        &context,
        CacheJob::Pin {
            snapshot_id: oldest.clone(),
            pinned: true,
        },
    );
    wait(&mut workflows);
    assert!(
        workflows.cheat_catalogues.plan.is_none(),
        "pin must invalidate the preview"
    );
    let policy = workflows.cheat_catalogues.policy();
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Plan(policy));
    wait(&mut workflows);
    assert_eq!(
        workflows
            .cheat_catalogues
            .plan
            .as_ref()
            .unwrap()
            .candidate_count,
        0,
        "the pinned copy is protected"
    );

    // Unpin, preview, confirm: exactly the previewed copy goes, and it is
    // the only change on disk.
    workflows.cheat_catalogues.start(
        &context,
        CacheJob::Pin {
            snapshot_id: oldest.clone(),
            pinned: false,
        },
    );
    wait(&mut workflows);
    let policy = workflows.cheat_catalogues.policy();
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Plan(policy));
    wait(&mut workflows);
    let plan = workflows.cheat_catalogues.plan.clone().unwrap();
    assert_eq!(plan.candidate_count, 1);
    let before = listing(&root);
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Prune(Box::new(plan)));
    wait(&mut workflows);
    let (ok, message) = workflows.cheat_catalogues.notice.clone().unwrap();
    assert!(ok, "{message}");
    assert!(workflows.cheat_catalogues.plan.is_none());
    assert!(listing(&root).len() < before.len());
    workflows.cheat_catalogues.start(&context, CacheJob::Verify);
    wait(&mut workflows);
    let report = workflows
        .cheat_catalogues
        .verification
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap();
    assert_eq!(report.entries.len(), 2, "the two newest copies remain");
    assert_eq!(report.invalid_count, 0);
}

#[test]
fn a_plan_whose_copy_changed_after_preview_is_not_executed_blindly() {
    let context = egui::Context::default();
    let root = temp_root("changed");
    build_snapshots(&root, &['1', '2', '3']);
    let mut workflows = workflows(&context, &root);
    let policy = workflows.cheat_catalogues.policy();
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Plan(policy));
    wait(&mut workflows);
    let plan = workflows.cheat_catalogues.plan.clone().unwrap();
    assert_eq!(plan.candidate_count, 1);
    let candidate = plan
        .entries
        .iter()
        .find(|entry| entry.disposition == CachePruneDisposition::Candidate)
        .unwrap();
    // Someone adds a file to the reviewed copy between preview and confirm.
    let path = PathBuf::from(&candidate.path.display);
    std::fs::write(path.join("added-after-preview.txt"), b"x").unwrap();
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Prune(Box::new(plan)));
    wait(&mut workflows);
    assert!(path.exists(), "a changed copy must be left in place");
    let (ok, message) = workflows.cheat_catalogues.notice.clone().unwrap();
    assert!(
        !ok,
        "a skipped/changed entry must not be reported as success: {message}"
    );
}

#[test]
fn stale_reads_are_dropped_after_a_pin_or_a_finished_download() {
    let context = egui::Context::default();
    let root = temp_root("stale-reads");
    build_snapshots(&root, &['1', '2']);
    let mut workflows = workflows(&context, &root);
    workflows.cheat_catalogues.start(&context, CacheJob::Verify);
    wait(&mut workflows);
    let id = workflows
        .cheat_catalogues
        .verification
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .entries[0]
        .snapshot_id
        .clone()
        .unwrap();
    workflows.cheat_catalogues.start(
        &context,
        CacheJob::Pin {
            snapshot_id: id,
            pinned: true,
        },
    );
    wait(&mut workflows);
    // The shown entry updates in place (no blank frame that would drop
    // controller focus) and a re-read is queued.
    let shown = workflows
        .cheat_catalogues
        .verification
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap();
    assert!(
        shown.entries.iter().any(|entry| entry.pinned),
        "pin state must update at once"
    );
    assert!(workflows.cheat_catalogues.refresh_pending);
    // The next frame re-reads the list instead of leaving it blank.
    workflows.poll_cheat_catalogues(&context);
    wait(&mut workflows);
    let report = workflows
        .cheat_catalogues
        .inventory
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap();
    assert!(report.entries.iter().any(|entry| entry.pinned));
    // A new request clears the previous result text.
    workflows
        .cheat_catalogues
        .start(&context, CacheJob::Inventory);
    assert!(workflows.cheat_catalogues.notice.is_none());
    wait(&mut workflows);
    // A finished download invalidates every earlier read and preview.
    workflows.cheat_catalogues.plan =
        Some(plan_retroarch_cheat_cache_prune(&root, &CachePrunePolicy::default()).unwrap());
    workflows.cheat_catalogues.retrieving_before = true;
    workflows.poll_cheat_catalogues(&context);
    assert!(workflows.cheat_catalogues.plan.is_none());
    assert!(workflows.cheat_catalogues.inventory.is_none());
}

#[test]
fn counts_are_worded_correctly() {
    assert_eq!(plural(1, "item", "items"), "item");
    assert_eq!(plural(0, "item", "items"), "items");
    assert_eq!(plural(2, "copy", "copies"), "copies");
}

#[test]
fn action_button_keeps_focus_while_busy_and_ignores_clicks_until_idle() {
    let context = egui::Context::default();
    let id = egui::Id::new("action-button-focus");
    let frame = |idle: bool, focus: bool, press: bool| {
        let mut input = egui::RawInput::default();
        if press {
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut result = (false, false);
        let _ = context.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.push_id(id, |ui| {
                    let response = ui.add(egui::Button::new("Protect from clean-up"));
                    if focus {
                        response.request_focus();
                    }
                    result = (action_button_from(&response, idle), response.has_focus());
                });
            });
        });
        result
    };
    frame(true, true, false);
    // A job is running: nothing fires, yet the button keeps keyboard focus.
    let (fired, focused) = frame(false, false, true);
    assert!(!fired, "a busy button must not start a second job");
    assert!(focused, "focus must survive the frames while a job runs");
    // Job done: the same focused button activates with Return.
    let (fired, focused) = frame(true, false, true);
    assert!(fired && focused, "Return must activate once idle again");
}
