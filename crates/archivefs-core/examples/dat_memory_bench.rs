//! Opt-in large-DAT memory/time benchmark and semantic-golden digest. Never
//! run by `cargo test`; drive it under `/usr/bin/time -v` (see
//! `scripts/bench-large-dat-memory.sh`). All data is synthetic and written
//! under the directory given on the command line.
//!
//! ```text
//! dat_memory_bench gen    <dir> <records>   # write <dir>/synthetic-<records>.dat
//! dat_memory_bench parse  <dir> <records>
//! dat_memory_bench persist <dir> <records>  # validate_dat_source + replace_expected_dat_inventory
//! dat_memory_bench index  <dir> <records>   # parse + DatIndex::build + lookups
//! dat_memory_bench golden <dir>             # mixed duplicate/conflict corpus digest
//! ```

use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

use archivefs_core::Database;
use archivefs_core::dat::index::DatIndex;
use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::parsers::parse_dat_file;
use archivefs_core::dat::sources::validation::validate_dat_source;
use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind};

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn hex(seed: u64, bytes: usize) -> String {
    let mut out = String::with_capacity(bytes * 2);
    let mut state = seed;
    while out.len() < bytes * 2 {
        state = mix(state);
        let _ = write!(out, "{state:016x}");
    }
    out.truncate(bytes * 2);
    out
}

fn limits() -> DatLimits {
    DatLimits::builder()
        .max_file_size(2 << 30)
        .max_entries(2_000_000)
        .build()
}

fn dat_path(dir: &Path, records: usize) -> PathBuf {
    dir.join(format!("synthetic-{records}.dat"))
}

fn gen_main(dir: &Path, records: usize) {
    use std::io::Write;
    let mut out = std::io::BufWriter::new(std::fs::File::create(dat_path(dir, records)).unwrap());
    writeln!(out, "<?xml version=\"1.0\"?>\n<datafile>\n<header><name>Synthetic Bench</name><description>Synthetic</description><version>1</version></header>").unwrap();
    for i in 0..records as u64 {
        writeln!(
            out,
            "<game name=\"game{i:07}\"><description>Synthetic Game {i:07}</description><rom name=\"rom{i:07}_0.bin\" size=\"1048576\" crc=\"{}\" md5=\"{}\" sha1=\"{}\"/></game>",
            hex(i * 3, 4),
            hex(i * 3 + 1, 16),
            hex(i * 3 + 2, 20)
        )
        .unwrap();
    }
    writeln!(out, "</datafile>").unwrap();
}

/// Duplicate-heavy corpus: shared hashes, shared filenames with different
/// hashes, case variants, multi-ROM games, missing hashes, a malformed ROM.
fn golden_corpus(dir: &Path) -> PathBuf {
    use std::io::Write;
    let path = dir.join("golden.dat");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    writeln!(out, "<?xml version=\"1.0\"?>\n<datafile>\n<header><name>Golden</name><version>1</version></header>").unwrap();
    for i in 0..3000u64 {
        let shared = if i % 10 == 9 { i - 1 } else { i };
        let name = if i % 7 == 6 {
            "SHARED.BIN".to_string()
        } else if i % 5 == 4 {
            format!("Mixed{}.Bin", i % 50)
        } else {
            format!("rom{i}.bin")
        };
        let sha1 = if i % 11 == 10 {
            String::new()
        } else {
            format!(" sha1=\"{}\"", hex(shared * 3 + 2, 20))
        };
        let sha256 = if i % 13 == 0 {
            format!(" sha256=\"{}\"", hex(i + 99, 32))
        } else {
            String::new()
        };
        let extra = if i % 4 == 0 {
            format!("<rom name=\"extra{i}.bin\" size=\"16\" crc=\"{}\"/>", hex(i + 7, 4))
        } else {
            String::new()
        };
        let bad = "";
        let clone = if i % 9 == 8 {
            format!(" cloneof=\"game{}\"", i - 1)
        } else {
            String::new()
        };
        writeln!(
            out,
            "<game name=\"game{i}\"{clone}><description>Game {i}</description><rom name=\"{name}\" size=\"{}\" crc=\"{}\" md5=\"{}\"{sha1}{sha256}/>{extra}{bad}</game>",
            1024 + i,
            hex(shared * 3, 4),
            hex(shared * 3 + 1, 16)
        )
        .unwrap();
    }
    writeln!(out, "</datafile>").unwrap();
    path
}

fn digest(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn vm(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or(0)
}

fn report(phase: &str, started: Instant) {
    println!(
        "phase={phase} secs={:.3} rss_kib={} hwm_kib={}",
        started.elapsed().as_secs_f64(),
        vm("VmRSS:"),
        vm("VmHWM:")
    );
}

fn lookups(index: &DatIndex, records: usize) {
    let sample = 100_000.min(records.max(1));
    let step = (records / sample).max(1);
    let keys = |f: &dyn Fn(u64) -> String| -> Vec<String> {
        (0..sample).map(|k| f((k * step) as u64)).collect()
    };
    let sha1 = keys(&|i| hex(i * 3 + 2, 20));
    let md5 = keys(&|i| hex(i * 3 + 1, 16));
    let crc = keys(&|i| hex(i * 3, 4));
    let names = keys(&|i| format!("ROM{i:07}_0.BIN"));
    let time = |label: &str, run: &dyn Fn() -> usize| {
        let t = Instant::now();
        let hits = run();
        println!(
            "lookup={label} n={sample} hits={hits} ns_per={:.0}",
            t.elapsed().as_nanos() as f64 / sample as f64
        );
    };
    time("sha1", &|| sha1.iter().map(|k| index.lookup_sha1(k).len()).sum());
    time("md5", &|| md5.iter().map(|k| index.lookup_md5(k).len()).sum());
    time("crc32", &|| crc.iter().map(|k| index.lookup_crc32(k).len()).sum());
    time("filename", &|| {
        names.iter().map(|k| index.lookup_filename(k).len()).sum()
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("");
    let dir = PathBuf::from(args.get(2).expect("dir"));
    std::fs::create_dir_all(&dir).unwrap();
    let records: usize = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
    let t0 = Instant::now();
    match mode {
        "gen" => {
            gen_main(&dir, records);
            report("gen", t0);
        }
        "parse" => {
            let outcome = parse_dat_file(&dat_path(&dir, records), limits()).unwrap();
            println!("games={}", outcome.dat.games.len());
            report("parse", t0);
        }
        "persist" => {
            let entry = DatSourceEntry::new(
                "bench".into(),
                "bench".into(),
                dat_path(&dir, records),
                DatSourceKind::File,
            );
            let (report_, projection) = validate_dat_source(&entry, limits());
            println!("entries={} state={:?}", projection.entries.len(), report_.state);
            report("validate_project", t0);
            let db_path = dir.join(format!("bench-{records}.sqlite"));
            let _ = std::fs::remove_file(&db_path);
            let mut db = Database::open_or_create(&db_path).unwrap();
            let t1 = Instant::now();
            let stored = db
                .replace_expected_dat_inventory(
                    "bench",
                    Some("1"),
                    None,
                    &projection.entries,
                    projection.duplicate_names_skipped as u64,
                )
                .unwrap();
            println!("stored={stored}");
            report("persist", t1);
            drop(db);
            println!("db_bytes={}", std::fs::metadata(&db_path).unwrap().len());
        }
        "index" => {
            let outcome = parse_dat_file(&dat_path(&dir, records), limits()).unwrap();
            report("parse", t0);
            let t1 = Instant::now();
            let index = DatIndex::build(&outcome.dat);
            println!("sha1_keys={}", index.sha1_count());
            report("index_build", t1);
            lookups(&index, records);
            report("done", t0);
        }
        "golden" => {
            let path = golden_corpus(&dir);
            // Malformed input must keep failing closed with the same error.
            let bad = dir.join("golden-bad.dat");
            std::fs::write(&bad, "<?xml version=\"1.0\"?><datafile><header><name>B</name></header><game name=\"g\"><rom name=\"b.bin\" size=\"notanumber\" crc=\"zz\"/></game></datafile>").unwrap();
            println!("malformed={:?}", parse_dat_file(&bad, DatLimits::default()).err());
            let outcome = parse_dat_file(&path, DatLimits::default()).unwrap();
            let index = DatIndex::build(&outcome.dat);
            println!("games={} warnings={}", outcome.dat.games.len(), outcome.warnings.len());
            let mut dump = String::new();
            macro_rules! bucket {
                ($name:literal, $map:expr) => {{
                    let mut keys: Vec<_> = $map.keys().cloned().collect();
                    keys.sort();
                    let mut text = String::new();
                    for k in &keys {
                        let _ = writeln!(text, "{k} => {:?}", $map[k]);
                    }
                    println!("{} keys={} digest={:016x}", $name, keys.len(), digest(&text));
                    dump.push_str(&text);
                }};
            }
            bucket!("crc32", index.by_crc32);
            bucket!("md5", index.by_md5);
            bucket!("sha1", index.by_sha1);
            bucket!("sha256", index.by_sha256);
            bucket!("filename", index.by_filename);
            let mut clone: Vec<_> = index.game_clone_of.iter().collect();
            clone.sort();
            println!("clone_of digest={:016x}", digest(&format!("{clone:?}")));
            // Public lookup API, including case-folded filename and misses.
            let mut api = String::new();
            for i in (0..3000u64).step_by(3) {
                let _ = writeln!(
                    api,
                    "{:?}|{:?}|{:?}|{:?}",
                    index.lookup_sha1(&hex(i * 3 + 2, 20)),
                    index.lookup_md5(&hex(i * 3 + 1, 16)),
                    index.lookup_crc32(&hex(i * 3, 4)),
                    index.lookup_filename(&format!("ROM{i}.BIN")),
                );
            }
            let _ = write!(api, "{:?}{:?}", index.lookup_filename("shared.bin"), index.lookup_sha1("nope"));
            println!("lookup_api digest={:016x}", digest(&api));
            println!("counts {} {} {} {} coll {} {} {} {}",
                index.crc32_count(), index.md5_count(), index.sha1_count(), index.sha256_count(),
                index.crc32_collisions(), index.md5_collisions(), index.sha1_collisions(), index.sha256_collisions());
            let projection = archivefs_core::dat::expected_inventory::project_expected_dat_inventory(&outcome.dat.games);
            println!("projection digest={:016x}", digest(&format!("{:?}", projection.entries)));
        }
        _ => eprintln!("unknown mode"),
    }
}
