//! Session-only configuration and connection testing for ScreenScraper.
//!
//! ScreenScraper is intentionally presented under Settings as optional
//! metadata enrichment. This page never writes provider credentials, starts a
//! library job, changes identity, or downloads media.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::identity_source::screenscraper::{
    IdentityContribution, LookupKind, LookupOutcome, ScreenScraperClient, ScreenScraperConfig,
    ScreenScraperCredentials, ScreenScraperError, ScreenScraperLookup, ScreenScraperSecret,
    UreqTransport, now_unix,
};

use eframe::egui;

use crate::ui::components as widgets;

const TEST_TITLE: &str = "EmuWiz";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenScraperUiStatus {
    NotConfigured,
    Testing,
    Ready,
    AuthenticationError,
    QuotaExhausted,
    TemporarilyUnavailable,
    Offline,
}

impl ScreenScraperUiStatus {
    fn label(self) -> &'static str {
        match self {
            Self::NotConfigured => "Not configured",
            Self::Testing => "Testing connection…",
            Self::Ready => "Ready",
            Self::AuthenticationError => "Authentication error",
            Self::QuotaExhausted => "Quota exhausted",
            Self::TemporarilyUnavailable => "Temporarily unavailable",
            Self::Offline => "Offline",
        }
    }

    fn tone(self) -> widgets::StatusTone {
        match self {
            Self::NotConfigured => widgets::StatusTone::Info,
            Self::Testing => widgets::StatusTone::Active,
            Self::Ready => widgets::StatusTone::Success,
            Self::AuthenticationError | Self::QuotaExhausted => widgets::StatusTone::Warning,
            Self::TemporarilyUnavailable | Self::Offline => widgets::StatusTone::Blocked,
        }
    }
}

/// State is deliberately session-only until EmuWiz has a safe secret-store
/// abstraction. The custom Debug implementation keeps entered secrets out of
/// test diagnostics and future debug output.
pub(crate) struct ScreenScraperPageState {
    pub(crate) developer_id: String,
    pub(crate) developer_password: String,
    pub(crate) user_id: String,
    pub(crate) user_password: String,
    status: ScreenScraperUiStatus,
    quota: Option<QuotaView>,
    last_success_unix_seconds: Option<u64>,
    last_error: Option<String>,
    candidate_count: Option<usize>,
    request: Option<Receiver<Result<LookupOutcome, ScreenScraperError>>>,
}

impl std::fmt::Debug for ScreenScraperPageState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenScraperPageState")
            .field("developer_id", &self.developer_id)
            .field("developer_password", &"redacted")
            .field("user_id", &self.user_id)
            .field("user_password", &"redacted")
            .field("status", &self.status)
            .field("quota", &self.quota)
            .field("last_success_unix_seconds", &self.last_success_unix_seconds)
            .field("last_error", &self.last_error)
            .field("candidate_count", &self.candidate_count)
            .field("request_pending", &self.request.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QuotaView {
    requests_today: Option<u64>,
    max_requests_per_day: Option<u64>,
    max_requests_per_minute: Option<u64>,
    reset_after_seconds: Option<u64>,
}

impl Default for ScreenScraperPageState {
    fn default() -> Self {
        Self {
            developer_id: String::new(),
            developer_password: String::new(),
            user_id: String::new(),
            user_password: String::new(),
            status: ScreenScraperUiStatus::NotConfigured,
            quota: None,
            last_success_unix_seconds: None,
            last_error: None,
            candidate_count: None,
            request: None,
        }
    }
}

impl ScreenScraperPageState {
    pub(crate) fn poll(&mut self) {
        let Some(request) = self.request.take() else {
            return;
        };
        match request.try_recv() {
            Ok(result) => self.finish_request(result),
            Err(TryRecvError::Empty) => self.request = Some(request),
            Err(TryRecvError::Disconnected) => {
                self.status = ScreenScraperUiStatus::TemporarilyUnavailable;
                self.last_error = Some("The connection test ended unexpectedly.".into());
            }
        }
    }

    fn finish_request(&mut self, result: Result<LookupOutcome, ScreenScraperError>) {
        match result {
            Ok(LookupOutcome::Candidates { candidates, quota }) => {
                self.status = ScreenScraperUiStatus::Ready;
                self.candidate_count = Some(candidates.len());
                self.quota = quota.map(QuotaView::from);
                self.last_success_unix_seconds = Some(now_unix());
                self.last_error = None;
            }
            Ok(LookupOutcome::NoResult { quota }) => {
                self.status = ScreenScraperUiStatus::Ready;
                self.candidate_count = Some(0);
                self.quota = quota.map(QuotaView::from);
                self.last_success_unix_seconds = Some(now_unix());
                self.last_error = None;
            }
            Err(error) => {
                self.status = status_for_error(&error);
                self.last_error = Some(public_error(&error));
            }
        }
    }

    fn begin_test_connection(&mut self) {
        self.poll();
        let developer_id = self.developer_id.trim().to_string();
        let developer_password = match ScreenScraperSecret::parse(&self.developer_password) {
            Ok(secret) => secret,
            Err(_) => {
                self.status = ScreenScraperUiStatus::NotConfigured;
                self.last_error = Some("Enter the developer credentials before testing.".into());
                return;
            }
        };
        let user_id = (!self.user_id.trim().is_empty()).then(|| self.user_id.trim().to_string());
        let user_password = if self.user_password.trim().is_empty() {
            None
        } else {
            match ScreenScraperSecret::parse(&self.user_password) {
                Ok(secret) => Some(secret),
                Err(_) => {
                    self.status = ScreenScraperUiStatus::NotConfigured;
                    self.last_error = Some("The optional user password is invalid.".into());
                    return;
                }
            }
        };
        let credentials = match ScreenScraperCredentials::new(
            &developer_id,
            developer_password,
            user_id.as_deref(),
            user_password,
        ) {
            Ok(credentials) => credentials,
            Err(_) => {
                self.status = ScreenScraperUiStatus::NotConfigured;
                self.last_error =
                    Some("Enter valid ScreenScraper credentials before testing.".into());
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.status = ScreenScraperUiStatus::Testing;
        self.last_error = None;
        self.candidate_count = None;
        self.request = Some(receiver);
        thread::spawn(move || {
            let config = ScreenScraperConfig {
                enabled: true,
                ..ScreenScraperConfig::default()
            };
            let client =
                ScreenScraperClient::new(config, Some(credentials), UreqTransport::default());
            let _ = sender.send(client.lookup(
                LookupKind::Search,
                &ScreenScraperLookup {
                    title: Some(TEST_TITLE.into()),
                    ..ScreenScraperLookup::default()
                },
            ));
        });
    }
}

impl From<archivefs_core::identity_source::screenscraper::QuotaSnapshot> for QuotaView {
    fn from(quota: archivefs_core::identity_source::screenscraper::QuotaSnapshot) -> Self {
        Self {
            requests_today: quota.requests_today,
            max_requests_per_day: quota.max_requests_per_day,
            max_requests_per_minute: quota.max_requests_per_minute,
            reset_after_seconds: None,
        }
    }
}

fn status_for_error(error: &ScreenScraperError) -> ScreenScraperUiStatus {
    match error {
        ScreenScraperError::Authentication { .. } => ScreenScraperUiStatus::AuthenticationError,
        ScreenScraperError::QuotaExhausted { .. } => ScreenScraperUiStatus::QuotaExhausted,
        ScreenScraperError::Network { .. } => ScreenScraperUiStatus::Offline,
        ScreenScraperError::Temporary { .. }
        | ScreenScraperError::MalformedResponse { .. }
        | ScreenScraperError::ResponseTooLarge { .. } => {
            ScreenScraperUiStatus::TemporarilyUnavailable
        }
        ScreenScraperError::Disabled
        | ScreenScraperError::MissingCredentials
        | ScreenScraperError::InvalidRequest { .. }
        | ScreenScraperError::NoResult => ScreenScraperUiStatus::NotConfigured,
    }
}

fn public_error(error: &ScreenScraperError) -> String {
    match error {
        ScreenScraperError::Authentication { .. } => {
            "ScreenScraper rejected the credentials. Check the developer and optional user details."
                .into()
        }
        ScreenScraperError::QuotaExhausted { retry_after, .. } => retry_after
            .map(|duration| {
                format!(
                    "ScreenScraper quota is exhausted; try again in {} seconds.",
                    duration.as_secs()
                )
            })
            .unwrap_or_else(|| "ScreenScraper quota is exhausted; try again later.".into()),
        ScreenScraperError::Network { .. } => {
            "ScreenScraper could not be reached. EmuWiz remains usable offline.".into()
        }
        ScreenScraperError::Temporary { .. }
        | ScreenScraperError::MalformedResponse { .. }
        | ScreenScraperError::ResponseTooLarge { .. } => {
            "ScreenScraper returned a temporary or invalid response. Try again later.".into()
        }
        ScreenScraperError::InvalidRequest { .. }
        | ScreenScraperError::Disabled
        | ScreenScraperError::MissingCredentials
        | ScreenScraperError::NoResult => {
            "ScreenScraper is not configured for a connection test.".into()
        }
    }
}

pub(crate) fn show_screen_scraper_settings(
    ui: &mut egui::Ui,
    state: &mut ScreenScraperPageState,
    busy: bool,
) {
    state.poll();
    widgets::section_header(
        ui,
        "5. Optional metadata provider",
        Some("Add descriptive metadata without changing EmuWiz's authoritative identity."),
    );
    widgets::card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.heading("ScreenScraper");
            widgets::status_badge(ui, state.status.label(), state.status.tone());
        });
        ui.label(
            "Optional online metadata enrichment for titles, descriptions, release information, and media references. EmuWiz identity remains independent.",
        );
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Identity").strong());
        ui.label("EmuWiz's local and project evidence remains authoritative. ScreenScraper can only add metadata candidates.");
        ui.label(egui::RichText::new("Metadata it may add").strong());
        ui.label("Title, alternate title, description, release date, developer, publisher, genre, players, region/language, provider IDs, and media references where available.");
        ui.label("A metadata match is never shown as Verified or Exact.");
        ui.label(
            "Media references are kept as links only; EmuWiz does not automatically cache artwork.",
        );

        ui.add_space(6.0);
        ui.label(egui::RichText::new("Session-only credentials").strong());
        ui.label("Credentials are held only while this window is open. EmuWiz does not yet have a safe provider secret store, so they are not saved to disk.");
        ui.horizontal(|ui| {
            ui.label("Developer ID");
            ui.add(egui::TextEdit::singleline(&mut state.developer_id).desired_width(220.0));
        });
        ui.horizontal(|ui| {
            ui.label("Developer password");
            ui.add(
                egui::TextEdit::singleline(&mut state.developer_password)
                    .password(true)
                    .desired_width(220.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("User ID (optional)");
            ui.add(egui::TextEdit::singleline(&mut state.user_id).desired_width(220.0));
        });
        ui.horizontal(|ui| {
            ui.label("User password (optional)");
            ui.add(
                egui::TextEdit::singleline(&mut state.user_password)
                    .password(true)
                    .desired_width(220.0),
            );
        });
        if widgets::action_button(
            ui,
            "Test connection",
            widgets::ActionStyle::Primary,
            !busy && state.request.is_none(),
        )
        .clicked()
        {
            state.begin_test_connection();
        }
        if let Some(error) = &state.last_error {
            widgets::banner(ui, "Provider status", error, widgets::StatusTone::Warning);
        }
        if let Some(quota) = &state.quota {
            ui.label(egui::RichText::new("Quota").strong());
            let today = quota
                .requests_today
                .map_or_else(|| "unknown".into(), |value| value.to_string());
            let daily = quota
                .max_requests_per_day
                .map_or_else(|| "unknown".into(), |value| value.to_string());
            let minute = quota
                .max_requests_per_minute
                .map_or_else(|| "unknown".into(), |value| value.to_string());
            ui.label(format!(
                "Requests today: {today} / {daily}; per-minute limit: {minute}"
            ));
            if let Some(reset) = quota.reset_after_seconds {
                ui.label(format!("Quota reset information: {reset} seconds"));
            }
        }
        if let Some(last_success) = state.last_success_unix_seconds {
            ui.label(format!(
                "Last successful request: session timestamp {last_success}"
            ));
        }
        widgets::technical_details(ui, "screenscraper_settings_advanced", |ui| {
            ui.label("Provider endpoint: https://api.screenscraper.fr/api2");
            ui.label(
                "Connection test: one harmless title search; no library scan or identity update",
            );
            ui.label(format!(
                "Identity contribution: {:?}",
                IdentityContribution::None
            ));
            if let Some(count) = state.candidate_count {
                ui.label(format!("Metadata candidates returned: {count}"));
            }
            ui.label(
                "Media URLs are references only. EmuWiz does not cache or redistribute artwork.",
            );
            ui.label(
                "Artwork rights may vary by source; automatic artwork downloads are not enabled.",
            );
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
                egui::Shape::Vec(nested) => {
                    nested.iter().any(|shape| shape_contains(shape, needle))
                }
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| shape_contains(&clipped.shape, needle))
    }

    fn render(state: &mut ScreenScraperPageState) -> egui::FullOutput {
        let context = egui::Context::default();
        context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                show_screen_scraper_settings(ui, state, false);
            });
        })
    }

    #[test]
    fn card_is_optional_metadata_and_not_identity() {
        let mut state = ScreenScraperPageState::default();
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "ScreenScraper"));
        assert!(rendered_text_contains(&output, "Optional metadata"));
        assert!(rendered_text_contains(&output, "Not configured"));
        assert!(rendered_text_contains(
            &output,
            "identity remains independent"
        ));
        assert!(!rendered_text_contains(&output, "Verified game"));
        assert!(!rendered_text_contains(&output, "Exact match"));
    }

    #[test]
    fn credentials_are_obscured_and_advanced_details_are_collapsed() {
        let mut state = ScreenScraperPageState {
            developer_password: "secret-value".into(),
            user_password: "user-secret".into(),
            ..ScreenScraperPageState::default()
        };
        let output = render(&mut state);
        assert!(!rendered_text_contains(&output, "secret-value"));
        assert!(!rendered_text_contains(&output, "user-secret"));
        assert!(rendered_text_contains(&output, "Technical details"));
        assert!(!rendered_text_contains(
            &output,
            "api.screenscraper.fr/api2"
        ));
        assert!(rendered_text_contains(
            &output,
            "does not automatically cache artwork"
        ));
    }

    #[test]
    fn status_labels_are_plain_language() {
        assert_eq!(
            ScreenScraperUiStatus::AuthenticationError.label(),
            "Authentication error"
        );
        assert_eq!(
            ScreenScraperUiStatus::QuotaExhausted.label(),
            "Quota exhausted"
        );
        assert_eq!(ScreenScraperUiStatus::Offline.label(), "Offline");
    }

    #[test]
    fn provider_states_and_quota_are_rendered_without_backend_names() {
        for (status, label) in [
            (ScreenScraperUiStatus::Ready, "Ready"),
            (
                ScreenScraperUiStatus::AuthenticationError,
                "Authentication error",
            ),
            (ScreenScraperUiStatus::QuotaExhausted, "Quota exhausted"),
            (ScreenScraperUiStatus::Offline, "Offline"),
        ] {
            let mut state = ScreenScraperPageState {
                status,
                quota: Some(QuotaView {
                    requests_today: Some(3),
                    max_requests_per_day: Some(50),
                    max_requests_per_minute: Some(5),
                    reset_after_seconds: Some(60),
                }),
                ..ScreenScraperPageState::default()
            };
            let output = render(&mut state);
            assert!(rendered_text_contains(&output, label));
            assert!(rendered_text_contains(&output, "Requests today: 3 / 50"));
            assert!(!rendered_text_contains(&output, "QuotaExhausted"));
        }
    }

    #[test]
    fn debug_output_redacts_session_credentials() {
        let state = ScreenScraperPageState {
            developer_password: "developer-secret".into(),
            user_password: "user-secret".into(),
            ..ScreenScraperPageState::default()
        };
        let debug = format!("{state:?}");
        assert!(!debug.contains("developer-secret"));
        assert!(!debug.contains("user-secret"));
    }

    #[test]
    fn connection_test_is_explicit_and_does_not_render_bulk_enrichment() {
        let mut state = ScreenScraperPageState::default();
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Test connection"));
        assert!(!rendered_text_contains(&output, "enrich the whole library"));
        assert_eq!(IdentityContribution::None, IdentityContribution::None);
        assert_eq!(TEST_TITLE, "EmuWiz");
    }
}
