//! Blue entry point to the existing, narrowly supported RPCS3 activation UI.
//! No independent enabled-set, ordering, configuration writer or Undo engine.

use std::path::Path;

use archivefs_core::game_identity::{GameIdentityReport, IdentityPlatform};
use eframe::egui;

use super::{Activity, LoadState, NativeWorkflows, Route, Section};

fn supported_identity(report: Option<&GameIdentityReport>, path: &Path) -> bool {
    report.is_some_and(|report| {
        report.archive_path == path
            && report.platform == IdentityPlatform::PlayStation3
            && report.verified_ps3_title_id().is_some()
    })
}

impl NativeWorkflows {
    pub(in crate::gui_v2) fn show_mod_activation(
        &mut self,
        ui: &mut egui::Ui,
        selected: Option<&crate::gui_v2::library::Game>,
        activity: &mut Activity,
    ) -> Option<Route> {
        ui.heading("Enable / disable supported runtime patches");
        ui.label("This control uses RPCS3's native patch configuration. It does not enable ordinary file replacement packages, texture packs or a generic mod stack. Those retain their existing installation and Undo workflows in Available packages.");
        ui.label("No load-order editing is available: arranging records or plans does not prove emulator precedence. A configured patch is not proof of an in-game effect.");
        let Some(game) = selected else {
            ui.label("Choose a game before inspecting installed runtime patches.");
            return ui
                .button("Choose a game")
                .clicked()
                .then_some(Route::Section(Section::Games));
        };
        if game.archive.identity_report.as_ref().is_some_and(|report| {
            report.archive_path == game.archive.absolute_path
                && report.platform != IdentityPlatform::PlayStation3
                && report.platform != IdentityPlatform::Other
        }) {
            ui.label("Enable / disable is unsupported for this platform's mod formats. Use the existing package installation and guarded Undo workflows; no emulator activation contract is assumed.");
            return None;
        }
        if !supported_identity(
            game.archive.identity_report.as_ref(),
            &game.archive.absolute_path,
        ) {
            ui.label("Activation is unavailable for this game: this slice requires a structural PS3 title ID for the selected game. Filenames, catalogue labels and installation records cannot authorize activation.");
            return ui
                .button("Review identity")
                .clicked()
                .then_some(Route::ReviewIdentity(game.archive.id));
        }
        let path = game.archive.absolute_path.clone();
        if !self
            .app
            .cheat_workflow
            .as_ref()
            .is_some_and(|w| w.archive_path == path)
        {
            // Existing coordinator: read-only identity/cache inspection only.
            self.app.open_cheats_mods_workspace(ui.ctx(), path);
        }
        match self.app.cheat_workflow.as_ref() {
            Some(workflow) if workflow.archive_path == game.archive.absolute_path => {}
            Some(workflow) => {
                ui.label(format!("Previous game's workflow is still retained: {}. Return to that game to review its completion and History before switching. No activation or Undo controls are available for this selection.", workflow.display_name));
                return None;
            }
            None => {
                ui.label("This selection could not be bound to a game workflow. Refresh the library and try again; no activation controls are available.");
                return None;
            }
        }
        if !self
            .app
            .cheat_workflow
            .as_ref()
            .is_some_and(crate::cheats_mods::rpcs3::is_rpcs3_route)
        {
            ui.label("The selected Launch route does not provide supported RPCS3 activation. Resolve Launch Readiness first; this control will not change your emulator selection.");
            return ui
                .button("Review Launch Readiness")
                .clicked()
                .then_some(Route::Task {
                    section: Section::Launch,
                    game: game.archive.id,
                });
        }
        let live = match &self.app.state {
            LoadState::Ready(data) => Some(data.as_ref()),
            LoadState::Loading { previous, .. } => previous.as_deref(),
            LoadState::Error(_) => None,
        };
        // Reuse the selected Launch profile, process checks, fresh evidence,
        // exact confirmation and receipt-backed Undo of the existing owner.
        let selection = self.app.rpcs3_cheat_selection(live);
        if selection.is_err() && ui.button("Review Launch Readiness").clicked() {
            return Some(Route::Task {
                section: Section::Launch,
                game: game.archive.id,
            });
        }
        if let Some(workflow) = self.app.cheat_workflow.as_mut() {
            workflow.rpcs3.selection = Some(selection);
            let identity = crate::ready_game_identity(workflow).cloned();
            crate::cheats_mods::rpcs3::show(
                ui,
                &mut workflow.rpcs3,
                &workflow.archive_path,
                identity.as_ref(),
            );
        }
        self.observe_cheat_activity(activity);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::game_identity::*;

    #[test]
    fn activation_entry_requires_selected_structural_ps3_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game");
        let mut report = inspect_game_identity(&path, Some("PS3"));
        assert!(!supported_identity(None, &path));
        assert!(!supported_identity(Some(&report), &path));
        report.evidence.push(IdentityEvidence {
            kind: IdentityKind::Ps3TitleId,
            status: IdentityStatus::Candidate,
            value: Some("BLUS12345".into()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: path.clone(),
                member_path: None,
                member_index: None,
                method: "synthetic structural fixture".into(),
            },
            diagnostic: "fixture".into(),
        });
        assert!(!supported_identity(Some(&report), &path));
        report.evidence.last_mut().unwrap().status = IdentityStatus::Verified;
        assert!(supported_identity(Some(&report), &path));
        assert!(!supported_identity(
            Some(&report),
            &temp.path().join("other-game")
        ));
        report.platform = IdentityPlatform::PlayStation2;
        assert!(!supported_identity(Some(&report), &path));
    }

    fn bridge_fixture(context: &egui::Context) -> (NativeWorkflows, crate::gui_v2::library::Game) {
        use archivefs_core::patch_manager::{
            CheatRouteRequest, CheatRouteTarget, route_cheat_install,
        };
        let mut workflows = NativeWorkflows::new(context.clone());
        workflows.app = crate::tests::app_with_cheats_mods_context();
        let w = workflows.app.cheat_workflow.as_mut().unwrap();
        let target = CheatRouteTarget::standalone("rpcs3");
        let decision = route_cheat_install(&CheatRouteRequest {
            platform: Some("PS3".into()),
            selected: Some(target.clone()),
            ..Default::default()
        });
        w.platform = Some("PS3".into());
        w.routing = crate::CheatRoutingState::for_decision(decision, Some(target));
        let path = w.archive_path.clone();
        let mut row = crate::gui_v2::tests::archive(42, "Synthetic PS3", Some("PS3"));
        row.absolute_path = path.clone();
        // Only the entry-point gate is under test. Native activation separately
        // re-derives real SFO/version/profile evidence in its existing fixtures.
        let mut report = inspect_game_identity(&path, Some("PS3"));
        report.evidence.push(IdentityEvidence {
            kind: IdentityKind::Ps3TitleId,
            status: IdentityStatus::Verified,
            value: Some("BLUS12345".into()),
            confidence: IdentityConfidence::StructuredMetadata,
            provenance: IdentityProvenance {
                archive_path: path.clone(),
                member_path: None,
                member_index: None,
                method: "synthetic bridge fixture".into(),
            },
            diagnostic: "fixture".into(),
        });
        row.identity_report = Some(report);
        let game = crate::gui_v2::library::Game::from_archive(row);
        (workflows, game)
    }

    fn render(
        context: &egui::Context,
        workflows: &mut NativeWorkflows,
        game: &crate::gui_v2::library::Game,
    ) -> String {
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert_eq!(
                    workflows.show_mod_activation(ui, Some(game), &mut Activity::default()),
                    None
                );
            });
        });
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn mods_reuses_rpcs3_workflow_and_refuses_unresolved_launch_without_applying() {
        use archivefs_core::patch_manager::CheatRouteTarget;
        let context = egui::Context::default();
        let (mut workflows, game) = bridge_fixture(&context);
        let path = game.archive.absolute_path.clone();
        let text = render(&context, &mut workflows, &game);
        assert!(text.contains("RPCS3 local patches"));
        let w = workflows.app.cheat_workflow.as_ref().unwrap();
        assert_eq!(w.archive_path, path);
        assert!(w.rpcs3.selection.as_ref().unwrap().is_err());
        assert!(!w.rpcs3.busy());
        assert_eq!(
            w.routing.selected_emulator,
            Some(CheatRouteTarget::standalone("rpcs3"))
        );
    }

    #[test]
    fn known_other_platform_refuses_activation_without_suggesting_reidentification() {
        let context = egui::Context::default();
        let (mut workflows, mut game) = bridge_fixture(&context);
        game.archive.identity_report.as_mut().unwrap().platform = IdentityPlatform::PlayStation2;
        let text = render(&context, &mut workflows, &game);
        assert!(text.contains("Enable / disable is unsupported for this platform"));
        assert!(!text.contains("Review identity"));
        assert!(!text.contains("RPCS3 local patches"));
        assert!(
            workflows
                .app
                .cheat_workflow
                .as_ref()
                .unwrap()
                .rpcs3
                .selection
                .is_none()
        );
    }

    #[test]
    fn retained_busy_workflow_never_shows_another_games_activation_or_undo() {
        let context = egui::Context::default();
        let (mut workflows, mut game) = bridge_fixture(&context);
        let old_path = workflows
            .app
            .cheat_workflow
            .as_ref()
            .unwrap()
            .archive_path
            .clone();
        workflows.app.cheat_workflow.as_mut().unwrap().rpcs3 =
            crate::cheats_mods::rpcs3::Rpcs3PageState::pending_activation_fixture();
        let temp = tempfile::tempdir().unwrap();
        game.archive.absolute_path = temp.path().join("another-game");
        game.archive.identity_report.as_mut().unwrap().archive_path =
            game.archive.absolute_path.clone();
        let text = render(&context, &mut workflows, &game);
        assert!(
            text.contains("Previous game's workflow is still retained"),
            "{text}"
        );
        assert!(!text.contains("RPCS3 local patches"));
        assert!(!text.contains("activation History / Undo"));
        let w = workflows.app.cheat_workflow.as_ref().unwrap();
        assert_eq!(w.archive_path, old_path);
        assert!(
            w.rpcs3.busy(),
            "Wrong-game panel must not consume the retained job result"
        );
        assert!(w.rpcs3.selection.is_none());
    }
}
