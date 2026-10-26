//! Manual observations only: no local identity authority or networking.
use archivefs_core::{launch::readiness::FirmwareReadiness, netplay_readiness::*};
use eframe::egui;
use serde::Deserialize;

const EXAMPLE: &str = r#"{
  "schema": 1,
  "left": {"provider":"retroarch", "platform":"", "sha256":"", "revision":"", "emulator_version":"", "core":"", "core_version":"", "player":1},
  "right": {"provider":"retroarch", "platform":"", "sha256":"", "revision":"", "emulator_version":"", "core":"", "core_version":"", "player":2}
}"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    provider: String,
    platform: String,
    sha256: String,
    revision: String,
    emulator_version: String,
    core: String,
    core_version: String,
    player: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    schema: u8,
    left: Observation,
    right: Observation,
}

impl Observation {
    fn peer(self, id: &str) -> Result<NetplayPeer, String> {
        let provider = match self.provider.as_str() {
            "retroarch" => NetplayProvider::RetroArch,
            "dolphin" => NetplayProvider::Dolphin,
            _ => return Err("Provider must be retroarch or dolphin.".into()),
        };
        if !self.sha256.is_empty()
            && (self.sha256.len() != 64 || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("SHA-256 must be empty (unknown) or exactly 64 hexadecimal digits.".into());
        }
        if !(1..=2).contains(&self.player) {
            return Err("This two-player comparison needs player 1 or 2 per peer.".into());
        }
        let known = |text: String| (!text.trim().is_empty()).then(|| text.trim().to_owned());
        let hash = known(self.sha256).map(|s| s.to_ascii_lowercase());
        Ok(NetplayPeer {
            peer_id: id.into(),
            emulator_identity: self.provider.clone(),
            provider,
            emulator_version: known(self.emulator_version),
            core: known(self.core).map(|name| NetplayCoreIdentity {
                name,
                version: known(self.core_version),
            }),
            game: NetplayGameIdentity {
                platform: self.platform.trim().into(),
                title: None,
                revision: known(self.revision),
                confidence: if hash.is_some() && !self.platform.trim().is_empty() {
                    NetplayIdentityConfidence::Strong
                } else {
                    NetplayIdentityConfidence::Unknown
                },
                content_hash: hash,
            },
            // Manual text cannot verify these requirements.
            firmware: FirmwareReadiness::Unknown,
            firmware_required: true,
            deterministic_settings: NetplayDeterministicSettings::Unknown,
            required_players: 2,
            controller_assignments: vec![NetplayControllerAssignment {
                player: self.player,
                controller: "manually reported".into(),
            }],
            network: NetplayNetworkCapability::Unknown,
            capabilities: vec![],
            requirements: vec![],
            evidence: vec![],
        })
    }
}

fn compare(text: &str) -> Result<NetplayReadinessPlan, String> {
    if text.len() > 64 * 1024 {
        return Err("Observation text exceeds 64 KiB.".into());
    }
    let pair: Pair = serde_json::from_str(text)
        .map_err(|_| "Invalid observation JSON (schema 1).".to_string())?;
    if pair.schema != 1 {
        return Err("Unsupported observation schema.".into());
    }
    Ok(plan_netplay_readiness(
        pair.left.peer("left")?,
        pair.right.peer("right")?,
    ))
}

pub(super) struct State {
    text: String,
    result: Option<Result<NetplayReadinessPlan, String>>,
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
    ui.heading("Netplay comparison");
    ui.label(
        "Compare two manually supplied observations. Read-only; no connection or emulator launch.",
    );
    ui.label("Paste schema-1 JSON below. Use empty strings for unknown values. SHA-256 values must describe the same content representation. These claims are not verified against your library or the other peer.");
    ui.label("Firmware, deterministic settings and network remain unknown, so this report cannot establish netplay readiness or emulator support. Recheck observations after either peer changes anything.");
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
    if ui.button("Compare observations").clicked() {
        state.result = Some(compare(&state.text));
    }
    match &state.result {
        Some(Ok(plan)) => {
            ui.label("Reported differences and unresolved requirements:");
            for issue in &plan.issues {
                ui.label(format!("• {issue:?}"));
            }
            ui.label("Manual comparison only; no verified compatibility or gameplay result.");
        }
        Some(Err(error)) => {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        None => {
            ui.label("Compare to produce a new report.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_claims_never_establish_readiness_and_edits_drop_results() {
        let text = EXAMPLE
            .replace(
                "\"sha256\":\"\"",
                &format!("\"sha256\":\"{}\"", "a".repeat(64)),
            )
            .replace("\"platform\":\"\"", "\"platform\":\"test\"");
        let plan = compare(&text).unwrap();
        assert!(plan.read_only);
        assert!(plan.issues.contains(&NetplayIssue::FirmwareUnknown));
        assert!(
            plan.issues
                .contains(&NetplayIssue::NetworkCapabilityUnknown)
        );
        assert_ne!(plan.compatibility, NetplayCompatibility::Compatible);
        let mut state = State {
            text,
            result: Some(Ok(plan)),
        };
        state.edited();
        assert!(state.result.is_none());
    }
    #[test]
    fn refuses_malformed_unbounded_or_unsupported_claims() {
        for text in [
            "x".repeat(65537),
            EXAMPLE.replace("\"schema\": 1", "\"schema\": 2"),
            EXAMPLE.replace("retroarch", "unknown"),
            EXAMPLE.replace("\"sha256\":\"\"", "\"sha256\":\"bad\""),
        ] {
            assert!(compare(&text).is_err());
        }
        let plan = compare(&EXAMPLE.replace("\"player\":2", "\"player\":1")).unwrap();
        assert!(
            plan.issues
                .contains(&NetplayIssue::DuplicatePlayerAssignment(1))
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
