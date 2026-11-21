//! Two device profiles in two temporary folders. No sockets anywhere.

use super::super::ContinuityError;
use super::super::device::*;
use super::super::store::*;
use super::*;

struct Device {
    dir: tempfile::TempDir,
    device: LocalDevice,
    store: SnapshotStore,
    card: PathBuf,
    clock: i64,
}

impl Device {
    /// `clock` is this device's idea of the time; devices disagree.
    fn new(name: &str, clock: i64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let device = LocalDevice::open_or_create(&dir.path().join("continuity"), name).unwrap();
        let store = SnapshotStore::open(&dir.path().join("continuity")).unwrap();
        let card = dir.path().join("Mcd001.ps2");
        Self {
            dir,
            device,
            store,
            card,
            clock,
        }
    }

    /// Plays: the card now holds `saves` (key, content), and a snapshot is taken.
    fn play(&mut self, saves: &[(&str, &str)]) -> SnapshotManifest {
        let body: String = saves
            .iter()
            .map(|(key, content)| format!("{key}={content};"))
            .collect();
        fs::write(&self.card, body).unwrap();
        self.clock += 60;
        let mut ask = request(self.device.identity.history(), &[&self.card], self.clock);
        ask.contained_saves = saves
            .iter()
            .map(|(key, content)| save(key, content))
            .collect();
        self.store
            .capture(&ask, &CLOSED)
            .unwrap()
            .manifest()
            .clone()
    }

    fn peers(&self) -> PeerBook {
        PeerBook::open(&self.dir.path().join("continuity")).unwrap()
    }
}

fn paired() -> (Device, Device) {
    let (desktop, laptop) = (
        Device::new("Desktop", 1_800_000_000),
        Device::new("Laptop", 1_700_000_000),
    );
    desktop
        .peers()
        .pair(&laptop.device.identity, &desktop.device.identity)
        .unwrap();
    laptop
        .peers()
        .pair(&desktop.device.identity, &laptop.device.identity)
        .unwrap();
    (desktop, laptop)
}

fn relation(here: &Device, there: &Device, key: &str) -> SaveComparison {
    let (mine, theirs) = (
        here.device.export(&here.store).unwrap(),
        there.device.export(&there.store).unwrap(),
    );
    here.peers().accept(&theirs).unwrap();
    compare(&mine.body, &theirs.body)
        .into_iter()
        .find(|c| c.key == key)
        .unwrap()
}

const GAME: &str = "ps2:BASLUS-20827MANHUNT";

#[test]
fn a_device_keeps_one_identity_and_its_key_stays_private() {
    let dir = tempfile::tempdir().unwrap();
    let first = LocalDevice::open_or_create(dir.path(), "Desktop")
        .unwrap()
        .identity;
    let again = LocalDevice::open_or_create(dir.path(), "Desktop")
        .unwrap()
        .identity;
    assert_eq!(first, again);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(dir.path().join("device.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let other = LocalDevice::open_or_create(tempfile::tempdir().unwrap().path(), "Desktop")
        .unwrap()
        .identity;
    assert_ne!(
        first.device_id, other.device_id,
        "a name does not make an identity"
    );
}

#[test]
fn nothing_is_read_from_a_device_that_was_not_paired() {
    let (mut desktop, laptop) = (Device::new("Desktop", 0), Device::new("Laptop", 0));
    desktop.play(&[(GAME, "v1")]);
    let list = desktop.device.export(&desktop.store).unwrap();
    let refused = laptop.peers().accept(&list).unwrap_err();
    assert!(
        matches!(refused, ContinuityError::Refused(ref text) if text.contains("not paired")),
        "{refused}"
    );
    assert!(
        laptop
            .peers()
            .pair(&laptop.device.identity, &laptop.device.identity)
            .is_err(),
        "not with itself"
    );
}

#[test]
fn altered_forged_and_replayed_lists_are_refused() {
    let (mut desktop, laptop) = paired();
    desktop.play(&[(GAME, "v1")]);
    let genuine = desktop.device.export(&desktop.store).unwrap();

    // Altered in transit: one history entry changed.
    let mut altered = genuine.clone();
    altered.body.histories[0].entries[0].fingerprint = "v9".into();
    assert!(laptop.peers().accept(&altered).is_err());
    let mut altered = genuine.clone();
    altered.body.sequence += 5;
    assert!(laptop.peers().accept(&altered).is_err());

    // Forged: an attacker with their own key claims the desktop's name and id.
    let mut attacker = Device::new("Desktop", 0);
    attacker.play(&[(GAME, "evil")]);
    let mut forged = attacker.device.export(&attacker.store).unwrap();
    assert!(laptop.peers().accept(&forged).is_err(), "unknown key");
    forged.body.device = desktop.device.identity.clone();
    assert!(
        laptop.peers().accept(&forged).is_err(),
        "right identity, wrong signature"
    );
    // ...and cannot be re-paired over the real one.
    let mut imposter = attacker.device.identity.clone();
    imposter.device_id = desktop.device.identity.device_id.clone();
    assert!(
        laptop
            .peers()
            .pair(&imposter, &laptop.device.identity)
            .is_err()
    );

    // Replayed: an old genuine list after a newer one was accepted.
    laptop.peers().accept(&genuine).unwrap();
    desktop.play(&[(GAME, "v2")]);
    let newer = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&newer).unwrap();
    let stale = laptop.peers().accept(&genuine).unwrap_err();
    assert!(
        matches!(stale, ContinuityError::Refused(ref text) if text.contains("older")),
        "{stale}"
    );
}

#[test]
fn order_comes_from_history_not_from_clocks() {
    // The laptop's clock is years behind the desktop's.
    let (mut desktop, mut laptop) = paired();
    desktop.play(&[(GAME, "v1")]);
    laptop.play(&[(GAME, "v1")]);
    assert_eq!(relation(&desktop, &laptop, GAME).relation, Relation::Same);

    // The laptop plays on. By its clock that was long before the desktop's
    // last snapshot; it is still the one that is ahead.
    laptop.play(&[(GAME, "v2")]);
    laptop.play(&[(GAME, "v3")]);
    let seen_from_desktop = relation(&desktop, &laptop, GAME);
    assert_eq!(
        seen_from_desktop.relation,
        Relation::OtherAhead { new_contents: 2 }
    );
    assert!(!seen_from_desktop.needs_decision);
    assert!(
        seen_from_desktop.there_current.unwrap().captured_unix
            < seen_from_desktop.here_current.unwrap().captured_unix
    );
    assert_eq!(
        relation(&laptop, &desktop, GAME).relation,
        Relation::ThisAhead { new_contents: 2 }
    );
}

#[test]
fn playing_on_both_devices_is_a_conflict_for_a_person_never_newest_wins() {
    let (mut desktop, mut laptop) = paired();
    desktop.play(&[(GAME, "v1")]);
    laptop.play(&[(GAME, "v1")]);
    desktop.play(&[(GAME, "desktop-2")]);
    laptop.play(&[(GAME, "laptop-2")]);
    laptop.play(&[(GAME, "laptop-3")]);
    let conflict = relation(&desktop, &laptop, GAME);
    assert_eq!(
        conflict.relation,
        Relation::Diverged {
            shared_fingerprint: "v1".into(),
            only_here: 1,
            only_there: 2
        }
    );
    assert!(conflict.needs_decision);
    assert!(conflict.proposal.contains("Keep both"));

    // Two playthroughs that never shared anything.
    let (mut a, mut b) = paired();
    a.play(&[(GAME, "mine")]);
    b.play(&[(GAME, "theirs")]);
    let unrelated = relation(&a, &b, GAME);
    assert_eq!(unrelated.relation, Relation::Unrelated);
    assert!(unrelated.needs_decision);
}

#[test]
fn saves_on_one_device_only_and_other_saves_on_the_same_card_are_told_apart() {
    let (mut desktop, mut laptop) = paired();
    let other = "ps2:BASLUS-20814MaxPay2";
    desktop.play(&[(GAME, "v1")]);
    laptop.play(&[(GAME, "v1"), (other, "m1")]);
    // Max Payne is saved again on the laptop; Manhunt is untouched there.
    laptop.play(&[(GAME, "v1"), (other, "m2")]);
    assert_eq!(relation(&desktop, &laptop, GAME).relation, Relation::Same);
    assert_eq!(
        relation(&desktop, &laptop, other).relation,
        Relation::OnlyThere
    );
    assert_eq!(
        relation(&laptop, &desktop, other).relation,
        Relation::OnlyHere
    );
}

/// Cuts the transfer after a number of bytes, or flips a bit.
struct Faulty {
    budget: usize,
    corrupt: bool,
}

impl Link for Faulty {
    fn carry(&mut self, chunk: &[u8]) -> Result<Vec<u8>, String> {
        if self.budget < chunk.len() {
            return Err("link dropped".into());
        }
        self.budget -= chunk.len();
        let mut out = chunk.to_vec();
        if self.corrupt {
            out[0] ^= 0x80;
        }
        Ok(out)
    }
}

fn big_snapshot(device: &mut Device) -> SnapshotManifest {
    fs::write(&device.card, vec![0x5A; 300_000]).unwrap();
    let mut ask = request(device.device.identity.history(), &[&device.card], 5);
    ask.contained_saves = vec![save(GAME, "big")];
    device
        .store
        .capture(&ask, &CLOSED)
        .unwrap()
        .manifest()
        .clone()
}

#[test]
fn an_interrupted_transfer_resumes_and_nothing_appears_until_it_is_whole() {
    let (mut desktop, laptop) = paired();
    let snapshot = big_snapshot(&mut desktop);
    let list = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&list).unwrap();
    let laptop_before = tree(&laptop.dir.path().join("continuity/objects"));

    let mut link = Faulty {
        budget: 150_000,
        corrupt: false,
    };
    let interrupted = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &snapshot.snapshot_id,
        &desktop.store,
        &laptop.store,
        &mut link,
    )
    .unwrap_err();
    assert!(
        matches!(interrupted, ContinuityError::Io(ref text) if text.contains("interrupted")),
        "{interrupted}"
    );
    assert!(laptop.store.list().is_empty(), "no snapshot is visible yet");
    assert_eq!(
        tree(&laptop.dir.path().join("continuity/objects")),
        laptop_before,
        "nothing entered the store"
    );

    let report = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &snapshot.snapshot_id,
        &desktop.store,
        &laptop.store,
        &mut DirectLink,
    )
    .unwrap();
    assert!(report.bytes_resumed >= 128 * 1024, "{report:?}");
    assert_eq!(report.bytes_resumed + report.bytes_carried, 300_000);
    assert!(laptop.store.verify(&snapshot.snapshot_id).unwrap().intact());
    // Received under the sender's name; the laptop's own history is separate.
    assert_eq!(
        laptop
            .store
            .get(&snapshot.snapshot_id)
            .unwrap()
            .body
            .provenance
            .device_id,
        desktop.device.identity.history()
    );
    assert!(
        laptop
            .device
            .export(&laptop.store)
            .unwrap()
            .body
            .snapshots
            .is_empty(),
        "a received snapshot is not re-signed as the laptop's"
    );
    // Receiving it again carries nothing.
    let again = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &snapshot.snapshot_id,
        &desktop.store,
        &laptop.store,
        &mut DirectLink,
    )
    .unwrap();
    assert_eq!(
        (
            again.files_received,
            again.files_already_present,
            again.bytes_carried
        ),
        (0, 1, 0)
    );
}

#[test]
fn data_damaged_in_flight_never_enters_the_store() {
    let (mut desktop, laptop) = paired();
    let snapshot = big_snapshot(&mut desktop);
    let list = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&list).unwrap();
    let mut link = Faulty {
        budget: usize::MAX,
        corrupt: true,
    };
    let damaged = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &snapshot.snapshot_id,
        &desktop.store,
        &laptop.store,
        &mut link,
    )
    .unwrap_err();
    assert!(matches!(damaged, ContinuityError::Corrupt(_)), "{damaged}");
    assert!(laptop.store.list().is_empty());
    assert_eq!(laptop.store.usage().objects, 0);
    assert!(
        fs::read_dir(laptop.store.root().join("incoming"))
            .unwrap()
            .next()
            .is_none(),
        "the damaged work file is gone"
    );
    // A clean retry then works from the beginning.
    receive_snapshot(
        &list,
        desktop.device.sequence(),
        &snapshot.snapshot_id,
        &desktop.store,
        &laptop.store,
        &mut DirectLink,
    )
    .unwrap();
}

#[test]
fn a_list_the_sender_has_moved_past_is_stale_and_not_acted_on() {
    let (mut desktop, laptop) = paired();
    let snapshot = big_snapshot(&mut desktop);
    let list = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&list).unwrap();
    desktop.play(&[(GAME, "newer")]);
    desktop.device.export(&desktop.store).unwrap(); // the desktop has since signed a newer list
    let stale = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &snapshot.snapshot_id,
        &desktop.store,
        &laptop.store,
        &mut DirectLink,
    )
    .unwrap_err();
    assert!(
        matches!(stale, ContinuityError::Refused(ref text) if text.contains("Compare again")),
        "{stale}"
    );
    assert!(laptop.store.list().is_empty());
}

#[test]
fn a_sender_whose_own_copy_is_damaged_sends_nothing() {
    let (mut desktop, laptop) = paired();
    let snapshot = big_snapshot(&mut desktop);
    let list = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&list).unwrap();
    let sha = &snapshot.body.files[0].sha256;
    let object = desktop
        .store
        .root()
        .join("objects")
        .join(&sha[..2])
        .join(sha);
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&object, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&object, b"rotted").unwrap();
    let refused = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &snapshot.snapshot_id,
        &desktop.store,
        &laptop.store,
        &mut DirectLink,
    )
    .unwrap_err();
    assert!(matches!(refused, ContinuityError::Corrupt(_)), "{refused}");
    assert!(laptop.store.list().is_empty());
}

#[test]
fn receiving_changes_no_save_on_the_receiving_device() {
    let (mut desktop, mut laptop) = paired();
    desktop.play(&[(GAME, "v1")]);
    laptop.play(&[(GAME, "laptop")]);
    let laptop_card = fs::read(&laptop.card).unwrap();
    let list = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&list).unwrap();
    let id = list.body.snapshots[0].snapshot_id.clone();
    let report = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &id,
        &desktop.store,
        &laptop.store,
        &mut DirectLink,
    )
    .unwrap();
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.contains("receiving is not restoring"))
    );
    assert_eq!(fs::read(&laptop.card).unwrap(), laptop_card);
    // Both histories are now on the laptop, side by side, for a person to choose.
    assert_eq!(laptop.store.list().len(), 2);
}

// ---- Regression tests for the independent review of 2026-11-21 ----

#[test]
fn a_paired_device_cannot_put_records_into_this_devices_histories() {
    let (mut desktop, mut laptop) = paired();
    let ours = laptop.play(&[(GAME, "laptop")]);
    // The desktop (paired, but hostile) signs a list holding a record that
    // claims the laptop's history key.
    desktop.play(&[(GAME, "v1")]);
    let mut list = desktop.device.export(&desktop.store).unwrap();
    let mut body = list.body.snapshots[0].body.clone();
    body.lineage = ours.body.lineage.clone();
    body.captured_unix = 4_000_000_000;
    let forged = SnapshotManifest {
        format_version: 1,
        snapshot_id: body_id(&body),
        body,
    };
    list.body.snapshots = vec![forged];
    // Even if it could sign this (it can: it is its own list), it is refused.
    assert!(laptop.peers().accept(&list).is_err());
    assert_eq!(laptop.store.list_lineage(&ours.body.lineage).len(), 1);
}

#[test]
fn a_second_different_list_with_the_same_number_is_refused_and_the_same_one_is_fine() {
    let (mut desktop, laptop) = paired();
    desktop.play(&[(GAME, "v1")]);
    let first = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&first).unwrap();
    laptop.peers().accept(&first).unwrap(); // the very same list again: harmless
    // The desktop's counter goes back (a power cut, a restored backup) and it
    // signs something else under the same number.
    let continuity = desktop.dir.path().join("continuity/device.json");
    let reverted = fs::read_to_string(&continuity)
        .unwrap()
        .replace("\"sequence\": 1", "\"sequence\": 0");
    fs::write(&continuity, reverted).unwrap();
    desktop.play(&[(GAME, "v2")]);
    let second = desktop.device.export(&desktop.store).unwrap();
    assert_eq!(second.body.sequence, first.body.sequence);
    let refused = laptop.peers().accept(&second).unwrap_err();
    assert!(
        matches!(refused, ContinuityError::Refused(ref text) if text.contains("same number")),
        "{refused}"
    );
}

#[test]
fn checking_a_list_changes_nothing_and_comparing_does_not_use_up_a_number() {
    let (mut desktop, mut laptop) = paired();
    desktop.play(&[(GAME, "v1")]);
    laptop.play(&[(GAME, "v1")]);
    let exported_earlier = laptop.device.export(&laptop.store).unwrap();
    let theirs = desktop.device.export(&desktop.store).unwrap();
    let before = tree(&laptop.dir.path().join("continuity"));
    laptop.peers().check(&theirs).unwrap();
    let mine = laptop.device.current_histories(&laptop.store);
    assert_eq!(compare(&mine, &theirs.body)[0].relation, Relation::Same);
    assert_eq!(
        tree(&laptop.dir.path().join("continuity")),
        before,
        "nothing written by looking"
    );
    assert_eq!(
        laptop.device.sequence(),
        exported_earlier.body.sequence,
        "the earlier list is still current"
    );
}

#[test]
fn a_pairing_file_that_cannot_be_read_is_an_error_not_an_empty_book() {
    let (desktop, laptop) = paired();
    let file = laptop.dir.path().join("continuity/peers.json");
    let good = fs::read(&file).unwrap();
    fs::write(&file, b"{ damaged").unwrap();
    assert!(matches!(
        PeerBook::open(&laptop.dir.path().join("continuity")),
        Err(ContinuityError::Corrupt(_))
    ));
    assert_eq!(
        fs::read(&file).unwrap(),
        b"{ damaged",
        "left exactly as found"
    );
    fs::write(&file, good).unwrap();
    assert!(laptop.peers().is_paired(&desktop.device.identity.device_id));
}

#[test]
fn a_transfer_whose_content_is_not_the_stated_size_is_refused_before_anything_is_kept() {
    let (mut desktop, laptop) = paired();
    let snapshot = big_snapshot(&mut desktop);
    let mut list = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&list).unwrap();
    // The list (as the receiver holds it) is altered to state another size.
    let mut body = snapshot.body.clone();
    body.files[0].size_bytes = 1;
    let lying = SnapshotManifest {
        format_version: 1,
        snapshot_id: body_id(&body),
        body,
    };
    list.body.snapshots = vec![lying.clone()];
    let refused = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &lying.snapshot_id,
        &desktop.store,
        &laptop.store,
        &mut DirectLink,
    )
    .unwrap_err();
    assert!(matches!(refused, ContinuityError::Refused(_)), "{refused}");
    assert!(laptop.store.list().is_empty());
    assert_eq!(laptop.store.usage().objects, 0);
}

#[test]
fn receiving_respects_this_stores_own_size_limit() {
    let (mut desktop, laptop) = paired();
    let snapshot = big_snapshot(&mut desktop); // 300,000 bytes
    let list = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().accept(&list).unwrap();
    let small = SnapshotStore::open_with_budget(&laptop.dir.path().join("small"), 100_000).unwrap();
    let refused = receive_snapshot(
        &list,
        desktop.device.sequence(),
        &snapshot.snapshot_id,
        &desktop.store,
        &small,
        &mut DirectLink,
    )
    .unwrap_err();
    assert!(matches!(refused, ContinuityError::NoSpace(_)), "{refused}");
    assert!(small.list().is_empty());
}

#[test]
fn histories_that_disagree_about_the_order_are_a_conflict() {
    let (mut desktop, mut laptop) = paired();
    desktop.play(&[(GAME, "A")]);
    desktop.play(&[(GAME, "B")]);
    laptop.play(&[(GAME, "B")]);
    laptop.play(&[(GAME, "A")]);
    let seen = relation(&desktop, &laptop, GAME);
    assert!(
        matches!(seen.relation, Relation::Diverged { .. }),
        "{:?}",
        seen.relation
    );
    assert!(seen.needs_decision);
}

#[test]
fn two_first_runs_end_up_with_one_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let root = root.clone();
            std::thread::spawn(move || {
                LocalDevice::open_or_create(&root, "Desktop")
                    .unwrap()
                    .identity
                    .device_id
            })
        })
        .collect();
    let ids: std::collections::BTreeSet<String> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(ids.len(), 1, "every start uses the key that is on disk");
    assert_eq!(
        LocalDevice::open_or_create(&root, "Desktop")
            .unwrap()
            .identity
            .device_id,
        *ids.iter().next().unwrap()
    );
}

// ---- Regression tests for the delta review of cdc75795 ----

#[test]
fn opening_an_existing_device_writes_nothing_and_an_unreadable_record_is_not_list_number_zero() {
    let (mut desktop, _laptop) = paired();
    desktop.play(&[(GAME, "v1")]);
    desktop.device.export(&desktop.store).unwrap();
    desktop.device.export(&desktop.store).unwrap();
    let root = desktop.dir.path().join("continuity");
    assert_eq!(LocalDevice::recorded_sequence(&root).unwrap(), 2);

    // No device record: opening the existing device must not write one.
    let record = root.join("device.json");
    let saved = fs::read(&record).unwrap();
    fs::remove_file(&record).unwrap();
    let before = tree(&root);
    let opened = LocalDevice::open_existing(&root)
        .unwrap()
        .expect("the key is there");
    assert_eq!(opened.identity.device_id, desktop.device.identity.device_id);
    assert_eq!(tree(&root), before, "nothing was created");
    assert_eq!(
        LocalDevice::recorded_sequence(&root).unwrap(),
        0,
        "no record yet: nothing signed"
    );

    // A record that is there but damaged: an error, and no list is signed.
    fs::write(&record, b"{ damaged").unwrap();
    assert!(matches!(
        LocalDevice::recorded_sequence(&root),
        Err(ContinuityError::Corrupt(_))
    ));
    assert!(
        desktop.device.export(&desktop.store).is_err(),
        "refuses to sign under a guessed number"
    );
    assert_eq!(
        fs::read(&record).unwrap(),
        b"{ damaged",
        "left exactly as found"
    );
    fs::write(&record, saved).unwrap();
    assert_eq!(
        desktop.device.export(&desktop.store).unwrap().body.sequence,
        3
    );

    // A folder with no device at all: nothing to open, nothing created.
    let empty = tempfile::tempdir().unwrap();
    assert!(LocalDevice::open_existing(empty.path()).unwrap().is_none());
    assert!(fs::read_dir(empty.path()).unwrap().next().is_none());
}

// ---- The local installation id and the signing key (owner decision 2026-11-21) ----

use super::super::installation;

fn files_under(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.metadata().unwrap().is_dir() {
                stack.push(path);
            } else {
                found.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

/// Takes a snapshot of a one-file "card" holding `content`, under `id`.
fn snapshot_under(
    store: &SnapshotStore,
    card: &Path,
    id: &str,
    content: &str,
    time: i64,
) -> SnapshotManifest {
    fs::write(card, format!("{GAME}={content};")).unwrap();
    let mut ask = request(id, &[card], time);
    ask.contained_saves = vec![save(GAME, content)];
    store.capture(&ask, &CLOSED).unwrap().manifest().clone()
}

#[test]
fn a_fresh_installation_gets_one_local_id_and_no_key_and_keeps_it_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("continuity");
    // Looking first creates nothing.
    assert_eq!(installation::existing(&root).unwrap(), None);
    assert!(!root.exists());
    let id = installation::open_or_create(&root).unwrap();
    assert!(installation::well_formed(&id), "{id}");
    // No signing key, no device record.
    assert!(!root.join("device.key").exists() && !root.join("device.json").exists());
    // "Restarts": every later look gives the same id and writes nothing.
    let before = files_under(&root);
    for _ in 0..3 {
        assert_eq!(installation::open_or_create(&root).unwrap(), id);
        assert_eq!(installation::existing(&root).unwrap(), Some(id.clone()));
    }
    assert_eq!(files_under(&root), before);
    // Two different installations do not share an id.
    let other = tempfile::tempdir().unwrap();
    assert_ne!(
        installation::open_or_create(&other.path().join("c")).unwrap(),
        id
    );
    // A damaged id file is an error, never quietly replaced by a new id
    // (which would split every history).
    fs::write(root.join("installation.json"), b"{ not json").unwrap();
    assert!(installation::open_or_create(&root).is_err());
    assert!(installation::existing(&root).is_err());
    assert_eq!(
        fs::read(root.join("installation.json")).unwrap(),
        b"{ not json"
    );
    fs::write(
        root.join("installation.json"),
        br#"{"format_version":1,"installation_id":"short","origin":"generated"}"#,
    )
    .unwrap();
    assert!(installation::open_or_create(&root).is_err());
}

#[test]
fn first_use_of_two_devices_joins_the_key_to_the_existing_history_without_splitting_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("continuity");
    let card = dir.path().join("Mcd001.ps2");
    // Weeks of ordinary use: sessions and snapshots, no two-device feature.
    let local = installation::open_or_create(&root).unwrap();
    let store = SnapshotStore::open(&root).unwrap();
    let first = snapshot_under(&store, &card, &local, "v1", 100);
    let second = snapshot_under(&store, &card, &local, "v2", 200);
    assert!(
        !root.join("device.key").exists(),
        "no key until two-device use"
    );
    let records_before = files_under(&root.join("manifests"));

    // The person turns on two-device use.
    let desktop = LocalDevice::open_or_create(&root, "Desktop").unwrap();
    assert!(root.join("device.key").exists());
    // The key speaks for the existing local history...
    assert_eq!(desktop.identity.history(), local);
    assert_eq!(desktop.identity.history_id.as_deref(), Some(local.as_str()));
    assert_ne!(desktop.identity.device_id, local);
    // ...the local id did not change, and no stored record was rewritten.
    assert_eq!(installation::open_or_create(&root).unwrap(), local);
    assert_eq!(files_under(&root.join("manifests")), records_before);

    // A snapshot after activation continues the same history.
    let third = snapshot_under(&store, &card, &local, "v3", 300);
    assert_eq!(first.body.lineage, second.body.lineage);
    assert_eq!(first.body.lineage, third.body.lineage);
    let timelines = super::super::workflow::timelines(&store);
    assert_eq!(timelines.len(), 1, "one history, not two");
    assert_eq!(timelines[0].entries.len(), 3);

    // The signed list carries everything, old and new, once each.
    let list = desktop.export(&store).unwrap();
    let ids: Vec<&str> = list
        .body
        .snapshots
        .iter()
        .map(|m| m.snapshot_id.as_str())
        .collect();
    assert_eq!(ids.len(), 3);
    for snapshot in [&first, &second, &third] {
        assert!(ids.contains(&snapshot.snapshot_id.as_str()));
    }
    assert_eq!(list.body.histories.len(), 1);

    // Another device pairs with it and accepts the list.
    let laptop = Device::new("Laptop", 1_700_000_000);
    laptop
        .peers()
        .pair(&desktop.identity, &laptop.device.identity)
        .unwrap();
    laptop.peers().accept(&list).unwrap();
    // Opening again later ("restart") gives the same identity and history.
    let again = LocalDevice::open_existing(&root).unwrap().unwrap();
    assert_eq!(again.identity, desktop.identity);
}

#[test]
fn a_profile_from_before_installation_ids_keeps_its_history_under_the_id_it_always_had() {
    // Such a profile has a key and records under the key's id, and no
    // installation file.
    let mut old = Device::new("Old desktop", 1_800_000_000);
    let root = old.dir.path().join("continuity");
    fs::remove_file(root.join("installation.json")).unwrap();
    // Its device record was written before the field existed.
    let record_path = root.join("device.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert!(
        record["identity"]
            .as_object_mut()
            .unwrap()
            .remove("history_id")
            .is_some()
    );
    fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let key_id = old.device.identity.device_id.clone();
    let before_change = snapshot_under(&old.store, &old.card, &key_id, "v1", 100);
    let records_before = files_under(&root.join("manifests"));

    // The new code, looking without creating: the id is the key's.
    assert_eq!(installation::existing(&root).unwrap(), Some(key_id.clone()));
    assert!(!root.join("installation.json").exists());
    // First recording after the change adopts it and writes it down.
    assert_eq!(installation::open_or_create(&root).unwrap(), key_id);
    let stored = fs::read_to_string(root.join("installation.json")).unwrap();
    assert!(stored.contains("adopted_from_device_key"), "{stored}");
    let after_change = snapshot_under(&old.store, &old.card, &key_id, "v2", 200);
    assert_eq!(before_change.body.lineage, after_change.body.lineage);
    assert_eq!(super::super::workflow::timelines(&old.store).len(), 1);
    // Nothing recorded earlier was rewritten.
    for (path, bytes) in &records_before {
        assert_eq!(&fs::read(path).unwrap(), bytes);
    }

    // Its identity declares nothing extra, so its lists are what they
    // always were, and a device that paired with it before still accepts
    // them.
    old.device = LocalDevice::open_existing(&root).unwrap().unwrap();
    assert_eq!(old.device.identity.history_id, None);
    assert_eq!(old.device.identity.history(), key_id);
    let as_json = serde_json::to_string(&old.device.identity).unwrap();
    assert!(!as_json.contains("history_id"), "{as_json}");
    let laptop = Device::new("Laptop", 1_700_000_000);
    laptop
        .peers()
        .pair(&old.device.identity, &laptop.device.identity)
        .unwrap();
    let list = old.device.export(&old.store).unwrap();
    assert_eq!(list.body.snapshots.len(), 2);
    laptop.peers().accept(&list).unwrap();
    // An identity written before the field existed still reads.
    let legacy: DeviceIdentity = serde_json::from_str(&as_json).unwrap();
    assert_eq!(legacy, old.device.identity);
}

/// A device whose installation file claims `history`.
fn device_claiming(history: &str, name: &str) -> Device {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("continuity");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("installation.json"),
        format!(r#"{{"format_version":1,"installation_id":"{history}","origin":"generated"}}"#),
    )
    .unwrap();
    let device = LocalDevice::open_or_create(&root, name).unwrap();
    let store = SnapshotStore::open(&root).unwrap();
    let card = dir.path().join("Mcd001.ps2");
    Device {
        dir,
        device,
        store,
        card,
        clock: 1_600_000_000,
    }
}

#[test]
fn a_device_cannot_claim_this_installations_history_or_another_paired_devices() {
    let (desktop, laptop) = paired();
    // A device that says it speaks for the laptop's own local history.
    let thief = device_claiming(laptop.device.identity.history(), "Thief");
    assert!(
        laptop
            .peers()
            .pair(&thief.device.identity, &laptop.device.identity)
            .is_err()
    );
    // Or for the laptop's key id.
    let thief = device_claiming(&laptop.device.identity.device_id, "Thief");
    assert!(
        laptop
            .peers()
            .pair(&thief.device.identity, &laptop.device.identity)
            .is_err()
    );
    // Or for the history of a device the laptop is already paired with.
    let thief = device_claiming(desktop.device.identity.history(), "Thief");
    assert!(
        laptop
            .peers()
            .pair(&thief.device.identity, &laptop.device.identity)
            .is_err()
    );
    let thief = device_claiming(&desktop.device.identity.device_id, "Thief");
    assert!(
        laptop
            .peers()
            .pair(&thief.device.identity, &laptop.device.identity)
            .is_err()
    );
    assert!(!laptop.peers().is_paired(&thief.device.identity.device_id));
    // A malformed claim is refused outright.
    let mut odd = desktop.device.identity.clone();
    odd.history_id = Some("not-an-id".into());
    let stranger = Device::new("Stranger", 1);
    assert!(
        stranger
            .peers()
            .pair(&odd, &stranger.device.identity)
            .is_err()
    );
}

#[test]
fn the_history_a_device_speaks_for_is_pinned_when_it_is_paired() {
    let (mut desktop, laptop) = paired();
    desktop.play(&[(GAME, "v1")]);
    let genuine = desktop.device.export(&desktop.store).unwrap();
    laptop.peers().check(&genuine).unwrap();
    // The pairing code covers the history as well as the key.
    let mut other_history = desktop.device.identity.clone();
    other_history.history_id = Some("0123456789abcdef0123456789abcdef".into());
    assert_ne!(
        other_history.fingerprint(),
        desktop.device.identity.fingerprint()
    );

    // The same key later claiming a different history (its own profile's id
    // file was swapped): its lists are refused, and it cannot re-pair.
    let root = desktop.dir.path().join("continuity");
    fs::write(
        root.join("installation.json"),
        br#"{"format_version":1,"installation_id":"0123456789abcdef0123456789abcdef","origin":"generated"}"#,
    )
    .unwrap();
    let changed = LocalDevice::open_existing(&root).unwrap().unwrap();
    assert_eq!(
        changed.identity.device_id,
        desktop.device.identity.device_id
    );
    let list = changed.export(&desktop.store).unwrap();
    let refused = laptop.peers().check(&list).unwrap_err();
    assert!(
        matches!(refused, ContinuityError::Refused(ref why) if why.contains("different history")),
        "{refused:?}"
    );
    assert!(
        laptop
            .peers()
            .pair(&changed.identity, &laptop.device.identity)
            .is_err()
    );

    // A list holding a snapshot recorded under the sender's key id instead
    // of the history it speaks for is not the sender's own.
    let (mut desktop, laptop) = paired();
    let key_id = desktop.device.identity.device_id.clone();
    snapshot_under(&desktop.store, &desktop.card, &key_id, "v1", 50);
    desktop.play(&[(GAME, "v2")]);
    let mut list = desktop.device.export(&desktop.store).unwrap();
    assert_eq!(
        list.body.snapshots.len(),
        1,
        "only its own history is exported"
    );
    let stray = desktop
        .store
        .list()
        .into_iter()
        .find(|m| m.body.provenance.device_id == key_id)
        .unwrap();
    list.body.snapshots.push(stray);
    assert!(laptop.peers().check(&list).is_err());
}

#[test]
fn a_lost_id_file_is_never_replaced_by_a_new_id_while_records_exist() {
    // Sessions recorded, no key, and the id file disappears.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("continuity");
    let id = installation::open_or_create(&root).unwrap();
    fs::create_dir_all(root.join("sessions")).unwrap();
    fs::write(root.join("sessions/one.json"), b"{}").unwrap();
    fs::remove_file(root.join("installation.json")).unwrap();
    let refused = installation::open_or_create(&root).unwrap_err();
    assert!(
        matches!(refused, ContinuityError::Corrupt(ref why) if why.contains("split their history")),
        "{refused:?}"
    );
    assert!(!root.join("installation.json").exists());
    assert_eq!(installation::existing(&root).unwrap(), None);

    // With two-device use switched on, the device record still names the
    // id, and it is recovered from there instead of the key's own id.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("continuity");
    let id2 = installation::open_or_create(&root).unwrap();
    let device = LocalDevice::open_or_create(&root, "Desktop").unwrap();
    assert_ne!(device.identity.device_id, id2);
    fs::remove_file(root.join("installation.json")).unwrap();
    assert_eq!(installation::existing(&root).unwrap(), Some(id2.clone()));
    assert_eq!(installation::open_or_create(&root).unwrap(), id2);
    let stored = fs::read_to_string(root.join("installation.json")).unwrap();
    assert!(stored.contains("recovered_from_device_record"), "{stored}");
    assert_eq!(
        LocalDevice::open_existing(&root).unwrap().unwrap().identity,
        device.identity
    );
    assert_ne!(id, id2);
}
