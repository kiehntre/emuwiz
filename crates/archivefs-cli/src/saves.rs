//! Headless, read-only saves and states reporting.

use archivefs_core::persistent_state_inventory::{PersistentStateRecord, PersistentStateType};
use archivefs_core::save_state_orchestration::{
    ConfiguredStateInventory, inventory_configured_state,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct SaveRecord<'a> {
    id: String,
    #[serde(flatten)]
    record: &'a PersistentStateRecord,
}

#[derive(Debug, Serialize)]
struct SaveReport<'a> {
    read_only: bool,
    records: Vec<SaveRecord<'a>>,
    unavailable: &'a [archivefs_core::save_state_orchestration::UnavailableStateRoot],
    summary: SaveSummary,
}

#[derive(Debug, Default, Serialize)]
struct SaveSummary {
    game_saves: usize,
    memory_cards: usize,
    savestates: usize,
    system_containers: usize,
    needs_review: usize,
    unavailable_roots: usize,
}

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = args.into_iter();
    let first = args.next().unwrap_or_else(|| "summary".into());
    let (subcommand, mut json) = if first == "--json" {
        ("summary".into(), true)
    } else {
        (first, false)
    };
    let mut verbose = false;
    let mut emulator = None;
    let mut state_type = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--verbose" | "-v" => verbose = true,
            "--emulator" => emulator = args.next(),
            "--type" => state_type = args.next(),
            other if other.starts_with("--emulator=") => emulator = Some(other[11..].to_string()),
            other if other.starts_with("--type=") => state_type = Some(other[7..].to_string()),
            other if subcommand == "inspect" && !other.starts_with('-') => {
                if emulator.is_none() {
                    emulator = Some(other.to_string());
                }
            }
            other => return Err(format!("unknown saves argument {other:?}").into()),
        }
    }
    if subcommand != "summary" && subcommand != "list" && subcommand != "inspect" {
        return Err(format!(
            "unknown saves sub-command {subcommand:?} (expected summary, list, or inspect)"
        )
        .into());
    }
    let report = inventory_configured_state();
    let summary = summarize(&report, emulator.as_deref(), state_type.as_deref());
    let records = report
        .inventory
        .records
        .iter()
        .enumerate()
        .filter(|(_, record)| matches_filter(record, emulator.as_deref(), state_type.as_deref()))
        .map(|(index, record)| SaveRecord {
            id: record_id(index, record),
            record,
        })
        .collect::<Vec<_>>();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&SaveReport {
                read_only: report.read_only,
                records,
                unavailable: &report.unavailable,
                summary
            })?
        );
    } else if subcommand == "inspect" {
        let Some(id) = emulator else {
            return Err("saves inspect requires a record id".into());
        };
        let (index, record) = report
            .inventory
            .records
            .iter()
            .enumerate()
            .find(|(index, record)| record_id(*index, record) == id)
            .ok_or_else(|| format!("save/state record {id:?} was not found"))?;
        print_record(&record_id(index, record), record, true);
    } else if subcommand == "summary" {
        print_summary(&summary);
    } else {
        print_list(&records, verbose);
    }
    Ok(())
}

fn summarize(
    report: &ConfiguredStateInventory,
    emulator: Option<&str>,
    state_type: Option<&str>,
) -> SaveSummary {
    let mut out = SaveSummary {
        unavailable_roots: report.unavailable.len(),
        ..Default::default()
    };
    for record in report
        .inventory
        .records
        .iter()
        .filter(|r| matches_filter(r, emulator, state_type))
    {
        match record.state_type {
            PersistentStateType::NativeSave => out.game_saves += 1,
            PersistentStateType::MemoryCard => out.memory_cards += 1,
            PersistentStateType::SaveState => out.savestates += 1,
            PersistentStateType::NandOrVirtualDisk => out.system_containers += 1,
            _ => {}
        }
        if matches!(
            record.portability_class,
            archivefs_core::persistent_state_inventory::PortabilityClass::NeedsReview
                | archivefs_core::persistent_state_inventory::PortabilityClass::DoNotTouch
        ) {
            out.needs_review += 1;
        }
    }
    out
}

fn matches_filter(
    record: &PersistentStateRecord,
    emulator: Option<&str>,
    state_type: Option<&str>,
) -> bool {
    emulator.is_none_or(|wanted| record.emulator.as_str().eq_ignore_ascii_case(wanted))
        && state_type.is_none_or(|wanted| {
            format!("{:?}", record.state_type).eq_ignore_ascii_case(wanted)
                || format!("{:?}", record.state_type)
                    .replace('_', "")
                    .eq_ignore_ascii_case(&wanted.replace('_', ""))
        })
}

fn record_id(index: usize, _record: &PersistentStateRecord) -> String {
    format!("state-{index:04}")
}

fn print_summary(summary: &SaveSummary) {
    println!("Saves & States");
    println!("  Game saves       {}", summary.game_saves);
    println!("  Memory cards     {}", summary.memory_cards);
    println!("  Savestates       {}", summary.savestates);
    println!("  System storage   {}", summary.system_containers);
    println!("  Needs review     {}", summary.needs_review);
    println!("  Unavailable roots {}", summary.unavailable_roots);
}

fn print_list(records: &[SaveRecord<'_>], verbose: bool) {
    println!("Saves & States ({})", records.len());
    for item in records {
        print_record(&item.id, item.record, verbose);
    }
}

fn print_record(id: &str, record: &PersistentStateRecord, detailed: bool) {
    println!(
        "{}: {} {:?} ({:?})",
        id,
        record.emulator.as_str(),
        record.state_type,
        record.portability_class
    );
    if detailed {
        println!("  Path: {}", record.path.display());
    }
    if let Some(profile) = &record.selected_installation {
        println!(
            "  Installation: {}{}",
            profile.installation_id,
            if profile.selected { " (selected)" } else { "" }
        );
    }
    if detailed && !record.warnings.is_empty() {
        println!("  Warnings: {}", record.warnings.join("; "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn type_filter_is_case_insensitive() {
        let record = PersistentStateRecord {
            emulator: archivefs_core::persistent_state_inventory::StateEmulator::Pcsx2,
            selected_installation: None,
            state_type: PersistentStateType::SaveState,
            game_identity: Vec::new(),
            path: "/tmp/x".into(),
            container_path: None,
            slot_profile_account: None,
            emulator_version: None,
            firmware_context: None,
            portability_class:
                archivefs_core::persistent_state_inventory::PortabilityClass::EmulatorBound,
            source_path_origin: archivefs_core::persistent_state_inventory::StatePathOrigin::Native,
            provenance: "test".into(),
            sha256: None,
            size_bytes: 0,
            warnings: Vec::new(),
        };
        assert!(matches_filter(&record, Some("pcsx2"), Some("savestate")));
    }
}
