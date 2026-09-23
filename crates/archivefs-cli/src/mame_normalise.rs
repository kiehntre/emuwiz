//! Read-only MAME DAT normaliser preview.

use std::path::PathBuf;

use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::mame_normalizer::{
    MameCollectionMode, MameNormalisationPlan, detect_mame_collection_mode,
    plan_mame_normalisation, plan_mame_normalisation_from_verified_joins,
};
use archivefs_core::dat::parsers::parse_dat_file;
use archivefs_core::{Database, default_database_path};
use sha2::{Digest, Sha256};

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let command = args.first().map(String::as_str).unwrap_or("");
    if !matches!(command, "preview" | "apply" | "verify" | "undo" | "recover") {
        return Err("mame-normalise <preview|apply|verify|undo|recover> ...".into());
    }
    if matches!(command, "undo" | "recover") {
        let journal = args.get(1).ok_or("journal path required")?;
        if command == "undo" {
            let count = archivefs_core::dat::mame_normalizer::undo_mame_normalisation(
                &PathBuf::from(journal),
            )?;
            println!("Undid {count} MAME set repairs.");
        } else {
            let count = archivefs_core::dat::mame_normalizer::recover_mame_normalisation(
                &PathBuf::from(journal),
            )?;
            println!("Recovered interrupted MAME repair ({count} journal entries restored).");
        }
        return Ok(());
    }
    if args.len() < 5 {
        return Err("mame-normalise <preview|apply|verify> <mame-folder> <dat-file> <split> <set> [--journal <path>]".into());
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
    let mut plan = if let Ok(database_path) = default_database_path()
        && let Ok(database) = Database::open_read_only(&database_path)
        && let Ok(joins) = database.mame_arcade_join_paths_for_dat(&dat_sha256)
        && !joins.is_empty()
    {
        plan_mame_normalisation_from_verified_joins(&root, &dat.dat, &joins, mode)?
    } else {
        plan_mame_normalisation(&root, &dat.dat, mode)?
    };
    let set_name = &args[4];
    let paths = plan
        .split_rebuilds
        .iter()
        .filter(|rebuild| {
            rebuild
                .parent_path
                .file_name()
                .is_some_and(|name| name.to_string_lossy() == set_name.as_str())
                || rebuild
                    .clone_path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy() == set_name.as_str())
                || rebuild
                    .parent_target
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy() == set_name.as_str())
                || rebuild
                    .clone_target
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy() == set_name.as_str())
        })
        .flat_map(|rebuild| [rebuild.parent_path.clone(), rebuild.clone_path.clone()])
        .collect::<std::collections::BTreeSet<_>>();
    plan = restrict_to_paths(plan, &paths);
    if command == "preview" {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else if command == "apply" {
        let journal = args
            .windows(2)
            .find(|pair| pair[0] == "--journal")
            .map(|pair| PathBuf::from(&pair[1]))
            .unwrap_or_else(|| root.join(".emuwiz-mame-normaliser.json"));
        let count =
            archivefs_core::dat::mame_normalizer::apply_mame_normalisation(&plan, &journal)?;
        println!(
            "Applied {count} MAME set repairs; journal: {}",
            journal.display()
        );
    } else {
        let count = archivefs_core::dat::mame_normalizer::verify_mame_normalisation(&plan)?;
        println!("Verified {count} published Split members.");
    }
    Ok(())
}

fn restrict_to_paths(
    mut plan: MameNormalisationPlan,
    paths: &std::collections::BTreeSet<PathBuf>,
) -> MameNormalisationPlan {
    plan.split_rebuilds.retain(|rebuild| {
        paths.contains(&rebuild.parent_path) && paths.contains(&rebuild.clone_path)
    });
    plan.sets.retain(|set| paths.contains(&set.current_path));
    plan.summary.total_sets = plan.sets.len();
    plan.summary.split_rebuilds = plan.split_rebuilds.len();
    plan.summary.moved_members = plan
        .split_rebuilds
        .iter()
        .map(|rebuild| rebuild.moved_members)
        .sum();
    plan
}
