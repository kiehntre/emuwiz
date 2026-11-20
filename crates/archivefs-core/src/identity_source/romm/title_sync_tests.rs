//! Rename → RomM title update → Undo, against a fake RomM that behaves like
//! RomM 5.3.1's `update_rom` for a name-only form (only `name` changes).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::identity_source::cache::{CACHE_FORMAT_VERSION, IdentityCache};
use crate::identity_source::model::IdentityProvider;
use crate::identity_source::net_policy::StaticResolver;
use crate::identity_source::path_map::{PathMapping, ProviderPathKind};
use crate::identity_source::romm::client::{RommHttpResponse, RommRequestError, RommTransport};
use crate::identity_source::romm::config::{RommSourceConfig, RommToken, ValidatedRommSource};
use crate::identity_source::romm::normalise::{NormalisationReport, normalise_rom};

/// A RomM holding one entry. GET returns it; a name-only PUT changes only
/// its `name` (and `name_sort_key`), like RomM 5.3.1.
#[derive(Default)]
struct FakeRomm {
    rom: Mutex<Option<Value>>,
    offline: bool,
    put_status: Option<u16>,
    puts: Mutex<Vec<String>>,
}

impl FakeRomm {
    fn holding(rom: Value) -> Self {
        Self {
            rom: Mutex::new(Some(rom)),
            ..Self::default()
        }
    }
    fn rom(&self) -> Value {
        self.rom.lock().unwrap().clone().expect("present")
    }
    fn edit(&self, field: &str, value: Value) {
        self.rom.lock().unwrap().as_mut().unwrap()[field] = value;
    }
    fn puts(&self) -> Vec<String> {
        self.puts.lock().unwrap().clone()
    }
}

impl RommTransport for FakeRomm {
    fn get(
        &self,
        url: &str,
        authorization: Option<&str>,
        _max_bytes: usize,
        _timeout: Duration,
    ) -> Result<RommHttpResponse, RommRequestError> {
        assert!(authorization.is_some_and(|value| value.starts_with("Bearer ")));
        if self.offline {
            return Err(RommRequestError::Transport {
                detail: "the connection failed".into(),
            });
        }
        let rom = self.rom.lock().unwrap().clone();
        let found = rom.filter(|rom| url.ends_with(&format!("/api/roms/{}", rom["id"])));
        Ok(match found {
            Some(rom) => RommHttpResponse {
                status: 200,
                body: rom.to_string().into_bytes(),
                location: None,
            },
            None => RommHttpResponse {
                status: 404,
                body: Vec::new(),
                location: None,
            },
        })
    }
}

impl RommTitleTransport for FakeRomm {
    fn put_form(
        &self,
        url: &str,
        authorization: &str,
        content_type: &str,
        body: &[u8],
        _max_bytes: usize,
        _timeout: Duration,
    ) -> Result<RommHttpResponse, RommRequestError> {
        assert!(authorization.starts_with("Bearer "));
        assert!(content_type.starts_with("multipart/form-data; boundary="));
        let body = String::from_utf8(body.to_vec()).unwrap();
        // Exactly one form field, `name`.
        assert_eq!(body.matches("Content-Disposition").count(), 1, "{body}");
        assert!(body.contains("name=\"name\""), "{body}");
        if let Some(status) = self.put_status {
            return Ok(RommHttpResponse {
                status,
                body: Vec::new(),
                location: None,
            });
        }
        let get = self.get(url, Some(authorization), 0, Duration::ZERO)?;
        if get.status != 200 {
            return Ok(get);
        }
        let name = body
            .split("\r\n\r\n")
            .nth(1)
            .and_then(|rest| rest.split("\r\n--").next())
            .unwrap()
            .to_string();
        self.puts.lock().unwrap().push(name.clone());
        self.edit("name_sort_key", json!(name.to_lowercase()));
        self.edit("name", json!(name));
        Ok(RommHttpResponse {
            status: 200,
            body: self.rom().to_string().into_bytes(),
            location: None,
        })
    }
}

struct Setup {
    _temp: tempfile::TempDir,
    games: PathBuf,
    records: PathBuf,
    source: ValidatedRommSource,
}

fn setup() -> Setup {
    let temp = tempfile::tempdir().unwrap();
    let games = temp.path().join("gba");
    std::fs::create_dir_all(&games).unwrap();
    let config = RommSourceConfig {
        enabled: true,
        url: "http://172.19.0.20:8080".into(),
        mappings: vec![PathMapping {
            provider_prefix: "roms/gba".into(),
            archivefs_prefix: games.clone(),
            provider_aliases: Vec::new(),
        }],
        media_mapping: None,
        provider_path_kind: ProviderPathKind::ProviderRelative,
        token_path: None,
    };
    let source = ValidatedRommSource::validate(
        &config,
        &RommToken::parse("rmm_sandbox_title_sync_test").unwrap(),
        &[temp.path().to_path_buf()],
        &StaticResolver::new(),
    )
    .expect("validates");
    Setup {
        records: temp.path().join("romm").join(RECORD_FOLDER),
        games,
        source,
        _temp: temp,
    }
}

/// RomM's entry for `file`, never identified, title derived from the name.
fn romm_entry(file: &str) -> Value {
    json!({
        "id": 4, "platform_id": 3, "platform_slug": "gba",
        "fs_name": file, "fs_name_no_tags": romm_derived_title(file),
        "fs_path": "roms/gba", "full_path": format!("roms/gba/{file}"),
        "fs_size_bytes": 12,
        "name": romm_derived_title(file), "name_sort_key": romm_derived_title(file).to_lowercase(),
        "is_identified": false, "is_unidentified": true,
        "igdb_id": null, "ss_id": null,
        "has_simple_single_file": true,
        "files": [{"file_name": file}],
        "sha1_hash": "ae0d6b471f819f4993edbf261664962afbaa579a",
        "path_cover_large": "roms/3/4/cover/big.png", "summary": "kept",
    })
}

fn cache_of(setup: &Setup, entries: &[Value]) -> IdentityCache {
    IdentityCache {
        format_version: CACHE_FORMAT_VERSION,
        provider: IdentityProvider::Romm,
        server_id: setup.source.server_id().to_string(),
        server_version: Some("5.3.1".into()),
        source_fingerprint: String::new(),
        imported_at_unix_seconds: 0,
        platforms: Vec::new(),
        records: entries
            .iter()
            .map(|entry| {
                normalise_rom(
                    entry,
                    setup.source.server_id(),
                    setup.source.mappings(),
                    0,
                    &mut NormalisationReport::default(),
                )
                .unwrap()
            })
            .collect(),
        rejected_hashes: Vec::new(),
        unknown_platforms: Vec::new(),
        server_reported_total: None,
    }
}

/// Renames `from` to `to` in the games folder, as Rename would.
fn rename(setup: &Setup, from: &str, to: &str) -> (PathBuf, PathBuf) {
    let old = setup.games.join(from);
    let new = setup.games.join(to);
    if !old.exists() {
        std::fs::write(&old, b"GBA rom data").unwrap();
    }
    std::fs::rename(&old, &new).unwrap();
    (old, new)
}

fn update(
    setup: &Setup,
    romm: &FakeRomm,
    cache: &IdentityCache,
    tx: &str,
    paths: &(PathBuf, PathBuf),
) -> Result<TitleSyncOutcome, String> {
    update_title_after_rename(
        &setup.source,
        romm,
        cache,
        &setup.records,
        tx,
        &paths.0,
        &paths.1,
        Ok(&[]),
        &AtomicBool::new(false),
    )
}

fn undo(setup: &Setup, romm: &FakeRomm, tx: &str) -> Result<Option<TitleSyncOutcome>, String> {
    restore_title_after_undo(
        &setup.source,
        romm,
        &setup.records,
        tx,
        &AtomicBool::new(false),
    )
}

fn left_alone(outcome: Result<TitleSyncOutcome, String>, expect: &str) {
    match outcome {
        Ok(TitleSyncOutcome::LeftAlone(reason)) => assert!(reason.contains(expect), "{reason}"),
        other => panic!("expected it left alone ({expect}), got {other:?}"),
    }
}

#[test]
fn romm_titles_are_derived_like_romm_does() {
    for (file, title) in [
        ("Zelda (USA).gba", "Zelda"),
        ("Zelda (USA) (Rev 1) [!].gba", "Zelda"),
        ("Super Mario Bros. 3 (USA).nes", "Super Mario Bros. 3"),
        ("Dr. Mario (Japan, USA).nes", "Dr. Mario"),
        ("Pac-Man Vol.nes", "Pac-Man Vol"),
        ("Game (Disc 1) Extra (USA).iso", "Game (Disc 1) Extra"),
        ("Archive.tar.gz", "Archive"),
        (
            "EmuWiz Synthetic Fixture (USA).gba",
            "EmuWiz Synthetic Fixture",
        ),
    ] {
        assert_eq!(romm_derived_title(file), title, "{file}");
    }
}

#[test]
fn a_rename_changes_only_the_romm_title_and_undo_puts_it_back() {
    let setup = setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    let before = romm.rom();
    let paths = rename(&setup, "Zelda (USA).gba", "Legend of Zelda (Europe).gba");

    let outcome = update(&setup, &romm, &cache, "tx-1", &paths).unwrap();
    assert_eq!(
        outcome,
        TitleSyncOutcome::Changed {
            from: "Zelda".into(),
            to: "Legend of Zelda".into()
        }
    );
    // Only the title (and RomM's own sort key) changed: same id, file, ids,
    // cover, summary, identification.
    let after = romm.rom();
    for (field, value) in before.as_object().unwrap() {
        if field != "name" && field != "name_sort_key" {
            assert_eq!(&after[field], value, "{field}");
        }
    }
    // Recorded beside the History entry.
    let record = load_record(&setup.records, "tx-1").unwrap().unwrap();
    assert_eq!(record.original_title, "Zelda");
    assert_eq!(record.written_title, "Legend of Zelda");
    assert_eq!(record.rom_id, "4");

    // History Undo (the file is back) restores the original title.
    std::fs::rename(&paths.1, &paths.0).unwrap();
    let restored = undo(&setup, &romm, "tx-1").unwrap().unwrap();
    assert_eq!(
        restored,
        TitleSyncOutcome::Changed {
            from: "Legend of Zelda".into(),
            to: "Zelda".into()
        }
    );
    assert_eq!(romm.rom()["name"], "Zelda");
    assert!(load_record(&setup.records, "tx-1").unwrap().is_none());
    // A second Undo has nothing to do.
    assert_eq!(undo(&setup, &romm, "tx-1").unwrap(), None);
}

#[test]
fn repeated_renames_undo_newest_first() {
    let setup = setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    let first = rename(&setup, "Zelda (USA).gba", "Zelda B (USA).gba");
    let cache = cache_of(&setup, &[romm.rom()]);
    update(&setup, &romm, &cache, "tx-a", &first).unwrap();
    // RomM scanned the rename and EmuWiz imported again.
    romm.edit("fs_name", json!("Zelda B (USA).gba"));
    romm.edit("fs_name_no_tags", json!("Zelda B"));
    romm.edit("full_path", json!("roms/gba/Zelda B (USA).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    let second = rename(&setup, "Zelda B (USA).gba", "Zelda C (USA).gba");
    update(&setup, &romm, &cache, "tx-b", &second).unwrap();
    assert_eq!(romm.rom()["name"], "Zelda C");

    assert!(matches!(
        undo(&setup, &romm, "tx-b"),
        Ok(Some(TitleSyncOutcome::Changed { .. }))
    ));
    assert_eq!(romm.rom()["name"], "Zelda B");
    assert!(matches!(
        undo(&setup, &romm, "tx-a"),
        Ok(Some(TitleSyncOutcome::Changed { .. }))
    ));
    assert_eq!(romm.rom()["name"], "Zelda");
}

#[test]
fn undo_never_overwrites_a_later_edit_in_romm() {
    let setup = setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    let paths = rename(&setup, "Zelda (USA).gba", "Zelda Classic (USA).gba");
    update(&setup, &romm, &cache, "tx-1", &paths).unwrap();
    romm.edit("name", json!("My Zelda"));
    let outcome = undo(&setup, &romm, "tx-1").unwrap().unwrap();
    assert!(
        matches!(&outcome, TitleSyncOutcome::LeftAlone(reason) if reason.contains("changed in RomM after the rename")),
        "{outcome:?}"
    );
    assert_eq!(romm.rom()["name"], "My Zelda");
    assert_eq!(romm.puts(), vec!["Zelda Classic".to_string()]);
}

#[test]
fn titles_that_are_not_romms_own_are_left_alone() {
    // Set by hand before the rename.
    let setup = setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    romm.edit("name", json!("My Favourite"));
    let cache = cache_of(&setup, &[romm.rom()]);
    let paths = rename(&setup, "Zelda (USA).gba", "Zelda 2 (USA).gba");
    left_alone(
        update(&setup, &romm, &cache, "tx", &paths),
        "may be a custom title",
    );

    // Identified by RomM.
    let setup = self::setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    romm.edit("is_identified", json!(true));
    romm.edit("is_unidentified", json!(false));
    let cache = cache_of(&setup, &[romm.rom()]);
    let paths = rename(&setup, "Zelda (USA).gba", "Zelda 2 (USA).gba");
    left_alone(update(&setup, &romm, &cache, "tx", &paths), "identified");

    // A name RomM's filename cleaning would change: saving would rename it.
    let setup = self::setup();
    let romm = FakeRomm::holding(romm_entry("Zelda + Link (USA).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    let paths = rename(&setup, "Zelda + Link (USA).gba", "Zelda 2 (USA).gba");
    left_alone(
        update(&setup, &romm, &cache, "tx", &paths),
        "rename this file",
    );

    // Never sent, never recorded.
    assert!(romm.puts().is_empty());
    assert!(load_record(&setup.records, "tx").unwrap().is_none());
}

#[test]
fn only_the_entry_linked_to_this_exact_file_is_changed() {
    // A similarly named entry is not this game.
    let setup = setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (Europe).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    let paths = rename(&setup, "Zelda (USA).gba", "Zelda 2 (USA).gba");
    left_alone(
        update(&setup, &romm, &cache, "tx", &paths),
        "no entry for this file",
    );

    // The linked entry was removed from RomM.
    let setup = self::setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    *romm.rom.lock().unwrap() = None;
    let paths = rename(&setup, "Zelda (USA).gba", "Zelda 2 (USA).gba");
    left_alone(update(&setup, &romm, &cache, "tx", &paths), "no longer has");

    // RomM's entry now points elsewhere, or the file differs.
    let setup = self::setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    romm.edit("full_path", json!("roms/gba/Other (USA).gba"));
    let paths = rename(&setup, "Zelda (USA).gba", "Zelda 2 (USA).gba");
    left_alone(
        update(&setup, &romm, &cache, "tx", &paths),
        "different file",
    );
    romm.edit("full_path", json!("roms/gba/Zelda (USA).gba"));
    romm.edit(
        "sha1_hash",
        json!("0000000000000000000000000000000000000000"),
    );
    left_alone(update(&setup, &romm, &cache, "tx", &paths), "hash");
    romm.edit(
        "sha1_hash",
        json!("ae0d6b471f819f4993edbf261664962afbaa579a"),
    );
    romm.edit("fs_size_bytes", json!(999));
    left_alone(update(&setup, &romm, &cache, "tx", &paths), "size");
    romm.edit("fs_size_bytes", json!(12));

    // The local file replaced since the import by other content of the same
    // size: RomM and the import still agree, but neither is this file.
    std::fs::write(&paths.1, b"other gamexx").unwrap();
    left_alone(
        update(&setup, &romm, &cache, "tx", &paths),
        "content is not what RomM's entry has",
    );
    // With the right content again, but RomM without hashes: cannot confirm.
    std::fs::write(&paths.1, b"GBA rom data").unwrap();
    romm.edit("sha1_hash", json!(null));
    left_alone(
        update(&setup, &romm, &cache, "tx", &paths),
        "no SHA-1 or MD5",
    );
    // MD5 alone is enough when it matches.
    romm.edit("md5_hash", json!("35d3cfa9d3ba1277294dd19912d09bcc"));
    assert!(matches!(
        update(&setup, &romm, &cache, "tx", &paths),
        Ok(TitleSyncOutcome::Changed { .. })
    ));
    assert!(romm.puts().len() == 1);

    // Two imported entries for the same file: cannot tell which.
    let setup = self::setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    let mut twin = romm.rom();
    twin["id"] = json!(5);
    let cache = cache_of(&setup, &[romm.rom(), twin]);
    let paths = rename(&setup, "Zelda (USA).gba", "Zelda 2 (USA).gba");
    left_alone(update(&setup, &romm, &cache, "tx", &paths), "more than one");
    assert!(romm.puts().is_empty());
}

#[test]
fn romm_offline_or_refusing_is_reported_and_undo_then_does_nothing() {
    let setup = setup();
    let mut romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    let paths = rename(&setup, "Zelda (USA).gba", "Zelda 2 (USA).gba");
    romm.offline = true;
    let error = update(&setup, &romm, &cache, "tx-off", &paths).unwrap_err();
    assert!(error.contains("could not be read"), "{error}");

    romm.offline = false;
    romm.put_status = Some(403);
    let error = update(&setup, &romm, &cache, "tx-403", &paths).unwrap_err();
    assert!(error.contains("roms.write"), "{error}");
    romm.put_status = Some(500);
    let error = update(&setup, &romm, &cache, "tx-500", &paths).unwrap_err();
    assert!(error.contains("HTTP 500"), "{error}");
    assert_eq!(romm.rom()["name"], "Zelda");

    // The refused write was recorded first, but Undo sees RomM never showed
    // the written title and changes nothing.
    romm.put_status = None;
    let outcome = undo(&setup, &romm, "tx-403").unwrap().unwrap();
    assert!(
        matches!(outcome, TitleSyncOutcome::LeftAlone(_)),
        "{outcome:?}"
    );
    assert_eq!(romm.rom()["name"], "Zelda");
    assert!(romm.puts().is_empty());
    // Undo while RomM is offline reports it.
    romm.offline = true;
    assert!(
        undo(&setup, &romm, "tx-403")
            .unwrap_err()
            .contains("could not be read")
    );
}

#[test]
fn a_record_for_another_server_or_a_bad_id_is_never_used() {
    let setup = setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    std::fs::create_dir_all(&setup.records).unwrap();
    let record = TitleSyncRecord {
        transaction_id: "tx".into(),
        server_id: "http://elsewhere:8080".into(),
        rom_id: "4".into(),
        original_title: "Zelda".into(),
        written_title: "Zelda".into(),
    };
    std::fs::write(
        setup.records.join("tx.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let outcome = undo(&setup, &romm, "tx").unwrap().unwrap();
    assert!(
        matches!(outcome, TitleSyncOutcome::LeftAlone(reason) if reason.contains("different RomM server"))
    );
    assert_eq!(undo(&setup, &romm, "../escape").unwrap(), None);
    assert!(romm.puts().is_empty());
    let _ = Path::new("");
}

fn update_with_history(
    setup: &Setup,
    romm: &FakeRomm,
    cache: &IdentityCache,
    paths: &(PathBuf, PathBuf),
    earlier: Result<&[PathBuf], &str>,
) -> Result<TitleSyncOutcome, String> {
    update_title_after_rename(
        &setup.source,
        romm,
        cache,
        &setup.records,
        "tx-new",
        &paths.0,
        &paths.1,
        earlier,
        &AtomicBool::new(false),
    )
}

/// Zelda (USA) was renamed to Zelda Classic (USA) by the earlier Rename:
/// RomM scanned the new file but kept the title "Zelda", and EmuWiz's RomM
/// import still lists the first name. Now it is renamed again.
fn renamed_before_title_updates() -> (Setup, FakeRomm, IdentityCache, PathBuf, (PathBuf, PathBuf)) {
    let setup = setup();
    let romm = FakeRomm::holding(romm_entry("Zelda (USA).gba"));
    let cache = cache_of(&setup, &[romm.rom()]);
    let first = rename(&setup, "Zelda (USA).gba", "Zelda Classic (USA).gba");
    romm.edit("fs_name", json!("Zelda Classic (USA).gba"));
    romm.edit("fs_name_no_tags", json!("Zelda Classic"));
    romm.edit("full_path", json!("roms/gba/Zelda Classic (USA).gba"));
    let second = rename(
        &setup,
        "Zelda Classic (USA).gba",
        "Legend of Zelda (USA).gba",
    );
    (setup, romm, cache, first.0, second)
}

#[test]
fn a_game_renamed_before_title_updates_gets_its_title_on_the_next_rename() {
    let (setup, romm, cache, first_name, second) = renamed_before_title_updates();
    // Without History the import's entry cannot be tied to this file.
    left_alone(
        update_with_history(&setup, &romm, &cache, &second, Ok(&[])),
        "no entry for this file",
    );
    // With History proving the earlier name: the same entry, its title from
    // that earlier name, so it is synchronised.
    let earlier = [first_name];
    let outcome = update_with_history(&setup, &romm, &cache, &second, Ok(&earlier)).unwrap();
    assert_eq!(
        outcome,
        TitleSyncOutcome::Changed {
            from: "Zelda".into(),
            to: "Legend of Zelda".into()
        }
    );
    // Undo restores what RomM showed before this rename.
    let restored = undo(&setup, &romm, "tx-new").unwrap().unwrap();
    assert!(matches!(restored, TitleSyncOutcome::Changed { to, .. } if to == "Zelda"));
    assert_eq!(romm.rom()["name"], "Zelda");
}

#[test]
fn a_custom_title_is_kept_even_with_the_full_history() {
    let (setup, romm, cache, first_name, second) = renamed_before_title_updates();
    romm.edit("name", json!("Hyrule Adventure"));
    let earlier = [first_name];
    let outcome = update_with_history(&setup, &romm, &cache, &second, Ok(&earlier));
    left_alone(outcome, "may be a custom title");
    assert!(romm.puts().is_empty());
}

#[test]
fn an_uncertain_history_leaves_the_title_and_says_why() {
    let (setup, romm, _, _, second) = renamed_before_title_updates();
    // EmuWiz imported again after RomM's scan, so the entry is linked to the
    // current file; only the title's origin depends on History.
    let cache = cache_of(&setup, &[romm.rom()]);
    let outcome = update_with_history(
        &setup,
        &romm,
        &cache,
        &second,
        Err("Rename History has more than one rename that produced it"),
    );
    left_alone(outcome, "could not be safely traced");
    assert!(romm.puts().is_empty());
    assert_eq!(romm.rom()["name"], "Zelda");
}

#[test]
fn title_updates_are_off_unless_turned_on() {
    use crate::identity_source::settings::ProviderSettings;
    // Settings written before the option existed stay read-only.
    let older: ProviderSettings = serde_json::from_str(
        r#"{"enabled": true, "url": "http://172.19.0.20:8080", "mappings": [], "token_path": null}"#,
    )
    .unwrap();
    assert!(!older.rename_updates_title);
    assert!(!ProviderSettings::default().rename_updates_title);
    // Off is not even written; on is.
    assert!(
        !serde_json::to_string(&older)
            .unwrap()
            .contains("rename_updates_title")
    );
    let on = ProviderSettings {
        rename_updates_title: true,
        ..older
    };
    let saved = serde_json::to_string(&on).unwrap();
    assert!(saved.contains("\"rename_updates_title\":true"), "{saved}");
    let read: ProviderSettings = serde_json::from_str(&saved).unwrap();
    assert!(read.rename_updates_title);
}

#[test]
fn title_updates_use_their_own_token_and_never_the_read_only_one() {
    use crate::identity_source::settings::{ProviderSettings, title_token_file};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let write_token = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    };
    let read_only = write_token("read-token", "read-only-token-for-tests");
    let title = write_token("title-token", "title-token-for-tests");
    let mut settings = ProviderSettings {
        rename_updates_title: true,
        ..ProviderSettings::default()
    };
    settings.source.token_path = Some(read_only.clone());

    // Not set: refused, and the read-only token is not used instead.
    let refused = title_token_file(&settings).unwrap_err();
    assert!(
        refused.contains("no title-update token is set"),
        "{refused}"
    );
    assert!(
        refused.contains("read-only token is never used"),
        "{refused}"
    );

    // The read-only token's file, directly or through another spelling or a
    // link: refused.
    for same in [
        read_only.clone(),
        dir.path().join(".").join("read-token"),
        {
            let link = dir.path().join("link-to-read-token");
            std::os::unix::fs::symlink(&read_only, &link).unwrap();
            link
        },
    ] {
        settings.title_token_path = Some(same.clone());
        let refused = title_token_file(&settings).unwrap_err();
        assert!(
            refused.contains("same file as EmuWiz's read-only RomM token"),
            "{}: {refused}",
            same.display()
        );
    }

    // Its own file: loaded with the usual checks.
    settings.title_token_path = Some(title.clone());
    let token = title_token_file(&settings).expect("the title-update token");
    assert_eq!(
        token.with_header_value(|value| value.to_string()),
        "Bearer title-token-for-tests"
    );
    std::fs::set_permissions(&title, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        title_token_file(&settings).is_err(),
        "a readable-by-others file is refused"
    );

    // Saved beside the read-only path, never in place of it; absent when unset.
    let saved = serde_json::to_string(&settings).unwrap();
    assert!(saved.contains("\"title_token_path\""), "{saved}");
    assert!(saved.contains("read-token"), "{saved}");
    assert!(
        !serde_json::to_string(&ProviderSettings::default())
            .unwrap()
            .contains("title_token_path")
    );
}

/// End to end against a disposable RomM sandbox, through EmuWiz's real HTTP
/// transport. Run by hand only:
/// `EMUWIZ_ROMM_SANDBOX_URL=http://<private address>:8080
///  EMUWIZ_ROMM_SANDBOX_RW_TOKEN=<token file with roms.read, roms.write, platforms.read>
///  EMUWIZ_ROMM_SANDBOX_RO_TOKEN=<token file with roms.read, platforms.read>
///  EMUWIZ_ROMM_SANDBOX_GAMES=<local folder RomM serves as roms/gba>
///  EMUWIZ_ROMM_SANDBOX_ROM_ID=<id of an EmuWiz synthetic fixture entry>
///  cargo test -- --ignored against_the_romm_sandbox`
/// It only touches an entry whose file name starts with "EmuWiz", and puts
/// the file and the title back at the end.
#[test]
#[ignore = "needs the disposable RomM sandbox"]
fn against_the_romm_sandbox() {
    use crate::identity_source::net_policy::SystemResolver;
    use crate::identity_source::romm::client::{RommClient, UreqTransport};
    use crate::identity_source::settings::{ProviderSettings, load_token_file, title_token_file};
    let env = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    let url = env("EMUWIZ_ROMM_SANDBOX_URL");
    let games = PathBuf::from(env("EMUWIZ_ROMM_SANDBOX_GAMES"));
    let id = env("EMUWIZ_ROMM_SANDBOX_ROM_ID");
    let source_with = |url: &str, token_file: &str| {
        let config = RommSourceConfig {
            enabled: true,
            url: url.to_string(),
            mappings: vec![PathMapping {
                provider_prefix: "roms/gba".into(),
                archivefs_prefix: games.clone(),
                provider_aliases: Vec::new(),
            }],
            media_mapping: None,
            provider_path_kind: ProviderPathKind::ProviderRelative,
            token_path: None,
        };
        // The write token only ever comes through the title-update setting,
        // beside a separate read-only token, as Blue loads it.
        let token = if token_file == "EMUWIZ_ROMM_SANDBOX_RW_TOKEN" {
            let mut settings = ProviderSettings::default();
            settings.source.token_path = Some(env("EMUWIZ_ROMM_SANDBOX_RO_TOKEN").into());
            settings.title_token_path = Some(env(token_file).into());
            title_token_file(&settings).expect("the separate title-update token")
        } else {
            load_token_file(Some(Path::new(&env(token_file)))).expect("token file")
        };
        ValidatedRommSource::validate(
            &config,
            &token,
            std::slice::from_ref(&games),
            &SystemResolver,
        )
        .expect("sandbox source validates")
    };
    let source = source_with(&url, "EMUWIZ_ROMM_SANDBOX_RW_TOKEN");
    let transport = UreqTransport::new();
    let cancel = AtomicBool::new(false);
    let client = RommClient::new(&source, &transport);
    let live = |client: &RommClient<'_, UreqTransport>| client.rom(&id, None).expect("RomM reads");
    let records = tempfile::tempdir().unwrap();
    let records = records.path();

    // EmuWiz's import of this one entry, as the importer records it.
    let before = live(&client);
    let fs_name = before["fs_name"].as_str().unwrap().to_string();
    assert!(
        fs_name.starts_with("EmuWiz"),
        "only a synthetic fixture: {fs_name}"
    );
    let setup_like = Setup {
        _temp: tempfile::tempdir().unwrap(),
        games: games.clone(),
        records: records.to_path_buf(),
        source: source.clone(),
    };
    let cache = cache_of(&setup_like, std::slice::from_ref(&before));
    let original_title = before["name"].as_str().unwrap().to_string();
    assert_eq!(
        original_title,
        romm_derived_title(&fs_name),
        "start from RomM's own title"
    );
    let file0 = games.join(&fs_name);
    let file1 = games.join("EmuWiz Title Sync (Europe).gba");
    let file2 = games.join("EmuWiz Title Sync Two (USA).gba");
    let manual = |title: &str| {
        // A person editing the title in RomM.
        let boundary = "emuwiz-sandbox-manual-edit";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{title}\r\n--{boundary}--\r\n"
        );
        let url = source
            .endpoint()
            .url_for(&format!("/api/roms/{id}"))
            .unwrap();
        let status = source.token().with_header_value(|header| {
            transport
                .put_form(
                    &url,
                    header,
                    &format!("multipart/form-data; boundary={boundary}"),
                    body.as_bytes(),
                    1 << 20,
                    Duration::from_secs(30),
                )
                .unwrap()
                .status
        });
        assert_eq!(status, 200);
    };
    let unchanged = |after: &Value| {
        for field in [
            "id",
            "fs_name",
            "fs_path",
            "igdb_id",
            "ss_id",
            "moby_id",
            "is_identified",
            "summary",
            "url_cover",
            "has_notes",
        ] {
            assert_eq!(after[field], before[field], "{field} kept");
        }
        let cover = |rom: &Value| {
            rom["path_cover_large"]
                .as_str()
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(cover(after), cover(&before), "same cover file");
    };

    // 1. Rename → RomM title follows, nothing else changes.
    std::fs::rename(&file0, &file1).unwrap();
    let outcome = update_title_after_rename(
        &source,
        &transport,
        &cache,
        records,
        "sbx-1",
        &file0,
        &file1,
        Ok(&[]),
        &cancel,
    )
    .unwrap();
    println!("rename 1: {}", outcome.line());
    assert_eq!(
        outcome,
        TitleSyncOutcome::Changed {
            from: original_title.clone(),
            to: "EmuWiz Title Sync".into()
        }
    );
    let after = live(&client);
    assert_eq!(after["name"], "EmuWiz Title Sync");
    unchanged(&after);

    // 2. A repeated rename (RomM has not scanned yet; History proves file0).
    std::fs::rename(&file1, &file2).unwrap();
    let earlier = [file0.clone()];
    let outcome = update_title_after_rename(
        &source,
        &transport,
        &cache,
        records,
        "sbx-2",
        &file1,
        &file2,
        Ok(&earlier),
        &cancel,
    )
    .unwrap();
    println!("rename 2: {}", outcome.line());
    assert_eq!(live(&client)["name"], "EmuWiz Title Sync Two");

    // 3. Undo the newest: back to the first rename's title.
    std::fs::rename(&file2, &file1).unwrap();
    let outcome = restore_title_after_undo(&source, &transport, records, "sbx-2", &cancel)
        .unwrap()
        .unwrap();
    println!("undo 2: {}", outcome.line());
    assert_eq!(live(&client)["name"], "EmuWiz Title Sync");

    // 4. A manual edit in RomM, then Undo: the edit is kept.
    manual("Sandbox Custom Title");
    let outcome = restore_title_after_undo(&source, &transport, records, "sbx-1", &cancel)
        .unwrap()
        .unwrap();
    println!("undo 1 after a manual edit: {}", outcome.line());
    assert!(matches!(outcome, TitleSyncOutcome::LeftAlone(_)));
    assert_eq!(live(&client)["name"], "Sandbox Custom Title");
    // Edited back to what EmuWiz wrote: Undo restores the original.
    manual("EmuWiz Title Sync");
    std::fs::rename(&file1, &file0).unwrap();
    let outcome = restore_title_after_undo(&source, &transport, records, "sbx-1", &cancel)
        .unwrap()
        .unwrap();
    println!("undo 1: {}", outcome.line());
    assert_eq!(live(&client)["name"], original_title.as_str());
    unchanged(&live(&client));

    // 5. A token without roms.write: refused by RomM, reported, title kept.
    let read_only = source_with(&url, "EMUWIZ_ROMM_SANDBOX_RO_TOKEN");
    std::fs::rename(&file0, &file1).unwrap();
    let error = update_title_after_rename(
        &read_only,
        &transport,
        &cache,
        records,
        "sbx-ro",
        &file0,
        &file1,
        Ok(&[]),
        &cancel,
    )
    .unwrap_err();
    println!("read-only token: {error}");
    assert!(error.contains("roms.write"), "{error}");
    assert_eq!(live(&client)["name"], original_title.as_str());

    // 6. RomM unreachable (a closed port on the same host).
    let host = url
        .trim_end_matches('/')
        .rsplit_once(':')
        .unwrap()
        .0
        .to_string();
    let offline = source_with(&format!("{host}:9"), "EMUWIZ_ROMM_SANDBOX_RW_TOKEN");
    // The import came from that same (now unreachable) server, so EmuWiz
    // really tries to reach it.
    let offline_setup = Setup {
        _temp: tempfile::tempdir().unwrap(),
        games: games.clone(),
        records: records.to_path_buf(),
        source: offline.clone(),
    };
    let offline_cache = cache_of(&offline_setup, std::slice::from_ref(&before));
    let error = update_title_after_rename(
        &offline,
        &transport,
        &offline_cache,
        records,
        "sbx-off",
        &file0,
        &file1,
        Ok(&[]),
        &cancel,
    )
    .unwrap_err();
    println!("offline: {error}");
    assert!(error.contains("could not be read"), "{error}");

    // 7. The imported entry no longer exists in RomM.
    let mut gone = before.clone();
    gone["id"] = json!(999_999_999u64);
    let gone_cache = cache_of(&setup_like, &[gone]);
    let outcome = update_title_after_rename(
        &source,
        &transport,
        &gone_cache,
        records,
        "sbx-gone",
        &file0,
        &file1,
        Ok(&[]),
        &cancel,
    )
    .unwrap();
    println!("missing entry: {}", outcome.line());
    assert!(matches!(outcome, TitleSyncOutcome::LeftAlone(_)));

    std::fs::rename(&file1, &file0).unwrap();
    assert_eq!(live(&client)["name"], original_title.as_str());
}
