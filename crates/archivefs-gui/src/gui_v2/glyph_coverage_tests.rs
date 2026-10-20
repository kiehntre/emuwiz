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
const USED: &str = "▶★›…—–·×•↔‹⬆⬇■";

#[test]
fn every_symbol_the_gui_draws_as_text_has_a_glyph() {
    assert_eq!(missing(USED), Vec::<char>::new());
    // The two that drew boxes are still missing, so they must stay out.
    assert_eq!(missing("→✓"), vec!['→', '✓']);
}

/// Arrows, technical symbols, box/shape drawing and dingbats: the ranges
/// where the default fonts have gaps. Text in other scripts (for example
/// Japanese fixture titles) is not a symbol and is not judged here.
fn is_symbol(c: char) -> bool {
    ('\u{2190}'..='\u{2BFF}').contains(&c)
}

#[test]
fn no_gui_source_string_uses_a_symbol_the_fonts_lack() {
    // Core strings (recovery hints, rename previews) are shown by the GUI too.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let roots = [manifest.join("src"), manifest.join("../archivefs-core/src")];
    let mut symbols = std::collections::BTreeMap::<char, Vec<String>>::new();
    let mut stack: Vec<std::path::PathBuf> = roots.to_vec();
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
            // Test files only echo or deliberately avoid symbols (negative
            // assertions, Unicode fixtures); drawn strings live in the sources.
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == "tests.rs"
                || name.ends_with("_tests.rs")
                || path.components().any(|part| part.as_os_str() == "tests")
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (number, line) in text.lines().enumerate() {
                let trimmed = line.trim_start();
                // In-file test modules assert on drawn text too.
                if trimmed.starts_with("//")
                    || trimmed.contains("assert")
                    || trimmed.contains("rendered_text_contains")
                {
                    continue;
                }
                // Code after a trailing comment is not a string.
                let code = line.split("//").next().unwrap_or(line);
                for c in code.chars().filter(|c| is_symbol(*c)) {
                    symbols.entry(c).or_default().push(format!(
                        "{}:{}",
                        path.display(),
                        number + 1
                    ));
                }
            }
        }
    }
    let all: String = symbols.keys().collect();
    let offenders: Vec<_> = missing(&all)
        .into_iter()
        .map(|c| format!("{c:?} U+{:04X} at {:?}", c as u32, symbols[&c]))
        .collect();
    assert!(offenders.is_empty(), "{offenders:#?}");
}
