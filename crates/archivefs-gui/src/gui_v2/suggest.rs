//! Title suggestions for the controller keyboard (Controller Everywhere).
//!
//! Built once per library on a worker thread, then queried per keystroke with
//! a bounded amount of work: a binary search for whole-title prefixes and a
//! capped scan of one first-letter bucket for word-prefix matches. The GUI
//! thread never walks the whole library to answer a keystroke.

use std::collections::{HashMap, HashSet};

/// Most suggestions offered.
pub(super) const LIMIT: usize = 5;
/// Most candidates one query may look at.
const MAX_SCAN: usize = 20_000;

#[derive(Default)]
pub(super) struct TitleIndex {
    /// Distinct titles as shown.
    titles: Vec<String>,
    /// (lowercase title, index into `titles`), sorted by the lowercase title.
    sorted: Vec<(String, u32)>,
    /// First character of any word -> titles with such a word.
    buckets: HashMap<char, Vec<u32>>,
}

fn words(lower: &str) -> impl Iterator<Item = &str> {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

impl TitleIndex {
    pub(super) fn build<'a>(titles: impl IntoIterator<Item = &'a str>) -> Self {
        let mut seen = HashSet::new();
        let mut index = Self::default();
        for title in titles {
            let trimmed = title.trim();
            if trimmed.is_empty() || !seen.insert(trimmed.to_lowercase()) {
                continue;
            }
            index.titles.push(trimmed.to_owned());
        }
        for (i, title) in index.titles.iter().enumerate() {
            let lower = title.to_lowercase();
            let mut firsts = HashSet::new();
            for word in words(&lower) {
                if let Some(first) = word.chars().next() {
                    firsts.insert(first);
                }
            }
            for first in firsts {
                index.buckets.entry(first).or_default().push(i as u32);
            }
            index.sorted.push((lower, i as u32));
        }
        index.sorted.sort();
        index
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.titles.len()
    }

    /// Up to `LIMIT` titles: whole-title prefixes first, then titles in which
    /// every typed word begins some word.
    pub(super) fn suggest(&self, query: &str) -> Vec<String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<u32> = Vec::new();
        let start = self
            .sorted
            .partition_point(|(key, _)| key.as_str() < query.as_str());
        for (key, i) in &self.sorted[start..] {
            if !key.starts_with(&query) || out.len() >= LIMIT {
                break;
            }
            out.push(*i);
        }
        if out.len() < LIMIT {
            let tokens: Vec<&str> = words(&query).collect();
            if let Some(first) = tokens.first().and_then(|t| t.chars().next())
                && let Some(bucket) = self.buckets.get(&first)
            {
                for &i in bucket.iter().take(MAX_SCAN) {
                    if out.len() >= LIMIT {
                        break;
                    }
                    if out.contains(&i) {
                        continue;
                    }
                    let lower = self.titles[i as usize].to_lowercase();
                    let title_words: Vec<&str> = words(&lower).collect();
                    if tokens
                        .iter()
                        .all(|t| title_words.iter().any(|w| w.starts_with(t)))
                    {
                        out.push(i);
                    }
                }
            }
        }
        out.into_iter()
            .map(|i| self.titles[i as usize].clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> TitleIndex {
        TitleIndex::build([
            "Super Mario World",
            "Super Mario Kart",
            "Mario Paint",
            "Mario Kart 64",
            "Metroid Prime",
            "The Legend of Zelda",
            "Zelda II",
            "super mario world",
            "  ",
        ])
    }

    #[test]
    fn whole_title_prefixes_come_first_and_duplicates_collapse() {
        let hits = index().suggest("mar");
        assert_eq!(hits[0], "Mario Kart 64");
        assert!(hits.contains(&"Mario Paint".to_owned()));
        assert_eq!(index().len(), 7, "case-duplicate and blank titles dropped");
    }

    #[test]
    fn word_prefixes_find_titles_that_do_not_start_with_the_query() {
        let hits = index().suggest("zel");
        assert!(hits.contains(&"Zelda II".to_owned()));
        assert!(hits.contains(&"The Legend of Zelda".to_owned()), "{hits:?}");
        let both = index().suggest("super kart");
        assert_eq!(both, vec!["Super Mario Kart".to_owned()]);
    }

    #[test]
    fn empty_and_unmatched_queries_suggest_nothing_and_results_are_capped() {
        assert!(index().suggest("").is_empty());
        assert!(index().suggest("   ").is_empty());
        assert!(index().suggest("qqq").is_empty());
        let many = TitleIndex::build(
            (0..500)
                .map(|n| format!("Game {n:03}"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str),
        );
        assert_eq!(many.suggest("game").len(), LIMIT);
    }

    #[test]
    fn a_133k_library_answers_a_keystroke_quickly() {
        let titles: Vec<String> = (0..133_000)
            .map(|n| {
                format!(
                    "{} {} Adventure {n}",
                    ["Super", "Mega", "Hyper", "Ultra"][n % 4],
                    ["Quest", "Racer", "Blaster", "Run", "Fighter"][n % 5]
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let index = TitleIndex::build(titles.iter().map(String::as_str));
        let built = started.elapsed();
        let started = std::time::Instant::now();
        for q in ["s", "su", "sup", "super q", "run adv", "zz"] {
            let _ = index.suggest(q);
        }
        let queried = started.elapsed();
        assert!(
            queried.as_millis() < 400,
            "six keystrokes took {queried:?} (build {built:?})"
        );
    }
}
