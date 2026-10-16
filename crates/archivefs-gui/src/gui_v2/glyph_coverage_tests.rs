//! Symbols in Blue-visible strings must exist in egui's default fonts;
//! a missing glyph is drawn as an empty box.

use eframe::egui;

fn missing(candidates: &str) -> Vec<char> {
    let context = egui::Context::default();
    super::readable_style(&context);
    let _ = context.run(egui::RawInput::default(), |_| {});
    context.fonts_mut(|fonts| {
        candidates
            .chars()
            .filter(|c| !c.is_whitespace())
            .filter(|c| !fonts.has_glyph(&egui::FontId::proportional(14.0), *c))
            .collect()
    })
}

/// Symbols the GUI draws as text. `→` and `✓` are deliberately absent:
/// the default fonts lack them (they are written `->`/`›` or painted).
const USED: &str = "▶★›…—–·×•";

#[test]
fn every_symbol_the_gui_draws_as_text_has_a_glyph() {
    assert_eq!(missing(USED), Vec::<char>::new());
    // The two that drew boxes are still missing, so they must stay out.
    assert_eq!(missing("→✓"), vec!['→', '✓']);
}

#[test]
fn no_gui_source_string_uses_a_symbol_the_fonts_lack() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs")
                || path.ends_with("glyph_coverage_tests.rs")
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (number, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                // Strings that come from core are compared in a few tests.
                if code.contains("rendered_text_contains") || code.contains("assert") {
                    continue;
                }
                if line.contains('→') || line.contains('✓') {
                    offenders.push(format!("{}:{}", path.display(), number + 1));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}
