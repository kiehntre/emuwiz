//! Headless PlayStation memory-card manager.
//!
//! Reads cards, makes verified backups, exports single saves and builds a
//! copy of a card with a save added. It never writes to a card it is given:
//! every result is a new file, and an existing file is never overwritten.
//! It does not open the library database.

use archivefs_core::memory_card_manager::{
    self as manager, CardReport,
    backup::{backup_card, default_backup_root, verify_backup},
    discovery::find_configured_cards,
    external_tools::find_external_tools,
};
use std::path::{Path, PathBuf};

const USAGE: &str = "\
emuwiz-cli memory-cards <command>

  find [--json]                         PS1/PS2 cards in the emulators' save folders
  list <card> [--json]                  what is on a card (read-only)
  backup <card> [--to <folder>]         verified copy in EmuWiz's backup folder
  verify <backup-record.json>           re-check a backup against its record and its card
  export <card> <save-name> <new-file>  one save as .mcs (PS1) or .psu (PS2)
  import <card> <save-file> <new-card>  write <new-card>: the card plus that save
  tools                                 separately installed editors EmuWiz can start

A card is never modified. `import` reads <card> and writes <new-card>, which must
not exist. PS1 saves: .mcs, .psv. PS2 saves: .psu.";

type Outcome = Result<(), Box<dyn std::error::Error>>;

pub fn run(args: Vec<String>) -> Outcome {
    let json = args.iter().any(|arg| arg == "--json");
    let mut to = None;
    let mut positional = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // Asking for help must only ever print help.
            "--help" | "-h" | "help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--json" => {}
            "--to" => to = Some(args.next().ok_or("--to needs a folder")?),
            other if other.starts_with("--") => {
                return Err(format!("unknown memory-cards option {other:?}\n\n{USAGE}").into());
            }
            _ => positional.push(arg),
        }
    }
    let mut positional = positional.into_iter();
    let Some(command) = positional.next() else {
        println!("{USAGE}");
        return Ok(());
    };
    let rest: Vec<String> = positional.collect();
    let path = |index: usize, what: &str| -> Result<PathBuf, String> {
        rest.get(index)
            .map(|value| absolute(Path::new(value)))
            .ok_or_else(|| format!("memory-cards {command} needs {what}\n\n{USAGE}"))
    };
    match command.as_str() {
        "find" => {
            let search = find_configured_cards();
            if json {
                println!("{}", serde_json::to_string_pretty(&search)?);
                return Ok(());
            }
            for card in &search.cards {
                println!(
                    "{}\n    {} · {}",
                    card.path.display(),
                    card.emulator.as_deref().unwrap_or("unknown emulator"),
                    card.format.label()
                );
            }
            for line in &search.unavailable {
                println!("not readable: {line}");
            }
            println!("{} card(s) found. Nothing was changed.", search.cards.len());
        }
        "list" => {
            let report = manager::read_card(&path(0, "a card")?)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_report(&report);
            }
        }
        "backup" => {
            let root = match to {
                Some(folder) => absolute(Path::new(&folder)),
                None => default_backup_root()?,
            };
            let backup = backup_card(&path(0, "a card")?, &root)?;
            println!(
                "Backup saved and verified.\n  card:    {}\n  backup:  {}\n  record:  {}\n  \
                 SHA-256: {}\n  saves:   {}\nThe card was only read.",
                backup.source.display(),
                backup.backup_file.display(),
                backup.record_file().display(),
                backup.sha256,
                backup.save_names.len()
            );
        }
        "verify" => {
            let check = verify_backup(&path(0, "a backup record (.json)")?)?;
            println!("{}", check.detail);
            if !check.backup_intact {
                return Err("the backup is not intact".into());
            }
        }
        "export" => {
            let save = rest
                .get(1)
                .ok_or_else(|| format!("memory-cards export needs a save name\n\n{USAGE}"))?;
            let export = manager::export_save(&path(0, "a card")?, save, &path(2, "a new file")?)?;
            println!(
                "Exported {} to {} (.{}, {} bytes, SHA-256 {}).\nThe card was only read.",
                export.save_name,
                export.destination.display(),
                export.format,
                export.bytes,
                export.sha256
            );
        }
        "import" => {
            let import = manager::import_save_into_copy(
                &path(0, "a card")?,
                &path(1, "a save file")?,
                &path(2, "a new card file")?,
            )?;
            println!(
                "New card written: {}\n  save added: {}\n  kind:       {}\n  SHA-256:    {}",
                import.new_card.display(),
                import.save_name,
                import.new_card_format.label(),
                import.new_card_sha256
            );
            for check in &import.checks {
                println!("  checked:    {check}");
            }
            println!(
                "The original card {} was not changed.",
                import.source_card.display()
            );
        }
        "tools" => {
            let tools = find_external_tools();
            if json {
                println!("{}", serde_json::to_string_pretty(&tools)?);
                return Ok(());
            }
            for tool in &tools {
                println!(
                    "{} ({}, {}): {}",
                    tool.name,
                    tool.handles,
                    tool.licence,
                    tool.executable.display()
                );
            }
            if tools.is_empty() {
                println!(
                    "Neither myMCpp nor PSMemoryManager is installed. They are optional, \
                     separate GPL programs; EmuWiz does not include them."
                );
            }
        }
        other => {
            return Err(format!("unknown memory-cards command {other:?}\n\n{USAGE}").into());
        }
    }
    Ok(())
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn print_report(report: &CardReport) {
    println!("{}", report.path.display());
    println!("  kind:    {}", report.format.label());
    println!("  size:    {} bytes", report.size_bytes);
    println!(
        "  state:   {}",
        match report.health {
            manager::CardHealth::Healthy => "readable (two readers agree)",
            manager::CardHealth::Warnings => "read with warnings",
            manager::CardHealth::Unreadable => "not readable",
        }
    );
    if let Some(free) = &report.free_space {
        println!("  free:    {free}");
    }
    if let Some(sha256) = &report.sha256 {
        println!("  SHA-256: {sha256}");
    }
    for note in &report.notes {
        println!("  note:    {note}");
    }
    println!("  saves:   {}", report.saves.len());
    for save in &report.saves {
        let size = match (save.blocks, save.file_count) {
            (Some(blocks), _) => format!("{blocks} block(s)"),
            (None, Some(files)) => format!("{} bytes in {files} file(s)", save.size_bytes),
            (None, None) => format!("{} bytes", save.size_bytes),
        };
        println!(
            "    {:<34} {:<11} {:<8} {}{}",
            save.name,
            save.product_code.as_deref().unwrap_or("-"),
            save.region.unwrap_or("-"),
            size,
            save.title
                .as_deref()
                .map(|title| format!("  \"{title}\""))
                .unwrap_or_default()
        );
    }
    match report.export_format {
        Some(format) => println!("  export:  single saves as .{format}"),
        None => println!("  export:  not available for this card"),
    }
    if report.import_formats.is_empty() {
        println!("  import:  not available for this card");
    } else {
        println!(
            "  import:  {} into a new copy of the card",
            report
                .import_formats
                .iter()
                .map(|format| format!(".{format}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for line in &report.unsupported {
        println!("  not supported: {line}");
    }
}
