//! Read-only promotion evidence. No apply switch, migrations or scan writes.
use archivefs_core::{
    Database, catalogue_health::preview_catalogue_health, load_source_folder_configs_from,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: catalogue_health_preview DATABASE CONFIG".into());
    }
    let start = std::time::Instant::now();
    let database = Database::open_catalogue_health_read_only(&args[0])?;
    let sources = load_source_folder_configs_from(std::path::Path::new(&args[1]))?;
    let report = preview_catalogue_health(
        &database,
        &sources.into_iter().map(|s| s.path).collect::<Vec<_>>(),
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"schema_version":database.schema_version()?,"read_only":true,"counts":report.counts,"elapsed_seconds":start.elapsed().as_secs_f64()})
        )?
    );
    Ok(())
}
