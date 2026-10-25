//! Catalogue audit: what the catalogue contains and whether every script can be
//! reached by the selector rules.
//!
//! A development and test tool, not a runtime feature. Print the report with
//! `cargo test -p archivefs-gui --lib print_catalogue_report -- --ignored --nocapture`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::model::{GuidanceCategory, GuidanceFact, GuidanceLevel, GuidancePage, GuidanceTopic};
use super::script::{GuidanceScript, ScriptOrigin};
use super::select::select_in;

#[derive(Debug)]
pub(super) struct CatalogueReport {
    pub(super) total: usize,
    pub(super) design_numbered: usize,
    pub(super) design_variants: usize,
    pub(super) legacy: usize,
    pub(super) ids: Vec<&'static str>,
    pub(super) duplicate_ids: Vec<&'static str>,
    pub(super) design_numbers_missing: Vec<u8>,
    pub(super) design_numbers_duplicated: Vec<u8>,
    pub(super) by_category: BTreeMap<String, usize>,
    pub(super) by_level: BTreeMap<String, usize>,
    pub(super) topics_without_scripts: Vec<GuidanceTopic>,
    /// Scripts that do not win against the rest of the catalogue even in the
    /// most favourable situation (exactly the facts they require).
    pub(super) unreachable: Vec<(&'static str, &'static str)>,
    /// Scripts whose selector (pages, required and excluded facts) is identical.
    pub(super) duplicate_selectors: Vec<(&'static str, &'static str)>,
}

/// The facts that make `script` eligible and nothing else.
fn witness(script: &GuidanceScript) -> Vec<GuidanceFact> {
    script.requires.iter().map(|kind| kind.sample()).collect()
}

fn pages_to_check(script: &GuidanceScript) -> Vec<GuidancePage> {
    if script.pages.is_empty() {
        vec![GuidancePage::Home]
    } else {
        script.pages.to_vec()
    }
}

pub(super) fn report(scripts: &'static [GuidanceScript]) -> CatalogueReport {
    let mut seen = BTreeSet::new();
    let mut duplicate_ids = Vec::new();
    for script in scripts {
        if !seen.insert(script.id) {
            duplicate_ids.push(script.id);
        }
    }
    let numbers: Vec<u8> = scripts.iter().filter_map(|s| s.design_number).collect();
    let design_numbers_missing = (1..=42u8).filter(|n| !numbers.contains(n)).collect();
    let design_numbers_duplicated = (1..=42u8)
        .filter(|n| numbers.iter().filter(|m| *m == n).count() > 1)
        .collect();

    let mut by_category = BTreeMap::new();
    let mut by_level = BTreeMap::new();
    for script in scripts {
        *by_category
            .entry(format!("{:?}", script.category))
            .or_insert(0) += 1;
        *by_level
            .entry(format!("{:?}", GuidanceLevel::Quick))
            .or_insert(0) += 1;
        for (level, present) in [
            (GuidanceLevel::Minimal, script.minimal.is_some()),
            (GuidanceLevel::Explain, script.explain.is_some()),
            (GuidanceLevel::Technical, script.technical.is_some()),
        ] {
            if present {
                *by_level.entry(format!("{level:?}")).or_insert(0) += 1;
            }
        }
    }

    let topics_without_scripts = GuidanceTopic::DESIGN
        .into_iter()
        .chain([GuidanceTopic::Activity])
        .filter(|topic| !scripts.iter().any(|s| s.topics.contains(topic)))
        .collect();

    let mut unreachable = Vec::new();
    for script in scripts {
        let facts = witness(script);
        for page in pages_to_check(script) {
            match select_in(scripts, page, &facts) {
                Some(winner) if winner.script.id == script.id => {}
                Some(winner) => unreachable.push((script.id, winner.script.id)),
                None => unreachable.push((script.id, "(nothing)")),
            }
        }
    }

    let mut duplicate_selectors = Vec::new();
    for (index, a) in scripts.iter().enumerate() {
        for b in &scripts[index + 1..] {
            if a.pages == b.pages && a.requires == b.requires && a.excludes == b.excludes {
                duplicate_selectors.push((a.id, b.id));
            }
        }
    }

    CatalogueReport {
        total: scripts.len(),
        design_numbered: scripts.iter().filter(|s| s.design_number.is_some()).count(),
        design_variants: scripts.iter().filter(|s| s.variant_of.is_some()).count(),
        legacy: scripts
            .iter()
            .filter(|s| s.origin == ScriptOrigin::Legacy)
            .count(),
        ids: scripts.iter().map(|s| s.id).collect(),
        duplicate_ids,
        design_numbers_missing,
        design_numbers_duplicated,
        by_category,
        by_level,
        topics_without_scripts,
        unreachable,
        duplicate_selectors,
    }
}

impl fmt::Display for CatalogueReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Mr Wiz catalogue report")?;
        writeln!(
            f,
            "  scripts: {} ({} numbered design + {} documented variants + {} legacy)",
            self.total, self.design_numbered, self.design_variants, self.legacy
        )?;
        writeln!(f, "  by category: {:?}", self.by_category)?;
        writeln!(f, "  scripts authoring each level: {:?}", self.by_level)?;
        writeln!(f, "  duplicate IDs: {:?}", self.duplicate_ids)?;
        writeln!(
            f,
            "  design numbers missing: {:?}",
            self.design_numbers_missing
        )?;
        writeln!(
            f,
            "  topics without a script: {:?}",
            self.topics_without_scripts
        )?;
        writeln!(
            f,
            "  unreachable (script, shadowed by): {:?}",
            self.unreachable
        )?;
        writeln!(f, "  duplicate selectors: {:?}", self.duplicate_selectors)?;
        writeln!(f, "  ids:")?;
        for id in &self.ids {
            writeln!(f, "    {id}")?;
        }
        Ok(())
    }
}

/// Every category value, so a test can say none is silently unused.
pub(super) fn all_categories() -> [GuidanceCategory; 6] {
    [
        GuidanceCategory::Tip,
        GuidanceCategory::Explain,
        GuidanceCategory::WhyBlocked,
        GuidanceCategory::Success,
        GuidanceCategory::Warning,
        GuidanceCategory::EmptyState,
    ]
}
