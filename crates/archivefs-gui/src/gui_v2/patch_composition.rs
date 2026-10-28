//! Selected-game patch composition. Core owns matching, writing and recovery.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
};

use archivefs_core::{
    patch_output_recovery::{
        PatchOutputRecoveryClassification as RecoveryClass, PatchOutputRecoveryInspection,
        PatchOutputRecoveryPlan, discover_pending_patch_outputs, dismiss_failed_patch_output,
        plan_patch_output_recovery, resume_patch_output, rollback_patch_output,
    },
    patch_package_composition::{
        PatchCompositionCandidate, PatchCompositionPlan, PatchCompositionReadiness,
        PatchCompositionResult, PatchPackageInspection, apply_patch_composition,
        build_patch_composition_plan, default_derived_output_path, provenance_path,
        recover_patch_composition_provenance,
    },
    standalone_patch::{PatchCompatibility, match_patch_source},
};
use eframe::egui;
use sha2::{Digest, Sha256};

use super::{library::Game, patch_package_preview};

enum Outcome {
    Inspected(Result<PatchPackageInspection, String>),
    Planned(Result<PatchCompositionPlan, String>),
    Applied(Result<PatchCompositionResult, String>),
    Recovered(Result<String, String>),
}

#[derive(Default)]
pub(super) struct State {
    selected: Option<(i64, PathBuf)>,
    package: Option<PathBuf>,
    inspection: Option<PatchPackageInspection>,
    order: Vec<usize>,
    order_reviewed: bool,
    plan: Option<PatchCompositionPlan>,
    confirmed: bool,
    job: Option<Receiver<Outcome>>,
    result: Option<Result<PatchCompositionResult, String>>,
    error: Option<String>,
    recovery: Vec<PatchOutputRecoveryInspection>,
    recovery_problems: Vec<String>,
    recovery_confirmation: Option<(PatchOutputRecoveryPlan, RecoveryAction)>,
    recovery_approved: bool,
}

#[derive(Clone, Copy)]
enum RecoveryAction {
    Resume,
    Rollback,
    DismissFailed,
}

fn output_root(source_sha256: &str) -> Result<PathBuf, String> {
    if source_sha256.len() != 64 || !source_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Verified game hash is unavailable.".into());
    }
    Ok(archivefs_core::app_dirs::data_dir()
        .map_err(|e| e.to_string())?
        .join("derived-patches")
        .join(source_sha256[..16].to_ascii_lowercase()))
}

fn verified_source(id: i64, path: &Path) -> Result<(PatchCompositionCandidate, String), String> {
    let database = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
    let library = super::backend::load_library(&database)?;
    let game = library
        .games
        .iter()
        .find(|game| game.archive.id == id && game.archive.absolute_path == path)
        .ok_or("Selected game is no longer in this library. Refresh Games.")?;
    if game.dat_exact.is_none() || game.inspection_incomplete {
        return Err("This game needs a current exact DAT verification before patching.".into());
    }
    let audited = game
        .dat_exact_hashes
        .as_ref()
        .and_then(|hashes| hashes.sha256.as_deref())
        .ok_or("Current exact DAT evidence has no SHA-256 for this file.")?
        .trim()
        .to_ascii_lowercase();
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > archivefs_core::standalone_patch::MAX_APPLY_BYTES
    {
        return Err(
            "The selected game must be a bounded regular file, not a link or archive member."
                .into(),
        );
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let current = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if current != audited {
        return Err("Game bytes differ from the current DAT-audited hash. Verify again.".into());
    }
    Ok((
        PatchCompositionCandidate {
            source_path: path.to_path_buf(),
            verified_identity: Some(format!("sha256:{audited}")),
        },
        audited,
    ))
}

fn prepare(
    id: i64,
    game_path: &Path,
    package: &Path,
    expected_package_hash: &str,
    order: &[usize],
) -> Result<PatchCompositionPlan, String> {
    let (source, source_hash) = verified_source(id, game_path)?;
    let inspection = patch_package_preview::inspect(package)?;
    if inspection.package_sha256 != expected_package_hash {
        return Err("Patch ZIP changed since inspection. Inspect it again.".into());
    }
    plan_for_verified_source(&inspection, source, &source_hash, game_path, order)
}

fn plan_for_verified_source(
    inspection: &PatchPackageInspection,
    source: PatchCompositionCandidate,
    source_hash: &str,
    game_path: &Path,
    order: &[usize],
) -> Result<PatchCompositionPlan, String> {
    let root = output_root(source_hash)?;
    plan_with_root(inspection, source, game_path, order, &root)
}

fn plan_with_root(
    inspection: &PatchPackageInspection,
    source: PatchCompositionCandidate,
    game_path: &Path,
    order: &[usize],
    root: &Path,
) -> Result<PatchCompositionPlan, String> {
    let first = order
        .first()
        .and_then(|index| inspection.patches.get(*index))
        .ok_or("Choose a complete patch order.")?;
    if first.inspection.source_crc32.is_none() || first.inspection.source_size.is_none() {
        return Err("The first patch has no embedded source size and checksum; compatibility cannot be verified. IPS and other unbound patches remain inspection-only.".into());
    }
    let source_bytes = fs::read(&source.source_path).map_err(|e| e.to_string())?;
    let source_match = match_patch_source(&first.inspection, Some(&source_bytes), false);
    if source_match.compatibility != PatchCompatibility::Compatible {
        return Err(format!(
            "The first patch does not match the selected game's bytes: {}",
            source_match.reason
        ));
    }
    let output = default_derived_output_path(game_path, &inspection.package_sha256, root)
        .map_err(|e| e.to_string())?;
    let (pending, problems) = discover_pending_patch_outputs(root);
    if pending
        .iter()
        .any(|record| record.journal.destination_path == output)
    {
        return Err("This exact destination has an interrupted patch operation. Review and resolve it below before creating another copy.".into());
    }
    if !problems.is_empty() {
        return Err("A patch recovery record could not be inspected. Review it before another write in this output folder.".into());
    }
    build_patch_composition_plan(inspection, &[source], output, root, Some(order))
        .map_err(|e| e.to_string())
}

fn execute(
    id: i64,
    game_path: &Path,
    package: &Path,
    expected_package_hash: &str,
    order: &[usize],
    reviewed: &PatchCompositionPlan,
) -> Result<PatchCompositionResult, String> {
    let fresh = prepare(id, game_path, package, expected_package_hash, order)?;
    if fresh != *reviewed {
        return Err("The reviewed plan changed. Preview again before creating an output.".into());
    }
    let inspection = patch_package_preview::inspect(package)?;
    if inspection.package_sha256 != fresh.package_sha256 {
        return Err("Patch ZIP changed immediately before execution.".into());
    }
    match fresh.readiness {
        PatchCompositionReadiness::Ready => apply_patch_composition(&fresh, &inspection),
        PatchCompositionReadiness::NeedsProvenanceRecovery { .. } => {
            recover_patch_composition_provenance(&fresh, &inspection)
        }
        PatchCompositionReadiness::AlreadyCreated { .. } => {
            return Err("The output already has verified package provenance.".into());
        }
    }
    .map_err(|e| e.to_string())
}

impl State {
    fn cancel_plan(&mut self) {
        self.plan = None;
        self.confirmed = false;
    }
    fn start(&mut self, work: impl FnOnce() -> Outcome + Send + 'static, ctx: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        self.job = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(work());
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(receiver) = &self.job else { return };
        let outcome = match receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.error =
                    Some("Patch worker stopped. Check interrupted outputs before retrying.".into());
                return;
            }
        };
        self.job = None;
        match outcome {
            Outcome::Inspected(Ok(report)) => {
                self.order = (0..report.patches.len()).collect();
                self.inspection = Some(report);
            }
            Outcome::Inspected(Err(error)) | Outcome::Planned(Err(error)) => {
                self.error = Some(error)
            }
            Outcome::Planned(Ok(plan)) => self.plan = Some(plan),
            Outcome::Applied(result) => {
                self.plan = None;
                self.result = Some(result);
            }
            Outcome::Recovered(result) => match result {
                Ok(message) => self.error = Some(message),
                Err(error) => self.error = Some(format!("Recovery was refused: {error}")),
            },
        }
    }

    fn refresh_recovery(&mut self, root: &Path) {
        let (found, problems) = discover_pending_patch_outputs(root);
        self.recovery = found;
        self.recovery_problems = problems;
        self.recovery_confirmation = None;
        self.recovery_approved = false;
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State, game: Option<&Game>) {
    ui.separator();
    ui.heading("Create a patched game copy");
    ui.label("Uses a current exact DAT-verified game and a patch with an embedded source checksum. The original game is never changed. This does not prove the result plays.");
    state.poll();
    let Some(game) = game else {
        ui.label("Select a game to compose a patch package.");
        return;
    };
    let selected = (game.archive.id, game.archive.absolute_path.clone());
    if state.selected.as_ref() != Some(&selected) {
        if state.job.is_some() {
            ui.label("A patch operation for the previously selected game is still running. Return to that game to see its result.");
            return;
        }
        if state.result.is_some() {
            ui.label("A patch operation for the previously selected game finished. Return to that game to review its result, or dismiss it below.");
            if ui.button("Dismiss previous result").clicked() {
                *state = State {
                    selected: Some(selected.clone()),
                    ..Default::default()
                };
            }
            return;
        }
        *state = State {
            selected: Some(selected.clone()),
            ..Default::default()
        };
    }
    if game.dat_exact.is_none()
        || game.inspection_incomplete
        || game
            .dat_exact_hashes
            .as_ref()
            .and_then(|h| h.sha256.as_ref())
            .is_none()
    {
        ui.label("A current exact DAT match with SHA-256 is required for this game.");
        return;
    }
    let root = match output_root(
        game.dat_exact_hashes
            .as_ref()
            .unwrap()
            .sha256
            .as_ref()
            .unwrap(),
    ) {
        Ok(root) => root,
        Err(error) => {
            ui.label(error);
            return;
        }
    };
    if state.job.is_some() {
        ui.spinner();
        ui.label("Working. Once writing starts, close or navigation cannot cancel it safely; check recovery if interrupted.");
    }
    if ui
        .add_enabled(state.job.is_none(), egui::Button::new("Choose patch ZIP…"))
        .clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("ZIP package", &["zip"])
            .pick_file()
    {
        state.package = Some(path.clone());
        state.inspection = None;
        state.plan = None;
        state.result = None;
        state.confirmed = false;
        state.error = None;
        state.start(
            move || Outcome::Inspected(patch_package_preview::inspect(&path)),
            ui.ctx(),
        );
    }
    if let Some(report) = state.inspection.clone() {
        ui.label(format!(
            "ZIP: {} · SHA-256 {}",
            report.package_path.display(),
            report.package_sha256
        ));
        ui.label("Package metadata is a claim; the first patch must independently match this game's bytes.");
        let patches = report.patches.clone();
        if patches.len() > 1 {
            ui.label("Set the patch order, then explicitly review it.");
        }
        for position in 0..state.order.len() {
            let index = state.order[position];
            let patch = &patches[index];
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{}. {} · {:?} · SHA-256 {}",
                    position + 1,
                    patch.relative_path,
                    patch.inspection.format,
                    patch.inspection.patch_sha256
                ));
                if position > 0 && ui.button("Up").clicked() {
                    state.order.swap(position, position - 1);
                    state.order_reviewed = false;
                    state.plan = None;
                    state.confirmed = false;
                }
                if position + 1 < patches.len() && ui.button("Down").clicked() {
                    state.order.swap(position, position + 1);
                    state.order_reviewed = false;
                    state.plan = None;
                    state.confirmed = false;
                }
            });
        }
        if patches.len() > 1
            && ui
                .checkbox(&mut state.order_reviewed, "I chose this patch order")
                .changed()
        {
            state.cancel_plan();
        }
        let can_plan = state.job.is_none()
            && !patches.is_empty()
            && (patches.len() == 1 || state.order_reviewed);
        if ui
            .add_enabled(can_plan, egui::Button::new("Preview derived copy"))
            .clicked()
        {
            let id = selected.0;
            let game_path = selected.1.clone();
            let package = report.package_path.clone();
            let hash = report.package_sha256.clone();
            let order = state.order.clone();
            state.plan = None;
            state.result = None;
            state.error = None;
            state.confirmed = false;
            state.start(
                move || Outcome::Planned(prepare(id, &game_path, &package, &hash, &order)),
                ui.ctx(),
            );
        }
    }
    if let Some(plan) = state.plan.clone() {
        ui.separator();
        ui.strong("Reviewed creation plan");
        ui.label(format!(
            "Verified game: {} · SHA-256 {}",
            plan.source_path.display(),
            plan.source_sha256
        ));
        ui.label(format!("New output: {}", plan.output_path.display()));
        ui.label(format!(
            "Package: {} · SHA-256 {}",
            plan.package_path.display(),
            plan.package_sha256
        ));
        ui.label(format!(
            "Provenance identity: {}",
            plan.verified_identity.as_deref().unwrap_or("unavailable")
        ));
        for step in &plan.patches {
            ui.label(format!(
                "{}. {} · {:?} · {}",
                step.order + 1,
                step.relative_path,
                step.format,
                step.patch_sha256
            ));
        }
        ui.label("Verification: current exact DAT SHA-256, matching embedded first-patch source size/checksum, and fresh Core plan. Source and ZIP are checked again immediately before writing.");
        if matches!(
            plan.readiness,
            PatchCompositionReadiness::AlreadyCreated { .. }
        ) {
            ui.label("This derived output already exists. No replacement will be made.");
        } else {
            let recovering = matches!(
                plan.readiness,
                PatchCompositionReadiness::NeedsProvenanceRecovery { .. }
            );
            if recovering {
                ui.label("A standalone output exists without package provenance. Recovery replays the whole patch chain privately and publishes provenance only if its bytes match this exact output.");
            }
            ui.checkbox(
                &mut state.confirmed,
                if recovering {
                    "Recover provenance for this exact output"
                } else {
                    "Create this new copy with the reviewed order and destination"
                },
            );
            if ui
                .add_enabled(
                    state.confirmed && state.job.is_none(),
                    egui::Button::new(if recovering {
                        "Verify and finish provenance"
                    } else {
                        "Create patched copy"
                    }),
                )
                .clicked()
            {
                let id = selected.0;
                let game_path = selected.1.clone();
                let package = plan.package_path.clone();
                let hash = plan.package_sha256.clone();
                let order = state.order.clone();
                state.confirmed = false;
                state.start(
                    move || {
                        Outcome::Applied(execute(id, &game_path, &package, &hash, &order, &plan))
                    },
                    ui.ctx(),
                );
            }
            if ui
                .add_enabled(state.job.is_none(), egui::Button::new("Cancel creation"))
                .clicked()
            {
                state.cancel_plan();
                ui.label(if recovering {
                    "Provenance recovery cancelled; the existing output was unchanged."
                } else {
                    "Cancelled before writing; no output was created."
                });
            }
        }
    }
    if let Some(result) = &state.result {
        match result {
            Ok(result) => {
                ui.strong(
                    if matches!(
                        result.readiness,
                        PatchCompositionReadiness::AlreadyCreated { .. }
                    ) {
                        "Package provenance recovered"
                    } else {
                        "Patched copy created"
                    },
                );
                ui.label(format!(
                    "Output: {} · SHA-256 {}",
                    result.output_path.display(),
                    result.output_sha256
                ));
                ui.label(format!(
                    "Package provenance: {}",
                    provenance_path(&result.output_path).display()
                ));
                ui.label(format!(
                    "Provenance: source {}, package {}, {} ordered patch(es), verified identity {}",
                    result.provenance.source_sha256,
                    result.provenance.package_sha256,
                    result.provenance.patches.len(),
                    result
                        .provenance
                        .verified_identity
                        .as_deref()
                        .unwrap_or("unavailable")
                ));
                ui.label("The original game is unchanged. The derived copy has not been verified as playable.");
            }
            Err(error) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Composition did not return success: {error}"),
                );
                ui.label("A derived output or provenance may already exist. Check interrupted outputs and inspect the exact destination before retrying. The original game was not a write target.");
            }
        }
    }
    if let Some(error) = &state.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if ui.button("Check interrupted patch outputs").clicked() {
        state.refresh_recovery(&root);
    }
    for problem in &state.recovery_problems {
        ui.label(format!("Recovery record needs review: {problem}"));
    }
    for inspection in state.recovery.clone() {
        ui.group(|ui| {
            ui.label(format!(
                "Interrupted output: {} · {:?}",
                inspection.journal.destination_path.display(),
                inspection.classification
            ));
            ui.label(&inspection.explanation);
            ui.label(format!("Journal: {}", inspection.journal_path.display()));
            let can_resume = matches!(
                inspection.classification,
                RecoveryClass::SafeToResume | RecoveryClass::PublicationLikelyComplete
            );
            let can_rollback =
                inspection.classification == RecoveryClass::PublicationLikelyComplete;
            let can_dismiss = inspection.classification == RecoveryClass::Failed
                && fs::symlink_metadata(&inspection.journal.destination_path).is_err();
            for (action, label, allowed) in [
                (RecoveryAction::Resume, "Resume verified output", can_resume),
                (
                    RecoveryAction::Rollback,
                    "Roll back owned output",
                    can_rollback,
                ),
                (
                    RecoveryAction::DismissFailed,
                    "Dismiss failed attempt",
                    can_dismiss,
                ),
            ] {
                if allowed
                    && ui
                        .add_enabled(state.job.is_none(), egui::Button::new(label))
                        .clicked()
                {
                    match plan_patch_output_recovery(&inspection.journal_path) {
                        Ok(plan) if plan.classification == inspection.classification => {
                            state.recovery_confirmation = Some((plan, action));
                            state.recovery_approved = false;
                        }
                        Ok(_) => {
                            state.error =
                                Some("Recovery state changed. Refresh and review it again.".into())
                        }
                        Err(error) => {
                            state.error = Some(format!("Recovery could not be planned: {error}"))
                        }
                    }
                }
            }
        });
    }
    if let Some((plan, action)) = state.recovery_confirmation.clone() {
        ui.checkbox(
            &mut state.recovery_approved,
            match action {
                RecoveryAction::Resume => "Confirm recovery resume",
                RecoveryAction::Rollback => "Confirm recovery rollback",
                RecoveryAction::DismissFailed => "Confirm dismissal of this failed attempt. Only its owned temporary output and matching receipt will be removed; source and patch files remain.",
            },
        );
        if ui
            .add_enabled(
                state.recovery_approved && state.job.is_none(),
                egui::Button::new("Run reviewed recovery"),
            )
            .clicked()
        {
            state.recovery_approved = false;
            state.recovery_confirmation = None;
            state.start(move || Outcome::Recovered((|| {
                match action {
                    RecoveryAction::Resume => resume_patch_output(&plan),
                    RecoveryAction::Rollback => rollback_patch_output(&plan),
                    RecoveryAction::DismissFailed => dismiss_failed_patch_output(&plan),
                }
                    .map_err(|e| e.to_string())?;
                Ok("Recovery completed. Refresh interrupted outputs and inspect the derived file before using it.".into())
            })()), ui.ctx());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
            }
        }
        !crc
    }

    fn bps(source: &[u8], target: &[u8]) -> Vec<u8> {
        assert!(source.len() < 128 && target.len() < 32);
        let mut bytes = b"BPS1".to_vec();
        bytes.extend([source.len() as u8 | 0x80, target.len() as u8 | 0x80, 0x80]);
        bytes.push((((target.len() - 1) << 2) | 1) as u8 | 0x80);
        bytes.extend(target);
        bytes.extend(crc32(source).to_le_bytes());
        bytes.extend(crc32(target).to_le_bytes());
        bytes.extend(crc32(&bytes).to_le_bytes());
        bytes
    }

    #[test]
    fn ordered_verified_composition_creates_only_a_derived_copy() {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game.rom");
        fs::write(&game, b"old").unwrap();
        let package = directory.path().join("patches.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&package).unwrap());
        zip.start_file("a.bps", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bps(b"old", b"Xld")).unwrap();
        zip.start_file("b.bps", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bps(b"Xld", b"Yld")).unwrap();
        zip.finish().unwrap();
        let report = patch_package_preview::inspect(&package).unwrap();
        let candidate = PatchCompositionCandidate {
            source_path: game.clone(),
            verified_identity: Some("sha256:verified".into()),
        };
        let root = directory.path().join("derived");
        assert!(plan_with_root(&report, candidate.clone(), &game, &[], &root).is_err());
        let plan = plan_with_root(&report, candidate, &game, &[0, 1], &root).unwrap();
        assert_eq!(
            plan.patches
                .iter()
                .map(|p| p.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.bps", "b.bps"]
        );
        let result = apply_patch_composition(&plan, &report).unwrap();
        assert_eq!(fs::read(&game).unwrap(), b"old");
        assert_eq!(fs::read(&result.output_path).unwrap(), b"Yld");
        assert_eq!(result.provenance.patches.len(), 2);
        assert_eq!(result.provenance.source_path, game);
        assert_eq!(result.provenance.package_path, package);
    }

    #[test]
    fn metadata_identity_cannot_override_mismatched_embedded_source() {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game.rom");
        fs::write(&game, b"old").unwrap();
        let package = directory.path().join("patches.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&package).unwrap());
        zip.start_file("patch.bps", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bps(b"bad", b"Xld")).unwrap();
        zip.start_file("patch.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"{"verified_identity":"sha256:verified"}"#)
            .unwrap();
        zip.finish().unwrap();
        let report = patch_package_preview::inspect(&package).unwrap();
        assert_eq!(
            report.metadata_source_identity.as_deref(),
            Some("sha256:verified")
        );
        let root = directory.path().join("derived");
        let candidate = PatchCompositionCandidate {
            source_path: game.clone(),
            verified_identity: Some("sha256:verified".into()),
        };
        assert!(plan_with_root(&report, candidate, &game, &[0], &root).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn failed_journal_blocks_only_its_destination_until_dismissed() {
        use archivefs_core::{
            patch_output_recovery::{
                PatchOutputIntent, PreparedPatchOutput, publish_durable_patch_output,
            },
            standalone_patch::StandalonePatchFormat,
        };
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game.rom");
        fs::write(&game, b"old").unwrap();
        let package = directory.path().join("patches.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&package).unwrap());
        zip.start_file("patch.bps", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bps(b"old", b"Xld")).unwrap();
        zip.finish().unwrap();
        let report = patch_package_preview::inspect(&package).unwrap();
        let root = directory.path().join("derived");
        fs::create_dir(&root).unwrap();
        let output = default_derived_output_path(&game, &report.package_sha256, &root).unwrap();
        let patch = directory.path().join("failing.ips");
        fs::write(&patch, b"bad").unwrap();
        let digest = |bytes: &[u8]| {
            Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let failed = publish_durable_patch_output(
            PatchOutputIntent {
                patch_format: StandalonePatchFormat::Ips,
                source_path: game.clone(),
                source_size: 3,
                source_sha256: digest(b"old"),
                patch_path: patch,
                patch_size: 3,
                patch_sha256: digest(b"bad"),
                destination_path: output.clone(),
                expected_output_size: None,
                provenance_path: provenance_path(&output),
            },
            || -> Result<PreparedPatchOutput, _> {
                Err(
                    archivefs_core::patch_output_recovery::PatchOutputRecoveryError::Failed(
                        "bad patch".into(),
                    ),
                )
            },
            |_| Ok(()),
        );
        assert!(failed.is_err());
        let candidate = PatchCompositionCandidate {
            source_path: game.clone(),
            verified_identity: Some("sha256:verified".into()),
        };
        assert!(plan_with_root(&report, candidate.clone(), &game, &[0], &root).is_err());
        let other_package = directory.path().join("other.zip");
        let mut other = zip::ZipWriter::new(fs::File::create(&other_package).unwrap());
        other
            .start_file("patch.bps", zip::write::SimpleFileOptions::default())
            .unwrap();
        other.write_all(&bps(b"old", b"Yld")).unwrap();
        other.finish().unwrap();
        let other_report = patch_package_preview::inspect(&other_package).unwrap();
        assert!(plan_with_root(&other_report, candidate.clone(), &game, &[0], &root).is_ok());
        let (pending, _) = discover_pending_patch_outputs(&root);
        let recovery = plan_patch_output_recovery(&pending[0].journal_path).unwrap();
        dismiss_failed_patch_output(&recovery).unwrap();
        assert!(plan_with_root(&report, candidate, &game, &[0], &root).is_ok());
    }

    #[test]
    fn stale_package_and_cancelled_review_never_create_a_copy() {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game.rom");
        fs::write(&game, b"old").unwrap();
        let package = directory.path().join("patches.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&package).unwrap());
        zip.start_file("a.bps", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bps(b"old", b"Xld")).unwrap();
        zip.finish().unwrap();
        let report = patch_package_preview::inspect(&package).unwrap();
        let root = directory.path().join("derived");
        let plan = plan_with_root(
            &report,
            PatchCompositionCandidate {
                source_path: game.clone(),
                verified_identity: Some("sha256:verified".into()),
            },
            &game,
            &[0],
            &root,
        )
        .unwrap();
        let output = plan.output_path.clone();
        let mut state = State {
            plan: Some(plan.clone()),
            confirmed: true,
            ..Default::default()
        };
        state.cancel_plan();
        assert!(state.plan.is_none() && !state.confirmed && !output.exists());
        fs::write(&package, b"changed").unwrap();
        assert!(matches!(
            apply_patch_composition(&plan, &report),
            Err(archivefs_core::patch_package_composition::PatchCompositionError::StalePackage)
        ));
        assert!(!output.exists());
        assert_eq!(fs::read(&game).unwrap(), b"old");
    }
}
