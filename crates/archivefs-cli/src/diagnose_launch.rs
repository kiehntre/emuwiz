//! `diagnose-launch`: explain a failed emulator launch from what it left
//! behind. A thin, read-only front end over `archivefs_core::daedalus`.
//!
//! The command reads only the files it is pointed at, looks at the paths it
//! is given and prints a report. It opens no database, starts no emulator
//! and changes nothing.
use std::path::PathBuf;
use std::time::Duration;

use archivefs_core::daedalus::collect::{LaunchArtefacts, collect, read_log_prefix};
use archivefs_core::daedalus::evidence::{ConfigLayer, ExitOutcome, SpawnFailure};
use archivefs_core::daedalus::rules::{Diagnosis, DiagnosisReport, diagnose, rule_set};

pub const HELP: &str = "  diagnose-launch --emulator <name> [evidence...] [--json]
                             Read-only: explain a failed emulator launch from the files it left behind.
                             Starts nothing and changes nothing. How the process ended and its
                             environment are taken as you state them and reported as supplied, not
                             observed. Evidence (all optional):
                               --exit-code <n> | --signal <n> | --spawn-error not-found|permission-denied|other
                               --duration-ms <n>  --launcher <name>  --game <title>
                               --stderr-file <path>  --log-file <path>
                               --settings <path>  --session-settings <path>  --core-settings <path>
                               --folder-settings <path>  --game-settings <path>
                               --game-file <path>  --core <path>  --save-dir <path>
                               --env NAME=VALUE  --env-unset NAME   (the launch environment; repeatable)
  diagnose-launch --rules [--json]
                             List the diagnostic rules and the rule-set version.
  diagnose-launch --help     Show this help.";

#[derive(Debug, PartialEq)]
enum Request {
    Help,
    Rules { json: bool },
    Diagnose { artefacts: Box<Parsed>, json: bool },
}

/// Parsed arguments, before any file is read.
#[derive(Debug, Default, PartialEq)]
struct Parsed {
    emulator: String,
    game: Option<String>,
    exit: Option<ExitOutcome>,
    duration_ms: Option<u64>,
    launcher: Option<String>,
    stderr_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
    settings: Vec<(PathBuf, ConfigLayer)>,
    game_file: Option<PathBuf>,
    core: Option<PathBuf>,
    save_directory: Option<PathBuf>,
    environment: Vec<(String, Option<String>)>,
}

fn usage(problem: impl std::fmt::Display) -> String {
    format!("diagnose-launch: {problem}\nRun `emuwiz-cli diagnose-launch --help` for usage.")
}

fn parse(args: &[String]) -> Result<Request, String> {
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h") {
        return Ok(Request::Help);
    }
    let mut parsed = Parsed::default();
    let mut json = false;
    let mut rules = false;
    let mut emulator = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        index += 1;
        let mut value = |name: &str| -> Result<String, String> {
            let value = args
                .get(index)
                .ok_or_else(|| usage(format!("{name} needs a value")))?;
            index += 1;
            Ok(value.clone())
        };
        let once = |already: bool, name: &str| -> Result<(), String> {
            if already {
                Err(usage(format!("{name} was given more than once")))
            } else {
                Ok(())
            }
        };
        match flag {
            "--json" => {
                once(json, flag)?;
                json = true;
            }
            "--rules" => {
                once(rules, flag)?;
                rules = true;
            }
            "--emulator" => {
                once(emulator.is_some(), flag)?;
                emulator = Some(value(flag)?);
            }
            "--game" => {
                once(parsed.game.is_some(), flag)?;
                parsed.game = Some(value(flag)?);
            }
            "--launcher" => {
                once(parsed.launcher.is_some(), flag)?;
                parsed.launcher = Some(value(flag)?);
            }
            "--exit-code" | "--signal" | "--spawn-error" => {
                once(parsed.exit.is_some(), "the way the process ended")?;
                let text = value(flag)?;
                let number = || {
                    text.parse::<i32>()
                        .map_err(|_| usage(format!("{flag} needs a whole number, not {text:?}")))
                };
                parsed.exit = Some(match flag {
                    "--exit-code" => {
                        let code = number()?;
                        if !(0..=255).contains(&code) {
                            return Err(usage(format!("{code} is not an exit status (0 to 255)")));
                        }
                        ExitOutcome::Exited { code }
                    }
                    "--signal" => {
                        let signal = number()?;
                        if !(1..=64).contains(&signal) {
                            return Err(usage(format!("{signal} is not a signal number")));
                        }
                        ExitOutcome::Signaled {
                            signal,
                            core_dumped: false,
                        }
                    }
                    _ => ExitOutcome::SpawnFailed {
                        reason: match text.as_str() {
                            "not-found" => SpawnFailure::ProgramNotFound,
                            "permission-denied" => SpawnFailure::PermissionDenied,
                            "other" => SpawnFailure::Other,
                            _ => {
                                return Err(usage(format!(
                                    "--spawn-error must be not-found, permission-denied or \
                                     other, not {text:?}"
                                )));
                            }
                        },
                        os_error: None,
                    },
                });
            }
            "--duration-ms" => {
                once(parsed.duration_ms.is_some(), flag)?;
                let text = value(flag)?;
                parsed.duration_ms = Some(text.parse().map_err(|_| {
                    usage(format!("--duration-ms needs a whole number, not {text:?}"))
                })?);
            }
            "--stderr-file" => {
                once(parsed.stderr_file.is_some(), flag)?;
                parsed.stderr_file = Some(value(flag)?.into());
            }
            "--log-file" => {
                once(parsed.log_file.is_some(), flag)?;
                parsed.log_file = Some(value(flag)?.into());
            }
            "--settings" | "--session-settings" | "--core-settings" | "--folder-settings"
            | "--game-settings" => {
                let layer = match flag {
                    "--settings" => ConfigLayer::Main,
                    "--session-settings" => ConfigLayer::Session,
                    "--core-settings" => ConfigLayer::Core,
                    "--folder-settings" => ConfigLayer::ContentDirectory,
                    _ => ConfigLayer::Game,
                };
                once(
                    parsed.settings.iter().any(|(_, found)| *found == layer),
                    flag,
                )?;
                parsed.settings.push((value(flag)?.into(), layer));
            }
            "--game-file" => {
                once(parsed.game_file.is_some(), flag)?;
                parsed.game_file = Some(value(flag)?.into());
            }
            "--core" => {
                once(parsed.core.is_some(), flag)?;
                parsed.core = Some(value(flag)?.into());
            }
            "--save-dir" => {
                once(parsed.save_directory.is_some(), flag)?;
                parsed.save_directory = Some(value(flag)?.into());
            }
            "--env" => {
                let pair = value(flag)?;
                let (name, setting) = pair
                    .split_once('=')
                    .filter(|(name, _)| !name.is_empty())
                    .ok_or_else(|| usage(format!("--env needs NAME=VALUE, not {pair:?}")))?;
                once(
                    parsed.environment.iter().any(|(known, _)| known == name),
                    &format!("the variable {name:?}"),
                )?;
                parsed
                    .environment
                    .push((name.to_string(), Some(setting.to_string())));
            }
            "--env-unset" => {
                let name = value(flag)?;
                if name.is_empty() || name.contains('=') {
                    return Err(usage(format!(
                        "--env-unset needs a variable name, not {name:?}"
                    )));
                }
                once(
                    parsed.environment.iter().any(|(known, _)| *known == name),
                    &format!("the variable {name:?}"),
                )?;
                parsed.environment.push((name, None));
            }
            other => return Err(usage(format!("unsupported argument {other:?}"))),
        }
    }
    if rules {
        let alone = args.len() == 1 + usize::from(json);
        return if alone {
            Ok(Request::Rules { json })
        } else {
            Err(usage("--rules takes no evidence arguments"))
        };
    }
    parsed.emulator = emulator
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| usage("--emulator <name> is required"))?;
    // An empty path is a mistake in the command, not a missing file.
    let paths = parsed
        .settings
        .iter()
        .map(|(path, _)| path)
        .chain(&parsed.stderr_file)
        .chain(&parsed.log_file)
        .chain(&parsed.game_file)
        .chain(&parsed.core)
        .chain(&parsed.save_directory);
    if paths.into_iter().any(|path| path.as_os_str().is_empty()) {
        return Err(usage("a path argument is empty"));
    }
    Ok(Request::Diagnose {
        artefacts: Box::new(parsed),
        json,
    })
}

fn artefacts(parsed: Parsed) -> Result<LaunchArtefacts, String> {
    let stderr = match &parsed.stderr_file {
        Some(path) => Some(
            read_log_prefix(path)
                .map_err(|error| format!("cannot read {:?}: {}", path.display(), error.kind()))?,
        ),
        None => None,
    };
    Ok(LaunchArtefacts {
        emulator: parsed.emulator,
        game: parsed.game,
        exit: parsed.exit,
        duration: parsed.duration_ms.map(Duration::from_millis),
        launcher: parsed.launcher,
        stderr,
        log_file: parsed.log_file,
        settings: parsed.settings,
        game_file: parsed.game_file,
        core: parsed.core,
        save_directory: parsed.save_directory,
        environment: parsed.environment,
        // Everything typed on the command line is the caller's account of
        // the launch. EmuWiz observed none of it.
        supplied_by_caller: true,
    })
}

pub fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let output = match parse(&args)? {
        Request::Help => HELP.to_string(),
        Request::Rules { json: true } => serde_json::to_string_pretty(&rule_set())?,
        Request::Rules { json: false } => format_rules(),
        Request::Diagnose {
            artefacts: parsed,
            json,
        } => {
            let report = diagnose(&collect(&artefacts(*parsed)?));
            if json {
                serde_json::to_string_pretty(&report)?
            } else {
                format_report(&report)
            }
        }
    };
    println!("{output}");
    Ok(())
}

fn format_rules() -> String {
    let info = rule_set();
    let mut out = format!(
        "EmuWiz launch diagnosis rules\nEngine version {}, rule set {}\n",
        info.version.engine_version,
        &info.version.digest[..16]
    );
    for rule in &info.rules {
        out.push_str(&format!(
            "\n{} (version {})\n  {}\n  {}\n",
            rule.id,
            rule.version,
            rule.category.label(),
            rule.summary
        ));
    }
    out
}

fn push_list(out: &mut String, label: &str, items: &[String]) {
    for (index, item) in items.iter().enumerate() {
        let lead = if index == 0 { label } else { "" };
        out.push_str(&format!("     {lead:<14}{item}\n"));
    }
}

fn push_diagnosis(
    out: &mut String,
    number: usize,
    diagnosis: &Diagnosis,
    report: &DiagnosisReport,
) {
    out.push_str(&format!(
        "\n  {number}. {} [{}]\n     {}\n     {}\n",
        diagnosis.title,
        diagnosis.standing(),
        diagnosis.category.label(),
        diagnosis.explanation
    ));
    let cite = |ids: &[archivefs_core::daedalus::EvidenceId]| -> Vec<String> {
        ids.iter()
            .map(|id| match report.evidence.get(id) {
                Some(item) => format!("{id}: {}", item.summary()),
                None => id.to_string(),
            })
            .collect()
    };
    push_list(out, "Evidence:", &cite(&diagnosis.supporting));
    push_list(out, "Against:", &cite(&diagnosis.contradicting));
    push_list(out, "Limits:", &diagnosis.limitations);
    let checks: Vec<String> = diagnosis
        .next_checks
        .iter()
        .map(|check| check.describe())
        .collect();
    push_list(out, "Next checks:", &checks);
    let repairs: Vec<String> = diagnosis
        .repairs
        .iter()
        .map(|hint| format!("{} (not applied)", hint.summary))
        .collect();
    push_list(out, "Could repair:", &repairs);
}

fn format_report(report: &DiagnosisReport) -> String {
    let mut out =
        String::from("EmuWiz launch diagnosis (read-only; nothing was started or changed)\n");
    if let Some(emulator) = &report.subject.emulator {
        out.push_str(&format!("Emulator: {emulator}\n"));
    }
    if let Some(game) = &report.subject.game {
        out.push_str(&format!("Game: {game}\n"));
    }
    out.push_str(&format!(
        "Rules: engine version {}, rule set {}\n\nVerdict: {}\n",
        report.rule_set.engine_version,
        &report.rule_set.digest[..16.min(report.rule_set.digest.len())],
        report.verdict.headline()
    ));
    if !report.symptoms.is_empty() {
        out.push_str("\nWhat happened\n");
        for (index, symptom) in report.symptoms.iter().enumerate() {
            push_diagnosis(&mut out, index + 1, symptom, report);
        }
    }
    if !report.causes.is_empty() {
        out.push_str("\nWhy (most firmly supported first)\n");
        for (index, cause) in report.causes.iter().enumerate() {
            push_diagnosis(&mut out, index + 1, cause, report);
        }
    }
    for conflict in &report.conflicts {
        out.push_str(&format!(
            "\nConflict: {} disagree with each other.\n",
            conflict.rules.join(" and ")
        ));
    }
    for rejected in &report.rejected {
        out.push_str(&format!(
            "\nNot reported: a finding from {} was refused because {}.\n",
            rejected.rule, rejected.reason
        ));
    }
    if !report.notes.is_empty() {
        out.push_str("\nNotes\n");
        for note in &report.notes {
            out.push_str(&format!("  - {note}\n"));
        }
    }
    out.push_str("\nNot observed, or not fully\n");
    let mut any_gap = false;
    for coverage in &report.coverage {
        let mut gaps = coverage.missing.clone();
        if coverage.stale > 0 {
            gaps.push(format!(
                "{} observation(s) out of date because the source changed",
                coverage.stale
            ));
        }
        if gaps.is_empty() {
            continue;
        }
        any_gap = true;
        let partly = if coverage.has_current_evidence() {
            " (partly observed)"
        } else {
            ""
        };
        out.push_str(&format!(
            "  - {}{partly}: {}\n",
            coverage.area.label(),
            gaps.join("; ")
        ));
    }
    if !any_gap {
        out.push_str("  (every area has some evidence and nothing failed to read)\n");
    }
    out
}

#[cfg(test)]
#[path = "diagnose_launch/tests.rs"]
mod tests;
