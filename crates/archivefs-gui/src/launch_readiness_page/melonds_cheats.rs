//! Optional, per-launch melonDS cheats for the Playing Library.
//!
//! The user picks the native `<rom name>.mch` cheat file and ticks cheats
//! (nothing is ticked by default). Launching is the core's
//! `preflight_and_launch_melonds_with_cheats`; this module only supplies what
//! the core cannot know:
//!
//! * the cheat file is bound by SHA-256 when chosen and re-read and
//!   re-hashed in the launch worker, so a file changed after the choice is
//!   refused before melonDS starts;
//! * the ROM identity is hashed from the exact ROM about to be launched, after
//!   the Playing Library authorised it (verified game key plus the captured
//!   file identity the core re-checks).
//!
//! melonDS reports nothing about which cheats it loaded, so a launch never
//! claims a cheat is active.
use super::{StandaloneLaunchRequest, StandaloneLaunchState, StandaloneProcess};
use archivefs_core::{
    launch::{
        CanonicalIdentityStatus, MelonDsLaunchRequest, ResolvedIdentity,
        cheat_runtime::CheatRuntimeRoots,
        cheat_runtime_melonds::{
            MelonDsCheatActivation, MelonDsCheatLaunch, MelonDsCheatSelection,
            preflight_and_launch_melonds_with_cheats,
        },
        installation::LaunchInstallation,
    },
    patch_manager::{
        MELONDS_CHEAT_MAX_BYTES, MelonDsCheatFile, MelonDsCheatItem, MelonDsProfileDiscoveryRoots,
        MelonDsRomIdentity, discover_melonds_profiles, parse_melonds_cheat_file,
    },
};
use eframe::egui;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub(super) const START_NOTE: &str = "melonDS does not say which cheats it loaded, and EmuWiz has not seen one work in a game. For this launch melonDS is pointed at a temporary copy holding only the cheats you ticked (and cheats switched on); your ROM, cheat file, saves and melonDS settings are not changed, and the copy is removed after melonDS exits.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) name: String,
    pub(super) category: Option<String>,
    pub(super) only_one_in_category: bool,
    pub(super) chosen: bool,
    pub(super) note: Option<&'static str>,
    /// Why this cheat cannot be launched; such a cheat cannot be chosen.
    pub(super) unsupported: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub(super) struct Choice {
    pub(super) rom: PathBuf,
    pub(super) file: PathBuf,
    pub(super) sha256: String,
    pub(super) entries: Vec<Entry>,
}

fn rom_stem(rom: &Path) -> Option<String> {
    rom.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.rsplit_once('.').map(|(stem, _)| stem.to_string()))
        .filter(|s| !s.is_empty())
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A regular, size-bounded file read once. The opened handle must be the very
/// file that was inspected, so a path swapped for a link or another file
/// between the check and the open is refused.
fn read_cheat_bytes(file: &Path) -> Result<Vec<u8>, String> {
    use std::os::unix::fs::MetadataExt;
    let unreadable = |e: std::io::Error| format!("The cheat file cannot be read: {e}");
    let metadata = std::fs::symlink_metadata(file).map_err(unreadable)?;
    if !metadata.is_file() {
        return Err("The cheat file must be a regular file, not a link or folder.".into());
    }
    let handle = std::fs::File::open(file).map_err(unreadable)?;
    let opened = handle.metadata().map_err(unreadable)?;
    if !opened.is_file() || (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
        return Err("The cheat file changed while it was being opened. Choose it again.".into());
    }
    let mut bytes = Vec::new();
    handle
        .take(MELONDS_CHEAT_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() > MELONDS_CHEAT_MAX_BYTES {
        return Err("The cheat file is larger than melonDS cheat files can be.".into());
    }
    Ok(bytes)
}

fn parse_clean(bytes: &[u8]) -> Result<MelonDsCheatFile, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        "The cheat file is not text, so it is not a melonDS cheat file.".to_string()
    })?;
    let parsed = parse_melonds_cheat_file(text);
    if let Some(issue) = parsed.issues.first() {
        return Err(format!(
            "This is not a usable melonDS cheat file ({issue:?}). Nothing was changed."
        ));
    }
    Ok(parsed)
}

impl Choice {
    /// Reads and validates `file` for `rom`. Writes nothing.
    pub(super) fn load(rom: &Path, file: &Path) -> Result<Self, String> {
        let stem = rom_stem(rom).ok_or("The ROM has no usable name.")?;
        let expected = format!("{stem}.mch");
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name != expected {
            return Err(format!(
                "melonDS finds cheats by the ROM's name. This game needs a file named {expected}, but the chosen file is {name:?}. Nothing was changed."
            ));
        }
        let bytes = read_cheat_bytes(file)?;
        let parsed = parse_clean(&bytes)?;
        let mut entries = Vec::new();
        let mut add = |entry: &archivefs_core::patch_manager::MelonDsCheatEntry,
                       category: Option<(&str, bool)>| {
            let (unsupported, note) = if entry.code.words.is_empty() {
                (Some("it has no code"), None)
            } else if entry.code.opaque_word_pairs > 0 {
                (
                    None,
                    Some("EmuWiz cannot interpret all of this code; melonDS runs it as written"),
                )
            } else {
                (None, None)
            };
            entries.push(Entry {
                name: entry.name.clone(),
                category: category.map(|(name, _)| name.to_string()),
                only_one_in_category: category.is_some_and(|(_, only_one)| only_one),
                chosen: false,
                note,
                unsupported,
            });
        };
        for item in &parsed.items {
            match item {
                MelonDsCheatItem::RootCode(entry) => add(entry, None),
                MelonDsCheatItem::Category(category) => {
                    for entry in &category.entries {
                        add(
                            entry,
                            Some((&category.name, category.only_one_code_enabled)),
                        );
                    }
                }
            }
        }
        if entries.is_empty() {
            return Err("The file contains no cheats.".into());
        }
        Ok(Self {
            rom: rom.to_path_buf(),
            file: file.to_path_buf(),
            sha256: hex(&bytes),
            entries,
        })
    }

    pub(super) fn chosen_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.chosen && e.unsupported.is_none())
            .count()
    }

    /// The ticked names. Refuses nothing ticked, and two ticked cheats in a
    /// category that allows only one.
    pub(super) fn chosen_names(&self) -> Result<Vec<String>, String> {
        let chosen: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| e.chosen && e.unsupported.is_none())
            .collect();
        if chosen.is_empty() {
            return Err(
                "Tick at least one cheat, or clear the cheat file to launch without cheats.".into(),
            );
        }
        for entry in &chosen {
            if entry.only_one_in_category
                && chosen
                    .iter()
                    .filter(|other| other.category == entry.category)
                    .count()
                    > 1
            {
                return Err(format!(
                    "Category \"{}\" allows only one enabled cheat.",
                    entry.category.as_deref().unwrap_or_default()
                ));
            }
        }
        Ok(chosen.iter().map(|e| e.name.clone()).collect())
    }

    /// Re-reads the file and refuses it unless it is byte-identical to what
    /// the user chose from.
    fn fresh_file(&self) -> Result<MelonDsCheatFile, String> {
        let bytes = read_cheat_bytes(&self.file)?;
        if hex(&bytes) != self.sha256 {
            return Err("The cheat file changed after you chose from it. Choose it again.".into());
        }
        parse_clean(&bytes)
    }
}

/// Only a real, readable `.nds` header gives a game code; the hash binds the
/// cheat selection to these exact bytes.
fn rom_identity(
    rom: &Path,
    authorised: &archivefs_core::launch::process_spawn::CapturedFileIdentity,
) -> Result<MelonDsRomIdentity, String> {
    let mut file = std::fs::File::open(rom).map_err(|e| format!("The ROM cannot be read: {e}"))?;
    // The bytes hashed below must belong to the file the Playing Library
    // authorised, not merely to whatever is at the path now.
    let opened = file
        .metadata()
        .map_err(|e| format!("The ROM cannot be read: {e}"))?;
    if !opened.is_file()
        || &archivefs_core::launch::process_spawn::CapturedFileIdentity::capture(&opened)
            != authorised
    {
        return Err("The ROM changed since the launch was authorised.".into());
    }
    let mut hasher = Sha256::new();
    let mut header = [0u8; 0x10];
    let mut seen = 0usize;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("The ROM cannot be read: {e}"))?;
        if n == 0 {
            break;
        }
        if seen < header.len() {
            let take = (header.len() - seen).min(n);
            header[seen..seen + take].copy_from_slice(&buffer[..take]);
        }
        seen += n;
        hasher.update(&buffer[..n]);
    }
    let code = &header[0x0C..0x10];
    if seen < header.len() || !code.iter().all(u8::is_ascii_graphic) {
        return Err("The ROM does not have a Nintendo DS header.".into());
    }
    Ok(MelonDsRomIdentity {
        rom_path: rom.to_path_buf(),
        rom_sha256: Some(
            hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        ),
        game_code: Some(String::from_utf8_lossy(code).into_owned()),
        // The Playing Library only builds this request for a Ready candidate
        // whose independently verified game key matches; the core re-checks
        // the captured ROM, emulator and configuration identity.
        verified: true,
        title_hint: None,
    })
}

fn launch_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("melonds-{}-{nanos}", std::process::id())
}

/// Starts melonDS with the chosen cheats, or refuses before it starts.
pub(super) fn launch(
    request: &MelonDsLaunchRequest,
    roots: &MelonDsProfileDiscoveryRoots,
    choice: &Choice,
    runtime_roots: &CheatRuntimeRoots,
) -> Result<StandaloneProcess, String> {
    let refuse = |why: String| format!("melonDS was not started: {why}");
    if choice.rom != request.selected_content_path {
        return Err(refuse(
            "the cheat file was chosen for a different game.".into(),
        ));
    }
    let names = choice.chosen_names().map_err(refuse)?;
    let file = choice.fresh_file().map_err(refuse)?;
    let identity =
        rom_identity(&request.selected_content_path, &request.content_identity).map_err(refuse)?;
    let real_config = discover_melonds_profiles(roots)
        .profiles
        .iter()
        .find(|p| p.profile_id == request.profile_id)
        .and_then(|p| p.config_path.clone());
    let selection = MelonDsCheatSelection {
        identity,
        file,
        selected: names,
        real_config,
    };
    let canonical = CanonicalIdentityStatus::Resolved(ResolvedIdentity {
        platform_id: request.expected_platform_id.clone(),
        game_key: request.expected_game_key.clone(),
    });
    match preflight_and_launch_melonds_with_cheats(
        request,
        roots,
        &canonical,
        Some(&request.expected_game_key),
        Some(&selection),
        &launch_id(),
        runtime_roots,
    ) {
        Ok(MelonDsCheatLaunch::WithCheats {
            mut session, notes, ..
        }) => {
            for note in notes {
                session.note(note);
            }
            Ok(StandaloneProcess::MelonDsCheats(session))
        }
        Ok(MelonDsCheatLaunch::Plain(process)) => Ok(StandaloneProcess::MelonDs(process)),
        Err(error) => Err(refuse(format!("{error:?}"))),
    }
}

/// Cheats are offered only where the core can really deliver them.
pub(super) fn unsupported_reason(request: &MelonDsLaunchRequest) -> Option<&'static str> {
    matches!(request.expected_installation, LaunchInstallation::Flatpak { .. })
        .then_some("Cheats are not supported for a Flatpak melonDS (its settings folder cannot be redirected). melonDS will launch without cheats.")
}

pub(super) fn activation_label() -> &'static str {
    MelonDsCheatActivation::MaterialStagedEmulatorPointedAtItEffectNotProven.label()
}

/// The chooser. Drawn only while no launch is in progress.
pub(super) fn show_chooser(
    ui: &mut egui::Ui,
    state: &mut StandaloneLaunchState,
    request: &StandaloneLaunchRequest,
) {
    let StandaloneLaunchRequest::MelonDs(melonds, _) = request else {
        return;
    };
    if state.is_active() {
        return;
    }
    let rom = &melonds.selected_content_path;
    if state.melonds_cheats.as_ref().is_some_and(|c| &c.rom != rom) {
        state.melonds_cheats = None;
        state.melonds_message = None;
    }
    ui.separator();
    ui.label("Cheats for this launch (optional)");
    if let Some(reason) = unsupported_reason(melonds) {
        state.melonds_cheats = None;
        ui.label(reason);
        return;
    }
    let stem = rom_stem(rom).unwrap_or_default();
    if ui.button(format!("Choose {stem}.mch…")).clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("melonDS cheat file", &["mch"])
            .set_directory(rom.parent().unwrap_or(Path::new("/")))
            .pick_file()
    {
        match Choice::load(rom, &path) {
            Ok(choice) => {
                state.melonds_cheats = Some(choice);
                state.melonds_message = None;
            }
            Err(error) => {
                state.melonds_cheats = None;
                state.melonds_message = Some(error);
            }
        }
    }
    let mut clear = false;
    if let Some(choice) = state.melonds_cheats.as_mut() {
        for entry in &mut choice.entries {
            let label = match &entry.category {
                Some(category) => format!("{category} · {}", entry.name),
                None => entry.name.clone(),
            };
            match entry.unsupported {
                None => {
                    ui.checkbox(&mut entry.chosen, label);
                    if let Some(note) = entry.note {
                        ui.small(format!("  {note}"));
                    }
                }
                Some(reason) => {
                    ui.add_enabled(false, egui::Checkbox::new(&mut entry.chosen, label));
                    ui.small(format!("  Not available: {reason}; it will not be used."));
                }
            }
        }
        ui.small(START_NOTE);
        clear = ui
            .button("Launch without cheats (clear cheat file)")
            .clicked();
    }
    if clear {
        state.melonds_cheats = None;
    }
    if let Some(message) = &state.melonds_message {
        ui.label(message.as_str());
    }
}

/// What a finished cheat launch can honestly say.
pub(super) fn session_summary(
    session: &archivefs_core::launch::cheat_runtime::CheatRuntimeSession<
        archivefs_core::launch::process_spawn::WatchedProcess,
    >,
) -> Vec<String> {
    let receipt = session.receipt();
    let exit = match receipt.process.as_ref().and_then(|p| p.exit.as_ref()) {
        Some(exit) if exit.success => "exited normally".to_string(),
        Some(exit) => format!("failed (exit code {:?})", exit.code),
        None => "exited (status unknown)".to_string(),
    };
    let mut lines = vec![format!(
        "melonDS {exit}; temporary cheat files: {:?}. EmuWiz did not see the cheats work.",
        receipt.cleanup
    )];
    lines.extend(receipt.notes.iter().cloned());
    lines.extend(receipt.errors.iter().cloned());
    lines
}
