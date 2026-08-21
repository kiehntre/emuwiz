//! Bridges the persisted DAT source registry (`archivefs_core::dat::sources`)
//! to the No-Intro source [`crate::selected_evidence_page::gather_selected_evidence`]
//! needs for its direct local lookup.
//!
//! # Why this exists separately from `selected_evidence_page`
//!
//! `gather_selected_evidence` already does the real hash lookup once it is
//! handed an `Option<&ImportedNoIntroSource>` - see its own module doc. What
//! it does not do, and what this module supplies, is *finding* that source:
//! resolving the app's persisted [`DatSourceRegistry`] down to "is there
//! exactly one enabled, platform-relevant No-Intro source right now", honestly
//! reporting when there is none or more than one, and never reparsing an
//! unchanged registry to answer that question again.
//!
//! # Not free, so it is cached
//!
//! [`archivefs_core::identity_source::no_intro::select_no_intro_source`]
//! parses and hashes every candidate DAT file it finds. [`NoIntroSourceCache`]
//! never calls it twice for an unchanged registry: it gates every call behind
//! [`archivefs_core::identity_source::no_intro::no_intro_selection_fingerprint`],
//! which is cheap, and only re-resolves when that fingerprint (or the
//! requested platform) actually changed. A source edit, a newly enabled
//! source, or a replaced DAT file on disk all change the fingerprint, which
//! is what makes previously cached evidence become stale rather than keep
//! being served.
//!
//! Nothing in this module touches `egui`; `resolve` performs blocking file
//! I/O. `main.rs`'s `gather_selected_evidence_with_registry` calls it from
//! inside the same background thread `start_selected_evidence_load` already
//! spawns - the same shape other blocking gathers in this crate already use
//! (see `main.rs`'s `gather_doctor_inputs`).

use std::sync::Arc;

use archivefs_core::dat::sources::DatSourceRegistry;
use archivefs_core::identity_source::no_intro::{
    ImportedNoIntroSource, NoIntroSourceLabel, NoIntroSourceSelection,
    no_intro_selection_fingerprint, select_no_intro_source,
};

/// The cached, display-ready shape of [`NoIntroSourceSelection`]. `Selected`
/// carries an `Arc` rather than the owned value so a cache hit is a cheap
/// clone, not a re-parse.
#[derive(Debug, Clone)]
pub(crate) enum NoIntroSourceState {
    /// No enabled, platform-relevant registered source identifies as
    /// No-Intro. Distinct from a load failure - the registry was consulted
    /// and honestly had nothing to offer.
    NotImported,
    Selected(Arc<ImportedNoIntroSource>),
    /// More than one enabled, platform-relevant source identifies as
    /// No-Intro. Never collapsed to a first pick.
    Ambiguous(Vec<NoIntroSourceLabel>),
}

/// Caches the registry's resolved No-Intro source for one platform at a
/// time, avoiding a re-parse on every frame or every file selection.
pub(crate) struct NoIntroSourceCache {
    fingerprint: Option<u64>,
    platform: Option<String>,
    state: NoIntroSourceState,
}

impl NoIntroSourceCache {
    pub(crate) fn new() -> Self {
        Self {
            fingerprint: None,
            platform: None,
            state: NoIntroSourceState::NotImported,
        }
    }

    /// Returns the cached resolution, re-resolving against `registry` only
    /// when the platform being asked about or the registry's fingerprint for
    /// it has changed since the last call.
    pub(crate) fn resolve(
        &mut self,
        registry: &DatSourceRegistry,
        platform_id: Option<&str>,
    ) -> &NoIntroSourceState {
        let fingerprint = no_intro_selection_fingerprint(registry, platform_id);
        let platform_changed = self.platform.as_deref() != platform_id;
        if platform_changed || self.fingerprint != Some(fingerprint) {
            self.state = match select_no_intro_source(registry, platform_id) {
                NoIntroSourceSelection::NotImported => NoIntroSourceState::NotImported,
                NoIntroSourceSelection::Selected(imported) => {
                    NoIntroSourceState::Selected(Arc::new(*imported))
                }
                NoIntroSourceSelection::Ambiguous(labels) => NoIntroSourceState::Ambiguous(labels),
            };
            self.fingerprint = Some(fingerprint);
            self.platform = platform_id.map(str::to_string);
        }
        &self.state
    }
}

impl Default for NoIntroSourceCache {
    fn default() -> Self {
        Self::new()
    }
}

/// A short, user-facing explanation of the registry's No-Intro state for the
/// current platform, for display when there is nothing (or too much) to look
/// a hash up in. `None` when there is exactly one source: the panel's
/// existing "DAT evidence" match/no-match wording already covers that case.
pub(crate) fn no_intro_source_note(state: &NoIntroSourceState) -> Option<String> {
    match state {
        NoIntroSourceState::NotImported => {
            Some("No enabled No-Intro DAT source is registered for this platform.".to_string())
        }
        NoIntroSourceState::Selected(_) => None,
        NoIntroSourceState::Ambiguous(labels) => {
            let names: Vec<String> = labels
                .iter()
                .map(|label| format!("{} ({})", label.display_name, label.source_id))
                .collect();
            Some(format!(
                "{} enabled No-Intro DAT sources match this platform ({}). Disable or reassign \
                 one in DAT Sources before it can be used automatically here.",
                labels.len(),
                names.join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use archivefs_core::dat::sources::{DatSourceEntry, DatSourceKind, DatSourceRegistry};

    use super::*;

    const GB_NO_INTRO_XML: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Nintendo - Game Boy</name>
        <version>20250101-120000</version>
        <author>No-Intro</author>
    </header>
    <game name="Alleyway (World)">
        <rom name="Alleyway (World).gb" size="32768" crc="9F73FA30" sha1="ed8070e011713527bdc03e2b9cec9f9c4a7e3aaa"/>
    </game>
</datafile>"#;

    const GB_NO_INTRO_XML_OTHER: &str = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Nintendo - Game Boy (Rebuild)</name>
        <version>20250601-000000</version>
        <author>No-Intro</author>
    </header>
    <game name="Tetris (World)">
        <rom name="Tetris (World).gb" size="32768" crc="AAAAAAAA" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"/>
    </game>
</datafile>"#;

    fn write_dat(dir: &Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn file_entry(id: &str, path: std::path::PathBuf, platform: Option<&str>) -> DatSourceEntry {
        let mut entry =
            DatSourceEntry::new(id.to_string(), id.to_string(), path, DatSourceKind::File);
        entry.platform = platform.map(str::to_string);
        entry
    }

    #[test]
    fn no_configured_source_yields_not_imported() {
        let registry = DatSourceRegistry::new();
        let mut cache = NoIntroSourceCache::new();

        let state = cache.resolve(&registry, Some("Game Boy"));
        assert!(matches!(state, NoIntroSourceState::NotImported));
        assert_eq!(
            no_intro_source_note(state).as_deref(),
            Some("No enabled No-Intro DAT source is registered for this platform.")
        );
    }

    #[test]
    fn disabled_source_is_ignored() {
        let dir = tempdir().unwrap();
        let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        let mut entry = file_entry("gb-no-intro", path, Some("Game Boy"));
        entry.enabled = false;
        registry.add(entry).unwrap();
        let mut cache = NoIntroSourceCache::new();

        let state = cache.resolve(&registry, Some("Game Boy"));
        assert!(matches!(state, NoIntroSourceState::NotImported));
    }

    #[test]
    fn wrong_platform_source_is_ignored() {
        let dir = tempdir().unwrap();
        let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-no-intro", path, Some("NES")))
            .unwrap();
        let mut cache = NoIntroSourceCache::new();

        let state = cache.resolve(&registry, Some("Game Boy"));
        assert!(matches!(state, NoIntroSourceState::NotImported));
    }

    #[test]
    fn configured_source_reaches_the_real_lookup_path() {
        let dir = tempdir().unwrap();
        let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-no-intro", path, Some("Game Boy")))
            .unwrap();
        let mut cache = NoIntroSourceCache::new();

        let state = cache.resolve(&registry, Some("Game Boy"));
        match state {
            NoIntroSourceState::Selected(imported) => {
                assert_eq!(imported.system_name, "Nintendo - Game Boy");
            }
            other => panic!("expected Selected, got {other:?}"),
        }
        assert_eq!(no_intro_source_note(state), None);
    }

    #[test]
    fn multiple_sources_are_ambiguous_never_a_first_pick() {
        let dir = tempdir().unwrap();
        let path_a = write_dat(dir.path(), "gb-a.dat", GB_NO_INTRO_XML);
        let path_b = write_dat(dir.path(), "gb-b.dat", GB_NO_INTRO_XML_OTHER);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-a", path_a, Some("Game Boy")))
            .unwrap();
        registry
            .add(file_entry("gb-b", path_b, Some("Game Boy")))
            .unwrap();
        let mut cache = NoIntroSourceCache::new();

        let state = cache.resolve(&registry, Some("Game Boy"));
        let labels = match state {
            NoIntroSourceState::Ambiguous(labels) => labels,
            other => panic!("expected Ambiguous, got {other:?}"),
        };
        let ids: Vec<&str> = labels
            .iter()
            .map(|label| label.source_id.as_str())
            .collect();
        assert_eq!(ids, ["gb-a", "gb-b"], "deterministic, sorted ordering");
        let note = no_intro_source_note(state).expect("ambiguity must be explained");
        assert!(note.contains("gb-a"));
        assert!(note.contains("gb-b"));
    }

    #[test]
    fn cache_does_not_reparse_when_the_registry_is_unchanged() {
        let dir = tempdir().unwrap();
        let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-no-intro", path.clone(), Some("Game Boy")))
            .unwrap();
        let mut cache = NoIntroSourceCache::new();

        cache.resolve(&registry, Some("Game Boy"));
        let first_import = match &cache.state {
            NoIntroSourceState::Selected(imported) => Arc::clone(imported),
            _ => panic!("expected Selected"),
        };

        // A second resolve against the exact same registry must reuse the
        // cached import rather than parse the DAT again.
        cache.resolve(&registry, Some("Game Boy"));
        let second_import = match &cache.state {
            NoIntroSourceState::Selected(imported) => Arc::clone(imported),
            _ => panic!("expected Selected"),
        };
        assert!(Arc::ptr_eq(&first_import, &second_import));
    }

    #[test]
    fn selected_platform_change_invalidates_the_cache() {
        let dir = tempdir().unwrap();
        let gb_path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-no-intro", gb_path, Some("Game Boy")))
            .unwrap();
        let mut cache = NoIntroSourceCache::new();

        let gb_state = cache.resolve(&registry, Some("Game Boy"));
        assert!(matches!(gb_state, NoIntroSourceState::Selected(_)));

        // No source is assigned to NES, so switching the selected platform
        // must not keep serving the Game Boy source cached above.
        let nes_state = cache.resolve(&registry, Some("NES"));
        assert!(matches!(nes_state, NoIntroSourceState::NotImported));
    }

    #[test]
    fn source_change_invalidates_stale_selected_evidence() {
        let dir = tempdir().unwrap();
        let path = write_dat(dir.path(), "gb.dat", GB_NO_INTRO_XML);
        let mut registry = DatSourceRegistry::new();
        registry
            .add(file_entry("gb-no-intro", path.clone(), Some("Game Boy")))
            .unwrap();
        let mut cache = NoIntroSourceCache::new();

        let state = cache.resolve(&registry, Some("Game Boy"));
        assert!(matches!(state, NoIntroSourceState::Selected(_)));

        // Disabling the only configured source is a registry change that
        // must invalidate the previously cached state, not keep serving it
        // as if the source were still enabled.
        registry.get_mut("gb-no-intro").unwrap().enabled = false;
        let state_after_disable = cache.resolve(&registry, Some("Game Boy"));
        assert!(matches!(
            state_after_disable,
            NoIntroSourceState::NotImported
        ));

        // Re-enabling and replacing the DAT file's contents on disk is also
        // a change the cache must not paper over.
        registry.get_mut("gb-no-intro").unwrap().enabled = true;
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, GB_NO_INTRO_XML_OTHER).unwrap();
        let state_after_swap = cache.resolve(&registry, Some("Game Boy"));
        match state_after_swap {
            NoIntroSourceState::Selected(imported) => {
                assert_eq!(imported.system_name, "Nintendo - Game Boy (Rebuild)");
            }
            other => panic!("expected a freshly re-resolved source, got {other:?}"),
        }
    }
}
