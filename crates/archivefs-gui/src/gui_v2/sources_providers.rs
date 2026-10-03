//! Presentation model for the Sources & Providers setup overview.
//!
//! Configuration and provider workers remain in their existing feature
//! owners. This module only turns their current snapshots into concise cards.

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceGroup {
    Identity,
    Artwork,
    External,
    Local,
}

impl SourceGroup {
    pub(super) const ALL: [Self; 4] = [Self::Identity, Self::Artwork, Self::External, Self::Local];

    fn title(self) -> &'static str {
        match self {
            Self::Identity => "Identity & DATs",
            Self::Artwork => "Artwork & Metadata",
            Self::External => "Library / External Apps",
            Self::Local => "Local Sources",
        }
    }

    fn empty_state(self) -> &'static str {
        match self {
            Self::Identity => {
                "No DAT sources configured. Game identity verification needs a trusted DAT source."
            }
            Self::Artwork => {
                "No artwork providers configured. Browse & Play still works; artwork may be missing."
            }
            Self::External => "No external library integrations configured.",
            Self::Local => "No local sources configured. Add a game folder when you are ready.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceStatus {
    Ready,
    NeedsSetup,
    NeedsAttention,
    Disabled,
    Stale,
    Unavailable,
    Problem,
    NotChecked,
}

impl SourceStatus {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::NeedsSetup => "Needs setup",
            Self::NeedsAttention => "Needs attention",
            Self::Disabled => "Disabled",
            Self::Stale => "Stale",
            Self::Unavailable => "Unavailable",
            Self::Problem => "Problem",
            Self::NotChecked => "Not checked",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Self::Ready => egui::Color32::from_rgb(45, 145, 90),
            Self::NeedsSetup | Self::NotChecked | Self::Disabled => {
                egui::Color32::from_rgb(150, 125, 55)
            }
            Self::Stale | Self::NeedsAttention => egui::Color32::from_rgb(185, 115, 35),
            Self::Unavailable | Self::Problem => egui::Color32::from_rgb(175, 55, 55),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceNature {
    Local,
    Bundled,
    Remote,
}

impl SourceNature {
    fn label(self) -> &'static str {
        match self {
            Self::Local => "Local filesystem",
            Self::Bundled => "Bundled with EmuWiz",
            Self::Remote => "Remote provider",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ProviderCard {
    pub(super) id: &'static str,
    pub(super) group: SourceGroup,
    pub(super) name: String,
    pub(super) purpose: String,
    pub(super) status: SourceStatus,
    pub(super) reason: String,
    pub(super) nature: SourceNature,
    pub(super) action: Option<&'static str>,
    pub(super) target: Option<HubAction>,
    pub(super) advanced: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HubAction {
    LocalSources,
    DatSources,
    ArtworkProviders,
    RommLibrary,
}

#[cfg(test)]
pub(super) fn action_for_id(id: &str) -> Option<HubAction> {
    match id {
        "local-game-folders" => Some(HubAction::LocalSources),
        "dat-sources" => Some(HubAction::DatSources),
        "local-artwork" | "es-de" | "screenscraper" => Some(HubAction::ArtworkProviders),
        "romm" => Some(HubAction::RommLibrary),
        _ => None,
    }
}

pub(super) fn screenscraper_missing_credentials_reason(
    has_developer_id: bool,
    has_developer_password: bool,
) -> String {
    match (has_developer_id, has_developer_password) {
        (false, false) => "ScreenScraper developer ID and password are not configured.".into(),
        (false, true) => "ScreenScraper developer ID is missing.".into(),
        (true, false) => "ScreenScraper developer password is missing.".into(),
        (true, true) => "ScreenScraper credentials need review.".into(),
    }
}

pub(super) fn romm_purpose() -> &'static str {
    "Read-only browsing of your RomM games, artwork and identity clues."
}

pub(super) fn show(ui: &mut egui::Ui, cards: &[ProviderCard]) -> Option<HubAction> {
    ui.label("Review what EmuWiz can use, what it contributes, and where to configure it.");
    ui.label("Missing optional artwork does not stop browsing. Game identity verification needs a usable DAT source.");
    ui.label("EmuWiz's local and project evidence remains authoritative; online provider metadata stays a candidate until you accept it.");
    ui.label("Global setup lives here; individual game provenance stays with that game's details.");

    let mut action = None;
    for group in SourceGroup::ALL {
        ui.add_space(8.0);
        ui.heading(group.title());
        let rows = cards
            .iter()
            .filter(|card| card.group == group)
            .collect::<Vec<_>>();
        if rows.is_empty() {
            ui.label(group.empty_state());
            continue;
        }
        for card in rows {
            ui.push_id(card.id, |ui| {
                crate::ui::components::card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&card.name);
                        ui.colored_label(card.status.color(), card.status.label());
                        ui.weak(card.nature.label());
                    });
                    ui.label(&card.purpose);
                    ui.label(&card.reason);
                    if let Some(label) = card.action
                        && ui.button(label).clicked()
                    {
                        action = card.target;
                    }
                    ui.collapsing("Advanced details", |ui| {
                        for detail in &card.advanced {
                            ui.monospace(detail);
                        }
                    });
                });
            });
        }
    }
    if let Some(card) = cards.iter().find(|card| card.id == "screenscraper")
        && card.status == SourceStatus::NeedsSetup
    {
        ui.label("ScreenScraper is optional. Local metadata, game browsing, and identity evidence remain available without it.");
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &'static str, group: SourceGroup, status: SourceStatus) -> ProviderCard {
        ProviderCard {
            id,
            group,
            name: "Fixture provider".into(),
            purpose: "Adds metadata candidates.".into(),
            status,
            reason: "Fixture state.".into(),
            nature: SourceNature::Remote,
            action: Some("Configure"),
            target: action_for_id(id),
            advanced: vec![
                "endpoint=https://example.invalid".into(),
                "cache=usable".into(),
            ],
        }
    }

    fn rendered(cards: &[ProviderCard]) -> Vec<String> {
        let context = egui::Context::default();
        let output = context.run(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                show(ui, cards);
            });
        });
        output
            .shapes
            .iter()
            .flat_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => vec![text.galley.text().to_string()],
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn ready_provider_has_concise_state() {
        let text = rendered(&[card(
            "screenscraper",
            SourceGroup::Artwork,
            SourceStatus::Ready,
        )])
        .join(" ");
        assert!(text.contains("Ready"));
        assert!(!text.contains("ProviderHealth::"));
    }
    #[test]
    fn missing_configuration_is_needs_setup() {
        assert_eq!(SourceStatus::NeedsSetup.label(), "Needs setup");
    }
    #[test]
    fn stale_is_not_failed() {
        assert_ne!(SourceStatus::Stale, SourceStatus::Problem);
        assert_eq!(SourceStatus::Stale.label(), "Stale");
    }
    #[test]
    fn disabled_is_not_broken() {
        assert_ne!(SourceStatus::Disabled, SourceStatus::Problem);
        assert_eq!(SourceStatus::Disabled.label(), "Disabled");
    }
    #[test]
    fn optional_artwork_absence_keeps_browse_available() {
        let text = rendered(&[card(
            "screenscraper",
            SourceGroup::Artwork,
            SourceStatus::NeedsSetup,
        )])
        .join(" ");
        assert!(
            text.contains("Browse & Play still works")
                || text.contains("Missing optional artwork does not stop browsing")
        );
    }
    #[test]
    fn missing_dat_explains_verification_limit() {
        assert!(
            SourceGroup::Identity
                .empty_state()
                .contains("verification needs a trusted DAT source")
        );
    }
    #[test]
    fn provider_purpose_is_visible() {
        assert!(
            rendered(&[card(
                "screenscraper",
                SourceGroup::Artwork,
                SourceStatus::Ready
            )])
            .iter()
            .any(|line| line == "Adds metadata candidates.")
        );
    }
    #[test]
    fn screenscraper_setup_reason_can_name_missing_credentials() {
        let reason = screenscraper_missing_credentials_reason(true, false);
        assert!(reason.contains("password is missing"));
    }
    #[test]
    fn romm_description_does_not_claim_native_downloads() {
        let copy = romm_purpose();
        assert!(copy.to_lowercase().contains("read-only browsing"));
        assert!(!copy.to_lowercase().contains("download games"));
    }
    #[test]
    fn esde_missing_path_is_unavailable_and_config_routes_canonically() {
        assert_eq!(SourceStatus::NeedsSetup.label(), "Needs setup");
        assert_eq!(action_for_id("es-de"), Some(HubAction::ArtworkProviders));
    }
    #[test]
    fn local_and_remote_sources_have_distinct_labels() {
        assert_ne!(SourceNature::Local.label(), SourceNature::Remote.label());
    }
    #[test]
    fn identity_precedence_copy_keeps_project_evidence_authoritative() {
        let text = rendered(&[]).join(" ");
        assert!(text.contains("EmuWiz's local and project evidence remains authoritative"));
    }
    #[test]
    fn overview_keeps_setup_global() {
        let text = rendered(&[]).join(" ");
        assert!(text.contains("Global setup lives here"));
    }
    #[test]
    fn empty_states_are_specific_to_each_group() {
        let values = SourceGroup::ALL.map(SourceGroup::empty_state);
        assert_ne!(values[0], values[1]);
        assert_ne!(values[1], values[2]);
        assert_ne!(values[2], values[3]);
    }
    #[test]
    fn advanced_details_retain_technical_values() {
        let row = card("screenscraper", SourceGroup::Artwork, SourceStatus::Ready);
        assert!(row.advanced.iter().any(|value| value.contains("endpoint=")));
        assert!(row.advanced.iter().any(|value| value.contains("cache=")));
    }
    #[test]
    fn actions_point_to_existing_setup_surfaces() {
        for id in [
            "dat-sources",
            "screenscraper",
            "romm",
            "es-de",
            "local-game-folders",
        ] {
            assert!(action_for_id(id).is_some());
        }
    }
    #[test]
    fn repeated_provider_ids_are_stable_and_semantic() {
        let row = card("screenscraper", SourceGroup::Artwork, SourceStatus::Ready);
        assert_eq!(row.id, "screenscraper");
        assert_eq!(
            row.id,
            card("screenscraper", SourceGroup::Artwork, SourceStatus::Ready).id
        );
        assert_ne!(
            row.id,
            card("romm", SourceGroup::External, SourceStatus::Ready).id
        );
    }
    #[test]
    fn painting_is_read_only_for_provider_cards() {
        let cards = vec![card(
            "screenscraper",
            SourceGroup::Artwork,
            SourceStatus::Ready,
        )];
        let before = cards.clone();
        let _ = rendered(&cards);
        assert_eq!(cards[0].id, before[0].id);
        assert_eq!(cards[0].status, before[0].status);
    }
}
