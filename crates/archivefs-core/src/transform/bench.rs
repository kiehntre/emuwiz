//! Project Ouroboros benchmark (disposable data only). A unit test, so it
//! runs with the crate-private, `cfg(test)`-only scratch authority and needs
//! no Cargo feature. Ignored by default; run it explicitly:
//!
//! ```text
//! EMUWIZ_OUROBOROS_BENCH_DIR=<empty scratch folder> \
//! EMUWIZ_OUROBOROS_BENCH_SECOND_DIR=/tmp/ob2 \
//! [EMUWIZ_OUROBOROS_BENCH_SIZES=100,10000] [EMUWIZ_OUROBOROS_BENCH_COPY_MIB=256] \
//! [EMUWIZ_OUROBOROS_BENCH_JOURNALS=200] \
//! /usr/bin/time -v <archivefs_core test binary> --ignored --exact --nocapture \
//!     transform::bench::ouroboros_benchmark
//! ```
//!
//! Every file, catalogue and journal it creates lives under the bench
//! folder (which must be empty) and, for the cross-filesystem phase, under
//! the second folder, which must be on a different ext4 filesystem. It never
//! opens the real EmuWiz catalogue.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::authority::ScratchAuthority;
use super::{self as transform, Context, TxnState, graph};
use crate::{Archive, Database};

struct Library {
    root_a: PathBuf,
    db: Database,
    dest: PathBuf,
    journal: PathBuf,
    auth: ScratchAuthority,
}

impl Library {
    fn plan(&self, source: &Path) -> transform::Preview {
        transform::plan_relocation_scratch(&self.db, source, &self.dest, &self.auth)
    }

    fn ctx(&self) -> Context<'_> {
        Context::new(&self.journal).with_authority(&self.auth)
    }
}

fn library(base: &Path, other: &Path, files: usize, size: usize) -> (Library, Vec<PathBuf>) {
    let mut auth = ScratchAuthority::new();
    let root_a = auth.provision_root(base, "lib-a").unwrap();
    let root_b = auth.provision_root(other, "lib-b").unwrap();
    std::fs::create_dir_all(root_a.join("nes")).unwrap();
    std::fs::create_dir_all(root_b.join("nes")).unwrap();
    let mut db = Database::open_or_create(base.join("catalogue.sqlite3")).unwrap();
    db.register_source_folders(&[root_a.clone(), root_b.clone()])
        .unwrap();
    let folder = db
        .list_source_folders()
        .unwrap()
        .into_iter()
        .find(|f| f.path == root_a)
        .unwrap()
        .id;
    let mut paths = Vec::with_capacity(files);
    let t = Instant::now();
    for i in 0..files {
        let p = root_a.join("nes").join(format!("Game {i:06} (USA).nes"));
        let mut bytes = vec![(i % 251) as u8; size];
        bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
        std::fs::write(&p, &bytes).unwrap();
        let archive = Archive::from_path_in_root(&p, &root_a).unwrap();
        db.upsert_archive(folder, &root_a, &archive).unwrap();
        paths.push(p);
    }
    eprintln!("  fixture: {files} files catalogued in {:.2?}", t.elapsed());
    let journal = base.join("journals");
    (
        Library {
            root_a,
            db,
            dest: root_b.join("nes"),
            journal,
            auth,
        },
        paths,
    )
}

fn planning(base: &Path, sizes: &[usize]) {
    println!("## Planning latency (same filesystem, files in one folder)");
    println!("| files in folder | graph build | plan (median of samples) | plan max |");
    println!("|---|---|---|---|");
    for &n in sizes {
        let dir = base.join(format!("plan-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        let (lib, paths) = library(&dir, &dir, n, 64);
        let t = Instant::now();
        let g = graph::build_directory(&lib.root_a.join("nes")).unwrap();
        let graph_time = t.elapsed();
        drop(g);
        let samples = 15.min(n);
        let mut times = Vec::new();
        for i in 0..samples {
            let src = &paths[i * n / samples];
            let t = Instant::now();
            let p = lib.plan(src);
            times.push(t.elapsed());
            assert!(p.blockers.is_empty(), "{:?}", p.blockers);
        }
        times.sort();
        println!(
            "| {n} | {graph_time:.2?} | {:.2?} | {:.2?} |",
            times[times.len() / 2],
            times[times.len() - 1]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

fn cp_seconds(src: &Path, dst: &Path) -> f64 {
    let t = Instant::now();
    let ok = std::process::Command::new("cp")
        .arg(src)
        .arg(dst)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let s = std::fs::File::open(dst).unwrap();
    s.sync_all().unwrap();
    t.elapsed().as_secs_f64()
}

fn throughput(base: &Path, second: &Path, mib: usize) {
    println!("\n## Cross-filesystem relocation throughput ({mib} MiB, ext4 -> ext4)");
    let other = tempfile::Builder::new()
        .prefix("ouroboros-bench-")
        .tempdir_in(second)
        .unwrap();
    let dir = base.join("copy");
    std::fs::create_dir_all(&dir).unwrap();
    let (lib, paths) = library(&dir, other.path(), 1, mib << 20);
    assert_ne!(
        std::fs::metadata(&lib.root_a).unwrap().dev(),
        std::fs::metadata(&lib.dest).unwrap().dev()
    );
    let preview = lib.plan(&paths[0]);
    let (plan, digest) = (preview.plan.unwrap(), preview.digest.unwrap());
    let ctx = lib.ctx();
    let t = Instant::now();
    let state = transform::apply(&ctx, &plan, &digest).unwrap();
    let engine_s = t.elapsed().as_secs_f64();
    assert_eq!(state, TxnState::Complete);
    let baseline = other.path().join("cp-baseline.bin");
    let retained = plan.retired_path.clone().unwrap();
    let cp_s = cp_seconds(&retained, &baseline);
    std::fs::remove_file(&baseline).unwrap();
    println!("| engine apply | cp + fsync | ratio | engine MiB/s |");
    println!("|---|---|---|---|");
    println!(
        "| {engine_s:.2} s | {cp_s:.2} s | {:.1}x | {:.0} |",
        engine_s / cp_s,
        mib as f64 / engine_s
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

fn journals(base: &Path, count: usize) {
    println!("\n## Journal scaling ({count} complete transactions)");
    let dir = base.join("journals");
    std::fs::create_dir_all(&dir).unwrap();
    let (lib, paths) = library(&dir, &dir, count, 64);
    let ctx = lib.ctx();
    let t = Instant::now();
    let mut last = None;
    for p in &paths {
        let pv = lib.plan(p);
        let (plan, digest) = (pv.plan.unwrap(), pv.digest.unwrap());
        assert_eq!(
            transform::apply(&ctx, &plan, &digest).unwrap(),
            TxnState::Complete
        );
        last = Some(plan.txn_id);
    }
    let apply_each = t.elapsed() / count as u32;
    let t = Instant::now();
    let rows = transform::list(&ctx);
    let list_t = t.elapsed();
    assert_eq!(rows.len(), count);
    let t = Instant::now();
    let recovered = transform::recover_all(&ctx);
    let recover_t = t.elapsed();
    assert!(
        recovered
            .iter()
            .all(|(_, r)| matches!(r, Ok(TxnState::Complete)))
    );
    let t = Instant::now();
    let blocker = transform::undo_blocker(&ctx, &last.unwrap());
    let undo_t = t.elapsed();
    assert!(blocker.is_none(), "{blocker:?}");
    println!("| apply (each, same fs) | list all | recover_all (no-op) | undo eligibility (one) |");
    println!("|---|---|---|---|");
    println!("| {apply_each:.2?} | {list_t:.2?} | {recover_t:.2?} | {undo_t:.2?} |");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn contention(base: &Path) {
    println!("\n## Catalogue contention (another writer holds the database for 1.5 s)");
    let dir = base.join("contention");
    std::fs::create_dir_all(&dir).unwrap();
    let (lib, paths) = library(&dir, &dir, 1, 4096);
    let pv = lib.plan(&paths[0]);
    let (plan, digest) = (pv.plan.unwrap(), pv.digest.unwrap());
    let db_path = lib.db.path().to_path_buf();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let c = rusqlite::Connection::open(db_path).unwrap();
        c.execute_batch("BEGIN IMMEDIATE").unwrap();
        ready_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        c.execute_batch("COMMIT").unwrap();
    });
    ready_rx.recv().unwrap();
    let ctx = lib.ctx();
    let t = Instant::now();
    let state = transform::apply(&ctx, &plan, &digest);
    let elapsed = t.elapsed();
    holder.join().unwrap();
    let label = match &state {
        Ok(s) => s.label().to_string(),
        Err(e) => format!("error: {e}"),
    };
    println!("| outcome | elapsed |\n|---|---|\n| {label} | {elapsed:.2?} |");
    if !matches!(state, Ok(TxnState::Complete)) {
        let after = transform::recover(&ctx, &plan.txn_id).map(|s| s.label().to_string());
        println!("\nRecovery after the writer released: {after:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[test]
#[ignore = "benchmark: run explicitly with EMUWIZ_OUROBOROS_BENCH_DIR"]
fn ouroboros_benchmark() {
    let Some(base) = env("EMUWIZ_OUROBOROS_BENCH_DIR").map(PathBuf::from) else {
        eprintln!("SKIPPED: EMUWIZ_OUROBOROS_BENCH_DIR (an empty scratch folder) is not set");
        return;
    };
    std::fs::create_dir_all(&base).unwrap();
    assert!(
        std::fs::read_dir(&base).unwrap().next().is_none(),
        "the bench folder must be empty"
    );
    let sizes: Vec<usize> = env("EMUWIZ_OUROBOROS_BENCH_SIZES")
        .unwrap_or_else(|| "100,10000".into())
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    let mib: usize = env("EMUWIZ_OUROBOROS_BENCH_COPY_MIB")
        .map(|s| s.parse().unwrap())
        .unwrap_or(256);
    let count: usize = env("EMUWIZ_OUROBOROS_BENCH_JOURNALS")
        .map(|s| s.parse().unwrap())
        .unwrap_or(200);
    // Everything below runs inside a fresh folder this test creates, so its
    // fixed working names can never match, or remove, anything that was
    // already there (even if the folder gains entries after the check).
    let work = tempfile::Builder::new()
        .prefix("ouroboros-bench-")
        .tempdir_in(&base)
        .unwrap();
    let base = work.path().to_path_buf();
    planning(&base, &sizes);
    match env("EMUWIZ_OUROBOROS_BENCH_SECOND_DIR") {
        Some(second) => throughput(&base, Path::new(&second), mib),
        None => println!(
            "\n## Cross-filesystem throughput: SKIPPED (no EMUWIZ_OUROBOROS_BENCH_SECOND_DIR)"
        ),
    }
    journals(&base, count);
    contention(&base);
}
