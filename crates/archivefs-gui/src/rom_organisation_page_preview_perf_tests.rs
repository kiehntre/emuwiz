//! Bulk Quick Rename preview: the one-pass archive-id lookup gives exactly the
//! preview the per-file lookup gave, and the whole preview scales linearly.

use super::{
    NameResolver, RetroArchSaveLocations, build_candidates_in, build_candidates_per_file,
    bulk_guard,
};
use archivefs_core::Database;
use archivefs_core::dat::classification::ContentSelectionPolicy;
use archivefs_core::dat::rom_organisation::{
    OrganisationCandidate, OrganisationMode, OrganisationPlan, OrganisationPlanRequest,
    build_organisation_plan_reporting, prepare_organisation_catalogue,
    revalidate_organisation_plan,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Library {
    _temp: tempfile::TempDir,
    root: PathBuf,
    database: PathBuf,
    saves: PathBuf,
    sources: Vec<PathBuf>,
}

/// `count` NES games in one folder (every 100th with a save beside it), plus
/// an arcade game, a disc named by a cue sheet, a file that is not in the
/// library, and (with `duplicate`) a path two archive rows share.
fn library(count: usize, duplicate: bool) -> Library {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("games");
    let nes = root.join("nes");
    let saves = temp.path().join("saves");
    for dir in [
        &nes,
        &root.join("arcade"),
        &root.join("psx"),
        &saves.join("core"),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let path = temp.path().join("library.sqlite3");
    let mut db = Database::open_or_create(&path).unwrap();
    let mut roots = vec![root.clone()];
    if duplicate {
        roots.push(root.join("psx"));
    }
    db.register_source_folders(&roots).unwrap();
    let folders = db.list_source_folders().unwrap();
    let source_id = |root: &Path| folders.iter().find(|f| f.path == root).unwrap().id;
    let main = source_id(&root);
    let mut sources = Vec::with_capacity(count + 4);
    let mut add = |db: &mut Database, file: PathBuf, platform: &str| {
        std::fs::write(&file, format!("synthetic {}", file.display())).unwrap();
        let archive = archivefs_core::Archive::from_path_in_root(&file, &root).unwrap();
        let id = db.upsert_archive(main, &root, &archive).unwrap().archive_id;
        db.set_manual_platform(id, platform).unwrap();
        sources.push(file);
    };
    for index in 0..count {
        let game = nes.join(format!("Game {index:05}.nes"));
        if index % 100 == 0 {
            std::fs::write(nes.join(format!("Game {index:05}.srm")), b"save").unwrap();
        }
        add(&mut db, game, "nes");
    }
    add(&mut db, root.join("arcade").join("galaga.zip"), "arcade");
    add(&mut db, root.join("psx").join("quest.iso"), "psx");
    std::fs::write(
        root.join("psx").join("quest.cue"),
        "FILE \"quest.iso\" BINARY\n",
    )
    .unwrap();
    let outside = nes.join("Not Scanned.nes");
    std::fs::write(&outside, b"x").unwrap();
    sources.push(outside);
    if duplicate {
        // The same file as a row of the nested `psx` source too: two rows,
        // one absolute path (an inconsistent library both lookups report).
        let psx = root.join("psx");
        let file = psx.join("quest.iso");
        let archive = archivefs_core::Archive::from_path_in_root(&file, &psx).unwrap();
        db.upsert_archive(source_id(&psx), &psx, &archive).unwrap();
    }
    drop(db);
    Library {
        _temp: temp,
        root,
        database: path,
        saves,
        sources,
    }
}

fn resolver(lib: &Library) -> NameResolver {
    NameResolver::for_test(Some(Database::open_read_only(&lib.database).unwrap()), None)
}

/// What a RomM title would propose: a new name for every file.
fn with_new_names(mut candidates: Vec<OrganisationCandidate>) -> Vec<OrganisationCandidate> {
    for candidate in &mut candidates {
        let stem = candidate
            .source_path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let ext = candidate
            .source_path
            .extension()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        candidate.canonical_name = Some(format!("{stem} (USA).{ext}"));
    }
    candidates
}

fn plan(lib: &Library, candidates: &[OrganisationCandidate]) -> OrganisationPlan {
    let mut sink = |_| {};
    let mut progress = archivefs_core::job_progress::ProgressReporter::new(&mut sink);
    build_organisation_plan_reporting(
        &OrganisationPlanRequest {
            master_root: &lib.root,
            mode: OrganisationMode::RenameInPlace,
            content_policy: ContentSelectionPolicy::AllEntries,
            candidates,
            generation: 1,
            unknown_folder: None,
            link_target_roots: None,
        },
        None,
        &mut progress,
    )
    .unwrap()
}

fn locations(lib: &Library) -> RetroArchSaveLocations {
    RetroArchSaveLocations {
        directories: vec![lib.saves.clone()],
        ..Default::default()
    }
}

#[test]
fn the_one_pass_lookup_gives_the_same_preview_as_one_lookup_per_file() {
    let lib = library(60, true);
    let db = Database::open_read_only(&lib.database).unwrap();
    let names = resolver(&lib);
    let before = build_candidates_per_file(&db, &lib.sources, 1, &names);
    let after = build_candidates_in(&db, &lib.sources, 1, &names);
    assert_eq!(after, before, "candidates and name choices");

    // The cases that matter are present and agree.
    let duplicate = lib.root.join("psx").join("quest.iso");
    assert!(
        after.1[&duplicate]
            .note
            .as_deref()
            .unwrap_or("")
            .contains("more than one archive row"),
        "{:?}",
        after.1[&duplicate]
    );
    let missing = lib.root.join("nes").join("Not Scanned.nes");
    assert!(matches!(
        after
            .0
            .iter()
            .find(|c| c.source_path == missing)
            .unwrap()
            .resolution,
        archivefs_core::platform::identity::PlatformIdentityResolution::Unknown { .. }
    ));

    // Same order, statuses, destinations and reasons, before and after the guard.
    let mut old_plan = plan(&lib, &with_new_names(before.0));
    let mut new_plan = plan(&lib, &with_new_names(after.0));
    assert_eq!(new_plan, old_plan);
    let order: Vec<_> = new_plan
        .entries
        .iter()
        .map(|e| e.source_path.clone())
        .collect();
    assert_eq!(
        order,
        old_plan
            .entries
            .iter()
            .map(|e| e.source_path.clone())
            .collect::<Vec<_>>()
    );
    let old_blocked = bulk_guard::guard_bulk_rename_plan(&db, &mut old_plan, &locations(&lib));
    let new_blocked = bulk_guard::guard_bulk_rename_plan(&db, &mut new_plan, &locations(&lib));
    assert_eq!(new_plan, old_plan);
    assert_eq!(new_blocked, old_blocked);
    // Arcade, the cue-named disc and the save holders (0) are blocked.
    assert!(new_blocked >= 2, "{new_blocked}");
}

#[test]
fn choosing_names_at_apply_uses_the_same_answers_without_the_one_pass_lookup() {
    let lib = library(30, true);
    let batched = resolver(&lib);
    let mut per_file = resolver(&lib);
    assert!(batched.archive_ids.is_some());
    per_file.archive_ids = None;
    for source in &lib.sources {
        assert_eq!(
            batched.choose(source),
            per_file.choose(source),
            "{}",
            source.display()
        );
    }
}

fn rss_kib(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|line| line.starts_with(field))
        .and_then(|line| line.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(0)
}

fn ms(duration: Duration) -> String {
    format!("{:.0}ms", duration.as_secs_f64() * 1000.0)
}

/// Benchmark (ignored by default): the whole Quick Rename preview at 1000,
/// 2000 and 4000 games in one folder, stage by stage, as Blue runs it.
/// `cargo-iso test -p archivefs-gui --lib -- --ignored --nocapture preview_pipeline_benchmark`
#[test]
#[ignore]
fn preview_pipeline_benchmark() {
    let sizes: Vec<usize> = std::env::var("PREVIEW_BENCH_SIZES")
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1000, 2000, 4000]);
    for count in sizes {
        let started = Instant::now();
        let lib = library(count, false);
        let setup = started.elapsed();
        let rss_start = rss_kib("VmRSS:");

        // --- preview, in Blue's order
        let t = Instant::now();
        let names = resolver(&lib);
        let load = t.elapsed();
        let db = Database::open_read_only(&lib.database).unwrap();
        let t = Instant::now();
        let (candidates, _) = build_candidates_in(&db, &lib.sources, 1, &names);
        let candidates_time = t.elapsed();
        let candidates = with_new_names(candidates);
        let t = Instant::now();
        let mut preview = plan(&lib, &candidates);
        let plan_time = t.elapsed();
        let t = Instant::now();
        let stats =
            bulk_guard::guard_bulk_rename_plan_with_stats(&db, &mut preview, &locations(&lib));
        let guard_time = t.elapsed();
        let t = Instant::now();
        prepare_organisation_catalogue(&mut preview, &db).unwrap();
        let catalogue_time = t.elapsed();
        let total = load + candidates_time + plan_time + guard_time + catalogue_time;
        let rss_preview = rss_kib("VmRSS:");

        // --- the old candidate stage, for comparison
        let t = Instant::now();
        let mut per_file_names = resolver(&lib);
        per_file_names.archive_ids = None;
        let _ = build_candidates_per_file(&db, &lib.sources, 1, &per_file_names);
        let old_candidates = t.elapsed();

        // --- Apply-time checks (nothing applied): every Suggested row approved
        let approved: std::collections::BTreeSet<String> = preview
            .suggested()
            .map(|e| e.source_path.to_string_lossy().into_owned())
            .collect();
        let t = Instant::now();
        let apply_names = resolver(&lib);
        // Apply chooses every name again (cost measured); the benchmark's
        // names stand in for RomM titles, so the same names are returned.
        let revalidate = revalidate_organisation_plan(&preview, &lib.database, &|p| {
            let _ = apply_names.choose(p);
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            let ext = p.extension().unwrap().to_string_lossy().into_owned();
            Some(format!("{stem} (USA).{ext}"))
        });
        let revalidate_time = t.elapsed();
        let t = Instant::now();
        let reguard =
            bulk_guard::refuse_if_newly_blocked(&db, &preview, &approved, &locations(&lib));
        let reguard_time = t.elapsed();

        println!(
            "PIPELINE games={count} setup={} | PREVIEW total={} = names_load={} candidates={} \
             plan={} guard={} catalogue={} | rows: suggested={} blocked={} | guard reads: \
             save_folders={} entries={} sidecar_folders={} | rss_start_kib={rss_start} \
             rss_after_preview_kib={rss_preview} peak_kib={} | old per-file candidates={} | \
             APPLY checks: revalidate(transaction.rs)={} ({}) reguard={} ({})",
            ms(setup),
            ms(total),
            ms(load),
            ms(candidates_time),
            ms(plan_time),
            ms(guard_time),
            ms(catalogue_time),
            preview.suggested().count(),
            stats.blocked,
            stats.saves.folder_reads,
            stats.saves.entries_read,
            stats.sidecar_folder_reads,
            rss_kib("VmHWM:"),
            ms(old_candidates),
            ms(revalidate_time),
            if revalidate.is_ok() { "ok" } else { "refused" },
            ms(reguard_time),
            if reguard.is_ok() { "ok" } else { "refused" },
        );
        assert_eq!(
            stats.blocked,
            count / 100 + 2,
            "saves + arcade + cue-named disc"
        );
    }
}
