//! Read-only ModDB page lookup for the Mods page.
//!
//! This only calls Core's `ModDbProvider` metadata path. It never follows a
//! download route, fetches a payload, extracts, installs or activates anything.
//! The page contacts moddb.com only when the user presses the explicit lookup
//! button; "Saved copy" never uses the network. Any package the user downloads
//! themselves goes through the existing "Inspect a mod package" workflow.
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{SystemTime, UNIX_EPOCH},
};

use archivefs_core::{
    game_identity::GameIdentityReport,
    mod_provider::{
        ModAcquisitionMode, ModCompatibilityAssessment, ModCompatibilityLevel, ModProviderError,
    },
    moddb::{ModDbCacheState, ModDbPageRecord, ModDbProvider, ModDbTransport},
};
use eframe::egui;

use crate::ui::components as widgets;
use crate::ui::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lookup {
    /// Contacts moddb.com (falls back to the saved copy, and says so).
    Network,
    /// Saved copy only; no network.
    SavedCopy,
}

type Outcome = Result<ModDbPageRecord, String>;

#[derive(Default)]
pub(super) struct State {
    url: String,
    task: Option<Receiver<Outcome>>,
    result: Option<Outcome>,
    /// The text-box value the shown result was requested with.
    result_url: String,
    requested: String,
}

impl State {
    pub(super) fn poll(&mut self) -> bool {
        let Some(task) = &self.task else { return false };
        let outcome = match task.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Err("The ModDB lookup stopped before finishing. Nothing was changed.".into())
            }
        };
        self.task = None;
        self.result = Some(outcome);
        self.result_url = self.requested.clone();
        true
    }

    fn start(&mut self, context: &egui::Context, mode: Lookup) {
        if self.task.is_some() {
            return;
        }
        let url = self.url.trim().to_string();
        self.requested = url.clone();
        let (sender, receiver) = mpsc::channel();
        self.task = Some(receiver);
        self.result = None;
        let context = context.clone();
        std::thread::spawn(move || {
            let outcome = cache_path().and_then(|path| {
                let provider = ModDbProvider::new()
                    .with_cache_path(path)
                    .map_err(|e| e.to_string())?;
                lookup(&provider, &url, mode)
            });
            let _ = sender.send(outcome);
            context.request_repaint();
        });
    }
}

fn cache_path() -> Result<PathBuf, String> {
    archivefs_core::app_dirs::data_dir()
        .map(|dir| dir.join("mod_discovery").join("moddb-cache.json"))
        .map_err(|e| format!("Could not locate EmuWiz's data folder: {e}"))
}

/// One lookup against the provided provider. Generic so tests use a fake
/// transport and never touch the network.
pub(super) fn lookup<T: ModDbTransport>(
    provider: &ModDbProvider<T>,
    url: &str,
    mode: Lookup,
) -> Outcome {
    let result = match mode {
        Lookup::Network => provider.inspect_url(url),
        Lookup::SavedCopy => provider.inspect_cached(url),
    };
    result.map_err(|e| match (mode, e) {
        (Lookup::SavedCopy, ModProviderError::ProviderUnavailable(_)) => {
            "There is no saved copy of this page yet.".to_string()
        }
        (_, e) => e.to_string(),
    })
}

/// Compatibility is only assessed against a game whose saved identity is
/// currently trustworthy; otherwise the caller passes `None`.
pub(super) fn assess(
    record: &ModDbPageRecord,
    identity: Option<(&GameIdentityReport, &str)>,
) -> Option<ModCompatibilityAssessment> {
    identity.map(|(report, title)| {
        record
            .result
            .game_evidence
            .assess_against(report, Some(title))
    })
}

pub(super) fn compatibility_text(assessment: Option<&ModCompatibilityAssessment>) -> String {
    match assessment {
        None => "Not assessed: choose a game with a verified identity to compare against.".into(),
        Some(a) => {
            let head = match a.level {
                ModCompatibilityLevel::Verified => {
                    "Verified: the page's identity matches this game exactly"
                }
                ModCompatibilityLevel::Likely => {
                    "Possible: the platform matches, but nothing proves the game"
                }
                ModCompatibilityLevel::TitleOnly => {
                    "Title only: the page's title matches; this is not proof"
                }
                ModCompatibilityLevel::Unknown => "Unknown: no usable evidence either way",
                ModCompatibilityLevel::Conflicting => {
                    "Conflict: the page's identity does not match this game"
                }
            };
            if a.reasons.is_empty() {
                head.into()
            } else {
                format!("{head} ({})", a.reasons.join("; "))
            }
        }
    }
}

pub(super) fn acquisition_text(mode: ModAcquisitionMode) -> &'static str {
    match mode {
        ModAcquisitionMode::BrowserRequired => {
            "Download it yourself in your browser from ModDB. EmuWiz will not download it."
        }
        ModAcquisitionMode::ExternalHost => {
            "Hosted on another site. EmuWiz will not download it; check the host yourself."
        }
        ModAcquisitionMode::DirectPermitted => {
            "A direct link was reported, but EmuWiz does not download from this page."
        }
        ModAcquisitionMode::Unavailable => "No download is available from this page.",
        ModAcquisitionMode::Unsupported => "This kind of download is not supported.",
    }
}

pub(super) fn freshness_text(state: ModDbCacheState, age_secs: u64) -> String {
    let age = match age_secs {
        s if s < 120 => "just now".to_string(),
        s if s < 7200 => format!("{} minutes ago", s / 60),
        s if s < 172_800 => format!("{} hours ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    };
    match state {
        ModDbCacheState::Fresh => format!("Fetched from ModDB {age}."),
        ModDbCacheState::Stale => format!("Saved copy from {age}; it may be out of date."),
        ModDbCacheState::Offline => {
            format!("ModDB could not be reached. Showing the saved copy from {age}.")
        }
    }
}

fn size_text(bytes: u64) -> String {
    format!("{:.1} MiB ({bytes} bytes)", bytes as f64 / 1_048_576.0)
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    identity: Option<(&GameIdentityReport, &str)>,
) {
    state.poll();
    widgets::card(ui, |ui| {
        ui.heading("Look up a mod on ModDB");
        ui.label("Paste a ModDB page address to read its details. Nothing is downloaded, installed or changed. EmuWiz contacts moddb.com only when you press \"Look up on ModDB\".");
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.url)
                    .hint_text("https://www.moddb.com/mods/…")
                    .desired_width(420.0),
            );
            let ready = state.task.is_none() && !state.url.trim().is_empty();
            if ui
                .add_enabled(ready, egui::Button::new("Look up on ModDB"))
                .on_hover_text("Contacts moddb.com to read this one page.")
                .clicked()
            {
                state.start(ui.ctx(), Lookup::Network);
            }
            if ui
                .add_enabled(ready, egui::Button::new("Saved copy (no network)"))
                .clicked()
            {
                state.start(ui.ctx(), Lookup::SavedCopy);
            }
        });
        if state.task.is_some() {
            ui.spinner();
            ui.label("Reading the page…");
        }
        let current = state.result_url == state.url.trim();
        if state.result.is_some() && !current && state.task.is_none() {
            ui.label(
                "The address changed since the last lookup. Look it up again to see its details.",
            );
        }
        match state.result.as_ref().filter(|_| current) {
            None => {}
            Some(Err(error)) => {
                ui.colored_label(
                    theme::warning(ui),
                    format!("Lookup did not complete: {error}"),
                );
                ui.label("Nothing was changed.");
            }
            Some(Ok(record)) => show_record(ui, record, identity),
        }
    });
}

fn show_record(
    ui: &mut egui::Ui,
    record: &ModDbPageRecord,
    identity: Option<(&GameIdentityReport, &str)>,
) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let result = &record.result;
    ui.separator();
    ui.strong(&result.title);
    ui.label(freshness_text(
        record.cache_state,
        now.saturating_sub(record.retrieved_at_unix_secs),
    ));
    if let Some(warning) = &record.warning {
        ui.colored_label(theme::warning(ui), warning);
    }
    if let Some(author) = &result.author_or_team {
        ui.label(format!("Author: {author}"));
    }
    if let Some(game) = &result.game_claim {
        ui.label(format!(
            "Says it is for: {game} (the page's claim, not verified)"
        ));
    }
    if !result.platform_claims.is_empty() {
        let names: Vec<String> = result
            .platform_claims
            .iter()
            .map(|p| format!("{p:?}"))
            .collect();
        ui.label(format!("Platform claims: {}", names.join(", ")));
    }
    if let Some(summary) = &result.summary {
        ui.label(summary);
    }
    ui.label(format!(
        "Compatibility: {}",
        compatibility_text(assess(record, identity).as_ref())
    ));
    ui.label("Licence and redistribution terms are not read by EmuWiz. Check them on the ModDB page before using or sharing this mod.");
    ui.horizontal_wrapped(|ui| {
        ui.label("Source:");
        // A link is opened only by the user clicking it.
        ui.hyperlink_to("Open on ModDB in your browser", &record.canonical_url);
    });

    for release in &record.releases {
        for file in &release.files {
            ui.add_space(theme::SPACE_SM);
            ui.label(format!(
                "File: {}",
                file.reported_filename
                    .as_deref()
                    .unwrap_or("name not reported")
            ));
            if let Some(size) = file.size_bytes {
                ui.label(format!("Size reported by ModDB: {}", size_text(size)));
            }
            if let Some(hash) = &file.reported_checksum {
                ui.label(format!(
                    "Checksum reported by ModDB ({:?}): {} — EmuWiz will compare it after you download and inspect the file.",
                    hash.algorithm, hash.value
                ));
            }
            ui.label(acquisition_text(file.download.mode));
        }
    }
    ui.label("After downloading it yourself, use \"Inspect a mod package\" below. That is the only route that can preview or apply a mod.");
    widgets::technical_details(ui, "moddb_lookup_details", |ui| {
        ui.label(format!("Requested: {}", record.original_url));
        ui.label(format!("Canonical: {}", record.canonical_url));
        ui.label(format!(
            "Page fingerprint (SHA-256): {}",
            record.source_fingerprint_sha256
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use archivefs_core::moddb::ModDbHttpResponse;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const PAGE: &str = r#"<html><head><title>SLES 53564 addon - Darkwatch Texture Pack</title></head>
      <body><h1>SLES 53564 addon - Darkwatch Texture Pack</h1>
      <a href="/games/darkwatch">Darkwatch</a><a href="/tags/ps2">ps2</a>
      <div>Filename SLES-53564.rar Category Texture Licence Proprietary Uploader fixture Added Jan 1st, 2026 Size 3.12gb (3,353,127,342 bytes) MD5 Hash a2789929816064d679306c62d53a3823 Embed Button Description</div>
      </body></html>"#;
    const URL: &str = "https://www.moddb.com/mods/darkwatch/addons/sles-53564";

    struct Fake {
        calls: Arc<AtomicUsize>,
        ok: bool,
    }
    impl ModDbTransport for Fake {
        fn get(&self, url: &str) -> Result<ModDbHttpResponse, ModProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.ok {
                return Err(ModProviderError::ProviderUnavailable("offline".into()));
            }
            Ok(ModDbHttpResponse {
                status: 200,
                location: None,
                content_type: Some("text/html".into()),
                body: PAGE.as_bytes().to_vec(),
                retrieved_url: url.into(),
            })
        }
    }

    fn provider(ok: bool) -> ModDbProvider<Fake> {
        counted(ok).0
    }

    fn counted(ok: bool) -> (ModDbProvider<Fake>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            ModDbProvider::with_transport(Fake {
                calls: calls.clone(),
                ok,
            }),
            calls,
        )
    }

    #[test]
    fn network_lookup_returns_metadata_without_a_download_route() {
        let p = provider(true);
        let record = lookup(&p, URL, Lookup::Network).unwrap();
        assert_eq!(record.cache_state, ModDbCacheState::Fresh);
        let file = &record.releases[0].files[0];
        assert!(file.download.acquisition_url.is_none());
        assert_eq!(file.download.mode, ModAcquisitionMode::BrowserRequired);
        assert!(acquisition_text(file.download.mode).contains("will not download"));
    }

    #[test]
    fn saved_copy_never_uses_the_network() {
        let (p, calls) = counted(true);
        assert!(lookup(&p, URL, Lookup::SavedCopy).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn saved_copy_is_served_offline_and_labelled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("moddb-cache.json");
        let online = provider(true).with_cache_path(path.clone()).unwrap();
        lookup(&online, URL, Lookup::Network).unwrap();
        let down = provider(false).with_cache_path(path.clone()).unwrap();
        let fallback = lookup(&down, URL, Lookup::Network).unwrap();
        assert_eq!(fallback.cache_state, ModDbCacheState::Offline);
        assert!(freshness_text(fallback.cache_state, 10).contains("could not be reached"));
        let saved = provider(true).with_cache_path(path).unwrap();
        assert!(lookup(&saved, URL, Lookup::SavedCopy).is_ok());
    }

    #[test]
    fn non_moddb_or_non_https_urls_are_refused_before_any_request() {
        let (p, calls) = counted(true);
        for bad in [
            "http://www.moddb.com/mods/x",
            "https://example.com/mods/x",
            "file:///etc/passwd",
            "",
        ] {
            assert!(lookup(&p, bad, Lookup::Network).is_err(), "{bad}");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn compatibility_is_not_assessed_without_a_verified_game() {
        let record = lookup(&provider(true), URL, Lookup::Network).unwrap();
        assert!(assess(&record, None).is_none());
        assert!(compatibility_text(None).contains("Not assessed"));
    }

    #[test]
    fn age_text_is_honest_about_stale_copies() {
        assert!(freshness_text(ModDbCacheState::Stale, 3 * 86_400).contains("out of date"));
        assert!(freshness_text(ModDbCacheState::Fresh, 5).contains("just now"));
    }
}
