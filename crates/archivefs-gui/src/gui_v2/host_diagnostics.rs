//! Host facts from the recovered observer. Never changes launch readiness.
use archivefs_core::host_capability_readiness::{HostCapabilities, detect_host_capabilities};
use eframe::egui;
use std::sync::mpsc::{self, Receiver, TryRecvError};
const LIMIT: &str = "Read-only host snapshot. CPU flags and total RAM do not prove an emulator will run well. Graphics APIs, driver compatibility and version-specific emulator requirements remain unknown. This page does not change launch checks or settings.";
#[derive(Default)]
pub(super) struct State {
    snapshot: Option<HostCapabilities>,
    loading: Option<Receiver<HostCapabilities>>,
    error: Option<String>,
}
impl State {
    fn start(&mut self, context: &egui::Context) {
        self.snapshot = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        let context = context.clone();
        std::thread::spawn(move || {
            let _ = tx.send(detect_host_capabilities());
            context.request_repaint();
        });
    }
    fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        match rx.try_recv() {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.loading = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.error = Some("Host observation stopped. Try again.".into());
                self.loading = None;
            }
        }
    }
}
fn lines(snapshot: &HostCapabilities) -> Vec<String> {
    let mut lines = vec![
        format!(
            "Architecture: {}",
            snapshot.architecture.as_deref().unwrap_or("unknown")
        ),
        format!(
            "Total physical memory: {}",
            snapshot
                .memory_bytes
                .map(|n| format!("{:.2} GiB ({n} bytes)", n as f64 / 1_073_741_824.0))
                .unwrap_or_else(|| "unknown (not observed on this platform)".into())
        ),
        format!(
            "Positively detected CPU features: {}",
            if snapshot.cpu.available_features.is_empty() {
                "none observed; absence is unknown".into()
            } else {
                snapshot
                    .cpu
                    .available_features
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
        "Undetected CPU features are unknown, not proof of missing hardware support.".into(),
        "Graphics APIs and driver compatibility: unknown (no GPU probe performed).".into(),
    ];
    for evidence in snapshot.cpu.evidence.iter().chain(&snapshot.evidence) {
        lines.push(format!(
            "{:?} · {} · {}",
            evidence.state, evidence.source, evidence.detail
        ));
    }
    lines
}
pub(super) fn show(ui: &mut egui::Ui, state: &mut State) {
    state.poll();
    ui.heading("Host capability diagnostics");
    ui.label(LIMIT);
    if ui
        .add_enabled(
            state.loading.is_none(),
            egui::Button::new("Observe host now"),
        )
        .clicked()
    {
        state.start(ui.ctx());
    }
    if state.loading.is_some() {
        ui.spinner();
    }
    if let Some(error) = &state.error {
        ui.label(error);
    }
    if let Some(snapshot) = &state.snapshot {
        ui.label(
            "Observed when you pressed Observe host now. Refresh after hardware or host changes.",
        );
        for line in lines(snapshot) {
            ui.label(line);
        }
    } else if state.loading.is_none() {
        ui.label("No host observation yet.");
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_facts_are_never_presented_as_requirement_failures() {
        let text = lines(&HostCapabilities::default()).join("\n");
        assert!(text.contains("Architecture: unknown"));
        assert!(text.contains("Total physical memory: unknown"));
        assert!(text.contains("absence is unknown"));
        assert!(text.contains("no GPU probe"));
        let mut state = State {
            snapshot: Some(HostCapabilities::default()),
            ..Default::default()
        };
        let context = egui::Context::default();
        let _ = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut state));
        });
        assert!(state.loading.is_none());
    }
    #[test]
    fn shows_observed_values_and_replaces_the_previous_snapshot() {
        let mut snapshot = HostCapabilities {
            architecture: Some("fixture-arch".into()),
            memory_bytes: Some(8 * 1_073_741_824),
            ..Default::default()
        };
        snapshot.cpu.available_features.insert("sse2".into());
        let (tx, rx) = mpsc::channel();
        tx.send(snapshot).unwrap();
        let mut state = State {
            loading: Some(rx),
            ..Default::default()
        };
        state.poll();
        let text = lines(state.snapshot.as_ref().unwrap()).join("\n");
        assert!(text.contains("fixture-arch"));
        assert!(text.contains("8.00 GiB"));
        assert!(text.contains("sse2"));
        state.start(&egui::Context::default());
        assert!(state.snapshot.is_none());
    }
}
