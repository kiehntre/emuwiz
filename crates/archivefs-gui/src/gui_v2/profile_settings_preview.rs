//! In-memory explanation of untrusted pasted per-game settings; no config IO.
use archivefs_core::game_profile_planner::*;
use eframe::egui;

const LIMIT: &str = "Pasted settings are unverified observations. Classification is not a recommendation to apply: game identity, exact emulator version and source provenance remain unverified. No configuration, saves, BIOS or game files are read or changed.";
#[derive(Default)]
pub(super) struct State {
    dolphin: bool,
    text: String,
    report: Option<Result<GameProfilePlan, String>>,
}
fn preview(text: &str, dolphin: bool) -> Result<GameProfilePlan, String> {
    if text.len() > 65536 || text.lines().count() > 256 {
        return Err("Use at most 64 KiB and 256 lines.".into());
    }
    let emulator = if dolphin { "Dolphin" } else { "PCSX2" };
    let platform = if dolphin { "GameCube" } else { "PS2" };
    let candidate = GameProfileCandidate {
        platform: platform.into(),
        game_identity: None,
        identity_confidence: GameProfileIdentityConfidence::Unknown,
        emulator: emulator.into(),
        emulator_version: None,
        profile: None,
        host: GameProfileHostContext {
            operating_system: None,
            gpu: None,
            gpu_driver: None,
            refresh_hz: None,
            resolution: None,
        },
    };
    let mut section = String::new();
    let mut settings = Vec::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with([';', '#']))
    {
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or("Expected INI key=value lines or [section] headings.")?;
        if key.trim().is_empty() {
            return Err("A setting key is empty.".into());
        }
        let key = if section.is_empty() {
            key.trim().into()
        } else {
            format!("{}.{}", section, key.trim())
        };
        settings.push(GameProfileSetting {
            key,
            proposed_value: value.trim().into(),
            evidence: GameProfileEvidence {
                provider: "Pasted settings (unverified)".into(),
                source_version: None,
                source_date: None,
                platform: Some(platform.into()),
                game_identity: None,
                game_identity_confidence: GameProfileIdentityConfidence::Unknown,
                emulator: emulator.into(),
                emulator_version: None,
                profile: None,
                reason: "User-supplied text; no verified association or applicability.".into(),
            },
        });
    }
    if settings.is_empty() {
        return Err("Paste at least one setting.".into());
    }
    Ok(plan_game_profile_recommendations(candidate, settings))
}
pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    ui.heading("Per-game settings explanation");
    ui.label(LIMIT);
    let mut edited = ui
        .checkbox(
            &mut state.dolphin,
            "Dolphin GameCube settings (otherwise PCSX2)",
        )
        .changed();
    ui.label("Paste INI settings to classify. Wii-specific applicability is not assessed here.");
    edited |= ui
        .add(
            egui::TextEdit::multiline(&mut state.text)
                .char_limit(65536)
                .desired_rows(8),
        )
        .changed();
    if edited {
        state.report = None;
    }
    if ui.button("Explain settings").clicked() {
        state.report = Some(preview(&state.text, state.dolphin));
    }
    match &state.report {
        Some(Err(e)) => {
            ui.label(e);
        }
        Some(Ok(p)) => {
            ui.label(format!(
                "{} settings; {} conflicts. No value is selected for conflicting settings.",
                p.recommendations.len(),
                p.conflicts.len()
            ));
            for r in &p.recommendations {
                ui.group(|ui| {
                    ui.label(format!(
                        "{}: {:?} / {:?}",
                        r.key, r.classification, r.applicability
                    ));
                    ui.label(&r.reason);
                });
            }
        }
        None => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_preview_preserves_unknown_authority_and_conflicts() {
        let p = preview(
            "[EmuCore]\nBiosPath=/private/bios\nGameFixes=1\nGameFixes=0\nUnknownSetting=x",
            false,
        )
        .unwrap();
        assert!(p.read_only);
        assert_eq!(p.conflicts.len(), 1);
        assert!(
            p.recommendations
                .iter()
                .any(|r| r.classification == GameProfileSettingClass::GlobalDangerous)
        );
        assert!(
            !p.recommendations
                .iter()
                .any(|r| r.applicability == GameProfileApplicability::Recommendable)
        );
        assert!(p.candidate.game_identity.is_none());
        assert!(preview("bad line", false).is_err());
        assert!(preview(&"x".repeat(65537), false).is_err());
    }
    #[test]
    fn blue_settings_route_is_read_only_and_renders() {
        let tools = super::super::all_tools::groups();
        assert!(tools.iter().flat_map(|g| &g.tools).any(|t| t.route
            == super::super::routes::Route::Tool(super::super::routes::ToolPage::ProfileSettings)));
        let mut state = State {
            text: "CPUThread=True".into(),
            dolphin: true,
            report: Some(preview("CPUThread=True", true)),
        };
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut state));
        });
    }
}
