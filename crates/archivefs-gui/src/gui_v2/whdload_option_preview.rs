//! Explanation only: pasted ToolTypes are never applied or bound to a game.
use archivefs_core::whdload_options::{
    WhdloadCustomMetadata, WhdloadOption, project_supplied_options,
};
use eframe::egui;
const LIMIT: &str = "Review supplied ToolTypes and slave Config text only. No .info, slave, game, emulator configuration or startup files are read or changed. Pasted text does not establish game/slave identity or launch compatibility. Missing options describe defaults within this supplied text, not your installation.";
type Report = (Vec<WhdloadOption>, Vec<WhdloadCustomMetadata>, Vec<String>);
#[derive(Default)]
pub(super) struct State {
    text: String,
    config: String,
    report: Option<Result<Report, String>>,
}
fn preview(text: &str, config: &str) -> Result<Report, String> {
    if text.len() + config.len() > 65536
        || text.lines().count() > 256
        || config
            .split(';')
            .filter(|record| !record.trim().is_empty())
            .count()
            > 256
    {
        return Err("Use at most 64 KiB, 256 ToolType lines and 256 Config records.".into());
    }
    let lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if lines.is_empty() && config.trim().is_empty() {
        return Err("Supply ToolTypes or Config metadata.".into());
    }
    Ok(project_supplied_options(&lines, config))
}
pub(super) fn show(ui: &mut egui::Ui, s: &mut State) {
    ui.heading("WHDLoad ToolType review");
    ui.label(LIMIT);
    ui.label("ToolTypes, one per line");
    let edited = ui
        .add(
            egui::TextEdit::multiline(&mut s.text)
                .char_limit(65536)
                .desired_rows(6),
        )
        .changed();
    ui.label("Optional slave Config text");
    let edited = ui
        .add(
            egui::TextEdit::multiline(&mut s.config)
                .char_limit(65536)
                .desired_rows(3),
        )
        .changed()
        || edited;
    if edited {
        s.report = None;
    }
    if ui.button("Explain supplied options").clicked() {
        s.report = Some(preview(&s.text, &s.config));
    }
    match &s.report {
        Some(Err(e)) => {
            ui.label(e);
        }
        Some(Ok((options, metadata, diagnostics))) => {
            for d in diagnostics {
                ui.label(d);
            }
            for o in options {
                ui.label(format!("{}: {:?} / {:?}", o.key, o.value, o.applicability));
                ui.label(&o.evidence.detail);
            }
            for m in metadata {
                ui.label(format!(
                    "CUSTOM{}: {:?} / {:?}",
                    m.slot, m.label, m.applicability
                ));
            }
        }
        None => {}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::whdload_options::WhdloadOptionApplicability;
    #[test]
    fn supplied_options_preserve_conflicts_unknowns_and_malformed_config() {
        let (o, _, d) = preview("PAL\nNTSC\nMYSTERY=1", "bad config").unwrap();
        assert!(
            o.iter()
                .any(|x| x.applicability == WhdloadOptionApplicability::Conflicting)
        );
        assert!(
            o.iter()
                .any(|x| x.applicability == WhdloadOptionApplicability::Unknown)
        );
        assert!(!d.is_empty());
        assert!(preview(&"x".repeat(65537), "").is_err());
    }
    #[test]
    fn blue_tooltype_review_renders() {
        let mut s = State {
            report: Some(preview("CUSTOM1=1", "C1:B:Unlimited lives;")),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show(ui, &mut s));
        });
    }
    #[test]
    fn config_record_budget_counts_an_unterminated_last_record() {
        assert!(preview("CUSTOM1=1", &vec!["C1:B:Example"; 257].join(";")).is_err());
        let (options, _, _) = preview("CUSTOM1=1", "C1:B").unwrap();
        assert_eq!(
            options
                .iter()
                .find(|o| o.key == "CUSTOM1")
                .unwrap()
                .applicability,
            WhdloadOptionApplicability::Unknown
        );
    }
}
