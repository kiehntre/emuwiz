//! Migration-only maintenance entry point. Never scans or imports media.
//! Production use requires separate owner approval and externally established
//! database-handle quiescence; the native upgrade lock only serializes upgrades.
use archivefs_core::{check_database_health, latest_schema_version, upgrade_library_database};
use std::{error::Error, path::Path};

const CONFIRMATION: &str = "UPGRADE SCHEMA 25 TO 26";

fn run(path: &Path, confirmation: Option<&str>) -> Result<(), Box<dyn Error>> {
    if confirmation.is_some_and(|value| value != CONFIRMATION) {
        return Err("Confirmation must be exactly UPGRADE SCHEMA 25 TO 26".into());
    }
    let health = check_database_health(path);
    println!("{health:#?}");
    if confirmation.is_none() {
        println!("Read-only inspection. No migration requested.");
        return Ok(());
    }
    if latest_schema_version() != 26 || health.schema_version != Some(25) {
        return Err("This caller authorizes only schema 25 to 26; nothing upgraded".into());
    }
    let report = upgrade_library_database(path)?;
    println!("{report:#?}");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, path] if flag == "--database" => run(Path::new(path), None),
        [flag, path, confirm, phrase] if flag == "--database" && confirm == "--confirm" => {
            run(Path::new(path), Some(phrase))
        }
        _ => Err("Usage: schema26_upgrade --database FILE [--confirm 'UPGRADE SCHEMA 25 TO 26']; without --confirm this only inspects. Production use requires separate owner approval and database quiescence.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_never_creates_a_missing_database() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("absent.sqlite3");
        run(&path, None).unwrap();
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn wrong_confirmation_never_opens_or_creates_database() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("absent.sqlite3");
        assert!(run(&path, Some("yes")).is_err());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn current_schema_is_refused_without_backup_or_changes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("current.sqlite3");
        drop(archivefs_core::Database::open_or_create(&path).unwrap());
        let before = std::fs::read(&path).unwrap();
        let entries_before = std::fs::read_dir(temp.path()).unwrap().count();
        run(&path, None).unwrap();
        assert!(run(&path, Some(CONFIRMATION)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            entries_before
        );
    }
}
