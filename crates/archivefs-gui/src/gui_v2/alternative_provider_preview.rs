//! Advisory comparison of supplied provider evidence; never launch authority.
use archivefs_core::alternative_launch_provider_planner::*;
use eframe::egui;
use serde::Deserialize;

const EXAMPLE: &str = r#"{
  "schema": 1,
  "source": "Describe where and when these observations were obtained",
  "game": {"canonical_id":null,"edition":null,"confidence":"unknown"},
  "providers": [{"id":"template","name":"Unverified provider template","kind":"source_port","version":null,"version_supported":null,"installed":false,"supported_game_ids":[],"requirements":[{"asset_kind":"game_data_archive","role":"Original game data (replace with documented requirements)","edition":null}]}],
  "assets": []
}"#;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    schema: u8,
    source: String,
    game: AlternativeLaunchGameIdentity,
    providers: Vec<AlternativeLaunchProvider>,
    assets: Vec<AlternativeLaunchAssetEvidence>,
}
struct Report {
    source: String,
    plan: AlternativeLaunchPlan,
}
fn compare(text: &str) -> Result<Report, String> {
    if text.len() > 64 * 1024 {
        return Err("Observation text exceeds 64 KiB.".into());
    }
    let input: Input =
        serde_json::from_str(text).map_err(|_| "Invalid provider observation JSON.".to_string())?;
    if input.schema != 1 || input.source.trim().is_empty() {
        return Err("Schema 1 and an observation source are required.".into());
    }
    if input.providers.is_empty()
        || input.providers.len() > 16
        || input.assets.len() > 256
        || input
            .providers
            .iter()
            .any(|p| p.requirements.len() > 64 || p.supported_game_ids.len() > 256)
    {
        return Err(
            "Supply 1–16 providers, at most 256 assets/game IDs and 64 requirements per provider."
                .into(),
        );
    }
    Ok(Report {
        source: input.source,
        plan: plan_alternative_launches(input.game, input.providers, input.assets),
    })
}
pub(super) struct State {
    text: String,
    result: Option<Result<Report, String>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            text: EXAMPLE.into(),
            result: None,
        }
    }
}
impl State {
    fn edited(&mut self) {
        self.result = None;
    }
}
pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    ui.heading("Alternative-provider requirements");
    ui.label("Read-only comparison of your supplied observations. No discovery, installation, configuration or launch.");
    ui.label("Claims about identity, installed providers, versions, support and original assets are not independently verified. Even an agreeing report is advisory and cannot authorise a launch. Refresh the observations when anything changes.");
    ui.collapsing("Observation format", |ui| {
        ui.label("Schema 1: source, game, providers, assets. Each provider needs id, name, kind (emulator/source_port/engine_reimplementation/native_port/recompilation), version, version_supported, installed, supported_game_ids and requirements. Each requirement needs asset_kind (grp/rff/art/game_data_archive), role and edition. Asset observations need asset_kind, identity, edition, present, source and reason. Unknown optional values are null. No built-in provider support list is inferred.");
    });
    if ui
        .add(
            egui::TextEdit::multiline(&mut state.text)
                .code_editor()
                .desired_rows(12)
                .char_limit(64 * 1024),
        )
        .changed()
    {
        state.edited();
    }
    if ui.button("Compare supplied requirements").clicked() {
        state.result = Some(compare(&state.text));
    }
    match &state.result {
        Some(Ok(report)) => {
            ui.label(format!(
                "Observation source (unverified): {}",
                report.source
            ));
            for projection in &report.plan.providers {
                ui.group(|ui| {
                    ui.strong(&projection.provider.name);
                    if projection.readiness == AlternativeLaunchReadiness::Ready {
                        ui.label("Supplied fields agree (unverified)");
                    } else {
                        ui.label(format!("Reported assessment: {:?}", projection.readiness));
                        ui.label(&projection.reason);
                    }
                    ui.label("This assessment describes supplied claims only; it does not establish actual compatibility or permission to use assets.");
                    for requirement in &projection.evidence.required_assets { ui.label(format!("Required: {} ({:?}, edition {:?})", requirement.role, requirement.asset_kind, requirement.edition)); }
                    for asset in &projection.evidence.discovered_assets { ui.label(format!("Reported asset: {:?}; present {}; source {}; {}", asset.asset_kind, asset.present, asset.source, asset.reason)); }
                });
            }
        }
        Some(Err(error)) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        None => {
            ui.label("Supply observations and compare to produce a report.");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> String {
        serde_json::json!({"schema":1,"source":"synthetic observation", "game":{"canonical_id":"example", "edition":null,"confidence":"exact"}, "providers":[{"id":"test","name":"Synthetic provider","kind":"source_port","version":"1","version_supported":true,"installed":true,"supported_game_ids":["example"],"requirements":[{"asset_kind":"grp","role":"original data","edition":null}]}],"assets":[]}).to_string()
    }
    #[test]
    fn missing_assets_are_reported_and_edits_invalidate() {
        let report = compare(&fixture()).unwrap();
        assert!(report.plan.read_only);
        assert_eq!(
            report.plan.providers[0].readiness,
            AlternativeLaunchReadiness::OriginalAssetsMissing
        );
        let mut state = State {
            text: fixture(),
            result: Some(Ok(report)),
        };
        state.edited();
        assert!(state.result.is_none());
    }
    #[test]
    fn limits_and_unknown_versions_refuse_readiness() {
        assert!(compare(&"x".repeat(65537)).is_err());
        assert_eq!(
            compare(EXAMPLE).unwrap().plan.providers[0].readiness,
            AlternativeLaunchReadiness::IdentityInsufficient
        );
        let text = fixture().replace("\"version_supported\":true", "\"version_supported\":null");
        assert_ne!(
            compare(&text).unwrap().plan.providers[0].readiness,
            AlternativeLaunchReadiness::Ready
        );
    }
    #[test]
    fn blue_page_renders() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut state));
        });
    }
}
