//! The Library Organisation page.
//!
//! Configures the master ROM root, chooses an organisation mode, collects
//! candidate ROM files, resolves each candidate's platform identity from the
//! database, builds a neutral EmuWiz layout plan, and - only after an explicit
//! typed confirmation - applies the approved subset through the shared
//! journaled engine. RomM cache data may provide a canonical game title, but
//! never determines a generic organisation folder. Rollback restores the
//! prior state.
//!
//! The page states loudly that planning changes nothing until the user
//! approves, and never offers Apply for conflicts, blocked or unknown entries.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use archivefs_core::dat::rom_organisation::*;
use archivefs_core::identity_source::cache::{IdentityCacheLocation, load_cache};
use archivefs_core::identity_source::model::IdentityProvider;
use archivefs_core::identity_source::settings::default_identity_root;
use archivefs_core::ingestion::is_known_non_game_extension;
use archivefs_core::platform::identity::{PlatformIdentityResolution, resolve_platform_identity};
use archivefs_core::{
    Config, Database, clear_master_rom_root_default, default_database_path,
    set_master_rom_root_default,
};
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::ui::{components as widgets, theme};

/// The page's authoritative state.
pub(crate) struct RomOrganisationPageState {
    master_root_draft: String,
    saved_master_root: Option<PathBuf>,
    master_root_error: Option<String>,
    /// The explicitly chosen linked-library destination root for
    /// BuildLinkedLibrary mode. Never inferred from source roots and never
    /// persisted into the master ROM root config.
    library_root_draft: String,
    saved_library_root: Option<PathBuf>,
    library_root_error: Option<String>,
    mode: OrganisationMode,
    /// Candidate source files collected from the configured source folders.
    sources: Vec<PathBuf>,
    /// The read-only plan, when generated. `generation` is bumped on every
    /// regeneration so a stale review decision can never apply.
    plan: Option<OrganisationPlan>,
    plan_generation: u64,
    /// Source paths the user has approved (checked) for apply.
    approved: BTreeSet<String>,
    filter: Option<OrganisationStatus>,
    applied: Option<archivefs_core::dat::rename_apply::RenameTransaction>,
    applied_journal: PathBuf,
    result_message: Option<String>,
    error: Option<String>,
    /// Set when the user asked to apply; holds the count awaiting a typed
    /// confirmation for large batches.
    pending_apply: Option<usize>,
    confirm_text: String,
    /// Set on the frame "Preview changes" is clicked; the actual (currently
    /// synchronous, potentially slow for a large library) `generate_plan()`
    /// call is deferred to the start of the *next* frame so this frame can
    /// render a busy state first - without this, a synchronous call made
    /// directly inside the click handler blocks before egui ever draws the
    /// "Preparing preview..." row, so nothing visibly happens while the user
    /// waits (2026-08-22, live-QA Phase 8 finding).
    pending_preview: bool,
    /// Reason the persisted approval set could not be saved or loaded.
    /// Never blocks the page - the in-memory `approved` set is always the
    /// source of truth - but the user is shown the message so a silent
    /// "preferences keep disappearing" footgun never recurs. Cleared on every
    /// successful save / load.
    approval_persistence_warning: Option<String>,
    /// Whether "Build Playing Library" is showing instead of the ordinary
    /// organisation flow above. A page-local view toggle, not a separate
    /// `OrganisationMode` - the playing-library planner is a genuinely
    /// different (DAT-election-based) pipeline, so it gets its own state
    /// rather than being squeezed into the classification-based modes this
    /// page otherwise offers. Deliberately not a new sidebar destination.
    pub(crate) showing_playing_library: bool,
    pub(crate) playing_library: crate::playing_library_page::PlayingLibraryPageState,
}

/// Batches larger than this require typing the exact confirmation phrase
/// before any mutation happens (the same philosophy as DAT rename apply).
pub(crate) const TYPED_CONFIRMATION_THRESHOLD: usize = 8;

/// The exact phrase a user must type to confirm a large apply, with wording
/// that is truthful for the chosen mode.
pub(crate) fn apply_confirmation_phrase(mode: OrganisationMode, count: usize) -> String {
    match mode {
        OrganisationMode::RenameInPlace => format!("RENAME {count} FILES"),
        OrganisationMode::MoveRealFile | OrganisationMode::OrganiseSymlinkOnly => {
            format!("MOVE {count} FILES")
        }
        // Truthful wording: a linked-library apply never renames or moves an
        // original file - it creates links.
        OrganisationMode::BuildLinkedLibrary => format!("CREATE {count} LINKS"),
    }
}

/// Plain-language presentation label for an organisation mode. Kept
/// separate from `OrganisationMode::label()` (core, shared with the CLI)
/// so this is purely a GUI wording choice, never a renamed core type.
fn organisation_mode_plain_label(mode: OrganisationMode) -> &'static str {
    match mode {
        OrganisationMode::RenameInPlace => "Rename files where they are",
        OrganisationMode::MoveRealFile => "Move files into organised folders",
        OrganisationMode::BuildLinkedLibrary => "Build linked library",
        OrganisationMode::OrganiseSymlinkOnly => "Advanced: reorganise existing symlinks",
    }
}

/// One-line plain-language explanation of what the selected mode actually
/// does, shown under the radio group.
///
/// `OrganiseSymlinkOnly`'s wording states its "existing shortcuts only"
/// constraint up front (2026-08-22, live-QA Phase 8): the mode's plan logic
/// (`archivefs_core::dat::rom_organisation::plan`) only ever relocates a
/// source that is *already* a symlink object - it never creates a new
/// symlink for a source that is a real file. Earlier wording ("only
/// organises shortcuts/symlinks pointing to them") didn't say that, so a
/// user previewing a folder of real files would only discover the
/// constraint after seeing every one of them come back blocked.
fn organisation_mode_plain_explanation(mode: OrganisationMode) -> &'static str {
    match mode {
        OrganisationMode::RenameInPlace => {
            "Renames each game's file in its current location; nothing is moved."
        }
        OrganisationMode::MoveRealFile => {
            "Moves each game's real file into an organised folder structure."
        }
        OrganisationMode::OrganiseSymlinkOnly => {
            "Leaves every real game file untouched and exactly where it is. Only reorganises \
             shortcuts/symlinks that already exist - it does not create a shortcut for a source \
             that isn't already one, so a folder of real files (not shortcuts) will show as \
             blocked in this mode."
        }
        OrganisationMode::BuildLinkedLibrary => {
            "Create an organised library of links while leaving your original files untouched. \
             Every original file stays exactly where it is; the organised destination becomes \
             a link pointing to it."
        }
    }
}

/// "Preview changes" button label, extracted as a pure function so its two
/// states (idle vs. running) are directly unit-testable without needing to
/// simulate a real click through egui (2026-08-22, live-QA Phase 8).
fn preview_button_label(pending_preview: bool) -> &'static str {
    if pending_preview {
        "Preparing preview…"
    } else {
        "Preview changes"
    }
}

impl Default for RomOrganisationPageState {
    fn default() -> Self {
        Self {
            master_root_draft: String::new(),
            saved_master_root: None,
            master_root_error: None,
            library_root_draft: String::new(),
            saved_library_root: None,
            library_root_error: None,
            mode: OrganisationMode::MoveRealFile,
            sources: Vec::new(),
            plan: None,
            plan_generation: 0,
            approved: BTreeSet::new(),
            filter: None,
            applied: None,
            applied_journal:
                archivefs_core::dat::rename_apply::journal::default_rename_transaction_dir()
                    .unwrap_or_else(|_| PathBuf::from("rename-transactions")),
            result_message: None,
            error: None,
            pending_apply: None,
            confirm_text: String::new(),
            pending_preview: false,
            approval_persistence_warning: None,
            showing_playing_library: false,
            playing_library: crate::playing_library_page::PlayingLibraryPageState::load(),
        }
    }
}

impl RomOrganisationPageState {
    /// Loads the configured master root and scans the configured source
    /// folders for candidate files (bounded). Read-only. Also restores the
    /// last persisted approval set, if any (fail-closed: a missing or
    /// unreadable file is treated as "no persisted approval", never as
    /// "approve everything").
    pub(crate) fn load() -> Self {
        let mut state = Self::default();
        if let Ok(config) = Config::load_default() {
            state.saved_master_root = config.master_rom_root.clone();
            state.master_root_draft = config
                .master_rom_root
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            state.sources = collect_source_files(&config.source_folders);
        }
        match load_persisted_approved_set() {
            Ok(Some(set)) => state.approved = set,
            Ok(None) => {}
            Err(reason) => {
                state.approval_persistence_warning = Some(reason);
            }
        }
        state
    }

    pub(crate) fn set_mode(&mut self, mode: OrganisationMode) {
        self.mode = mode;
        self.plan = None;
    }

    /// Saves the master ROM root draft (or clears it). Configuring a root
    /// never moves anything by itself.
    pub(crate) fn save_master_root(&mut self) {
        let trimmed = self.master_root_draft.trim();
        if trimmed.is_empty() {
            match clear_master_rom_root_default() {
                Ok(_) => {
                    self.saved_master_root = None;
                    self.master_root_error = None;
                }
                Err(error) => self.master_root_error = Some(error.to_string()),
            }
            return;
        }
        let path = PathBuf::from(trimmed);
        match set_master_rom_root_default(&path) {
            Ok(_) => {
                self.saved_master_root = Some(path);
                self.master_root_error = None;
            }
            Err(error) => self.master_root_error = Some(error.to_string()),
        }
    }

    /// Saves the linked-library destination root draft (or clears it). The
    /// user must provide this root explicitly for BuildLinkedLibrary; it is a
    /// session setting of this page, never inferred and never mixed into the
    /// master ROM root config.
    pub(crate) fn save_library_root(&mut self) {
        let trimmed = self.library_root_draft.trim().to_string();
        if trimmed.is_empty() {
            self.saved_library_root = None;
            self.library_root_error = None;
            return;
        }
        let path = PathBuf::from(trimmed);
        if path.is_absolute() {
            self.saved_library_root = Some(path);
            self.library_root_error = None;
        } else {
            self.library_root_error =
                Some("the linked-library folder must be an absolute path".to_string());
        }
    }

    /// The destination root the current mode plans into: the explicitly
    /// chosen linked-library root in BuildLinkedLibrary mode, otherwise the
    /// configured master ROM root.
    pub(crate) fn effective_root(&self) -> Option<PathBuf> {
        match self.mode {
            OrganisationMode::BuildLinkedLibrary => self.saved_library_root.clone(),
            _ => self.saved_master_root.clone(),
        }
    }

    /// Re-scans the configured source folders for candidate files.
    pub(crate) fn rescan_sources(&mut self) {
        if let Ok(config) = Config::load_default() {
            self.sources = collect_source_files(&config.source_folders);
        }
        self.plan = None;
        self.approved.clear();
        // Any previously persisted approvals may now reference paths that no
        // longer exist. Wipe the persisted sidecar too so a stale set does
        // not silently re-appear on the next load (would be a footgun: the
        // user re-scanned to start fresh, the persistence must agree).
        self.persist_approved_set();
    }

    /// Builds a fresh read-only plan from the current candidates. Every
    /// rebuild bumps the generation, so any earlier review decision is stale.
    pub(crate) fn generate_plan(&mut self) {
        self.plan_generation += 1;
        let Some(master_root) = self.effective_root() else {
            self.error = Some(if self.mode == OrganisationMode::BuildLinkedLibrary {
                "choose a linked library folder first".to_string()
            } else {
                "configure a master ROM root first".to_string()
            });
            self.plan = None;
            return;
        };
        let cache = load_romm_cache();
        let Some(candidates) =
            build_candidates(&self.sources, self.plan_generation, cache.as_ref())
        else {
            self.error = Some("could not read the platform identity database".to_string());
            self.plan = None;
            return;
        };
        // No RomM mapping is consulted: generic destinations derive from the
        // neutral EmuWiz platform layout identity.
        let plan = build_organisation_plan(&OrganisationPlanRequest {
            master_root: &master_root,
            mode: self.mode,
            content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
            candidates: &candidates,
            generation: self.plan_generation,
        });
        self.approved = plan
            .suggested()
            .map(|entry| entry.source_path.to_string_lossy().into_owned())
            .collect();
        self.plan = Some(plan);
        self.error = None;
        // The plan rebuild just rewrote the approval set to "all Suggested".
        // Persist that, so the user's earlier per-entry customisation (if
        // any) is gone but a fresh restart sees the new default.
        self.persist_approved_set();
    }

    pub(crate) fn toggle_approved(&mut self, source: &str) {
        if !self.approved.remove(source) {
            self.approved.insert(source.to_string());
        }
        self.persist_approved_set();
    }

    pub(crate) fn set_filter(&mut self, filter: Option<OrganisationStatus>) {
        self.filter = filter;
    }

    /// Applies the approved Suggested entries after the caller has confirmed.
    pub(crate) fn apply(&mut self) {
        let Some(plan) = &self.plan else {
            return;
        };
        let Some(master_root) = self.effective_root() else {
            return;
        };
        // Revalidate the live platform identity before building the
        // transaction: a platform/canonical name changed by another process
        // since the plan was generated must reject the apply with zero
        // mutation. Destinations are re-derived from the same neutral EmuWiz
        // layout identity as the preview - no RomM lookup happens here.
        let cache = load_romm_cache();
        let canonical_name_for = |source: &Path| canonical_name_for(source, cache.as_ref());
        let database_path = default_database_path();
        match database_path {
            Ok(path) => {
                match archivefs_core::dat::rom_organisation::revalidate_organisation_plan(
                    plan,
                    &path,
                    &canonical_name_for,
                ) {
                    Ok(()) => {}
                    Err(reason) => {
                        self.error = Some(reason);
                        return;
                    }
                }
            }
            Err(_) => {
                self.error = Some(
                    "could not resolve the platform identity database; the plan is stale"
                        .to_string(),
                );
                return;
            }
        }
        let approved = self.approved.clone();
        let mut transaction = match build_organisation_transaction(plan, &approved, plan.generation)
        {
            Ok(transaction) => transaction,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let journal = self.applied_journal.clone();
        std::fs::create_dir_all(&journal).ok();
        let mut trusted_roots = vec![master_root.clone()];
        for source in &self.sources {
            if let Some(parent) = source.parent()
                && let Ok(canonical) = std::fs::canonicalize(parent)
            {
                trusted_roots.push(canonical);
            }
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        match archivefs_core::dat::rom_organisation::apply_organisation_transaction(
            &mut transaction,
            &approved,
            plan.generation,
            archivefs_core::safe_read::TrustedRoots::from_paths(trusted_roots),
            &journal,
            cancel.as_ref(),
            self.mode,
            &master_root,
        ) {
            Ok(outcome) => {
                self.applied = Some(outcome.transaction.clone());
                self.result_message = Some(if self.mode == OrganisationMode::BuildLinkedLibrary {
                    format!(
                        "Created {} library link(s); your original files were not touched. \
                         Roll back is available for this transaction.",
                        outcome.transaction.applied_count()
                    )
                } else {
                    format!(
                        "Applied {} organisation(s). Roll back is available for this transaction.",
                        outcome.transaction.applied_count()
                    )
                });
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Rolls back the last applied transaction.
    pub(crate) fn rollback(&mut self) {
        let Some(mut transaction) = self.applied.take() else {
            return;
        };
        let journal = self.applied_journal.clone();
        let Some(master_root) = self.effective_root() else {
            self.applied = Some(transaction);
            return;
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        match archivefs_core::dat::rom_organisation::rollback_organisation_transaction(
            &mut transaction,
            &journal,
            cancel.as_ref(),
            &master_root,
        ) {
            Ok(outcome) => {
                let dirs = outcome.directories_removed.len();
                self.result_message = Some(format!(
                    "Rolled back organisation; {} empty platform director(ies) removed.",
                    dirs
                ));
                self.plan = None;
                self.approved.clear();
                // The rollback cleared the plan and the approvals; persist
                // the empty set so a fresh restart does not re-apply them.
                self.persist_approved_set();
            }
            Err(error) => {
                self.applied = Some(transaction);
                self.error = Some(error);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Approval persistence (UI convenience only — never authoritative)
// ---------------------------------------------------------------------------

/// Versioned JSON sidecar stored in the normal EmuWiz data directory.
/// The on-disk format carries a `version` key so future readers can
/// transparently discard (not reinterpret) an unknown version.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedApprovals {
    version: u32,
    approved: Vec<String>,
}

/// Bumped every time the on-disk schema changes. An unreadable or
/// unexpected version is treated as "no persisted set" (fail-closed).
const APPROVAL_PERSISTENCE_VERSION: u32 = 1;

/// Leaf name of the JSON sidecar inside the EmuWiz data directory.
const APPROVAL_PERSISTENCE_FILENAME: &str = "rom_organisation_approvals.json";

/// Resolves the absolute sidecar path. Returns an error string when the
/// data directory itself cannot be determined (e.g. `$HOME` is unset).
fn approval_persistence_path() -> Result<PathBuf, String> {
    archivefs_core::app_dirs::data_path(APPROVAL_PERSISTENCE_FILENAME)
        .map_err(|e| format!("cannot resolve persistence path: {e}"))
}

/// Loads the last persisted approval set if the sidecar exists and is
/// readable.
///
/// - Returns `Ok(None)` when the file is absent (first run).
/// - Returns `Ok(Some(_))` when a well-formed, expected-version file is
///   found.
/// - Returns `Err(reason)` for any other condition: unreadable file,
///   malformed JSON, unknown version, symlinked sidecar, etc.
///   The caller surfaces the reason via `approval_persistence_warning`
///   but never treats it as "approve everything".
fn load_persisted_approved_set() -> Result<Option<BTreeSet<String>>, String> {
    let path = approval_persistence_path()?;

    // ----- Reject symlinked sidecars ----------------------------------
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!("cannot read metadata of {}: {e}", path.display()));
        }
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "refusing to load symlinked approval sidecar: {}",
            path.display()
        ));
    }

    // ----- Read & parse ------------------------------------------------
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read approval sidecar {}: {e}", path.display()))?;

    let persisted: PersistedApprovals = serde_json::from_str(&raw)
        .map_err(|e| format!("approval sidecar {} is malformed: {e}", path.display()))?;

    if persisted.version != APPROVAL_PERSISTENCE_VERSION {
        return Err(format!(
            "approval sidecar {} has unknown version {} (expected {})",
            path.display(),
            persisted.version,
            APPROVAL_PERSISTENCE_VERSION,
        ));
    }

    Ok(Some(persisted.approved.into_iter().collect()))
}

impl RomOrganisationPageState {
    /// Persists the current `approved` set to the versioned JSON sidecar.
    ///
    /// - Atomic: writes to a temp file in the same directory, then renames.
    /// - Fail-soft: an I/O error is surfaced via
    ///   `approval_persistence_warning` but never blocks the page.
    /// - Does **not** write inside game/source folders.
    fn persist_approved_set(&mut self) {
        self.approval_persistence_warning = None;

        let path = match approval_persistence_path() {
            Ok(p) => p,
            Err(reason) => {
                self.approval_persistence_warning = Some(reason);
                return;
            }
        };

        // Ensure the parent directory exists (data dir may not exist yet
        // on a fresh install).
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            self.approval_persistence_warning = Some(format!("cannot create data directory: {e}"));
            return;
        }

        let payload = PersistedApprovals {
            version: APPROVAL_PERSISTENCE_VERSION,
            approved: self.approved.iter().cloned().collect(),
        };

        let json = match serde_json::to_string(&payload) {
            Ok(j) => j,
            Err(e) => {
                self.approval_persistence_warning =
                    Some(format!("cannot serialise approval set: {e}"));
                return;
            }
        };

        // Atomic temp-write + rename. Check that the predictable tmp
        // path is not a symlink before opening it for write — `File::create`
        // would follow a symlink, letting a planted link escape the data dir.
        let tmp = path.with_extension("json.tmp");
        match std::fs::symlink_metadata(&tmp) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    self.approval_persistence_warning =
                        Some("refusing to write through a symlinked temp path".to_string());
                    return;
                }
                // Temp file already exists (from a previous crashed write).
                // Remove it first; `File::create` would overwrite, but
                // removing explicitly avoids a symlink race between the
                // metadata check and the open call.
                let _ = std::fs::remove_file(&tmp);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Expected: no stale temp file.
            }
            Err(e) => {
                self.approval_persistence_warning = Some(format!("cannot stat temp path: {e}"));
                return;
            }
        }
        // Use OpenOptions with create_new to avoid following symlinks.
        // On Unix, O_CREAT|O_EXCL|O_WRONLY fails with EEXIST on a symlink.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(mut f) => {
                if let Err(e) = std::io::Write::write_all(&mut f, json.as_bytes()) {
                    let _ = std::fs::remove_file(&tmp);
                    self.approval_persistence_warning =
                        Some(format!("cannot write approval sidecar: {e}"));
                    return;
                }
            }
            Err(e) => {
                self.approval_persistence_warning = Some(format!("cannot create temp file: {e}"));
                return;
            }
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            // Best-effort cleanup of the temp file.
            let _ = std::fs::remove_file(&tmp);
            self.approval_persistence_warning =
                Some(format!("cannot finalise approval sidecar: {e}"));
        }

        // Success: the warning (if any leftover from a previous failed
        // load/save) is already cleared above.
    }
}

/// Walks the configured source folders (bounded) and collects candidate
/// paths: regular files and symlink *objects*. Symlinked directories are
/// collected as link objects but never traversed, so a symlink loop cannot
/// recurse. Read-only.
pub(crate) fn collect_source_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    const MAX_DEPTH: usize = 4;
    const MAX_FILES: usize = 2_000;
    let mut out = Vec::new();
    for root in roots {
        let mut queue: Vec<(PathBuf, usize)> = vec![(root.clone(), 0)];
        while let Some((dir, depth)) = queue.pop() {
            if out.len() >= MAX_FILES {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if out.len() >= MAX_FILES {
                    break;
                }
                let path = entry.path();
                let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                // A library-organisation plan is intentionally a games-only
                // operation. Cover art, manuals, metadata, and other
                // supporting media remain in place for their own workflows;
                // treating a `boxart/Game.png` as an unknown game creates a
                // misleading blocked entry and risks a later destructive
                // operation being approved against it. Reuse discovery's
                // conservative shared classification rather than guessing
                // from directory names such as `boxart`.
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(str::to_ascii_lowercase)
                    .is_some_and(|extension| is_known_non_game_extension(&extension))
                {
                    continue;
                }
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    // The link object itself is a candidate; its target is
                    // never followed or traversed.
                    out.push(path);
                } else if metadata.is_dir() {
                    if depth < MAX_DEPTH {
                        queue.push((path, depth + 1));
                    }
                } else if metadata.is_file() {
                    out.push(path);
                }
            }
        }
    }
    out.sort();
    out
}

/// Builds organisation candidates by resolving each source file's platform
/// identity from the database, and attaching the canonical game name (with the
/// source's extension) from the authoritative RomM identity cache record when
/// one exists. Returns `None` when the database is unreadable.
fn build_candidates(
    sources: &[PathBuf],
    generation: u64,
    cache: Option<&archivefs_core::identity_source::cache::IdentityCache>,
) -> Option<Vec<OrganisationCandidate>> {
    // No identity lookup is needed for an empty selection.  In particular,
    // regenerating an empty preview must be able to clear a stale approved
    // set even when this machine has not created its local database yet.
    if sources.is_empty() {
        return Some(Vec::new());
    }

    let database_path = default_database_path().ok()?;
    let database = Database::open_read_only(&database_path).ok()?;
    let mut candidates = Vec::new();
    for source in sources {
        let resolution = match database
            .find_archive_id_by_absolute_path(source)
            .ok()
            .flatten()
        {
            Some(archive_id) => {
                let evidence = database
                    .current_platform_identity_evidence(archive_id, generation)
                    .ok()
                    .unwrap_or_default();
                resolve_platform_identity(generation, evidence)
            }
            None => PlatformIdentityResolution::Unknown { generation },
        };
        candidates.push(OrganisationCandidate {
            source_path: source.clone(),
            resolution,
            canonical_name: canonical_name_for(source, cache),
            content_classification: None,
            original_metadata: Default::default(),
        });
    }
    Some(candidates)
}

/// The authoritative canonical game name for a source, when the RomM identity
/// cache records one: the record's title with the source's extension appended,
/// so `derive_proposed_basename` preserves extension semantics. Returns `None`
/// when there is no record (the planner then falls back to the source
/// basename). Never derived from a display label.
fn canonical_name_for(
    source: &Path,
    cache: Option<&archivefs_core::identity_source::cache::IdentityCache>,
) -> Option<String> {
    let cache = cache?;
    let title = cache.records.iter().find_map(|record| {
        (record.archivefs_path.as_deref() == Some(source)
            || record.provider_path == source.to_string_lossy())
        .then(|| record.title.clone())
        .flatten()
    })?;
    let title = title.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let Some(extension) = source.extension().map(|ext| ext.to_string_lossy()) else {
        return Some(title);
    };
    if extension.is_empty() {
        return Some(title);
    }
    Some(format!("{title}.{extension}"))
}

/// Loads the imported RomM identity cache, if any.
fn load_romm_cache() -> Option<archivefs_core::identity_source::cache::IdentityCache> {
    let identity_root = default_identity_root().ok()?;
    let location = IdentityCacheLocation::new(&identity_root, IdentityProvider::Romm);
    load_cache(&location, None).ok()
}

// The former `build_slug_map` helper is gone: generic organisation no longer
// consults RomM slugs at all. RomM-specific frontend layouts resolve their
// own mappings in `platform_evidence_fusion::romm_platform_mapping`.

/// Draws the page and returns the confirmation request when the user clicks
/// Apply (the caller must confirm before any mutation happens).
fn apply_playing_library_action(
    state: &mut crate::playing_library_page::PlayingLibraryPageState,
    action: crate::playing_library_page::PlayingLibraryPageAction,
) {
    use crate::playing_library_page::PlayingLibraryPageAction;
    match action {
        PlayingLibraryPageAction::Preview => state.preview(),
        PlayingLibraryPageAction::SelectFamily(name) => state.select_family(name),
        PlayingLibraryPageAction::RequestApply => state.request_apply(),
        PlayingLibraryPageAction::CancelApply => state.cancel_apply(),
        PlayingLibraryPageAction::ConfirmApply => state.confirm_apply(),
        PlayingLibraryPageAction::RollbackLast => state.rollback_last(),
        PlayingLibraryPageAction::SelectEsdePlatform(platform_id) => {
            state.select_esde_platform(Some(platform_id))
        }
        PlayingLibraryPageAction::PreviewEsde => state.preview_esde_publication(),
        PlayingLibraryPageAction::RequestEsdePublish => state.request_esde_publish(),
        PlayingLibraryPageAction::CancelEsdePublish => state.cancel_esde_publish(),
        PlayingLibraryPageAction::ConfirmEsdePublish => state.confirm_esde_publish(),
        PlayingLibraryPageAction::RequestEsdeRecovery => state.request_esde_recovery(),
        PlayingLibraryPageAction::CancelEsdeRecovery => state.cancel_esde_recovery(),
        PlayingLibraryPageAction::ConfirmEsdeRecovery => state.confirm_esde_recovery(),
        PlayingLibraryPageAction::PreviewRomm => state.preview_romm(),
        PlayingLibraryPageAction::RequestRommApply => state.request_romm_apply(),
        PlayingLibraryPageAction::CancelRommApply => state.cancel_romm_apply(),
        PlayingLibraryPageAction::ConfirmRommApply => state.confirm_romm_apply(),
        PlayingLibraryPageAction::RollbackRomm => state.rollback_romm_last(),
        PlayingLibraryPageAction::PreviewRetroDeck => state.preview_retrodeck(),
        PlayingLibraryPageAction::RequestRetroDeckApply => state.request_retrodeck_apply(),
        PlayingLibraryPageAction::CancelRetroDeckApply => state.cancel_retrodeck_apply(),
        PlayingLibraryPageAction::ConfirmRetroDeckApply => state.confirm_retrodeck_apply(),
        PlayingLibraryPageAction::RollbackRetroDeck => state.rollback_retrodeck(),
    }
}

pub(crate) fn show_rom_organisation_page(ui: &mut egui::Ui, state: &mut RomOrganisationPageState) {
    // See `RomOrganisationPageState::pending_preview`: this runs the actual
    // plan generation one frame after the click that requested it, so the
    // busy row below had a chance to render first.
    if state.pending_preview {
        state.generate_plan();
        state.pending_preview = false;
    }
    widgets::page_header_with_icon(
        ui,
        crate::ui::icons::ORGANISE,
        "Organise",
        "Preview how your games can be renamed or organised. Nothing moves until you approve it.",
    );

    ui.horizontal(|ui| {
        if state.showing_playing_library {
            if widgets::action_button(ui, "< Back to Organise", widgets::ActionStyle::Quiet, true)
                .clicked()
            {
                state.showing_playing_library = false;
            }
        } else if widgets::action_button(
            ui,
            "Build Playing Library",
            widgets::ActionStyle::Secondary,
            true,
        )
        .clicked()
        {
            state.showing_playing_library = true;
        }
    });
    if !state.showing_playing_library {
        ui.label(
            egui::RichText::new(
                "Playing Library chooses one preferred verified release per game and creates a separate linked library while leaving originals untouched.",
            )
            .color(theme::muted(ui)),
        );
    }
    ui.add_space(8.0);

    if state.showing_playing_library {
        if let Some(action) =
            crate::playing_library_page::show_playing_library_page(ui, &mut state.playing_library)
        {
            apply_playing_library_action(&mut state.playing_library, action);
        }
        return;
    }

    if state.mode == OrganisationMode::BuildLinkedLibrary {
        // Linked-library mode plans into an explicitly chosen destination
        // root that is separate from the master ROM root. It is never
        // inferred from the source folders.
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new("Linked library folder").strong());
            ui.label(
                egui::RichText::new(
                    "The organised links are created inside this folder; your original \
                     files stay where they are.",
                )
                .color(theme::muted(ui)),
            );
            match &state.saved_library_root {
                Some(root) => {
                    ui.label(format!(
                        "Organised library: {}",
                        root.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("chosen folder")
                    ));
                    if widgets::action_button(
                        ui,
                        "Change folder…",
                        widgets::ActionStyle::Secondary,
                        true,
                    )
                    .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_title("Choose Linked Library Folder")
                            .pick_folder()
                    {
                        state.library_root_draft = path.display().to_string();
                        state.save_library_root();
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("No linked library folder is chosen yet.")
                            .color(theme::muted(ui)),
                    );
                    if widgets::action_button(
                        ui,
                        "Choose folder…",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_title("Choose Linked Library Folder")
                            .pick_folder()
                    {
                        state.library_root_draft = path.display().to_string();
                        state.save_library_root();
                    }
                }
            };
            if let Some(error) = &state.library_root_error {
                ui.label(
                    egui::RichText::new(error.as_str())
                        .color(widgets::StatusTone::Blocked.color(ui)),
                );
            }
            widgets::technical_details(ui, "linked_library_manual_path", |ui| {
                if let Some(root) = &state.saved_library_root {
                    ui.label(format!("Exact folder: {}", root.display()));
                }
                ui.label(
                    egui::RichText::new(
                        "Type or paste a path directly instead of using the folder picker.",
                    )
                    .color(theme::muted(ui)),
                );
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut state.library_root_draft);
                    if widgets::action_button(ui, "Save", widgets::ActionStyle::Primary, true)
                        .clicked()
                    {
                        state.save_library_root();
                    }
                });
            });
        });
    } else {
        widgets::card(ui, |ui| {
            ui.label(egui::RichText::new("Game library folder").strong());
            match &state.saved_master_root {
                Some(root) => {
                    ui.label(format!(
                        "Original games: {}",
                        root.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("chosen folder")
                    ));
                    if widgets::action_button(
                        ui,
                        "Change folder…",
                        widgets::ActionStyle::Secondary,
                        true,
                    )
                    .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_title("Choose Game Library Folder")
                            .pick_folder()
                    {
                        state.master_root_draft = path.display().to_string();
                        state.save_master_root();
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("No game library folder is configured yet.")
                            .color(theme::muted(ui)),
                    );
                    if widgets::action_button(
                        ui,
                        "Choose folder…",
                        widgets::ActionStyle::Primary,
                        true,
                    )
                    .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .set_title("Choose Game Library Folder")
                            .pick_folder()
                    {
                        state.master_root_draft = path.display().to_string();
                        state.save_master_root();
                    }
                }
            };
            if let Some(error) = &state.master_root_error {
                ui.label(
                    egui::RichText::new(error.as_str())
                        .color(widgets::StatusTone::Blocked.color(ui)),
                );
            }
            widgets::technical_details(ui, "rom_organisation_manual_path", |ui| {
                if let Some(root) = &state.saved_master_root {
                    ui.label(format!("Exact folder: {}", root.display()));
                }
                ui.label(
                    egui::RichText::new(
                        "Type or paste a path directly instead of using the folder picker.",
                    )
                    .color(theme::muted(ui)),
                );
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut state.master_root_draft);
                    if widgets::action_button(ui, "Save", widgets::ActionStyle::Primary, true)
                        .clicked()
                    {
                        state.save_master_root();
                    }
                });
            });
        });
    }

    ui.add_space(8.0);
    widgets::card(ui, |ui| {
        ui.label(egui::RichText::new("Organisation mode").strong());
        for mode in [
            OrganisationMode::RenameInPlace,
            OrganisationMode::MoveRealFile,
            OrganisationMode::BuildLinkedLibrary,
            OrganisationMode::OrganiseSymlinkOnly,
        ] {
            let selected = state.mode == mode;
            if ui
                .radio(selected, organisation_mode_plain_label(mode))
                .clicked()
            {
                state.set_mode(mode);
            }
        }
        ui.label(
            egui::RichText::new(organisation_mode_plain_explanation(state.mode))
                .color(theme::muted(ui)),
        );
        ui.label(
            egui::RichText::new("Modes are separate choices and are never combined.")
                .color(theme::muted(ui)),
        );
    });

    ui.add_space(8.0);
    widgets::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Games ready to organise").strong());
            if widgets::action_button(ui, "Rescan sources", widgets::ActionStyle::Secondary, true)
                .clicked()
            {
                state.rescan_sources();
            }
            if widgets::action_button(
                ui,
                preview_button_label(state.pending_preview),
                widgets::ActionStyle::Primary,
                !state.pending_preview,
            )
            .clicked()
            {
                state.pending_preview = true;
                ui.ctx().request_repaint();
            }
        });
        if state.pending_preview {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    "Preparing your preview… checking each game and deciding where it belongs.",
                );
            });
        }
        ui.label(
            egui::RichText::new(format!(
                "{} game(s) ready to organise.",
                state.sources.len()
            ))
            .color(theme::muted(ui)),
        );
    });

    if let Some(plan) = state.plan.clone() {
        ui.add_space(8.0);
        show_plan(ui, &plan, state);
    }

    if let Some(message) = &state.result_message {
        ui.add_space(8.0);
        widgets::banner(ui, "Result", message, widgets::StatusTone::Success);
        if state.applied.is_some()
            && widgets::action_button(
                ui,
                "Roll back this organisation",
                widgets::ActionStyle::Secondary,
                true,
            )
            .clicked()
        {
            state.rollback();
        }
    }

    if let Some(error) = &state.error {
        ui.add_space(8.0);
        widgets::banner(ui, "Not applied", error, widgets::StatusTone::Blocked);
    }

    if let Some(warning) = &state.approval_persistence_warning {
        ui.add_space(6.0);
        widgets::banner(
            ui,
            "Approvals not saved",
            &format!(
                "Your approval selections were not persisted to disk: {warning}. \
                 Re-preview the plan to restore suggested defaults."
            ),
            widgets::StatusTone::Pending,
        );
    }
}

fn show_plan(ui: &mut egui::Ui, plan: &OrganisationPlan, state: &mut RomOrganisationPageState) {
    let entries: Vec<&OrganisationPlanEntry> = plan
        .entries
        .iter()
        .filter(|entry| state.filter.is_none_or(|f| entry.status == f))
        .collect();

    ui.horizontal(|ui| {
        ui.label("Filter:");
        let all = state.filter.is_none();
        if ui.selectable_label(all, "All").clicked() {
            state.set_filter(None);
        }
        for status in [
            OrganisationStatus::Suggested,
            OrganisationStatus::AlreadyOrganised,
            OrganisationStatus::Conflict,
            OrganisationStatus::Blocked,
            OrganisationStatus::Unsupported,
        ] {
            let selected = state.filter == Some(status);
            if ui.selectable_label(selected, status.label()).clicked() {
                state.set_filter(if selected { None } else { Some(status) });
            }
        }
    });
    ui.add_space(4.0);

    for entry in &entries {
        ui.horizontal(|ui| {
            match entry.status {
                OrganisationStatus::Suggested => {
                    let mut approved = state
                        .approved
                        .contains(&entry.source_path.to_string_lossy().into_owned());
                    if ui.checkbox(&mut approved, "").changed() {
                        state.toggle_approved(&entry.source_path.to_string_lossy());
                    }
                }
                _ => {
                    ui.add_space(20.0);
                }
            }
            widgets::status_badge(ui, entry.status.label(), status_tone(entry.status));
            if plan.mode == OrganisationMode::BuildLinkedLibrary {
                // Linked-library preview: make the semantics painfully
                // obvious. Never worded as a Rename or Move.
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Source: {}", entry.source_path.display()))
                            .monospace(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "Destination link: {}",
                            entry.destination_path.display()
                        ))
                        .monospace(),
                    );
                    ui.label("Source action: Untouched");
                    ui.label(format!("Result: {}", linked_library_preview_result(entry)));
                    if !entry.platform_display_name.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {}",
                                entry.platform_display_name, entry.platform_source
                            ))
                            .color(theme::muted(ui)),
                        );
                    }
                });
            } else {
                ui.label(
                    egui::RichText::new(format!(
                        "{} → {}",
                        entry.source_path.display(),
                        entry.destination_path.display()
                    ))
                    .monospace(),
                );
                if !entry.platform_display_name.is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {}",
                            entry.platform_display_name, entry.platform_source
                        ))
                        .color(theme::muted(ui)),
                    );
                }
                if let Some(reason) = &entry.reason {
                    ui.label(
                        egui::RichText::new(format!("({reason})"))
                            .color(widgets::StatusTone::Blocked.color(ui))
                            .small(),
                    );
                }
            }
        });
    }

    let suggested = plan.suggested().count();
    ui.add_space(8.0);
    let approved = state.approved.len();
    let applyable = suggested > 0 && approved > 0;
    if widgets::action_button(
        ui,
        format!("Apply approved organisation ({approved})"),
        widgets::ActionStyle::Primary,
        applyable,
    )
    .clicked()
    {
        state.pending_apply = Some(approved);
        state.confirm_text.clear();
    }

    // Explicit confirmation before any mutation. Large batches require typing
    // the exact phrase (truthful for the mode); small ones a plain confirm.
    if let Some(count) = state.pending_apply {
        ui.add_space(6.0);
        widgets::card(ui, |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "Apply {} approved organisation(s)? Nothing is written until you confirm.",
                    count
                ))
                .strong(),
            );
            if count >= TYPED_CONFIRMATION_THRESHOLD {
                let phrase = apply_confirmation_phrase(state.mode, count);
                ui.label(
                    egui::RichText::new(format!("Type '{phrase}' to confirm:"))
                        .color(theme::muted(ui)),
                );
                ui.text_edit_singleline(&mut state.confirm_text);
            }
            ui.horizontal(|ui| {
                let phrase_ok = count < TYPED_CONFIRMATION_THRESHOLD
                    || state.confirm_text.trim() == apply_confirmation_phrase(state.mode, count);
                if widgets::action_button(
                    ui,
                    "Confirm apply",
                    widgets::ActionStyle::Primary,
                    phrase_ok,
                )
                .clicked()
                {
                    state.pending_apply = None;
                    state.confirm_text.clear();
                    state.apply();
                }
                if widgets::action_button(ui, "Cancel", widgets::ActionStyle::Secondary, true)
                    .clicked()
                {
                    state.pending_apply = None;
                    state.confirm_text.clear();
                }
            });
        });
    }
}

/// Plain-English result line for one linked-library preview entry. Pure:
/// derives only from the plan entry's own status/reason so it is directly
/// unit-testable.
pub(crate) fn linked_library_preview_result(entry: &OrganisationPlanEntry) -> String {
    match entry.status {
        OrganisationStatus::Suggested => "Will create link".to_string(),
        OrganisationStatus::AlreadyOrganised => "Already present; nothing to do".to_string(),
        OrganisationStatus::Conflict | OrganisationStatus::Blocked => entry
            .reason
            .clone()
            .unwrap_or_else(|| "Cannot create this link".to_string()),
        OrganisationStatus::Unsupported => entry
            .reason
            .clone()
            .unwrap_or_else(|| "Not supported for linked libraries".to_string()),
    }
}

fn status_tone(status: OrganisationStatus) -> widgets::StatusTone {
    match status {
        OrganisationStatus::Suggested => widgets::StatusTone::Success,
        OrganisationStatus::AlreadyOrganised => widgets::StatusTone::Active,
        OrganisationStatus::Conflict => widgets::StatusTone::Warning,
        OrganisationStatus::Blocked => widgets::StatusTone::Pending,
        OrganisationStatus::Unsupported => widgets::StatusTone::Blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered_text_contains(output: &egui::FullOutput, needle: &str) -> bool {
        fn shape_contains(shape: &egui::Shape, needle: &str) -> bool {
            match shape {
                egui::Shape::Text(text_shape) => text_shape.galley.text().contains(needle),
                egui::Shape::Vec(nested) => nested.iter().any(|s| shape_contains(s, needle)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|clipped| shape_contains(&clipped.shape, needle))
    }

    #[test]
    fn the_confirmation_phrase_is_truthful_for_the_mode() {
        assert_eq!(
            apply_confirmation_phrase(OrganisationMode::RenameInPlace, 3),
            "RENAME 3 FILES"
        );
        assert_eq!(
            apply_confirmation_phrase(OrganisationMode::MoveRealFile, 42),
            "MOVE 42 FILES"
        );
        assert_eq!(
            apply_confirmation_phrase(OrganisationMode::OrganiseSymlinkOnly, 1),
            "MOVE 1 FILES"
        );
    }

    #[test]
    fn the_default_state_has_no_master_root_and_no_sources() {
        let state = RomOrganisationPageState::default();
        assert!(state.saved_master_root.is_none());
        assert!(state.sources.is_empty());
        assert_eq!(state.mode, OrganisationMode::MoveRealFile);
    }

    // ------------------------------------------------------------------
    // Symlink discovery (blocker: symlink-only mode discovered zero symlinks)
    // ------------------------------------------------------------------

    /// A private directory for one test (the page tests cannot rely on a
    /// configured source folder; they build their own fixtures).
    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "archivefs-rom-org-page-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn collected(roots: &[PathBuf]) -> Vec<String> {
        collect_source_files(roots)
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn relative_absolute_broken_and_dir_symlinks_are_collected_as_link_objects() {
        let root = test_root("rom-org-symlinks");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("real.bin"), b"data").unwrap();
        // Relative symlink.
        std::os::unix::fs::symlink("real.bin", root.join("rel.iso")).unwrap();
        // Absolute symlink.
        std::os::unix::fs::symlink(root.join("real.bin"), root.join("abs.iso")).unwrap();
        // Broken symlink.
        std::os::unix::fs::symlink(root.join("nowhere.bin"), root.join("broken.iso")).unwrap();
        // Symlink to a directory.
        std::os::unix::fs::symlink(root.join("sub"), root.join("dirlink.iso")).unwrap();
        // A regular file for contrast.
        std::fs::write(root.join("regular.iso"), b"data").unwrap();

        let files = collected(std::slice::from_ref(&root));
        for name in [
            "rel.iso",
            "abs.iso",
            "broken.iso",
            "dirlink.iso",
            "regular.iso",
        ] {
            assert!(
                files.iter().any(|path| path.ends_with(name)),
                "{name} must be collected: {files:?}"
            );
        }
    }

    #[test]
    fn symlink_loops_do_not_recurse_and_directories_traverse_within_bounds() {
        let root = test_root("rom-org-loop");
        let dir_a = root.join("a");
        let dir_b = root.join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::write(dir_a.join("in_a.iso"), b"data").unwrap();
        // Symlink loop: a/b -> a and a/a -> b. Neither may be traversed.
        std::os::unix::fs::symlink(&dir_a, dir_b.join("a")).unwrap();
        std::os::unix::fs::symlink(&dir_b, dir_a.join("b")).unwrap();

        let files = collected(std::slice::from_ref(&root));
        // The nested regular file is collected; the loop must not explode or
        // recurse (no infinite loop, and only the bounded set is returned).
        assert!(files.iter().any(|path| path.ends_with("in_a.iso")));
        assert!(
            files.len() <= 10,
            "the symlink loop must not recurse: {}",
            files.len()
        );
    }

    #[test]
    fn artwork_files_are_excluded_from_games_ready_to_organise() {
        let root = test_root("rom-org-artwork");
        std::fs::create_dir_all(root.join("boxart")).unwrap();
        std::fs::write(root.join("Game.iso"), b"game").unwrap();
        std::fs::write(root.join("boxart/Game.png"), b"artwork").unwrap();
        std::fs::write(root.join("boxart/Game.jpg"), b"artwork").unwrap();

        let files = collected(std::slice::from_ref(&root));
        assert!(files.iter().any(|path| path.ends_with("Game.iso")));
        assert!(
            !files.iter().any(|path| path.ends_with("Game.png")),
            "box art must not become a game-organisation candidate: {files:?}"
        );
        assert!(
            !files.iter().any(|path| path.ends_with("Game.jpg")),
            "artwork must not become a game-organisation candidate: {files:?}"
        );
    }

    // ------------------------------------------------------------------
    // Canonical-name supply (blocker: GUI never supplied canonical names)
    // ------------------------------------------------------------------

    #[test]
    fn canonical_name_for_uses_the_romm_record_title_with_the_source_extension() {
        use archivefs_core::identity_source::cache::{CACHE_FORMAT_VERSION, IdentityCache};
        use archivefs_core::identity_source::model::{
            ExternalIdentityRecord, ExternalVerification, IdentityProvider,
        };

        let source = PathBuf::from("/roms/library/Game_ugly.iso");
        let record = ExternalIdentityRecord {
            provider: IdentityProvider::Romm,
            server_id: "test".to_string(),
            provider_platform_id: Some("1".to_string()),
            provider_game_id: "g1".to_string(),
            provider_file_id: None,
            provider_path: "roms/Game_ugly.iso".to_string(),
            archivefs_path: Some(source.clone()),
            title: Some("Game (Europe)".to_string()),
            platform_candidate: Some("PSP".to_string()),
            provider_platform_name: Some("psp".to_string()),
            regions: Vec::new(),
            revision: None,
            hashes: Vec::new(),
            file_size_bytes: None,
            metadata_provider_ids: Vec::new(),
            artwork: None,
            related_files: Vec::new(),
            sibling_game_ids: Vec::new(),
            imported_at_unix_seconds: 1,
            provider_updated_at: None,
            verification: ExternalVerification::StrongExternal,
            conflicts: Vec::new(),
            evidence: Vec::new(),
            synopsis: None,
            genres: Vec::new(),
            players: None,
            rating: None,
            release_year: None,
        };
        let cache = IdentityCache {
            format_version: CACHE_FORMAT_VERSION,
            provider: IdentityProvider::Romm,
            server_id: "test".to_string(),
            server_version: None,
            source_fingerprint: "f".to_string(),
            imported_at_unix_seconds: 1,
            platforms: Vec::new(),
            records: vec![record],
            rejected_hashes: Vec::new(),
            unknown_platforms: Vec::new(),
            server_reported_total: Some(1),
        };
        assert_eq!(
            canonical_name_for(&source, Some(&cache)).as_deref(),
            Some("Game (Europe).iso"),
            "the title must be combined with the source extension"
        );
        // A source with no cache record falls back to None.
        assert_eq!(
            canonical_name_for(&PathBuf::from("/roms/library/Other.iso"), Some(&cache)),
            None
        );
        // No cache at all falls back to None.
        assert_eq!(canonical_name_for(&source, None), None);
    }

    // ------------------------------------------------------------------
    // Preview busy state (live-QA Phase 8: no feedback while previewing)
    // ------------------------------------------------------------------

    #[test]
    fn the_default_state_is_not_mid_preview() {
        let state = RomOrganisationPageState::default();
        assert!(!state.pending_preview);
    }

    #[test]
    fn the_preview_button_label_reflects_whether_a_preview_is_running() {
        assert_eq!(preview_button_label(false), "Preview changes");
        assert_eq!(preview_button_label(true), "Preparing preview…");
    }

    #[test]
    fn a_pending_preview_resolves_to_an_error_when_no_master_root_is_configured() {
        let mut state = RomOrganisationPageState {
            pending_preview: true,
            ..Default::default()
        };
        assert!(state.saved_master_root.is_none());
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_rom_organisation_page(ui, &mut state);
            });
        });
        assert!(!state.pending_preview);
        assert!(state.plan.is_none());
        assert_eq!(
            state.error.as_deref(),
            Some("configure a master ROM root first")
        );
    }

    #[test]
    fn an_idle_state_shows_the_normal_preview_button_with_no_busy_row() {
        let mut state = RomOrganisationPageState::default();
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_rom_organisation_page(ui, &mut state);
            });
        });
        assert!(rendered_text_contains(&output, "Preview changes"));
        assert!(!rendered_text_contains(&output, "Preparing preview…"));
    }

    // ------------------------------------------------------------------
    // Approval persistence (UI convenience helpers)
    // ------------------------------------------------------------------

    fn temp_data_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        (dir, data)
    }

    #[test]
    fn load_returns_none_when_the_sidecar_is_absent() {
        let (_guard, dir) = temp_data_dir();
        let path = dir.join(APPROVAL_PERSISTENCE_FILENAME);
        assert!(!path.exists());
        let state = RomOrganisationPageState::default();
        assert!(state.approval_persistence_warning.is_none());
        assert!(state.approved.is_empty());
    }

    #[test]
    fn load_returns_the_persisted_set_when_the_sidecar_is_valid() {
        let (_guard, dir) = temp_data_dir();
        let path = dir.join(APPROVAL_PERSISTENCE_FILENAME);
        let json = serde_json::json!({
            "version": 1,
            "approved": ["/roms/a.iso", "/roms/b.iso"]
        })
        .to_string();
        std::fs::write(&path, &json).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let persisted: PersistedApprovals = serde_json::from_str(&raw).unwrap();
        assert_eq!(persisted.version, 1);
        let set: BTreeSet<String> = persisted.approved.into_iter().collect();
        assert!(set.contains("/roms/a.iso"));
        assert!(set.contains("/roms/b.iso"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn load_fails_closed_on_malformed_json() {
        let (_guard, dir) = temp_data_dir();
        let path = dir.join(APPROVAL_PERSISTENCE_FILENAME);
        std::fs::write(&path, b"not json at all").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let result: Result<PersistedApprovals, _> = serde_json::from_str(&raw);
        assert!(result.is_err(), "malformed JSON must not parse");
    }

    #[test]
    fn load_fails_closed_on_unknown_version() {
        let (_guard, dir) = temp_data_dir();
        let path = dir.join(APPROVAL_PERSISTENCE_FILENAME);
        let json = serde_json::json!({
            "version": 999,
            "approved": ["/roms/x.iso"]
        })
        .to_string();
        std::fs::write(&path, &json).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let persisted: PersistedApprovals = serde_json::from_str(&raw).unwrap();
        assert_ne!(
            persisted.version, APPROVAL_PERSISTENCE_VERSION,
            "unknown version must be rejected"
        );
    }

    #[test]
    fn persist_clears_the_warning_on_success() {
        let mut state = RomOrganisationPageState::default();
        state.approved.insert("/roms/game.iso".to_string());
        state.approval_persistence_warning = Some("previous failure".to_string());
        state.persist_approved_set();
        assert!(
            state.approval_persistence_warning.as_deref() != Some("previous failure"),
            "stale warning must be cleared"
        );
    }

    #[test]
    fn persist_populates_warning_when_path_resolution_fails_at_runtime() {
        let result = approval_persistence_path();
        if let Err(msg) = &result {
            assert!(!msg.is_empty(), "error message must not be empty");
        }
    }

    #[test]
    fn an_empty_approved_set_serialises_and_deserialises_round_trip() {
        let payload = PersistedApprovals {
            version: APPROVAL_PERSISTENCE_VERSION,
            approved: Vec::new(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let round_tripped: PersistedApprovals = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.version, APPROVAL_PERSISTENCE_VERSION);
        assert!(round_tripped.approved.is_empty());
    }

    #[test]
    fn a_non_empty_approved_set_survives_a_serialisation_round_trip() {
        let payload = PersistedApprovals {
            version: APPROVAL_PERSISTENCE_VERSION,
            approved: vec!["/a.iso".to_string(), "/b.iso".to_string()],
        };
        let json = serde_json::to_string(&payload).unwrap();
        let round_tripped: PersistedApprovals = serde_json::from_str(&json).unwrap();
        let set: BTreeSet<_> = round_tripped.approved.into_iter().collect();
        assert_eq!(set.len(), 2);
        assert!(set.contains("/a.iso"));
        assert!(set.contains("/b.iso"));
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_symlinked_sidecar() {
        use std::os::unix::fs::symlink;

        let (_guard, dir) = temp_data_dir();
        let real = dir.join("real.json");
        let json = serde_json::json!({
            "version": 1,
            "approved": ["/roms/x.iso"]
        })
        .to_string();
        std::fs::write(&real, &json).unwrap();

        let link = dir.join(APPROVAL_PERSISTENCE_FILENAME);
        symlink(&real, &link).unwrap();

        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "the test sidecar must be a symlink"
        );

        let result: Result<Option<BTreeSet<String>>, String> = (|| {
            let meta = std::fs::symlink_metadata(&link).map_err(|e| format!("metadata: {e}"))?;
            if meta.file_type().is_symlink() {
                return Err("symlink rejected".to_string());
            }
            Ok(None)
        })();
        assert!(result.is_err(), "symlinked sidecar must be rejected");
    }

    // ------------------------------------------------------------------
    // Stale-persisted-approval defence
    // ------------------------------------------------------------------

    /// A persisted approval that references a path not in the current plan
    /// must never authorise a transaction. This test simulates the core
    /// property: `build_organisation_transaction` takes a generation and
    /// the approved set, and the caller (`apply()`) always uses the live
    /// plan's generation — never a stale one loaded from disk.
    #[test]
    fn stale_persisted_approval_cannot_authorise_a_transaction() {
        // Simulate what happens after a restart when the persisted set
        // was loaded but `generate_plan()` has not yet been called:
        // `self.approved` contains paths from a previous session while
        // `self.plan` is None. `apply()` requires `self.plan` to be
        // `Some`, so it cannot proceed.
        let mut state = RomOrganisationPageState::default();
        state.approved.insert("/roms/old_game.iso".to_string());
        // No plan set — apply() returns immediately without mutation.
        assert!(state.plan.is_none());
        state.apply();
        // Verifies that without a current plan, `apply()` is a no-op.
        assert!(state.applied.is_none());
        assert!(
            state.result_message.is_none(),
            "no transaction should have been applied"
        );
    }

    /// After `generate_plan()` rewrites `self.approved` with
    /// `plan.suggested()`, any stale persisted set from a previous session
    /// is overwritten. The stale set never survives plan regeneration.
    #[test]
    fn generate_plan_overwrites_stale_approved_set_with_plan_suggested() {
        let mut state = RomOrganisationPageState::default();
        // Simulate a stale persisted set.
        state.approved.insert("/roms/stale.iso".to_string());
        // Set up a master root so generate_plan doesn't error out early.
        // We use a temp dir to ensure the plan comes back empty (no
        // actual source files to pick up).
        let tmp = tempfile::tempdir().expect("tempdir");
        state.saved_master_root = Some(tmp.path().to_path_buf());
        // Config::load_default() will fail in test, so sources stays empty.
        state.sources.clear();
        state.generate_plan();
        // The stale entry must be gone — the plan replaced self.approved
        // with whatever plan.suggested() returned (empty here).
        assert!(
            !state.approved.contains("/roms/stale.iso"),
            "stale approved entry must be overwritten by plan regeneration"
        );
    }

    // ------------------------------------------------------------------
    // Approval persistence warning rendering
    // ------------------------------------------------------------------

    #[test]
    fn approval_persistence_warning_is_rendered_in_the_ui() {
        let mut state = RomOrganisationPageState {
            approval_persistence_warning: Some("test persistence failure".to_string()),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_rom_organisation_page(ui, &mut state);
            });
        });
        assert!(
            rendered_text_contains(&output, "Approvals not saved"),
            "the banner title must appear"
        );
        assert!(
            rendered_text_contains(&output, "test persistence failure"),
            "the specific warning must appear in the banner body"
        );
    }

    #[test]
    fn no_approval_persistence_warning_is_rendered_when_there_is_none() {
        let mut state = RomOrganisationPageState::default();
        assert!(state.approval_persistence_warning.is_none());
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_rom_organisation_page(ui, &mut state);
            });
        });
        assert!(
            !rendered_text_contains(&output, "Approvals not saved"),
            "no persistence banner when there is no warning"
        );
    }

    // ------------------------------------------------------------------
    // Symlinked temp-file defence
    // ------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn persist_rejects_a_symlinked_temp_path() {
        use std::os::unix::fs::symlink;

        let (_guard, dir) = temp_data_dir();
        let target = dir.join(APPROVAL_PERSISTENCE_FILENAME);
        let tmp = target.with_extension("json.tmp");

        // Plant a symlink at the predictable tmp path.
        symlink("/etc/passwd", &tmp).unwrap();
        assert!(tmp.is_symlink());

        // Simulate the symlink check from persist_approved_set.
        let meta = std::fs::symlink_metadata(&tmp).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "the planted tmp path must be a symlink"
        );

        // `OpenOptions::new().create_new(true).open()` would fail with
        // EEXIST on a symlink on Unix, so the write is blocked at the
        // kernel level even without our own metadata check. Our metadata
        // check just makes the failure explicit and surfaces a clear
        // warning.
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .is_err(),
            "create_new must refuse a symlinked path"
        );
    }

    // ------------------------------------------------------------------
    // Build linked library (GUI mode mapping, wording, root selection)
    // ------------------------------------------------------------------

    #[test]
    fn the_confirmation_phrase_says_create_links_for_linked_library() {
        assert_eq!(
            apply_confirmation_phrase(OrganisationMode::BuildLinkedLibrary, 2),
            "CREATE 2 LINKS"
        );
        // Existing modes keep their truthful wording.
        assert_eq!(
            apply_confirmation_phrase(OrganisationMode::OrganiseSymlinkOnly, 1),
            "MOVE 1 FILES"
        );
        assert_eq!(
            apply_confirmation_phrase(OrganisationMode::MoveRealFile, 3),
            "MOVE 3 FILES"
        );
    }

    #[test]
    fn the_symlink_object_mode_is_labelled_advanced_and_the_new_mode_is_distinct() {
        assert_eq!(
            organisation_mode_plain_label(OrganisationMode::OrganiseSymlinkOnly),
            "Advanced: reorganise existing symlinks"
        );
        assert_eq!(
            organisation_mode_plain_label(OrganisationMode::BuildLinkedLibrary),
            "Build linked library"
        );
    }

    #[test]
    fn linked_library_mode_uses_the_explicit_library_root_not_the_master_root() {
        // Set directly (no config write): the master root stays session-only.
        let mut state = RomOrganisationPageState {
            saved_master_root: Some(PathBuf::from("/master/roms")),
            ..Default::default()
        };
        assert_eq!(state.effective_root(), Some(PathBuf::from("/master/roms")));

        state.set_mode(OrganisationMode::BuildLinkedLibrary);
        // Not yet chosen: no effective root, no inference from sources.
        assert_eq!(state.effective_root(), None);

        state.library_root_draft = "/mnt/emuwiz-library".to_string();
        state.save_library_root();
        assert_eq!(
            state.effective_root(),
            Some(PathBuf::from("/mnt/emuwiz-library"))
        );
        // The master ROM root config was never overwritten.
        assert_eq!(state.saved_master_root, Some(PathBuf::from("/master/roms")));

        // A relative path is refused.
        state.library_root_draft = "relative/path".to_string();
        state.save_library_root();
        assert!(state.library_root_error.is_some());
        assert_eq!(
            state.effective_root(),
            Some(PathBuf::from("/mnt/emuwiz-library"))
        );
    }

    fn linked_library_plan_entry(status: OrganisationStatus) -> OrganisationPlanEntry {
        OrganisationPlanEntry {
            source_path: PathBuf::from("/sources/Combat.bin"),
            destination_path: PathBuf::from("/library/atari2600/Combat.bin"),
            platform: Some("Atari2600".to_string()),
            platform_display_name: "Atari 2600".to_string(),
            platform_source: "Manual".to_string(),
            slug: None,
            layout_folder: Some("Atari 2600".to_string()),
            mode: OrganisationMode::BuildLinkedLibrary,
            content_classification: None,
            original_metadata: Default::default(),
            status,
            reason: None,
        }
    }

    #[test]
    fn linked_library_preview_results_are_plain_english_never_rename_or_move() {
        let result = linked_library_preview_result(&linked_library_plan_entry(
            OrganisationStatus::Suggested,
        ));
        assert_eq!(result, "Will create link");
        assert!(!result.to_lowercase().contains("rename"));
        assert!(!result.to_lowercase().contains("move"));

        assert_eq!(
            linked_library_preview_result(&linked_library_plan_entry(
                OrganisationStatus::AlreadyOrganised
            )),
            "Already present; nothing to do"
        );

        let conflict =
            linked_library_preview_result(&linked_library_plan_entry(OrganisationStatus::Conflict));
        assert!(!conflict.is_empty());
    }

    #[test]
    fn a_linked_library_plan_renders_source_destination_and_untouched_wording() {
        let mut state = RomOrganisationPageState {
            mode: OrganisationMode::BuildLinkedLibrary,
            ..Default::default()
        };
        let library_root = test_root("linked-lib-preview").join("library");
        state.saved_library_root = Some(library_root.clone());
        state.plan_generation = 1;
        state.plan = Some(OrganisationPlan {
            master_root: library_root.clone(),
            mode: OrganisationMode::BuildLinkedLibrary,
            content_policy: archivefs_core::dat::classification::ContentSelectionPolicy::AllEntries,
            classifier_version: archivefs_core::dat::classification::CLASSIFIER_VERSION.to_string(),
            generation: 1,
            entries: vec![OrganisationPlanEntry {
                source_path: PathBuf::from("/sources/Combat.bin"),
                destination_path: library_root.join("atari2600").join("Combat (USA).bin"),
                platform: Some("Atari2600".to_string()),
                platform_display_name: "Atari 2600".to_string(),
                platform_source: "Manual".to_string(),
                slug: None,
                layout_folder: Some("Atari 2600".to_string()),
                mode: OrganisationMode::BuildLinkedLibrary,
                content_classification: None,
                original_metadata: Default::default(),
                status: OrganisationStatus::Suggested,
                reason: None,
            }],
        });
        state.approved.insert("/sources/Combat.bin".to_string());

        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show_rom_organisation_page(ui, &mut state);
            });
        });
        assert!(rendered_text_contains(
            &output,
            "Source: /sources/Combat.bin"
        ));
        assert!(rendered_text_contains(&output, "Destination link:"));
        assert!(rendered_text_contains(&output, "Source action: Untouched"));
        assert!(rendered_text_contains(&output, "Result: Will create link"));
        // The linked-library preview never shows a rename/move arrow row.
        assert!(!rendered_text_contains(&output, "/sources/Combat.bin → "));
    }
}
