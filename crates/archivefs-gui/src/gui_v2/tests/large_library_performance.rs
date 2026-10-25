//! Explicit disposable measurement; never points at an owner's catalogue.
use super::*;
use rusqlite::{Connection, ffi, params};
use std::{
    collections::BTreeMap,
    ffi::{CStr, c_void},
    sync::Mutex,
    time::Instant,
};

static SQL: Mutex<Option<BTreeMap<String, (usize, u64)>>> = Mutex::new(None);
unsafe extern "C" fn profile(_: u32, _: *mut c_void, stmt: *mut c_void, ns: *mut c_void) -> i32 {
    // SQLite PROFILE supplies a live sqlite3_stmt and a u64 elapsed-ns pointer
    // for the duration of this synchronous callback; retain neither pointer.
    let sql = unsafe { ffi::sqlite3_sql(stmt.cast()) };
    if sql.is_null() {
        return 0;
    }
    let text = unsafe { CStr::from_ptr(sql) }.to_string_lossy();
    let elapsed = unsafe { *ns.cast::<u64>() };
    if let Ok(mut guard) = SQL.lock()
        && let Some(counts) = guard.as_mut()
    {
        let row = counts.entry(text.into_owned()).or_default();
        row.0 += 1;
        row.1 += elapsed;
    }
    0
}
unsafe extern "C" fn trace_connection(
    db: *mut ffi::sqlite3,
    _: *mut *mut std::ffi::c_char,
    _: *const ffi::sqlite3_api_routines,
) -> i32 {
    unsafe {
        ffi::sqlite3_trace_v2(
            db,
            ffi::SQLITE_TRACE_PROFILE as u32,
            Some(profile),
            std::ptr::null_mut(),
        )
    }
}
fn begin_sql() {
    *SQL.lock().unwrap() = Some(BTreeMap::new());
}
fn report_sql(label: &str) {
    let counts = SQL.lock().unwrap().take().unwrap();
    let total: usize = counts.values().map(|v| v.0).sum();
    let ns: u64 = counts.values().map(|v| v.1).sum();
    println!(
        "PERF_SQL {label} statements={total} sqlite_ms={:.3} distinct={}",
        ns as f64 / 1e6,
        counts.len()
    );
    for (query, (calls, time)) in counts {
        println!(
            "PERF_QUERY {label} calls={calls} ms={:.3} sql={query}",
            time as f64 / 1e6
        );
    }
}
fn resources(label: &str) {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let memory: Vec<_> = status
        .lines()
        .filter(|l| l.starts_with("VmRSS:") || l.starts_with("VmHWM:"))
        .collect();
    let stat = fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    println!(
        "PERF_RESOURCE {label} {} user_ticks={} system_ticks={}",
        memory.join(" "),
        fields[11],
        fields[12]
    );
}
fn thread_cpu_ms() -> f64 {
    fs::read_to_string("/proc/thread-self/schedstat")
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<f64>()
        .unwrap()
        / 1e6
}
fn measure<T>(label: &str, work: impl FnOnce() -> T) -> T {
    let cpu = thread_cpu_ms();
    let start = Instant::now();
    let result = work();
    println!(
        "PERF_TIME {label} ms={:.3} thread_cpu_ms={:.3}",
        start.elapsed().as_secs_f64() * 1e3,
        thread_cpu_ms() - cpu
    );
    result
}
fn render(context: &egui::Context, app: &mut App, label: &str) {
    begin_sql();
    let mut times = Vec::new();
    let mut cpu_times = Vec::new();
    for _ in 0..12 {
        let cpu = thread_cpu_ms();
        let start = Instant::now();
        let _ = frame(context, app, [1280.0, 820.0]);
        app.finish_frame();
        times.push(start.elapsed().as_secs_f64() * 1e3);
        cpu_times.push(thread_cpu_ms() - cpu);
    }
    let first = times[0];
    let first_cpu = cpu_times[0];
    cpu_times[2..].sort_by(f64::total_cmp);
    println!(
        "PERF_FRAME_CPU {label} first_ms={first_cpu:.3} warm_p50_ms={:.3}",
        cpu_times[7]
    );
    times[2..].sort_by(f64::total_cmp);
    println!(
        "PERF_FRAME {label} first_ms={first:.3} warm_p50_ms={:.3} warm_max_ms={:.3}",
        times[7], times[11]
    );
    report_sql(label);
    resources(label);
}
#[test]
#[ignore = "explicit 130k disposable headless benchmark; run alone with --exact --ignored --nocapture"]
fn blue_large_library_measurement() {
    if std::env::var_os("EMUWIZ_PERF_CHILD").is_none() {
        let bin = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "gui_v2::tests::large_library_performance::blue_large_library_measurement",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("EMUWIZ_PERF_CHILD", "1")
            .env("PATH", bin.path())
            .env_remove("APPIMAGE")
            .env_remove("APPDIR")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    assert!(
        archivefs_core::test_environment::isolated_root().is_some(),
        "private HOME/XDG required"
    );
    const N: usize = 130_000;
    let temp = tempfile::tempdir().unwrap();
    let path = archivefs_core::default_database_path().unwrap();
    assert!(path.starts_with(archivefs_core::test_environment::isolated_root().unwrap()));
    assert!(!path.exists(), "fresh private benchmark database required");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let _ = archivefs_core::Database::open_or_create(&path).unwrap();
    let mut db = Connection::open(&path).unwrap();
    let tx = db.transaction().unwrap();
    tx.execute("INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')",[temp.path().as_os_str().as_encoded_bytes()]).unwrap();
    {
        let mut archive=tx.prepare("INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,size_bytes,modified_time_unix_seconds,first_seen_at,last_seen_at,created_at,updated_at) VALUES(?1,1,?2,?3,?2,'zip',?4,?4,1024,1,'now','now','now','now')").unwrap();
        let mut platform=tx.prepare("INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) VALUES(?1,?2,'manual',1,'now')").unwrap();
        for id in 1..=N {
            let name = format!("Game {:06} (Region {})", (id * 7919) % N, id % 4);
            let relative = format!("{name}.zip");
            let absolute = temp.path().join(&relative);
            archive
                .execute(params![
                    id as i64,
                    relative.as_bytes(),
                    absolute.as_os_str().as_encoded_bytes(),
                    name
                ])
                .unwrap();
            platform
                .execute(params![
                    id as i64,
                    [
                        "SNES",
                        "Amiga",
                        "BBCMicro",
                        "ZXSpectrum",
                        "PlayStation",
                        "AtariST",
                        "MegaDrive",
                        "Arcade"
                    ][id % 8]
                ])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    drop(db);
    // Auto-extension affects only this explicit isolated benchmark process.
    // It is removed before returning; connection profiles retain no DB pointers.
    unsafe {
        assert_eq!(
            ffi::sqlite3_auto_extension(Some(trace_connection)),
            ffi::SQLITE_OK
        );
    }
    for run in 0..3 {
        begin_sql();
        let library = measure(&format!("startup{run}"), || {
            backend::load_library_with_catalogues(&path, &Default::default()).unwrap()
        });
        assert_eq!(library.games.len(), N);
        report_sql(&format!("startup{run}"));
        resources(&format!("startup{run}"));
        if run == 2 {
            let context = egui::Context::default();
            let mut app = fixture(&context);
            app.simple.enabled = true;
            app.library = Arc::new(library);
            app.loaded = true;
            app.indices = app.library.filter(&app.filter);
            render(&context, &mut app, "home");
            app.go(Route::Section(Section::Platforms));
            render(&context, &mut app, "platforms");
            app.go(Route::Section(Section::Games));
            render(&context, &mut app, "games_grid");
            app.filter.list = true;
            render(&context, &mut app, "games_list");
            app.filter.list = false;
            app.go(Route::BrowsePlay);
            render(&context, &mut app, "browse_play");
            for search in ["Game", "Region 2", "zz-nothing"] {
                app.filter.search = search.into();
                let indices = measure(&format!("search_{search}"), || {
                    app.library.filter(&app.filter)
                });
                println!("PERF_MATCH search={search:?} rows={}", indices.len());
                render(&context, &mut app, &format!("browse_search_{search}"));
            }
            app.filter = Default::default();
            app.filter.platform = "Amiga".into();
            measure("platform_filter", || {
                assert_eq!(app.library.filter(&app.filter).len(), N / 8)
            });
            app.go(Route::Section(Section::Museum));
            render(&context, &mut app, "museum_platform");
            // Detail state is synthetic; no real media is inspected or launched.
            app.go(Route::Game(65_000));
            app.detail = Some(Detail {
                game: 65_000,
                ..Default::default()
            });
            render(&context, &mut app, "game_details");
            for id in 0..2_000 {
                let job = app
                    .activity
                    .queue(&format!("Synthetic job {id}"), Route::Home, false);
                app.activity.finish(job, "synthetic complete".into(), None);
            }
            app.go(Route::Section(Section::Activity));
            assert_eq!(app.activity.jobs.len(), 100);
            render(&context, &mut app, "activity_100_after_2000");
            app.canonical_organisation_history = (0..1000)
                .map(|n| {
                    use archivefs_core::dat::rename_apply::model::{EntryState, TransactionState};
                    let path = &app.library.games[n].archive.absolute_path;
                    super::super::history_view::tests::tx(
                        &format!("fixture-{n}"),
                        TransactionState::Applied,
                        vec![super::super::history_view::tests::item(
                            path.to_str().unwrap(),
                            &format!("{}.moved", path.display()),
                            EntryState::Applied,
                        )],
                    )
                })
                .collect();
            app.go(Route::Section(Section::History));
            render(&context, &mut app, "history");
            measure("artwork_empty_providers", || {
                let index = MediaIndex::discover(&app.library);
                resources("artwork_live");
                assert_eq!(index.covers.len(), 0);
                println!(
                    "PERF_ARTWORK resolved={} warnings={}",
                    index.resolved.len(),
                    index.warnings.len()
                );
            });
            resources("after_artwork");
        }
    }
    // Actual asynchronous coordinator lifecycle, with private default roots.
    // The eframe Frame argument is unused by App::ui; mirror its calls here.
    let context = egui::Context::default();
    begin_sql();
    let start = Instant::now();
    let mut app = App::new(context.clone());
    while !app.loaded && start.elapsed() < std::time::Duration::from_secs(120) {
        full_frame(&context, &mut app);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        app.loaded,
        "synthetic startup failed: {:?}",
        app.library_failure.as_ref().map(|_| "library failure")
    );
    println!(
        "PERF_TIME async_startup_to_loaded ms={:.3}",
        start.elapsed().as_secs_f64() * 1e3
    );
    report_sql("async_startup_to_loaded");
    full_render(&context, &mut app, "home");
    app.go(Route::BrowsePlay);
    full_render(&context, &mut app, "browse_play_grid");
    app.filter.search = "Region 2".into();
    app.change_filter();
    full_render(&context, &mut app, "search_debounce");
    app.go(Route::Game(65_000));
    full_render(&context, &mut app, "game_details");
    app.go(Route::Section(Section::History));
    full_render(&context, &mut app, "history");
    app.go(Route::Section(Section::Activity));
    full_render(&context, &mut app, "activity");
    resources("full_cycle_end");
    unsafe {
        ffi::sqlite3_cancel_auto_extension(Some(trace_connection));
    }
}

fn full_frame(context: &egui::Context, app: &mut App) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        },
        |ctx| {
            app.mods.poll();
            let (mut library, mut artwork) = (false, false);
            if let Some(workflows) = &mut app.native_workflows {
                workflows.poll(ctx, &mut app.activity);
                if app.load_job.is_none() {
                    library = workflows.take_source_library_reload()
                        | workflows.take_dat_identity_reload();
                }
                if !app.artwork.index_loading {
                    artwork = workflows.take_artwork_reload();
                }
            }
            if library {
                app.load(false);
            }
            if artwork {
                app.refresh_artwork_index();
            }
            app.poll(ctx);
            app.show(ctx);
            app.simple.paint_focus_ring(ctx);
            app.finish_frame();
        },
    );
}
fn full_render(context: &egui::Context, app: &mut App, label: &str) {
    let expected = app.router.current.clone();
    begin_sql();
    let mut times = Vec::new();
    let mut cpu_times = Vec::new();
    for _ in 0..32 {
        let cpu = thread_cpu_ms();
        let start = Instant::now();
        full_frame(context, app);
        times.push(start.elapsed().as_secs_f64() * 1e3);
        cpu_times.push(thread_cpu_ms() - cpu);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(
        app.router.current, expected,
        "async navigation changed measured route"
    );
    println!("PERF_ROUTE {label} {:?}", app.router.current);
    let first_cpu = cpu_times[0];
    cpu_times.sort_by(f64::total_cmp);
    println!(
        "PERF_FULL_CPU {label} first_ms={first_cpu:.3} p50_ms={:.3} p95_ms={:.3} filter_pending={}",
        cpu_times[16],
        cpu_times[30],
        app.filter_dirty.is_some() || app.filter_inflight
    );
    let first = times[0];
    times.sort_by(f64::total_cmp);
    println!(
        "PERF_FULL_FRAME {label} first_ms={first:.3} p50_ms={:.3} p95_ms={:.3} max_ms={:.3} active_jobs={} artwork_active={}",
        times[16],
        times[30],
        times[31],
        app.activity.active(),
        app.artwork.active()
    );
    report_sql(&format!("full_{label}"));
    resources(&format!("full_{label}"));
}
