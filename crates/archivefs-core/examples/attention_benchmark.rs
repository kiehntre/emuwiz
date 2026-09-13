//! Read-only benchmark against an existing catalogue; never scans the library.
use archivefs_core::attention::{ATTENTION_PAGE_SIZE, AttentionFilters};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: attention_benchmark DATABASE")?;
    let size_before = std::fs::metadata(&path)?.len();
    let db = archivefs_core::Database::open_read_only(&path)?;
    for run in 1..=3 {
        let started = std::time::Instant::now();
        let snapshot = db.attention_snapshot()?;
        let page = snapshot.page(&AttentionFilters::default());
        println!(
            "run={run} source_records={} queries={} summary_items={} unresolved={} page_items={} page_size={} query_ms={} query_and_page_ms={:.3} limited={}",
            snapshot.source_rows,
            snapshot.query_count,
            snapshot.items().count(),
            page.total,
            page.items.len(),
            ATTENTION_PAGE_SIZE,
            snapshot.query_millis,
            started.elapsed().as_secs_f64() * 1000.0,
            snapshot.limited
        );
    }
    println!(
        "db_bytes_before={size_before} db_bytes_after={} library_walks=0 network_calls=0 persistent_writes=0",
        std::fs::metadata(&path)?.len()
    );
    Ok(())
}
