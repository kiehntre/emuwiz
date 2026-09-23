//! Read-only MAME DAT normaliser preview.

use std::path::PathBuf;

use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::mame_normalizer::{
    MameCollectionMode, detect_mame_collection_mode, plan_mame_normalisation,
    plan_mame_normalisation_from_verified_joins,
};
use archivefs_core::dat::parsers::parse_dat_file;
use archivefs_core::{Database, default_database_path};
use sha2::{Digest, Sha256};

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 4 || args[0] != "preview" {
        return Err(
            "mame-normalise preview <mame-folder> <dat-file> <merged|split|non-merged|not-sure>"
                .into(),
        );
    }
    let mode = match args[3].as_str() {
        "merged" => MameCollectionMode::Merged,
        "split" => MameCollectionMode::Split,
        "non-merged" => MameCollectionMode::NonMerged,
        "not-sure" => MameCollectionMode::NotSure,
        other => return Err(format!("unknown MAME collection mode: {other}").into()),
    };
    let dat = parse_dat_file(&PathBuf::from(&args[2]), DatLimits::default())?;
    let root = PathBuf::from(&args[1]);
    let mode = if mode == MameCollectionMode::NotSure {
        let detected = detect_mame_collection_mode(&root, &dat.dat)?;
        eprintln!(
            "Detected MAME collection layout: {detected:?}; preview is read-only and still requires user confirmation before mutation."
        );
        detected
    } else {
        mode
    };
    let dat_sha256 = Sha256::digest(std::fs::read(&args[2])?);
    let dat_sha256 = dat_sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let plan = if let Ok(database_path) = default_database_path()
        && let Ok(database) = Database::open_read_only(&database_path)
        && let Ok(joins) = database.mame_arcade_join_paths_for_dat(&dat_sha256)
        && !joins.is_empty()
    {
        plan_mame_normalisation_from_verified_joins(&root, &dat.dat, &joins, mode)?
    } else {
        plan_mame_normalisation(&root, &dat.dat, mode)?
    };
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}
