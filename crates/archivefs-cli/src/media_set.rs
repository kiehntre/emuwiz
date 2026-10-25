use archivefs_core::{
    media_set::*,
    safe_read::{TrustedRoots, open_bounded_read},
};
use std::{
    error::Error,
    path::{Path, PathBuf},
};

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Box<dyn Error>> {
    let mut file = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.detail())?;
    const LIMIT: usize = 128 * 1024 * 1024;
    if file.len() > LIMIT as u64 {
        return Err("Diagnostic JSON exceeds 128 MiB".into());
    }
    let bytes = file
        .read_exact_at(0, file.len() as usize, LIMIT)
        .ok_or("Could not read diagnostic JSON within bounds")?;
    Ok(serde_json::from_slice(&bytes)?)
}
pub fn run(args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let mut args = args.peekable();
    let operation = args.next().unwrap_or_else(|| "help".into());
    if matches!(operation.as_str(), "help" | "--help" | "-h") {
        println!(
            "media-set <inspect|explain|plan> <paths...> [--platform NAME] [--catalogue records.json] [--profile profile.json] [--max-files N] [--max-read-bytes N] [--no-optical-native]\nRead-only JSON diagnostics. No files, playlists or launch commands are written. Catalogue input is an explicit evidence snapshot; no DAT authority is loaded."
        );
        return Ok(());
    }
    if !matches!(operation.as_str(), "inspect" | "explain" | "plan") {
        return Err("Unknown media-set command".into());
    }
    let mut paths = Vec::new();
    let mut platform = None;
    let mut catalogue = None;
    let mut profile = None;
    let mut limits = InspectionLimits::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--platform" => platform = Some(args.next().ok_or("--platform requires a name")?),
            "--catalogue" => {
                catalogue = Some(PathBuf::from(
                    args.next().ok_or("--catalogue requires a path")?,
                ))
            }
            "--profile" => {
                profile = Some(read_json::<MediaProfile>(Path::new(
                    &args.next().ok_or("--profile requires a path")?,
                ))?)
            }
            "--max-files" => {
                limits.max_files = args.next().ok_or("--max-files requires a count")?.parse()?
            }
            "--max-read-bytes" => {
                limits.max_read_bytes = args
                    .next()
                    .ok_or("--max-read-bytes requires a count")?
                    .parse()?
            }
            "--no-optical-native" => limits.native_optical = false,
            "--json" => {}
            "--" => {
                paths.extend(args.map(PathBuf::from));
                break;
            }
            _ if arg.starts_with('-') => return Err(format!("Unknown option {arg}").into()),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if operation == "explain" && paths.len() != 1 {
        return Err(
            "media-set explain requires exactly one path, optionally with --catalogue".into(),
        );
    }
    if paths.is_empty() && catalogue.is_none() {
        return Err("Supply explicit media paths or --catalogue".into());
    }
    let mut records = if let Some(path) = catalogue {
        read_json::<Vec<MediaRecord>>(&path)?
    } else {
        inspect_paths(&paths, platform.as_deref(), &TrustedRoots::none(), &limits)
    };
    if records.len() > 100_000 {
        return Err(
            "Diagnostic input exceeds 100,000 records; use the core API for larger catalogues"
                .into(),
        );
    }
    if let Some(platform) = platform {
        for r in &mut records {
            if r.platform.is_none() {
                r.platform = Some(platform.clone());
            }
        }
    }
    let mut report = resolve_index(index_media(records));
    if operation == "explain" {
        let target = &paths[0];
        report.sets.retain(|s| {
            s.members
                .iter()
                .flat_map(|m| &m.representations)
                .any(|r| &r.record.source.path == target)
        });
        if report.sets.is_empty() {
            return Err("The requested path is absent from the supplied topology records".into());
        }
    }
    if operation == "plan" {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &report
                    .sets
                    .iter()
                    .map(|s| media_swap_plan(s, profile.as_ref()))
                    .collect::<Vec<_>>()
            )?
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}
