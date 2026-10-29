//! Explicit POKE choices at the existing RetroArch launch boundary.
use archivefs_core::classic_trainer_evidence::launch::{self, PokeLaunchPreview};
use archivefs_core::emulator_environment::HostReadOnlyFilesystem;
use archivefs_core::emulator_environment::retroarch::{DiscoveryEnvironment, ProfileKind};
use archivefs_core::launch::cheat_runtime_retroarch::{
    RetroArchCheatLaunch, RetroArchCheatSelection,
};
use archivefs_core::launch::libretro_probe::ProbeLauncher;
use archivefs_core::launch::{RetroArchLaunchRequest, preflight_retroarch_launch};
use eframe::egui;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

type Scope = RetroArchLaunchRequest;
type PreviewResult = Result<PokeLaunchPreview, String>;
#[derive(Default)]
pub(super) struct Chooser {
    scope: Option<Scope>,
    enabled: bool,
    selected: BTreeSet<usize>,
    acknowledged: bool,
    preview: Option<PokeLaunchPreview>,
    pending: Option<Receiver<PreviewResult>>,
    problem: Option<String>,
    requested: Vec<String>,
    profile_status: Option<String>,
}

fn profile_path(game: &Path) -> Result<PathBuf, String> {
    let database = archivefs_core::default_database_path().map_err(|e| e.to_string())?;
    let root = database.parent().ok_or("No application data folder.")?;
    let digest = Sha256::digest(game.as_os_str().as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Ok(root
        .join("cheat-profiles")
        .join(format!("spectrum-{hex}.json")))
}
#[derive(Clone)]
pub(super) struct Selection {
    preview: PokeLaunchPreview,
    entries: Vec<usize>,
    acknowledged: bool,
    request: RetroArchLaunchRequest,
}

impl Chooser {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        request: &RetroArchLaunchRequest,
        busy: bool,
    ) -> Option<Selection> {
        let scope = request.clone();
        if self.scope.as_ref() != Some(&scope) {
            *self = Self {
                scope: Some(scope),
                ..Default::default()
            };
        }
        if request.expected_platform_id != "ZX Spectrum" {
            return None;
        }
        let mut selection = None;
        ui.collapsing("POKE trainers (optional, default OFF)", |ui| {
            ui.label(launch::LIMITATION);
            if !self.requested.is_empty() {
                ui.label(format!("Last requested POKEs: {}", self.requested.join(", ")));
            }
            ui.label("Verified association does not prove gameplay compatibility. Prepared cheat files and a started emulator do not confirm activation. Ordinary Play remains available.");
            if request.profile.profile_kind != ProfileKind::Native || request.core_stem != "fuse" {
                ui.label("Trainer activation is blocked for this emulator configuration. Select native RetroArch with the verified Fuse core.");
                return;
            }
            ui.add_enabled_ui(!busy, |ui| {
                ui.checkbox(&mut self.enabled, "Use explicitly selected POKE trainers for this launch");
                if !self.enabled { self.selected.clear(); self.acknowledged = false; return; }
                if let Some(receiver) = &self.pending {
                    match receiver.try_recv() {
                        Ok(Ok(preview)) => { self.preview = Some(preview); self.pending = None; }
                        Ok(Err(error)) => { self.problem = Some(error); self.pending = None; }
                        Err(mpsc::TryRecvError::Disconnected) => { self.problem = Some("Trainer check stopped; nothing was launched.".into()); self.pending = None; }
                        Err(mpsc::TryRecvError::Empty) => { ui.spinner(); ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)); }
                    }
                }
                if self.pending.is_none() && ui.button("Load / refresh verified POKE entries").clicked() {
                    self.preview = None; self.selected.clear(); self.acknowledged = false; self.problem = None;
                    let path = request.selected_content_path.clone();
                    let (sender, receiver) = mpsc::channel(); self.pending = Some(receiver);
                    let context = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let result = crate::gui_v2::classic_trainers::current_launch_evidence(&path)
                            .and_then(|(game, roots)| launch::preview(&game, &path, &roots));
                        let _ = sender.send(result); context.request_repaint();
                    });
                }
                if let Some(error) = &self.problem { ui.label(format!("Trainer activation blocked: {error}")); }
                if let Some(preview) = &self.preview {
                    ui.label(format!("Pinned game identity (matches this verified copy): {}", preview.identity_summary()));
                    ui.label("Source: local same-name .pok, bound to this DAT-verified snapshot by .pok.target. World of Spectrum/ZXDB references alone do not verify this release.");
                    for (index, title) in preview.titles().iter().enumerate() {
                        let mut selected = self.selected.contains(&index);
                        if ui.checkbox(&mut selected, format!("{}: {title}", index + 1)).changed() {
                            self.acknowledged = false;
                            if selected { self.selected.insert(index); } else { self.selected.remove(&index); }
                        }
                        if let Some(detail) = preview.entry_summary(index) { ui.small(detail); }
                    }
                    let requested: Vec<_> = self.selected.iter().map(|&i| preview.titles()[i].clone()).collect();
                    ui.label(format!("Requested POKEs: {}", if requested.is_empty() { "none".into() } else { requested.join(", ") }));
                    ui.horizontal(|ui| {
                        if ui.button("Load saved POKE profile").clicked() {
                            self.profile_status = Some(match profile_path(preview.game_path()).and_then(|path| launch::load_profile(&path, preview)) {
                                Ok(entries) => { self.selected = entries.into_iter().collect(); self.acknowledged = false; "Saved choice loaded. Review the game, addresses and selection again before launch.".into() },
                                Err(error) => format!("Saved choice unavailable: {error}"),
                            });
                        }
                        if ui.add_enabled(!self.selected.is_empty(), egui::Button::new("Save selected POKE profile")).clicked() {
                            let entries: Vec<_> = self.selected.iter().copied().collect();
                            self.profile_status = Some(match profile_path(preview.game_path()).and_then(|path| launch::save_profile(&path, preview, &entries)) {
                                Ok(()) => "Saved choice for this exact snapshot and trainer. No cheat was activated.".into(),
                                Err(error) => format!("Choice was not saved: {error}"),
                            });
                        }
                    });
                    if let Some(status) = &self.profile_status { ui.label(status); }
                    ui.label("This action launches only these selected POKE entries. Sources, pin, game identity and core are checked again before launch. This launch starts from the snapshot with automatic state loading disabled. In-game activation remains unconfirmed.");
                    ui.label("These trainers require review before launch. Approval does not confirm that they work in this game.");
                    ui.add_enabled_ui(!requested.is_empty(), |ui| { ui.checkbox(&mut self.acknowledged, "I reviewed these POKE trainers for this game"); });
                    if ui.add_enabled(!requested.is_empty() && self.acknowledged, egui::Button::new("Launch with selected POKE trainers")).clicked() {
                        self.requested = requested;
                        selection = Some(Selection { preview: preview.clone(), entries: self.selected.iter().copied().collect(), acknowledged: self.acknowledged, request: request.clone() });
                        self.enabled = false; self.selected.clear(); self.acknowledged = false;
                    }
                }
            });
        });
        selection
    }
}

pub(super) fn launch(
    request: &RetroArchLaunchRequest,
    selected: &Selection,
    probe: &dyn ProbeLauncher,
    environment: &DiscoveryEnvironment,
) -> Result<RetroArchCheatLaunch, String> {
    if !selected.acknowledged || &selected.request != request {
        return Err("Trainer review approval is absent or stale; select and review again.".into());
    }
    if request.profile.profile_kind != ProfileKind::Native
        || request.core_stem != "fuse"
        || request.expected_platform_id != "ZX Spectrum"
        || request.selected_content_path != selected.preview.game_path()
    {
        return Err("Trainer activation refused: game or emulator selection changed. Ordinary Play is available.".into());
    }
    let command = preflight_retroarch_launch(request, &HostReadOnlyFilesystem, environment)
        .map_err(|e| format!("Trainer launch preflight refused: {e:?}"))?;
    let (game, roots) =
        crate::gui_v2::classic_trainers::current_launch_evidence(&request.selected_content_path)?;
    let mut prepared = launch::prepare(
        &selected.preview,
        &game,
        &roots,
        &command.selection.core_library,
        &command.executable,
        &selected.entries,
    )?;
    // Preserve the assessor's NeedsReview state. Only this explicit, scoped
    // user acknowledgement authorizes the existing planner's review gate.
    for selection in &mut prepared.selections {
        selection.review_acknowledged = selected.acknowledged;
    }
    super::retroarch_cheats::launch_selection(
        request,
        RetroArchCheatSelection {
            candidates: prepared.candidates,
            selections: prepared.selections,
            core_library_name: String::new(),
            effective_overrides: vec![],
            expected_core: Some(command.selection.core_library),
            expected_executable: Some(command.executable),
            additional_sources: prepared.sources,
            start_from_snapshot: true,
        },
        probe,
        environment,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::classic_trainer_evidence::ClassicTrainerGame;
    use archivefs_core::emulator_environment::retroarch::{ProfileRef, ProfileScope};
    use archivefs_core::safe_read::TrustedRoots;
    use sha2::{Digest, Sha256};

    #[test]
    fn distinct_non_utf8_game_paths_have_distinct_profile_paths() {
        use std::os::unix::ffi::OsStringExt;

        let first = PathBuf::from(std::ffi::OsString::from_vec(b"game-\xff.z80".to_vec()));
        let second = PathBuf::from(std::ffi::OsString::from_vec(b"game-\xfe.z80".to_vec()));
        assert_ne!(
            profile_path(&first).unwrap(),
            profile_path(&second).unwrap()
        );
    }

    fn fixture() -> (tempfile::TempDir, RetroArchLaunchRequest, PokeLaunchPreview) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.z80");
        let mut bytes = vec![0; 30 + 49152];
        bytes[7] = 128;
        let trainer = "N Lives\nZ 8 40000 9 0\nN Energy\nZ 8 40001 8 0\nY\n";
        let hash = |b: &[u8]| {
            Sha256::digest(b)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        std::fs::write(&path, &bytes).unwrap();
        std::fs::write(path.with_extension("pok"), trainer).unwrap();
        std::fs::write(
            path.with_extension("pok.target"),
            format!(
                "trainer-sha256 {}\ngame-sha256 {}\n",
                hash(trainer.as_bytes()),
                hash(&bytes)
            ),
        )
        .unwrap();
        let game = ClassicTrainerGame {
            platform: "ZX Spectrum".into(),
            identity_verified: true,
            identities: vec![format!("sha256:{}", hash(&bytes))],
        };
        let preview =
            launch::preview(&game, &path, &TrustedRoots::from_paths([dir.path()])).unwrap();
        let request = RetroArchLaunchRequest {
            selected_content_path: path,
            expected_platform_id: "ZX Spectrum".into(),
            expected_game_key: "game".into(),
            profile: ProfileRef {
                profile_kind: ProfileKind::Native,
                scope: ProfileScope::User,
            },
            core_stem: "fuse".into(),
            saturn_core_selection: None,
            expected_snapshot_evidence: None,
            expected_appimage_executable: None,
            expected_executable_binding: None,
            expected_configuration_file: None,
        };
        (dir, request, preview)
    }

    fn frame(
        chooser: &mut Chooser,
        ctx: &egui::Context,
        request: &RetroArchLaunchRequest,
        events: Vec<egui::Event>,
    ) -> (Option<Selection>, Vec<(String, egui::Pos2)>) {
        // Headless frames do not advance wall time. Finish the disclosure
        // animation so subsequent clicks target fully visible controls.
        ctx.style_mut(|style| style.animation_time = 0.0);
        let mut selection = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 1700.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    selection = chooser.show(ui, request, false);
                });
            },
        );
        let texts = output
            .shapes
            .iter()
            .filter_map(|s| {
                if let egui::Shape::Text(t) = &s.shape {
                    Some((t.galley.text().to_string(), t.pos + t.galley.size() * 0.5))
                } else {
                    None
                }
            })
            .collect();
        (selection, texts)
    }

    fn click(
        chooser: &mut Chooser,
        ctx: &egui::Context,
        request: &RetroArchLaunchRequest,
        label: &str,
    ) -> Option<Selection> {
        let (_, texts) = frame(chooser, ctx, request, vec![]);
        let pos = texts
            .iter()
            .find(|(s, _)| s == label)
            .unwrap_or_else(|| panic!("missing {label}: {texts:?}"))
            .1;
        let mut selected = None;
        for pressed in [true, false] {
            selected = frame(
                chooser,
                ctx,
                request,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            )
            .0
            .or(selected);
        }
        selected
    }

    #[test]
    fn default_off_selection_and_explicit_launch_click_are_distinct() {
        let (_dir, request, preview) = fixture();
        let ctx = egui::Context::default();
        let mut chooser = Chooser::default();
        frame(&mut chooser, &ctx, &request, vec![]);
        assert!(!chooser.enabled);
        assert!(chooser.selected.is_empty());
        click(
            &mut chooser,
            &ctx,
            &request,
            "POKE trainers (optional, default OFF)",
        );
        chooser.preview = Some(preview);
        assert!(
            click(
                &mut chooser,
                &ctx,
                &request,
                "Use explicitly selected POKE trainers for this launch"
            )
            .is_none()
        );
        assert!(chooser.selected.is_empty());
        assert!(click(&mut chooser, &ctx, &request, "1: Lives").is_none());
        assert_eq!(chooser.selected, BTreeSet::from([0]));
        let (_, texts) = frame(&mut chooser, &ctx, &request, vec![]);
        assert!(texts.iter().any(|(s, _)| s == "Requested POKEs: Lives"));
        assert!(
            texts
                .iter()
                .any(|(s, _)| s.contains("activation remains unconfirmed"))
        );
        assert!(
            click(
                &mut chooser,
                &ctx,
                &request,
                "Launch with selected POKE trainers"
            )
            .is_none()
        );
        click(
            &mut chooser,
            &ctx,
            &request,
            "I reviewed these POKE trainers for this game",
        );
        let selected = click(
            &mut chooser,
            &ctx,
            &request,
            "Launch with selected POKE trainers",
        )
        .unwrap();
        assert_eq!(selected.entries, vec![0]);
        assert!(selected.acknowledged);
        assert!(!chooser.acknowledged);
        assert!(!chooser.enabled);
        assert!(chooser.selected.is_empty());
    }

    #[test]
    fn game_change_discards_selections_and_pending_results() {
        let (_dir, request, preview) = fixture();
        let ctx = egui::Context::default();
        let mut chooser = Chooser::default();
        frame(&mut chooser, &ctx, &request, vec![]);
        chooser.enabled = true;
        chooser.preview = Some(preview.clone());
        chooser.selected.insert(0);
        let (sender, receiver) = mpsc::channel();
        chooser.pending = Some(receiver);
        let mut other = request.clone();
        other.expected_game_key = "other".into();
        assert!(frame(&mut chooser, &ctx, &other, vec![]).0.is_none());
        assert!(!chooser.enabled);
        assert!(chooser.preview.is_none());
        assert!(chooser.selected.is_empty());
        assert!(sender.send(Ok(preview)).is_err());
    }

    #[test]
    fn unsupported_profile_never_offers_an_activation_button() {
        let (_dir, mut request, _) = fixture();
        request.profile.profile_kind = ProfileKind::Flatpak;
        let ctx = egui::Context::default();
        let mut chooser = Chooser::default();
        frame(&mut chooser, &ctx, &request, vec![]);
        click(
            &mut chooser,
            &ctx,
            &request,
            "POKE trainers (optional, default OFF)",
        );
        let (selection, texts) = frame(&mut chooser, &ctx, &request, vec![]);
        assert!(selection.is_none());
        assert!(!chooser.enabled);
        assert!(
            texts
                .iter()
                .any(|(s, _)| s.contains("blocked for this emulator"))
        );
        assert!(
            !texts
                .iter()
                .any(|(s, _)| s == "Launch with selected POKE trainers")
        );
    }
    #[test]
    fn changing_entries_or_installation_revokes_review_approval() {
        let (_dir, request, preview) = fixture();
        let ctx = egui::Context::default();
        let mut chooser = Chooser::default();
        frame(&mut chooser, &ctx, &request, vec![]);
        click(
            &mut chooser,
            &ctx,
            &request,
            "POKE trainers (optional, default OFF)",
        );
        click(
            &mut chooser,
            &ctx,
            &request,
            "Use explicitly selected POKE trainers for this launch",
        );
        chooser.preview = Some(preview);
        click(&mut chooser, &ctx, &request, "1: Lives");
        click(
            &mut chooser,
            &ctx,
            &request,
            "I reviewed these POKE trainers for this game",
        );
        assert!(chooser.acknowledged);
        click(&mut chooser, &ctx, &request, "2: Energy");
        assert!(!chooser.acknowledged);
        assert!(
            click(
                &mut chooser,
                &ctx,
                &request,
                "Launch with selected POKE trainers"
            )
            .is_none()
        );
        click(
            &mut chooser,
            &ctx,
            &request,
            "I reviewed these POKE trainers for this game",
        );
        assert!(chooser.acknowledged);
        let mut changed = request.clone();
        changed.expected_configuration_file = Some("/different/settings.cfg".into());
        frame(&mut chooser, &ctx, &changed, vec![]);
        assert!(!chooser.acknowledged);
        assert!(!chooser.enabled);
        assert!(chooser.preview.is_none());
        assert!(chooser.selected.is_empty());
    }
}
