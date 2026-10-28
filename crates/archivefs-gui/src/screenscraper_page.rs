//! ScreenScraper configuration, optional Secret Service storage, and connection testing.
//!
//! ScreenScraper is intentionally presented under Settings as optional
//! metadata enrichment. It writes credentials only through the system secret
//! store. It never starts a library job, changes identity, or downloads media.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use archivefs_core::identity_source::screenscraper::{
    IdentityContribution, LookupKind, LookupOutcome, ScreenScraperClient, ScreenScraperConfig,
    ScreenScraperCredentials, ScreenScraperError, ScreenScraperLookup, ScreenScraperSecret,
    UreqTransport, now_unix,
};

use eframe::egui;

use crate::screenscraper_secret_store::{SecretStore, StoreError, StoredCredentials};
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
    pub(crate) fn label(self) -> &'static str {
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

/// Secret Service credentials are loaded when available. The custom Debug
/// implementation keeps entered secrets out of diagnostics.
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
    stored: bool,
    invalid_stored: bool,
    store_available: bool,
    storage_message: Option<&'static str>,
    unsaved_edits: bool,
}

impl std::fmt::Debug for ScreenScraperPageState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenScraperPageState")
            .field("developer_id", &"redacted")
            .field("developer_password", &"redacted")
            .field("user_id", &"redacted")
            .field("user_password", &"redacted")
            .field("status", &self.status)
            .field("quota", &self.quota)
            .field("last_success_unix_seconds", &self.last_success_unix_seconds)
            .field("last_error", &self.last_error)
            .field("candidate_count", &self.candidate_count)
            .field("request_pending", &self.request.is_some())
            .field("stored", &self.stored)
            .field("invalid_stored", &self.invalid_stored)
            .field("store_available", &self.store_available)
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
        #[cfg(test)]
        let store = SecretStore::new(std::path::Path::new("/no-such-secret-tool"));
        #[cfg(not(test))]
        let store = SecretStore::system();
        Self::load_from_store(&store)
    }
}

impl ScreenScraperPageState {
    fn load_from_store(store: &SecretStore) -> Self {
        let loaded = store.load();
        let stored = matches!(loaded, Ok(Some(_)));
        let invalid_stored = matches!(loaded, Err(StoreError::InvalidRecord));
        let store_available = store.available();
        let record = loaded.ok().flatten();
        let mut state = Self {
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
            stored,
            invalid_stored,
            store_available,
            storage_message: None,
            unsaved_edits: false,
        };
        if let Some(record) = record
            && state.set_loaded(record).is_err()
        {
            state.stored = false;
            state.invalid_stored = true;
            state.storage_message = Some("Stored credentials are invalid; enter them again.");
        }
        state
    }

    fn set_loaded(&mut self, record: StoredCredentials) -> Result<(), ()> {
        if record.developer_id.len() > 256
            || record.user_id.len() > 256
            || record.user_id.chars().any(|ch| ch.is_control())
        {
            return Err(());
        }
        let developer_password =
            ScreenScraperSecret::parse(&record.developer_password).map_err(|_| ())?;
        let user_password = if record.user_password.is_empty() {
            None
        } else {
            Some(ScreenScraperSecret::parse(&record.user_password).map_err(|_| ())?)
        };
        ScreenScraperCredentials::new(
            &record.developer_id,
            developer_password,
            Some(&record.user_id),
            user_password,
        )
        .map_err(|_| ())?;
        self.developer_id = record.developer_id;
        self.developer_password = record.developer_password;
        self.user_id = record.user_id;
        self.user_password = record.user_password;
        Ok(())
    }

    fn save_credentials(&mut self) {
        if self.credentials().is_none()
            || self.user_id.len() > 256
            || self.user_id.chars().any(|ch| ch.is_control())
        {
            self.storage_message =
                Some("Enter valid developer and optional user credentials before saving.");
            return;
        }
        let record = StoredCredentials {
            developer_id: self.developer_id.trim().into(),
            developer_password: self.developer_password.trim().into(),
            user_id: self.user_id.trim().into(),
            user_password: self.user_password.trim().into(),
        };
        match SecretStore::system().save(&record) {
            Ok(()) => {
                self.stored = true;
                self.invalid_stored = false;
                self.unsaved_edits = false;
                self.storage_message = Some(
                    "Credentials saved in the system secret store. Connection not yet verified.",
                );
            }
            Err(StoreError::TooLarge) => {
                self.storage_message = Some(
                    "Credentials are too long for the system secret store. They remain in this session only.",
                );
            }
            Err(_) => {
                self.storage_message = Some(if self.stored {
                    "Could not update the secret store. Current edits remain in this session; previously saved credentials may still be present."
                } else {
                    "System secret storage is unavailable. Credentials remain in this session only."
                })
            }
        }
    }

    fn forget_credentials(&mut self) {
        self.forget_credentials_with(&SecretStore::system());
    }

    fn forget_credentials_with(&mut self, store: &SecretStore) {
        let removed = store.forget().is_ok();
        self.developer_id.clear();
        self.developer_password.clear();
        self.user_id.clear();
        self.user_password.clear();
        if removed {
            self.stored = false;
            self.invalid_stored = false;
        }
        self.unsaved_edits = false;
        self.status = ScreenScraperUiStatus::NotConfigured;
        self.quota = None;
        self.last_error = None;
        self.storage_message = Some(if removed {
            "Credentials forgotten from the session and system secret store."
        } else {
            "Session credentials cleared. Stored credentials could not be removed or verified; try again when the secret store is available."
        });
    }

    pub(crate) fn status(&self) -> ScreenScraperUiStatus {
        self.status
    }

    pub(crate) fn last_success_unix_seconds(&self) -> Option<u64> {
        self.last_success_unix_seconds
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.credentials().is_some()
    }

    pub(crate) fn credentials(&self) -> Option<ScreenScraperCredentials> {
        let developer_id = self.developer_id.trim();
        let developer_password = ScreenScraperSecret::parse(&self.developer_password).ok()?;
        let user_id = (!self.user_id.trim().is_empty()).then(|| self.user_id.trim());
        let user_password = if self.user_password.trim().is_empty() {
            None
        } else {
            Some(ScreenScraperSecret::parse(&self.user_password).ok()?)
        };
        ScreenScraperCredentials::new(developer_id, developer_password, user_id, user_password).ok()
    }

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
        ui.label(egui::RichText::new("Credentials").strong());
        if state.invalid_stored {
            ui.label("Stored credentials are invalid. Replace or forget them.");
        } else if state.stored {
            if state.unsaved_edits {
                ui.label(
                    "Saved credentials are available after restart. Current edits are not saved.",
                );
            } else {
                ui.label("Saved in the system secret store; available after restart.");
            }
        } else if state.store_available {
            ui.label("Session-only until saved in the system secret store.");
        } else {
            ui.label("System secret storage is unavailable. Credentials are held only while this window is open and are not saved to disk.");
        }
        let mut edited = false;
        ui.horizontal(|ui| {
            ui.label("Developer ID");
            edited |= ui
                .add(egui::TextEdit::singleline(&mut state.developer_id).desired_width(220.0))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Developer password");
            edited |= ui
                .add(
                    egui::TextEdit::singleline(&mut state.developer_password)
                        .password(true)
                        .desired_width(220.0),
                )
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("User ID (optional)");
            edited |= ui
                .add(egui::TextEdit::singleline(&mut state.user_id).desired_width(220.0))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("User password (optional)");
            edited |= ui
                .add(
                    egui::TextEdit::singleline(&mut state.user_password)
                        .password(true)
                        .desired_width(220.0),
                )
                .changed();
        });
        if edited {
            state.unsaved_edits = true;
            state.storage_message = None;
        }
        ui.horizontal(|ui| {
            if widgets::action_button(
                ui,
                if state.stored {
                    "Update saved credentials"
                } else {
                    "Save credentials"
                },
                widgets::ActionStyle::Secondary,
                !busy && state.store_available,
            )
            .clicked()
            {
                state.save_credentials();
            }
            if widgets::action_button(
                ui,
                "Forget credentials",
                widgets::ActionStyle::Secondary,
                !busy
                    && state.request.is_none()
                    && (state.store_available
                        || state.stored
                        || state.invalid_stored
                        || state.is_configured()),
            )
            .clicked()
            {
                state.forget_credentials();
            }
        });
        if let Some(message) = state.storage_message {
            ui.label(message);
        }
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
            developer_id: "developer-account".into(),
            developer_password: "developer-secret".into(),
            user_id: "user-account".into(),
            user_password: "user-secret".into(),
            ..ScreenScraperPageState::default()
        };
        let debug = format!("{state:?}");
        assert!(!debug.contains("developer-secret"));
        assert!(!debug.contains("developer-account"));
        assert!(!debug.contains("user-secret"));
        assert!(!debug.contains("user-account"));
    }

    #[test]
    fn connection_test_is_explicit_and_does_not_render_bulk_enrichment() {
        let mut state = ScreenScraperPageState::default();
        let output = render(&mut state);
        assert!(rendered_text_contains(&output, "Test connection"));
        assert!(rendered_text_contains(&output, "Save credentials"));
        assert!(rendered_text_contains(&output, "Forget credentials"));
        assert!(!rendered_text_contains(&output, "enrich the whole library"));
        assert_eq!(IdentityContribution::None, IdentityContribution::None);
        assert_eq!(TEST_TITLE, "EmuWiz");
    }

    #[test]
    fn invalid_loaded_credentials_do_not_enter_session_or_debug_output() {
        let mut state = ScreenScraperPageState::default();
        assert!(
            state
                .set_loaded(StoredCredentials {
                    developer_id: "dev".into(),
                    developer_password: "invalid\nsecret".into(),
                    user_id: "user".into(),
                    user_password: "synthetic-secret".into(),
                })
                .is_err()
        );
        assert!(!state.is_configured());
        assert!(!format!("{state:?}").contains("synthetic-secret"));
        assert_eq!(
            status_for_error(&ScreenScraperError::Authentication { status: 401 }),
            ScreenScraperUiStatus::AuthenticationError
        );
        assert!(
            !public_error(&ScreenScraperError::Authentication { status: 401 })
                .contains("synthetic-secret")
        );
    }

    #[cfg(unix)]
    #[test]
    fn forget_removes_stale_store_item_even_when_lookup_was_unavailable() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("secret-tool");
        let stale = root.path().join("stale");
        std::fs::write(&stale, b"synthetic-stale-record").unwrap();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncase \"$1\" in\n lookup) exit 1 ;;\n clear) rm -f '{}' ;;\nesac\n",
                stale.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = SecretStore::new(&script);
        let mut state = ScreenScraperPageState {
            developer_id: "dev".into(),
            developer_password: "synthetic-password".into(),
            ..ScreenScraperPageState::default()
        };
        assert!(!state.stored);
        state.forget_credentials_with(&store);
        assert!(!stale.exists());
        assert!(state.developer_password.is_empty());
        assert_eq!(
            state.storage_message,
            Some("Credentials forgotten from the session and system secret store.")
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires an isolated, functioning Secret Service"]
    fn real_secret_service_restart_and_removal() {
        let program = std::env::var_os("EMUWIZ_REAL_SECRET_TOOL")
            .expect("set EMUWIZ_REAL_SECRET_TOOL to an isolated secret-tool executable");
        let store = SecretStore::new(std::path::Path::new(&program));
        let record = StoredCredentials {
            developer_id: "synthetic-dev".into(),
            developer_password: "synthetic-dev-password".into(),
            user_id: "synthetic-user".into(),
            user_password: "synthetic-user-password".into(),
        };
        store.save(&record).unwrap();
        let mut restarted = ScreenScraperPageState::load_from_store(&store);
        assert!(restarted.is_configured());
        assert!(restarted.stored);
        assert!(!format!("{restarted:?}").contains("synthetic-user-password"));
        restarted.forget_credentials_with(&store);
        assert!(!restarted.is_configured());
        let after_removal = ScreenScraperPageState::load_from_store(&store);
        assert!(!after_removal.is_configured());
    }
}
