//! Game Details "Check Hasheous": one optional online lookup for the selected
//! game, started only by the person clicking the button.
//!
//! The request is the existing, privacy-tested
//! [`crate::selected_evidence_page::run_hasheous_check`]: only a SHA-1 leaves
//! the machine, never a path or file name. The SHA-1 is, in order:
//!
//! 1. the one EmuWiz saved from this game's own current catalogue check
//!    ([`Game::saved_hashes`], the same current-file rules as DAT evidence);
//! 2. otherwise, after the same click, the whole file's SHA-1 worked out on a
//!    worker by the verification engine's own reader
//!    ([`archivefs_core::identity_source::hashing::hash_file_reporting`]:
//!    trusted roots only, bounded, refuses a file that changes while read,
//!    cancellable). The file must still be the one the Library last scanned
//!    (size and modification time), and a container, compressed disc image
//!    or disc descriptor is refused: its own bytes are not what a catalogue
//!    lists, and no member or track hash is substituted.
//!
//! What Hasheous answers is a remote service saying that the hash appears in
//! another project's catalogue (No-Intro, Redump, ...). It is shown as such and
//! kept in memory for this session only, bound to the game and the exact file
//! it was worked out from. It never becomes DAT verification, never changes the
//! Library, the database, identity evidence or launch checks, and never
//! downloads anything. A worked-out SHA-1 is not saved either.
use super::library::Game;
use crate::selected_evidence_page::HasheousCheckOutcome;
use archivefs_core::identity_source::hashing::{FileFingerprint, HashRefusal, hash_file_reporting};
use archivefs_core::platform_evidence_fusion::evidence_lineage::{
    ClaimType, EvidenceObservation, IdentityScope, SourceFamily,
};
use archivefs_core::safe_read::TrustedRoots;
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

#[cfg(test)]
mod tests;
mod zip_rom;

pub(super) const CHECK_LABEL: &str = "Check Hasheous";
pub(super) const CHECK_AGAIN_LABEL: &str = "Check Hasheous again";
pub(super) const CANCEL_LABEL: &str = "Cancel";
pub(super) const NOT_VERIFICATION: &str = "Hasheous is an online service. A match means it lists this hash in another project's catalogue; it is not checked against your catalogues and does not verify the game or change launch checks. Nothing is saved.";
pub(super) const HASH_FIRST: &str = "EmuWiz will first read this file on this computer to work out its SHA-1, then send only the SHA-1 to hasheous.org.";
/// What a ZIP result says about where its SHA-1 came from.
pub(super) fn zip_worked_out(member: &str) -> String {
    format!(
        "SHA-1 of {} inside this ZIP, as stored (no header removed), worked out just now. It is not saved.",
        clean(member)
    )
}

/// The one short line under the idle button; the full wording is on hover.
pub(super) const IDLE_HASH_FIRST: &str =
    "Optional: reads this file here, then sends only its SHA-1. Not verification.";
pub(super) const IDLE_SAVED: &str =
    "Optional: sends only this game's saved SHA-1. Not verification.";
pub(super) const SAVED_SHA1: &str = "Sends only this game's saved SHA-1 to hasheous.org.";
pub(super) const WORKED_OUT_HERE: &str =
    "SHA-1 worked out from this file just now. It is not saved.";
pub(super) const FILE_CHANGED: &str =
    "This game's saved hashes no longer match its file. Check this game again first.";
pub(super) const CHANGED_SINCE_SCAN: &str =
    "This file has changed since the Library last scanned it. Rescan it first; nothing was sent.";
pub(super) const UNAVAILABLE_FILE: &str =
    "The game file is missing or damaged, so it cannot be checked.";
pub(super) const INCOMPLETE_SCAN: &str =
    "The last scan could not finish reading this file. Rescan it first.";
pub(super) const NO_SCAN_SIZE: &str =
    "The Library has no recorded size for this file. Rescan it first.";
pub(super) const ARCHIVE: &str = "This archive type is not read here. Only a ZIP holding one cartridge ROM can be checked, and no other file's hash is substituted.";
pub(super) const COMPRESSED_DISC: &str =
    "This is a compressed disc image. Its own hash is not the disc's, so it is not sent.";
pub(super) const DESCRIPTOR: &str = "This file lists a disc's track files. Catalogues list the tracks, so this file's own hash is not sent.";
pub(super) const NOT_GAME_FILE: &str =
    "This file is not a game file type EmuWiz recognises, so it is not read or sent.";
pub(super) const NO_MATCH: &str =
    "No Hasheous entry matched the submitted SHA-1. This does not mean the game is unknown.";
/// Shown with a no-match: Hasheous's header convention is not established, so
/// a headered ROM may be listed only under its headerless hash.
pub(super) const WHOLE_FILE_NOTE: &str = "The SHA-1 covers the whole file as stored, including any ROM header (such as an NES header). A catalogue that lists the game without its header has a different SHA-1.";
pub(super) const CANCELLED: &str = "Cancelled. Nothing was sent.";

/// Whose SHA-1 a lookup uses, and the file it is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Availability {
    /// The saved, current catalogue-check SHA-1.
    Saved {
        sha1: String,
    },
    /// No saved SHA-1: a click works it out from this exact scanned file.
    HashOnClick(ScannedFile),
    Unavailable(&'static str),
}

/// The file as the Library last scanned it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScannedFile {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified_unix_seconds: Option<i64>,
    /// Set for a ZIP: the system whose single ROM inside it is hashed.
    pub zip_platform: Option<String>,
}

impl ScannedFile {
    /// True when `seen` is still the file the Library scanned.
    fn matches(&self, seen: &FileFingerprint) -> bool {
        seen.size_bytes == self.size_bytes
            && (self.modified_unix_seconds.is_none()
                || seen.modified_unix_seconds == self.modified_unix_seconds)
    }
}

pub(super) fn availability(game: &Game) -> Availability {
    if game.dat_proof_invalidated {
        return Availability::Unavailable(FILE_CHANGED);
    }
    if let Some(sha1) = game
        .saved_hashes
        .as_ref()
        .and_then(|hashes| hashes.sha1.as_deref())
        .map(str::trim)
        .filter(|sha1| is_sha1(sha1))
    {
        return Availability::Saved {
            sha1: sha1.to_ascii_lowercase(),
        };
    }
    if game.attention {
        return Availability::Unavailable(UNAVAILABLE_FILE);
    }
    if game.inspection_incomplete {
        return Availability::Unavailable(INCOMPLETE_SCAN);
    }
    if let Some(reason) = unsupported_format(&game.archive.absolute_path) {
        return Availability::Unavailable(reason);
    }
    if !is_game_file(&game.archive.absolute_path) {
        return Availability::Unavailable(NOT_GAME_FILE);
    }
    let Some(size_bytes) = game.archive.size_bytes else {
        return Availability::Unavailable(NO_SCAN_SIZE);
    };
    Availability::HashOnClick(ScannedFile {
        path: game.archive.absolute_path.clone(),
        size_bytes,
        modified_unix_seconds: game.archive.modified_time_unix_seconds,
        zip_platform: is_zip(&game.archive.absolute_path).then(|| game.platform.clone()),
    })
}

/// Files whose own bytes are not what a catalogue lists.
pub(super) fn unsupported_format(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "7z" | "rar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "lha" | "lzh" | "tar" => Some(ARCHIVE),
        "chd" | "rvz" | "wia" | "cso" | "zso" | "pbp" | "gcz" | "wbfs" => Some(COMPRESSED_DISC),
        "cue" | "gdi" | "m3u" | "ccd" | "mds" | "toc" => Some(DESCRIPTOR),
        _ => None,
    }
}

/// Only file types the Library scanner itself treats as game media are read,
/// so a stale or wrong Library row can never send an unrelated file's hash.
fn is_game_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| {
            archivefs_core::media_registry::is_watch_relevant_extension(&extension)
        })
}

fn is_zip(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

fn is_sha1(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The lookup itself; the real one goes to Hasheous, tests pass a fake.
pub(super) type Lookup = Arc<dyn Fn(&str) -> HasheousCheckOutcome + Send + Sync>;
/// The folders a click may read from; the real one is the configured game folders.
pub(super) type Roots = Arc<dyn Fn() -> TrustedRoots + Send + Sync>;

fn live_lookup() -> Lookup {
    Arc::new(|sha1| {
        use archivefs_core::identity_source::hasheous::client::{
            HASHEOUS_DEFAULT_BASE_URL, HasheousConfig, REQUEST_TIMEOUT,
        };
        // Clicking the button is the opt-in; Hasheous has no stored setting.
        let config = HasheousConfig {
            enabled: true,
            base_url: HASHEOUS_DEFAULT_BASE_URL.to_string(),
            timeout: REQUEST_TIMEOUT,
        };
        crate::selected_evidence_page::run_hasheous_check_live(&config, sha1)
    })
}

fn configured_roots() -> Roots {
    Arc::new(|| {
        archivefs_core::Config::load_default()
            .ok()
            .as_ref()
            .map(TrustedRoots::from_config)
            .unwrap_or_else(TrustedRoots::none)
    })
}

enum Message {
    Progress(f32),
    /// The SHA-1 is known; for a ZIP, the member it came from.
    Hashed(Option<String>),
    Refused(String),
    Cancelled,
    Answer(HasheousCheckOutcome),
}

#[derive(Debug)]
enum Phase {
    Hashing(Option<f32>),
    LookingUp,
    Done(HasheousCheckOutcome),
    Refused(String),
    Cancelled,
}

struct Entry {
    /// What the lookup was bound to when it was clicked.
    basis: Availability,
    phase: Phase,
    receiver: Option<Receiver<Message>>,
    cancel: Arc<AtomicBool>,
    /// The ZIP member the SHA-1 was worked out from.
    member: Option<String>,
}

/// Per-game results for this session, keyed by library row and bound to the
/// saved SHA-1 or scanned file they came from, so a result only ever shows on
/// its own game and file.
pub(super) struct State {
    entries: HashMap<i64, Entry>,
    lookup: Lookup,
    roots: Roots,
}

impl Default for State {
    fn default() -> Self {
        Self::with(live_lookup(), configured_roots())
    }
}

/// What the panel shows for one game.
#[derive(Debug)]
pub(super) enum View<'a> {
    Unavailable(&'static str),
    Idle {
        hash_first: bool,
    },
    Hashing(Option<f32>),
    LookingUp,
    Done {
        outcome: &'a HasheousCheckOutcome,
        worked_out_here: bool,
        /// The ZIP member whose SHA-1 was sent.
        member: Option<&'a str>,
    },
    Refused(&'a str),
    Cancelled,
}

/// What the person asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Request {
    Check,
    Cancel,
}

impl State {
    pub(super) fn with(lookup: Lookup, roots: Roots) -> Self {
        Self {
            entries: HashMap::new(),
            lookup,
            roots,
        }
    }

    /// Starts one lookup off the UI thread (hashing first when needed). Does
    /// nothing when the game cannot be checked or a lookup is running.
    pub(super) fn start(&mut self, game: &Game, context: Option<egui::Context>) -> bool {
        let basis = availability(game);
        if matches!(basis, Availability::Unavailable(_)) {
            return false;
        }
        let id = game.archive.id;
        if self
            .entries
            .get(&id)
            .is_some_and(|entry| entry.receiver.is_some())
        {
            return false;
        }
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let phase = match &basis {
            Availability::Saved { .. } => Phase::LookingUp,
            _ => Phase::Hashing(None),
        };
        let job = Job {
            basis: basis.clone(),
            lookup: Arc::clone(&self.lookup),
            roots: Arc::clone(&self.roots),
            cancel: Arc::clone(&cancel),
            sender,
            context,
        };
        std::thread::spawn(move || job.run());
        self.entries.insert(
            id,
            Entry {
                basis,
                phase,
                receiver: Some(receiver),
                cancel,
                member: None,
            },
        );
        true
    }

    /// Stops a lookup that is still working out its SHA-1. Once Hasheous has
    /// been asked, the answer is shown as usual.
    pub(super) fn cancel(&mut self, game: &Game) {
        if let Some(entry) = self.entries.get(&game.archive.id)
            && matches!(entry.phase, Phase::Hashing(_))
        {
            entry.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Settles worker messages. Each lands only on the game it was started
    /// for, whichever page is showing.
    pub(super) fn poll(&mut self) {
        for entry in self.entries.values_mut() {
            let Some(receiver) = &entry.receiver else {
                continue;
            };
            let mut finished = false;
            loop {
                match receiver.try_recv() {
                    Ok(Message::Progress(fraction)) => entry.phase = Phase::Hashing(Some(fraction)),
                    Ok(Message::Hashed(member)) => {
                        entry.member = member;
                        entry.phase = Phase::LookingUp;
                    }
                    Ok(Message::Refused(reason)) => {
                        entry.phase = Phase::Refused(reason);
                        finished = true;
                    }
                    Ok(Message::Cancelled) => {
                        entry.phase = Phase::Cancelled;
                        finished = true;
                    }
                    Ok(Message::Answer(outcome)) => {
                        entry.phase = Phase::Done(outcome);
                        finished = true;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if !finished {
                            entry.phase = Phase::Done(HasheousCheckOutcome::NetworkError(
                                "the lookup stopped before it answered".to_string(),
                            ));
                        }
                        finished = true;
                        break;
                    }
                }
                if finished {
                    break;
                }
            }
            if finished {
                entry.receiver = None;
            }
        }
    }

    pub(super) fn view(&self, game: &Game) -> View<'_> {
        let basis = availability(game);
        if let Availability::Unavailable(reason) = basis {
            return View::Unavailable(reason);
        }
        // A result for other hashes or another version of the file is not this file's.
        let Some(entry) = self
            .entries
            .get(&game.archive.id)
            .filter(|entry| entry.basis == basis)
        else {
            return View::Idle {
                hash_first: matches!(basis, Availability::HashOnClick(_)),
            };
        };
        match &entry.phase {
            Phase::Hashing(fraction) => View::Hashing(*fraction),
            Phase::LookingUp => View::LookingUp,
            Phase::Done(outcome) => View::Done {
                outcome,
                worked_out_here: matches!(basis, Availability::HashOnClick(_)),
                member: entry.member.as_deref(),
            },
            Phase::Refused(reason) => View::Refused(reason),
            Phase::Cancelled => View::Cancelled,
        }
    }
}

/// One click's background work.
struct Job {
    basis: Availability,
    lookup: Lookup,
    roots: Roots,
    cancel: Arc<AtomicBool>,
    sender: Sender<Message>,
    context: Option<egui::Context>,
}

impl Job {
    fn run(self) {
        let message = match self.sha1() {
            Ok(sha1) if !self.cancel.load(Ordering::SeqCst) => {
                Message::Answer((self.lookup)(&sha1))
            }
            Ok(_) => Message::Cancelled,
            Err(message) => message,
        };
        self.send(message);
    }

    fn send(&self, message: Message) {
        let _ = self.sender.send(message);
        if let Some(context) = &self.context {
            context.request_repaint();
        }
    }

    fn sha1(&self) -> Result<String, Message> {
        let file = match &self.basis {
            Availability::Saved { sha1 } => return Ok(sha1.clone()),
            Availability::HashOnClick(file) => file,
            Availability::Unavailable(reason) => return Err(Message::Refused((*reason).into())),
        };
        // The Library's change detection: only the file it scanned is read,
        // and it must be the very same file object (device, inode, change
        // time) after reading as before, so a replacement is never hashed.
        let before = match FileFingerprint::observe(&file.path) {
            Some(seen) if file.matches(&seen) => seen,
            Some(_) => return Err(Message::Refused(CHANGED_SINCE_SCAN.into())),
            None => return Err(Message::Refused(UNAVAILABLE_FILE.into())),
        };
        let unchanged = || FileFingerprint::observe(&file.path).as_ref() == Some(&before);
        let roots = (self.roots)();
        if let Some(platform) = &file.zip_platform {
            let rom = zip_rom::sole_rom_sha1(&file.path, platform, &roots, &self.cancel);
            return match rom {
                Ok(rom) if unchanged() && is_sha1(&rom.sha1) => {
                    self.send(Message::Hashed(Some(rom.member)));
                    Ok(rom.sha1)
                }
                Ok(_) | Err(zip_rom::ZipRefusal::Changed) => {
                    Err(Message::Refused(CHANGED_SINCE_SCAN.into()))
                }
                Err(zip_rom::ZipRefusal::Cancelled) => Err(Message::Cancelled),
                Err(zip_rom::ZipRefusal::Refused(reason)) => Err(Message::Refused(reason)),
            };
        }
        let last = std::sync::Mutex::new(-1_i32);
        let hashed = hash_file_reporting(&file.path, &roots, Some(&self.cancel), &|progress| {
            if let Some(fraction) = progress.fraction() {
                let step = (fraction * 100.0) as i32;
                let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                if step != *last {
                    *last = step;
                    self.send(Message::Progress(fraction));
                }
            }
        });
        match hashed {
            Ok(hashes) if hashes.fingerprint == before && unchanged() && is_sha1(&hashes.sha1) => {
                self.send(Message::Hashed(None));
                Ok(hashes.sha1.to_ascii_lowercase())
            }
            Ok(_) => Err(Message::Refused(CHANGED_SINCE_SCAN.into())),
            Err(HashRefusal::Cancelled) => Err(Message::Cancelled),
            Err(refusal) => Err(Message::Refused(format!(
                "This file could not be read, so nothing was sent: {}",
                clean(&refusal.detail())
            ))),
        }
    }
}

/// One catalogue entry Hasheous reported for the hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReportedMatch {
    pub source: String,
    pub title: Option<String>,
}

/// The parts of a found answer the panel shows, read from the untrusted
/// response only as display text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Summary {
    pub matches: Vec<ReportedMatch>,
    pub platform: Option<String>,
    pub publisher: Option<String>,
}

pub(super) fn summarise(observations: &[EvidenceObservation]) -> Summary {
    let mut summary = Summary::default();
    for observation in observations {
        match observation.claim {
            ClaimType::PlatformCandidate
                if observation.identity_scope == IdentityScope::PlatformIdentity =>
            {
                summary.platform = observation.platform_candidate.as_deref().map(clean);
            }
            ClaimType::DisplayMetadata => {
                summary.publisher = observation.release_candidate.as_deref().map(clean);
            }
            _ if observation.identity_scope == IdentityScope::DumpIdentity => {
                let found = ReportedMatch {
                    source: source_label(observation.provenance.upstream_source).to_string(),
                    title: observation.release_candidate.as_deref().map(clean),
                };
                if !summary.matches.contains(&found) {
                    summary.matches.push(found);
                }
            }
            _ => {}
        }
    }
    summary
}

/// Provider text is shown as plain, bounded, single-line text.
fn clean(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    match trimmed.char_indices().nth(120) {
        Some((cut, _)) => format!("{}…", &trimmed[..cut]),
        None => trimmed.to_string(),
    }
}

pub(super) fn source_label(family: SourceFamily) -> &'static str {
    match family {
        SourceFamily::NoIntro => "No-Intro",
        SourceFamily::TOSEC => "TOSEC",
        SourceFamily::Redump => "Redump",
        SourceFamily::MAMEArcade => "MAME",
        SourceFamily::MAMESoftwareList => "MAME software list",
        SourceFamily::MAMERedump => "MAME (Redump)",
        SourceFamily::WHDLoad => "WHDLoad",
        SourceFamily::Retroplay => "Retroplay",
        SourceFamily::PureDOS => "eXoDOS/PureDOS",
        SourceFamily::TotalDOSCollection => "Total DOS Collection",
        SourceFamily::FBNeo => "FinalBurn Neo",
        SourceFamily::RetroAchievements => "RetroAchievements",
        SourceFamily::ScreenScraper => "ScreenScraper",
        SourceFamily::GenericMetadata => "Hasheous",
        SourceFamily::Unknown => "an unrecognised source",
    }
}

/// The one-line status for an answer: no match, failure and match never
/// share wording.
pub(super) fn status_line(outcome: &HasheousCheckOutcome) -> String {
    match outcome {
        HasheousCheckOutcome::Found(observations) => {
            let count = summarise(observations).matches.len();
            if count == 0 {
                "Hasheous answered but named no catalogue entry".to_string()
            } else if count == 1 {
                "Hasheous lists this SHA-1 in 1 catalogue entry".to_string()
            } else {
                format!("Hasheous lists this SHA-1 in {count} catalogue entries")
            }
        }
        HasheousCheckOutcome::NoMatch => NO_MATCH.to_string(),
        HasheousCheckOutcome::Disabled => "Hasheous lookups are turned off".to_string(),
        HasheousCheckOutcome::Timeout => {
            "Hasheous did not answer in time. Try again later.".to_string()
        }
        HasheousCheckOutcome::RateLimited => {
            "Hasheous is busy and asked EmuWiz to slow down. Try again later.".to_string()
        }
        HasheousCheckOutcome::NetworkError(detail) => {
            format!("Hasheous could not be checked: {}", clean(detail))
        }
    }
}

/// Draws the Hasheous section. Returns what the person asked for; drawing
/// never starts or stops anything itself.
pub(super) fn show(ui: &mut egui::Ui, view: &View<'_>) -> Option<Request> {
    let mut request = None;
    ui.horizontal_wrapped(|ui| {
        ui.strong("Hasheous (online)");
        match view {
            View::Unavailable(reason) => {
                ui.add_enabled(false, egui::Button::new(CHECK_LABEL))
                    .on_disabled_hover_text(*reason);
            }
            View::Idle { hash_first } => {
                let hint = if *hash_first { HASH_FIRST } else { SAVED_SHA1 };
                if ui
                    .button(CHECK_LABEL)
                    .on_hover_text(format!("{hint}\n\n{NOT_VERIFICATION}"))
                    .clicked()
                {
                    request = Some(Request::Check);
                }
            }
            View::Hashing(fraction) => {
                ui.spinner();
                ui.label(match fraction {
                    Some(fraction) => format!(
                        "Working out this file's SHA-1… {}%",
                        (fraction * 100.0).round() as u32
                    ),
                    None => "Working out this file's SHA-1…".to_string(),
                });
                if ui.small_button(CANCEL_LABEL).clicked() {
                    request = Some(Request::Cancel);
                }
            }
            View::LookingUp => {
                ui.spinner();
                ui.label("Checking Hasheous…");
            }
            View::Done { .. } | View::Refused(_) | View::Cancelled => {
                if ui.small_button(CHECK_AGAIN_LABEL).clicked() {
                    request = Some(Request::Check);
                }
            }
        }
    });
    match view {
        View::Unavailable(reason) => {
            ui.small(*reason);
        }
        View::Idle { hash_first } => {
            ui.small(if *hash_first {
                IDLE_HASH_FIRST
            } else {
                IDLE_SAVED
            });
        }
        View::Done {
            outcome,
            worked_out_here,
            member,
        } => {
            ui.label(status_line(outcome));
            if matches!(outcome, HasheousCheckOutcome::NoMatch) {
                ui.small(WHOLE_FILE_NOTE);
            }
            if let HasheousCheckOutcome::Found(observations) = outcome {
                let summary = summarise(observations);
                for found in &summary.matches {
                    ui.label(format!(
                        "· {} — reported by Hasheous from {}",
                        found.title.as_deref().unwrap_or("(no title given)"),
                        found.source
                    ));
                }
                if let Some(platform) = &summary.platform {
                    ui.small(format!("Hasheous platform: {platform}"));
                }
                if let Some(publisher) = &summary.publisher {
                    ui.small(format!("Hasheous publisher: {publisher}"));
                }
            }
            match member {
                Some(member) => {
                    ui.small(zip_worked_out(member));
                }
                None if *worked_out_here => {
                    ui.small(WORKED_OUT_HERE);
                }
                None => {}
            }
            ui.small(NOT_VERIFICATION);
        }
        View::Refused(reason) => {
            ui.label(*reason);
        }
        View::Cancelled => {
            ui.label(CANCELLED);
        }
        View::Hashing(_) | View::LookingUp => {}
    }
    request
}
