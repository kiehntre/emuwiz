//! Presentation-only catalogue index: no source resolution, trust decisions or I/O.
use eframe::egui;

pub(crate) const PAGE_SIZE: usize = 40;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Group {
    #[default]
    Compatible,
    NeedsChecking,
    Unassigned,
    Incompatible,
    Disabled,
    All,
}
impl Group {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Compatible => "Compatible and ready",
            Self::NeedsChecking => "Needs checking / stale",
            Self::Unassigned => "Platform not established",
            Self::Incompatible => "Other platforms / unsupported",
            Self::Disabled => "Disabled",
            Self::All => "All catalogues",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Row {
    pub(crate) source_index: usize,
    pub(crate) name: String,
    pub(crate) search: String,
    pub(crate) group: Group,
}

#[derive(Clone, Default)]
pub(crate) struct State {
    pub(crate) query: String,
    pub(crate) group: Group,
    pub(crate) letter: Option<char>,
    page: usize,
    rows: Vec<Row>,
    matching: Vec<usize>,
    previous: Option<(String, Group, Option<char>)>,
}
impl State {
    pub(crate) fn replace(&mut self, mut rows: Vec<Row>) {
        // Stable alphabetical ordering inside explicit readiness groups. Never
        // choose a catalogue or infer platform/trust from a display name.
        for row in &mut rows {
            row.search = search_text(&row.search);
        }
        rows.sort_by_cached_key(|row| (rank(row.group), row.name.to_lowercase(), row.source_index));
        self.rows = rows;
        self.previous = None;
    }
    pub(crate) fn controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search platform, catalogue or provider")
                    .desired_width(260.0),
            );
            egui::ComboBox::from_id_salt("catalogue-group")
                .selected_text(self.group.label())
                .show_ui(ui, |ui| {
                    for group in [
                        Group::Compatible,
                        Group::NeedsChecking,
                        Group::Unassigned,
                        Group::Incompatible,
                        Group::Disabled,
                        Group::All,
                    ] {
                        let count = self
                            .rows
                            .iter()
                            .filter(|row| group == Group::All || row.group == group)
                            .count();
                        ui.selectable_value(
                            &mut self.group,
                            group,
                            format!("{} ({count})", group.label()),
                        );
                    }
                });
            egui::ComboBox::from_id_salt("catalogue-letter")
                .selected_text(
                    self.letter
                        .map_or("A–Z: All".into(), |c| format!("A–Z: {c}")),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.letter, None, "All letters");
                    for letter in ('A'..='Z').chain(std::iter::once('#')) {
                        ui.selectable_value(&mut self.letter, Some(letter), letter.to_string());
                    }
                });
            if ui.button("Clear filters").clicked() {
                self.query.clear();
                self.letter = None;
                self.group = Group::All;
            }
        });
        self.refilter();
        let pages = self.matching.len().div_ceil(PAGE_SIZE).max(1);
        self.page = self.page.min(pages - 1);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.page > 0, egui::Button::new("Previous catalogues"))
                .clicked()
            {
                self.page -= 1;
            }
            ui.label(format!(
                "{} matching · Page {} of {pages} · up to {PAGE_SIZE} rows",
                self.matching.len(),
                self.page + 1
            ));
            if ui
                .add_enabled(self.page + 1 < pages, egui::Button::new("Next catalogues"))
                .clicked()
            {
                self.page += 1;
            }
        });
    }
    pub(crate) fn refilter(&mut self) {
        let key = (search_text(self.query.trim()), self.group, self.letter);
        if self.previous.as_ref() == Some(&key) {
            return;
        }
        self.matching = self
            .rows
            .iter()
            .filter(|row| {
                (self.group == Group::All || row.group == self.group)
                    && key
                        .0
                        .split_whitespace()
                        .all(|word| row.search.contains(word))
                    && self.letter.is_none_or(|letter| {
                        let first = row
                            .name
                            .trim_start()
                            .chars()
                            .next()
                            .unwrap_or('#')
                            .to_ascii_uppercase();
                        if letter == '#' {
                            !first.is_ascii_alphabetic()
                        } else {
                            first == letter
                        }
                    })
            })
            .map(|row| row.source_index)
            .collect();
        self.page = 0;
        self.previous = Some(key);
    }
    pub(crate) fn visible(&self) -> &[usize] {
        let start = (self.page * PAGE_SIZE).min(self.matching.len());
        &self.matching[start..(start + PAGE_SIZE).min(self.matching.len())]
    }
}
fn search_text(text: &str) -> String {
    text.to_lowercase().replace(['-', '_'], "")
}

fn rank(group: Group) -> u8 {
    match group {
        Group::Compatible => 0,
        Group::NeedsChecking => 1,
        Group::Unassigned => 2,
        Group::Incompatible => 3,
        Group::Disabled => 4,
        Group::All => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_list_pages_searches_provider_platform_and_alphabet_without_reordering_sources() {
        let mut state = State::default();
        state.replace(
            (0..5000)
                .map(|i| Row {
                    source_index: i,
                    name: format!("Zeta {i:05}"),
                    search: format!("zeta {i:05} amiga tosec"),
                    group: Group::Compatible,
                })
                .collect(),
        );
        state.refilter();
        assert_eq!(state.visible().len(), PAGE_SIZE);
        state.page = 124;
        assert_eq!(state.visible(), &(4960..5000).collect::<Vec<_>>());
        for query in ["Amiga", "TOSEC", "Zeta 04999"] {
            state.query = query.into();
            state.refilter();
            assert!(state.visible().contains(&4999) || state.matching.len() == 5000);
            assert_eq!(state.page, 0);
        }
        state.letter = Some('A');
        state.refilter();
        assert!(state.visible().is_empty());
        state.letter = Some('Z');
        state.refilter();
        assert_eq!(state.visible(), &[4999]);
    }
    #[test]
    fn groups_separate_disabled_unknown_incompatible_and_stale_without_implicit_selection() {
        let groups = [
            Group::Disabled,
            Group::Unassigned,
            Group::NeedsChecking,
            Group::Incompatible,
            Group::Compatible,
        ];
        let mut state = State::default();
        state.replace(
            groups
                .iter()
                .enumerate()
                .map(|(i, &group)| Row {
                    source_index: i,
                    name: "Same label".into(),
                    search: "same label".into(),
                    group,
                })
                .collect(),
        );
        for (i, &group) in groups.iter().enumerate() {
            state.group = group;
            state.refilter();
            assert_eq!(state.visible(), &[i]);
        }
    }
}

#[cfg(test)]
mod provider_search_tests {
    use super::*;
    #[test]
    fn provider_and_platform_terms_match_together_with_common_provider_spelling() {
        let mut state = State::default();
        state.replace(vec![Row {
            source_index: 7,
            name: "Amiga catalogue".into(),
            search: "Amiga catalogue No-Intro pack".into(),
            group: Group::Compatible,
        }]);
        state.query = "Amiga NoIntro".into();
        state.refilter();
        assert_eq!(state.visible(), &[7]);
        state.query = "Amiga TOSEC".into();
        state.refilter();
        assert!(state.visible().is_empty());
    }
}
