//! `database-recover-journal`: the copy-first way back from a library that
//! EmuWiz refuses to open because a write was cut off.
//!
//! With no arguments it only prepares: the database and its journal are
//! copied, SQLite recovers the copy, the copy is checked and a report is
//! printed. The library itself is not changed. `--apply <folder> --confirm`
//! then lets SQLite recover the library itself, after proving it is still
//! exactly what was copied and checked.
use std::fmt::Write as _;
use std::path::PathBuf;

use archivefs_core::{
    JournalRecoveryReceipt, apply_journal_recovery, default_database_path, prepare_journal_recovery,
};

fn describe(receipt: &JournalRecoveryReceipt) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Library database: {}", receipt.database_path.display());
    let _ = writeln!(out, "Recovery folder:  {}", receipt.recovery_dir.display());
    let _ = writeln!(
        out,
        "  original/   the database ({} bytes, SHA-256 {}) and its journal ({} bytes), exactly as found",
        receipt.database.bytes, receipt.database.sha256, receipt.journal.bytes
    );
    let _ = writeln!(
        out,
        "  recovered/  a copy after SQLite's own rollback (SHA-256 {})",
        receipt.recovered.sha256
    );
    let _ = writeln!(
        out,
        "The interrupted write {}.",
        if receipt.journal_restored_content {
            "had reached the database; the rollback undid it"
        } else {
            "had not reached the database; the journal was a leftover and the rollback changed nothing"
        }
    );
    let _ = writeln!(
        out,
        "Recovered copy: integrity check ok, record relationships ok, schema {}{}.",
        receipt.schema_version,
        if receipt.opens_as_current_library {
            ", opens as a current library"
        } else {
            ", not the current schema (it will need the normal library upgrade)"
        }
    );
    let saved: u64 = receipt.saved_dat_rows.values().sum();
    let _ = writeln!(
        out,
        "Saved DAT results in the recovered copy: {saved}. Recovery adds and upgrades none; \
         anything the cut-off write had not finished saving is simply not there."
    );
    out
}

pub fn run(args: Vec<String>) -> Result<String, Box<dyn std::error::Error>> {
    let mut database = None;
    let mut apply = None;
    let mut confirm = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--database" => {
                database = Some(PathBuf::from(
                    args.next()
                        .ok_or("database-recover-journal: --database needs a path")?,
                ));
            }
            "--apply" => {
                apply = Some(PathBuf::from(args.next().ok_or(
                    "database-recover-journal: --apply needs the recovery folder",
                )?));
            }
            "--confirm" => confirm = true,
            other => {
                return Err(format!(
                    "database-recover-journal: unknown argument {other}. Nothing was changed.\n\
                     usage: database-recover-journal [--database <path>]\n\
                     \x20      database-recover-journal [--database <path>] --apply <recovery-folder> --confirm"
                )
                .into());
            }
        }
    }
    let database = match database {
        Some(path) => path,
        None => default_database_path()?,
    };
    let Some(recovery_dir) = apply else {
        let receipt = prepare_journal_recovery(&database)?;
        let mut out = describe(&receipt);
        let _ = writeln!(
            out,
            "The library was not changed and EmuWiz will still refuse to open it.\n\
             To let SQLite recover the library itself (the same rollback, on the same bytes, \
             that was just checked on the copy), close EmuWiz and run:\n\
             \x20 emuwiz-cli database-recover-journal --apply '{}' --confirm\n\
             The recovery folder is never deleted by EmuWiz; keep it until you are satisfied.",
            receipt.recovery_dir.display()
        );
        return Ok(out);
    };
    if !confirm {
        return Err(
            "database-recover-journal: --apply changes the library database and needs --confirm. \
             Nothing was changed."
                .into(),
        );
    }
    let receipt = apply_journal_recovery(&database, &recovery_dir)?;
    let mut out = describe(&receipt);
    let _ = writeln!(
        out,
        "Applied. SQLite rolled the interrupted write back on the library database, and the \
         result matches the checked copy byte for byte. The originals stay in {}.",
        receipt.recovery_dir.join("original").display()
    );
    Ok(out)
}
