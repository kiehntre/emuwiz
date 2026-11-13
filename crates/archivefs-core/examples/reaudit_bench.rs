//! Opt-in benchmark for DAT verification of a disposable, generated library.
//! Never run by `cargo test`. Everything is written under the directory given
//! on the command line; nothing else is read or changed.
//!
//! Each phase is its own process so that peak memory is per phase:
//!
//! ```text
//! reaudit_bench gen <dir> <loose> <zipped> <kib-per-game>  # games, catalogue, library
//! reaudit_bench platform <dir>          # existing whole-system verification
//! reaudit_bench strip <dir>             # turn saved rows into pre-proof rows
//! reaudit_bench plan <dir>              # list what needs checking (reads no game)
//! reaudit_bench recheck <dir> <games-per-set> [stop-after-sets]
//! reaudit_bench change <dir> <n>        # rewrite the first n loose games
//! reaudit_bench catalogue <dir>         # new catalogue bytes, same entries
//! ```
//!
//! Every phase prints one JSON line with wall time, bytes and peak memory.

use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use archivefs_core::Database;
use archivefs_core::dat::catalogue_selection::{
    CatalogueInventoryInputs, CatalogueRef, ResolvedCatalogue, resolve_catalogue,
};
use archivefs_core::dat::configured_catalogues::{ConfiguredCatalogue, configured_catalogues};
use archivefs_core::dat::library_verification::preview_library_verification;
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::reaudit::{
    ReauditLimits, ReauditProgress, ReauditStop, plan_reaudit, run_reaudit,
};
use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};
use archivefs_core::safe_read::TrustedRoots;
use sha1::{Digest, Sha1};

const PLATFORM: &str = "ZX Spectrum";

struct Bench {
    dir: PathBuf,
}

impl Bench {
    fn db(&self) -> PathBuf {
        self.dir.join("library.sqlite3")
    }
    fn dat(&self) -> PathBuf {
        self.dir.join("catalogue.dat")
    }
    fn roms(&self) -> PathBuf {
        self.dir.join("roms")
    }
    fn registry(&self) -> DatSourceRegistry {
        let mut registry = DatSourceRegistry::default();
        registry
            .add(DatSourceEntry::new(
                "bench".into(),
                "Benchmark catalogue".into(),
                self.dat(),
                DatSourceKind::File,
            ))
            .unwrap();
        registry
    }
    fn resolved(&self) -> ResolvedCatalogue {
        resolve_catalogue(
            &CatalogueRef::local("bench"),
            CatalogueInventoryInputs {
                local_registry: &self.registry(),
                managed_sources: &Default::default(),
                managed_root: &self.dir,
                limits: DatLimits::default(),
            },
        )
        .unwrap()
    }
    fn configured(&self) -> std::collections::HashMap<String, ConfiguredCatalogue> {
        configured_catalogues(CatalogueInventoryInputs {
            local_registry: &self.registry(),
            managed_sources: &Default::default(),
            managed_root: &self.dir,
            limits: DatLimits::default(),
        })
    }
}

fn content(seed: u64, bytes: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(bytes + 8);
    while out.len() < bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(bytes);
    out
}

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn write_dat(bench: &Bench, games: &[(usize, usize, String)], version: &str) {
    let mut xml = format!(
        "<datafile><header><name>Sinclair ZX Spectrum - Games - [TAP]</name><description>Sinclair ZX Spectrum - Games - [TAP]</description><version>{version}</version><author>TOSEC</author></header>"
    );
    for (n, size, sha1) in games {
        xml.push_str(&format!(
            "<game name=\"Game {n:06}\"><rom name=\"Game {n:06}.tap\" size=\"{size}\" sha1=\"{sha1}\"/></game>"
        ));
    }
    xml.push_str("</datafile>");
    std::fs::write(bench.dat(), xml).unwrap();
}

fn peak_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn report(phase: &str, started: Instant, bytes: u64, extra: serde_json::Value) {
    let seconds = started.elapsed().as_secs_f64();
    let mut line = serde_json::json!({
        "phase": phase,
        "seconds": (seconds * 1000.0).round() / 1000.0,
        "bytes": bytes,
        "mib_per_second": if seconds > 0.0 { ((bytes as f64 / 1_048_576.0 / seconds) * 10.0).round() / 10.0 } else { 0.0 },
        "peak_rss_mib": peak_rss_kib() / 1024,
    });
    line.as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    println!("{line}");
}

/// Samples, from a second thread, how long SQLite's rollback journal beside
/// the library has a live header: the time during which a killed process
/// would leave a journal EmuWiz refuses to open over.
struct JournalSampler {
    stop: std::sync::Arc<AtomicBool>,
    handle: std::thread::JoinHandle<(u64, u64, u64, f64, f64)>,
}

impl JournalSampler {
    fn start(database: &Path) -> Self {
        let mut journal = database.as_os_str().to_os_string();
        journal.push("-journal");
        let journal = PathBuf::from(journal);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            use std::io::Read as _;
            let (mut samples, mut live, mut intervals) = (0u64, 0u64, 0u64);
            let (mut total, mut longest) = (0f64, 0f64);
            let mut since: Option<Instant> = None;
            while !flag.load(Ordering::Relaxed) {
                let mut head = [0u8; 8];
                let is_live = std::fs::File::open(&journal)
                    .and_then(|mut file| file.read_exact(&mut head))
                    .is_ok()
                    && head != [0u8; 8];
                samples += 1;
                match (is_live, since) {
                    (true, None) => {
                        since = Some(Instant::now());
                        intervals += 1;
                        live += 1;
                    }
                    (true, Some(_)) => live += 1,
                    (false, Some(started)) => {
                        let seconds = started.elapsed().as_secs_f64();
                        total += seconds;
                        longest = longest.max(seconds);
                        since = None;
                    }
                    (false, None) => {}
                }
                std::thread::sleep(std::time::Duration::from_micros(100));
            }
            (samples, live, intervals, total, longest)
        });
        Self { stop, handle }
    }

    fn finish(self) -> serde_json::Value {
        self.stop.store(true, Ordering::Relaxed);
        let (samples, live, intervals, total, longest) = self.handle.join().unwrap();
        serde_json::json!({
            "journal_samples": samples,
            "journal_live_samples": live,
            "journal_live_intervals": intervals,
            "journal_live_ms": (total * 1e5).round() / 100.0,
            "journal_live_longest_ms": (longest * 1e5).round() / 100.0,
        })
    }
}

fn merged(mut a: serde_json::Value, b: serde_json::Value) -> serde_json::Value {
    a.as_object_mut()
        .unwrap()
        .extend(b.as_object().cloned().unwrap_or_default());
    a
}

fn generate(bench: &Bench, loose: usize, zipped: usize, kib: usize) {
    let started = Instant::now();
    std::fs::create_dir_all(bench.roms()).unwrap();
    Database::open_or_create(bench.db()).unwrap();
    let mut conn = rusqlite::Connection::open(bench.db()).unwrap();
    conn.execute(
        "INSERT INTO source_folders(id,path,first_seen_at,last_seen_in_config_at) VALUES(1,?1,'now','now')",
        [bench.roms().as_os_str().as_bytes()],
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    let mut games = Vec::new();
    let mut total = 0u64;
    for n in 0..loose + zipped {
        // Sizes vary around the requested mean, as real sets do.
        let size = kib * 1024 / 2 + (n * 7919 % (kib * 1024));
        let bytes = content(n as u64 + 1, size);
        games.push((n, size, sha1_hex(&bytes)));
        let name = if n < loose {
            format!("g{n:06}.tap")
        } else {
            format!("g{n:06}.zip")
        };
        let path = bench.roms().join(&name);
        if n < loose {
            std::fs::write(&path, &bytes).unwrap();
        } else {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            zip.start_file(
                format!("Game {n:06}.tap"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(&bytes).unwrap();
            zip.finish().unwrap();
        }
        let on_disk = std::fs::metadata(&path).unwrap().len();
        total += on_disk;
        tx.execute(
            "INSERT INTO archives(id,source_folder_id,relative_path,absolute_path_cached,file_name_cached,archive_kind,display_name,normalized_name,size_bytes,first_seen_at,last_seen_at,created_at,updated_at) VALUES(?1,1,?2,?3,?2,'direct_game_image',?4,?4,?5,'now','now','now','now')",
            rusqlite::params![n as i64 + 1, name.as_bytes(), path.as_os_str().as_bytes(), name, on_disk as i64],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO platform_assignments(archive_id,platform,source,is_current,assigned_at) VALUES(?1,?2,'manual',1,'now')",
            rusqlite::params![n as i64 + 1, PLATFORM],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    write_dat(bench, &games, "bench-1");
    std::fs::write(
        bench.dir.join("games.json"),
        serde_json::to_vec(&games).unwrap(),
    )
    .unwrap();
    report(
        "gen",
        started,
        total,
        serde_json::json!({"loose": loose, "zipped": zipped}),
    );
}

fn platform(bench: &Bench) {
    let started = Instant::now();
    let preview =
        preview_library_verification(bench.resolved(), bench.db(), PLATFORM.into(), None).unwrap();
    let bytes = preview.bytes;
    let sampler = JournalSampler::start(&bench.db());
    let totals = preview
        .execute(&TrustedRoots::none(), &AtomicBool::new(false), &|_| {})
        .unwrap();
    let journal = sampler.finish();
    report(
        "platform",
        started,
        bytes,
        merged(
            serde_json::json!({"verified": totals.verified, "no_match": totals.no_match, "not_checked": totals.not_checked, "saved": totals.saved}),
            journal,
        ),
    );
}

fn strip(bench: &Bench) {
    let started = Instant::now();
    let mut conn = rusqlite::Connection::open(bench.db()).unwrap();
    let rows: Vec<(i64, Vec<u8>)> = conn
        .prepare("SELECT id, facts_json FROM library_dat_identities")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let tx = conn.transaction().unwrap();
    for (id, bytes) in &rows {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("audit_file");
        object["source"]
            .as_object_mut()
            .unwrap()
            .remove("catalogue_sha256");
        tx.execute(
            "UPDATE library_dat_identities SET facts_json = ?1 WHERE id = ?2",
            rusqlite::params![serde_json::to_vec(&value).unwrap(), id],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    report("strip", started, 0, serde_json::json!({"rows": rows.len()}));
}

fn plan(bench: &Bench) {
    let started = Instant::now();
    let plan = plan_reaudit(&bench.db(), &bench.configured()).unwrap();
    report(
        "plan",
        started,
        0,
        serde_json::json!({"to_check": plan.games(), "current": plan.current, "blocked": plan.blocked.len(), "saved_rows": plan.saved_rows}),
    );
}

fn recheck(bench: &Bench, games_per_set: usize, stop_after: Option<usize>) {
    let started = Instant::now();
    let plan = plan_reaudit(&bench.db(), &bench.configured()).unwrap();
    let bytes = plan.bytes();
    let cancel = AtomicBool::new(false);
    let sets = AtomicUsize::new(0);
    let sampler = JournalSampler::start(&bench.db());
    let report_ = run_reaudit(
        &bench.db(),
        &plan,
        &|_| Ok(bench.resolved()),
        &TrustedRoots::none(),
        &cancel,
        ReauditLimits {
            max_games_per_set: games_per_set,
            ..Default::default()
        },
        &|progress| {
            if matches!(progress, ReauditProgress::SetSaved { .. })
                && Some(sets.fetch_add(1, Ordering::Relaxed) + 1) == stop_after
            {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap();
    let journal = sampler.finish();
    let done = report_.verified + report_.no_match + report_.ambiguous;
    let done_bytes = if plan.games() == 0 {
        0
    } else {
        (bytes as f64 * done as f64 / plan.games() as f64) as u64
    };
    report(
        "recheck",
        started,
        done_bytes,
        merged(
            journal,
            serde_json::json!({
                "planned": report_.planned, "verified": report_.verified, "no_match": report_.no_match,
                "not_checked": report_.not_checked.len(), "sets_saved": report_.sets_saved,
                "not_reached": report_.not_reached, "games_per_set": games_per_set,
                "stopped": match report_.stopped { None => "no".to_string(), Some(ReauditStop::Cancelled) => "on request".into(), Some(ReauditStop::Interrupted(r)) => r },
            }),
        ),
    );
}

fn change(bench: &Bench, n: usize) {
    let started = Instant::now();
    for index in 0..n {
        let path = bench.roms().join(format!("g{index:06}.tap"));
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
    }
    report("change", started, 0, serde_json::json!({"changed": n}));
}

fn catalogue(bench: &Bench) {
    let started = Instant::now();
    let games: Vec<(usize, usize, String)> =
        serde_json::from_slice(&std::fs::read(bench.dir.join("games.json")).unwrap()).unwrap();
    write_dat(bench, &games, "bench-2");
    report(
        "catalogue",
        started,
        0,
        serde_json::json!({"entries": games.len()}),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number = |index: usize| -> usize {
        args.get(index)
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("argument {index} must be a number"))
    };
    let (Some(phase), Some(dir)) = (args.first(), args.get(1)) else {
        eprintln!(
            "usage: reaudit_bench <gen|platform|strip|plan|recheck|change|catalogue> <dir> ..."
        );
        std::process::exit(2);
    };
    let bench = Bench {
        dir: Path::new(dir).to_path_buf(),
    };
    match phase.as_str() {
        "gen" => generate(&bench, number(2), number(3), number(4)),
        "platform" => platform(&bench),
        "strip" => strip(&bench),
        "plan" => plan(&bench),
        "recheck" => recheck(&bench, number(2), args.get(3).and_then(|v| v.parse().ok())),
        "change" => change(&bench, number(2)),
        "catalogue" => catalogue(&bench),
        other => panic!("unknown phase {other}"),
    }
}
