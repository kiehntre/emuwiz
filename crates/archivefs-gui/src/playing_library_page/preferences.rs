//! Persist user choices only. A restored preference never restores a preview,
//! verification result, output transaction, or permission to apply.
use super::PlayingLibraryPageState;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SavedPreferences {
    preferred_regions: String,
    preferred_languages: String,
    prefer_newest_revision: bool,
    prefer_parent: bool,
    exclude_beta: bool,
    exclude_proto: bool,
    exclude_demo: bool,
    exclude_sample: bool,
}

impl Default for SavedPreferences {
    fn default() -> Self {
        Self::from_state(&PlayingLibraryPageState::default())
    }
}

impl SavedPreferences {
    pub(crate) fn from_state(state: &PlayingLibraryPageState) -> Self {
        Self {
            preferred_regions: state.preferred_regions_draft.clone(),
            preferred_languages: state.preferred_languages_draft.clone(),
            prefer_newest_revision: state.prefer_newest_revision,
            prefer_parent: state.prefer_parent,
            exclude_beta: state.exclude_beta,
            exclude_proto: state.exclude_proto,
            exclude_demo: state.exclude_demo,
            exclude_sample: state.exclude_sample,
        }
    }

    pub(crate) fn restore(&self, state: &mut PlayingLibraryPageState) {
        state.preferred_regions_draft = self.preferred_regions.clone();
        state.preferred_languages_draft = self.preferred_languages.clone();
        state.prefer_newest_revision = self.prefer_newest_revision;
        state.prefer_parent = self.prefer_parent;
        state.exclude_beta = self.exclude_beta;
        state.exclude_proto = self.exclude_proto;
        state.exclude_demo = self.exclude_demo;
        state.exclude_sample = self.exclude_sample;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_restores_policy_without_restoring_apply_authority() {
        let before = PlayingLibraryPageState {
            preferred_regions_draft: "USA, Europe".into(),
            preferred_languages_draft: "French, English".into(),
            prefer_newest_revision: false,
            prefer_parent: false,
            exclude_beta: false,
            exclude_demo: false,
            pending_apply: Some(7),
            confirm_text: "CREATE 7 LINKS".into(),
            ..Default::default()
        };
        let bytes = serde_json::to_vec(&SavedPreferences::from_state(&before)).unwrap();
        let saved: SavedPreferences = serde_json::from_slice(&bytes).unwrap();
        let mut after = PlayingLibraryPageState::default();
        saved.restore(&mut after);
        assert_eq!(before.build_policy(), after.build_policy());
        assert!(after.plan.is_none());
        assert!(after.preview_transaction.is_none());
        assert!(after.pending_apply.is_none());
        assert!(after.confirm_text.is_empty());
        assert!(after.applied.is_none());
    }

    #[test]
    fn older_settings_keep_existing_defaults() {
        let saved: SavedPreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(saved, SavedPreferences::default());
    }
}
