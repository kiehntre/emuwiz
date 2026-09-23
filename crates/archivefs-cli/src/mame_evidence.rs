//! Manual, incremental MAME physical-member evidence refresh.

use std::path::PathBuf;

use archivefs_core::dat::mame_arcade_join::{
    MAME_0174_SHA256, MameEvidenceRefreshReport, load_verified_mame_0174,
    refresh_mame_member_evidence,
};
use archivefs_core::{Database, default_database_path};

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    match args.first().map(String::as_str) {
        Some("evidence") => evidence(args.into_iter().skip(1).collect()),
        _ => Err("mame evidence <refresh|status> ...".into()),
    }
}

fn evidence(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    match args.first().map(String::as_str) {
        Some("refresh") => refresh(args.into_iter().skip(1).collect()),
        Some("status") => status(),
        _ => Err("mame evidence <refresh|status> ...".into()),
    }
}

fn refresh(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut root = None;
    let mut dat_path = None;
    let mut set = None;
    let mut json = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--dat" => dat_path = args.next().map(PathBuf::from),
            "--set" => set = args.next(),
            "--json" => json = true,
            other => return Err(format!("unknown mame evidence refresh option: {other}").into()),
        }
    }
    let root = root.ok_or("mame evidence refresh requires --root <path>")?;
    let dat_path = dat_path.ok_or("mame evidence refresh requires --dat <path>")?;
    let dat = load_verified_mame_0174(&dat_path)?;
    let database_path = default_database_path()?;
    let mut database = Database::open_or_create(&database_path)?;
    eprintln!(
        "Refreshing MAME member evidence under {}{}; unchanged members reuse exact path/size/mtime cache rows.",
        root.display(),
        set.as_deref().map_or(String::new(), |name| format!(" for family {name}"))
    );
    let report = refresh_mame_member_evidence(&mut database, &dat, &root, set.as_deref())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    Ok(())
}

fn print_report(report: &MameEvidenceRefreshReport) {
    println!(
        "published {} sets; {} members seen; {} reused; {} rehashed; {} actionable; {} failed; {} cache rows published",
        report.sets_published,
        report.members_seen,
        report.members_reused,
        report.members_rehashed,
        report.members_actionable,
        report.members_failed,
        report.cache_rows_published
    );
}

fn status() -> Result<(), Box<dyn std::error::Error>> {
    let database_path = default_database_path()?;
    let database = Database::open_read_only(&database_path)?;
    let status = database.mame_member_evidence_status(MAME_0174_SHA256)?;
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}
