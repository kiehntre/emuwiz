//! This installation's local id: what snapshots and play sessions are
//! recorded under.
//!
//! It is **not** a cryptographic identity. It is a random number written
//! once to `<root>/installation.json` and reused for good, so that a save's
//! history stays one history. The signing key of the two-device feature is
//! a separate thing, created only when that feature is used; when it is,
//! the key declares (and signs) which installation id it speaks for, and
//! history recorded before that carries over untouched. See
//! [`super::device::DeviceIdentity::history_id`].
//!
//! Records made before this file existed were recorded under the id derived
//! from the device key. On such a profile the installation id **is** that
//! id, adopted as it stands, so nothing recorded earlier changes identity
//! and no record is rewritten.

use super::ContinuityError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

const FILE: &str = "installation.json";

/// Where the id came from. Recorded for a person reading the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A new random id: nothing had been recorded in this profile before.
    Generated,
    /// The id this profile's records already carried (the one derived from
    /// its device key), kept so that existing history stays continuous.
    AdoptedFromDeviceKey,
    /// The id file was missing; the id was taken from this profile's own
    /// device record, which names the installation id its key speaks for.
    RecoveredFromDeviceRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InstallationFile {
    format_version: u32,
    installation_id: String,
    origin: Origin,
}

/// An id is 32 lowercase hex digits (128 bits), the same shape as the ids
/// existing records carry.
pub fn well_formed(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn read(root: &Path) -> Result<Option<String>, ContinuityError> {
    let path = root.join(FILE);
    match fs::read(&path) {
        Ok(bytes) => {
            let file: InstallationFile = serde_json::from_slice(&bytes).map_err(|error| {
                ContinuityError::Corrupt(format!(
                    "{} cannot be understood ({error}); it was left as it is",
                    path.display()
                ))
            })?;
            if !well_formed(&file.installation_id) {
                // Never replaced by a fresh id: that would split every
                // history recorded so far.
                return Err(ContinuityError::Corrupt(format!(
                    "{} does not hold a valid id; it was left as it is",
                    path.display()
                )));
            }
            Ok(Some(file.installation_id))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(super::io("cannot read", &path, error)),
    }
}

/// The id this profile's records are under, **without creating anything**:
/// the stored id, or the id of a device key made before installation ids
/// existed, or `None` for a profile where nothing has been recorded.
pub fn existing(root: &Path) -> Result<Option<String>, ContinuityError> {
    if let Some(id) = read(root)? {
        return Ok(Some(id));
    }
    if let Some(id) = super::device::recorded_history_id(root) {
        return Ok(Some(id));
    }
    super::device::key_derived_id(root)
}

/// Whether anything has been recorded under `root` already.
fn holds_records(root: &Path) -> bool {
    ["sessions", "manifests"].iter().any(|part| {
        fs::read_dir(root.join(part))
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    })
}

/// The id to record under, created and stored on first use and the same
/// ever after. Creates the folder and `installation.json`; never a key.
pub fn open_or_create(root: &Path) -> Result<String, ContinuityError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        .map_err(|error| super::io("cannot create", root, error))?;
    if let Some(id) = read(root)? {
        return Ok(id);
    }
    let recovered = super::device::recorded_history_id(root);
    let (installation_id, origin) = match (recovered, super::device::key_derived_id(root)?) {
        // The id file is gone but this profile's device record still says
        // which id its history is under.
        (Some(id), _) => (id, Origin::RecoveredFromDeviceRecord),
        (None, Some(id)) => (id, Origin::AdoptedFromDeviceKey),
        // No id anywhere, yet records exist: they are under an id that has
        // been lost. A new id would silently start a second history beside
        // them, so nothing is created.
        (None, None) if holds_records(root) => {
            return Err(ContinuityError::Corrupt(format!(
                "{} is missing although this profile already holds records; a new id was not created, because it would split their history",
                root.join(FILE).display()
            )));
        }
        (None, None) => {
            use ring::rand::SecureRandom;
            let mut bytes = [0u8; 16];
            ring::rand::SystemRandom::new()
                .fill(&mut bytes)
                .map_err(|_| ContinuityError::Io("cannot create an installation id".into()))?;
            (
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                Origin::Generated,
            )
        }
    };
    let file = InstallationFile {
        format_version: 1,
        installation_id,
        origin,
    };
    let text = serde_json::to_vec_pretty(&file).map_err(|e| ContinuityError::Io(e.to_string()))?;
    // Put in place only if none is there yet, then use whatever is on disk:
    // if another EmuWiz got there first, its id is this installation's id.
    super::device::create_private_once(&root.join(FILE), &text)?;
    read(root)?.ok_or_else(|| {
        ContinuityError::Io("the installation id could not be read back after writing it".into())
    })
}
