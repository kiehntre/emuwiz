//! Focused UI for verified single-track CUE/BIN to CHD conversion.
//!
//! This is deliberately separate from equivalent-content review: conversion
//! creates a new representation, whereas duplicate review quarantines one.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::ui::{components as widgets, theme};
use archivefs_core::repair::{
    ChdConversionPlan, ChdConversionResult, ChdConversionSourceMode, ChdConversionTransaction,
    build_chd_conversion_plan, execute_chd_conversion, rollback_chd_conversion,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;

struct Candidate {
    path: PathBuf,
    plan: Option<ChdConversionPlan>,
    reason: Option<String>,
}
pub(crate) struct OpticalConversionPageState {
    pub(crate) source_root_draft: String,
    candidates: Vec<Candidate>,
    selected: Option<usize>,
    source_mode: ChdConversionSourceMode,
    confirm: bool,
    result: Option<ChdConversionResult>,
    transaction: Option<ChdConversionTransaction>,
    error: Option<String>,
}

impl Default for OpticalConversionPageState {
    fn default() -> Self {
        Self {
            source_root_draft: String::new(),
            candidates: Vec::new(),
            selected: None,
            source_mode: ChdConversionSourceMode::KeepSource,
            confirm: false,
            result: None,
            transaction: None,
            error: None,
        }
    }
}

fn collect_cues(root: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_cues(&path, output);
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case("cue"))
        {
            output.push(path);
        }
    }
}

impl OpticalConversionPageState {
    fn scan(&mut self) {
        self.candidates.clear();
        self.selected = None;
        self.confirm = false;
        self.result = None;
        self.transaction = None;
        self.error = None;
        let root = PathBuf::from(self.source_root_draft.trim());
        if !root.is_dir() {
            self.error = Some("choose an existing source folder first".into());
            return;
        }
        let mut cues = Vec::new();
        collect_cues(&root, &mut cues);
        cues.sort();
        for cue in cues {
            let target = cue.with_extension("chd");
            match build_chd_conversion_plan(&cue, &target, self.source_mode, None) {
                Ok(plan) => self.candidates.push(Candidate {
                    path: cue,
                    plan: Some(plan),
                    reason: None,
                }),
                Err(error) => self.candidates.push(Candidate {
                    path: cue,
                    plan: None,
                    reason: Some(error.to_string()),
                }),
            }
        }
    }

    fn convert(&mut self) {
        let Some(index) = self.selected else { return };
        let Some(mut plan) = self
            .candidates
            .get(index)
            .and_then(|candidate| candidate.plan.clone())
        else {
            self.error = Some("the selected source is not eligible for conversion".into());
            return;
        };
        plan.source_mode = self.source_mode;
        let root = PathBuf::from(self.source_root_draft.trim());
        let journal =
            match archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir() {
                Ok(path) => path,
                Err(error) => {
                    self.error = Some(error.to_string());
                    return;
                }
            };
        let mut trusted_paths = vec![root.clone()];
        if let Some(parent) = plan.target_path.parent() {
            trusted_paths.push(parent.to_path_buf());
        }
        let trusted = TrustedRoots::from_paths(trusted_paths);
        match execute_chd_conversion(&plan, trusted, &journal, &root, &AtomicBool::new(false)) {
            Ok((result, transaction)) => {
                self.result = Some(result);
                self.transaction = Some(transaction);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn rollback(&mut self) {
        let Some(transaction) = self.transaction.as_mut() else {
            self.error = Some("nothing is available to undo".into());
            return;
        };
        let Ok(journal) =
            archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
        else {
            self.error = Some("could not locate the repair journal".into());
            return;
        };
        match rollback_chd_conversion(transaction, &journal, &AtomicBool::new(false)) {
            Ok(_) => {
                self.result = None;
                self.transaction = None;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("rollback failed: {error}")),
        }
    }
}

pub(crate) fn show_optical_conversion_page(
    ui: &mut egui::Ui,
    state: &mut OpticalConversionPageState,
) {
    ui.heading("Convert Disc Images");
    ui.label("Create a CHD only from a verified single-track MODE1/2048 CUE/BIN source. The staged CHD is independently fingerprint-verified before finalization.");
    ui.add_space(8.0);
    widgets::card(ui, |ui| {
        ui.heading("Source");
        ui.horizontal_wrapped(|ui| {
            ui.label("Source folder:");
            ui.add_sized(
                [ui.available_width().min(520.0).max(220.0), 24.0],
                egui::TextEdit::singleline(&mut state.source_root_draft),
            );
            if ui.button("Choose folder").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_folder()
            {
                state.source_root_draft = path.display().to_string();
            }
            if ui.button("Scan").clicked() {
                state.scan();
            }
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(state.source_root_draft.as_str())
                    .monospace()
                    .color(theme::muted(ui)),
            )
            .wrap(),
        );
    });
    ui.add_space(theme::SECTION_GAP);
    widgets::card(ui, |ui| {
        ui.heading("Output and safety");
        let mut quarantine = state.source_mode == ChdConversionSourceMode::QuarantineSource;
        if ui
            .checkbox(
                &mut quarantine,
                "Quarantine originals after verified conversion",
            )
            .changed()
        {
            state.source_mode = if quarantine {
                ChdConversionSourceMode::QuarantineSource
            } else {
                ChdConversionSourceMode::KeepSource
            };
        }
        ui.label(
            if state.source_mode == ChdConversionSourceMode::KeepSource {
                "Keep originals (default)"
            } else {
                "Quarantine originals after verified conversion"
            },
        );
    });
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::RED, error);
    }
    if state.candidates.is_empty() && state.error.is_none() {
        ui.label("No CUE/BIN conversion candidates scanned yet.");
    }
    for index in 0..state.candidates.len() {
        let candidate_path = state.candidates[index].path.clone();
        let plan = state.candidates[index].plan.clone();
        let reason = state.candidates[index].reason.clone();
        let eligible = plan.is_some();
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(
                    state.selected == Some(index),
                    candidate_path.display().to_string(),
                )
                .clicked()
                && eligible
            {
                state.selected = Some(index);
                state.confirm = false;
            }
            if eligible {
                ui.label("Eligible");
            } else {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    reason.as_deref().unwrap_or("blocked"),
                );
            }
        });
        if state.selected == Some(index)
            && let Some(plan) = &plan
        {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("Target: {}", plan.target_path.display()))
                        .monospace(),
                )
                .wrap(),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{} sectors · canonical SHA-256 {}",
                        plan.source_fingerprint.structure.logical_sector_count,
                        plan.source_fingerprint.canonical_sha256
                    ))
                    .monospace(),
                )
                .wrap(),
            );
            if !state.confirm {
                if ui.button("Confirm conversion").clicked() {
                    state.confirm = true;
                }
            } else if ui.button("Convert now").clicked() {
                state.convert();
            }
        }
    }
    if let Some(result) = &state.result {
        let target_path = result.target_path.display().to_string();
        let transaction_id = result.transaction_id.clone();
        let source_quarantined = result.source_quarantined;
        ui.separator();
        widgets::card(ui, |ui| {
            ui.heading("Result");
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("Verified CHD created: {}", target_path))
                        .monospace(),
                )
                .wrap(),
            );
            ui.label(format!("Transaction: {transaction_id}"));
            ui.label(if source_quarantined {
                "Originals quarantined after verification."
            } else {
                "Originals kept."
            });
            if ui.button("Undo conversion").clicked() {
                state.rollback();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(root: &Path) -> PathBuf {
        let bin = root.join("Track space ü.bin");
        let cue = root.join("Game space ü.cue");
        std::fs::write(&bin, vec![0x42u8; 2048 * 16]).unwrap();
        std::fs::write(
            &cue,
            "FILE \"Track space ü.bin\" BINARY\nTRACK 01 MODE1/2048\nINDEX 01 00:00:00\n",
        )
        .unwrap();
        cue
    }

    #[test]
    fn scan_exposes_eligible_source_without_mutating_it() {
        let directory = tempfile::tempdir().unwrap();
        let cue = source(directory.path());
        let before = std::fs::read(&cue).unwrap();
        let mut state = OpticalConversionPageState {
            source_root_draft: directory.path().display().to_string(),
            ..Default::default()
        };
        state.scan();
        assert_eq!(state.candidates.len(), 1);
        assert!(state.candidates[0].plan.is_some());
        assert_eq!(state.source_mode, ChdConversionSourceMode::KeepSource);
        assert_eq!(std::fs::read(&cue).unwrap(), before);
    }

    #[test]
    fn scan_surfaces_target_collision_as_blocked() {
        let directory = tempfile::tempdir().unwrap();
        let cue = source(directory.path());
        std::fs::write(cue.with_extension("chd"), b"existing").unwrap();
        let mut state = OpticalConversionPageState {
            source_root_draft: directory.path().display().to_string(),
            ..Default::default()
        };
        state.scan();
        assert_eq!(state.candidates.len(), 1);
        assert!(state.candidates[0].plan.is_none());
        assert!(
            state.candidates[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("target already exists"))
        );
    }
}
