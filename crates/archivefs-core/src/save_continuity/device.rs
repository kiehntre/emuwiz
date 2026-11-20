//! Two devices, compared offline.
//!
//! **There is no network code in this module.** It defines what two EmuWiz
//! devices would say to each other and checks every rule against two local
//! folders standing in for a desktop and a laptop. A real transport would
//! carry exactly these messages inside an authenticated, encrypted channel;
//! that is future work and needs its own approval.
//!
//! The rules, each of which exists because the obvious shortcut loses saves:
//!
//! - **A device is its key.** Each device makes an Ed25519 key once. Its id
//!   is a hash of the public key. Everything it sends is signed. A device is
//!   believed only after a person has paired it (pinned its key); a message
//!   signed by any other key is refused, whatever name it claims.
//! - **Order comes from history, never from clocks.** Each save has a list of
//!   the contents it has had, oldest first. One device is "ahead" only if the
//!   other's list is the start of its own. Clocks on two machines disagree;
//!   times are shown, never compared.
//! - **Two different continuations are a conflict.** If both lists moved on
//!   from a common point, or share nothing, a person decides. There is no
//!   "newest wins".
//! - **Messages do not go backwards.** Every signed list carries a counter;
//!   an older list than one already seen from that device is refused, and a
//!   transfer is refused if the sender has moved on since the list was made.
//! - **A transfer can stop anywhere.** Content arrives into a work file,
//!   resumes from where it stopped, and enters the store only after its
//!   SHA-256 matches. The snapshot record goes in last.
//! - **Receiving is not restoring.** A received snapshot sits in the store
//!   under the sender's name. Using it is a separate, confirmed step.

use super::store::{CaptureBoundary, SnapshotManifest, SnapshotStore};
use super::{ContinuityError, now_unix, sha256_hex};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const MANIFEST_SET_FORMAT_VERSION: u32 = 1;
pub const TRANSFER_CHUNK_BYTES: usize = 64 * 1024;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The public half of a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    /// First 32 hex digits of SHA-256(public key).
    pub device_id: String,
    /// A label a person chose ("Desktop"). Not trusted for anything.
    pub name: String,
    pub public_key: String,
}

impl DeviceIdentity {
    fn id_for(public_key: &[u8]) -> String {
        sha256_hex(public_key)[..32].to_string()
    }

    /// The short code two people compare when pairing.
    pub fn fingerprint(&self) -> String {
        self.device_id
            .as_bytes()
            .chunks(4)
            .take(4)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("-")
    }

    fn key_bytes(&self) -> Result<Vec<u8>, ContinuityError> {
        let key = unhex(&self.public_key)
            .filter(|key| key.len() == 32)
            .ok_or_else(|| {
                ContinuityError::Refused("the device's public key is malformed".into())
            })?;
        if Self::id_for(&key) != self.device_id {
            return Err(ContinuityError::Refused(
                "the device id does not belong to its key".into(),
            ));
        }
        Ok(key)
    }
}

/// This device: its identity, its private key, and where both are kept.
pub struct LocalDevice {
    pub identity: DeviceIdentity,
    key: Ed25519KeyPair,
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct DeviceFile {
    identity: DeviceIdentity,
    /// Counter for signed lists; only ever goes up.
    sequence: u64,
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ContinuityError> {
    let work = path.with_extension(format!("part-{}", std::process::id()));
    let _ = fs::remove_file(&work);
    let result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&work)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&work, path));
    result.map_err(|error| super::io("cannot write", path, error))
}

impl LocalDevice {
    /// Loads this profile's device, creating its key on first use. The key
    /// never leaves `<root>/device.key` (mode 0600).
    pub fn open_or_create(root: &Path, name: &str) -> Result<Self, ContinuityError> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .map_err(|error| super::io("cannot create", root, error))?;
        let key_path = root.join("device.key");
        let info_path = root.join("device.json");
        let pkcs8 = match fs::read(&key_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let rng = ring::rand::SystemRandom::new();
                let document = Ed25519KeyPair::generate_pkcs8(&rng)
                    .map_err(|_| ContinuityError::Io("cannot create a device key".into()))?;
                write_private(&key_path, document.as_ref())?;
                document.as_ref().to_vec()
            }
            Err(error) => return Err(super::io("cannot read", &key_path, error)),
        };
        let key = Ed25519KeyPair::from_pkcs8(&pkcs8)
            .map_err(|_| ContinuityError::Corrupt("the device key file is damaged".into()))?;
        let public = key.public_key().as_ref().to_vec();
        let identity = DeviceIdentity {
            device_id: DeviceIdentity::id_for(&public),
            name: name.to_string(),
            public_key: hex(&public),
        };
        let device = Self {
            identity,
            key,
            root: root.to_path_buf(),
        };
        if fs::symlink_metadata(&info_path).is_err() {
            device.write_info(0)?;
        }
        Ok(device)
    }

    fn write_info(&self, sequence: u64) -> Result<(), ContinuityError> {
        let file = DeviceFile {
            identity: self.identity.clone(),
            sequence,
        };
        let text =
            serde_json::to_vec_pretty(&file).map_err(|e| ContinuityError::Io(e.to_string()))?;
        write_private(&self.root.join("device.json"), &text)
    }

    /// The counter of the newest list this device has signed.
    pub fn sequence(&self) -> u64 {
        fs::read(self.root.join("device.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DeviceFile>(&bytes).ok())
            .map_or(0, |file| file.sequence)
    }

    /// Signs a list of this device's own snapshots in `store`.
    pub fn export(&self, store: &SnapshotStore) -> Result<SignedManifestSet, ContinuityError> {
        let snapshots: Vec<SnapshotManifest> = store
            .list()
            .into_iter()
            .filter(|manifest| manifest.body.provenance.device_id == self.identity.device_id)
            .collect();
        let sequence = self.sequence() + 1;
        self.write_info(sequence)?;
        let body = ManifestSetBody {
            format_version: MANIFEST_SET_FORMAT_VERSION,
            device: self.identity.clone(),
            sequence,
            generated_unix: now_unix(),
            histories: histories(&snapshots),
            snapshots,
        };
        let signature = self.key.sign(&body.signing_bytes());
        Ok(SignedManifestSet {
            body,
            signature: hex(signature.as_ref()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub snapshot_id: String,
    /// The sender's clock. Shown, never compared across devices.
    pub captured_unix: i64,
    pub fingerprint: String,
    pub boundary: CaptureBoundary,
}

/// The contents one save has had on one device, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveHistory {
    /// `ps2:BASLUS-20827MANHUNT`, or `file:<lineage>:<name>` for a save
    /// with no identity of its own.
    pub key: String,
    pub lineage: String,
    pub name: String,
    pub product_code: Option<String>,
    pub title: Option<String>,
    pub emulator: String,
    pub entries: Vec<HistoryEntry>,
}

/// Builds per-save histories from snapshots. A content that repeats in
/// consecutive snapshots appears once.
pub fn histories(snapshots: &[SnapshotManifest]) -> Vec<SaveHistory> {
    let mut by_lineage: BTreeMap<&str, Vec<&SnapshotManifest>> = BTreeMap::new();
    for manifest in snapshots {
        by_lineage
            .entry(&manifest.body.lineage)
            .or_default()
            .push(manifest);
    }
    let mut out: BTreeMap<(String, String), SaveHistory> = BTreeMap::new();
    for (lineage, mut chain) in by_lineage {
        chain.sort_by_key(|manifest| (manifest.body.captured_unix, manifest.snapshot_id.clone()));
        // Follow parent links where they exist; the sort is only a fallback
        // for the first snapshot and for records received out of order.
        let mut ordered: Vec<&SnapshotManifest> = Vec::with_capacity(chain.len());
        let mut remaining = chain;
        while !remaining.is_empty() {
            let last = ordered.last().map(|manifest| manifest.snapshot_id.as_str());
            let next = remaining
                .iter()
                .position(|manifest| manifest.body.parent.as_deref() == last)
                .unwrap_or(0);
            ordered.push(remaining.remove(next));
        }
        for manifest in ordered {
            let mut items: Vec<(String, String, Option<String>, Option<String>, String)> = manifest
                .body
                .contained_saves
                .iter()
                .map(|save| {
                    (
                        save.key.clone(),
                        save.name.clone(),
                        save.product_code.clone(),
                        save.title.clone(),
                        save.fingerprint.clone(),
                    )
                })
                .collect();
            if items.is_empty() {
                // A save with no identity of its own: each file is its own history.
                items = manifest
                    .body
                    .files
                    .iter()
                    .map(|file| {
                        (
                            format!("file:{lineage}:{}", file.name),
                            file.name.clone(),
                            None,
                            None,
                            file.sha256.clone(),
                        )
                    })
                    .collect();
            }
            for (key, name, product_code, title, fingerprint) in items {
                let history = out
                    .entry((key.clone(), lineage.to_string()))
                    .or_insert_with(|| SaveHistory {
                        key,
                        lineage: lineage.to_string(),
                        name,
                        product_code,
                        title,
                        emulator: manifest.body.provenance.emulator.clone(),
                        entries: Vec::new(),
                    });
                if history
                    .entries
                    .last()
                    .map(|entry| entry.fingerprint.as_str())
                    != Some(fingerprint.as_str())
                {
                    history.entries.push(HistoryEntry {
                        snapshot_id: manifest.snapshot_id.clone(),
                        captured_unix: manifest.body.captured_unix,
                        fingerprint,
                        boundary: manifest.body.boundary,
                    });
                }
            }
        }
    }
    out.into_values().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSetBody {
    pub format_version: u32,
    pub device: DeviceIdentity,
    pub sequence: u64,
    pub generated_unix: i64,
    pub histories: Vec<SaveHistory>,
    pub snapshots: Vec<SnapshotManifest>,
}

impl ManifestSetBody {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = b"emuwiz-save-continuity-manifest-set-v1\0".to_vec();
        bytes.extend_from_slice(&serde_json::to_vec(self).unwrap_or_default());
        bytes
    }
}

/// What one device tells another: its save histories, signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedManifestSet {
    pub body: ManifestSetBody,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Peer {
    identity: DeviceIdentity,
    paired_unix: i64,
    highest_sequence: u64,
}

/// The devices a person has paired with this one, and the newest list seen
/// from each. Kept in `<root>/peers.json`.
pub struct PeerBook {
    path: PathBuf,
    peers: BTreeMap<String, Peer>,
}

impl PeerBook {
    pub fn open(root: &Path) -> Self {
        let path = root.join("peers.json");
        let peers = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, peers }
    }

    fn save(&self) -> Result<(), ContinuityError> {
        let text = serde_json::to_vec_pretty(&self.peers)
            .map_err(|e| ContinuityError::Io(e.to_string()))?;
        write_private(&self.path, &text)
    }

    pub fn is_paired(&self, device_id: &str) -> bool {
        self.peers.contains_key(device_id)
    }

    /// Pins `identity` as a trusted device. This is the step a person
    /// confirms after comparing [`DeviceIdentity::fingerprint`] on both
    /// screens. A device that is already paired cannot be re-paired with a
    /// different key.
    pub fn pair(
        &mut self,
        identity: &DeviceIdentity,
        this_device: &DeviceIdentity,
    ) -> Result<(), ContinuityError> {
        identity.key_bytes()?;
        if identity.device_id == this_device.device_id {
            return Err(ContinuityError::Refused(
                "a device cannot be paired with itself".into(),
            ));
        }
        if let Some(existing) = self.peers.get(&identity.device_id) {
            if existing.identity.public_key != identity.public_key {
                return Err(ContinuityError::Refused(
                    "this device id is already paired with a different key".into(),
                ));
            }
            return Ok(());
        }
        self.peers.insert(
            identity.device_id.clone(),
            Peer {
                identity: identity.clone(),
                paired_unix: now_unix(),
                highest_sequence: 0,
            },
        );
        self.save()
    }

    /// Checks a received list: paired device, pinned key, valid signature,
    /// well-formed snapshots, and not older than one already accepted.
    pub fn accept(&mut self, set: &SignedManifestSet) -> Result<(), ContinuityError> {
        let body = &set.body;
        if body.format_version != MANIFEST_SET_FORMAT_VERSION {
            return Err(ContinuityError::Refused(
                "the other device speaks a version this EmuWiz does not".into(),
            ));
        }
        let key = body.device.key_bytes()?;
        let peer = self.peers.get(&body.device.device_id).ok_or_else(|| {
            ContinuityError::Refused(format!(
                "device {} is not paired with this one; nothing from it is read",
                body.device.fingerprint()
            ))
        })?;
        if peer.identity.public_key != body.device.public_key {
            return Err(ContinuityError::Refused(
                "the sender's key is not the key that was paired".into(),
            ));
        }
        let signature = unhex(&set.signature)
            .ok_or_else(|| ContinuityError::Refused("the signature is malformed".into()))?;
        UnparsedPublicKey::new(&ED25519, &key)
            .verify(&body.signing_bytes(), &signature)
            .map_err(|_| ContinuityError::Refused("the list's signature does not match its contents; it was altered or is not from the paired device".into()))?;
        if body.sequence < peer.highest_sequence {
            return Err(ContinuityError::Refused(format!(
                "this list (number {}) is older than one already received from that device (number {}); refusing to go backwards",
                body.sequence, peer.highest_sequence
            )));
        }
        for manifest in &body.snapshots {
            if super::store::body_id(&manifest.body) != manifest.snapshot_id {
                return Err(ContinuityError::Corrupt(
                    "a snapshot record in the list does not match its id".into(),
                ));
            }
            if manifest.body.provenance.device_id != body.device.device_id {
                return Err(ContinuityError::Refused(
                    "the list contains a snapshot the sender did not take".into(),
                ));
            }
        }
        if histories(&body.snapshots) != body.histories {
            return Err(ContinuityError::Refused(
                "the list's histories do not follow from its snapshots".into(),
            ));
        }
        let sequence = body.sequence;
        if let Some(peer) = self.peers.get_mut(&body.device.device_id) {
            peer.highest_sequence = sequence;
        }
        self.save()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "relation")]
pub enum Relation {
    /// Both devices have the same current content.
    Same,
    /// The other device continued from where this one is.
    OtherAhead {
        new_contents: usize,
    },
    /// This device continued from where the other one is.
    ThisAhead {
        new_contents: usize,
    },
    /// Both continued separately from a shared point.
    Diverged {
        shared_fingerprint: String,
        only_here: usize,
        only_there: usize,
    },
    /// Both have this save and their histories share nothing.
    Unrelated,
    OnlyHere,
    OnlyThere,
    /// One device keeps this save in more than one place with different
    /// content; which one is "the" save there is not decidable.
    SeveralCopies {
        device: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveComparison {
    pub key: String,
    pub name: String,
    pub title: Option<String>,
    pub relation: Relation,
    pub here_current: Option<HistoryEntry>,
    pub there_current: Option<HistoryEntry>,
    /// A person must choose before anything can be proposed.
    pub needs_decision: bool,
    /// What EmuWiz proposes, in words. Never carried out by a comparison.
    pub proposal: String,
}

fn current<'a>(histories: &[&'a SaveHistory]) -> Result<Option<&'a SaveHistory>, ()> {
    match histories {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many => {
            let first = many[0].entries.last().map(|entry| &entry.fingerprint);
            if many
                .iter()
                .all(|history| history.entries.last().map(|entry| &entry.fingerprint) == first)
            {
                // Same content everywhere: the longest history speaks for them.
                Ok(many
                    .iter()
                    .max_by_key(|history| history.entries.len())
                    .copied())
            } else {
                Err(())
            }
        }
    }
}

/// Compares this device's histories with another's. Reads nothing from
/// disk and changes nothing; the result is for a person.
pub fn compare(here: &ManifestSetBody, there: &ManifestSetBody) -> Vec<SaveComparison> {
    let group = |body: &'_ ManifestSetBody| -> BTreeMap<String, Vec<SaveHistory>> {
        let mut map: BTreeMap<String, Vec<SaveHistory>> = BTreeMap::new();
        for history in &body.histories {
            // A save without identity is only comparable by name within one
            // lineage, which never matches across devices: keep it apart.
            map.entry(history.key.clone())
                .or_default()
                .push(history.clone());
        }
        map
    };
    let (mine, theirs) = (group(here), group(there));
    let mut keys: Vec<&String> = mine.keys().chain(theirs.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut out = Vec::new();
    for key in keys {
        let a: Vec<&SaveHistory> = mine
            .get(key)
            .map(|list| list.iter().collect())
            .unwrap_or_default();
        let b: Vec<&SaveHistory> = theirs
            .get(key)
            .map(|list| list.iter().collect())
            .unwrap_or_default();
        let sample = a.first().or(b.first()).copied();
        let (name, title) = sample
            .map(|h| (h.name.clone(), h.title.clone()))
            .unwrap_or_default();
        let other = &there.device.name;
        let (relation, here_current, there_current) = match (current(&a), current(&b)) {
            (Err(()), _) => (
                Relation::SeveralCopies {
                    device: here.device.name.clone(),
                },
                None,
                None,
            ),
            (_, Err(())) => (
                Relation::SeveralCopies {
                    device: other.clone(),
                },
                None,
                None,
            ),
            (Ok(Some(x)), Ok(None)) => (Relation::OnlyHere, x.entries.last().cloned(), None),
            (Ok(None), Ok(Some(y))) => (Relation::OnlyThere, None, y.entries.last().cloned()),
            (Ok(None), Ok(None)) => continue,
            (Ok(Some(x)), Ok(Some(y))) => {
                let xs: Vec<&str> = x
                    .entries
                    .iter()
                    .map(|entry| entry.fingerprint.as_str())
                    .collect();
                let ys: Vec<&str> = y
                    .entries
                    .iter()
                    .map(|entry| entry.fingerprint.as_str())
                    .collect();
                (
                    relate(&xs, &ys),
                    x.entries.last().cloned(),
                    y.entries.last().cloned(),
                )
            }
        };
        let (needs_decision, proposal) = match &relation {
            Relation::Same => (
                false,
                "Nothing to do: both devices hold the same save.".to_string(),
            ),
            Relation::OtherAhead { new_contents } => (
                false,
                format!(
                    "{other} has continued this save ({new_contents} newer version(s)). Proposed: receive {other}'s current snapshot into this device's store. Nothing is put in place until you confirm a recovery."
                ),
            ),
            Relation::ThisAhead { new_contents } => (
                false,
                format!(
                    "This device has continued this save ({new_contents} newer version(s)). Proposed: offer this device's current snapshot to {other}."
                ),
            ),
            Relation::Diverged {
                only_here,
                only_there,
                ..
            } => (
                true,
                format!(
                    "Both devices played on separately ({only_here} version(s) here, {only_there} on {other}) from the same starting point. EmuWiz cannot merge saves or tell which has more progress. Keep both: receive {other}'s as a separate snapshot and choose in the game."
                ),
            ),
            Relation::Unrelated => (
                true,
                format!(
                    "Both devices have this save, with no shared history. They may be two separate playthroughs. Nothing will replace either; choose which to continue."
                ),
            ),
            Relation::OnlyHere => (
                false,
                format!("Only this device has this save. Proposed: offer it to {other}."),
            ),
            Relation::OnlyThere => (
                false,
                format!(
                    "Only {other} has this save. Proposed: receive it into this device's store."
                ),
            ),
            Relation::SeveralCopies { device } => (
                true,
                format!(
                    "{device} keeps this save in more than one place with different content. Decide which one is current there first."
                ),
            ),
        };
        out.push(SaveComparison {
            key: key.clone(),
            name,
            title,
            relation,
            here_current,
            there_current,
            needs_decision,
            proposal,
        });
    }
    out
}

fn relate(here: &[&str], there: &[&str]) -> Relation {
    let (Some(here_last), Some(there_last)) = (here.last(), there.last()) else {
        return Relation::Unrelated;
    };
    if here_last == there_last {
        return Relation::Same;
    }
    // The other device's current content is somewhere in this device's past
    // (or the reverse): one continued from the other. This also holds when
    // old history was pruned on one side.
    if let Some(at) = here
        .iter()
        .rposition(|fingerprint| fingerprint == there_last)
    {
        return Relation::ThisAhead {
            new_contents: here.len() - 1 - at,
        };
    }
    if let Some(at) = there
        .iter()
        .rposition(|fingerprint| fingerprint == here_last)
    {
        return Relation::OtherAhead {
            new_contents: there.len() - 1 - at,
        };
    }
    // The most recent content both have had.
    for (index, fingerprint) in here.iter().enumerate().rev() {
        if let Some(other_index) = there.iter().rposition(|candidate| candidate == fingerprint) {
            return Relation::Diverged {
                shared_fingerprint: (*fingerprint).to_string(),
                only_here: here.len() - 1 - index,
                only_there: there.len() - 1 - other_index,
            };
        }
    }
    Relation::Unrelated
}

/// A stand-in for the wire between two devices. Tests use it to cut a
/// transfer at any byte and to damage data in flight.
pub trait Link {
    /// Carries one chunk. `Err` is an interruption: the transfer stops and
    /// can be resumed later.
    fn carry(&mut self, chunk: &[u8]) -> Result<Vec<u8>, String>;
}

/// A link that never fails: a plain local copy.
pub struct DirectLink;

impl Link for DirectLink {
    fn carry(&mut self, chunk: &[u8]) -> Result<Vec<u8>, String> {
        Ok(chunk.to_vec())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TransferReport {
    pub snapshot_id: String,
    pub files_received: usize,
    pub files_already_present: usize,
    pub bytes_carried: u64,
    /// Bytes that did not need sending again after an earlier interruption.
    pub bytes_resumed: u64,
    pub checks: Vec<String>,
}

fn incoming_path(to: &SnapshotStore, sha256: &str) -> PathBuf {
    to.root().join("incoming").join(format!("{sha256}.part"))
}

/// Receives one snapshot named in an accepted list from `from` into `to`.
///
/// `sender_sequence_now` is what the sender says its counter is at the
/// start of the transfer; if it has moved past the list being acted on, the
/// list is stale and the transfer is refused before any byte moves.
///
/// Interrupted: returns [`ContinuityError::Io`], keeps what arrived, and the
/// same call later continues from there. Damaged in flight: the work file is
/// discarded and [`ContinuityError::Corrupt`] returned; nothing damaged ever
/// enters the store. Only when every file is in does the snapshot appear.
pub fn receive_snapshot(
    accepted: &SignedManifestSet,
    sender_sequence_now: u64,
    snapshot_id: &str,
    from: &SnapshotStore,
    to: &SnapshotStore,
    link: &mut dyn Link,
) -> Result<TransferReport, ContinuityError> {
    if sender_sequence_now != accepted.body.sequence {
        return Err(ContinuityError::Refused(format!(
            "the other device has changed since its list was read (list number {}, now {}). Compare again before transferring",
            accepted.body.sequence, sender_sequence_now
        )));
    }
    let manifest = accepted
        .body
        .snapshots
        .iter()
        .find(|manifest| manifest.snapshot_id == snapshot_id)
        .ok_or_else(|| {
            ContinuityError::NotFound(format!("snapshot {snapshot_id} in the other device's list"))
        })?;
    let incoming = to.root().join("incoming");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&incoming)
        .map_err(|error| super::io("cannot create", &incoming, error))?;
    let needed: u64 = manifest
        .body
        .files
        .iter()
        .filter(|file| !to.has_object(&file.sha256))
        .map(|file| file.size_bytes)
        .sum();
    let free = super::store::free_bytes(to.root())?;
    if free < needed.saturating_add(super::store::FREE_SPACE_RESERVE_BYTES) {
        return Err(ContinuityError::NoSpace(format!(
            "{free} bytes free, {needed} needed for this transfer"
        )));
    }
    let mut report = TransferReport {
        snapshot_id: snapshot_id.to_string(),
        ..TransferReport::default()
    };
    for file in &manifest.body.files {
        if to.has_object(&file.sha256) && to.read_object(&file.sha256).is_ok() {
            report.files_already_present += 1;
            continue;
        }
        // The sender reads (and so verifies) its own copy before sending.
        let source = from.read_object(&file.sha256)?;
        let part = incoming_path(to, &file.sha256);
        let mut have = match fs::symlink_metadata(&part) {
            Ok(meta) if meta.is_file() && meta.len() <= source.len() as u64 => meta.len() as usize,
            Ok(_) => {
                let _ = fs::remove_file(&part);
                0
            }
            Err(_) => 0,
        };
        report.bytes_resumed += have as u64;
        let mut out = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&part)
            .map_err(|error| super::io("cannot write", &part, error))?;
        while have < source.len() {
            let end = (have + TRANSFER_CHUNK_BYTES).min(source.len());
            let chunk = match link.carry(&source[have..end]) {
                Ok(chunk) => chunk,
                Err(why) => {
                    let _ = out.sync_all();
                    return Err(ContinuityError::Io(format!(
                        "transfer interrupted ({why}); {have} of {} bytes of {} are kept and will be continued",
                        source.len(),
                        file.name
                    )));
                }
            };
            if chunk.len() != end - have {
                let _ = fs::remove_file(&part);
                return Err(ContinuityError::Corrupt(format!(
                    "{} arrived with the wrong length; discarded",
                    file.name
                )));
            }
            out.write_all(&chunk)
                .map_err(|error| super::io("cannot write", &part, error))?;
            report.bytes_carried += chunk.len() as u64;
            have = end;
        }
        out.sync_all()
            .map_err(|error| super::io("cannot flush", &part, error))?;
        drop(out);
        let received = fs::read(&part).map_err(|error| super::io("cannot read", &part, error))?;
        if sha256_hex(&received) != file.sha256 {
            let _ = fs::remove_file(&part);
            return Err(ContinuityError::Corrupt(format!(
                "{} did not arrive intact (SHA-256 mismatch); what arrived was discarded and nothing entered the store",
                file.name
            )));
        }
        to.put_object(&received, &file.sha256)?;
        let _ = fs::remove_file(&part);
        report.files_received += 1;
    }
    to.put_manifest(manifest)?;
    let verified = to.verify_manifest(manifest);
    if !verified.intact() {
        return Err(ContinuityError::Corrupt(
            "the received snapshot does not verify".into(),
        ));
    }
    report.checks = vec![
        "the list was signed by the paired device and is its current one".to_string(),
        "every file matched its SHA-256 before it entered the store".to_string(),
        "the snapshot record matches its id".to_string(),
        "the received snapshot was verified again after it was stored".to_string(),
        "no save in any emulator was touched: receiving is not restoring".to_string(),
    ];
    Ok(report)
}
