//! Explicit manual staging inventory/cleanup. No database or collection scan.
use archivefs_core::dat::mame_merged_reconstruction::staging_lifecycle::{self, StagingItem};
use std::path::PathBuf;

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    const HELP: &str = "mame staging inventory --staging <absolute-path> --journal <absolute-path>\nSave its JSON output for review, then: mame staging cleanup --staging <path> --journal <path> --review <saved-inventory.json> --file <exact-path> --confirm 'DELETE REDUNDANT MAME OUTPUT' --exclusive-access 'I HAVE EXCLUSIVE STAGING AND JOURNAL ACCESS'\nOnly redundant reconstructed outputs after verified Undo can be cleaned. Originals, unresolved recovery copies and unknown files remain. No Undo-surrender option exists. Stop all other writers to staging and journals, including manual file operations, before confirming exclusive access. Linux leases do not prevent an uncooperative process from replacing a directory entry.";
    let mut args = args.into_iter();
    let action = args.next().ok_or(HELP)?;
    let (mut staging, mut journal, mut review, mut file, mut confirmation, mut exclusive_access) =
        (None, None, None, None, None, None);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or(HELP)?;
        match arg.as_str() {
            "--staging" => staging = Some(PathBuf::from(value)),
            "--journal" => journal = Some(PathBuf::from(value)),
            "--review" => review = Some(PathBuf::from(value)),
            "--file" => file = Some(PathBuf::from(value)),
            "--confirm" => confirmation = Some(value),
            "--exclusive-access" => exclusive_access = Some(value),
            _ => return Err(HELP.into()),
        }
    }
    let staging = staging.ok_or(HELP)?;
    let journal = journal.ok_or(HELP)?;
    match action.as_str() {
        "inventory"
            if review.is_none()
                && file.is_none()
                && confirmation.is_none()
                && exclusive_access.is_none() =>
        {
            println!(
                "{}",
                serde_json::to_string_pretty(&staging_lifecycle::inventory(&staging, &journal)?)?
            );
        }
        "cleanup" => {
            let items: Vec<StagingItem> =
                serde_json::from_slice(&std::fs::read(review.ok_or(HELP)?)?)?;
            let path = file.ok_or(HELP)?;
            let mut matching = items.iter().filter(|i| i.path == path);
            let item = matching
                .next()
                .ok_or("file absent from reviewed inventory")?;
            if matching.next().is_some() {
                return Err("ambiguous reviewed inventory".into());
            }
            staging_lifecycle::cleanup(
                &staging,
                &journal,
                item,
                &confirmation.ok_or(HELP)?,
                &exclusive_access.ok_or(HELP)?,
            )?;
            println!(
                "Removed verified redundant output {}. Undo originals and journal history retained.",
                path.display()
            );
        }
        _ => return Err(HELP.into()),
    }
    Ok(())
}
