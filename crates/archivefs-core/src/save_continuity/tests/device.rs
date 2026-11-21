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
        let mut ask = request(&self.device.identity.device_id, &[&self.card], self.clock);
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
    let mut ask = request(&device.device.identity.device_id, &[&device.card], 5);
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
        desktop.device.identity.device_id
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
