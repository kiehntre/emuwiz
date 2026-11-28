//! Check Games Stage D: catalogue import rehearsal on a scratch profile.
//!
//! Runs the DAT Sources page's own steps against a copy of the owner's
//! database and configuration:
//! 1. add each catalogue file by reference, as "Add DAT file" does;
//! 2. apply the page's safe matches (undoable recognition);
//! 3. for an add-on catalogue the page leaves for a person to choose, set
//!    the platform as the person's choice;
//! 4. Check Games: resolve the catalogue, preview, verify and save into the
//!    database copy.
//!
//! The game files are read through a read-only mount; the harness refuses
//! unless the database and configuration it writes are inside the scratch
//! profile. Owner approval 2026-11-28: "Prepare and test imports against
//! scratch data before touching production."

use archivefs_core::dat::catalogue_selection::{
    CatalogueInventoryInputs, CatalogueRef, resolve_catalogue,
};
use archivefs_core::dat::library_verification::preview_library_verification;
use archivefs_core::dat::managed_sources::{
    default_managed_dat_sources_config_path, load_managed_dat_sources_from,
};
use archivefs_core::dat::sources::recognition::{apply_safe_matches, safe_matches};
use archivefs_core::dat::sources::{
    DatSourceEntry, DatSourceKind, DatSourceRegistry, default_dat_sources_config_path,
    load_dat_sources_config_from, load_dat_sources_config_with_revision,
    save_dat_sources_config_to,
};
use archivefs_core::safe_read::TrustedRoots;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// (library platform, catalogue: an installed source id or a file to add).
const RUNS: &[(&str, &str)] = &[
    (
        "Amiga",
        "add:$ROOT/catalogues/libretro/Commodore - Amiga.dat",
    ),
    ("Nintendo DS", "193"),
    ("Nintendo DS", "200"),
    ("Nintendo DS", "196"),
    ("TurboGrafx-16", "140"),
    (
        "Apple II",
        "add:$TOSEC/Apple II - Games - [DSK] (TOSEC-v2025-01-15_CM).dat",
    ),
    ("Apple II", "apple-iigs-games-2mg-tosec-v2024-07-03-cm"),
    (
        "AtariST",
        "add:$TOSEC/Atari ST - Games - [STX] (TOSEC-v2024-07-03_CM).dat",
    ),
    (
        "Amstrad CPC",
        "add:$TOSEC/Amstrad CPC - Games - [DSK] (TOSEC-v2025-01-15_CM).dat",
    ),
    (
        "MSX2",
        "add:$TOSEC/MSX MSX2 - Games - [DSK] (TOSEC-v2025-01-15_CM).dat",
    ),
    ("PSP", "257"),
];

fn registry(path: &Path) -> DatSourceRegistry {
    let (registry, warnings) =
        DatSourceRegistry::from_config(&load_dat_sources_config_from(path).unwrap());
    assert!(warnings.is_empty(), "registry warnings: {warnings:?}");
    registry
}

#[test]
#[ignore = "Stage D rehearsal on a scratch copy; run explicitly inside its sandbox"]
fn stage_d_catalogue_import_rehearsal() {
    let env = |name: &str| PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name}")));
    let (root, out, tosec) = (
        env("EMUWIZ_SD_ROOT"),
        env("EMUWIZ_SD_OUT"),
        env("EMUWIZ_SD_TOSEC"),
    );
    for path in [&root, &out] {
        assert!(
            path.starts_with("/tmp/claude-1000/"),
            "{} is not scratch",
            path.display()
        );
    }
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
    let managed_config = default_managed_dat_sources_config_path().unwrap();
    assert!(managed_config.starts_with(&root));
    let mut report = String::new();

    // 1. Add the new catalogue files by reference, as the DAT page does.
    let mut draft = registry(&config_path);
    let mut ids = Vec::new();
    for (platform, catalogue) in RUNS {
        let id = if let Some(file) = catalogue.strip_prefix("add:") {
            let path = PathBuf::from(
                file.replace("$ROOT", root.to_str().unwrap())
                    .replace("$TOSEC", tosec.to_str().unwrap()),
            );
            assert!(path.is_file(), "{}", path.display());
            let id = draft.suggest_id(&path);
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            draft
                .add(DatSourceEntry {
                    origin: Some("added on the DAT Sources page".to_string()),
                    ..DatSourceEntry::new(id.clone(), name, path, DatSourceKind::File)
                })
                .unwrap();
            writeln!(report, "added {id}").unwrap();
            id
        } else {
            catalogue.to_string()
        };
        ids.push((*platform, id));
    }
    save_dat_sources_config_to(&config_path, &draft.to_config()).unwrap();

    // 2. The page's safe matches, limited to these catalogues.
    let (config, revision) = load_dat_sources_config_with_revision(&config_path).unwrap();
    let (checked, _) = DatSourceRegistry::from_config(&config);
    let wanted: Vec<_> = safe_matches(&checked)
        .into_iter()
        .filter(|m| ids.iter().any(|(_, id)| *id == m.source_id))
        .collect();
    for m in &wanted {
        writeln!(
            report,
            "safe match: {} -> {} (header {:?})",
            m.source_id, m.platform, m.header_name
        )
        .unwrap();
    }
    if !wanted.is_empty() {
        apply_safe_matches(&config_path, &revision, &checked, &wanted).unwrap();
    }

    // 3. A person's choice where the page leaves one.
    let mut draft = registry(&config_path);
    for (platform, id) in &ids {
        let entry = draft
            .get_mut(id)
            .unwrap_or_else(|| panic!("no source {id}"));
        match entry.platform.as_deref() {
            None => {
                entry.set_platform_by_person(Some(platform.to_string()));
                writeln!(report, "person's choice: {id} -> {platform}").unwrap();
            }
            Some(set) => writeln!(report, "platform already set: {id} = {set}").unwrap(),
        }
    }
    save_dat_sources_config_to(&config_path, &draft.to_config()).unwrap();
    let registry = registry(&config_path);
    let managed = load_managed_dat_sources_from(&managed_config).unwrap();
    let managed_root = archivefs_core::dat::updates::managed_dat_root().unwrap();

    // 4. Check Games, one catalogue at a time.
    let trusted = TrustedRoots::from_paths([PathBuf::from("/mnt/usbdrive/games")]);
    writeln!(report, "\nplatform | catalogue | selected | eligible | verified | no match | ambiguous | not checked | errors | saved | protected | seconds").unwrap();
    for (platform, id) in &ids {
        let inputs = CatalogueInventoryInputs {
            local_registry: &registry,
            managed_sources: &managed,
            managed_root: &managed_root,
            limits: Default::default(),
        };
        let started = std::time::Instant::now();
        let line = match resolve_catalogue(&CatalogueRef::local(id.clone()), inputs)
            .map_err(|e| e.to_string())
            .and_then(|resolved| {
                preview_library_verification(resolved, database.clone(), platform.to_string(), None)
            }) {
            Err(error) => format!("{platform} | {id} | refused: {error}"),
            Ok(preview) => {
                let (selected, eligible) = (preview.selected_games, preview.eligible_games);
                if eligible == 0 {
                    let mut reasons = std::collections::BTreeMap::new();
                    for e in &preview.exclusions {
                        *reasons.entry(e.reason.clone()).or_insert(0) += 1;
                    }
                    format!("{platform} | {id} | {selected} | 0 | nothing eligible: {reasons:?}")
                } else {
                    match preview.execute(&trusted, &AtomicBool::new(false), &|_| {}) {
                        Err(error) => {
                            format!("{platform} | {id} | {selected} | {eligible} | failed: {error}")
                        }
                        Ok(t) => format!(
                            "{platform} | {id} | {selected} | {eligible} | {} | {} | {} | {} | {} | {} | {} | {}",
                            t.verified,
                            t.no_match,
                            t.ambiguous,
                            t.not_checked,
                            t.errors,
                            t.saved,
                            t.protected_prior,
                            started.elapsed().as_secs()
                        ),
                    }
                }
            }
        };
        writeln!(report, "{line}").unwrap();
        std::fs::write(out.join("report.txt"), &report).unwrap();
    }
    std::fs::write(out.join("report.txt"), &report).unwrap();
}
