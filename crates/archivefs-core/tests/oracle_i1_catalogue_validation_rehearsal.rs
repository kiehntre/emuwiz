//! ORACLE I1: catalogue validation rehearsal on a scratch profile.
//!
//! Does exactly what the DAT Sources page's Validate / Validate all does for
//! each catalogue of one system that has never been validated:
//! `validate_dat_source`, then (only when Valid / ValidWithWarnings) persist
//! the expected inventory into the database, then record the observed health
//! on the catalogue entry and save the configuration. Nothing is matched
//! here; matching is done afterwards by the installed `emuwiz-cli dat recheck
//! --unchecked`, the production route.
//!
//! Refuses unless the database and configuration it writes are inside the
//! scratch profile. Owner approval 2026-11-28: "I1 approved ... Start with the
//! ZX Spectrum catalogue validation and verification rehearsal."

use archivefs_core::dat::limits::DatLimits;
use archivefs_core::dat::sources::{
    DatHealthState, DatSourceRegistry, default_dat_sources_config_path,
    load_dat_sources_config_from, save_dat_sources_config_to, validate_dat_source,
};
use std::fmt::Write as _;
use std::path::PathBuf;

#[test]
#[ignore = "ORACLE I1 rehearsal on a scratch copy; run explicitly inside its sandbox"]
fn oracle_i1_catalogue_validation_rehearsal() {
    let env = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name}"));
    let root = PathBuf::from(env("EMUWIZ_I1_ROOT"));
    let platform = env("EMUWIZ_I1_PLATFORM");
    assert!(
        root.starts_with("/tmp/claude-1000/"),
        "{} is not scratch",
        root.display()
    );
    // SAFETY: this test runs alone (--test-threads=1), before any thread.
    unsafe {
        std::env::set_var("EMUWIZ_DATA_HOME", root.join("data"));
        std::env::set_var("EMUWIZ_CONFIG_HOME", root.join("config"));
    }
    let config_path = default_dat_sources_config_path().unwrap();
    let database = archivefs_core::default_database_path().unwrap();
    assert!(
        config_path.starts_with(&root) && database.starts_with(&root),
        "writes outside the profile"
    );

    let (mut registry, warnings) =
        DatSourceRegistry::from_config(&load_dat_sources_config_from(&config_path).unwrap());
    assert!(warnings.is_empty(), "registry warnings: {warnings:?}");
    let pending: Vec<_> = registry
        .entries()
        .iter()
        .filter(|e| e.enabled && e.platform.as_deref() == Some(platform.as_str()))
        .filter(|e| e.health.state.is_none())
        .cloned()
        .collect();
    let mut report = format!("never-validated {platform} catalogues: {}\n", pending.len());
    let mut db = archivefs_core::Database::open_or_create(&database).unwrap();
    for entry in &pending {
        let (validation, expected) = validate_dat_source(entry, DatLimits::default());
        let mut saved = 0;
        if matches!(
            validation.state,
            DatHealthState::Valid | DatHealthState::ValidWithWarnings
        ) {
            saved = db
                .persist_validated_dat_inventory(&validation, &expected)
                .unwrap();
        }
        let health = validation.to_health(&entry.path, entry.kind);
        writeln!(
            report,
            "{} | {:?} | {} | inventory rows {saved}",
            entry.id, validation.state, validation.summary
        )
        .unwrap();
        registry.get_mut(&entry.id).unwrap().health = health;
    }
    save_dat_sources_config_to(&config_path, &registry.to_config()).unwrap();
    std::fs::write(root.join("i1-validation-report.txt"), &report).unwrap();
    print!("{report}");
}
