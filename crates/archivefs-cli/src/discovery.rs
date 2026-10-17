//! Read-only native Discovery preparation. Apply remains separately confirmed in Blue.
use archivefs_core::ingestion::explicit_preview::preview_pinned_paths;
use std::{collections::BTreeMap, error::Error, path::PathBuf, sync::atomic::AtomicBool};

pub fn run(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    if args.next().as_deref() != Some("preview-paths") {
        return Err("discovery preview-paths --request FILE --sha256 PIN [--database FILE] [--config FILE] --save FILE\nRead-only, explicit paths only. Save native evidence outside ROM roots; load it in Blue for review. No Apply command.".into());
    }
    let mut options = BTreeMap::new();
    while let Some(key) = args.next() {
        if !matches!(
            key.as_str(),
            "--request" | "--sha256" | "--database" | "--config" | "--save"
        ) || options.contains_key(&key)
        {
            return Err(format!("Unknown or repeated Discovery option: {key}").into());
        }
        options.insert(key, args.next().ok_or("Missing option value")?);
    }
    let request = PathBuf::from(options.remove("--request").ok_or("Missing --request")?);
    let pin = options.remove("--sha256").ok_or("Missing --sha256")?;
    let save = PathBuf::from(options.remove("--save").ok_or("Missing --save")?);
    let database = options
        .remove("--database")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(archivefs_core::default_database_path)?;
    let config = options
        .remove("--config")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(archivefs_core::default_config_path)?;
    let result = preview_pinned_paths(
        &database,
        &config,
        &request,
        &pin,
        &AtomicBool::new(false),
        &mut |n, total| {
            if n % 25 == 0 || n == total {
                eprintln!("Discovery explicit preparation {n}/{total}");
            }
        },
    )?;
    let digest = result.plan.save_evidence(&save)?;
    let rows: Vec<_> = result.plan.selection_rows().into_iter().map(|r| serde_json::json!({
        "id": r.id, "path": r.path, "platform": r.platform, "kind": r.kind,
        "eligible": r.eligible, "alternate": r.alternate, "multipart": r.multipart, "companions": r.companion_count
    })).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "read_only": true, "root": result.plan.root(), "requested": result.requested,
            "inspected_input_bytes": result.inspected_input_bytes,
        "eligible": result.plan.projected_insertions(), "refused": result.rejected,
            "exclusions": result.plan.exclusions(), "catalogues": result.plan.catalogue_basis(),
            "approval_sha256": result.plan.approval_sha256(), "exclusions_sha256": result.plan.exclusions_sha256(),
            "saved_evidence": save, "saved_sha256": digest, "candidates": rows,
            "production_import_performed": false
        }))?
    );
    Ok(())
}
