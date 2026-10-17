//! Native Flycast source review and the verified installation controller.
mod install;
use crate::*;
use archivefs_core::launch::{LaunchPlan, LaunchReadiness, LaunchTarget};
use archivefs_core::patch_manager::*;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

type ReviewResult<T> = std::result::Result<T, String>;

pub(crate) const INSTALL_BLOCKER: &str = "Install requires a verified official v2.7 x86-64 binary and its ordinary Linux XDG profile. Unknown builds, Flatpak and portable overrides are refused with evidence during Preview. Source compatibility is verified; in-game effectiveness is untested.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchView {
    pub(crate) profile_id: String,
    pub(crate) profile_path: Option<PathBuf>,
}

/// Reuse the same selection projection shown by Blue Launch. Never pick the
/// first discovered profile, and never create a separate saved preference.
pub(crate) fn selected_launch(plan: &LaunchPlan, game: &Path) -> ReviewResult<LaunchView> {
    if plan.platform_id.as_deref() != Some("Dreamcast") {
        return Err("Dreamcast Launch identity is unavailable".into());
    }
    let candidate = crate::gui_v2::launch_readiness_summary::selected_candidate(plan)
        .ok_or("Launch has no unambiguous selected emulator/profile")?;
    if candidate.readiness == LaunchReadiness::Blocked || !candidate.blockers.is_empty() {
        return Err("The selected Launch candidate is blocked; review Launch Readiness".into());
    }
    if candidate.content.resolved_path.as_deref() != Some(game) || candidate.content.requires_mount
    {
        return Err("Launch selection is for different or unresolved content".into());
    }
    match &candidate.target {
        LaunchTarget::Standalone { adapter_id: "flycast", profile_id, profile_path } => Ok(LaunchView {
            profile_id: profile_id.clone(), profile_path: profile_path.clone(),
        }),
        _ => Err("Launch currently selects another emulator; choosing Flycast cheats does not change Launch".into()),
    }
}

impl ArchiveFsApp {
    pub(crate) fn flycast_cheat_launch_view(
        &self,
        live: Option<&LoadedData>,
    ) -> ReviewResult<LaunchView> {
        let workflow = self.cheat_workflow.as_ref().ok_or("Choose a game")?;
        if !is_flycast_route(workflow) {
            return Err("Flycast cheats are not selected".into());
        }
        let game = &workflow.archive_path;
        match self.build_launch_readiness_input(live) {
            crate::launch_readiness_page::LaunchReadinessInput::Plan { plan, .. } => {
                selected_launch(&plan, game)
            }
            _ => Err(
                "Launch selection is not available yet; inspect this game's Launch Readiness"
                    .into(),
            ),
        }
    }
}

pub(crate) fn is_flycast_route(w: &CheatWorkflowState) -> bool {
    matches!(&w.routing.decision, CheatRouteDecision::Routed(r) if matches!(&r.target, CheatRouteTarget::Standalone { adapter_id } if adapter_id == "flycast"))
}

fn verified_product(identity: Option<&GameIdentityReport>, game: &Path) -> ReviewResult<String> {
    use archivefs_core::game_identity::{IdentityKind, IdentityPlatform, IdentityStatus};
    let id = identity
        .filter(|id| {
            id.archive_path == game && id.complete && id.platform == IdentityPlatform::Dreamcast
        })
        .ok_or("Complete Dreamcast identity for this exact game is required")?;
    let evidence: Vec<_> = id
        .evidence
        .iter()
        .filter(|e| e.kind == IdentityKind::DreamcastProductCode)
        .collect();
    if evidence.is_empty()
        || evidence
            .iter()
            .any(|e| e.status != IdentityStatus::Verified || e.value.is_none())
    {
        return Err("Dreamcast product code is missing, uncertain or conflicting".into());
    }
    let code = evidence[0].value.as_deref().unwrap();
    if code.is_empty()
        || !code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b' '))
        || evidence.iter().any(|e| e.value.as_deref() != Some(code))
    {
        return Err("Dreamcast product-code evidence conflicts or is malformed".into());
    }
    Ok(code.into())
}

struct Loaded {
    path: PathBuf,
    bytes: Vec<u8>,
    file: FlycastCheatFile,
}

fn inspect_source(path: &Path) -> ReviewResult<Loaded> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !path.is_absolute() || !meta.is_file() || meta.file_type().is_symlink() {
        return Err("Choose a regular local native Flycast file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(FLYCAST_CHEAT_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let file = parse_flycast_cheat_file(&bytes).map_err(|e| e.to_string())?;
    Ok(Loaded {
        path: path.into(),
        bytes,
        file,
    })
}

/// Inspection eligibility only; never an installation capability claim.
fn supported(entry: &FlycastCheatEntry) -> bool {
    matches!(
        entry.code,
        DreamcastCheatCode::DirectWrite {
            width: 1 | 2 | 4,
            ..
        }
    )
}

#[derive(Default)]
pub(crate) struct FlycastPageState {
    pub(crate) launch: Option<ReviewResult<LaunchView>>,
    install: install::InstallState,
    receiver: Option<Receiver<ReviewResult<Loaded>>>,
    loaded: Option<Loaded>,
    selected: Vec<bool>,
    enabled: Vec<bool>,
    expected_product: String,
    reviewed: bool,
    failure: Option<String>,
    context: Option<(
        PathBuf,
        Option<GameIdentityReport>,
        Option<ReviewResult<LaunchView>>,
    )>,
}
impl FlycastPageState {
    pub(crate) fn busy(&self) -> bool {
        self.install.busy()
    }
    fn prepare_source_pick(&mut self) {
        self.reviewed = false;
        self.install.invalidate();
    }
    fn accept(&mut self, result: ReviewResult<Loaded>) {
        self.reviewed = false;
        self.install.invalidate();
        match result {
            Ok(loaded) => {
                self.selected = vec![false; loaded.file.entries.len()];
                self.enabled = vec![false; loaded.file.entries.len()];
                self.loaded = Some(loaded);
                self.failure = None;
            }
            Err(e) => {
                self.loaded = None;
                self.failure = Some(e);
            }
        }
    }
    fn update_context(&mut self, game: &Path, identity: Option<&GameIdentityReport>) {
        let key = (game.to_path_buf(), identity.cloned(), self.launch.clone());
        if self.context.as_ref() != Some(&key) {
            self.reviewed = false;
            self.install.invalidate();
            self.context = Some(key);
        }
    }
    fn review(&mut self, product: &ReviewResult<String>) -> ReviewResult<()> {
        self.reviewed = false;
        let code = product.as_ref().map_err(Clone::clone)?;
        if code != self.expected_product.trim() {
            return Err(
                "Cheat source product-code declaration does not match verified game identity"
                    .into(),
            );
        }
        let l = self.loaded.as_ref().ok_or("Choose a local native file")?;
        if inspect_source(&l.path)?.bytes != l.bytes {
            return Err("Local source changed or disappeared; choose it again".into());
        }
        if !l.file.warnings.is_empty() {
            return Err(
                "Parser warnings prevent a supported-entry review; inspect the diagnostics".into(),
            );
        }
        if !self.selected.iter().any(|s| *s) {
            return Err("Select at least one supported entry".into());
        }
        self.reviewed = true;
        Ok(())
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut FlycastPageState,
    game: &Path,
    identity: Option<&GameIdentityReport>,
) {
    let product = verified_product(identity, game);
    state.update_context(game, identity);
    if let Some(rx) = &state.receiver {
        let result = match rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                "Local-file inspection interrupted; no changes made".into(),
            )),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        if let Some(result) = result {
            state.receiver = None;
            state.accept(result);
        }
    }
    ui.heading("Standalone Flycast cheats");
    ui.label("Native sectionless Flycast .cht files. Preview → Confirm → Install → History/Undo. In-game effectiveness remains unverified.");
    ui.label("Installation supports validated 8/16/32-bit native set-value entries in Dreamcast RAM. Conditional, increment, copy, masked and repeated operations are refused. CodeBreaker/GameShark code-only files are not converted.");
    match &state.launch {
        Some(Ok(view)) => {
            ui.label(format!(
                "Launch profile: {} · {:?}",
                view.profile_id, view.profile_path
            ));
        }
        Some(Err(e)) => {
            ui.label(e);
        }
        None => {
            ui.label("Launch profile has not been resolved");
        }
    }
    ui.label("Preview checks this Launch projection against the shared AUTO/PREFERRED/FORCED policy and freshly verifies the executable, exact XDG profile and game-to-file mapping.");
    match &product {
        Ok(code) => {
            ui.label(format!("Verified Dreamcast product code: {code}"));
        }
        Err(e) => {
            ui.colored_label(egui::Color32::YELLOW, e);
        }
    }
    ui.label("Disc-specific applicability is unverified: core product-code evidence does not establish the cheat's disc/revision binding. A matching declaration is not authentication of the cheat source.");
    if ui
        .add_enabled(
            state.receiver.is_none() && !state.busy(),
            egui::Button::new("Choose local Flycast .cht file"),
        )
        .clicked()
    {
        let (tx, rx) = mpsc::channel();
        state.receiver = Some(rx);
        state.prepare_source_pick();
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            #[cfg(not(test))]
            let result = rfd::FileDialog::new()
                .add_filter("Flycast native cheats", &["cht"])
                .pick_file()
                .ok_or("No file selected".to_string())
                .and_then(|p| inspect_source(&p));
            #[cfg(test)]
            let result = Err("Tests must supply a disposable source explicitly".into());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    ui.label("Product code declared by your cheat source (required to review matching entries)");
    let mut changed = ui
        .add_enabled(
            !state.busy(),
            egui::TextEdit::singleline(&mut state.expected_product),
        )
        .changed();
    if let Some(l) = &state.loaded {
        ui.label(format!("Local source: {}", l.path.display()));
        ui.label(format!("Parser diagnostics: {:?}", l.file.warnings));
        let clean = l.file.warnings.is_empty();
        for (i, e) in l.file.entries.iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .add_enabled(
                        clean && supported(e) && !state.install.busy(),
                        egui::Checkbox::new(&mut state.selected[i], &e.description),
                    )
                    .changed();
                changed |= ui
                    .add_enabled(
                        clean && supported(e) && state.selected[i] && !state.install.busy(),
                        egui::Checkbox::new(&mut state.enabled[i], "Intended enabled state"),
                    )
                    .changed();
                ui.label(format!("Source enabled: {} · {:?}", e.enabled, e.code));
            });
            ui.collapsing(format!("Native fields for entry {}", e.index), |ui| {
                for (k, v) in &e.fields {
                    ui.monospace(format!("{k}={v}"));
                }
            });
        }
    }
    if changed {
        state.reviewed = false;
        state.install.invalidate();
    }
    if ui
        .add_enabled(
            state.loaded.is_some() && product.is_ok() && !state.busy(),
            egui::Button::new("Preview selected Flycast cheats"),
        )
        .clicked()
    {
        if let Err(e) = state.review(&product) {
            state.failure = Some(e);
        } else {
            state.failure = None;
            let loaded = state.loaded.as_ref().unwrap();
            state.install.preview(
                ui.ctx(),
                state
                    .launch
                    .clone()
                    .unwrap_or_else(|| Err("Resolve Blue Launch first".into())),
                game.to_path_buf(),
                product.as_ref().unwrap().clone(),
                loaded.path.clone(),
                state
                    .selected
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| **s)
                    .map(|(i, _)| (i, state.enabled[i]))
                    .collect(),
            );
        }
    }
    if state.reviewed {
        ui.heading("Selected entries");
        if let Some(l) = &state.loaded {
            for (i, e) in l
                .file
                .entries
                .iter()
                .enumerate()
                .filter(|(i, _)| state.selected[*i])
            {
                ui.label(format!(
                    "{} · intended enabled: {} · {:?}",
                    e.description, state.enabled[i], e.code
                ));
            }
        }
    }
    if let Some(failure) = &state.failure {
        ui.colored_label(egui::Color32::YELLOW, failure);
    }
    ui.label(INSTALL_BLOCKER);
    install::show(ui, &mut state.install, state.launch.clone(), game, &product);
    if state.receiver.is_some() {
        ui.spinner();
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests;
