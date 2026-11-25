//! Library Makeover PR A benchmarks and acceptance on the scratch GBC fixture.
//! `#[ignore]`: runs only when asked, and only when EMUWIZ_BENCH_FIXTURE names
//! the fixture profile that EMUWIZ_DATA_HOME and EMUWIZ_CONFIG_HOME are inside
//! (fails closed otherwise). Prints one `BENCH key=value` line per measurement.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use archivefs_core::dat::rom_organisation::OrganisationMode;
use archivefs_core::job_progress::{JobProgress, ProgressReporter};

use super::scope::{OrganisationScope, ScopedGame};
use super::*;

/// The fixture profile, after refusing any path outside it.
fn fixture() -> PathBuf {
    let root = std::env::var_os("EMUWIZ_BENCH_FIXTURE").expect("EMUWIZ_BENCH_FIXTURE");
    let root = std::fs::canonicalize(root).expect("fixture");
    for key in ["EMUWIZ_DATA_HOME", "EMUWIZ_CONFIG_HOME", "HOME"] {
        let value = PathBuf::from(std::env::var_os(key).unwrap_or_else(|| panic!("{key} unset")));
        let real = std::fs::canonicalize(&value).expect(key);
        assert_eq!(real, value, "{key} is not canonical (symlink?)");
        assert!(
            real.starts_with(&root),
            "{key} outside the fixture: refused"
        );
    }
    root
}

fn library_games(limit: Option<usize>) -> Vec<ScopedGame> {
    let db = std::env::var("EMUWIZ_DATA_HOME").unwrap() + "/library.sqlite3";
    let connection =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = connection
        .prepare(
            "SELECT id, CAST(absolute_path_cached AS TEXT), last_verified_missing_at IS NOT NULL \
             FROM archives ORDER BY id LIMIT ?1",
        )
        .unwrap();
    statement
        .query_map([limit.map_or(-1, |n| n as i64)], |row| {
            Ok(ScopedGame {
                archive_id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                known_missing: row.get(2)?,
            })
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Drops the fixture's files from the page cache, so hashing is measured cold.
fn evict(dir: &Path) {
    let status = std::process::Command::new("python3")
        .args(["-I", "-c", EVICT, &dir.display().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

const EVICT: &str = "import os,sys\nfor d,_,fs in os.walk(sys.argv[1]):\n for f in fs:\n  p=os.path.join(d,f)\n  if os.path.isfile(p) and not os.path.islink(p):\n   fd=os.open(p,os.O_RDONLY)\n   os.posix_fadvise(fd,0,0,os.POSIX_FADV_DONTNEED)\n   os.close(fd)\n";

struct Timed {
    elapsed: Duration,
    longest_gap: Duration,
    completed: bool,
}

/// One preview on this thread, with the longest wait between progress events.
fn preview(state: &mut RomOrganisationPageState, cancel: Option<&AtomicBool>) -> Timed {
    let started = Instant::now();
    let last = std::cell::Cell::new(Instant::now());
    let longest_gap = std::cell::Cell::new(Duration::ZERO);
    let mut sink = |_: JobProgress| {
        longest_gap.set(longest_gap.get().max(last.get().elapsed()));
        last.set(Instant::now());
    };
    let mut progress = ProgressReporter::new(&mut sink);
    let completed = state.generate_plan_reporting(cancel, &mut progress);
    drop(progress);
    Timed {
        elapsed: started.elapsed(),
        longest_gap: longest_gap.get().max(last.get().elapsed()),
        completed,
    }
}

fn groups(state: &RomOrganisationPageState) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for entry in &state.plan.as_ref().unwrap().entries {
        let check = state
            .saved_checks
            .get(&entry.source_path)
            .copied()
            .unwrap_or(scope::SavedCheck::Missing);
        let group = scope::rename_group(entry, state.name_choices.get(&entry.source_path), check);
        *counts.entry(group.label()).or_insert(0) += 1;
    }
    counts
}

fn page() -> RomOrganisationPageState {
    let mut state = RomOrganisationPageState::load_deferred();
    state.ensure_mode(OrganisationMode::RenameInPlace);
    state
}

fn report(name: &str, state: &RomOrganisationPageState, timed: &Timed) {
    println!(
        "BENCH {name} seconds={:.3} longest_progress_gap_ms={} completed={} files={} groups={:?} incomplete={} not_on_disk={} other_fs={} error={:?}",
        timed.elapsed.as_secs_f64(),
        timed.longest_gap.as_millis(),
        timed.completed,
        state.sources.len(),
        state.plan.as_ref().map(|_| groups(state)),
        state.source_scan_incomplete.len(),
        state.not_on_disk.len(),
        state.other_filesystems.len(),
        state.error(),
    );
}

/// Name, size and SHA-256 of every file under `dir`.
fn tree(dir: &Path) -> BTreeMap<PathBuf, (u64, String)> {
    use sha2::Digest;
    let mut out = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let bytes = std::fs::read(entry.path()).unwrap();
                let sum: String = sha2::Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                out.insert(entry.path(), (bytes.len() as u64, sum));
            }
        }
    }
    out
}

fn ioprio() -> libc::c_long {
    // SAFETY: plain integer syscall on the calling thread.
    unsafe { libc::syscall(libc::SYS_ioprio_get, 1, 0) }
}

#[test]
#[ignore = "benchmark: needs the scratch GBC fixture"]
fn bench_previews() {
    let root = fixture();
    let gbc = root.join("roms/gbc");
    for (name, scope) in [
        (
            "games_10",
            OrganisationScope::Games {
                games: library_games(Some(10)),
                hidden_by_filter: 0,
            },
        ),
        (
            "games_100",
            OrganisationScope::Games {
                games: library_games(Some(100)),
                hidden_by_filter: 0,
            },
        ),
        (
            "platform_gbc",
            OrganisationScope::Platforms {
                names: ["Game Boy Color".to_string()].into(),
                games: library_games(None),
            },
        ),
        ("folder_gbc", OrganisationScope::Folders(vec![gbc.clone()])),
    ] {
        for run in ["cold", "warm"] {
            if run == "cold" {
                evict(&root);
            }
            let mut state = page();
            state.set_scope(Some(scope.clone()));
            let timed = preview(&mut state, None);
            report(&format!("{name}_{run}"), &state, &timed);
        }
    }
    // Cancellation: a full-platform preview cancelled after 20 ms.
    let mut state = page();
    state.set_scope(Some(OrganisationScope::Platforms {
        names: ["Game Boy Color".to_string()].into(),
        games: library_games(None),
    }));
    evict(&root);
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let timed = std::thread::scope(|threads| {
        threads.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            cancel.store(true, Ordering::Relaxed);
        });
        preview(&mut state, Some(&cancel))
    });
    println!(
        "BENCH cancel_preview seconds_to_return={:.3} after_cancel_ms={} completed={} plan_kept={}",
        timed.elapsed.as_secs_f64(),
        started.elapsed().as_millis().saturating_sub(20),
        timed.completed,
        state.plan.is_some(),
    );
}

/// The first `limit` Library games whose platform preview is a ready rename,
/// so the Apply benchmarks have real work (the first 100 Library games have none).
fn ready_games(limit: usize) -> Vec<ScopedGame> {
    let mut state = page();
    state.set_scope(Some(OrganisationScope::Platforms {
        names: ["Game Boy Color".to_string()].into(),
        games: library_games(None),
    }));
    preview(&mut state, None);
    let ready: std::collections::HashSet<PathBuf> = state
        .plan
        .as_ref()
        .unwrap()
        .suggested()
        .map(|entry| entry.source_path.clone())
        .collect();
    library_games(None)
        .into_iter()
        .filter(|game| ready.contains(&game.path))
        .take(limit)
        .collect()
}

#[test]
#[ignore = "acceptance: needs the scratch GBC fixture; renames fixture copies, then undoes"]
fn bench_apply_and_undo() {
    let root = fixture();
    let gbc = root.join("roms/gbc");
    let before = tree(&gbc);
    let unrelated_before = tree(&root.join("roms-unrelated"));
    let priority_before = ioprio();

    // 1. A refused Apply (one approved file changed) restores priority and limit.
    let hundred_ready = ready_games(100);
    assert_eq!(
        hundred_ready.len(),
        100,
        "the fixture has 100 ready renames"
    );
    let mut state = page();
    state.set_scope(Some(OrganisationScope::Games {
        games: hundred_ready.clone(),
        hidden_by_filter: 0,
    }));
    preview(&mut state, None);
    let ready: Vec<PathBuf> = state
        .plan
        .as_ref()
        .unwrap()
        .suggested()
        .map(|e| e.source_path.clone())
        .collect();
    let victim = ready.first().expect("a ready rename").clone();
    let original = std::fs::read(&victim).unwrap();
    let mut changed = original.clone();
    changed[0] ^= 0xFF;
    std::fs::write(&victim, &changed).unwrap();
    let started = Instant::now();
    state.apply();
    println!(
        "BENCH apply_refused seconds={:.3} error={:?} priority_restored={}",
        started.elapsed().as_secs_f64(),
        state.error(),
        ioprio() == priority_before
    );
    assert!(state.applied().is_none());
    std::fs::write(&victim, &original).unwrap();
    assert_eq!(tree(&gbc).len(), before.len());

    // 2. Apply the 100-game scope, then Undo; the tree must be byte-identical.
    let mut state = page();
    state.set_scope(Some(OrganisationScope::Games {
        games: hundred_ready,
        hidden_by_filter: 0,
    }));
    preview(&mut state, None);
    let approved = state.approved.len();
    evict(&root);
    let started = Instant::now();
    state.apply();
    let applied = state.applied().map(|t| t.entries.len());
    println!(
        "BENCH apply_100 approved={approved} seconds={:.3} applied={applied:?} error={:?} priority_restored={}",
        started.elapsed().as_secs_f64(),
        state.error(),
        ioprio() == priority_before
    );
    let after_apply = tree(&gbc);
    let started = Instant::now();
    state.rollback();
    println!(
        "BENCH undo_100 seconds={:.3} error={:?} tree_restored={} contents_unchanged_by_apply={} unrelated_untouched={}",
        started.elapsed().as_secs_f64(),
        state.error(),
        tree(&gbc) == before,
        after_apply
            .values()
            .collect::<std::collections::BTreeSet<_>>()
            == before.values().collect::<std::collections::BTreeSet<_>>(),
        tree(&root.join("roms-unrelated")) == unrelated_before,
    );

    // 3. Whole platform Apply (throttled above 1,000 files), cancelled after 5 s.
    let mut state = page();
    state.set_scope(Some(OrganisationScope::Platforms {
        names: ["Game Boy Color".to_string()].into(),
        games: library_games(None),
    }));
    preview(&mut state, None);
    let approved = state.approved.len();
    evict(&root);
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    std::thread::scope(|threads| {
        threads.spawn(|| {
            std::thread::sleep(Duration::from_secs(5));
            cancel.store(true, Ordering::Relaxed);
        });
        state.apply_cancellable(&cancel);
    });
    println!(
        "BENCH apply_platform_cancelled approved={approved} seconds={:.3} applied={:?} error={:?} priority_restored={}",
        started.elapsed().as_secs_f64(),
        state.applied().map(|t| t.entries.len()),
        state.error(),
        ioprio() == priority_before
    );
    if state.applied().is_some() {
        state.rollback();
    }
    println!(
        "BENCH after_platform_cancel tree_restored={}",
        tree(&gbc) == before
    );

    // 4. Whole platform Apply to completion (throttled), then Undo.
    let mut state = page();
    state.set_scope(Some(OrganisationScope::Platforms {
        names: ["Game Boy Color".to_string()].into(),
        games: library_games(None),
    }));
    preview(&mut state, None);
    let approved = state.approved.len();
    evict(&root);
    let started = Instant::now();
    state.apply();
    println!(
        "BENCH apply_platform approved={approved} seconds={:.3} applied={:?} error={:?} priority_restored={}",
        started.elapsed().as_secs_f64(),
        state.applied().map(|t| t.entries.len()),
        state.error(),
        ioprio() == priority_before
    );
    let started = Instant::now();
    state.rollback();
    println!(
        "BENCH undo_platform seconds={:.3} error={:?} tree_restored={} unrelated_untouched={}",
        started.elapsed().as_secs_f64(),
        state.error(),
        tree(&gbc) == before,
        tree(&root.join("roms-unrelated")) == unrelated_before,
    );
}
