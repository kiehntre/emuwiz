//! A diagnosis as plain text.
//!
//! One renderer, shared by the command line, Game Doctor and the export, so
//! that the three never say different things about the same report. The
//! text holds whatever the report holds: pass a report through
//! [`super::redact::Redactor::report`] first when it is going to be shown.

use super::evidence::EvidenceId;
use super::rules::{Diagnosis, DiagnosisReport};

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
    let cite = |ids: &[EvidenceId]| -> Vec<String> {
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

/// The report as text under `heading`, which becomes its first line.
pub fn report_text(report: &DiagnosisReport, heading: &str) -> String {
    let mut out = format!("{heading}\n");
    if let Some(emulator) = &report.subject.emulator {
        out.push_str(&format!("Emulator: {emulator}\n"));
    }
    if let Some(game) = &report.subject.game {
        out.push_str(&format!("Game: {game}\n"));
    }
    out.push_str(&format!(
        "Rules: engine version {}, rule set {}\n\n{}: {}\nVerdict: {}\n",
        report.rule_set.engine_version,
        report
            .rule_set
            .digest
            .get(..16)
            .unwrap_or(&report.rule_set.digest),
        if report.launch_was_supplied() {
            "Launch (as supplied; EmuWiz did not observe it)"
        } else {
            "Launch"
        },
        report.launch.describe(),
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
