//! Measures the selected-family MAME paths against a database copy
//! (`EMUWIZ_DATA_HOME`), never the user's live one.
//!
//! `mame_family_perf <refresh|plan> <before|after> <root> <dat> <set> [out.json]`
//!
//! `before` runs the full-collection shape this crate had before the family
//! scoping (unscoped join load, DAT re-read and re-hash); `after` runs the
//! scoped one. `plan` writes the reconstruction plan as JSON so the two can be
//! diffed byte for byte.

use std::path::Path;
use std::time::Instant;

use archivefs_core::Database;
use archivefs_core::dat::mame_arcade_join::{
    load_verified_mame_0174, refresh_mame_member_evidence,
};
use archivefs_core::dat::mame_merged_reconstruction::{
    build_merged_reconstruction_plan, discover_packed_zip_sources, reconstruction_family_names,
};
use archivefs_core::default_database_path;
use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [what, mode, root, dat_path, set, rest @ ..] = args.as_slice() else {
        return Err(
            "mame_family_perf <refresh|plan> <before|after> <root> <dat> <set> [out]".into(),
        );
    };
    let (root, dat_path) = (Path::new(root), Path::new(dat_path));
    let started = Instant::now();
    let dat = load_verified_mame_0174(dat_path)?;
    eprintln!("load+parse DAT: {:?}", started.elapsed());
    match what.as_str() {
        "refresh" => {
            let mut database = Database::open_or_create(&default_database_path()?)?;
            let t = Instant::now();
            let report = refresh_mame_member_evidence(&mut database, &dat, root, Some(set))?;
            eprintln!("refresh family: {:?}", t.elapsed());
            println!("{}", serde_json::to_string(&report)?);
        }
        "plan" => {
            let after = mode == "after";
            let t = Instant::now();
            let dat_sha256 = if after {
                dat.sha256.clone()
            } else {
                Sha256::digest(std::fs::read(dat_path)?)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            };
            eprintln!("dat digest: {:?}", t.elapsed());
            let database = Database::open_read_only(&default_database_path()?)?;
            let t = Instant::now();
            let mut joins = if after {
                let family = reconstruction_family_names(&dat.parsed, set)?;
                database.mame_arcade_join_paths_for_dat_sets(&dat_sha256, Some(&family))?
            } else {
                database.mame_arcade_join_paths_for_dat(&dat_sha256)?
            };
            eprintln!("load joins: {} rows in {:?}", joins.len(), t.elapsed());
            let t = Instant::now();
            joins.extend(discover_packed_zip_sources(
                root,
                &dat.parsed,
                set,
                &dat_sha256,
            )?);
            eprintln!("packed zip discovery: {:?}", t.elapsed());
            let t = Instant::now();
            let plan =
                build_merged_reconstruction_plan(root, &dat.parsed, &joins, set, &dat_sha256)?;
            eprintln!("build plan: {:?}", t.elapsed());
            if let Some(out) = rest.first() {
                std::fs::write(out, serde_json::to_vec_pretty(&plan)?)?;
            }
        }
        other => return Err(format!("unknown step {other}").into()),
    }
    eprintln!("total: {:?}", started.elapsed());
    Ok(())
}
