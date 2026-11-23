//! The one change EmuWiz may make in RomM: after a Rename, the displayed
//! title of that game's RomM entry, and on Undo, that title put back.
//!
//! Everything else in this adapter only reads. This module is used only when
//! the person turned on "Rename also updates RomM's title" and gave EmuWiz a
//! separate RomM client token that may edit ROMs (`roms.write`), loaded by
//! [`crate::identity_source::settings::title_token_file`] and used for nothing
//! else; browsing and import keep the read-only token. Otherwise RomM is
//! never written to.
//!
//! # What it changes, and when it refuses
//!
//! One request, `PUT /api/roms/{id}` with a single form field, `name`. RomM
//! keeps every field the form does not send (its ids, file name, cover,
//! manual, notes, collections, and identification), which was confirmed
//! against RomM 5.3.1's own `update_rom`. The title is changed only when all
//! of these hold, read live from RomM just before the write:
//!
//! - the entry is the one EmuWiz's last RomM import linked to this exact file
//!   (by its mapped path, never by a similar name or title), it is still that
//!   one file, and its size and hashes still agree with what was imported;
//! - RomM never identified the game (`is_identified` false), so its title is
//!   only the one RomM derived from the file name;
//! - its title is still that derived one, so nobody edited it by hand;
//! - RomM would not rename the file while saving (a name RomM's own filename
//!   cleaning would change).
//!
//! What was written is recorded beside the rename's History entry (keyed by
//! its transaction id) before the write. Undo restores the original title
//! only while RomM's title is still exactly the one written; a later edit, by
//! hand or by RomM, is never overwritten.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::{
    MAX_RESPONSE_BYTES, REQUEST_TIMEOUT, RommClient, RommHttpResponse, RommRequestError,
    RommTransport, UreqTransport,
};
use super::config::ValidatedRommSource;
use crate::identity_source::cache::IdentityCache;
use crate::identity_source::model::HashAlgorithm;

/// Folder, under the RomM settings directory, holding one record per rename
/// whose RomM title EmuWiz changed.
pub const RECORD_FOLDER: &str = "rename-title-sync";

/// What EmuWiz wrote to RomM for one rename, so Undo can put it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleSyncRecord {
    /// The rename's History (journal) transaction id.
    pub transaction_id: String,
    /// The RomM instance (its approved origin), so Undo never writes to
    /// another server.
    pub server_id: String,
    pub rom_id: String,
    pub original_title: String,
    pub written_title: String,
}

/// How a title update or restore ended, in words for the rename card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TitleSyncOutcome {
    /// RomM's title was changed from the first to the second.
    Changed { from: String, to: String },
    /// Nothing was sent to RomM, for this reason.
    LeftAlone(String),
}

impl TitleSyncOutcome {
    pub fn line(&self) -> String {
        match self {
            Self::Changed { from, to } => {
                format!("RomM: its title changed from \"{from}\" to \"{to}\".")
            }
            Self::LeftAlone(reason) => format!("RomM's title was not changed: {reason}"),
        }
    }
}

/// The one write: a form `PUT`. Separate from [`RommTransport`] so the
/// read-only client cannot reach it.
pub trait RommTitleTransport: RommTransport {
    fn put_form(
        &self,
        url: &str,
        authorization: &str,
        content_type: &str,
        body: &[u8],
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<RommHttpResponse, RommRequestError>;
}

impl RommTitleTransport for UreqTransport {
    fn put_form(
        &self,
        url: &str,
        authorization: &str,
        content_type: &str,
        body: &[u8],
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<RommHttpResponse, RommRequestError> {
        let request = self
            .agent()
            .put(url)
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            .header("Content-Type", content_type)
            .config()
            .timeout_global(Some(timeout))
            .build();
        super::client::finish_response(request.send(body), max_bytes)
    }
}

/// The title RomM gives a file it never identified: the name without its
/// extension and without trailing `(...)` / `[...]` groups. Mirrors RomM
/// 5.3.1's `compute_file_name_no_tags`; [`update_title_after_rename`] checks
/// it against what RomM itself reports before relying on it.
pub fn romm_derived_title(file_name: &str) -> String {
    let mut name = strip_extension(file_name).trim().to_string();
    loop {
        let trimmed = name.trim_end();
        let close = trimmed.chars().last();
        let open = match close {
            Some(')') => '(',
            Some(']') => '[',
            _ => break,
        };
        let inner = &trimmed[..trimmed.len() - 1];
        let Some(start) = inner.rfind(open) else {
            break;
        };
        if inner[start + 1..].contains(close.unwrap_or(')')) {
            break;
        }
        name = trimmed[..start].to_string();
    }
    name.trim().to_string()
}

/// RomM's `\.(([a-z]+\.)*\w+)$`: the leftmost dot after which come only
/// lowercase-letter parts and a final word part.
fn strip_extension(file_name: &str) -> &str {
    for (index, _) in file_name.match_indices('.') {
        let rest = &file_name[index + 1..];
        let parts: Vec<&str> = rest.split('.').collect();
        let (last, middle) = parts.split_last().expect("split yields one part");
        let word =
            |part: &str| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_');
        if word(last)
            && middle
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase()))
        {
            return &file_name[..index];
        }
    }
    file_name
}

/// Whether RomM's filename cleaning (run on every save) would change
/// `fs_name`, in which case saving the title would also rename the file.
fn romm_would_rename(fs_name: &str) -> bool {
    fs_name.trim() != fs_name
        || fs_name
            .chars()
            .any(|c| "\\/:|*?\"<>+".contains(c) || c.is_ascii_control())
}

/// Where the records live: `<RomM settings directory>/rename-title-sync`.
pub fn record_folder(romm_settings_directory: &Path) -> PathBuf {
    romm_settings_directory.join(RECORD_FOLDER)
}

fn record_path(folder: &Path, transaction_id: &str) -> Option<PathBuf> {
    let safe = !transaction_id.is_empty()
        && transaction_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    safe.then(|| folder.join(format!("{transaction_id}.json")))
}

/// Whether the file at `path` has the hash RomM reports for the entry
/// (SHA-1, or MD5 when RomM has only that). `Err` when there is nothing to
/// compare or the file cannot be read.
fn local_file_matches_romm(path: &Path, live: &Value, cancel: &AtomicBool) -> Result<bool, String> {
    let reported = |field| text(live, field).filter(|value| !value.is_empty());
    let (expected, actual) = if let Some(sha1) = reported("sha1_hash") {
        (sha1, digest_file::<sha1::Sha1>(path, cancel)?)
    } else if let Some(md5) = reported("md5_hash") {
        (md5, digest_file::<md5::Md5>(path, cancel)?)
    } else {
        return Err("RomM has no SHA-1 or MD5 for this entry (its hashing may be off), so EmuWiz cannot confirm it is this file.".into());
    };
    Ok(actual.eq_ignore_ascii_case(expected))
}

/// The file's digest as lower-case hex, read in 1 MiB pieces.
fn digest_file<D: sha1::Digest>(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    use std::io::Read as _;
    let unreadable = |error: std::io::Error| {
        format!(
            "this file could not be read to check it ({}).",
            error.kind()
        )
    };
    let mut file = std::fs::File::open(path).map_err(unreadable)?;
    let mut digest = D::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("the check was cancelled.".into());
        }
        let read = file.read(&mut buffer).map_err(unreadable)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn save_record(folder: &Path, record: &TitleSyncRecord) -> Result<(), String> {
    let path = record_path(folder, &record.transaction_id)
        .ok_or("the rename's History id is not usable as a record name")?;
    std::fs::create_dir_all(folder).map_err(|error| format!("could not record it: {error}"))?;
    let temporary = path.with_extension("json.partial");
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes)
        .and_then(|()| std::fs::File::open(&temporary)?.sync_all())
        .and_then(|()| std::fs::rename(&temporary, &path))
        .map_err(|error| format!("could not record it: {error}"))
}

/// The record of what EmuWiz wrote for `transaction_id`, if it wrote anything.
pub fn load_record(folder: &Path, transaction_id: &str) -> Result<Option<TitleSyncRecord>, String> {
    let Some(path) = record_path(folder, transaction_id) else {
        return Ok(None);
    };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("the RomM title record could not be read: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("the RomM title record could not be read: {error}")),
    }
}

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn same_id(value: &Value, id: &str) -> bool {
    match value.get("id") {
        Some(Value::Number(number)) => number.to_string() == id,
        Some(Value::String(text)) => text == id,
        _ => false,
    }
}

/// Reads entry `id` as RomM holds it now. `Ok(None)`: RomM no longer has it.
fn live_rom<T: RommTransport>(
    client: &RommClient<'_, T>,
    id: &str,
    cancel: &AtomicBool,
) -> Result<Option<Value>, String> {
    match client.rom(id, Some(cancel)) {
        Ok(value) => Ok(Some(value)),
        Err(RommRequestError::HttpStatus { status: 404 }) => Ok(None),
        Err(error) => Err(format!("RomM could not be read ({}).", error.detail())),
    }
}

/// Sends the one form field `name`. Checks RomM answered with that title.
fn put_title<T: RommTitleTransport>(
    source: &ValidatedRommSource,
    transport: &T,
    id: &str,
    title: &str,
) -> Result<(), String> {
    let url = source
        .endpoint()
        .url_for(&format!("/api/roms/{id}"))
        .map_err(|refusal| format!("RomM's address was refused ({refusal:?})."))?;
    // A title is one line of text; this boundary cannot occur in it.
    if title.contains(['\r', '\n']) {
        return Err("the title has a line break, so it was not sent.".into());
    }
    let boundary = format!(
        "emuwiz-title-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    );
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(title.as_bytes());
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let response = source.token().with_header_value(|header| {
        transport.put_form(
            &url,
            header,
            &content_type,
            &body,
            MAX_RESPONSE_BYTES,
            REQUEST_TIMEOUT,
        )
    });
    let response =
        response.map_err(|error| format!("RomM could not be reached ({}).", error.detail()))?;
    match response.status {
        200 => {}
        401 | 403 => {
            return Err("RomM refused the change: the title-update token may not edit games (it needs the roms.write permission).".into());
        }
        404 => return Err("RomM no longer has this game's entry.".into()),
        status => return Err(format!("RomM refused the change (HTTP {status}).")),
    }
    let answered: Value = serde_json::from_slice(&response.body).map_err(|_| {
        "RomM's answer could not be read, so the change is not confirmed.".to_string()
    })?;
    if text(&answered, "name") != Some(title) || !same_id(&answered, id) {
        return Err(
            "RomM's answer does not show the new title, so the change is not confirmed.".into(),
        );
    }
    Ok(())
}

/// After `old_path` was renamed to `new_path` (History entry
/// `transaction_id`): changes the RomM title of the entry linked to that file
/// from the one RomM derived from the old name to the one it would derive
/// from the new name. `Err` is a RomM failure to report; the rename itself
/// stands either way.
///
/// `earlier_paths` are the names Rename History proves this file had before
/// `old_path` (a game renamed before title updates existed still shows the
/// title of such a name, and EmuWiz's RomM import may still list it there);
/// `Err` says why History could not settle them, and then only `old_path`
/// counts.
#[allow(clippy::too_many_arguments)]
pub fn update_title_after_rename<T: RommTitleTransport>(
    source: &ValidatedRommSource,
    transport: &T,
    cache: &IdentityCache,
    record_folder: &Path,
    transaction_id: &str,
    old_path: &Path,
    new_path: &Path,
    earlier_paths: Result<&[PathBuf], &str>,
    cancel: &AtomicBool,
) -> Result<TitleSyncOutcome, String> {
    let left = |reason: &str| Ok(TitleSyncOutcome::LeftAlone(reason.to_string()));
    let mut known_paths: Vec<&Path> = vec![old_path];
    known_paths.extend(
        earlier_paths
            .unwrap_or_default()
            .iter()
            .map(PathBuf::as_path),
    );
    if cache.server_id != source.server_id() {
        return left(
            "EmuWiz's RomM import is from a different RomM server. Import RomM again first.",
        );
    }
    let linked: Vec<_> = cache
        .records
        .iter()
        .filter(|record| {
            record
                .archivefs_path
                .as_deref()
                .is_some_and(|path| known_paths.contains(&path))
        })
        .collect();
    let record = match linked.as_slice() {
        [] => return left("EmuWiz's last RomM import has no entry for this file."),
        [record] => *record,
        _ => {
            return left(
                "more than one RomM entry is linked to this file, so EmuWiz cannot tell which is this game.",
            );
        }
    };
    let id = record.provider_game_id.as_str();
    let client = RommClient::new(source, transport);
    let Some(live) = live_rom(&client, id, cancel)? else {
        return left("RomM no longer has the entry EmuWiz imported for this file.");
    };
    if !same_id(&live, id) {
        return left("RomM answered with a different entry.");
    }
    if live.get("is_identified") != Some(&Value::Bool(false))
        || live.get("is_unidentified") != Some(&Value::Bool(true))
    {
        return left(
            "RomM identified this game, so its title comes from RomM's metadata and is left to RomM.",
        );
    }
    let files = live
        .get("files")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if live.get("has_simple_single_file") != Some(&Value::Bool(true)) || files != 1 {
        return left("RomM's entry is not a single file, so EmuWiz cannot be sure it is this one.");
    }
    // The live entry must still be this file, by its mapped path: RomM has
    // either not scanned since (old name) or already picked up the rename.
    let live_path = source
        .mappings()
        .translate(&super::normalise::provider_path_of(&live))
        .archivefs_path()
        .map(Path::to_path_buf);
    if !live_path
        .as_deref()
        .is_some_and(|path| path == new_path || known_paths.contains(&path))
    {
        return left("RomM's entry now points at a different file.");
    }
    let size = std::fs::metadata(new_path)
        .map(|metadata| metadata.len())
        .ok();
    let live_size = live.get("fs_size_bytes").and_then(Value::as_u64);
    if size.is_none()
        || live_size != size
        || record
            .file_size_bytes
            .is_some_and(|bytes| Some(bytes) != size)
    {
        return left("RomM's entry does not have this file's size.");
    }
    for hash in &record.hashes {
        let field = match hash.algorithm {
            HashAlgorithm::Sha1 => "sha1_hash",
            HashAlgorithm::Md5 => "md5_hash",
            HashAlgorithm::Crc32 => "crc_hash",
        };
        if let Some(live_hash) = text(&live, field).filter(|value| !value.is_empty())
            && !live_hash.eq_ignore_ascii_case(&hash.value)
        {
            return left("RomM's entry no longer has this file's hash.");
        }
    }
    // The renamed file itself must be what RomM hashed: two RomM snapshots
    // agreeing says nothing about a local file replaced since the import.
    match local_file_matches_romm(new_path, &live, cancel) {
        Ok(true) => {}
        Ok(false) => return left("this file's content is not what RomM's entry has."),
        Err(why) => return left(&why),
    }
    let Some(fs_name) = text(&live, "fs_name") else {
        return left("RomM did not report the entry's file name.");
    };
    if romm_would_rename(fs_name) {
        return left(
            "saving a title would make RomM rename this file (its name has characters RomM replaces).",
        );
    }
    let new_name = file_name(new_path);
    // Cross-check the title rule against RomM's own answer for its file name.
    if text(&live, "fs_name_no_tags") != Some(romm_derived_title(fs_name).as_str()) {
        return left(
            "RomM derives titles differently from what EmuWiz expects, so nothing is guessed.",
        );
    }
    // The title must be the one RomM gives this file's name now, or one of
    // the names History proves it had. Anything else may be a custom title.
    let current = text(&live, "name").unwrap_or_default();
    let from_a_known_name = known_paths
        .iter()
        .any(|path| romm_derived_title(&file_name(path)) == current);
    if !from_a_known_name {
        return match earlier_paths {
            Err(why) => left(&format!(
                "its RomM title could not be safely traced to this file's names ({why}), so it is kept."
            )),
            Ok(_) => left(
                "its RomM title is not the one RomM gives this file's current or earlier names, so it may be a custom title and is kept.",
            ),
        };
    }
    let written = romm_derived_title(&new_name);
    if written.is_empty() || written == current {
        return left("it already has the title the new name gives.");
    }
    let sync = TitleSyncRecord {
        transaction_id: transaction_id.to_string(),
        server_id: source.server_id().to_string(),
        rom_id: id.to_string(),
        original_title: current.to_string(),
        written_title: written.clone(),
    };
    // Recorded before the write, so Undo knows of it even if EmuWiz stops
    // straight after; Undo acts only if RomM really shows the written title.
    save_record(record_folder, &sync)?;
    put_title(source, transport, id, &written)?;
    Ok(TitleSyncOutcome::Changed {
        from: sync.original_title,
        to: written,
    })
}

/// After History Undo of `transaction_id`: puts RomM's original title back,
/// only while RomM still shows exactly the title EmuWiz wrote. `Ok(None)`:
/// EmuWiz changed nothing in RomM for that rename.
pub fn restore_title_after_undo<T: RommTitleTransport>(
    source: &ValidatedRommSource,
    transport: &T,
    record_folder: &Path,
    transaction_id: &str,
    cancel: &AtomicBool,
) -> Result<Option<TitleSyncOutcome>, String> {
    let Some(sync) = load_record(record_folder, transaction_id)? else {
        return Ok(None);
    };
    let left = |reason: &str| Ok(Some(TitleSyncOutcome::LeftAlone(reason.to_string())));
    if sync.server_id != source.server_id() {
        return left("EmuWiz is now connected to a different RomM server than the one it changed.");
    }
    let client = RommClient::new(source, transport);
    let Some(live) = live_rom(&client, &sync.rom_id, cancel)? else {
        return left("RomM no longer has the game's entry.");
    };
    if !same_id(&live, &sync.rom_id) {
        return left("RomM answered with a different entry.");
    }
    if text(&live, "name") != Some(sync.written_title.as_str()) {
        return left("its title was changed in RomM after the rename, so that change is kept.");
    }
    if live.get("is_identified") != Some(&Value::Bool(false)) {
        return left("RomM has identified the game since, so its title is left to RomM.");
    }
    if text(&live, "fs_name").is_none_or(romm_would_rename) {
        return left("saving a title would make RomM rename this file.");
    }
    put_title(source, transport, &sync.rom_id, &sync.original_title)?;
    if let Some(path) = record_path(record_folder, transaction_id) {
        let _ = std::fs::remove_file(path);
    }
    Ok(Some(TitleSyncOutcome::Changed {
        from: sync.written_title,
        to: sync.original_title,
    }))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "title_sync_tests.rs"]
mod tests;
