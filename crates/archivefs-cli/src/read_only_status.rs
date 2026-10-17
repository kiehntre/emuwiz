//! Recovered from cli-release-polish 0f985d55. Thin read-only consumers of
//! existing lifecycle and operation projections; no executors or new stores.
use std::path::{Path, PathBuf};

use archivefs_core::emulator_inventory::discover_installed_emulators_read_only;
use archivefs_core::emulator_lifecycle::{
    LifecycleContext, LifecycleState, inspect_all_emulator_lifecycles,
};
use archivefs_core::managed_emulator_install::ManagedInstallHealth;
use archivefs_core::operation::OperationRegistry;
use serde::Serialize;

pub const HELP: &str = "  emulators [status] [--json]  Read-only PATH candidates and managed emulator status; no executable/version probes. Other installation sources, selection, launch readiness and updates are not assessed.\n  activity [list] [--json]   Read-only unsettled/needs-attention operation receipts; not a live process monitor.\n  history [list] [--json]    Read-only operation receipts, including completed operations. History coverage is best-effort; unavailable evidence is reported.\n  Each accepts --help. None repairs, installs, downloads, launches or changes state.";

fn options(command: &str, args: &[String]) -> Result<(bool, bool), String> {
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        return Ok((false, true));
    }
    let subcommand = if command == "emulators" {
        "status"
    } else {
        "list"
    };
    let mut json = false;
    let mut sub = false;
    for arg in args {
        match arg.as_str() {
            "--json" if !json => json = true,
            value if value == subcommand && !sub => sub = true,
            _ => {
                return Err(format!(
                    "usage: {command} [{subcommand}] [--json] | --help; unsupported argument {arg:?}"
                ));
            }
        }
    }
    Ok((json, false))
}

pub fn run(command: &str, args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let (json, help) = options(command, &args)?;
    if help {
        println!("{HELP}");
        return Ok(());
    }
    let output = if command == "emulators" {
        emulator_output(json)?
    } else {
        history_output(command, &HistoryPaths::default(), json)?
    };
    println!("{output}");
    Ok(())
}

fn emulator_output(json: bool) -> Result<String, serde_json::Error> {
    let inventory = discover_installed_emulators_read_only();
    let mut problems = inventory
        .warnings
        .iter()
        .map(|warning| warning.message.clone())
        .collect::<Vec<_>>();
    let mut managed = inventory.managed_installations.clone();
    for entry in &mut managed {
        if entry.health != ManagedInstallHealth::Healthy {
            problems.push(format!(
                "Managed {} at {}: {:?} ({:?})",
                entry.emulator_id,
                entry.install_root.display(),
                entry.health,
                entry.role
            ));
        }
        // The core's invalid-manifest record stores its diagnostic in this
        // legacy field. Present it as a problem, never as an installed version.
        if entry.health == ManagedInstallHealth::InvalidManifest {
            if let Some(detail) = entry.installed_version.take() {
                problems.push(format!(
                    "Managed {}: {detail}",
                    entry.install_root.display()
                ));
            }
        }
    }
    let context = LifecycleContext::from_inventory(inventory);
    let projections = inspect_all_emulator_lifecycles(&context);
    let scope = "PATH candidates for MAME, Dolphin, RPCS3, PCSX2, PPSSPP, DuckStation and xemu, plus existing managed-install manifests. Missing means no candidate in these sources, not absence from the machine.";
    let not_evaluated = [
        "executable/version probes",
        "Flatpak and additional AppImage locations",
        "configured selection",
        "launch readiness",
        "update availability",
        "package ownership queries",
    ];
    if json {
        let rendered = projections.iter().map(|projection| serde_json::json!({
            "emulator_id":projection.emulator_id,
            "state":emulator_state(projection.state),
            "installations":projection.installations.iter().map(|installation| serde_json::json!({
                "exact_binding":installation.exact_binding,
                "installation_type":installation.installation_type,
                "ownership_category":installation.ownership_category,
                "version":installation.version,
                "channel":installation.channel,
                "local_health":installation.local_health,
                "launch_readiness":installation.launch_readiness.as_ref().map(|v|format!("{v:?}")),
                "update_authority":installation.update_authority,
                "update_status":installation.update_status.as_ref().map(|v|format!("{v:?}")),
                "provenance":installation.provenance,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>();
        return serde_json::to_string_pretty(&serde_json::json!({
            "schema_version":1,"command":"emulators","read_only":true,
            "coverage_complete":false,"scope":scope,"not_evaluated":not_evaluated,
            "problems":problems,"emulators":rendered,"managed_installations":managed,
        }));
    }
    let mut text = format!(
        "Emulator status (read-only; probe-free)\nScope: {scope}\nNot evaluated: {}\n",
        not_evaluated.join(", ")
    );
    if projections.is_empty() {
        text.push_str("No emulator candidates found in the inspected sources; other sources are not assessed.\n");
    }
    for emulator in projections {
        text.push_str(&format!(
            "{}: {} ({} candidate(s) in inspected sources)\n",
            emulator.emulator_id,
            emulator_state(emulator.state),
            emulator.installations.len()
        ));
        for install in emulator.installations {
            text.push_str(&format!(
                "  {:?} | health={:?} | version={} ({:?}) | readiness={:?}\n",
                install.exact_binding,
                install.local_health,
                install.version.version.as_deref().unwrap_or("unknown"),
                install.version.source,
                install.launch_readiness
            ));
        }
    }
    for problem in problems {
        text.push_str(&format!("Warning: {problem}\n"));
    }
    for entry in managed {
        text.push_str(&format!(
            "Managed {}: {:?} | {:?} | version={} | {}\n",
            entry.emulator_id,
            entry.health,
            entry.role,
            entry.installed_version.as_deref().unwrap_or("unknown"),
            entry.install_root.display()
        ));
    }
    Ok(text)
}

// Core lifecycle state is local inventory health, not a release check. The
// CLI must not advertise an unqueried managed version as up to date.
fn emulator_state(state: LifecycleState) -> String {
    match state {
        LifecycleState::InstalledCurrent => "InstalledLocalVersionKnownUpdateNotEvaluated".into(),
        LifecycleState::Missing => "NoCandidateInInspectedSources".into(),
        other => format!("{other:?}"),
    }
}

type Location = Result<PathBuf, String>;
struct HistoryPaths {
    rename: Location,
    views: Location,
    shared: Location,
    backups: Location,
    database: Location,
}

impl Default for HistoryPaths {
    fn default() -> Self {
        Self {
            rename: archivefs_core::dat::rename_apply::default_rename_transaction_dir()
                .map_err(|e| e.to_string()),
            views: archivefs_core::default_library_view_history_dir().map_err(|e| e.to_string()),
            shared: archivefs_core::patch_manager::default_shared_history_root()
                .map_err(|e| e.detail),
            backups: archivefs_core::patch_manager::default_shared_backup_root()
                .map_err(|e| e.detail),
            database: archivefs_core::default_database_path().map_err(|e| e.to_string()),
        }
    }
}

#[derive(Serialize)]
struct Source {
    name: &'static str,
    path: Option<PathBuf>,
    state: &'static str,
}

// Preflight catches errors the historical best-effort registry readers omit.
// Never substitute an empty/CWD path when resolving an application root fails.
fn directory(
    name: &'static str,
    path: &Location,
    sources: &mut Vec<Source>,
    problems: &mut Vec<String>,
) -> Option<PathBuf> {
    let path = match path {
        Ok(path) if path.is_absolute() => path,
        Ok(path) => {
            problems.push(format!(
                "{name} location unavailable: relative application path {}",
                path.display()
            ));
            sources.push(Source {
                name,
                path: None,
                state: "unavailable",
            });
            return None;
        }
        Err(error) => {
            problems.push(format!("{name} location unavailable: {error}"));
            sources.push(Source {
                name,
                path: None,
                state: "unavailable",
            });
            return None;
        }
    };
    let state = match std::fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries {
                if let Err(error) = entry {
                    problems.push(format!("{name} directory enumeration incomplete: {error}"));
                }
            }
            "inspected"
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "not_created",
        Err(error) => {
            problems.push(format!("{name} unavailable at {}: {error}", path.display()));
            "unavailable"
        }
    };
    sources.push(Source {
        name,
        path: Some(path.clone()),
        state,
    });
    (state != "unavailable").then(|| path.clone())
}

fn history_output(
    command: &str,
    paths: &HistoryPaths,
    json: bool,
) -> Result<String, serde_json::Error> {
    let mut sources = Vec::new();
    let mut problems = Vec::new();
    let mut registry = OperationRegistry::default();
    if let Some(path) = directory("rename", &paths.rename, &mut sources, &mut problems) {
        registry = OperationRegistry::from_rename_journals(&path);
    }
    if let Some(path) = directory("library_views", &paths.views, &mut sources, &mut problems) {
        registry.append_library_view_history(&path);
    }
    if let Some(path) = directory("shared_apply", &paths.shared, &mut sources, &mut problems) {
        match &paths.backups {
            Ok(backups) => registry.append_shared_apply_history(&path, backups),
            Err(error) => problems.push(format!(
                "Shared history unavailable: backup location could not be resolved: {error}"
            )),
        }
    }
    let parent = paths
        .database
        .as_ref()
        .map(|path| path.parent().unwrap_or(Path::new("/")).to_path_buf())
        .map_err(Clone::clone);
    if directory("database_receipts", &parent, &mut sources, &mut problems).is_some() {
        if let Ok(path) = &paths.database {
            registry.append_database_backup_history(path);
            registry.append_database_restore_history(path);
        }
    }
    problems.append(&mut registry.problems);
    registry.records.sort_by(|a, b| {
        b.created_at_unix
            .cmp(&a.created_at_unix)
            .then(a.operation_id.cmp(&b.operation_id))
    });
    let records = if command == "activity" {
        registry.active().collect::<Vec<_>>()
    } else {
        registry.records.iter().collect()
    };
    let coverage = "Best-effort existing workflow receipts only. Library View history and database receipt readers have 200-entry limits; shared history has a 512-journal limit. Availability is checked before reading and can change during observation. Missing or malformed evidence may be omitted; this is not a live process monitor or a fresh recovery approval.";
    if json {
        return serde_json::to_string_pretty(&serde_json::json!({
            "schema_version":1,"command":command,"read_only":true,"coverage_complete":false,
            "coverage_notes":[coverage],"sources":sources,"records":records,"problems":problems,
        }));
    }
    let mut text = format!(
        "{} (read-only; {} recorded operation(s))\n{coverage}\n",
        if command == "activity" {
            "Activity: unsettled / needs attention"
        } else {
            "History"
        },
        records.len()
    );
    if records.is_empty() {
        text.push_str("No matching records in the inspected sources.\n");
    }
    for record in records {
        text.push_str(&format!(
            "{} | {} | {}\n  {}\n  Recovery: {} — {}\n",
            record.kind.label(),
            record.state.label(),
            record.operation_id,
            record.output.summary,
            record.recovery.classification.label(),
            record.recovery.explanation
        ));
        if let Some(error) = &record.error {
            text.push_str(&format!("  Error: {error}\n"));
        }
    }
    for source in sources {
        text.push_str(&format!(
            "Source {}: {} ({})\n",
            source.name,
            source.state,
            source
                .path
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "unavailable location".into())
        ));
    }
    for problem in problems {
        text.push_str(&format!("Warning: {problem}\n"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests;
