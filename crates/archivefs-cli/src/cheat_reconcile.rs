//! Read-only cross-source cheat reconciliation report.
//!
//! The command consumes a JSON array of already-ingested reconciliation entries.  The
//! core service remains the authority for identity and relationship safety; this
//! module only handles argument validation and presentation.

use archivefs_core::patch_manager::{
    CheatReconciliationEntry, CheatReconciliationGroup, CheatReconciliationOutcome,
    CheatRelationship, reconcile_cheats_for_game,
};
use std::fs;
use std::path::PathBuf;

const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;

pub fn run(mut args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let json = take_flag(&mut args, "--json");
    let relationship = take_named(&mut args, "--relationship")?;
    let input = take_named(&mut args, "--input")?.or_else(|| args.first().cloned());
    if input.is_none() {
        return Err("cheats reconcile requires <entries.json> (or --input <path>)".into());
    }
    if args
        .iter()
        .any(|arg| arg != input.as_deref().unwrap_or_default())
    {
        return Err(format!("cheats reconcile does not accept {args:?}").into());
    }
    let path = PathBuf::from(input.expect("checked above"));
    let bytes = fs::read(&path).map_err(|error| {
        format!(
            "could not read reconciliation input {}: {error}",
            path.display()
        )
    })?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "reconciliation input exceeds the {} MiB bound",
            MAX_INPUT_BYTES / (1024 * 1024)
        )
        .into());
    }
    let entries: Vec<CheatReconciliationEntry> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid reconciliation JSON {}: {error}", path.display()))?;
    let outcome = reconcile_cheats_for_game(entries);
    match outcome {
        CheatReconciliationOutcome::Unavailable { reason } => {
            Err(format!("reconciliation unavailable: {reason}").into())
        }
        CheatReconciliationOutcome::Ready(result) => {
            let filtered = relationship
                .as_deref()
                .map(parse_relationship)
                .transpose()?;
            if json {
                let value = if let Some(filter) = filtered {
                    let mut value = serde_json::to_value(&result)?;
                    if let Some(groups) = value.get_mut("groups").and_then(|v| v.as_array_mut()) {
                        groups.retain(|group| {
                            group.get("relationship").and_then(|v| v.as_str())
                                == Some(relationship_name(&filter))
                        });
                    }
                    value
                } else {
                    serde_json::to_value(&result)?
                };
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                print_report(&result, filtered);
            }
            Ok(())
        }
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == flag) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_named(
    args: &mut Vec<String>,
    flag: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(index) = args.iter().position(|arg| arg == flag) else {
        return Ok(None);
    };
    args.remove(index);
    if index >= args.len() {
        return Err(format!("{flag} requires a value").into());
    }
    Ok(Some(args.remove(index)))
}

fn parse_relationship(value: &str) -> Result<CheatRelationship, Box<dyn std::error::Error>> {
    match value.to_ascii_lowercase().as_str() {
        "duplicates" | "exact" | "exact-semantic-duplicate" => {
            Ok(CheatRelationship::ExactSemanticDuplicate)
        }
        "raw-duplicates" | "exact-raw-duplicate" => Ok(CheatRelationship::ExactRawDuplicate),
        "conflicts" | "same-title-different-code" => Ok(CheatRelationship::SameTitleDifferentCode),
        "related" | "related-unproven" => Ok(CheatRelationship::RelatedUnproven),
        "unique" => Ok(CheatRelationship::Unique),
        _ => Err(format!(
            "unknown relationship {value:?}; expected duplicates, conflicts, related, or unique"
        )
        .into()),
    }
}

fn relationship_name(value: &CheatRelationship) -> &'static str {
    match value {
        CheatRelationship::ExactSemanticDuplicate => "ExactSemanticDuplicate",
        CheatRelationship::ExactRawDuplicate => "ExactRawDuplicate",
        CheatRelationship::SameTitleDifferentCode => "SameTitleDifferentCode",
        CheatRelationship::RelatedUnproven => "RelatedUnproven",
        CheatRelationship::Unique => "Unique",
    }
}

fn print_report(
    result: &archivefs_core::patch_manager::CheatReconciliationResult,
    filter: Option<CheatRelationship>,
) {
    println!("Game: {}", result.game_identity);
    println!("Platform: {:?}", result.platform);
    println!();
    let groups = result
        .groups
        .iter()
        .filter(|group| {
            filter
                .as_ref()
                .is_none_or(|value| value == &group.relationship)
        })
        .collect::<Vec<_>>();
    let count = |relationship| {
        result
            .groups
            .iter()
            .filter(|group| group.relationship == relationship)
            .count()
    };
    println!(
        "Exact duplicates: {} groups",
        count(CheatRelationship::ExactSemanticDuplicate)
            + count(CheatRelationship::ExactRawDuplicate)
    );
    println!(
        "Conflicts: {}",
        count(CheatRelationship::SameTitleDifferentCode)
    );
    println!(
        "Related/unproven: {}",
        count(CheatRelationship::RelatedUnproven)
    );
    println!("Unique: {}", count(CheatRelationship::Unique));
    println!();
    println!("No auto-winner.");
    for group in groups {
        print_group(group, &result.entries);
    }
}

fn print_group(group: &CheatReconciliationGroup, entries: &[CheatReconciliationEntry]) {
    let title = entries
        .get(group.entry_indices[0])
        .map(|entry| entry.title.as_str())
        .unwrap_or(&group.normalized_title);
    println!();
    println!("{title}");
    println!("  {}", relationship_label(&group.relationship));
    if !group.differences.is_empty() {
        for difference in &group.differences {
            println!("  Difference: {difference}");
        }
    }
    println!("  Sources:");
    for index in &group.entry_indices {
        if let Some(entry) = entries.get(*index) {
            println!("    - {}", entry.source);
            for provenance in &entry.provenance {
                println!("      {provenance}");
            }
        }
    }
}

fn relationship_label(relationship: &CheatRelationship) -> &'static str {
    match relationship {
        CheatRelationship::ExactSemanticDuplicate => "Exact semantic duplicate",
        CheatRelationship::ExactRawDuplicate => "Exact raw duplicate",
        CheatRelationship::SameTitleDifferentCode => "Conflict: same title, different code",
        CheatRelationship::RelatedUnproven => "Related, unproven",
        CheatRelationship::Unique => "Unique",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relationship_filter_names_are_stable() {
        assert_eq!(
            relationship_name(&CheatRelationship::ExactRawDuplicate),
            "ExactRawDuplicate"
        );
        assert!(parse_relationship("conflicts").is_ok());
        assert!(parse_relationship("nope").is_err());
    }
}
