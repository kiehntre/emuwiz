//! Optional, per-launch RetroArch cheats for the Playing Library (native and
//! AppImage RetroArch, single-disc games).
//!
//! The user picks one `.cht` file and ticks cheats; nothing is ticked by
//! default. Everything the core needs but the GUI cannot know is established
//! in the launch worker, not trusted from the screen:
//!
//! * the exact core file is asked for its own `library_name` in the isolated
//!   probe (`.info` `corename`, the file stem and RetroArch's folders are all
//!   unreliable); if that cannot be established, cheats are refused with an
//!   explanation and an ordinary launch stays available;
//! * RetroArch's override files are inspected read-only; an override that would
//!   defeat a pinned setting, or one that cannot be read safely, blocks cheats;
//! * the chosen file's SHA-256 was taken when it was previewed and is re-checked
//!   by the cheat runtime at launch, as is the probed core file and the
//!   settings file.
//!
//! RetroArch gives no sign of whether a cheat loaded, so a launch never
//! claims a cheat is active.
use super::RetroArchLaunchState;
use archivefs_core::emulator_environment::retroarch::discover_retroarch_environment;
use archivefs_core::emulator_environment::retroarch::{
    DiscoveryEnvironment, ProfileKind, ProfileRef,
};
use archivefs_core::emulator_environment::{FsProbe, HostReadOnlyFilesystem};
use archivefs_core::launch::cheat_launch_plan::{
    ApplicabilityVerdict, CheatCandidate, CheatLaunchSelection, applicability_verdict,
};
use archivefs_core::launch::cheat_runtime_retroarch::{
    RetroArchCheatActivation, RetroArchCheatLaunch, RetroArchCheatLaunchError,
    RetroArchCheatSelection, RetroArchHandoffFailure, preflight_and_launch_retroarch_with_cheats,
    retroarch_facts,
};
use archivefs_core::launch::libretro_probe::{
    CurrentExecutableLauncher, ProbeLauncher, probe_core_library_name,
};
use archivefs_core::launch::retroarch_cheat_candidates::{
    ExcludedCheat, retroarch_launch_candidates,
};
use archivefs_core::launch::retroarch_overrides::discover_retroarch_overrides;
use archivefs_core::launch::{RetroArchLaunchRequest, preflight_retroarch_launch};
use archivefs_core::patch_manager::{
    CheatPackAssociation, CheatPackCatalogueGame, CheatPackLimits, UserCheatLibraryGame,
    preview_cheat_pack,
};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) const START_NOTE: &str = "RetroArch does not say whether a cheat loaded, and EmuWiz has not seen one work in a game. For this launch RetroArch gets a temporary profile holding only the cheats you ticked (and cheats switched on); your ROM, cheat file, saves and RetroArch settings are not changed, and the temporary profile is removed after RetroArch exits.";

/// One cheat in the chooser.
#[derive(Clone, Debug)]
pub(super) struct Row {
    pub(super) candidate: CheatCandidate,
    pub(super) ticked: bool,
    /// Index into `candidate.variants`, when the user has chosen one.
    pub(super) variant: Option<usize>,
    /// The user reviewed a weak match.
    pub(super) acknowledged: bool,
}

impl Row {
    /// Several implementations, or an unresolved conflict: the user must pick.
    fn needs_choice(&self) -> bool {
        self.candidate.variants.len() > 1 || self.candidate.unresolved_conflict
    }

    fn needs_review(&self) -> bool {
        let reviewed = |index: usize| {
            applicability_verdict(&self.candidate.variants[index].applicability)
                == ApplicabilityVerdict::ReviewRequired
        };
        match self.variant {
            Some(index) => reviewed(index),
            None => (0..self.candidate.variants.len()).any(reviewed),
        }
    }
}

/// The user's cheat file and choices, for one specific launch request.
#[derive(Clone, Debug)]
pub(super) struct Choice {
    pub(super) content: PathBuf,
    pub(super) profile: ProfileRef,
    pub(super) core_stem: String,
    pub(super) file: PathBuf,
    pub(super) rows: Vec<Row>,
    pub(super) excluded: Vec<ExcludedCheat>,
}

impl Choice {
    /// Reads and assesses `file` for `request`. Writes nothing.
    pub(super) fn load(request: &RetroArchLaunchRequest, file: &Path) -> Result<Self, String> {
        let is_cht = file
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cht"));
        if !is_cht {
            return Err("Choose a RetroArch cheat file (.cht). Nothing was changed.".into());
        }
        let name = file
            .file_name()
            .map(PathBuf::from)
            .ok_or("The cheat file has no name.")?;
        let title = request
            .selected_content_path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let catalogue = [CheatPackCatalogueGame {
            game: UserCheatLibraryGame {
                game_id: request.expected_game_key.clone(),
                title: title.clone(),
                platform: Some(request.expected_platform_id.clone()),
                ..UserCheatLibraryGame::default()
            },
            facts: Vec::new(),
            revision: None,
        }];
        // The file is chosen for this game by the user; EmuWiz cannot prove the
        // cheats belong to it, so the match is by title only and always needs
        // the user's review.
        let association = CheatPackAssociation {
            title: Some(title),
            platform: Some(request.expected_platform_id.clone()),
            ..CheatPackAssociation::default()
        };
        let associations = BTreeMap::from([(name, association)]);
        let preview = preview_cheat_pack(
            file,
            &catalogue,
            &associations,
            &BTreeSet::new(),
            &CheatPackLimits::default(),
        )
        .map_err(|e| format!("The cheat file cannot be used: {e}"))?;
        let Some(game) = preview.games.first() else {
            return Err("No cheat in this file could be matched to this game.".into());
        };
        let found = retroarch_launch_candidates(&preview, &game.key.clone());
        if found.candidates.is_empty() && found.excluded.is_empty() {
            return Err("This file contains no cheats EmuWiz can use for this game.".into());
        }
        Ok(Self {
            content: request.selected_content_path.clone(),
            profile: request.profile,
            core_stem: request.core_stem.clone(),
            file: file.to_path_buf(),
            rows: found
                .candidates
                .into_iter()
                .map(|candidate| Row {
                    candidate,
                    ticked: false,
                    variant: None,
                    acknowledged: false,
                })
                .collect(),
            excluded: found.excluded,
        })
    }

    pub(super) fn matches(&self, request: &RetroArchLaunchRequest) -> bool {
        self.content == request.selected_content_path
            && self.profile == request.profile
            && self.core_stem == request.core_stem
    }

    pub(super) fn ticked_count(&self) -> usize {
        self.rows.iter().filter(|row| row.ticked).count()
    }

    /// The planner's selections, or why the choices are not yet complete.
    pub(super) fn selections(&self) -> Result<Vec<CheatLaunchSelection>, String> {
        let ticked: Vec<&Row> = self.rows.iter().filter(|row| row.ticked).collect();
        if ticked.is_empty() {
            return Err(
                "Tick at least one cheat, or clear the cheat file to launch without cheats.".into(),
            );
        }
        let mut selections = Vec::new();
        for row in ticked {
            let title = &row.candidate.title;
            let variant_id = if row.needs_choice() {
                let Some(index) = row.variant else {
                    return Err(format!(
                        "\"{title}\" has more than one version. Choose which one to use."
                    ));
                };
                Some(row.candidate.variants[index].variant_id.clone())
            } else {
                None
            };
            if row.needs_review() && !row.acknowledged {
                return Err(format!(
                    "\"{title}\" is not proven to belong to this game. Tick \"I reviewed this cheat for this game\" to use it."
                ));
            }
            selections.push(CheatLaunchSelection {
                logical_id: row.candidate.logical_id.clone(),
                variant_id,
                review_acknowledged: row.acknowledged,
            });
        }
        Ok(selections)
    }
}

/// Where cheats cannot be offered at all.
pub(super) fn unavailable_reason(
    request: &RetroArchLaunchRequest,
    multi_disc: bool,
) -> Option<&'static str> {
    if multi_disc {
        return Some(
            "Cheats are not available for multi-disc games (they launch through a playlist). The game launches normally.",
        );
    }
    if request.profile.profile_kind == ProfileKind::Flatpak {
        return Some(
            "Cheats are not supported for a Flatpak RetroArch. The game launches normally.",
        );
    }
    None
}

/// What the launch worker produced.
pub(super) fn describe(error: RetroArchCheatLaunchError) -> String {
    match error {
        RetroArchCheatLaunchError::HandoffFailed { failure, receipt } => {
            let what = match failure {
                RetroArchHandoffFailure::PreparationFailed { detail } => {
                    format!("the temporary cheat profile could not be prepared ({detail})")
                }
                RetroArchHandoffFailure::SpawnFailed { detail } => {
                    format!("RetroArch could not be started ({detail})")
                }
            };
            format!("{what}. Temporary files: {:?}.", receipt.cleanup)
        }
        RetroArchCheatLaunchError::Preflight(error) => {
            format!("the launch check failed ({error:?})")
        }
        RetroArchCheatLaunchError::Cheats(error) => {
            format!("the cheats cannot be applied ({error:?})")
        }
        RetroArchCheatLaunchError::Facts(detail) => detail,
        RetroArchCheatLaunchError::Spawn(error) => {
            format!("RetroArch could not be started ({error:?})")
        }
        RetroArchCheatLaunchError::InvalidLaunchId => "an internal launch id was invalid".into(),
        RetroArchCheatLaunchError::Sandbox(error) => {
            format!("RetroArch cannot be shown to reach the files it needs ({error:?})")
        }
    }
}

/// If the launch check failed because RetroArch's settings file is a symbolic
/// link (which EmuWiz's RetroArch discovery deliberately does not follow, for
/// plain launches as well as cheat launches), say so plainly instead of the
/// opaque "no candidate" error.
fn symlinked_settings_explanation(
    request: &RetroArchLaunchRequest,
    filesystem: &HostReadOnlyFilesystem,
    environment: &DiscoveryEnvironment,
) -> Option<String> {
    let report = discover_retroarch_environment(filesystem, environment).ok()?;
    let profile = report.profiles.iter().find(|p| {
        p.profile_kind == request.profile.profile_kind && p.scope == request.profile.scope
    })?;
    (profile.config_file.probe == FsProbe::Symlink).then(|| {
        format!(
            "RetroArch's settings file {} is a symbolic link, and EmuWiz does not follow symbolic links for RetroArch's settings (so it cannot launch RetroArch from here, with or without cheats). Replace the link with a regular copy of the file, or launch RetroArch yourself.",
            profile.config_file.path.display
        )
    })
}

fn launch_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("retroarch-{}-{nanos}", std::process::id())
}

/// The whole launch, run on a worker thread that lives until it is done (the
/// probe's parent-death signal is tied to this thread). Every refusal happens
/// before RetroArch starts.
pub(super) fn launch(
    request: &RetroArchLaunchRequest,
    choice: &Choice,
    probe: &dyn ProbeLauncher,
    environment: &DiscoveryEnvironment,
) -> Result<RetroArchCheatLaunch, String> {
    let refuse = |why: String| format!("RetroArch was not started: {why}");
    if !choice.matches(request) {
        return Err(refuse(
            "the cheat file was chosen for a different game.".into(),
        ));
    }
    if let Some(why) = unavailable_reason(request, false) {
        return Err(refuse(why.into()));
    }
    let selections = choice.selections().map_err(refuse)?;
    let filesystem = HostReadOnlyFilesystem;
    // The canonical preflight tells us which core file and profile will be used.
    let command = preflight_retroarch_launch(request, &filesystem, environment).map_err(|e| {
        refuse(
            symlinked_settings_explanation(request, &filesystem, environment)
                .unwrap_or_else(|| format!("the launch check failed ({e:?})")),
        )
    })?;
    let core_path = command.selection.core_library.clone();
    // 1. The core's own library name, from the core itself.
    let probed = probe_core_library_name(&core_path, probe).map_err(|e| refuse(e.explanation()))?;
    // 2. Override files, read-only.
    let candidates: Vec<CheatCandidate> = choice.rows.iter().map(|r| r.candidate.clone()).collect();
    let mut selection = RetroArchCheatSelection {
        candidates,
        selections,
        core_library_name: probed.library_name.clone(),
        effective_overrides: Vec::new(),
    };
    let facts = retroarch_facts(
        &command,
        &filesystem,
        environment,
        &selection,
        PathBuf::from("/"),
    )
    .map_err(|e| refuse(e))?;
    let Some(real_config) = facts.real_config_path.clone() else {
        return Err(refuse(
            "RetroArch's settings file could not be located.".into(),
        ));
    };
    let home = environment.home.as_ref().map(PathBuf::from);
    let overrides = discover_retroarch_overrides(
        &real_config,
        &probed.library_name,
        &request.selected_content_path,
        home.as_deref(),
    );
    if let Some(why) = overrides.refusal() {
        return Err(refuse(why));
    }
    selection.effective_overrides = overrides.findings;
    // 3. Everything verified is re-checked immediately before the launch.
    probed
        .ensure_unchanged()
        .map_err(|e| refuse(e.explanation()))?;
    if let Some(binding) = &overrides.configuration {
        binding.ensure_unchanged().map_err(refuse)?;
    }
    preflight_and_launch_retroarch_with_cheats(
        request,
        &filesystem,
        environment,
        &selection,
        &launch_id(),
    )
    .map_err(|e| refuse(describe(e)))
}

/// The lines shown once a cheat launch has finished.
pub(super) fn session_summary(
    session: &archivefs_core::launch::cheat_runtime::CheatRuntimeSession<
        archivefs_core::launch::LaunchedRetroArchProcess,
    >,
) -> Vec<String> {
    let receipt = session.receipt();
    let exit = match receipt.process.as_ref().and_then(|p| p.exit.as_ref()) {
        Some(exit) if exit.success => "exited normally".to_string(),
        Some(exit) => format!("failed (exit code {:?})", exit.code),
        None => "exited (status unknown)".to_string(),
    };
    let mut lines = vec![format!(
        "RetroArch {exit}; temporary cheat profile: {:?}. EmuWiz did not see the cheats work.",
        receipt.cleanup
    )];
    lines.extend(receipt.notes.iter().cloned());
    lines.extend(receipt.errors.iter().cloned());
    lines
}

pub(super) fn activation_label() -> &'static str {
    RetroArchCheatActivation::PreparedActivationNotVerified.label()
}

/// The production probe launcher.
pub(super) fn production_probe() -> Arc<dyn ProbeLauncher + Send + Sync> {
    Arc::new(CurrentExecutableLauncher)
}

/// The chooser. Drawn only while no launch is in progress.
pub(super) fn show_chooser(
    ui: &mut egui::Ui,
    state: &mut RetroArchLaunchState,
    request: &RetroArchLaunchRequest,
    multi_disc: bool,
) {
    if state.is_active() {
        return;
    }
    if state
        .cheats
        .as_ref()
        .is_some_and(|choice| !choice.matches(request))
    {
        state.cheats = None;
        state.cheats_message = None;
    }
    ui.separator();
    ui.label("Cheats for this launch (optional)");
    if let Some(reason) = unavailable_reason(request, multi_disc) {
        state.cheats = None;
        ui.label(reason);
        return;
    }
    if ui.button("Choose a cheat file (.cht)…").clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("RetroArch cheat file", &["cht"])
            .pick_file()
    {
        match Choice::load(request, &path) {
            Ok(choice) => {
                state.cheats = Some(choice);
                state.cheats_message = None;
            }
            Err(error) => {
                state.cheats = None;
                state.cheats_message = Some(error);
            }
        }
    }
    let mut clear = false;
    if let Some(choice) = state.cheats.as_mut() {
        for row in &mut choice.rows {
            ui.checkbox(&mut row.ticked, &row.candidate.title);
            if row.ticked {
                if row.needs_choice() {
                    let labels: Vec<String> = row
                        .candidate
                        .variants
                        .iter()
                        .map(|v| format!("{} · {}", v.source.source_id, v.variant_id))
                        .collect();
                    egui::ComboBox::from_id_salt(("retroarch_variant", &row.candidate.logical_id))
                        .selected_text(
                            row.variant
                                .map_or("Choose a version".to_string(), |i| labels[i].clone()),
                        )
                        .show_ui(ui, |ui| {
                            for (index, label) in labels.iter().enumerate() {
                                ui.selectable_value(&mut row.variant, Some(index), label);
                            }
                        });
                }
                if row.needs_review() {
                    ui.checkbox(&mut row.acknowledged, "I reviewed this cheat for this game");
                }
            }
        }
        for excluded in &choice.excluded {
            ui.add_enabled(
                false,
                egui::Checkbox::new(
                    &mut false,
                    excluded
                        .title
                        .clone()
                        .unwrap_or_else(|| excluded.logical_key.clone()),
                ),
            );
            ui.small(format!(
                "  Not available: {}.",
                excluded.reason.explanation()
            ));
        }
        ui.small(START_NOTE);
        clear = ui
            .button("Launch without cheats (clear cheat file)")
            .clicked();
    }
    if clear {
        state.cheats = None;
    }
    if let Some(message) = &state.cheats_message {
        ui.label(message.as_str());
    }
}
